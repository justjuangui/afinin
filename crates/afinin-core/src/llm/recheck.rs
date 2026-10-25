// SPDX-License-Identifier: MIT OR Apache-2.0

//! Re-checks of reference answers: the same question asked again against
//! current data and context, compared with the saved answer.

use serde::Serialize;

use super::agent::{Citation, Turn};
use super::tools::describe;
use super::grounding::states_figure;
use super::trace::call_key;

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
    /// The reference's expected values, checked against the new answer.
    pub expectations: Vec<ExpectationResult>,
}

/// An expected value of a reference answer and whether the new answer has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectationResult {
    pub expected: String,
    pub met: bool,
}

/// Expected values: a figure must be stated (at any rounding, so `$1,234.56`
/// is met by "$1.23k"); other text must appear, ignoring case.
pub fn check_expectations(answer: &str, expected: &[String]) -> Vec<ExpectationResult> {
    let lower = answer.to_lowercase();
    expected
        .iter()
        .map(|e| ExpectationResult {
            expected: e.clone(),
            met: states_figure(answer, e).unwrap_or_else(|| lower.contains(&e.to_lowercase())),
        })
        .collect()
}

fn key(c: &Citation) -> String {
    call_key(&c.tool, &c.input)
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
    use serde_json::{Value, json};

    fn cite(tool: &str, input: Value) -> Citation {
        Citation {
            n: 1,
            tool: tool.into(),
            input,
            ok: true,
            ..Default::default()
        }
    }

    #[test]
    fn expected_figures_match_at_any_rounding_and_text_ignores_case() {
        let answer = "September spend was $1,234.56 [1], up 12.5% — mostly Amazon EC2. Forecast 1.2k.";
        let expected: Vec<String> = ["$1,234.56", "12.5%", "1,200", "amazon ec2", "$999.00", "Lambda"]
            .map(String::from)
            .into();
        let met: Vec<bool> = check_expectations(answer, &expected)
            .iter()
            .map(|r| r.met)
            .collect();
        assert_eq!(met, [true, true, true, true, false, false]);
        // Text around a figure is text: the words must appear too.
        assert!(!check_expectations("Lambda spend $500.", &["EC2 spend $500".into()])[0].met);
        assert!(check_expectations("EC2 spend $500.", &["ec2 spend $500".into()])[0].met);
        // Units: an amount is not a percentage, and the other way round.
        assert!(!check_expectations("Up 100% [1].", &["$100".into()])[0].met);
        assert!(!check_expectations("It cost $12.5.", &["12.5%".into()])[0].met);
        // A credit is not a charge.
        assert!(!check_expectations("A credit of -$500 [1].", &["$500".into()])[0].met);
        assert!(!check_expectations("Refunds: $-500.", &["$500".into()])[0].met);
        assert!(check_expectations("A credit of -$500 [1].", &["-$500".into()])[0].met);
        assert!(!check_expectations("Spend fell 12.5%.", &["-12.5%".into()])[0].met);
        // Tables often drop the currency; small amounts still count.
        assert!(check_expectations("| 1 | Static IP | 3.65 [1] |", &["$3.65".into()])[0].met);
        assert!(check_expectations("Spend was USD 980.", &["USD 980".into()])[0].met);
        // Abbreviated answers count within 5%.
        assert!(check_expectations("Spend was $1.2k.", &["$1,234.56".into()])[0].met);
        assert!(!check_expectations("Spend was $1,334.56.", &["$1,234.56".into()])[0].met);
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
