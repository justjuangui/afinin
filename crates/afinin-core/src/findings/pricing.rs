// SPDX-License-Identifier: MIT OR Apache-2.0

//! Public prices for savings estimates (FR-34) when CUR has no actual cost.
//!
//! Prices come from the AWS Price List Query API (free) and are cached
//! locally; built-in us-east-1 list prices fill gaps. Estimates are always
//! labelled as public prices.

use std::collections::HashMap;

use aws_config::{Region, SdkConfig};
use aws_sdk_pricing::types::{Filter, FilterType};
use serde::Serialize;

use crate::error::Result;
use crate::inventory::collect::aws_err;

pub const HOURS_PER_MONTH: f64 = 730.0;

/// Built-in us-east-1 list prices, USD per month, for services billed per
/// item or per GB stored (not fetched from the Price List API).
pub mod builtin {
    /// Per secret.
    pub const SECRET: f64 = 0.40;
    /// Per customer-managed key, enabled or disabled.
    pub const KMS_KEY: f64 = 1.00;
    /// Per GB of ECR image storage.
    pub const ECR_GB: f64 = 0.10;
    /// Per GB of CloudWatch Logs storage (archived data).
    pub const LOGS_GB: f64 = 0.03;
    /// Per GB of EFS storage by class (Regional / One Zone).
    pub const EFS_STANDARD_GB: f64 = 0.30;
    pub const EFS_ONE_ZONE_GB: f64 = 0.16;
    pub const EFS_IA_GB: f64 = 0.016;
    pub const EFS_ONE_ZONE_IA_GB: f64 = 0.0133;
    pub const EFS_ARCHIVE_GB: f64 = 0.008;
    /// Per MiB/s of provisioned EFS throughput above the baseline that
    /// Standard storage includes.
    pub const EFS_PROVISIONED_MIBPS: f64 = 6.00;
    /// Throughput included per GiB of Standard storage (50 MiB/s per TiB).
    pub const EFS_BASELINE_MIBPS_PER_GIB: f64 = 50.0 / 1024.0;
    /// Per hosted zone (the first 25 of an account).
    pub const ROUTE53_ZONE: f64 = 0.50;
}

/// Monthly list price of a resource billed per item or per GB stored
/// (Secrets Manager, KMS, ECR, CloudWatch Logs, EFS, Route 53). Request,
/// ingestion and transfer charges come on top. None for other types.
pub fn platform_monthly(r: &crate::inventory::Resource) -> Option<f64> {
    use crate::inventory::ResourceType as T;
    let num = |k: &str| r.attrs.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let text = |k: &str| r.attrs.get(k).and_then(|v| v.as_str());
    match r.resource_type {
        // Secrets scheduled for deletion and keys pending deletion aren't billed.
        T::SecretsManagerSecret if text("deleted").is_none() => Some(builtin::SECRET),
        T::KmsKey if matches!(text("state"), Some("Enabled" | "Disabled")) => Some(builtin::KMS_KEY),
        // No size when its listing failed (e.g. ecr:DescribeImages denied):
        // no price either, rather than a confident $0.
        T::EcrRepository => Some(r.attrs.get("size_gib")?.as_f64()? * builtin::ECR_GB),
        T::LogGroup => Some(r.attrs.get("stored_gib")?.as_f64()? * builtin::LOGS_GB),
        T::EfsFileSystem => {
            let one_zone = r.attrs.get("one_zone").and_then(|v| v.as_bool()) == Some(true);
            let (standard, ia) = if one_zone {
                (builtin::EFS_ONE_ZONE_GB, builtin::EFS_ONE_ZONE_IA_GB)
            } else {
                (builtin::EFS_STANDARD_GB, builtin::EFS_IA_GB)
            };
            // Only throughput above what the stored data includes is billed.
            let throughput = if text("throughput_mode") == Some("provisioned") {
                let included = num("standard_gib") * builtin::EFS_BASELINE_MIBPS_PER_GIB;
                (num("provisioned_mibps") - included).max(0.0) * builtin::EFS_PROVISIONED_MIBPS
            } else {
                0.0
            };
            Some(
                num("standard_gib") * standard
                    + num("ia_gib") * ia
                    + num("archive_gib") * builtin::EFS_ARCHIVE_GB
                    + throughput,
            )
        }
        T::Route53HostedZone => Some(builtin::ROUTE53_ZONE),
        _ => None,
    }
}

/// A billable dimension we estimate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PriceItem {
    /// Per GB-month, by volume type (`gp3`, `gp2`, `io1`, `io2`, `st1`, `sc1`, `standard`).
    EbsStorage(String),
    /// Per provisioned IOPS-month (`gp3` above 3,000, `io1`, `io2`).
    EbsIops(String),
    /// gp3 throughput above 125 MiB/s, per MiB/s-month.
    EbsThroughput,
    /// Per GB-month of snapshot storage (standard tier).
    EbsSnapshot,
    /// Per hour for an idle public IPv4 address / Elastic IP.
    PublicIpv4Idle,
    NatGatewayHour,
    /// Per hour, by type (`application`, `network`, `gateway`).
    LoadBalancerHour(String),
}

impl PriceItem {
    pub fn key(&self) -> String {
        match self {
            Self::EbsStorage(t) => format!("ebs-storage:{t}"),
            Self::EbsIops(t) => format!("ebs-iops:{t}"),
            Self::EbsThroughput => "ebs-throughput:gp3".into(),
            Self::EbsSnapshot => "ebs-snapshot".into(),
            Self::PublicIpv4Idle => "ipv4-idle".into(),
            Self::NatGatewayHour => "nat-hour".into(),
            Self::LoadBalancerHour(t) => format!("elb-hour:{t}"),
        }
    }

    /// us-east-1 on-demand list prices, used until the Price List API answers.
    pub fn default_usd(&self) -> f64 {
        match self {
            Self::EbsStorage(t) => match t.as_str() {
                "gp3" => 0.08,
                "gp2" => 0.10,
                "io1" | "io2" => 0.125,
                "st1" => 0.045,
                "sc1" => 0.015,
                _ => 0.05,
            },
            Self::EbsIops(t) => match t.as_str() {
                "gp3" => 0.005,
                _ => 0.065,
            },
            Self::EbsThroughput => 0.04,
            Self::EbsSnapshot => 0.05,
            Self::PublicIpv4Idle => 0.005,
            Self::NatGatewayHour => 0.045,
            Self::LoadBalancerHour(t) => match t.as_str() {
                "gateway" => 0.0125,
                _ => 0.0225,
            },
        }
    }

    /// Price List query: service code, filters and the usage-type suffix to pick.
    fn query(&self) -> (&'static str, Vec<(&'static str, String)>, &'static str) {
        match self {
            Self::EbsStorage(t) => (
                "AmazonEC2",
                vec![
                    ("productFamily", "Storage".into()),
                    ("volumeApiName", t.clone()),
                ],
                "VolumeUsage",
            ),
            Self::EbsIops(t) => (
                "AmazonEC2",
                vec![
                    ("productFamily", "System Operation".into()),
                    ("volumeApiName", t.clone()),
                ],
                "IOPS",
            ),
            Self::EbsThroughput => (
                "AmazonEC2",
                vec![
                    ("productFamily", "Provisioned Throughput".into()),
                    ("volumeApiName", "gp3".into()),
                ],
                "Throughput",
            ),
            Self::EbsSnapshot => (
                "AmazonEC2",
                vec![("productFamily", "Storage Snapshot".into())],
                "EBS:SnapshotUsage",
            ),
            Self::PublicIpv4Idle => ("AmazonVPC", vec![], "PublicIPv4:IdleAddress"),
            Self::NatGatewayHour => (
                "AmazonEC2",
                vec![("productFamily", "NAT Gateway".into())],
                "NatGateway-Hours",
            ),
            Self::LoadBalancerHour(t) => (
                "AWSELB",
                vec![(
                    "productFamily",
                    match t.as_str() {
                        "network" => "Load Balancer-Network",
                        "gateway" => "Load Balancer-Gateway",
                        _ => "Load Balancer-Application",
                    }
                    .into(),
                )],
                "LoadBalancerUsage",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// AWS Price List API for the resource's region.
    PriceList,
    /// Built-in us-east-1 list price.
    BuiltIn,
    /// The resource's Lightsail bundle price, from the Lightsail API.
    LightsailBundle,
}

/// Cached prices by (region, item key).
#[derive(Debug, Clone, Default)]
pub struct PriceBook(pub HashMap<(String, String), f64>);

impl PriceBook {
    pub fn get(&self, region: &str, item: &PriceItem) -> (f64, PriceSource) {
        match self.0.get(&(region.to_owned(), item.key())) {
            Some(p) => (*p, PriceSource::PriceList),
            None => (item.default_usd(), PriceSource::BuiltIn),
        }
    }
}

/// On-demand USD price of the first product whose usage type contains `suffix`.
/// For tiered prices (io2 IOPS) the first tier wins: it's what most volumes pay.
pub(crate) fn parse_price(price_list: &[String], suffix: &str) -> Option<f64> {
    for raw in price_list {
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let usage = doc
            .pointer("/product/attributes/usagetype")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !usage.contains(suffix) {
            continue;
        }
        let Some(on_demand) = doc.pointer("/terms/OnDemand").and_then(|v| v.as_object()) else {
            continue;
        };
        let mut first_tier = None;
        let mut lowest: Option<f64> = None;
        for dim in on_demand
            .values()
            .filter_map(|t| t.get("priceDimensions").and_then(|v| v.as_object()))
            .flat_map(|d| d.values())
        {
            let Some(usd) = dim
                .pointer("/pricePerUnit/USD")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| *v > 0.0)
            else {
                continue;
            };
            if dim.get("beginRange").and_then(|v| v.as_str()) == Some("0") {
                first_tier = Some(usd);
            }
            lowest = Some(lowest.map_or(usd, |l| l.min(usd)));
        }
        if let Some(p) = first_tier.or(lowest) {
            return Some(p);
        }
    }
    None
}

/// Fetches current prices for `items` in `region`. Failures keep built-in prices.
pub async fn fetch(
    config: &SdkConfig,
    region: &str,
    items: &[PriceItem],
) -> Result<Vec<(PriceItem, f64)>> {
    // The Price List Query API is served from us-east-1.
    let client = aws_sdk_pricing::Client::from_conf(
        aws_sdk_pricing::config::Builder::from(config)
            .region(Region::new("us-east-1"))
            .build(),
    );
    let mut out = Vec::new();
    for item in items {
        let (service, filters, suffix) = item.query();
        let mut call = client.get_products().service_code(service).max_results(100);
        for (field, value) in filters
            .into_iter()
            .chain([("regionCode", region.to_owned())])
        {
            call = call.filters(
                Filter::builder()
                    .r#type(FilterType::TermMatch)
                    .field(field)
                    .value(value)
                    .build()
                    .map_err(|e| crate::error::Error::Other(e.to_string()))?,
            );
        }
        let pages = call
            .into_paginator()
            .send()
            .try_collect()
            .await
            .map_err(aws_err)?;
        let list: Vec<String> = pages
            .iter()
            .flat_map(|p| p.price_list().iter().cloned())
            .collect();
        if let Some(usd) = parse_price(&list, suffix) {
            out.push((item.clone(), usd));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_price_list_documents() {
        let doc = r#"{"product":{"attributes":{"usagetype":"EUW1-EBS:VolumeUsage.gp3"}},
            "terms":{"OnDemand":{"X":{"priceDimensions":{"Y":{"beginRange":"0","pricePerUnit":{"USD":"0.0880000000"}}}}}}}"#;
        let other = r#"{"product":{"attributes":{"usagetype":"EUW1-EBS:SnapshotUsage"}},
            "terms":{"OnDemand":{"X":{"priceDimensions":{"Y":{"pricePerUnit":{"USD":"0.05"}}}}}}}"#;
        assert_eq!(
            parse_price(&[other.into(), doc.into()], "VolumeUsage"),
            Some(0.088)
        );
        assert_eq!(parse_price(&[doc.into()], "NatGateway-Hours"), None);

        let mut book = PriceBook::default();
        book.0
            .insert(("eu-west-1".into(), "ebs-storage:gp3".into()), 0.088);
        assert_eq!(
            book.get("eu-west-1", &PriceItem::EbsStorage("gp3".into())),
            (0.088, PriceSource::PriceList)
        );
        assert_eq!(
            book.get("us-west-2", &PriceItem::EbsStorage("gp3".into())),
            (0.08, PriceSource::BuiltIn)
        );
    }
}
