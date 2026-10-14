// SPDX-License-Identifier: MIT OR Apache-2.0

//! Object storage behind a trait (S3 in production, a directory in tests) and
//! read-only validation of a Data Export.

use std::future::Future;
use std::path::Path;

use aws_config::{BehaviorVersion, Region, SdkConfig};

use super::{CheckStatus, CurConfig, CurValidation};
use crate::error::{Error, Result};
use crate::inventory::collect::aws_err;

/// S3 request and transfer prices used for the "what Afinín costs you" view.
pub const S3_LIST_COST_USD: f64 = 0.005 / 1000.0;
pub const S3_GET_COST_USD: f64 = 0.0004 / 1000.0;
pub const S3_TRANSFER_COST_PER_GB_USD: f64 = 0.09;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectInfo {
    pub key: String,
    pub etag: String,
    pub size: u64,
    /// RFC 3339; used to pick the latest manifest.
    pub last_modified: String,
}

pub trait ObjectStore {
    /// All objects under `prefix`, with the number of list requests made.
    fn list(&self, prefix: &str) -> impl Future<Output = Result<(Vec<ObjectInfo>, u32)>> + Send;
    fn read(&self, key: &str) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn download(&self, key: &str, dest: &Path) -> impl Future<Output = Result<u64>> + Send;
}

pub struct S3Store {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3Store {
    pub fn new(config: &SdkConfig, cur: &CurConfig) -> Self {
        let conf = aws_sdk_s3::config::Builder::from(config)
            .region(Region::new(cur.region.clone()))
            .build();
        Self {
            client: aws_sdk_s3::Client::from_conf(conf),
            bucket: cur.bucket.clone(),
        }
    }
}

impl ObjectStore for S3Store {
    async fn list(&self, prefix: &str) -> Result<(Vec<ObjectInfo>, u32)> {
        let pages = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .into_paginator()
            .send()
            .try_collect()
            .await
            .map_err(aws_err)?;
        let requests = pages.len() as u32;
        let objects = pages
            .iter()
            .flat_map(|p| p.contents())
            .filter_map(|o| {
                Some(ObjectInfo {
                    key: o.key()?.to_owned(),
                    etag: o.e_tag().unwrap_or_default().trim_matches('"').to_owned(),
                    size: o.size().unwrap_or(0).max(0) as u64,
                    last_modified: o
                        .last_modified()
                        .and_then(|d| d.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime).ok())
                        .unwrap_or_default(),
                })
            })
            .collect();
        Ok((objects, requests))
    }

    async fn read(&self, key: &str) -> Result<Vec<u8>> {
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(aws_err)?;
        let bytes = out
            .body
            .collect()
            .await
            .map_err(|e| Error::Aws(e.to_string()))?;
        Ok(bytes.into_bytes().to_vec())
    }

    async fn download(&self, key: &str, dest: &Path) -> Result<u64> {
        let bytes = self.read(key).await?;
        std::fs::write(dest, &bytes)?;
        Ok(bytes.len() as u64)
    }
}

/// Checks an export with read-only calls: Data Exports `GetExport`/`ListExports`
/// and an S3 listing of its destination.
pub async fn validate(config: &SdkConfig, wanted: &CurConfig) -> CurValidation {
    let mut v = CurValidation {
        checks: Vec::new(),
        detected: None,
        billing_periods: Vec::new(),
    };
    let exports = aws_sdk_bcmdataexports::Client::from_conf(
        aws_sdk_bcmdataexports::config::Builder::from(config)
            // Data Exports has a single endpoint.
            .region(Region::new("us-east-1"))
            .behavior_version(BehaviorVersion::latest())
            .build(),
    );

    let list = match exports
        .list_exports()
        .into_paginator()
        .send()
        .try_collect()
        .await
    {
        Ok(pages) => pages,
        Err(e) => {
            v.push(
                CheckStatus::Warning,
                format!(
                    "Could not list Data Exports ({}). Grant bcm-data-exports:ListExports and GetExport to let Afinín check the export settings.",
                    aws_err(e)
                ),
            );
            Vec::new()
        }
    };
    let reference = list
        .iter()
        .flat_map(|p| p.exports())
        .find(|e| e.export_name() == wanted.export_name);
    match (reference, list.is_empty()) {
        (Some(r), _) => match exports.get_export().export_arn(r.export_arn()).send().await {
            Ok(out) => check_export(&mut v, wanted, out.export()),
            Err(e) => v.push(CheckStatus::Warning, format!("Could not read the export: {}", aws_err(e))),
        },
        (None, false) => v.push(
            CheckStatus::Error,
            format!("No Data Export named `{}` in this account. Exports live in the management (payer) account.", wanted.export_name),
        ),
        (None, true) => {}
    }

    let cfg = v.detected.clone().unwrap_or_else(|| wanted.clone());
    let store = S3Store::new(config, &cfg);
    match store.list(&format!("{}data/", cfg.root())).await {
        Ok((objects, _)) => {
            let mut periods: Vec<String> = objects.iter().filter_map(|o| super::ingest::billing_period(&o.key)).collect();
            periods.sort();
            periods.dedup();
            if periods.is_empty() {
                v.push(
                    CheckStatus::Warning,
                    "The bucket is readable but has no data yet. AWS delivers the first export within 24 hours.",
                );
            } else {
                v.push(CheckStatus::Ok, format!("Found data for {} billing period(s).", periods.len()));
            }
            v.billing_periods = periods;
        }
        Err(e) => v.push(
            CheckStatus::Error,
            format!("Cannot list s3://{}/{}: {e}. The role needs s3:ListBucket and s3:GetObject on the bucket.", cfg.bucket, cfg.root()),
        ),
    }
    v
}

fn check_export(
    v: &mut CurValidation,
    wanted: &CurConfig,
    export: Option<&aws_sdk_bcmdataexports::types::Export>,
) {
    let Some(export) = export else { return };
    let query = export
        .data_query()
        .map(|q| q.query_statement().to_ascii_lowercase())
        .unwrap_or_default();
    if query.contains("from cost_and_usage_report") {
        v.push(CheckStatus::Ok, "Export uses the CUR 2.0 table.");
    } else {
        v.push(
            CheckStatus::Error,
            "Export is not a CUR 2.0 (COST_AND_USAGE_REPORT) export.",
        );
    }
    if query.contains("line_item_resource_id") {
        v.push(CheckStatus::Ok, "Resource IDs are included.");
    } else {
        v.push(
            CheckStatus::Warning,
            "Resource IDs are not included: costs per resource and on the graph won't be available. Enable INCLUDE_RESOURCES.",
        );
    }
    if let Some(s3) = export
        .destination_configurations()
        .and_then(|d| d.s3_destination())
    {
        if let Some(out) = s3.s3_output_configurations() {
            if out.format().as_str() == "PARQUET" {
                v.push(CheckStatus::Ok, "Format is Parquet.");
            } else {
                v.push(CheckStatus::Error, "Format must be Parquet.");
            }
        }
        v.detected = Some(CurConfig {
            account_id: wanted.account_id.clone(),
            bucket: s3.s3_bucket().to_owned(),
            prefix: s3.s3_prefix().to_owned(),
            export_name: export.name().to_owned(),
            region: s3.s3_region().to_owned(),
        });
    }
}
