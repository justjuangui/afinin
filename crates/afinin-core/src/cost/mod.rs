// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cost analysis, tier 1: AWS Cost Explorer (FR-10..FR-16).
//!
//! Data is synced into the local database and every dashboard query runs
//! locally, so views are fast, work offline and cost nothing (FR-14, NFR-21).

pub mod csv;
pub mod dates;
pub mod source;
pub mod sync;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Cost metric (FR-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostMetric {
    Unblended,
    /// Spreads Savings Plans / RI upfront fees over their term.
    Amortized,
    /// Unblended, net of discounts.
    NetUnblended,
}

impl CostMetric {
    pub const ALL: [CostMetric; 3] = [Self::Unblended, Self::Amortized, Self::NetUnblended];

    /// Local column name.
    pub(crate) fn column(self) -> &'static str {
        match self {
            Self::Unblended => "unblended",
            Self::Amortized => "amortized",
            Self::NetUnblended => "net_unblended",
        }
    }

    /// Cost Explorer metric name.
    pub(crate) fn ce_name(self) -> &'static str {
        match self {
            Self::Unblended => "UnblendedCost",
            Self::Amortized => "AmortizedCost",
            Self::NetUnblended => "NetUnblendedCost",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Daily,
    Monthly,
}

impl Granularity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "DAILY",
            Self::Monthly => "MONTHLY",
        }
    }
}

/// Second grouping dimension of a synced dataset. Every dataset is also
/// grouped by linked account, which keeps account filtering available
/// everywhere and lets us de-duplicate overlapping sources.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Dimension {
    Service,
    Region,
    UsageType,
    /// Cost allocation tag key.
    Tag(String),
}

impl Dimension {
    /// Stable identifier used in the database.
    pub(crate) fn id(&self) -> String {
        match self {
            Self::Service => "SERVICE".into(),
            Self::Region => "REGION".into(),
            Self::UsageType => "USAGE_TYPE".into(),
            Self::Tag(k) => format!("TAG:{k}"),
        }
    }
}

/// What a breakdown is grouped by (FR-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "key", rename_all = "snake_case")]
pub enum GroupBy {
    Account,
    Service,
    Region,
    UsageType,
    Tag(String),
    /// Individual resource; needs CUR 2.0 data.
    Resource,
}

impl GroupBy {
    /// Dataset holding the data for this grouping.
    pub(crate) fn dataset(&self) -> Dimension {
        match self {
            // `Resource` is served from CUR, not from a Cost Explorer dataset.
            Self::Account | Self::Service | Self::Resource => Dimension::Service,
            Self::Region => Dimension::Region,
            Self::UsageType => Dimension::UsageType,
            Self::Tag(k) => Dimension::Tag(k.clone()),
        }
    }
}

/// Filters applied to a cost query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostFilter {
    /// AWS account IDs; empty means all.
    #[serde(default)]
    pub linked_accounts: Vec<String>,
    /// Service names; empty means all. With Cost Explorer data this is only
    /// available when grouping by service or account.
    #[serde(default)]
    pub services: Vec<String>,
}

/// A cost breakdown request over local data (FR-11, FR-12, FR-13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostQuery {
    pub workspace_id: String,
    /// Inclusive, `YYYY-MM-DD`.
    pub start: String,
    /// Exclusive, `YYYY-MM-DD`.
    pub end: String,
    pub granularity: Granularity,
    pub metric: CostMetric,
    pub group_by: GroupBy,
    #[serde(default)]
    pub filter: CostFilter,
}

impl CostQuery {
    pub(crate) fn validate(&self) -> Result<()> {
        dates::parse(&self.start)?;
        dates::parse(&self.end)?;
        if self.start >= self.end {
            return Err(Error::Other("start date must be before end date".into()));
        }
        if !self.filter.services.is_empty()
            && (self.group_by.dataset() != Dimension::Service || self.group_by == GroupBy::Resource)
        {
            return Err(Error::Other(
                "with Cost Explorer data, the service filter is only available when grouping \
                 by service or account; enable CUR for full filtering"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// One row of a breakdown: a group's cost per period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostGroup {
    pub key: String,
    pub label: String,
    /// Aligned with [`CostSeries::periods`].
    pub values: Vec<f64>,
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostSeries {
    /// Period start dates, `YYYY-MM-DD`.
    pub periods: Vec<String>,
    /// Sorted by total, descending.
    pub groups: Vec<CostGroup>,
    pub period_totals: Vec<f64>,
    pub total: f64,
    pub unit: String,
    /// Periods whose data AWS still marks as estimated.
    pub estimated_periods: Vec<String>,
}

/// Change of each group between two periods (FR-12).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostComparisonRow {
    pub key: String,
    pub label: String,
    pub current: f64,
    pub previous: f64,
    pub delta: f64,
    /// `None` when the previous value is zero.
    pub delta_pct: Option<f64>,
}

/// Headline numbers for the dashboard (FR-10).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostOverview {
    pub metric: CostMetric,
    pub unit: String,
    /// Month to date (data through yesterday).
    pub month_to_date: f64,
    /// Same days of the previous month.
    pub previous_month_to_date: f64,
    pub month_over_month_pct: Option<f64>,
    pub last_month_total: f64,
    /// Month-to-date actuals plus the AWS forecast for the rest of the month.
    /// `None` when filtered or when AWS has no forecast.
    pub forecast_month_end: Option<f64>,
    /// Latest day with data.
    pub data_through: Option<String>,
}

/// A value to filter by, e.g. an account or a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DimensionValue {
    pub key: String,
    pub label: String,
}

pub(crate) fn pct_change(current: f64, previous: f64) -> Option<f64> {
    (previous.abs() > f64::EPSILON).then(|| (current - previous) / previous.abs() * 100.0)
}
