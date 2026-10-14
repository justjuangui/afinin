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

impl Storage {
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
