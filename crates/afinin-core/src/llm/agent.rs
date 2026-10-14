// SPDX-License-Identifier: MIT OR Apache-2.0

//! The tool-use loop for one chat turn.

use serde::Serialize;
use serde_json::{Value, json};

use super::redact::Redactor;
use super::{Block, Message, Provider, Reply, Role, StopReason, ToolSpec};
use crate::error::Result;

/// Tool rounds per user message before giving up.
const MAX_ROUNDS: usize = 8;

/// A conversation with one provider. History is append-only.
pub struct Conversation {
    pub id: String,
    pub messages: Vec<Message>,
    pub redactor: Redactor,
    pub citations: Vec<Citation>,
}

/// A tool call the answer can cite as `[n]` (FR-46). Stored unmasked for the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Citation {
    pub n: usize,
    pub tool: String,
    pub input: Value,
    pub result: Value,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Turn {
    pub conversation_id: String,
    /// Answer with identifiers restored.
    pub answer: String,
    pub citations: Vec<Citation>,
    pub stop: StopReason,
    pub model: String,
    pub redacted: bool,
    /// Distinct identifiers masked so far in this conversation.
    pub masked_values: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn system_prompt(today: &str, redacted: bool) -> String {
    let mut s = format!(
        "You are the assistant inside Afinín, a read-only AWS cost and inventory app. Today is {today} (UTC).\n\
         \n\
         Answer questions about the user's AWS spend, resources and findings using the tools. Rules:\n\
         - Every number you state must come from a tool result. Do not add, subtract or estimate numbers yourself; \
         call a tool that returns the figure you need (compare_periods returns changes, query_costs returns totals and shares).\n\
         - Each tool result has a `source` number. Cite it after the figures it supports, like [2].\n\
         - Afinín cannot change AWS resources. When suggesting fixes, describe the steps for the user to take in the AWS console or their infrastructure code.\n\
         - Costs are in the `unit` the tool returns. Cost Explorer data for the last day or two is an AWS estimate.\n\
         - If the data needed isn't available (e.g. per-resource cost without CUR), say so and suggest what would make it available.\n\
         - Be concise: lead with the answer, then the supporting figures."
    );
    if redacted {
        s.push_str(
            "\n- Identifiers are masked as placeholders such as ACCOUNT_1, ARN_2, RESOURCE_3, IP_1, HOST_1 and NAME_4. \
             Use them exactly as written, including in tool inputs; the app restores the real values for the user.",
        );
    }
    s
}

/// Runs one user turn: asks the model, executes its tool calls locally, and
/// repeats until it answers.
pub async fn run_turn(
    provider: &dyn Provider,
    tools: &[ToolSpec],
    conv: &mut Conversation,
    user_text: &str,
    today: &str,
    mut execute: impl FnMut(&str, &Value) -> Result<Value>,
    mut log: impl FnMut(&Reply),
) -> Result<Turn> {
    let system = system_prompt(today, conv.redactor.enabled());
    let masked = conv.redactor.mask(user_text);
    conv.messages.push(Message::user_text(masked));

    let first_citation = conv.citations.len();
    let (mut input_tokens, mut output_tokens) = (0, 0);
    let mut model = provider.model().to_owned();
    for _ in 0..MAX_ROUNDS {
        let reply = provider.complete(&system, &conv.messages, tools).await?;
        log(&reply);
        input_tokens += reply.input_tokens;
        output_tokens += reply.output_tokens;
        model = reply.model.clone();
        let calls: Vec<(String, String, Value)> = reply
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();
        let text: Vec<String> = reply
            .blocks
            .iter()
            .filter_map(|b| {
                if let Block::Text(t) = b {
                    Some(t.clone())
                } else {
                    None
                }
            })
            .collect();
        conv.messages.push(Message {
            role: Role::Assistant,
            blocks: reply.blocks.clone(),
        });

        if calls.is_empty()
            || reply.stop != StopReason::ToolUse && reply.stop != StopReason::EndTurn
        {
            let mut answer = conv.redactor.unmask(&text.join("\n\n"));
            match reply.stop {
                StopReason::Refusal => {
                    answer = "The model declined to answer this request. Try rephrasing, or ask about a narrower topic.".into()
                }
                StopReason::MaxTokens => answer.push_str("\n\n_(The answer was cut off at the model's length limit.)_"),
                _ => {}
            }
            return Ok(Turn {
                conversation_id: conv.id.clone(),
                answer,
                citations: conv.citations[first_citation..].to_vec(),
                stop: reply.stop,
                model,
                redacted: conv.redactor.enabled(),
                masked_values: conv.redactor.masked_count(),
                input_tokens,
                output_tokens,
            });
        }

        // All results go back in a single user message.
        let mut results = Vec::new();
        for (id, name, input) in calls {
            let input = conv.redactor.unmask_json(&input);
            let n = conv.citations.len() + 1;
            let (result, ok) = match execute(&name, &input) {
                Ok(v) => (v, true),
                Err(e) => (json!({"error": e.to_string()}), false),
            };
            let mut for_model = json!({"source": n});
            if let (Some(dst), Some(src)) = (for_model.as_object_mut(), result.as_object()) {
                dst.extend(src.clone());
            } else {
                for_model["result"] = result.clone();
            }
            let content = conv.redactor.mask_json(&for_model).to_string();
            conv.citations.push(Citation {
                n,
                tool: name,
                input,
                result,
                ok,
            });
            results.push(Block::ToolResult {
                id,
                content,
                is_error: !ok,
            });
        }
        conv.messages.push(Message {
            role: Role::User,
            blocks: results,
        });
    }

    Ok(Turn {
        conversation_id: conv.id.clone(),
        answer: "I stopped after several rounds of tool calls without reaching an answer. Try a more specific question.".into(),
        citations: conv.citations[first_citation..].to_vec(),
        stop: StopReason::Other,
        model,
        redacted: conv.redactor.enabled(),
        masked_values: conv.redactor.masked_count(),
        input_tokens,
        output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Provider, ProviderKind};
    use futures::future::BoxFuture;
    use std::sync::Mutex;

    /// Calls `query_costs` once with whatever account placeholder it saw, then answers.
    struct Scripted {
        seen: Mutex<Vec<Value>>,
    }

    impl Provider for Scripted {
        fn complete<'a>(
            &'a self,
            system: &'a str,
            messages: &'a [Message],
            _: &'a [ToolSpec],
        ) -> BoxFuture<'a, Result<Reply>> {
            Box::pin(async move {
                let wire: Vec<Value> = messages
                    .iter()
                    .flat_map(|m| m.blocks.iter())
                    .map(|b| match b {
                        Block::Text(t) => json!(t),
                        Block::ToolResult { content, .. } => json!(content),
                        Block::ToolUse { input, .. } => input.clone(),
                        Block::Opaque(_) => Value::Null,
                    })
                    .collect();
                let request = json!({"system": system, "messages": wire});
                self.seen.lock().unwrap().push(request.clone());
                let first = messages.len() == 1;
                let blocks = if first {
                    vec![Block::ToolUse {
                        id: "t1".into(),
                        name: "query_costs".into(),
                        input: json!({"account_ids": ["ACCOUNT_1"]}),
                    }]
                } else {
                    vec![Block::Text(
                        "ACCOUNT_1 spent 42.5 on Amazon EC2 [1], mostly ARN_1.".into(),
                    )]
                };
                Ok(Reply {
                    blocks,
                    stop: if first {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    },
                    model: "scripted".into(),
                    input_tokens: 10,
                    output_tokens: 5,
                    request_json: request,
                    response_json: Value::Null,
                })
            })
        }
        fn kind(&self) -> ProviderKind {
            ProviderKind::Anthropic
        }
        fn model(&self) -> &str {
            "scripted"
        }
    }

    #[tokio::test]
    async fn redacts_everything_sent_and_restores_the_answer() {
        let provider = Scripted {
            seen: Mutex::default(),
        };
        let mut conv = Conversation {
            id: "c1".into(),
            messages: Vec::new(),
            redactor: Redactor::new(true, ["checkout".to_owned()]),
            citations: Vec::new(),
        };
        let mut executed = Vec::new();
        let mut logged = Vec::new();
        let turn = run_turn(
            &provider,
            &[],
            &mut conv,
            "Why did 123456789012 spend more?",
            "2026-10-14",
            |name, input| {
                executed.push((name.to_owned(), input.clone()));
                Ok(json!({"total": 42.5, "top": "arn:aws:ec2:us-east-1:123456789012:instance/i-0123456789abcdef0", "app": "checkout"}))
            },
            |reply| logged.push(reply.request_json.to_string()),
        )
        .await
        .unwrap();

        // The tool ran with the real account ID…
        assert_eq!(executed[0].1["account_ids"][0], "123456789012");
        // …while nothing sent to the model contains real identifiers (acceptance criterion).
        assert_eq!(logged.len(), 2);
        for request in &logged {
            assert!(!request.contains("123456789012"), "{request}");
            assert!(!request.contains("arn:aws"), "{request}");
            assert!(!request.contains("checkout"), "{request}");
        }
        assert_eq!(
            turn.answer,
            "123456789012 spent 42.5 on Amazon EC2 [1], mostly arn:aws:ec2:us-east-1:123456789012:instance/i-0123456789abcdef0."
        );
        assert_eq!(turn.citations.len(), 1);
        assert_eq!(turn.citations[0].tool, "query_costs");
        assert_eq!(turn.citations[0].result["total"], 42.5);
    }
}
