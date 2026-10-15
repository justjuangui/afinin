// SPDX-License-Identifier: MIT OR Apache-2.0

//! Claude via the Anthropic Messages API (raw HTTP: there is no official Rust SDK).

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{check, http};
use crate::error::{Error, Result};
use crate::llm::{
    Block, Message, Opaque, Provider, ProviderKind, Reply, Role, StopReason, ToolSpec,
};

const API: &str = "https://api.anthropic.com/v1/messages";
/// Server-side refusal fallback, scalar `"default"` form.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub struct Anthropic {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl Anthropic {
    pub fn new(model: String, api_key: String) -> Result<Self> {
        if api_key.trim().is_empty() {
            return Err(Error::Other(
                "set an Anthropic API key in Settings → Assistant".into(),
            ));
        }
        Ok(Self {
            client: http()?,
            api_key,
            model,
        })
    }

    /// Models that accept `fallbacks: "default"` and `output_config.effort`.
    fn current_generation(&self) -> bool {
        [
            "claude-fable-5-1",
            "claude-opus-5-5",
            "claude-opus-5",
            "claude-sonnet-5-5",
        ]
        .contains(&self.model.as_str())
    }
}

fn to_wire(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            // Assistant turns are echoed exactly as received (thinking and
            // fallback blocks included); history is append-only.
            let raw: Vec<Value> = m
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Opaque(o) if o.provider == ProviderKind::Anthropic => o.json.clone(),
                    _ => None,
                })
                .collect();
            let content: Vec<Value> = if m.role == Role::Assistant && !raw.is_empty() {
                raw
            } else {
                m.blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text(t) => Some(json!({"type": "text", "text": t})),
                        Block::ToolUse { id, name, input } => Some(json!({"type": "tool_use", "id": id, "name": name, "input": input})),
                        Block::ToolResult { id, content, is_error } => {
                            Some(json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error}))
                        }
                        Block::Opaque(_) => None,
                    })
                    .collect()
            };
            json!({"role": role, "content": content})
        })
        .collect()
}

impl Provider for Anthropic {
    fn complete<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<Reply>> {
        Box::pin(async move {
            // Prompt caching: tools and system prompt are the same on every
            // round, and each round resends the whole conversation, so mark
            // the end of the system prompt and of the latest message. Later
            // rounds read that prefix at a tenth of the input price.
            let mut wire = to_wire(messages);
            if let Some(last) = wire
                .last_mut()
                .and_then(|m| m["content"].as_array_mut())
                .and_then(|c| c.last_mut())
            {
                last["cache_control"] = json!({"type": "ephemeral"});
            }
            let mut request = json!({
                "model": self.model,
                "max_tokens": 16000,
                "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
                "messages": wire,
                "tools": tools.iter().map(|t| json!({
                    "name": t.name, "description": t.description, "input_schema": t.schema
                })).collect::<Vec<_>>(),
            });
            let mut req = self
                .client
                .post(API)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01");
            if self.current_generation() {
                // Chat over tool results: medium effort balances quality and latency.
                request["output_config"] = json!({"effort": "medium"});
                request["fallbacks"] = json!("default");
                req = req.header("anthropic-beta", FALLBACK_BETA);
            }
            let resp = req
                .json(&request)
                .send()
                .await
                .map_err(|e| Error::Other(format!("cannot reach Anthropic: {e}")))?;
            let body = check(resp).await?;

            let mut blocks = Vec::new();
            for b in body["content"].as_array().into_iter().flatten() {
                blocks.push(Block::Opaque(Opaque {
                    provider: ProviderKind::Anthropic,
                    json: Some(b.clone()),
                    native: None,
                }));
                match b["type"].as_str() {
                    Some("text") => blocks.push(Block::Text(
                        b["text"].as_str().unwrap_or_default().to_owned(),
                    )),
                    Some("tool_use") => blocks.push(Block::ToolUse {
                        id: b["id"].as_str().unwrap_or_default().to_owned(),
                        name: b["name"].as_str().unwrap_or_default().to_owned(),
                        input: b["input"].clone(),
                    }),
                    _ => {}
                }
            }
            let stop = match body["stop_reason"].as_str() {
                Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
                Some("tool_use") => StopReason::ToolUse,
                Some("max_tokens") => StopReason::MaxTokens,
                Some("refusal") => StopReason::Refusal,
                _ => StopReason::Other,
            };
            Ok(Reply {
                blocks,
                stop,
                model: body["model"].as_str().unwrap_or(&self.model).to_owned(),
                input_tokens: body
                    .pointer("/usage/input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: body
                    .pointer("/usage/output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_read_tokens: body
                    .pointer("/usage/cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_write_tokens: body
                    .pointer("/usage/cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                request_json: request,
                response_json: body,
            })
        })
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_assistant_blocks_verbatim() {
        let thinking = json!({"type": "thinking", "thinking": "", "signature": "sig"});
        let tool =
            json!({"type": "tool_use", "id": "t1", "name": "query_costs", "input": {"a": 1}});
        let messages = vec![
            Message::user_text("hi"),
            Message {
                role: Role::Assistant,
                blocks: vec![
                    Block::Opaque(Opaque {
                        provider: ProviderKind::Anthropic,
                        json: Some(thinking.clone()),
                        native: None,
                    }),
                    Block::Opaque(Opaque {
                        provider: ProviderKind::Anthropic,
                        json: Some(tool.clone()),
                        native: None,
                    }),
                    Block::ToolUse {
                        id: "t1".into(),
                        name: "query_costs".into(),
                        input: json!({"a": 1}),
                    },
                ],
            },
            Message {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    id: "t1".into(),
                    content: "{}".into(),
                    is_error: false,
                }],
            },
        ];
        let wire = to_wire(&messages);
        assert_eq!(
            wire[1]["content"],
            json!([thinking, tool]),
            "no duplicate tool_use, thinking kept"
        );
        assert_eq!(wire[2]["content"][0]["type"], "tool_result");
        assert_eq!(wire[2]["content"][0]["tool_use_id"], "t1");
    }
}
