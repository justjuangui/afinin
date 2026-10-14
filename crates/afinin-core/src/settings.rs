// SPDX-License-Identifier: MIT OR Apache-2.0

//! User settings (FR-52). Defaults keep the AWS cost of running Afinín low (NFR-30).

use serde::{Deserialize, Serialize};

use crate::cost::CostMetric;
use crate::error::{Error, Result};
use crate::findings::Sensitivity;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Hours between automatic syncs; 0 disables background sync (FR-50).
    pub sync_interval_hours: u32,
    /// Days of daily cost data fetched on the first sync. The previous month is
    /// always included so month-over-month comparisons work.
    pub daily_history_days: u32,
    /// Months of monthly cost history fetched on the first sync.
    pub monthly_history_months: u32,
    /// Cost allocation tag keys to break costs down by (FR-11).
    pub cost_tag_keys: Vec<String>,
    pub default_metric: CostMetric,
    /// Discover resources during sync (FR-20).
    pub inventory_enabled: bool,
    /// Regions to scan; empty means every region enabled in the account (FR-52).
    pub inventory_regions: Vec<String>,
    /// Cost anomaly sensitivity (FR-30).
    pub anomaly_sensitivity: Sensitivity,
    /// Snapshots older than this are flagged (FR-31).
    pub snapshot_age_days: u32,
    /// Refresh Savings Plans / RI coverage weekly (4 Cost Explorer requests, $0.04).
    pub commitments_enabled: bool,
    /// Import AWS Cost Anomaly Detection findings on each sync ($0.01 per request).
    pub import_aws_anomalies: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sync_interval_hours: 24,
            daily_history_days: 60,
            monthly_history_months: 12,
            cost_tag_keys: Vec::new(),
            default_metric: CostMetric::Unblended,
            inventory_enabled: true,
            inventory_regions: Vec::new(),
            anomaly_sensitivity: Sensitivity::Medium,
            snapshot_age_days: 90,
            commitments_enabled: true,
            import_aws_anomalies: false,
        }
    }
}

impl Settings {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(1..=365).contains(&self.daily_history_days) {
            return Err(Error::Other(
                "daily history must be between 1 and 365 days".into(),
            ));
        }
        // Cost Explorer keeps at most 14 months of history by default.
        if !(1..=14).contains(&self.monthly_history_months) {
            return Err(Error::Other(
                "monthly history must be between 1 and 14 months".into(),
            ));
        }
        if self
            .cost_tag_keys
            .iter()
            .any(|k| k.trim().is_empty() || k.contains('$'))
        {
            return Err(Error::Other(
                "tag keys must be non-empty and cannot contain `$`".into(),
            ));
        }
        if self.inventory_regions.iter().any(|r| {
            r.is_empty()
                || !r
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }) {
            return Err(Error::Other("regions must look like `us-east-1`".into()));
        }
        Ok(())
    }
}
