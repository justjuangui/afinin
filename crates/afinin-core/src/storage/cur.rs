// SPDX-License-Identifier: MIT OR Apache-2.0

//! CUR 2.0 storage: export configs, loaded billing periods and the daily
//! per-resource cost table queried for resource breakdowns and the graph overlay.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use duckdb::types::Value;
use duckdb::{OptionalExt, params, params_from_iter};
use serde::Serialize;

use super::Storage;
use crate::cost::{CostGroup, CostMetric, CostQuery, CostSeries, Granularity, GroupBy, dates};
use crate::cur::CurConfig;
use crate::error::{Error, Result};
use crate::model::now_rfc3339;

/// CUR coverage of a workspace.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CurStatus {
    pub configs: Vec<CurConfig>,
    pub billing_periods: Vec<String>,
    pub first_date: Option<String>,
    pub last_date: Option<String>,
    pub last_ingested_at: Option<String>,
}

fn sql_list(paths: &[PathBuf]) -> Result<String> {
    let items = paths
        .iter()
        .map(|p| {
            p.to_str()
                .map(|s| format!("'{}'", s.replace('\'', "''")))
                .ok_or_else(|| Error::Other("temporary path is not valid UTF-8".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("[{}]", items.join(", ")))
}

/// SQL expressions for the columns we read, adapted to what the files contain:
/// CUR 2.0 exports only include the columns selected in their query.
struct Columns(HashSet<String>);

impl Columns {
    fn has(&self, c: &str) -> bool {
        self.0.contains(c)
    }

    fn or(&self, c: &str, fallback: &str) -> String {
        if self.has(c) {
            format!("coalesce(CAST({c} AS VARCHAR), {fallback})")
        } else {
            fallback.to_owned()
        }
    }

    fn num(&self, c: &str) -> String {
        if self.has(c) {
            format!("coalesce(CAST({c} AS DOUBLE), 0)")
        } else {
            "0".into()
        }
    }

    /// AWS's documented amortized-cost formula, using whatever columns exist.
    fn amortized(&self) -> String {
        let unblended = self.num("line_item_unblended_cost");
        if !self.has("line_item_line_item_type") {
            return unblended;
        }
        let mut arms = Vec::new();
        if self.has("savings_plan_savings_plan_effective_cost") {
            arms.push(format!(
                "WHEN 'SavingsPlanCoveredUsage' THEN coalesce(CAST(savings_plan_savings_plan_effective_cost AS DOUBLE), {unblended})"
            ));
        }
        if self.has("savings_plan_total_commitment_to_date")
            && self.has("savings_plan_used_commitment")
        {
            arms.push(format!(
                "WHEN 'SavingsPlanRecurringFee' THEN {} - {}",
                self.num("savings_plan_total_commitment_to_date"),
                self.num("savings_plan_used_commitment")
            ));
        }
        arms.push("WHEN 'SavingsPlanNegation' THEN 0".into());
        arms.push("WHEN 'SavingsPlanUpfrontFee' THEN 0".into());
        if self.has("reservation_effective_cost") {
            arms.push(format!(
                "WHEN 'DiscountedUsage' THEN coalesce(CAST(reservation_effective_cost AS DOUBLE), {unblended})"
            ));
        }
        if self.has("reservation_unused_amortized_upfront_fee_for_billing_period")
            || self.has("reservation_unused_recurring_fee")
        {
            arms.push(format!(
                "WHEN 'RIFee' THEN {} + {}",
                self.num("reservation_unused_amortized_upfront_fee_for_billing_period"),
                self.num("reservation_unused_recurring_fee")
            ));
        }
        // RI upfront fees are spread over the term through DiscountedUsage.
        let fallback = if self.has("reservation_reservation_a_r_n") {
            format!(
                "CASE WHEN line_item_line_item_type = 'Fee' AND coalesce(reservation_reservation_a_r_n, '') <> '' THEN 0 ELSE {unblended} END"
            )
        } else {
            unblended
        };
        format!(
            "CASE line_item_line_item_type {} ELSE {fallback} END",
            arms.join(" ")
        )
    }
}

/// Preferred source per linked account, as for Cost Explorer data: the
/// connected account whose export covers the most linked accounts.
const CUR_PREF: &str = "
    WITH ws AS (SELECT id FROM accounts WHERE workspace_id = ?),
    cov AS (
        SELECT source_account, count(DISTINCT linked_account) AS n FROM cur_daily
        WHERE source_account IN (SELECT id FROM ws) GROUP BY 1
    ),
    pairs AS (
        SELECT DISTINCT d.source_account, d.linked_account, c.n FROM cur_daily d JOIN cov c USING (source_account)
    ),
    pref AS (
        SELECT linked_account, first(source_account ORDER BY n DESC, source_account) AS src FROM pairs GROUP BY 1
    )";

impl Storage {
    // ---- configuration ----------------------------------------------------

    pub fn save_cur_config(&self, cfg: &CurConfig) -> Result<()> {
        if cfg.bucket.trim().is_empty()
            || cfg.export_name.trim().is_empty()
            || cfg.region.trim().is_empty()
        {
            return Err(Error::Other(
                "bucket, export name and region are required".into(),
            ));
        }
        self.get_account(&cfg.account_id)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO cur_configs VALUES (?, ?, ?, ?, ?)",
            params![
                cfg.account_id,
                cfg.bucket.trim(),
                cfg.prefix.trim_matches('/'),
                cfg.export_name.trim(),
                cfg.region.trim()
            ],
        )?;
        Ok(())
    }

    pub fn cur_config(&self, account_id: &str) -> Result<Option<CurConfig>> {
        Ok(self
            .conn
            .query_row(
                "SELECT account_id, bucket, prefix, export_name, region FROM cur_configs WHERE account_id = ?",
                params![account_id],
                |r| {
                    Ok(CurConfig {
                        account_id: r.get(0)?,
                        bucket: r.get(1)?,
                        prefix: r.get(2)?,
                        export_name: r.get(3)?,
                        region: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Removes the config and all CUR data loaded through it.
    pub fn delete_cur_config(&self, account_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM cur_configs WHERE account_id = ?",
            params![account_id],
        )?;
        tx.execute(
            "DELETE FROM cur_periods WHERE source_account = ?",
            params![account_id],
        )?;
        tx.execute(
            "DELETE FROM cur_daily WHERE source_account = ?",
            params![account_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn cur_status(&self, workspace_id: &str) -> Result<CurStatus> {
        let mut stmt = self.conn.prepare(
            "SELECT c.account_id FROM cur_configs c JOIN accounts a ON a.id = c.account_id WHERE a.workspace_id = ?",
        )?;
        let ids = stmt
            .query_map(params![workspace_id], |r| r.get::<_, String>(0))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let configs = ids
            .iter()
            .filter_map(|id| self.cur_config(id).transpose())
            .collect::<Result<Vec<_>>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT billing_period FROM cur_periods
             WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?) ORDER BY 1",
        )?;
        let billing_periods = stmt
            .query_map(params![workspace_id], |r| r.get::<_, String>(0))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let (first_date, last_date, last_ingested_at) = self.conn.query_row(
            "SELECT min(d.usage_date), max(d.usage_date),
                    (SELECT max(ingested_at) FROM cur_periods p
                     WHERE p.source_account IN (SELECT id FROM accounts WHERE workspace_id = ?1))
             FROM cur_daily d WHERE d.source_account IN (SELECT id FROM accounts WHERE workspace_id = ?1)",
            params![workspace_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(CurStatus {
            configs,
            billing_periods,
            first_date,
            last_date,
            last_ingested_at,
        })
    }

    // ---- ingest -----------------------------------------------------------

    pub(crate) fn cur_period_fingerprint(
        &self,
        source: &str,
        period: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT fingerprint FROM cur_periods WHERE source_account = ? AND billing_period = ?",
                params![source, period],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Replaces one billing period with the contents of `files` (Parquet).
    pub(crate) fn load_cur_period(
        &self,
        source: &str,
        period: &str,
        files: &[PathBuf],
        fingerprint: &str,
    ) -> Result<u64> {
        let list = sql_list(files)?;
        let read = format!("read_parquet({list}, union_by_name = true)");
        let mut stmt = self
            .conn
            .prepare(&format!("DESCRIBE SELECT * FROM {read}"))?;
        let cols = Columns(
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<duckdb::Result<HashSet<_>>>()?,
        );
        for required in ["line_item_usage_start_date", "line_item_unblended_cost"] {
            if !cols.has(required) {
                return Err(Error::Other(format!(
                    "CUR files lack the `{required}` column; is this a CUR 2.0 export?"
                )));
            }
        }
        let net = if cols.has("line_item_net_unblended_cost") {
            format!(
                "coalesce(CAST(line_item_net_unblended_cost AS DOUBLE), {})",
                cols.num("line_item_unblended_cost")
            )
        } else {
            cols.num("line_item_unblended_cost")
        };
        let sql = format!(
            "INSERT INTO cur_daily
             SELECT ?, ?, strftime(CAST(line_item_usage_start_date AS DATE), '%Y-%m-%d') AS usage_date,
                    {linked} AS linked_account, {product} AS product_code, {region} AS region,
                    {usage_type} AS usage_type, {resource} AS resource_id,
                    sum({unblended}), sum({amortized}), sum({net}), sum({public}), sum({usage}),
                    any_value({currency})
             FROM {read}
             GROUP BY usage_date, linked_account, product_code, region, usage_type, resource_id",
            linked = cols.or("line_item_usage_account_id", "''"),
            product = cols.or("line_item_product_code", "''"),
            region = cols.or("product_region_code", "''"),
            usage_type = cols.or("line_item_usage_type", "''"),
            resource = cols.or("line_item_resource_id", "''"),
            unblended = cols.num("line_item_unblended_cost"),
            amortized = cols.amortized(),
            public = cols.num("pricing_public_on_demand_cost"),
            usage = cols.num("line_item_usage_amount"),
            currency = cols.or("line_item_currency_code", "'USD'"),
        );

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM cur_daily WHERE source_account = ? AND billing_period = ?",
            params![source, period],
        )?;
        let rows = tx.execute(&sql, params![source, period])? as u64;
        tx.execute(
            "INSERT OR REPLACE INTO cur_periods VALUES (?, ?, ?, ?, ?)",
            params![source, period, fingerprint, now_rfc3339(), rows as i64],
        )?;
        tx.commit()?;
        Ok(rows)
    }

    // ---- queries ----------------------------------------------------------

    /// Cost per resource ID in `[start, end)` (FR-24).
    pub fn cur_resource_costs(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        start: &str,
        end: &str,
    ) -> Result<HashMap<String, f64>> {
        let sql = format!(
            "{CUR_PREF}
             SELECT d.resource_id, sum(d.{m}) FROM cur_daily d
             JOIN pref p ON d.source_account = p.src AND d.linked_account = p.linked_account
             WHERE d.resource_id <> '' AND d.usage_date >= ? AND d.usage_date < ?
             GROUP BY 1",
            m = metric.column()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![workspace_id, start, end], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    /// Display names for CUR resource IDs, from the discovered inventory.
    fn resource_labels(&self, workspace_id: &str) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT arn, native_id, name, resource_type FROM resources
             WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)",
        )?;
        let mut out = HashMap::new();
        for row in stmt.query_map(params![workspace_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (arn, native, name, kind) = row?;
            let label = match name {
                Some(n) if n != native => format!("{n} ({native})"),
                _ => native.clone(),
            };
            let label = format!("{label} · {kind}");
            out.insert(arn, label.clone());
            out.insert(native, label);
        }
        Ok(out)
    }

    /// Breakdown by resource from CUR data (FR-11).
    pub(crate) fn cur_series(&self, q: &CostQuery) -> Result<CostSeries> {
        if q.group_by != GroupBy::Resource {
            return Err(Error::Other("only resource breakdowns use CUR data".into()));
        }
        let start = match q.granularity {
            Granularity::Daily => q.start.clone(),
            Granularity::Monthly => dates::format(dates::first_of_month(dates::parse(&q.start)?)),
        };
        let period = match q.granularity {
            Granularity::Daily => "d.usage_date",
            Granularity::Monthly => "substr(d.usage_date, 1, 7) || '-01'",
        };
        let mut params: Vec<Value> = vec![
            Value::Text(q.workspace_id.clone()),
            Value::Text(start.clone()),
            Value::Text(q.end.clone()),
        ];
        let mut filter = String::new();
        if !q.filter.linked_accounts.is_empty() {
            filter = format!(
                " AND d.linked_account IN ({})",
                vec!["?"; q.filter.linked_accounts.len()].join(", ")
            );
            params.extend(
                q.filter
                    .linked_accounts
                    .iter()
                    .map(|a| Value::Text(a.clone())),
            );
        }
        let sql = format!(
            "{CUR_PREF}
             SELECT {period}, CASE WHEN d.resource_id = '' THEN '' ELSE d.resource_id END,
                    any_value(d.product_code), sum(d.{m}), any_value(d.currency)
             FROM cur_daily d JOIN pref p ON d.source_account = p.src AND d.linked_account = p.linked_account
             WHERE d.usage_date >= ? AND d.usage_date < ?{filter}
             GROUP BY 1, 2",
            m = q.metric.column()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(params), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let has_cur: bool = self.conn.query_row(
            "SELECT count(*) > 0 FROM cur_daily WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)",
            params![q.workspace_id],
            |r| r.get(0),
        )?;
        if rows.is_empty() && !has_cur {
            return Err(Error::Other(
                "costs per resource need CUR 2.0 data: set up a CUR export from the Accounts tab"
                    .into(),
            ));
        }

        let periods = super::cost::period_list(&start, &q.end, q.granularity)?;
        let index: HashMap<&str, usize> = periods
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_str(), i))
            .collect();
        let labels = self.resource_labels(&q.workspace_id)?;
        let mut groups: HashMap<String, CostGroup> = HashMap::new();
        let mut totals = vec![0.0; periods.len()];
        let mut unit = None;
        for (p, id, product, amount, currency) in rows {
            let Some(&i) = index.get(p.as_str()) else {
                continue;
            };
            // Charges without a resource (support, tax, data transfer…) group per product.
            let key = if id.is_empty() {
                format!("(no resource) {product}")
            } else {
                id
            };
            let g = groups.entry(key.clone()).or_insert_with(|| CostGroup {
                label: labels.get(&key).cloned().unwrap_or_else(|| {
                    if key.starts_with("(no resource)") {
                        key.clone()
                    } else {
                        format!("{key} · {product}")
                    }
                }),
                key,
                values: vec![0.0; periods.len()],
                total: 0.0,
            });
            g.values[i] += amount;
            g.total += amount;
            totals[i] += amount;
            unit.get_or_insert(currency);
        }
        let mut groups: Vec<CostGroup> = groups.into_values().collect();
        groups.sort_by(|a, b| b.total.total_cmp(&a.total).then_with(|| a.key.cmp(&b.key)));
        Ok(CostSeries {
            total: totals.iter().sum(),
            periods,
            groups,
            period_totals: totals,
            unit: unit.unwrap_or_else(|| "USD".into()),
            estimated_periods: Vec::new(),
        })
    }
}
