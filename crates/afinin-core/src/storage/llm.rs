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
    pub created_at: String,
}

/// Assistant requests per provider and model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LlmUsage {
    pub provider: String,
    pub model: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl Storage {
    pub fn llm_usage(&self, since: &str) -> Result<Vec<LlmUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, model, count(*), sum(input_tokens), sum(output_tokens) FROM llm_log
             WHERE created_at >= ? GROUP BY ALL ORDER BY sum(input_tokens) + sum(output_tokens) DESC",
        )?;
        let rows = stmt.query_map(params![since], |r| {
            Ok(LlmUsage {
                provider: r.get(0)?,
                model: r.get(1)?,
                requests: r.get(2)?,
                input_tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                output_tokens: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn log_llm_request(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        provider: &str,
        model: &str,
        redacted: bool,
        request: &str,
        response: &str,
        tokens: (u64, u64),
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO llm_log (workspace_id, conversation_id, provider, model, redacted, request, response,
                                  input_tokens, output_tokens, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                workspace_id,
                conversation_id,
                provider,
                model,
                redacted,
                request,
                response,
                tokens.0 as i64,
                tokens.1 as i64,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn llm_log(&self, workspace_id: &str, limit: usize) -> Result<Vec<LlmLogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, conversation_id, provider, model, redacted, request, response, input_tokens, output_tokens, created_at
             FROM llm_log WHERE workspace_id = ? ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![workspace_id, limit as i64], |r| {
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
                created_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    pub fn clear_llm_log(&self, workspace_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM llm_log WHERE workspace_id = ?",
            params![workspace_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_sums_requests_per_model_since_a_date() {
        let db = Storage::open_in_memory().unwrap();
        for (model, tokens) in [("a", (100, 10)), ("a", (50, 5)), ("b", (1, 1))] {
            db.log_llm_request("w", "c", "Anthropic", model, true, "{}", "{}", tokens)
                .unwrap();
        }
        let usage = db.llm_usage("2000-01-01").unwrap();
        assert_eq!(usage.len(), 2);
        assert_eq!(
            (
                usage[0].model.as_str(),
                usage[0].requests,
                usage[0].input_tokens,
                usage[0].output_tokens
            ),
            ("a", 2, 150, 15)
        );
        assert!(db.llm_usage("2999-01-01").unwrap().is_empty());
    }
}
