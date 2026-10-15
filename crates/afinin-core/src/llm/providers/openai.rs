// SPDX-License-Identifier: MIT OR Apache-2.0

//! OpenAI-compatible `/chat/completions` (Ollama, LM Studio, vLLM, OpenAI…).

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{check, http};
use crate::error::{Error, Result};
use crate::llm::{Block, Message, Provider, ProviderKind, Reply, Role, StopReason, ToolSpec};

pub struct OpenAiCompatible {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiCompatible {
    pub fn new(base_url: &str, model: String, api_key: Option<String>) -> Result<Self> {
        Ok(Self {
            client: http()?,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            api_key,
        })
    }
}

fn to_wire(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": system})];
    for m in messages {
        match m.role {
            Role::User => {
                let mut text = Vec::new();
                for b in &m.blocks {
                    match b {
                        Block::Text(t) => text.push(t.clone()),
                        Block::ToolResult { id, content, .. } => {
                            out.push(
                                json!({"role": "tool", "tool_call_id": id, "content": content}),
                            );
                        }
                        _ => {}
                    }
                }
                if !text.is_empty() {
                    out.push(json!({"role": "user", "content": text.join("\n\n")}));
                }
            }
            Role::Assistant => {
                let text: Vec<&str> = m
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let Block::Text(t) = b {
                            Some(t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                let calls: Vec<Value> = m
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolUse { id, name, input } => Some(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": input.to_string()}
                        })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text.join("\n\n")) }});
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
        }
    }
    out
}

impl Provider for OpenAiCompatible {
    fn complete<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<Reply>> {
        Box::pin(async move {
            let request = json!({
                "model": self.model,
                "messages": to_wire(system, messages),
                "tools": tools.iter().map(|t| json!({
                    "type": "function",
                    "function": {"name": t.name, "description": t.description, "parameters": t.schema}
                })).collect::<Vec<_>>(),
            });
            let mut req = self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .json(&request);
            if let Some(key) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
                req = req.bearer_auth(key);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| Error::Other(format!("cannot reach {}: {e}", self.base_url)))?;
            let body = check(resp).await?;

            let choice = body
                .pointer("/choices/0")
                .ok_or_else(|| Error::Other("LLM returned no choices".into()))?;
            let msg = &choice["message"];
            let mut blocks = Vec::new();
            if let Some(t) = msg["content"].as_str().filter(|t| !t.trim().is_empty()) {
                blocks.push(Block::Text(t.to_owned()));
            }
            for (i, call) in msg["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let f = &call["function"];
                // Some servers send arguments as an object instead of a JSON string.
                let input = match &f["arguments"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                    other if other.is_object() => other.clone(),
                    _ => json!({}),
                };
                blocks.push(Block::ToolUse {
                    id: call["id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("call_{i}")),
                    name: f["name"].as_str().unwrap_or_default().to_owned(),
                    input,
                });
            }
            let has_tools = blocks.iter().any(|b| matches!(b, Block::ToolUse { .. }));
            let stop = match choice["finish_reason"].as_str() {
                _ if has_tools => StopReason::ToolUse,
                Some("length") => StopReason::MaxTokens,
                Some("content_filter") => StopReason::Refusal,
                _ => StopReason::EndTurn,
            };
            // prompt_tokens includes cached tokens; report them apart, as
            // the other providers do.
            let tokens = |path: &str| body.pointer(path).and_then(|v| v.as_u64()).unwrap_or(0);
            let cached = tokens("/usage/prompt_tokens_details/cached_tokens");
            Ok(Reply {
                blocks,
                stop,
                model: body["model"].as_str().unwrap_or(&self.model).to_owned(),
                input_tokens: tokens("/usage/prompt_tokens").saturating_sub(cached),
                cache_read_tokens: cached,
                cache_write_tokens: 0,
                output_tokens: tokens("/usage/completion_tokens"),
                request_json: request,
                response_json: body,
            })
        })
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_tool_turns() {
        let messages = vec![
            Message::user_text("hi"),
            Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolUse {
                    id: "c1".into(),
                    name: "get_insights".into(),
                    input: json!({}),
                }],
            },
            Message {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    id: "c1".into(),
                    content: "{\"x\":1}".into(),
                    is_error: false,
                }],
            },
        ];
        let wire = to_wire("sys", &messages);
        assert_eq!(wire[0]["role"], "system");
        assert_eq!(wire[2]["tool_calls"][0]["function"]["arguments"], "{}");
        assert_eq!(wire[2]["content"], Value::Null);
        assert_eq!(
            wire[3],
            json!({"role": "tool", "tool_call_id": "c1", "content": "{\"x\":1}"})
        );
    }
}
