// SPDX-License-Identifier: MIT OR Apache-2.0

use duckdb::params;
use serde::Serialize;

use super::Storage;
use crate::error::Result;
use crate::model::now_rfc3339;

/// One request to an LLM provider, as sent (FR-45).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LlmLogEntry {
    pub id: i64,
    pub conversation_id: String,
    pub provider: String,
    pub model: String,
    pub redacted: bool,
    pub request: String,
    pub response: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Round within its answer (1 = the question); None for older entries.
    pub round: Option<i64>,
    pub latency_ms: Option<i64>,
    pub created_at: String,
}

/// A request to log, as sent and received.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoggedRequest<'a> {
    pub workspace_id: &'a str,
    pub conversation_id: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub redacted: bool,
    pub request: &'a str,
    pub response: &'a str,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub round: usize,
    pub latency_ms: u64,
}

/// Assistant requests per provider and model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LlmUsage {
    pub provider: String,
    pub model: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Estimated from the model's price in Settings; None without a price.
    pub cost_usd: Option<f64>,
}

impl Storage {
    pub fn llm_usage(&self, since: &str) -> Result<Vec<LlmUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, model, count(*), sum(input_tokens), sum(output_tokens),
                    sum(coalesce(cache_read_tokens, 0)), sum(coalesce(cache_write_tokens, 0))
             FROM llm_log WHERE created_at >= ? GROUP BY ALL
             ORDER BY sum(input_tokens) + sum(output_tokens) DESC",
        )?;
        let rows = stmt.query_map(params![since], |r| {
            let sum = |i| r.get::<_, Option<i64>>(i).map(|v| v.unwrap_or(0));
            Ok(LlmUsage {
                provider: r.get(0)?,
                model: r.get(1)?,
                requests: r.get(2)?,
                input_tokens: sum(3)?,
                output_tokens: sum(4)?,
                cache_read_tokens: sum(5)?,
                cache_write_tokens: sum(6)?,
                cost_usd: None,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    /// Stores one request and returns its log entry id.
    pub(crate) fn log_llm_request(&self, r: &LoggedRequest<'_>) -> Result<i64> {
        Ok(self.conn.query_row(
            "INSERT INTO llm_log (workspace_id, conversation_id, provider, model, redacted, request, response,
                                  input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                                  turn_round, latency_ms, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            params![
                r.workspace_id,
                r.conversation_id,
                r.provider,
                r.model,
                r.redacted,
                r.request,
                r.response,
                r.input_tokens as i64,
                r.output_tokens as i64,
                r.cache_read_tokens as i64,
                r.cache_write_tokens as i64,
                r.round as i64,
                r.latency_ms as i64,
                now_rfc3339()
            ],
            |row| row.get(0),
        )?)
    }

    fn query_log(&self, filter: &str, args: &[&dyn duckdb::ToSql]) -> Result<Vec<LlmLogEntry>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, conversation_id, provider, model, redacted, request, response, input_tokens, output_tokens,
                    coalesce(cache_read_tokens, 0), coalesce(cache_write_tokens, 0), turn_round, latency_ms, created_at
             FROM llm_log WHERE {filter}"
        ))?;
        let rows = stmt.query_map(args, |r| {
            Ok(LlmLogEntry {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                provider: r.get(2)?,
                model: r.get(3)?,
                redacted: r.get(4)?,
                request: r.get(5)?,
                response: r.get(6)?,
                input_tokens: r.get(7)?,
                output_tokens: r.get(8)?,
                cache_read_tokens: r.get(9)?,
                cache_write_tokens: r.get(10)?,
                round: r.get(11)?,
                latency_ms: r.get(12)?,
                created_at: r.get(13)?,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    pub fn llm_log(&self, workspace_id: &str, limit: usize) -> Result<Vec<LlmLogEntry>> {
        self.query_log(
            "workspace_id = ? AND request <> '' ORDER BY id DESC LIMIT ?",
            &[&workspace_id, &(limit as i64)],
        )
    }

    /// One logged request of a workspace, for the trace of an answer.
    pub fn llm_log_entry(&self, workspace_id: &str, id: i64) -> Result<Option<LlmLogEntry>> {
        Ok(self
            .query_log("workspace_id = ? AND id = ?", &[&workspace_id, &id])?
            .pop())
    }

    /// Forgets what was sent and received but keeps the token counts, so
    /// monthly usage and the budget still add up.
    pub fn clear_llm_log(&self, workspace_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE llm_log SET request = '', response = '' WHERE workspace_id = ?",
            params![workspace_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str, tokens: (u64, u64, u64)) -> LoggedRequest<'_> {
        LoggedRequest {
            workspace_id: "w",
            conversation_id: "c",
            provider: "Anthropic",
            model,
            redacted: true,
            request: "{}",
            response: "{}",
            input_tokens: tokens.0,
            output_tokens: tokens.1,
            cache_read_tokens: tokens.2,
            cache_write_tokens: 0,
            round: 1,
            latency_ms: 900,
        }
    }

    #[test]
    fn usage_sums_requests_per_model_since_a_date() {
        let db = Storage::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for (model, tokens) in [("a", (100, 10, 1000)), ("a", (50, 5, 0)), ("b", (1, 1, 0))] {
            ids.push(db.log_llm_request(&request(model, tokens)).unwrap());
        }
        let usage = db.llm_usage("2000-01-01").unwrap();
        assert_eq!(usage.len(), 2);
        assert_eq!(
            (
                usage[0].model.as_str(),
                usage[0].requests,
                usage[0].input_tokens,
                usage[0].output_tokens,
                usage[0].cache_read_tokens
            ),
            ("a", 2, 150, 15, 1000)
        );
        assert!(db.llm_usage("2999-01-01").unwrap().is_empty());

        let first = db.llm_log_entry("w", ids[0]).unwrap().unwrap();
        assert_eq!((first.round, first.latency_ms), (Some(1), Some(900)));
        assert!(db.llm_log_entry("other", ids[0]).unwrap().is_none());

        // Clearing forgets the content, not the usage.
        db.clear_llm_log("w").unwrap();
        assert!(db.llm_log("w", 10).unwrap().is_empty());
        assert_eq!(db.llm_usage("2000-01-01").unwrap()[0].input_tokens, 150);
    }
}
