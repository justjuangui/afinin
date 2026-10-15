// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reference answers as examples: for a new question, the queries that
//! answered the most similar reference questions. Only the queries are
//! shown, never the old answers: their figures would be stale and the model
//! could repeat them.

use std::collections::HashSet;

use serde_json::Value;

use crate::storage::StoredTurn;

/// At most this many examples per question.
pub const MAX_EXAMPLES: usize = 3;
/// Word overlap (Jaccard) below which a reference isn't similar enough.
const MIN_SIMILARITY: f64 = 0.2;

/// Words that say little about which data a question needs.
const STOP: &[&str] = &[
    "a", "an", "and", "are", "by", "did", "do", "does", "for", "how", "i", "in", "is", "it", "me",
    "much", "my", "of", "on", "our", "the", "to", "us", "was", "we", "what", "which", "why",
    "with",
];

fn words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1 && !STOP.contains(w))
        .map(str::to_owned)
        .collect()
}

fn similarity(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        a.intersection(b).count() as f64 / union as f64
    }
}

/// The references most similar to `question` that ran at least one query.
pub fn select<'a>(question: &str, references: &'a [StoredTurn]) -> Vec<&'a StoredTurn> {
    let q = words(question);
    let mut scored: Vec<(f64, &StoredTurn)> = references
        .iter()
        .filter(|r| r.turn.citations.iter().any(|c| c.ok))
        .map(|r| (similarity(&q, &words(&r.question)), r))
        .filter(|(s, _)| *s >= MIN_SIMILARITY)
        .collect();
    // Most similar first; newer references break ties.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
    scored
        .into_iter()
        .take(MAX_EXAMPLES)
        .map(|(_, r)| r)
        .collect()
}

/// Examples for the prompt: each question with the tool calls that answered it.
pub fn render(examples: &[&StoredTurn]) -> String {
    examples
        .iter()
        .map(|r| {
            let asked = r.created_at.get(..10).unwrap_or(&r.created_at);
            let calls: Vec<String> = r
                .turn
                .citations
                .iter()
                .filter(|c| c.ok && c.duplicate_of.is_none())
                .map(|c| format!("- {} {}", c.tool, compact(&c.input)))
                .collect();
            format!(
                "Question (asked {asked}): {}\n{}",
                r.question,
                calls.join("\n")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StopReason;
    use crate::llm::agent::{Citation, Turn};
    use serde_json::json;

    fn reference(id: i64, question: &str, tool: &str) -> StoredTurn {
        StoredTurn {
            id,
            question: question.into(),
            screen: None,
            turn: Turn {
                conversation_id: "c".into(),
                answer: "EC2 rose by 120.5 [1].".into(),
                citations: vec![Citation {
                    n: 1,
                    tool: tool.into(),
                    input: json!({"group_by": "service", "start": "2026-09-01", "end": "2026-10-01"}),
                    result: json!({"total": 120.5}),
                    ok: true,
                    ..Default::default()
                }],
                stop: StopReason::EndTurn,
                model: "m".into(),
                redacted: true,
                masked_values: 0,
                input_tokens: 0,
                output_tokens: 0,
                assumptions: Vec::new(),
                context_revision: 0,
                unverified_figures: Vec::new(),
                turn_id: Some(id),
                data_as_of: None,
                missing_data: Vec::new(),
                reference_examples: 0,
                ..Default::default()
            },
            rating: None,
            rating_note: None,
            reference: true,
            expected: Vec::new(),
            created_at: "2026-10-01T10:00:00Z".into(),
        }
    }

    #[test]
    fn picks_similar_references_and_shows_queries_not_answers() {
        let refs = vec![
            reference(1, "Why did our bill go up last month?", "compare_periods"),
            reference(
                2,
                "What are the top savings opportunities?",
                "list_findings",
            ),
            reference(
                3,
                "Which services grew the most last month?",
                "compare_periods",
            ),
        ];
        let picked = select("Why did the bill go up in September?", &refs);
        assert_eq!(picked.iter().map(|r| r.id).collect::<Vec<_>>(), [1]);
        let text = render(&picked);
        assert!(
            text.starts_with("Question (asked 2026-10-01): Why did our bill go up last month?")
        );
        assert!(text.contains(r#"- compare_periods {"#), "{text}");
        assert!(
            !text.contains("120.5"),
            "old figures never reach the prompt"
        );
        assert!(select("Show me the Lambda functions", &refs).is_empty());
    }
}
