// SPDX-License-Identifier: MIT OR Apache-2.0

//! Optional LLM assistant (FR-40..FR-46).
//!
//! The model never sees raw data dumps or computes numbers: it calls read-only
//! tools backed by the local query engine, every identifier is masked for cloud
//! providers, and every request is logged locally ("show what was sent").

pub mod agent;
pub mod context;
pub mod examples;
pub mod grounding;
pub mod providers;
pub mod recheck;
pub mod redact;
pub mod tools;
pub mod trace;

use std::any::Any;
use std::collections::BTreeMap;
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

/// A model's price in USD per million tokens, as entered in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenPrice {
    pub input: f64,
    pub output: f64,
    /// Prompt cache reads; a tenth of `input` when not set (Anthropic, Bedrock).
    pub cache_read: Option<f64>,
    /// Prompt cache writes; 1.25 × `input` when not set (Anthropic, Bedrock).
    pub cache_write: Option<f64>,
}

impl TokenPrice {
    pub fn cost(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
        let read = self.cache_read.unwrap_or(self.input * 0.1);
        let write = self.cache_write.unwrap_or(self.input * 1.25);
        (input as f64 * self.input
            + output as f64 * self.output
            + cache_read as f64 * read
            + cache_write as f64 * write)
            / 1e6
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Prices by model ID, for cost estimates.
    pub prices: BTreeMap<String, TokenPrice>,
    /// Stop asking once this month's estimated assistant cost reaches it.
    pub monthly_budget_usd: Option<f64>,
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
            prices: BTreeMap::new(),
            monthly_budget_usd: None,
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

    /// The price for a model as a provider reports it: the one entered in
    /// Settings, else Anthropic's list price for its own model IDs
    /// ([`known_price`]). Zero prices count as not set.
    pub fn price(&self, model: &str) -> Option<TokenPrice> {
        best_match(
            model,
            self.prices
                .iter()
                .filter(|(_, p)| p.input > 0.0 || p.output > 0.0)
                .map(|(id, p)| (id.as_str(), *p)),
        )
        .or_else(|| known_price(model))
    }
}

/// When [`KNOWN_PRICES`] was last checked against Anthropic's pricing page.
pub const KNOWN_PRICES_AS_OF: &str = "October 2026";

/// Anthropic API list prices (USD per million tokens) for its own model IDs.
/// Cache reads are 0.1 × input unless the model prices them differently;
/// cache writes are 1.25 × input (5-minute cache, which the assistant uses).
/// Bedrock (`anthropic.claude-…`) and other resellers price on their own, so
/// their IDs never match here.
const KNOWN_PRICES: &[(&str, f64, f64, Option<f64>)] = &[
    ("claude-fable-5-1", 10.0, 50.0, Some(0.25)),
    ("claude-fable-5", 10.0, 50.0, None),
    ("claude-opus-5-5", 4.0, 20.0, Some(0.20)),
    ("claude-opus-5", 5.0, 25.0, None),
    ("claude-opus-4-8", 5.0, 25.0, None),
    ("claude-opus-4-7", 5.0, 25.0, None),
    ("claude-opus-4-6", 5.0, 25.0, None),
    ("claude-sonnet-5-5", 2.0, 10.0, Some(0.20)),
    ("claude-sonnet-5", 2.0, 10.0, None),
    ("claude-sonnet-4-6", 3.0, 15.0, None),
    // Prompts up to 100K tokens; longer ones cost 5 ×.
    ("claude-haiku-5-5", 0.10, 0.50, None),
    ("claude-haiku-4-5", 1.0, 5.0, None),
];

/// Anthropic's list price for one of its model IDs (or a dated version of it).
pub fn known_price(model: &str) -> Option<TokenPrice> {
    best_match(
        model,
        KNOWN_PRICES.iter().map(|&(id, input, output, cache_read)| {
            (
                id,
                TokenPrice {
                    input,
                    output,
                    cache_read,
                    cache_write: None,
                },
            )
        }),
    )
}

/// The price whose ID is `model` or a version of it. Providers may answer
/// with a version of the configured ID (`claude-x-20260901`, `model-v1:0`,
/// `model@2026…`), so a priced ID followed by a version also counts; another
/// model sharing a prefix (`gpt-4` / `gpt-4o`, `claude-opus-5` /
/// `claude-opus-5-5`) does not.
fn best_match<'a>(model: &str, prices: impl Iterator<Item = (&'a str, TokenPrice)>) -> Option<TokenPrice> {
    let version_of = |id: &str| {
        model.strip_prefix(id).is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with([':', '@'])
                || rest.starts_with("-v")
                || rest
                    .strip_prefix('-')
                    .is_some_and(|d| d.len() >= 6 && d.chars().take(6).all(|c| c.is_ascii_digit()))
        })
    };
    prices
        .filter(|(id, _)| version_of(id))
        .max_by_key(|(id, _)| id.len())
        .map(|(_, p)| p)
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// The model declined (safety classifiers).
    Refusal,
    #[default]
    Other,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub blocks: Vec<Block>,
    pub stop: StopReason,
    pub model: String,
    /// Input tokens billed at the full rate: excludes cache reads and writes.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_match_dated_model_ids_and_cost_cache_tokens_less() {
        let mut cfg = LlmSettings::default();
        let price = TokenPrice {
            input: 5.0,
            output: 25.0,
            ..Default::default()
        };
        cfg.prices.insert("claude-x".into(), price);
        cfg.prices.insert("unpriced".into(), TokenPrice::default());
        assert_eq!(cfg.price("claude-x"), Some(price));
        assert_eq!(cfg.price("claude-x-20260901"), Some(price));
        assert_eq!(cfg.price("claude-x-v1:0"), Some(price));
        // Another model that only shares a prefix is not priced.
        assert_eq!(cfg.price("claude-xl"), None);
        assert_eq!(cfg.price("claude-x-5"), None);
        assert_eq!(cfg.price("unpriced"), None);
        assert_eq!(cfg.price("other"), None);
        // Anthropic's own IDs have a list price unless one is entered;
        // Bedrock IDs and prefixes of other models don't.
        assert_eq!(cfg.price("claude-opus-5-5").map(|p| p.input), Some(4.0));
        assert_eq!(cfg.price("claude-opus-5-20260101").map(|p| p.input), Some(5.0));
        assert_eq!(cfg.price("claude-opus-5-5").and_then(|p| p.cache_read), Some(0.20));
        assert_eq!(cfg.price("anthropic.claude-opus-5-5"), None);
        cfg.prices.insert("claude-opus-5-5".into(), price);
        assert_eq!(cfg.price("claude-opus-5-5"), Some(price));
        // 1M uncached in, 1M out, 1M cache reads (0.5), 1M cache writes (6.25).
        let cost = price.cost(1_000_000, 1_000_000, 1_000_000, 1_000_000);
        assert!((cost - 36.75).abs() < 1e-9, "{cost}");
    }
}
