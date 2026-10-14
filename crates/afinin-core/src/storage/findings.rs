// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashMap;

use duckdb::{params, params_from_iter};

use super::Storage;
use crate::cost::{CostMetric, Dimension, Granularity};
use crate::error::{Error, Result};
use crate::findings::FindingStatus;
use crate::findings::anomalies::DailyCost;
use crate::findings::commitments::{AwsAnomaly, CommitmentSummary};
use crate::findings::pricing::PriceBook;
use crate::model::now_rfc3339;

/// Prices older than this are refreshed from the Price List API.
pub const PRICE_TTL_DAYS: i64 = 30;

fn json<T: serde::Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Other(e.to_string()))
}

impl Storage {
    // ---- prices -----------------------------------------------------------

    pub(crate) fn price_book(&self) -> Result<PriceBook> {
        let mut stmt = self.conn.prepare("SELECT region, item, usd FROM prices")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                r.get::<_, f64>(2)?,
            ))
        })?;
        Ok(PriceBook(rows.collect::<duckdb::Result<_>>()?))
    }

    /// Item keys in `region` fetched within the TTL.
    pub(crate) fn fresh_price_keys(
        &self,
        region: &str,
        cutoff_rfc3339: &str,
    ) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT item FROM prices WHERE region = ? AND fetched_at >= ?")?;
        let rows = stmt.query_map(params![region, cutoff_rfc3339], |r| r.get(0))?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    pub(crate) fn save_prices(&self, region: &str, prices: &[(String, f64)]) -> Result<()> {
        let now = now_rfc3339();
        for (item, usd) in prices {
            self.conn.execute(
                "INSERT OR REPLACE INTO prices VALUES (?, ?, ?, ?)",
                params![region, item, usd, now],
            )?;
        }
        Ok(())
    }

    // ---- finding state (FR-33) --------------------------------------------

    pub fn set_finding_status(&self, finding_id: &str, status: &FindingStatus) -> Result<()> {
        if *status == FindingStatus::Open {
            self.conn.execute(
                "DELETE FROM finding_state WHERE finding_id = ?",
                params![finding_id],
            )?;
        } else {
            self.conn.execute(
                "INSERT OR REPLACE INTO finding_state VALUES (?, ?, ?)",
                params![finding_id, json(status)?, now_rfc3339()],
            )?;
        }
        Ok(())
    }

    pub(crate) fn finding_states(&self) -> Result<HashMap<String, FindingStatus>> {
        let mut stmt = self
            .conn
            .prepare("SELECT finding_id, status FROM finding_state")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|(id, s)| Some((id, serde_json::from_str(&s).ok()?)))
            .collect())
    }

    /// Records first-seen dates for new IDs and returns them all.
    pub(crate) fn mark_findings_seen(
        &self,
        ids: &[&str],
        today: &str,
    ) -> Result<HashMap<String, String>> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO finding_seen VALUES (?, ?)")?;
            for id in ids {
                insert.execute(params![id, today])?;
            }
        }
        tx.commit()?;
        let mut stmt = self
            .conn
            .prepare("SELECT finding_id, first_seen FROM finding_seen")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    // ---- inputs for rules -------------------------------------------------

    /// Daily cost per linked account and service, de-duplicated (FR-30).
    pub(crate) fn daily_service_costs(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        start: &str,
        end: &str,
    ) -> Result<Vec<DailyCost>> {
        let facts = super::cost::FactsSpec {
            workspace_id,
            granularity: Granularity::Daily,
            dimension: &Dimension::Service,
            start,
            end,
            linked_accounts: &[],
            keys: &[],
            source: None,
        }
        .build();
        let sql = format!(
            "{}SELECT linked_account, key, period_start, sum({}) FROM f GROUP BY 1, 2, 3",
            facts.sql,
            metric.column()
        );
        let labels: HashMap<String, String> = self
            .linked_account_values(workspace_id)?
            .into_iter()
            .map(|v| (v.key, v.label))
            .collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(facts.params), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (account_id, service, date, amount) = row?;
            out.push(DailyCost {
                account_label: labels
                    .get(&account_id)
                    .cloned()
                    .unwrap_or_else(|| account_id.clone()),
                account_id,
                service,
                date,
                amount,
            });
        }
        Ok(out)
    }

    /// NAT gateway GB processed and processing cost per resource (CUR), or
    /// `None` when the workspace has no CUR data.
    #[allow(clippy::type_complexity)]
    pub(crate) fn cur_nat_usage(
        &self,
        workspace_id: &str,
        start: &str,
        end: &str,
    ) -> Result<Option<(HashMap<String, f64>, HashMap<String, f64>)>> {
        let has_cur: bool = self.conn.query_row(
            "SELECT count(*) > 0 FROM cur_daily WHERE usage_date >= ?
               AND source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)",
            params![start, workspace_id],
            |r| r.get(0),
        )?;
        if !has_cur {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare(
            "SELECT resource_id, sum(usage_amount), sum(net_unblended) FROM cur_daily
             WHERE usage_type LIKE '%NatGateway-Bytes' AND resource_id <> ''
               AND usage_date >= ? AND usage_date < ?
               AND source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)
             GROUP BY 1",
        )?;
        let (mut gb, mut cost) = (HashMap::new(), HashMap::new());
        for row in stmt.query_map(params![start, end, workspace_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })? {
            let (id, g, c) = row?;
            // CUR identifies NAT gateways by ARN; also index by the native ID.
            let native = id.rsplit('/').next().unwrap_or(&id).to_owned();
            gb.insert(native.clone(), g);
            cost.insert(native, c);
            gb.insert(id.clone(), g);
            cost.insert(id, c);
        }
        Ok(Some((gb, cost)))
    }

    // ---- commitments & AWS anomalies --------------------------------------

    pub(crate) fn commitments_fetched_at(&self, source: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT max(fetched_at) FROM commitments WHERE source_account = ?",
            params![source],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn save_commitments(
        &self,
        source: &str,
        summaries: &[CommitmentSummary],
    ) -> Result<()> {
        let now = now_rfc3339();
        for s in summaries {
            self.conn.execute(
                "INSERT OR REPLACE INTO commitments VALUES (?, ?, ?, ?)",
                params![source, s.kind, json(s)?, now],
            )?;
        }
        Ok(())
    }

    pub fn commitments(&self, workspace_id: &str) -> Result<Vec<(String, CommitmentSummary)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.source_account, c.summary FROM commitments c JOIN accounts a ON a.id = c.source_account
             WHERE a.workspace_id = ? ORDER BY 1, c.kind",
        )?;
        let rows = stmt.query_map(params![workspace_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|(a, s)| Some((a, serde_json::from_str(&s).ok()?)))
            .collect())
    }

    pub(crate) fn save_aws_anomalies(&self, source: &str, anomalies: &[AwsAnomaly]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM aws_anomalies WHERE source_account = ?",
            params![source],
        )?;
        for a in anomalies {
            tx.execute(
                "INSERT INTO aws_anomalies VALUES (?, ?, ?)",
                params![source, a.id, json(a)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn aws_anomalies(&self, workspace_id: &str) -> Result<Vec<(String, AwsAnomaly)>> {
        let mut stmt = self.conn.prepare(
            "SELECT x.source_account, x.data FROM aws_anomalies x JOIN accounts a ON a.id = x.source_account
             WHERE a.workspace_id = ?",
        )?;
        let rows = stmt.query_map(params![workspace_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|(a, d)| Some((a, serde_json::from_str(&d).ok()?)))
            .collect())
    }
}
