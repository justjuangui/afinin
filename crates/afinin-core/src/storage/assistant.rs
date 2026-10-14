// SPDX-License-Identifier: MIT OR Apache-2.0

//! Saved assistant conversations: one row per answered question, with what
//! the answer was based on and the user's rating.

use duckdb::{OptionalExt, params};
use serde::{Deserialize, Serialize};

use super::Storage;
use crate::error::{Error, Result};
use crate::llm::agent::Turn;
use crate::model::now_rfc3339;

/// Longest title kept, taken from the first question.
const TITLE_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub turns: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Up,
    Down,
}

impl Rating {
    fn as_str(self) -> &'static str {
        match self {
            Rating::Up => "up",
            Rating::Down => "down",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "up" => Some(Rating::Up),
            "down" => Some(Rating::Down),
            _ => None,
        }
    }
}

/// One saved question and answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredTurn {
    pub id: i64,
    pub question: String,
    pub turn: Turn,
    pub rating: Option<Rating>,
    pub rating_note: Option<String>,
    pub created_at: String,
}

fn title(question: &str) -> String {
    let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.chars().count() <= TITLE_CHARS {
        q
    } else {
        format!("{}…", q.chars().take(TITLE_CHARS - 1).collect::<String>())
    }
}

impl Storage {
    /// Saves an answered question; creates the conversation on its first turn.
    pub(crate) fn save_assistant_turn(
        &self,
        workspace_id: &str,
        question: &str,
        turn: &Turn,
        latency_ms: u64,
    ) -> Result<i64> {
        let now = now_rfc3339();
        let document = serde_json::to_string(turn)
            .map_err(|e| Error::Other(format!("cannot store assistant turn: {e}")))?;
        let tx = self.conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE assistant_conversations SET updated_at = ? WHERE id = ?",
            params![now, turn.conversation_id],
        )?;
        if updated == 0 {
            tx.execute(
                "INSERT INTO assistant_conversations (id, workspace_id, title, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
                params![turn.conversation_id, workspace_id, title(question), now, now],
            )?;
        }
        let id: i64 = tx.query_row(
            "INSERT INTO assistant_turns (conversation_id, workspace_id, question, turn, latency_ms, created_at)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
            params![
                turn.conversation_id,
                workspace_id,
                question,
                document,
                latency_ms as i64,
                now
            ],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Conversations of a workspace, most recent first.
    pub fn assistant_conversations(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<ConversationSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.created_at, c.updated_at,
                    (SELECT count(*) FROM assistant_turns t WHERE t.conversation_id = c.id)
             FROM assistant_conversations c WHERE c.workspace_id = ?
             ORDER BY c.updated_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![workspace_id, limit as i64], |r| {
            Ok(ConversationSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                turns: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    /// The saved turns of a conversation, oldest first.
    pub fn assistant_turns(&self, conversation_id: &str) -> Result<Vec<StoredTurn>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, question, turn, rating, rating_note, created_at
             FROM assistant_turns WHERE conversation_id = ? ORDER BY id",
        )?;
        let rows = stmt.query_map(params![conversation_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (id, question, doc, rating, rating_note, created_at) = row?;
            Ok(StoredTurn {
                id,
                question,
                turn: serde_json::from_str(&doc).map_err(|e| {
                    Error::Other(format!("stored assistant turn {id} is invalid: {e}"))
                })?,
                rating: rating.as_deref().and_then(Rating::parse),
                rating_note,
                created_at,
            })
        })
        .collect()
    }

    /// Which workspace a conversation belongs to, if it was saved.
    pub fn assistant_conversation_workspace(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT workspace_id FROM assistant_conversations WHERE id = ?",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Records (or with `None`, clears) the user's rating of an answer.
    pub fn rate_assistant_turn(
        &self,
        turn_id: i64,
        rating: Option<Rating>,
        note: Option<&str>,
    ) -> Result<()> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        let n = self.conn.execute(
            "UPDATE assistant_turns SET rating = ?, rating_note = ?, rated_at = ? WHERE id = ?",
            params![
                rating.map(Rating::as_str),
                rating.and(note),
                rating.map(|_| now_rfc3339()),
                turn_id
            ],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("assistant turn {turn_id}")));
        }
        Ok(())
    }

    pub fn delete_assistant_conversation(&self, conversation_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM assistant_turns WHERE conversation_id = ?",
            params![conversation_id],
        )?;
        tx.execute(
            "DELETE FROM assistant_conversations WHERE id = ?",
            params![conversation_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StopReason;

    fn turn(conversation_id: &str, answer: &str) -> Turn {
        Turn {
            conversation_id: conversation_id.into(),
            answer: answer.into(),
            citations: Vec::new(),
            stop: StopReason::EndTurn,
            model: "m".into(),
            redacted: true,
            masked_values: 0,
            input_tokens: 10,
            output_tokens: 5,
            assumptions: vec!["Period: 2026-09-01 to 2026-09-30".into()],
            context_revision: 2,
            unverified_figures: Vec::new(),
            turn_id: None,
            data_as_of: Some("2026-10-14T08:00:00Z".into()),
        }
    }

    #[test]
    fn saves_lists_rates_and_deletes_conversations() {
        let db = Storage::open_in_memory().unwrap();
        let ws = db.create_workspace("W").unwrap();
        let long = format!(
            "Why did {} spend go up?",
            "our very large production".repeat(5)
        );
        let first = db
            .save_assistant_turn(&ws.id, &long, &turn("c1", "Because EC2."), 1200)
            .unwrap();
        db.save_assistant_turn(&ws.id, "And in August?", &turn("c1", "Less."), 800)
            .unwrap();
        db.save_assistant_turn(&ws.id, "Top findings?", &turn("c2", "Two."), 500)
            .unwrap();

        let list = db.assistant_conversations(&ws.id, 10).unwrap();
        assert_eq!(list.len(), 2);
        let c1 = list.iter().find(|c| c.id == "c1").unwrap();
        assert_eq!(c1.turns, 2);
        assert!(c1.title.ends_with('…') && c1.title.chars().count() == TITLE_CHARS);
        assert_eq!(
            db.assistant_conversation_workspace("c1")
                .unwrap()
                .as_deref(),
            Some(ws.id.as_str())
        );

        db.rate_assistant_turn(first, Some(Rating::Down), Some("  wrong month  "))
            .unwrap();
        let turns = db.assistant_turns("c1").unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].question, long);
        assert_eq!(
            turns[0].turn.assumptions,
            ["Period: 2026-09-01 to 2026-09-30"]
        );
        assert_eq!(turns[0].rating, Some(Rating::Down));
        assert_eq!(turns[0].rating_note.as_deref(), Some("wrong month"));
        db.rate_assistant_turn(first, None, Some("ignored"))
            .unwrap();
        assert_eq!(db.assistant_turns("c1").unwrap()[0].rating_note, None);

        db.delete_assistant_conversation("c1").unwrap();
        assert!(db.assistant_turns("c1").unwrap().is_empty());
        db.delete_workspace(&ws.id).unwrap();
        assert!(db.assistant_conversations(&ws.id, 10).unwrap().is_empty());
        assert!(db.assistant_turns("c2").unwrap().is_empty());
    }
}
