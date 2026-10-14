// SPDX-License-Identifier: MIT OR Apache-2.0

//! Re-checks of reference answers: the same question asked again against
//! current data and context, compared with the saved answer.

use serde::Serialize;
use serde_json::Value;

use super::agent::{Citation, Turn};
use super::tools::describe;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recheck {
    pub turn_id: i64,
    pub question: String,
    /// The saved reference answer.
    pub before: Turn,
    /// The answer now; not saved to the conversation history.
    pub after: Turn,
    /// Queries the reference answer ran that the new one did not.
    pub queries_removed: Vec<String>,
    /// Queries the new answer ran that the reference did not.
    pub queries_added: Vec<String>,
}

/// JSON with object keys sorted, so equal inputs compare equal whatever
/// order the model wrote them in.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&o[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn key(c: &Citation) -> String {
    format!("{}:{}", c.tool, canonical(&c.input))
}

/// Queries only in `before` and only in `after`, as readable labels.
pub fn compare_queries(before: &[Citation], after: &[Citation]) -> (Vec<String>, Vec<String>) {
    let mut unmatched: Vec<&Citation> = after.iter().collect();
    let mut removed = Vec::new();
    for b in before {
        match unmatched.iter().position(|a| key(a) == key(b)) {
            Some(i) => {
                unmatched.remove(i);
            }
            None => removed.push(describe(&b.tool, &b.input)),
        }
    }
    let added = unmatched
        .into_iter()
        .map(|a| describe(&a.tool, &a.input))
        .collect();
    (removed, added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cite(tool: &str, input: Value) -> Citation {
        Citation {
            n: 1,
            tool: tool.into(),
            input,
            result: Value::Null,
            ok: true,
        }
    }

    #[test]
    fn same_queries_in_any_key_order_match() {
        let before = [
            cite(
                "query_costs",
                json!({"group_by": "service", "start": "2026-09-01", "end": "2026-10-01"}),
            ),
            cite("list_findings", json!({})),
        ];
        let after = [
            cite("list_findings", json!({})),
            cite(
                "query_costs",
                json!({"end": "2026-10-01", "start": "2026-09-01", "group_by": "service"}),
            ),
            cite(
                "query_costs",
                json!({"group_by": "region", "start": "2026-09-01", "end": "2026-10-01"}),
            ),
        ];
        let (removed, added) = compare_queries(&before, &after);
        assert!(removed.is_empty());
        assert_eq!(
            added,
            ["Querying costs by region, 2026-09-01 to 2026-09-30"]
        );
        let (removed, added) = compare_queries(&after, &before);
        assert_eq!(
            removed,
            ["Querying costs by region, 2026-09-01 to 2026-09-30"]
        );
        assert!(added.is_empty());
    }
}
