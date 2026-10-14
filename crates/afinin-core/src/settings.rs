// SPDX-License-Identifier: MIT OR Apache-2.0

//! User settings (FR-52). Defaults keep the AWS cost of running Afinín low (NFR-30).

use serde::{Deserialize, Serialize};

use crate::cost::CostMetric;
use crate::error::{Error, Result};

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sync_interval_hours: 24,
            daily_history_days: 60,
            monthly_history_months: 12,
            cost_tag_keys: Vec::new(),
            default_metric: CostMetric::Unblended,
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
        Ok(())
    }
}
