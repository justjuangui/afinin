// SPDX-License-Identifier: MIT OR Apache-2.0

//! Incremental CUR 2.0 ingest.
//!
//! For each billing period the export's latest manifest lists the current data
//! files (older executions may still be in the bucket). A period is reloaded
//! only when that file set changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

use super::source::{
    ObjectInfo, ObjectStore, S3_GET_COST_USD, S3_LIST_COST_USD, S3_TRANSFER_COST_PER_GB_USD,
};
use super::{CurConfig, CurReport};
use crate::error::{Error, Result};
use crate::storage::Storage;

/// `…/BILLING_PERIOD=2026-10/…` → `2026-10`.
pub fn billing_period(key: &str) -> Option<String> {
    let rest = key.split("BILLING_PERIOD=").nth(1)?;
    let period: String = rest.chars().take_while(|c| *c != '/').collect();
    (period.len() == 7 && period.as_bytes()[4] == b'-').then_some(period)
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(rename = "dataFiles", default)]
    data_files: Vec<String>,
}

/// Current data files of one billing period.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PeriodPlan {
    pub period: String,
    pub files: Vec<ObjectInfo>,
    /// Identifies the file set; unchanged fingerprint → skip.
    pub fingerprint: String,
}

/// Groups objects per billing period, preferring the latest manifest's files.
pub(crate) async fn plan<S: ObjectStore>(
    store: &S,
    cfg: &CurConfig,
    objects: &[ObjectInfo],
) -> Result<(Vec<PeriodPlan>, u32)> {
    let mut gets = 0;
    let by_key: BTreeMap<&str, &ObjectInfo> = objects.iter().map(|o| (o.key.as_str(), o)).collect();
    let mut periods: BTreeMap<String, (Vec<&ObjectInfo>, Option<&ObjectInfo>)> = BTreeMap::new();
    for o in objects {
        let Some(period) = billing_period(&o.key) else {
            continue;
        };
        let entry = periods.entry(period).or_default();
        if o.key.contains("/metadata/") && o.key.ends_with("Manifest.json") {
            if entry.1.is_none_or(|m| m.last_modified < o.last_modified) {
                entry.1 = Some(o);
            }
        } else if o.key.contains("/data/") && o.key.ends_with(".parquet") {
            entry.0.push(o);
        }
    }

    let bucket_prefix = format!("s3://{}/", cfg.bucket);
    let mut plans = Vec::new();
    for (period, (all, manifest)) in periods {
        let mut files: Vec<ObjectInfo> = match manifest {
            Some(m) => {
                let raw = store.read(&m.key).await?;
                gets += 1;
                let manifest: Manifest = serde_json::from_slice(&raw)
                    .map_err(|e| Error::Other(format!("bad CUR manifest {}: {e}", m.key)))?;
                let listed: Vec<ObjectInfo> = manifest
                    .data_files
                    .iter()
                    .filter_map(|f| {
                        by_key
                            .get(f.strip_prefix(&bucket_prefix).unwrap_or(f))
                            .map(|o| (*o).clone())
                    })
                    .collect();
                // A manifest pointing at files not (yet) listed: fall back to everything.
                if listed.is_empty() {
                    all.into_iter().cloned().collect()
                } else {
                    listed
                }
            }
            None => all.into_iter().cloned().collect(),
        };
        if files.is_empty() {
            continue;
        }
        files.sort_by(|a, b| a.key.cmp(&b.key));
        let fingerprint = files
            .iter()
            .map(|f| format!("{}@{}", f.key, f.etag))
            .collect::<Vec<_>>()
            .join("|");
        plans.push(PeriodPlan {
            period,
            files,
            fingerprint,
        });
    }
    Ok((plans, gets))
}

/// Deletes downloaded files even when loading fails.
struct TempFiles(Vec<PathBuf>);

impl Drop for TempFiles {
    fn drop(&mut self) {
        for f in &self.0 {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// Downloads changed billing periods and loads them into the database.
pub async fn ingest<S: ObjectStore>(
    db: &Mutex<Storage>,
    store: &S,
    cfg: &CurConfig,
    tmp_dir: &Path,
) -> Result<CurReport> {
    let lock = || db.lock().unwrap_or_else(|e| e.into_inner());
    let source = cfg.account_id.as_str();
    let (objects, lists) = store.list(&cfg.root()).await?;
    let (plans, manifest_gets) = plan(store, cfg, &objects).await?;
    {
        let db = lock();
        db.record_api_call(
            source,
            "s3",
            "ListObjectsV2",
            lists as f64 * S3_LIST_COST_USD,
        )?;
        if manifest_gets > 0 {
            db.record_api_call(
                source,
                "s3",
                "GetObject",
                manifest_gets as f64 * S3_GET_COST_USD,
            )?;
        }
    }

    let mut report = CurReport {
        periods_found: plans.len(),
        ..Default::default()
    };
    std::fs::create_dir_all(tmp_dir)?;
    for plan in plans {
        if lock()
            .cur_period_fingerprint(source, &plan.period)?
            .as_deref()
            == Some(plan.fingerprint.as_str())
        {
            continue;
        }
        let mut tmp = TempFiles(Vec::new());
        let mut bytes = 0;
        for (i, f) in plan.files.iter().enumerate() {
            let dest = tmp_dir.join(format!("{}-{}-{i}.parquet", source, plan.period));
            tmp.0.push(dest.clone());
            bytes += store.download(&f.key, &dest).await?;
        }
        let rows = lock().load_cur_period(source, &plan.period, &tmp.0, &plan.fingerprint)?;
        lock().record_api_call(
            source,
            "s3",
            "GetObject",
            plan.files.len() as f64 * S3_GET_COST_USD
                + bytes as f64 / 1e9 * S3_TRANSFER_COST_PER_GB_USD,
        )?;
        report.files_downloaded += plan.files.len();
        report.bytes_downloaded += bytes;
        report.rows += rows;
        report.periods_ingested.push(plan.period);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::{CostFilter, CostMetric, CostQuery, Granularity, GroupBy};
    use crate::profiles::ProfileKind;

    /// A directory standing in for the bucket.
    struct DirStore(PathBuf);

    impl ObjectStore for DirStore {
        async fn list(&self, prefix: &str) -> Result<(Vec<ObjectInfo>, u32)> {
            let mut out = Vec::new();
            fn walk(base: &Path, dir: &Path, out: &mut Vec<ObjectInfo>) {
                for e in std::fs::read_dir(dir).unwrap() {
                    let p = e.unwrap().path();
                    if p.is_dir() {
                        walk(base, &p, out);
                    } else {
                        let meta = std::fs::metadata(&p).unwrap();
                        let content = std::fs::read(&p).unwrap();
                        out.push(ObjectInfo {
                            // S3 keys always use `/`, even on Windows.
                            key: p
                                .strip_prefix(base)
                                .unwrap()
                                .to_string_lossy()
                                .replace('\\', "/"),
                            etag: format!(
                                "{:x}",
                                content.len() * 31
                                    + content.iter().map(|b| *b as usize).sum::<usize>()
                            ),
                            size: meta.len(),
                            last_modified: format!("{:?}", meta.modified().unwrap()),
                        });
                    }
                }
            }
            walk(&self.0, &self.0, &mut out);
            out.retain(|o| o.key.starts_with(prefix));
            Ok((out, 1))
        }

        async fn read(&self, key: &str) -> Result<Vec<u8>> {
            Ok(std::fs::read(self.0.join(key))?)
        }

        async fn download(&self, key: &str, dest: &Path) -> Result<u64> {
            Ok(std::fs::copy(self.0.join(key), dest)?)
        }
    }

    /// Writes a CUR-like Parquet file with DuckDB.
    fn write_parquet(path: &Path, rows: &str, with_sp: bool) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let sp = if with_sp {
            ", savings_plan_savings_plan_effective_cost DOUBLE"
        } else {
            ""
        };
        conn.execute_batch(&format!(
            "CREATE TABLE t (line_item_usage_start_date TIMESTAMP, line_item_usage_account_id VARCHAR,
               line_item_product_code VARCHAR, product_region_code VARCHAR, line_item_usage_type VARCHAR,
               line_item_resource_id VARCHAR, line_item_line_item_type VARCHAR, line_item_unblended_cost DOUBLE,
               line_item_net_unblended_cost DOUBLE, line_item_currency_code VARCHAR{sp});
             INSERT INTO t VALUES {rows};
             COPY t TO '{}' (FORMAT PARQUET);",
            path.display()
        ))
        .unwrap();
    }

    #[tokio::test]
    async fn ingests_incrementally_and_follows_manifest() {
        let bucket = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = bucket.path().join("cur/afinin");
        // Old execution (superseded) and the current one listed in the manifest.
        write_parquet(
            &root.join("data/BILLING_PERIOD=2026-10/old-00001.snappy.parquet"),
            "('2026-10-01', '111', 'AmazonEC2', 'us-east-1', 'BoxUsage', 'i-1', 'Usage', 999, 999, 'USD')",
            false,
        );
        write_parquet(
            &root.join("data/BILLING_PERIOD=2026-10/new-00001.snappy.parquet"),
            "('2026-10-01 05:00', '111', 'AmazonEC2', 'us-east-1', 'BoxUsage', 'i-1', 'Usage', 10, 9, 'USD', NULL),
             ('2026-10-01 06:00', '111', 'AmazonEC2', 'us-east-1', 'BoxUsage', 'i-1', 'SavingsPlanCoveredUsage', 10, 10, 'USD', 6),
             ('2026-10-02', '111', 'AmazonS3', 'us-east-1', 'TimedStorage', 'assets', 'Usage', 2, 2, 'USD', NULL),
             ('2026-10-02', '222', 'AWSLambda', 'eu-west-1', 'Request', 'arn:aws:lambda:eu-west-1:222:function:f', 'Usage', 1, 1, 'USD', NULL)",
            true,
        );
        std::fs::create_dir_all(root.join("metadata/BILLING_PERIOD=2026-10")).unwrap();
        std::fs::write(
            root.join("metadata/BILLING_PERIOD=2026-10/afinin-Manifest.json"),
            r#"{"dataFiles":["s3://bkt/cur/afinin/data/BILLING_PERIOD=2026-10/new-00001.snappy.parquet"]}"#,
        )
        .unwrap();
        write_parquet(
            &root.join("data/BILLING_PERIOD=2026-09/x-00001.snappy.parquet"),
            "('2026-09-15', '111', 'AmazonEC2', 'us-east-1', 'BoxUsage', 'i-1', 'Usage', 5, 5, 'USD')",
            false,
        );

        let storage = Storage::open_in_memory().unwrap();
        let ws = storage.create_workspace("W").unwrap();
        let acc = storage
            .add_account(&ws.id, "Payer", "p", ProfileKind::Sso)
            .unwrap();
        let db = Mutex::new(storage);
        let cfg = CurConfig {
            account_id: acc.id.clone(),
            bucket: "bkt".into(),
            prefix: "cur".into(),
            export_name: "afinin".into(),
            region: "us-east-1".into(),
        };
        let store = DirStore(bucket.path().to_path_buf());

        let r = ingest(&db, &store, &cfg, tmp.path()).await.unwrap();
        assert_eq!(r.periods_ingested, vec!["2026-09", "2026-10"]);
        assert_eq!(r.files_downloaded, 2, "superseded execution skipped");
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            0,
            "downloads deleted"
        );

        // Nothing changed → nothing downloaded.
        let again = ingest(&db, &store, &cfg, tmp.path()).await.unwrap();
        assert!(again.periods_ingested.is_empty());

        let storage = db.lock().unwrap();
        let costs = storage
            .cur_resource_costs(&ws.id, CostMetric::Unblended, "2026-10-01", "2026-11-01")
            .unwrap();
        assert_eq!(costs.get("i-1"), Some(&20.0));
        assert_eq!(costs.get("assets"), Some(&2.0));
        // Amortized uses the Savings Plan effective cost for covered usage…
        let amortized = storage
            .cur_resource_costs(&ws.id, CostMetric::Amortized, "2026-10-01", "2026-11-01")
            .unwrap();
        assert_eq!(amortized.get("i-1"), Some(&16.0));
        // …and falls back to unblended in files without Savings Plan columns.
        let amortized = storage
            .cur_resource_costs(&ws.id, CostMetric::Amortized, "2026-09-01", "2026-10-01")
            .unwrap();
        assert_eq!(amortized.get("i-1"), Some(&5.0));

        let q = CostQuery {
            workspace_id: ws.id.clone(),
            start: "2026-09-01".into(),
            end: "2026-11-01".into(),
            granularity: Granularity::Monthly,
            metric: CostMetric::Unblended,
            group_by: GroupBy::Resource,
            filter: CostFilter {
                linked_accounts: vec!["111".into()],
                services: vec![],
            },
        };
        let s = storage.cost_series(&q).unwrap();
        assert_eq!(s.periods, vec!["2026-09-01", "2026-10-01"]);
        assert_eq!(s.groups[0].key, "i-1");
        assert_eq!(s.groups[0].values, vec![5.0, 20.0]);
        assert_eq!(s.total, 27.0);
    }

    #[test]
    fn parses_billing_period() {
        assert_eq!(
            billing_period("p/x/data/BILLING_PERIOD=2026-10/f.parquet").as_deref(),
            Some("2026-10")
        );
        assert_eq!(billing_period("p/x/data/other/f.parquet"), None);
    }
}
