// SPDX-License-Identifier: MIT OR Apache-2.0

//! What one answer cost and how it got there: each round trip to the model,
//! the tool calls it made, and patterns that waste tokens (repeated calls,
//! large results resent every round, the round limit).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::StopReason;
use super::agent::{Citation, MAX_ROUNDS, Turn};

/// Tool results above this many estimated tokens are flagged: every later
/// round of the conversation sends them again.
pub const LARGE_RESULT_TOKENS: usize = 4_000;

/// One request to the model and the tool calls it asked for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Round {
    /// 1-based, in order.
    pub n: usize,
    /// Input tokens billed at the full rate (excludes cache reads and writes).
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Time waiting for the model.
    pub ms: u64,
    pub stop: StopReason,
    /// Sources (citation numbers) of the tool calls this round asked for.
    pub sources: Vec<usize>,
    /// Request log entry, for "show what was sent".
    pub log_id: Option<i64>,
    /// Estimated from the model's price in Settings.
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

impl Round {
    /// Every input token the model read, cached or not.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// A pattern in the trace that costs tokens for little gain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceFlag {
    /// The same tool call more than once in one answer.
    RepeatedCall { label: String, times: usize },
    /// Gave up at the round limit without answering.
    RoundLimit { rounds: usize },
    /// Tool calls that failed (usually a bad input the model then retried).
    ToolErrors { count: usize },
    /// A tool result large enough to dominate later rounds.
    LargeResult { source: usize, tokens: usize },
}

/// JSON with object keys sorted, so equal inputs compare equal whatever
/// order the model wrote them in.
pub fn canonical(v: &Value) -> Value {
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

/// Identity of a tool call: tool name and canonical input.
pub fn call_key(tool: &str, input: &Value) -> String {
    format!("{tool}:{}", canonical(input))
}

/// Rough token count of text sent to a model (about four characters per
/// token for JSON and English); good enough to spot what dominates.
pub fn estimate_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

pub fn flags(turn: &Turn) -> Vec<TraceFlag> {
    let mut out = Vec::new();
    let mut seen: Vec<(String, &Citation, usize)> = Vec::new();
    for c in &turn.citations {
        let key = call_key(&c.tool, &c.input);
        match seen.iter_mut().find(|(k, ..)| *k == key) {
            Some((_, _, times)) => *times += 1,
            None => seen.push((key, c, 1)),
        }
    }
    for (_, c, times) in seen {
        if times > 1 {
            out.push(TraceFlag::RepeatedCall {
                label: super::tools::describe(&c.tool, &c.input),
                times,
            });
        }
    }
    if turn.stop == StopReason::Other && turn.rounds.len() >= MAX_ROUNDS {
        out.push(TraceFlag::RoundLimit {
            rounds: turn.rounds.len(),
        });
    }
    let errors = turn.citations.iter().filter(|c| !c.ok).count();
    if errors > 0 {
        out.push(TraceFlag::ToolErrors { count: errors });
    }
    for c in &turn.citations {
        let tokens = estimate_tokens(c.sent_chars);
        if tokens > LARGE_RESULT_TOKENS {
            out.push(TraceFlag::LargeResult {
                source: c.n,
                tokens,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flags_repeats_errors_large_results_and_the_round_limit() {
        let cite = |n, input: Value, ok, sent_chars| Citation {
            n,
            tool: "query_costs".into(),
            input,
            ok,
            sent_chars,
            ..Default::default()
        };
        let mut turn = Turn {
            stop: StopReason::EndTurn,
            citations: vec![
                cite(1, json!({"group_by": "service", "start": "2026-09-01"}), true, 100),
                cite(2, json!({"start": "2026-09-01", "group_by": "service"}), true, 100),
                cite(3, json!({"group_by": "nope"}), false, 50),
                cite(4, json!({"group_by": "resource"}), true, 40_000),
            ],
            rounds: vec![Round::default(); 3],
            ..Default::default()
        };
        let f = flags(&turn);
        assert!(matches!(&f[0], TraceFlag::RepeatedCall { times: 2, .. }), "{f:?}");
        assert_eq!(f[1], TraceFlag::ToolErrors { count: 1 });
        assert_eq!(
            f[2],
            TraceFlag::LargeResult {
                source: 4,
                tokens: 10_000
            }
        );
        assert_eq!(f.len(), 3);

        turn.stop = StopReason::Other;
        turn.rounds = vec![Round::default(); MAX_ROUNDS];
        assert!(flags(&turn).contains(&TraceFlag::RoundLimit { rounds: MAX_ROUNDS }));
    }
}
