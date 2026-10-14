// SPDX-License-Identifier: MIT OR Apache-2.0

use duckdb::{OptionalExt, params};
use serde::Serialize;

use super::Storage;
use crate::error::{Error, Result};
use crate::llm::context::{StoredContext, WorkspaceContext};
use crate::model::now_rfc3339;

/// One saved revision of a workspace context.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextRevision {
    pub revision: i64,
    pub created_at: String,
    pub context: WorkspaceContext,
}

fn parse(document: &str) -> Result<WorkspaceContext> {
    serde_json::from_str(document)
        .map_err(|e| Error::Other(format!("stored workspace context is invalid: {e}")))
}

impl Storage {
    pub fn workspace_context(&self, workspace_id: &str) -> Result<StoredContext> {
        let row: Option<(String, i64, String)> = self
            .conn
            .query_row(
                "SELECT document, revision, updated_at FROM workspace_context WHERE workspace_id = ?",
                params![workspace_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((doc, revision, updated_at)) => StoredContext {
                context: parse(&doc)?,
                revision,
                updated_at: Some(updated_at),
            },
            None => StoredContext::default(),
        })
    }

    /// Saves a new revision unless nothing changed. Every revision is kept in
    /// an append-only history.
    pub fn save_workspace_context(
        &self,
        workspace_id: &str,
        context: WorkspaceContext,
    ) -> Result<StoredContext> {
        let context = context.normalized()?;
        let current = self.workspace_context(workspace_id)?;
        if current.revision > 0 && current.context == context {
            return Ok(current);
        }
        let exists: bool = self.conn.query_row(
            "SELECT count(*) > 0 FROM workspaces WHERE id = ?",
            params![workspace_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Error::NotFound(format!("workspace {workspace_id}")));
        }
        let document = serde_json::to_string(&context)
            .map_err(|e| Error::Other(format!("cannot store workspace context: {e}")))?;
        let revision = current.revision + 1;
        let now = now_rfc3339();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM workspace_context WHERE workspace_id = ?",
            params![workspace_id],
        )?;
        tx.execute(
            "INSERT INTO workspace_context (workspace_id, document, revision, updated_at) VALUES (?, ?, ?, ?)",
            params![workspace_id, document, revision, now],
        )?;
        tx.execute(
            "INSERT INTO workspace_context_history (workspace_id, revision, document, created_at) VALUES (?, ?, ?, ?)",
            params![workspace_id, revision, document, now],
        )?;
        tx.commit()?;
        Ok(StoredContext {
            context,
            revision,
            updated_at: Some(now),
        })
    }

    /// Saved revisions, newest first.
    pub fn workspace_context_history(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<ContextRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT revision, created_at, document FROM workspace_context_history
             WHERE workspace_id = ? ORDER BY revision DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![workspace_id, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (revision, created_at, doc) = row?;
            Ok(ContextRevision {
                revision,
                created_at,
                context: parse(&doc)?,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn saves_revisions_and_skips_unchanged() {
        let db = Storage::open_in_memory().unwrap();
        let ws = db.create_workspace("W").unwrap();
        assert_eq!(db.workspace_context(&ws.id).unwrap().revision, 0);

        let ctx = WorkspaceContext {
            business: "Furniture store".into(),
            accounts: BTreeMap::from([("111111111111".into(), "prod".into())]),
            ..Default::default()
        };
        assert_eq!(
            db.save_workspace_context(&ws.id, ctx.clone())
                .unwrap()
                .revision,
            1
        );
        // Whitespace-only differences are not a new revision.
        let mut same = ctx.clone();
        same.business = " Furniture store ".into();
        assert_eq!(db.save_workspace_context(&ws.id, same).unwrap().revision, 1);

        let mut changed = ctx;
        changed.tags = vec![crate::llm::context::Entry {
            key: "team".into(),
            meaning: "owning squad".into(),
        }];
        let saved = db.save_workspace_context(&ws.id, changed.clone()).unwrap();
        assert_eq!(saved.revision, 2);
        assert_eq!(db.workspace_context(&ws.id).unwrap().context, changed);

        let history = db.workspace_context_history(&ws.id, 10).unwrap();
        assert_eq!(
            history.iter().map(|h| h.revision).collect::<Vec<_>>(),
            [2, 1]
        );

        db.delete_workspace(&ws.id).unwrap();
        assert!(db.workspace_context_history(&ws.id, 10).unwrap().is_empty());
        assert!(
            db.save_workspace_context(&ws.id, WorkspaceContext::default())
                .is_err()
        );
    }
}
