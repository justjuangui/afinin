// SPDX-License-Identifier: MIT OR Apache-2.0

//! Optional LLM assistant (FR-40..FR-46).
//!
//! The model never sees raw data dumps or computes numbers: it calls read-only
//! tools backed by the local query engine, every identifier is masked for cloud
//! providers, and every request is logged locally ("show what was sent").

pub mod agent;
pub mod context;
pub mod grounding;
pub mod providers;
pub mod recheck;
pub mod redact;
pub mod tools;

use std::any::Any;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Which LLM backend to use (FR-40). `None` keeps the app fully usable without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    None,
    /// Ollama, LM Studio, vLLM, OpenAI and other `/v1/chat/completions` servers.
    OpenAiCompatible,
    Anthropic,
    /// Amazon Bedrock Converse API with a connected account's credentials.
    Bedrock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    pub provider: ProviderKind,
    /// OpenAI-compatible base URL, e.g. `http://localhost:11434/v1` for Ollama.
    pub base_url: String,
    pub model: String,
    /// Mask identifiers before sending. `None` = on for cloud providers, off
    /// for local servers (FR-44).
    pub redact: Option<bool>,
    /// Bedrock: connected account whose credentials call Bedrock.
    pub bedrock_account_id: Option<String>,
    pub bedrock_region: String,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            provider: ProviderKind::None,
            base_url: "http://localhost:11434/v1".into(),
            model: String::new(),
            redact: None,
            bedrock_account_id: None,
            bedrock_region: "us-east-1".into(),
        }
    }
}

impl LlmSettings {
    /// The model to use, falling back to a sensible default per provider.
    pub fn model(&self) -> String {
        if !self.model.trim().is_empty() {
            return self.model.trim().to_owned();
        }
        match self.provider {
            ProviderKind::Anthropic => "claude-opus-5-5".into(),
            ProviderKind::Bedrock => "anthropic.claude-opus-5-5".into(),
            ProviderKind::OpenAiCompatible => "llama3.1".into(),
            ProviderKind::None => String::new(),
        }
    }

    /// A server on this machine (no data leaves it).
    pub fn is_local(&self) -> bool {
        self.provider == ProviderKind::OpenAiCompatible && {
            let host = self
                .base_url
                .split("://")
                .nth(1)
                .unwrap_or(&self.base_url)
                .split(['/', ':'])
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            matches!(
                host.as_str(),
                "localhost" | "127.0.0.1" | "::1" | "[::1]" | "0.0.0.0"
            ) || host.ends_with(".local")
        }
    }

    pub fn redaction_enabled(&self) -> bool {
        self.redact.unwrap_or(!self.is_local())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// Provider-specific content we must echo back unchanged (thinking blocks,
/// fallback markers, Bedrock reasoning) but never inspect.
#[derive(Clone)]
pub struct Opaque {
    pub provider: ProviderKind,
    pub json: Option<serde_json::Value>,
    pub native: Option<Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for Opaque {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opaque")
            .field("provider", &self.provider)
            .field("json", &self.json)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum Block {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    Opaque(Opaque),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub blocks: Vec<Block>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            blocks: vec![Block::Text(text.into())],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// The model declined (safety classifiers).
    Refusal,
    Other,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub blocks: Vec<Block>,
    pub stop: StopReason,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Exactly what was sent and received, for the local request log (FR-45).
    pub request_json: serde_json::Value,
    pub response_json: serde_json::Value,
}

pub trait Provider: Send + Sync {
    fn complete<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<Reply>>;
    fn kind(&self) -> ProviderKind;
    fn model(&self) -> &str;
}
