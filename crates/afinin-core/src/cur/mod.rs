// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cost data, tier 2: CUR 2.0 via AWS Data Exports (FR-15, FR-24, FR-34).
//!
//! Afinín never creates the export (it is read-only); the setup wizard hands
//! the user a template and validates the result with read-only calls. Parquet
//! files are downloaded incrementally, loaded into the encrypted database and
//! deleted right away, so billing data never rests unencrypted on disk.

pub mod ingest;
pub mod source;
pub mod template;

use serde::{Deserialize, Serialize};

/// Where an account's CUR 2.0 export is delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurConfig {
    /// Connected account (`accounts.id`) whose credentials read the bucket.
    pub account_id: String,
    pub bucket: String,
    /// S3 prefix configured on the export; may be empty.
    pub prefix: String,
    pub export_name: String,
    /// Bucket region.
    pub region: String,
}

impl CurConfig {
    /// `<prefix>/<export name>/` — root of everything the export writes.
    pub fn root(&self) -> String {
        let prefix = self.prefix.trim_matches('/');
        if prefix.is_empty() {
            format!("{}/", self.export_name)
        } else {
            format!("{prefix}/{}/", self.export_name)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warning,
    Error,
}

/// One line of the setup wizard's validation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurCheck {
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurValidation {
    pub checks: Vec<CurCheck>,
    /// Settings read from the export, to prefill the form.
    pub detected: Option<CurConfig>,
    /// Billing periods with data in the bucket, e.g. `2026-10`.
    pub billing_periods: Vec<String>,
}

impl CurValidation {
    pub fn ok(&self) -> bool {
        !self.checks.iter().any(|c| c.status == CheckStatus::Error)
    }

    pub(crate) fn push(&mut self, status: CheckStatus, message: impl Into<String>) {
        self.checks.push(CurCheck {
            status,
            message: message.into(),
        });
    }
}

/// Outcome of an incremental CUR ingest.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CurReport {
    pub periods_found: usize,
    pub periods_ingested: Vec<String>,
    pub files_downloaded: usize,
    pub bytes_downloaded: u64,
    pub rows: u64,
}
