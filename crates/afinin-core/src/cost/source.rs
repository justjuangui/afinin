// SPDX-License-Identifier: MIT OR Apache-2.0

//! Access to Cost Explorer behind a trait, so sync logic is testable offline.

use std::future::Future;

use aws_config::{Region, SdkConfig, retry::RetryConfig};
use aws_sdk_costexplorer::Client;
use aws_sdk_costexplorer::error::ProvideErrorMetadata;
use aws_sdk_costexplorer::types::{DateInterval, GroupDefinition, GroupDefinitionType};

use super::{CostMetric, Dimension, Granularity};
use crate::error::{Error, Result};

/// Cost Explorer charges per request (FR-14, NFR-30).
pub const CE_REQUEST_COST_USD: f64 = 0.01;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostRequest {
    pub granularity: Granularity,
    pub dimension: Dimension,
    /// Inclusive, `YYYY-MM-DD`.
    pub start: String,
    /// Exclusive, `YYYY-MM-DD`.
    pub end: String,
    pub next_token: Option<String>,
}

/// Cost of one (period, linked account, dimension value).
#[derive(Debug, Clone, PartialEq)]
pub struct CostRow {
    pub period_start: String,
    pub linked_account: String,
    pub key: String,
    pub unblended: f64,
    pub amortized: f64,
    pub net_unblended: f64,
    pub unit: String,
    pub estimated: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CostPage {
    pub rows: Vec<CostRow>,
    /// Linked account ID → account name, when AWS provides it.
    pub account_names: Vec<(String, String)>,
    pub next_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    pub amount: f64,
    pub unit: String,
}

pub trait CostSource {
    fn cost_and_usage(&self, req: &CostRequest) -> impl Future<Output = Result<CostPage>> + Send;

    /// Forecast for `[start, end)`; `None` when AWS has too little history.
    fn forecast(
        &self,
        metric: CostMetric,
        start: &str,
        end: &str,
    ) -> impl Future<Output = Result<Option<Forecast>>> + Send;
}

/// Cost Explorer through the AWS SDK. Only `Get*` operations are used.
pub struct CostExplorer {
    client: Client,
}

impl CostExplorer {
    pub fn new(config: &SdkConfig) -> Self {
        let conf = aws_sdk_costexplorer::config::Builder::from(config)
            // Cost Explorer has a single endpoint in the aws partition.
            .region(Region::new("us-east-1"))
            // Backoff on throttling (NFR-13).
            .retry_config(RetryConfig::standard().with_max_attempts(5))
            .build();
        Self {
            client: Client::from_conf(conf),
        }
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }
}

fn interval(start: &str, end: &str) -> Result<DateInterval> {
    DateInterval::builder()
        .start(start)
        .end(end)
        .build()
        .map_err(|e| Error::Other(e.to_string()))
}

fn group(kind: GroupDefinitionType, key: &str) -> GroupDefinition {
    GroupDefinition::builder().r#type(kind).key(key).build()
}

/// Turns SDK errors into actionable messages.
fn ce_error<E>(err: aws_sdk_costexplorer::error::SdkError<E>) -> Error
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match err.as_service_error().and_then(|e| e.code()) {
        Some("AccessDeniedException") => Error::Aws(
            "access denied to Cost Explorer: the role needs ce:GetCostAndUsage and \
             ce:GetCostForecast (see policies/afinin-readonly.json)"
                .into(),
        ),
        Some("OptInRequired") | Some("DataUnavailableException") => {
            Error::Aws("Cost Explorer has no data for this account yet (it can take 24 hours after enabling it)".into())
        }
        _ => Error::aws(err),
    }
}

fn amount(
    metrics: Option<&std::collections::HashMap<String, aws_sdk_costexplorer::types::MetricValue>>,
    metric: CostMetric,
) -> (f64, Option<String>) {
    metrics
        .and_then(|m| m.get(metric.ce_name()))
        .map(|v| {
            (
                v.amount().and_then(|a| a.parse().ok()).unwrap_or(0.0),
                v.unit().map(str::to_owned),
            )
        })
        .unwrap_or((0.0, None))
}

impl CostSource for CostExplorer {
    async fn cost_and_usage(&self, req: &CostRequest) -> Result<CostPage> {
        let second = match &req.dimension {
            Dimension::Service => group(GroupDefinitionType::Dimension, "SERVICE"),
            Dimension::Region => group(GroupDefinitionType::Dimension, "REGION"),
            Dimension::UsageType => group(GroupDefinitionType::Dimension, "USAGE_TYPE"),
            Dimension::Tag(k) => group(GroupDefinitionType::Tag, k),
        };
        let mut call = self
            .client
            .get_cost_and_usage()
            .time_period(interval(&req.start, &req.end)?)
            .granularity(match req.granularity {
                Granularity::Daily => aws_sdk_costexplorer::types::Granularity::Daily,
                Granularity::Monthly => aws_sdk_costexplorer::types::Granularity::Monthly,
            })
            .group_by(group(GroupDefinitionType::Dimension, "LINKED_ACCOUNT"))
            .group_by(second)
            .set_next_page_token(req.next_token.clone());
        for m in CostMetric::ALL {
            call = call.metrics(m.ce_name());
        }
        let out = call.send().await.map_err(ce_error)?;

        let tag_prefix = match &req.dimension {
            Dimension::Tag(k) => Some(format!("{k}$")),
            _ => None,
        };
        let mut rows = Vec::new();
        for result in out.results_by_time() {
            let Some(period_start) = result.time_period().map(|p| p.start().to_owned()) else {
                continue;
            };
            for g in result.groups() {
                let [linked_account, key] = g.keys() else {
                    continue;
                };
                // Tag values arrive as `key$value`; an empty value means untagged.
                let key = match &tag_prefix {
                    Some(p) => key.strip_prefix(p.as_str()).unwrap_or(key).to_owned(),
                    None => key.clone(),
                };
                let (unblended, unit) = amount(g.metrics(), CostMetric::Unblended);
                let (amortized, _) = amount(g.metrics(), CostMetric::Amortized);
                let (net_unblended, _) = amount(g.metrics(), CostMetric::NetUnblended);
                rows.push(CostRow {
                    period_start: period_start.clone(),
                    linked_account: linked_account.clone(),
                    key,
                    unblended,
                    amortized,
                    net_unblended,
                    unit: unit.unwrap_or_else(|| "USD".into()),
                    estimated: result.estimated(),
                });
            }
        }
        let account_names = out
            .dimension_value_attributes()
            .iter()
            .filter_map(|d| {
                let name = d.attributes()?.get("description")?;
                Some((d.value()?.to_owned(), name.clone()))
            })
            .collect();
        Ok(CostPage {
            rows,
            account_names,
            next_token: out.next_page_token().map(str::to_owned),
        })
    }

    async fn forecast(
        &self,
        metric: CostMetric,
        start: &str,
        end: &str,
    ) -> Result<Option<Forecast>> {
        let ce_metric = match metric {
            CostMetric::Unblended => aws_sdk_costexplorer::types::Metric::UnblendedCost,
            CostMetric::Amortized => aws_sdk_costexplorer::types::Metric::AmortizedCost,
            CostMetric::NetUnblended => aws_sdk_costexplorer::types::Metric::NetUnblendedCost,
        };
        let result = self
            .client
            .get_cost_forecast()
            .time_period(interval(start, end)?)
            .metric(ce_metric)
            // Daily, summed here: with monthly granularity AWS answers with
            // its estimate for the whole month, actuals included, which the
            // overview then added to the actuals again (twice the spend so far).
            .granularity(aws_sdk_costexplorer::types::Granularity::Daily)
            .send()
            .await;
        match result {
            Ok(out) => {
                let days = out.forecast_results_by_time();
                if days.is_empty() {
                    return Ok(None);
                }
                let amount = days
                    .iter()
                    .filter_map(|d| d.mean_value()?.parse::<f64>().ok())
                    .sum();
                let unit = out
                    .total()
                    .and_then(|t| t.unit())
                    .unwrap_or("USD")
                    .to_owned();
                Ok(Some(Forecast { amount, unit }))
            }
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.is_data_unavailable_exception()) =>
            {
                Ok(None)
            }
            Err(e) => Err(ce_error(e)),
        }
    }
}
