// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error type shared across the core library.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(#[from] duckdb::Error),

    #[error("keychain error: {0}")]
    Secrets(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("AWS profile `{0}` not found")]
    ProfileNotFound(String),

    #[error("invalid profile configuration: {0}")]
    InvalidProfile(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("AWS error: {0}")]
    Aws(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub(crate) fn aws(err: impl std::error::Error) -> Self {
        // Full error chain helps users diagnose permission and token problems.
        let mut msg = err.to_string();
        let mut source = err.source();
        while let Some(s) = source {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            source = s.source();
        }
        Self::Aws(msg)
    }
}

// Errors cross the IPC boundary as plain strings.
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
