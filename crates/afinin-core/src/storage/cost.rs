// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cost data: writes from sync and the local query engine behind the dashboard.

use std::collections::HashMap;

use duckdb::types::Value;
use duckdb::{OptionalExt, params, params_from_iter};
use serde::Serialize;

use super::Storage;
use crate::cost::source::{CostRow, Forecast};
use crate::cost::{
    CostComparisonRow, CostGroup, CostMetric, CostOverview, CostQuery, CostSeries, Dimension,
    DimensionValue, Granularity, GroupBy, dates, pct_change,
};
use crate::error::Result;
use crate::model::now_rfc3339;

/// (linked account, service) → cost.
pub(crate) type ServiceCosts = HashMap<(String, String), f64>;

/// Calls made to a billable AWS API (FR-14, NFR-30).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiUsage {
    pub service: String,
    pub operation: String,
    pub calls: i64,
    pub cost_usd: f64,
}

/// Facts visible in a workspace, de-duplicated: when several connected
/// accounts report the same linked account (e.g. a management account and the
/// member itself), only the source covering the most accounts is used.
pub(super) struct Facts {
    pub sql: String,
    pub params: Vec<Value>,
}

pub(super) struct FactsSpec<'a> {
    pub workspace_id: &'a str,
    pub granularity: Granularity,
    pub dimension: &'a Dimension,
    pub start: &'a str,
    pub end: &'a str,
    pub linked_accounts: &'a [String],
    pub keys: &'a [String],
    pub source: Option<&'a str>,
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

impl FactsSpec<'_> {
    pub(super) fn build(&self) -> Facts {
        let mut params = vec![
            text(self.workspace_id),
            text(self.granularity.as_str()),
            text(&self.dimension.id()),
            text(self.start),
            text(self.end),
        ];
        let mut sql = String::from(
            "WITH ws AS (SELECT id FROM accounts WHERE workspace_id = ?),
             cov AS (
                 SELECT source_account, count(*) AS n FROM linked_accounts
                 WHERE source_account IN (SELECT id FROM ws) GROUP BY 1
             ),
             pref AS (
                 SELECT la.linked_account_id,
                        first(la.source_account ORDER BY c.n DESC, la.source_account) AS src
                 FROM linked_accounts la JOIN cov c USING (source_account)
                 GROUP BY 1
             ),
             f AS (
                 SELECT f.* FROM cost_facts f
                 JOIN pref p ON f.source_account = p.src AND f.linked_account = p.linked_account_id
                 WHERE f.granularity = ? AND f.dimension = ?
                   AND f.period_start >= ? AND f.period_start < ?",
        );
        if !self.linked_accounts.is_empty() {
            sql.push_str(&format!(
                " AND f.linked_account IN ({})",
                placeholders(self.linked_accounts.len())
            ));
            params.extend(self.linked_accounts.iter().map(|a| text(a)));
        }
        if !self.keys.is_empty() {
            sql.push_str(&format!(
                " AND f.key IN ({})",
                placeholders(self.keys.len())
            ));
            params.extend(self.keys.iter().map(|k| text(k)));
        }
        if let Some(src) = self.source {
            sql.push_str(" AND f.source_account = ?");
            params.push(text(src));
        }
        sql.push_str(")\n");
        Facts { sql, params }
    }
}

impl Storage {
    // ---- writes (sync) ----------------------------------------------------

    /// Replaces the facts of one dataset in `[start, end)` with `rows`.
    pub(crate) fn replace_cost_rows(
        &self,
        source: &str,
        granularity: Granularity,
        dimension: &Dimension,
        start: &str,
        end: &str,
        rows: &[CostRow],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM cost_facts WHERE source_account = ? AND granularity = ? AND dimension = ?
               AND period_start >= ? AND period_start < ?",
            params![source, granularity.as_str(), dimension.id(), start, end],
        )?;
        {
            let mut app = tx.appender("cost_facts")?;
            let dim = dimension.id();
            for r in rows {
                app.append_row(params![
                    source,
                    granularity.as_str(),
                    dim,
                    r.period_start,
                    r.linked_account,
                    r.key,
                    r.unblended,
                    r.amortized,
                    r.net_unblended,
                    r.unit,
                    r.estimated
                ])?;
            }
            app.flush()?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records linked accounts seen by a source; a known name is never erased.
    pub(crate) fn upsert_linked_accounts(
        &self,
        source: &str,
        accounts: &[(String, Option<String>)],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO linked_accounts (source_account, linked_account_id, name) VALUES (?, ?, ?)
             ON CONFLICT DO UPDATE SET name = coalesce(excluded.name, linked_accounts.name)",
        )?;
        for (id, name) in accounts {
            stmt.execute(params![source, id, name])?;
        }
        Ok(())
    }

    /// Rebuilds monthly facts from daily data for months starting at `from_month`.
    pub(crate) fn rollup_monthly(
        &self,
        source: &str,
        dimension: &Dimension,
        from_month: &str,
    ) -> Result<()> {
        let dim = dimension.id();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM cost_facts WHERE source_account = ? AND dimension = ?
               AND granularity = 'MONTHLY' AND period_start >= ?",
            params![source, dim, from_month],
        )?;
        tx.execute(
            "INSERT INTO cost_facts
             SELECT source_account, 'MONTHLY', dimension,
                    strftime(date_trunc('month', period_start::DATE), '%Y-%m-%d'),
                    linked_account, key,
                    sum(unblended), sum(amortized), sum(net_unblended),
                    any_value(unit), bool_or(estimated)
             FROM cost_facts
             WHERE source_account = ? AND dimension = ? AND granularity = 'DAILY'
               AND period_start >= ?
             GROUP BY ALL",
            params![source, dim, from_month],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// `(daily_from, daily_through)` of a dataset, if it was ever synced.
    pub(crate) fn cost_sync_state(
        &self,
        source: &str,
        dimension: &Dimension,
    ) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT daily_from, daily_through FROM cost_sync_state
                 WHERE source_account = ? AND dimension = ?",
                params![source, dimension.id()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub(crate) fn set_cost_sync_state(
        &self,
        source: &str,
        dimension: &Dimension,
        daily_from: &str,
        daily_through: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO cost_sync_state VALUES (?, ?, ?, ?)",
            params![source, dimension.id(), daily_from, daily_through],
        )?;
        Ok(())
    }

    pub(crate) fn save_forecast(
        &self,
        source: &str,
        metric: CostMetric,
        start: &str,
        end: &str,
        forecast: Option<&Forecast>,
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cost_forecasts WHERE source_account = ? AND metric = ?",
            params![source, metric.column()],
        )?;
        if let Some(f) = forecast {
            self.conn.execute(
                "INSERT INTO cost_forecasts VALUES (?, ?, ?, ?, ?, ?)",
                params![source, metric.column(), start, end, f.amount, f.unit],
            )?;
        }
        Ok(())
    }

    pub(crate) fn set_sync_running(&self, account_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO account_sync (account_id, state, started_at) VALUES (?, 'running', ?)
             ON CONFLICT DO UPDATE SET state = 'running', started_at = excluded.started_at,
                                       finished_at = NULL",
            params![account_id, now_rfc3339()],
        )?;
        Ok(())
    }

    pub(crate) fn set_sync_finished(&self, account_id: &str, error: Option<&str>) -> Result<()> {
        let now = now_rfc3339();
        self.conn.execute(
            "UPDATE account_sync SET
                state = ?, finished_at = ?, error = ?,
                last_success_at = CASE WHEN ? THEN ? ELSE last_success_at END
             WHERE account_id = ?",
            params![
                if error.is_some() { "error" } else { "ok" },
                now,
                error,
                error.is_none(),
                now,
                account_id
            ],
        )?;
        Ok(())
    }

    /// Marks syncs interrupted by an app exit as failed.
    pub(crate) fn reset_stale_syncs(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE account_sync SET state = 'error', error = 'interrupted' WHERE state = 'running'",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn record_api_call(
        &self,
        account_id: &str,
        service: &str,
        operation: &str,
        cost_usd: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO api_calls VALUES (?, ?, ?, ?, ?)",
            params![now_rfc3339(), account_id, service, operation, cost_usd],
        )?;
        Ok(())
    }

    /// Billable API calls made since `since` (RFC 3339 or `YYYY-MM-DD`).
    pub fn api_usage(&self, since: &str) -> Result<Vec<ApiUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT service, operation, count(*), sum(cost_usd) FROM api_calls
             WHERE called_at >= ? GROUP BY ALL ORDER BY 4 DESC",
        )?;
        let rows = stmt.query_map(params![since], |r| {
            Ok(ApiUsage {
                service: r.get(0)?,
                operation: r.get(1)?,
                calls: r.get(2)?,
                cost_usd: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    // ---- reads (dashboard) ------------------------------------------------

    fn labels(&self, workspace_id: &str, group_by: &GroupBy) -> Result<HashMap<String, String>> {
        if *group_by != GroupBy::Account {
            return Ok(HashMap::new());
        }
        Ok(self
            .linked_account_values(workspace_id)?
            .into_iter()
            .map(|v| (v.key, v.label))
            .collect())
    }

    fn label(labels: &HashMap<String, String>, group_by: &GroupBy, key: &str) -> String {
        if let Some(l) = labels.get(key) {
            return l.clone();
        }
        match (group_by, key) {
            (GroupBy::Tag(_), "") => "(untagged)".into(),
            (GroupBy::Region, "") | (GroupBy::Region, "global") => "Global".into(),
            (_, "") => "(none)".into(),
            _ => key.to_owned(),
        }
    }

    /// Linked accounts with data in a workspace, labelled `Name (id)`.
    pub fn linked_account_values(&self, workspace_id: &str) -> Result<Vec<DimensionValue>> {
        let mut stmt = self.conn.prepare(
            "SELECT linked_account_id, max(name) FROM linked_accounts
             WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)
             GROUP BY 1 ORDER BY coalesce(max(name), linked_account_id)",
        )?;
        let rows = stmt.query_map(params![workspace_id], |r| {
            let id: String = r.get(0)?;
            let name: Option<String> = r.get(1)?;
            Ok(DimensionValue {
                label: match name {
                    Some(n) if !n.is_empty() => format!("{n} ({id})"),
                    _ => id.clone(),
                },
                key: id,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    /// Distinct values of a grouping, for filter pickers.
    pub fn dimension_values(
        &self,
        workspace_id: &str,
        group_by: &GroupBy,
    ) -> Result<Vec<DimensionValue>> {
        match group_by {
            GroupBy::Account => return self.linked_account_values(workspace_id),
            GroupBy::Resource => return Ok(Vec::new()),
            _ => {}
        }
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT key FROM cost_facts
             WHERE dimension = ? AND source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)
             ORDER BY 1",
        )?;
        let keys = stmt
            .query_map(params![group_by.dataset().id(), workspace_id], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let labels = HashMap::new();
        Ok(keys
            .into_iter()
            .map(|k| DimensionValue {
                label: Self::label(&labels, group_by, &k),
                key: k,
            })
            .collect())
    }

    /// Breakdown of costs per group and period (FR-11, FR-12, FR-13).
    pub fn cost_series(&self, q: &CostQuery) -> Result<CostSeries> {
        q.validate()?;
        if q.group_by == GroupBy::Resource {
            return self.cur_series(q);
        }
        let start = match q.granularity {
            Granularity::Daily => q.start.clone(),
            Granularity::Monthly => dates::format(dates::first_of_month(dates::parse(&q.start)?)),
        };
        let periods = period_list(&start, &q.end, q.granularity)?;
        let dimension = q.group_by.dataset();
        let facts = FactsSpec {
            workspace_id: &q.workspace_id,
            granularity: q.granularity,
            dimension: &dimension,
            start: &start,
            end: &q.end,
            linked_accounts: &q.filter.linked_accounts,
            keys: &q.filter.services,
            source: None,
        }
        .build();
        let group_col = if q.group_by == GroupBy::Account {
            "linked_account"
        } else {
            "key"
        };
        let sql = format!(
            "{}SELECT period_start, {group_col}, sum({metric}), bool_or(estimated), any_value(unit)
             FROM f GROUP BY 1, 2",
            facts.sql,
            metric = q.metric.column()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(facts.params), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;

        let index: HashMap<&str, usize> = periods
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_str(), i))
            .collect();
        let labels = self.labels(&q.workspace_id, &q.group_by)?;
        let mut groups: HashMap<String, CostGroup> = HashMap::new();
        let mut period_totals = vec![0.0; periods.len()];
        let mut estimated = vec![false; periods.len()];
        let mut unit = None;
        for (period, key, amount, est, u) in rows {
            let Some(&i) = index.get(period.as_str()) else {
                continue;
            };
            let g = groups.entry(key.clone()).or_insert_with(|| CostGroup {
                label: Self::label(&labels, &q.group_by, &key),
                key,
                values: vec![0.0; periods.len()],
                total: 0.0,
            });
            g.values[i] += amount;
            g.total += amount;
            period_totals[i] += amount;
            estimated[i] |= est;
            unit.get_or_insert(u);
        }
        let mut groups: Vec<CostGroup> = groups.into_values().collect();
        groups.sort_by(|a, b| {
            b.total
                .total_cmp(&a.total)
                .then_with(|| a.label.cmp(&b.label))
        });
        Ok(CostSeries {
            total: period_totals.iter().sum(),
            estimated_periods: periods
                .iter()
                .zip(&estimated)
                .filter(|(_, e)| **e)
                .map(|(p, _)| p.clone())
                .collect(),
            periods,
            groups,
            period_totals,
            unit: unit.unwrap_or_else(|| "USD".into()),
        })
    }

    /// Cost per (linked account, service) in `[start, end)`, for the graph overlay.
    pub(crate) fn service_cost_by_account(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        start: &str,
        end: &str,
    ) -> Result<(ServiceCosts, String)> {
        let facts = FactsSpec {
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
            "{}SELECT linked_account, key, sum({}), any_value(unit) FROM f GROUP BY 1, 2",
            facts.sql,
            metric.column()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut unit = None;
        let mut out = HashMap::new();
        for row in stmt.query_map(params_from_iter(facts.params), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (account, service, amount, u) = row?;
            unit.get_or_insert(u);
            out.insert((account, service), amount);
        }
        Ok((out, unit.unwrap_or_else(|| "USD".into())))
    }

    /// Compares each group's total in the query range with `[prev_start, prev_end)` (FR-12).
    pub fn compare_costs(
        &self,
        q: &CostQuery,
        prev_start: &str,
        prev_end: &str,
    ) -> Result<Vec<CostComparisonRow>> {
        let current = self.cost_series(q)?;
        let previous = self.cost_series(&CostQuery {
            start: prev_start.to_owned(),
            end: prev_end.to_owned(),
            ..q.clone()
        })?;
        let prev: HashMap<&str, &CostGroup> = previous
            .groups
            .iter()
            .map(|g| (g.key.as_str(), g))
            .collect();
        let mut rows: Vec<CostComparisonRow> = current
            .groups
            .iter()
            .map(|g| {
                let p = prev.get(g.key.as_str()).map_or(0.0, |p| p.total);
                CostComparisonRow {
                    key: g.key.clone(),
                    label: g.label.clone(),
                    current: g.total,
                    previous: p,
                    delta: g.total - p,
                    delta_pct: pct_change(g.total, p),
                }
            })
            .collect();
        let seen: std::collections::HashSet<&str> =
            current.groups.iter().map(|g| g.key.as_str()).collect();
        rows.extend(
            previous
                .groups
                .iter()
                .filter(|g| !seen.contains(g.key.as_str()))
                .map(|g| CostComparisonRow {
                    key: g.key.clone(),
                    label: g.label.clone(),
                    current: 0.0,
                    previous: g.total,
                    delta: -g.total,
                    delta_pct: Some(-100.0),
                }),
        );
        rows.sort_by(|a, b| b.delta.abs().total_cmp(&a.delta.abs()));
        Ok(rows)
    }

    fn total(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        linked_accounts: &[String],
        start: &str,
        end: &str,
        source: Option<&str>,
    ) -> Result<f64> {
        if start >= end {
            return Ok(0.0);
        }
        let facts = FactsSpec {
            workspace_id,
            granularity: Granularity::Daily,
            dimension: &Dimension::Service,
            start,
            end,
            linked_accounts,
            keys: &[],
            source,
        }
        .build();
        let sql = format!(
            "{}SELECT coalesce(sum({}), 0) FROM f",
            facts.sql,
            metric.column()
        );
        Ok(self
            .conn
            .query_row(&sql, params_from_iter(facts.params), |r| r.get(0))?)
    }

    /// Headline numbers (FR-10). `today` is the current UTC date.
    pub fn cost_overview(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        linked_accounts: &[String],
        today: time::Date,
    ) -> Result<CostOverview> {
        let month_start = dates::first_of_month(today);
        let prev_start = dates::add_months(today, -1);
        // Same number of elapsed days in the previous month, capped at its end.
        let elapsed = (today - month_start).whole_days();
        let prev_same = dates::add_days(prev_start, elapsed).min(month_start);
        let (ms, ps, psame, t) = (
            dates::format(month_start),
            dates::format(prev_start),
            dates::format(prev_same),
            dates::format(today),
        );

        let mtd = self.total(workspace_id, metric, linked_accounts, &ms, &t, None)?;
        let prev_mtd = self.total(workspace_id, metric, linked_accounts, &ps, &psame, None)?;
        let last_month = self.total(workspace_id, metric, linked_accounts, &ps, &ms, None)?;

        let forecast = if linked_accounts.is_empty() {
            self.forecast_month_end(
                workspace_id,
                metric,
                &ms,
                &dates::format(dates::add_months(today, 1)),
            )?
        } else {
            None
        };

        let data_through: Option<String> = self.conn.query_row(
            "SELECT max(period_start) FROM cost_facts
             WHERE granularity = 'DAILY' AND dimension = 'SERVICE'
               AND source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)",
            params![workspace_id],
            |r| r.get(0),
        )?;
        let unit: Option<String> = self
            .conn
            .query_row(
                "SELECT any_value(unit) FROM cost_facts
                 WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)",
                params![workspace_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        Ok(CostOverview {
            metric,
            unit: unit.unwrap_or_else(|| "USD".into()),
            month_to_date: mtd,
            previous_month_to_date: prev_mtd,
            month_over_month_pct: pct_change(mtd, prev_mtd),
            last_month_total: last_month,
            forecast_month_end: forecast,
            data_through,
        })
    }

    /// Actuals up to each source's forecast start plus its forecast, summed over
    /// the sources the workspace uses. `None` if any of them lacks a forecast.
    fn forecast_month_end(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        month_start: &str,
        next_month: &str,
    ) -> Result<Option<f64>> {
        let mut stmt = self.conn.prepare(
            "WITH ws AS (SELECT id FROM accounts WHERE workspace_id = ?),
             cov AS (
                 SELECT source_account, count(*) AS n FROM linked_accounts
                 WHERE source_account IN (SELECT id FROM ws) GROUP BY 1
             ),
             pref AS (
                 SELECT la.linked_account_id,
                        first(la.source_account ORDER BY c.n DESC, la.source_account) AS src
                 FROM linked_accounts la JOIN cov c USING (source_account)
                 GROUP BY 1
             )
             SELECT DISTINCT p.src, fc.period_start, fc.amount
             FROM pref p LEFT JOIN cost_forecasts fc
               ON fc.source_account = p.src AND fc.metric = ? AND fc.period_end = ?",
        )?;
        let sources = stmt
            .query_map(params![workspace_id, metric.column(), next_month], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        if sources.is_empty() {
            return Ok(None);
        }
        let mut total = 0.0;
        for (src, start, amount) in sources {
            let (Some(start), Some(amount)) = (start, amount) else {
                return Ok(None);
            };
            total +=
                amount + self.total(workspace_id, metric, &[], month_start, &start, Some(&src))?;
        }
        Ok(Some(total))
    }
}

/// All period starts in `[start, end)`.
pub(super) fn period_list(start: &str, end: &str, granularity: Granularity) -> Result<Vec<String>> {
    let (mut d, end) = (dates::parse(start)?, dates::parse(end)?);
    let mut out = Vec::new();
    while d < end {
        out.push(dates::format(d));
        d = match granularity {
            Granularity::Daily => dates::add_days(d, 1),
            Granularity::Monthly => dates::add_months(d, 1),
        };
    }
    Ok(out)
}
