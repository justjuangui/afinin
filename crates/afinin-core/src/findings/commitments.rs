// SPDX-License-Identifier: MIT OR Apache-2.0

//! Savings Plans / Reserved Instances coverage and utilization (FR-32) and the
//! optional import of AWS Cost Anomaly Detection findings (FR-30). Each Cost
//! Explorer request costs $0.01, so callers refresh these sparingly.

use aws_config::SdkConfig;
use aws_sdk_costexplorer::Client;
use aws_sdk_costexplorer::types::{
    AnomalyDateInterval, DateInterval, Dimension, DimensionValues, Expression, Granularity,
};
use serde::{Deserialize, Serialize};

use crate::cost::source::CostExplorer;
use crate::error::{Error, Result};
use crate::inventory::collect::aws_err;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommitmentSummary {
    /// `savings_plans` or `reservations`.
    pub kind: String,
    /// Reservations: the Cost Explorer service they cover (one row each).
    #[serde(default)]
    pub service: Option<String>,
    pub coverage_pct: Option<f64>,
    pub utilization_pct: Option<f64>,
    /// On-demand spend not covered by commitments.
    pub on_demand_cost: Option<f64>,
    /// Commitment paid for but not used.
    pub unused_commitment: Option<f64>,
    pub start: String,
    pub end: String,
}

/// An AWS Cost Anomaly Detection finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AwsAnomaly {
    pub id: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub impact: f64,
    pub dimension: Option<String>,
    /// e.g. "Amazon EC2 · us-east-1 · 111122223333 · BoxUsage:m5.large"
    pub root_causes: Vec<String>,
}

fn pct(s: Option<&str>) -> Option<f64> {
    s.and_then(|v| v.parse().ok())
}

fn interval(start: &str, end: &str) -> Result<DateInterval> {
    DateInterval::builder()
        .start(start)
        .end(end)
        .build()
        .map_err(|e| Error::Other(e.to_string()))
}

/// Fetches both summaries; returns them and the number of billable requests made.
pub async fn fetch(
    config: &SdkConfig,
    start: &str,
    end: &str,
) -> (Vec<CommitmentSummary>, u32, Vec<String>) {
    let client = CostExplorer::new(config).client().clone();
    let mut calls = 0;
    let mut warnings = Vec::new();
    let mut out = Vec::new();

    let mut sp = CommitmentSummary {
        kind: "savings_plans".into(),
        start: start.into(),
        end: end.into(),
        ..Default::default()
    };
    match savings_plans(&client, start, end, &mut sp, &mut calls).await {
        Ok(()) => out.push(sp),
        Err(e) => warnings.push(format!("Savings Plans: {e}")),
    }
    // Cost Explorer reports reservations for one service per request and
    // defaults to EC2, so an RDS or ElastiCache reservation would never show.
    for service in RI_SERVICES {
        let mut ri = CommitmentSummary {
            kind: "reservations".into(),
            service: Some((*service).into()),
            start: start.into(),
            end: end.into(),
            ..Default::default()
        };
        match reservations(&client, start, end, service, &mut ri, &mut calls).await {
            Ok(true) => out.push(ri),
            Ok(false) => {}
            Err(e) => warnings.push(format!("Reservations ({service}): {e}")),
        }
    }
    (out, calls, warnings)
}

async fn savings_plans(
    client: &Client,
    start: &str,
    end: &str,
    s: &mut CommitmentSummary,
    calls: &mut u32,
) -> Result<()> {
    *calls += 1;
    match client
        .get_savings_plans_utilization()
        .time_period(interval(start, end)?)
        .send()
        .await
    {
        Ok(out) => {
            if let Some(t) = out.total() {
                s.utilization_pct = t
                    .utilization()
                    .and_then(|u| pct(u.utilization_percentage()));
                s.unused_commitment = t.utilization().and_then(|u| pct(u.unused_commitment()));
            }
        }
        // No Savings Plans in the account: coverage still matters.
        Err(e)
            if e.as_service_error()
                .is_some_and(|s| s.is_data_unavailable_exception()) => {}
        Err(e) => return Err(aws_err(e)),
    }
    *calls += 1;
    let out = client
        .get_savings_plans_coverage()
        .time_period(interval(start, end)?)
        .granularity(Granularity::Monthly)
        .send()
        .await
        .map_err(aws_err)?;
    let (mut covered, mut on_demand) = (0.0, 0.0);
    for c in out
        .savings_plans_coverages()
        .iter()
        .filter_map(|c| c.coverage())
    {
        covered += pct(c.spend_covered_by_savings_plans()).unwrap_or(0.0);
        on_demand += pct(c.on_demand_cost()).unwrap_or(0.0);
    }
    s.on_demand_cost = Some(on_demand);
    if covered + on_demand > 0.0 {
        s.coverage_pct = Some(covered / (covered + on_demand) * 100.0);
    }
    Ok(())
}

/// Services Cost Explorer reports reservations for, EC2 first.
pub const RI_SERVICES: &[&str] = &[
    "Amazon Elastic Compute Cloud - Compute",
    "Amazon Relational Database Service",
    "Amazon ElastiCache",
    "Amazon Redshift",
    "Amazon Elasticsearch Service",
];

fn service_filter(service: &str) -> Expression {
    Expression::builder()
        .dimensions(
            DimensionValues::builder()
                .key(Dimension::Service)
                .values(service)
                .build(),
        )
        .build()
}

/// Reservation utilization and coverage of one service. Returns whether the
/// row is worth showing: the service has reservations, or it is EC2 (its
/// coverage says how much on-demand compute could be reserved).
async fn reservations(
    client: &Client,
    start: &str,
    end: &str,
    service: &str,
    s: &mut CommitmentSummary,
    calls: &mut u32,
) -> Result<bool> {
    *calls += 1;
    let has_reservations = match client
        .get_reservation_utilization()
        .time_period(interval(start, end)?)
        .filter(service_filter(service))
        .send()
        .await
    {
        Ok(out) => match out.total() {
            Some(t) => {
                s.utilization_pct = pct(t.utilization_percentage());
                // Cost Explorer doesn't report unused RI cost directly; utilization says enough.
                true
            }
            None => false,
        },
        Err(e)
            if e.as_service_error()
                .is_some_and(|s| s.is_data_unavailable_exception()) =>
        {
            false
        }
        Err(e) => return Err(aws_err(e)),
    };
    if !has_reservations && service != RI_SERVICES[0] {
        return Ok(false);
    }
    *calls += 1;
    match client
        .get_reservation_coverage()
        .time_period(interval(start, end)?)
        .filter(service_filter(service))
        .send()
        .await
    {
        Ok(out) => {
            if let Some(t) = out.total() {
                s.coverage_pct = t
                    .coverage_hours()
                    .and_then(|h| pct(h.coverage_hours_percentage()));
                s.on_demand_cost = t.coverage_cost().and_then(|c| pct(c.on_demand_cost()));
            }
        }
        Err(e)
            if e.as_service_error()
                .is_some_and(|s| s.is_data_unavailable_exception()) => {}
        Err(e) => return Err(aws_err(e)),
    }
    Ok(true)
}

/// AWS Cost Anomaly Detection findings since `start` (one billable request per page).
pub async fn aws_anomalies(config: &SdkConfig, start: &str) -> Result<(Vec<AwsAnomaly>, u32)> {
    let client = CostExplorer::new(config).client().clone();
    let interval = AnomalyDateInterval::builder()
        .start_date(start)
        .build()
        .map_err(|e| Error::Other(e.to_string()))?;
    let mut out = Vec::new();
    let mut calls = 0;
    let mut token = None;
    loop {
        calls += 1;
        let page = client
            .get_anomalies()
            .date_interval(interval.clone())
            .set_next_page_token(token)
            .send()
            .await
            .map_err(aws_err)?;
        for a in page.anomalies() {
            out.push(AwsAnomaly {
                id: a.anomaly_id().to_owned(),
                start: a.anomaly_start_date().map(str::to_owned),
                end: a.anomaly_end_date().map(str::to_owned),
                impact: a.impact().map(|i| i.total_impact()).unwrap_or(0.0),
                dimension: a.dimension_value().map(str::to_owned),
                root_causes: a
                    .root_causes()
                    .iter()
                    .map(|c| {
                        [
                            c.service(),
                            c.region(),
                            c.linked_account_name().or(c.linked_account()),
                            c.usage_type(),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ")
                    })
                    .collect(),
            });
        }
        token = page.next_page_token().map(str::to_owned);
        if token.is_none() {
            break;
        }
    }
    Ok((out, calls))
}
