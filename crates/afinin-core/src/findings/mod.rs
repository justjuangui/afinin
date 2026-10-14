// SPDX-License-Identifier: MIT OR Apache-2.0

//! Findings: waste, anomalies and patterns (FR-30..FR-34).
//!
//! Findings are computed on demand from local data; only their state
//! (dismissed, snoozed) and first-seen date are stored.

pub mod anomalies;
pub mod commitments;
pub mod pricing;
pub mod rules;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::inventory::ResourceType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    /// From an estimated monthly amount (savings or excess spend).
    pub fn from_amount(usd: f64) -> Self {
        if usd >= 100.0 {
            Self::High
        } else if usd >= 20.0 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Waste,
    Anomaly,
    Pattern,
}

/// Where a savings figure comes from (FR-34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingsBasis {
    /// What the resource actually cost over the last 30 days (CUR).
    Actual,
    /// Estimate from public prices (Price List API or built-in list price).
    PublicPrice,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingResource {
    pub arn: String,
    pub label: String,
    pub resource_type: Option<ResourceType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FindingStatus {
    Open,
    Dismissed {
        note: Option<String>,
    },
    /// Hidden until the date (`YYYY-MM-DD`).
    Snoozed {
        until: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// Stable across runs: rule + subject.
    pub id: String,
    pub rule: &'static str,
    pub kind: FindingKind,
    pub severity: Severity,
    pub title: String,
    pub summary: String,
    pub how_to_fix: String,
    pub account_id: String,
    pub region: String,
    pub resources: Vec<FindingResource>,
    pub monthly_savings: Option<f64>,
    pub savings_basis: Option<SavingsBasis>,
    /// Anomalies: spend above baseline.
    pub impact: Option<f64>,
    /// Facts behind the finding, shown in the UI and given to the LLM.
    pub evidence: BTreeMap<String, serde_json::Value>,
    pub first_seen: Option<String>,
    pub status: FindingStatus,
}

impl Finding {
    pub(crate) fn new(rule: &'static str, kind: FindingKind, subject: &str) -> Self {
        Self {
            id: format!("{rule}:{subject}"),
            rule,
            kind,
            severity: Severity::Low,
            title: String::new(),
            summary: String::new(),
            how_to_fix: String::new(),
            account_id: String::new(),
            region: String::new(),
            resources: Vec::new(),
            monthly_savings: None,
            savings_basis: None,
            impact: None,
            evidence: BTreeMap::new(),
            first_seen: None,
            status: FindingStatus::Open,
        }
    }

    pub(crate) fn evidence(mut self, key: &str, value: impl Serialize) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.evidence.insert(key.to_owned(), v);
        }
        self
    }
}

/// Anomaly sensitivity (FR-30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Low,
    Medium,
    High,
}

impl Sensitivity {
    /// (standard deviations above baseline, minimum daily excess in USD).
    pub fn thresholds(self) -> (f64, f64) {
        match self {
            Self::Low => (4.0, 25.0),
            Self::Medium => (3.0, 10.0),
            Self::High => (2.0, 5.0),
        }
    }
}
