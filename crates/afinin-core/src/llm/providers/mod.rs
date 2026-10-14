// SPDX-License-Identifier: MIT OR Apache-2.0

mod anthropic;
mod bedrock;
mod openai;

pub use anthropic::Anthropic;
pub use bedrock::Bedrock;
pub use openai::OpenAiCompatible;

use std::time::Duration;

use crate::error::{Error, Result};

/// HTTP client for LLM APIs. Long timeout: answers with tool use can take minutes.
pub(crate) fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| Error::Other(format!("HTTP client: {e}")))
}

/// Turns a non-2xx response into a readable error without echoing secrets.
pub(crate) async fn check(resp: reqwest::Response) -> Result<serde_json::Value> {
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let msg = body
        .pointer("/error/message")
        .or_else(|| body.pointer("/message"))
        .or_else(|| body.get("error"))
        .and_then(|m| m.as_str())
        .unwrap_or("request failed");
    Err(Error::Other(format!("LLM API error {status}: {msg}")))
}
