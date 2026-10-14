// SPDX-License-Identifier: MIT OR Apache-2.0

//! Amazon Bedrock Converse API, called with a connected account's credentials.

use std::collections::HashMap;
use std::sync::Arc;

use aws_config::{Region, SdkConfig};
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, ConverseOutput, Message as BrMessage, StopReason as BrStop,
    SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema, ToolResultBlock,
    ToolResultContentBlock, ToolResultStatus, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::{Document, Number};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::inventory::collect::aws_err;
use crate::llm::{
    Block, Message, Opaque, Provider, ProviderKind, Reply, Role, StopReason, ToolSpec,
};

pub struct Bedrock {
    client: Client,
    model: String,
}

impl Bedrock {
    pub fn new(config: &SdkConfig, region: &str, model: String) -> Self {
        let conf = aws_sdk_bedrockruntime::config::Builder::from(config)
            .region(Region::new(region.to_owned()))
            .build();
        Self {
            client: Client::from_conf(conf),
            model,
        }
    }
}

pub(crate) fn to_doc(v: &Value) -> Document {
    match v {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => Document::Number(if let Some(u) = n.as_u64() {
            Number::PosInt(u)
        } else if let Some(i) = n.as_i64() {
            Number::NegInt(i)
        } else {
            Number::Float(n.as_f64().unwrap_or(0.0))
        }),
        Value::String(s) => Document::String(s.clone()),
        Value::Array(a) => Document::Array(a.iter().map(to_doc).collect()),
        Value::Object(o) => Document::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), to_doc(v)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}

pub(crate) fn from_doc(d: &Document) -> Value {
    match d {
        Document::Null => Value::Null,
        Document::Bool(b) => json!(b),
        Document::Number(Number::PosInt(u)) => json!(u),
        Document::Number(Number::NegInt(i)) => json!(i),
        Document::Number(Number::Float(f)) => json!(f),
        Document::String(s) => json!(s),
        Document::Array(a) => Value::Array(a.iter().map(from_doc).collect()),
        Document::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), from_doc(v))).collect())
        }
    }
}

fn build_err(e: impl std::fmt::Display) -> Error {
    Error::Other(format!("Bedrock request: {e}"))
}

/// Converse messages plus a JSON mirror of them for the request log.
fn to_wire(messages: &[Message]) -> Result<(Vec<BrMessage>, Vec<Value>)> {
    let mut out = Vec::new();
    let mut log = Vec::new();
    for m in messages {
        let role = match m.role {
            Role::User => ConversationRole::User,
            Role::Assistant => ConversationRole::Assistant,
        };
        let native: Vec<ContentBlock> = m
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Opaque(o) if o.provider == ProviderKind::Bedrock => o
                    .native
                    .as_ref()
                    .and_then(|n| n.downcast_ref::<ContentBlock>())
                    .cloned(),
                _ => None,
            })
            .collect();
        let mut content = Vec::new();
        let mut content_log = Vec::new();
        if m.role == Role::Assistant && !native.is_empty() {
            // Echo assistant turns as received (reasoning blocks included).
            for b in native {
                content_log.push(match &b {
                    ContentBlock::Text(t) => json!({"text": t}),
                    ContentBlock::ToolUse(t) => json!({"toolUse": {"toolUseId": t.tool_use_id(), "name": t.name(), "input": from_doc(t.input())}}),
                    _ => json!({"reasoningContent": "(echoed)"}),
                });
                content.push(b);
            }
        } else {
            for b in &m.blocks {
                match b {
                    Block::Text(t) => {
                        content_log.push(json!({"text": t}));
                        content.push(ContentBlock::Text(t.clone()));
                    }
                    Block::ToolUse { id, name, input } => {
                        content_log.push(
                            json!({"toolUse": {"toolUseId": id, "name": name, "input": input}}),
                        );
                        content.push(ContentBlock::ToolUse(
                            ToolUseBlock::builder()
                                .tool_use_id(id)
                                .name(name)
                                .input(to_doc(input))
                                .build()
                                .map_err(build_err)?,
                        ));
                    }
                    Block::ToolResult {
                        id,
                        content: text,
                        is_error,
                    } => {
                        content_log.push(json!({"toolResult": {"toolUseId": id, "content": [{"text": text}], "status": if *is_error { "error" } else { "success" }}}));
                        content.push(ContentBlock::ToolResult(
                            ToolResultBlock::builder()
                                .tool_use_id(id)
                                .content(ToolResultContentBlock::Text(text.clone()))
                                .status(if *is_error {
                                    ToolResultStatus::Error
                                } else {
                                    ToolResultStatus::Success
                                })
                                .build()
                                .map_err(build_err)?,
                        ));
                    }
                    Block::Opaque(_) => {}
                }
            }
        }
        log.push(json!({"role": if m.role == Role::User { "user" } else { "assistant" }, "content": content_log}));
        out.push(
            BrMessage::builder()
                .role(role)
                .set_content(Some(content))
                .build()
                .map_err(build_err)?,
        );
    }
    Ok((out, log))
}

impl Provider for Bedrock {
    fn complete<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<Reply>> {
        Box::pin(async move {
            let (wire, log) = to_wire(messages)?;
            let tool_config = ToolConfiguration::builder()
                .set_tools(Some(
                    tools
                        .iter()
                        .map(|t| {
                            ToolSpecification::builder()
                                .name(t.name)
                                .description(t.description)
                                .input_schema(ToolInputSchema::Json(to_doc(&t.schema)))
                                .build()
                                .map(Tool::ToolSpec)
                                .map_err(build_err)
                        })
                        .collect::<Result<Vec<_>>>()?,
                ))
                .build()
                .map_err(build_err)?;
            let request_json = json!({
                "modelId": self.model,
                "system": [{"text": system}],
                "messages": log,
                "toolConfig": {"tools": tools.iter().map(|t| json!({"toolSpec": {"name": t.name, "description": t.description, "inputSchema": {"json": t.schema}}})).collect::<Vec<_>>()},
            });

            let out = self
                .client
                .converse()
                .model_id(&self.model)
                .system(SystemContentBlock::Text(system.to_owned()))
                .set_messages(Some(wire))
                .tool_config(tool_config)
                .send()
                .await
                .map_err(aws_err)?;

            let mut blocks = Vec::new();
            let mut response_content = Vec::new();
            if let Some(ConverseOutput::Message(msg)) = out.output() {
                for b in msg.content() {
                    blocks.push(Block::Opaque(Opaque {
                        provider: ProviderKind::Bedrock,
                        json: None,
                        native: Some(Arc::new(b.clone())),
                    }));
                    match b {
                        ContentBlock::Text(t) => {
                            response_content.push(json!({"text": t}));
                            blocks.push(Block::Text(t.clone()));
                        }
                        ContentBlock::ToolUse(t) => {
                            let input = from_doc(t.input());
                            response_content.push(json!({"toolUse": {"toolUseId": t.tool_use_id(), "name": t.name(), "input": input}}));
                            blocks.push(Block::ToolUse {
                                id: t.tool_use_id().to_owned(),
                                name: t.name().to_owned(),
                                input,
                            });
                        }
                        _ => response_content
                            .push(json!({"other": "reasoning or unsupported block"})),
                    }
                }
            }
            let stop = match out.stop_reason() {
                BrStop::EndTurn | BrStop::StopSequence => StopReason::EndTurn,
                BrStop::ToolUse => StopReason::ToolUse,
                BrStop::MaxTokens => StopReason::MaxTokens,
                BrStop::ContentFiltered | BrStop::GuardrailIntervened => StopReason::Refusal,
                _ => StopReason::Other,
            };
            let usage = out.usage();
            Ok(Reply {
                blocks,
                stop,
                model: self.model.clone(),
                input_tokens: usage.map(|u| u.input_tokens().max(0) as u64).unwrap_or(0),
                output_tokens: usage.map(|u| u.output_tokens().max(0) as u64).unwrap_or(0),
                request_json,
                response_json: json!({"stopReason": out.stop_reason().as_str(), "content": response_content}),
            })
        })
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Bedrock
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_round_trip() {
        let v = json!({"a": [1, -2, 3.5, "x", true, null], "b": {"c": "d"}});
        assert_eq!(from_doc(&to_doc(&v)), v);
    }
}
