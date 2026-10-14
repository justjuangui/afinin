// SPDX-License-Identifier: MIT OR Apache-2.0

//! Inventory sync: scans regions concurrently and stores each one (FR-20, NFR-10, NFR-13).

use std::sync::Mutex;

use aws_config::{Region, SdkConfig};
use futures::StreamExt;
use serde::Serialize;

use super::Collected;
use super::collect::{self, Ctx, HOME_REGION};
use crate::error::{Error, Result};
use crate::storage::Storage;

/// Regions scanned at the same time for one account.
const REGION_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct InventoryReport {
    pub regions: usize,
    pub resources: usize,
    pub warnings: Vec<String>,
}

fn regional(config: &SdkConfig, region: &str) -> SdkConfig {
    config
        .to_builder()
        .region(Region::new(region.to_owned()))
        .build()
}

/// Scans `regions` (all enabled ones when empty) for one connected account.
pub async fn sync_inventory(
    db: &Mutex<Storage>,
    config: &SdkConfig,
    source_account: &str,
    regions: &[String],
) -> Result<InventoryReport> {
    let identity = aws_sdk_sts::Client::new(config)
        .get_caller_identity()
        .send()
        .await
        .map_err(Error::aws)?;
    let account = identity
        .account()
        .ok_or_else(|| Error::Aws("STS returned no account ID".into()))?
        .to_owned();

    let regions = if regions.is_empty() {
        collect::enabled_regions(&regional(config, HOME_REGION)).await?
    } else {
        regions.to_vec()
    };

    let mut report = InventoryReport {
        regions: regions.len(),
        ..Default::default()
    };
    let mut scans: Vec<(String, Collected)> = futures::stream::iter(regions.iter().cloned())
        .map(|region| {
            let account = account.clone();
            async move {
                let cfg = regional(config, &region);
                let found = collect::collect_region(&Ctx {
                    config: &cfg,
                    account: &account,
                    region: &region,
                })
                .await;
                (region, found)
            }
        })
        .buffer_unordered(REGION_CONCURRENCY)
        .collect()
        .await;
    let home = regional(config, HOME_REGION);
    let global = collect::collect_global(&Ctx {
        config: &home,
        account: &account,
        region: HOME_REGION,
    })
    .await;
    scans.push(("global".into(), global));

    // Tags from every region's tagging API apply to every scan (S3 buckets
    // are listed globally but tagged in their own region).
    let all_tags: std::collections::HashMap<_, _> = scans
        .iter_mut()
        .flat_map(|(_, c)| std::mem::take(&mut c.tags_by_arn))
        .collect();
    let mut kept = Vec::new();
    for (region, mut found) in scans {
        found.tags_by_arn = all_tags.clone();
        found.apply_tags();
        found.tags_by_arn.clear();
        report.resources += found.resources.len();
        report.warnings.extend(found.warnings.iter().cloned());
        db.lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace_inventory(source_account, &region, &found)?;
        kept.push(region);
    }
    db.lock()
        .unwrap_or_else(|e| e.into_inner())
        .prune_inventory(source_account, &kept)?;
    report.warnings.sort();
    Ok(report)
}
