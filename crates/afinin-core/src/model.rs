// SPDX-License-Identifier: MIT OR Apache-2.0

//! Domain types persisted by the app and exposed to the UI.

use serde::{Deserialize, Serialize};

use crate::profiles::ProfileKind;

/// A group of accounts, e.g. "Company A" or "Personal" (FR-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

/// An AWS account connected through a local profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub workspace_id: String,
    /// Display name chosen by the user.
    pub name: String,
    /// AWS CLI profile used to obtain credentials.
    pub profile: String,
    pub kind: ProfileKind,
    /// Filled in after the first successful connection test.
    pub aws_account_id: Option<String>,
    pub last_test: Option<ConnectionTest>,
    pub created_at: String,
    /// Latest data sync (FR-50).
    pub sync: Option<SyncStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Running,
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub state: SyncState,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub last_success_at: Option<String>,
    pub error: Option<String>,
}

/// Result of a "Test connection" check (FR-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionTest {
    pub ok: bool,
    pub tested_at: String,
    pub aws_account_id: Option<String>,
    pub arn: Option<String>,
    pub error: Option<String>,
}

pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of UTC now cannot fail")
}
