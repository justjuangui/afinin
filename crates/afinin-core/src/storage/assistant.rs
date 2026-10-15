// SPDX-License-Identifier: MIT OR Apache-2.0

//! Saved assistant conversations: one row per answered question, with what
//! the answer was based on and the user's rating.

use duckdb::{OptionalExt, params};
use serde::{Deserialize, Serialize};

use super::Storage;
use crate::error::{Error, Result};
use crate::llm::agent::Turn;
use crate::llm::context::ScreenContext;
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
    /// What the user was looking at when asking, replayed by re-checks.
    pub screen: Option<ScreenContext>,
    pub turn: Turn,
    pub rating: Option<Rating>,
    pub rating_note: Option<String>,
    /// Kept as a reference answer to re-check against current data.
    pub reference: bool,
    /// Values a re-check's answer must state (figures or text).
    pub expected: Vec<String>,
    pub created_at: String,
}

/// Expected values per reference answer, and characters per value.
pub const MAX_EXPECTED: usize = 10;
pub const MAX_EXPECTED_CHARS: usize = 200;

fn to_json<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Other(format!("cannot store assistant turn: {e}")))
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
        screen: Option<&ScreenContext>,
        turn: &Turn,
        latency_ms: u64,
    ) -> Result<i64> {
        let now = now_rfc3339();
        let document = to_json(turn)?;
        let screen = screen.map(to_json).transpose()?;
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
            "INSERT INTO assistant_turns (conversation_id, workspace_id, question, screen, turn, latency_ms, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
            params![
                turn.conversation_id,
                workspace_id,
                question,
                screen,
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

    /// Saved turns matching `filter` (a SQL condition with one `?` for `arg`).
    fn query_turns(&self, filter: &str, arg: &dyn duckdb::ToSql) -> Result<Vec<StoredTurn>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, question, screen, turn, rating, rating_note, is_reference, created_at, expected
             FROM assistant_turns WHERE {filter} ORDER BY id"
        ))?;
        let rows = stmt.query_map([arg], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<bool>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (id, question, screen, doc, rating, rating_note, is_reference, created_at, expected) =
                row?;
            let invalid = |e: serde_json::Error| {
                Error::Other(format!("stored assistant turn {id} is invalid: {e}"))
            };
            Ok(StoredTurn {
                id,
                question,
                screen: screen
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(invalid)?,
                turn: serde_json::from_str(&doc).map_err(invalid)?,
                rating: rating.as_deref().and_then(Rating::parse),
                rating_note,
                reference: is_reference.unwrap_or(false),
                expected: expected
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(invalid)?
                    .unwrap_or_default(),
                created_at,
            })
        })
        .collect()
    }

    /// The saved turns of a conversation, oldest first.
    pub fn assistant_turns(&self, conversation_id: &str) -> Result<Vec<StoredTurn>> {
        self.query_turns("conversation_id = ?", &conversation_id)
    }

    pub fn assistant_turn(&self, turn_id: i64) -> Result<StoredTurn> {
        self.query_turns("id = ?", &turn_id)?
            .pop()
            .ok_or_else(|| Error::NotFound(format!("assistant turn {turn_id}")))
    }

    /// Answers that reported missing data, newest first.
    pub fn assistant_gaps(&self, workspace_id: &str, limit: usize) -> Result<Vec<StoredTurn>> {
        let mut found: Vec<StoredTurn> = self
            .query_turns(
                // Turns are stored as compact JSON; an empty list is `"missing_data":[]`.
                r#"workspace_id = ? AND turn LIKE '%"missing_data":[{%'"#,
                &workspace_id,
            )?
            .into_iter()
            .rev()
            .collect();
        found.truncate(limit);
        Ok(found)
    }

    /// Answers kept as references for re-checks, oldest first.
    pub fn reference_turns(&self, workspace_id: &str) -> Result<Vec<StoredTurn>> {
        self.query_turns("is_reference AND workspace_id = ?", &workspace_id)
    }

    pub fn set_reference_turn(&self, turn_id: i64, reference: bool) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE assistant_turns SET is_reference = ? WHERE id = ?",
            params![reference, turn_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("assistant turn {turn_id}")));
        }
        Ok(())
    }

    /// Replaces a turn's expected values: trimmed, without blanks or repeats,
    /// at most [`MAX_EXPECTED`] of at most [`MAX_EXPECTED_CHARS`] characters.
    pub fn set_expected(&self, turn_id: i64, expected: &[String]) -> Result<Vec<String>> {
        let mut clean: Vec<String> = Vec::new();
        for e in expected {
            let e = e.trim();
            if e.is_empty() || clean.iter().any(|c| c.eq_ignore_ascii_case(e)) {
                continue;
            }
            if e.chars().count() > MAX_EXPECTED_CHARS {
                return Err(Error::Other(format!(
                    "an expected value can be at most {MAX_EXPECTED_CHARS} characters"
                )));
            }
            clean.push(e.to_owned());
        }
        if clean.len() > MAX_EXPECTED {
            return Err(Error::Other(format!(
                "a reference answer can have at most {MAX_EXPECTED} expected values"
            )));
        }
        let n = self.conn.execute(
            "UPDATE assistant_turns SET expected = ? WHERE id = ?",
            params![to_json(&clean)?, turn_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("assistant turn {turn_id}")));
        }
        Ok(clean)
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
            missing_data: Vec::new(),
            reference_examples: 0,
            ..Default::default()
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
            .save_assistant_turn(&ws.id, &long, None, &turn("c1", "Because EC2."), 1200)
            .unwrap();
        db.save_assistant_turn(&ws.id, "And in August?", None, &turn("c1", "Less."), 800)
            .unwrap();
        db.save_assistant_turn(
            &ws.id,
            "Top findings?",
            Some(&ScreenContext {
                view: "Findings".into(),
                details: Vec::new(),
            }),
            &turn("c2", "Two."),
            500,
        )
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

        // References: marked turns, with the screen they were asked from.
        let c2 = db.assistant_turns("c2").unwrap()[0].id;
        assert!(db.reference_turns(&ws.id).unwrap().is_empty());
        db.set_reference_turn(c2, true).unwrap();
        let refs = db.reference_turns(&ws.id).unwrap();
        assert_eq!(refs.len(), 1);
        assert!(refs[0].reference);
        assert_eq!(
            refs[0].screen.as_ref().map(|s| s.view.as_str()),
            Some("Findings")
        );
        assert_eq!(db.assistant_turn(c2).unwrap().question, "Top findings?");

        // Expected values: trimmed, no blanks or repeats, limited.
        assert!(refs[0].expected.is_empty());
        let stored = db
            .set_expected(c2, &[" $1,234.56 ".into(), "".into(), "$1,234.56".into(), "EC2".into()])
            .unwrap();
        assert_eq!(stored, ["$1,234.56", "EC2"]);
        assert_eq!(db.assistant_turn(c2).unwrap().expected, stored);
        let too_many: Vec<String> = (0..=MAX_EXPECTED).map(|i| i.to_string()).collect();
        assert!(db.set_expected(c2, &too_many).is_err());
        assert!(db.set_expected(-1, &[]).is_err());

        // Answers that reported missing data are listed as gaps.
        assert!(db.assistant_gaps(&ws.id, 10).unwrap().is_empty());
        let mut gap = turn("c2", "I can't break this down per resource.");
        gap.missing_data = vec![crate::llm::agent::MissingData {
            need: crate::llm::agent::MissingNeed::Cur,
            detail: "Per-resource costs need CUR.".into(),
        }];
        db.save_assistant_turn(&ws.id, "Cost of i-123?", None, &gap, 300)
            .unwrap();
        let gaps = db.assistant_gaps(&ws.id, 10).unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].question, "Cost of i-123?");

        db.delete_assistant_conversation("c1").unwrap();
        assert!(db.assistant_turns("c1").unwrap().is_empty());
        db.delete_workspace(&ws.id).unwrap();
        assert!(db.assistant_conversations(&ws.id, 10).unwrap().is_empty());
        assert!(db.assistant_turns("c2").unwrap().is_empty());
    }
}
