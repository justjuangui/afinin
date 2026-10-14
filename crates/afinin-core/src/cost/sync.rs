// SPDX-License-Identifier: MIT OR Apache-2.0

//! Incremental Cost Explorer sync (FR-14, FR-50, FR-51).
//!
//! First sync of a dataset: monthly history plus daily data from at least the
//! start of the previous month. Later syncs only re-fetch the last few days,
//! which AWS may still revise. Monthly figures for months covered by daily data
//! are rebuilt locally, so they never need to be fetched again.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;
use time::Date;

use super::source::{CE_REQUEST_COST_USD, CostRequest, CostRow, CostSource};
use super::{CostMetric, Dimension, Granularity, dates};
use crate::error::Result;
use crate::settings::Settings;
use crate::storage::Storage;

/// Days re-fetched on each incremental sync; AWS finalizes recent costs late.
const REFRESH_DAYS: i64 = 5;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SyncReport {
    pub api_calls: u32,
    pub estimated_cost_usd: f64,
    pub rows: usize,
    /// Non-fatal problems, e.g. no forecast available yet.
    pub warnings: Vec<String>,
}

pub(crate) fn datasets(settings: &Settings) -> Vec<Dimension> {
    let mut dims = vec![Dimension::Service, Dimension::Region, Dimension::UsageType];
    dims.extend(
        settings
            .cost_tag_keys
            .iter()
            .map(|k| Dimension::Tag(k.trim().to_owned())),
    );
    dims
}

/// First month entirely covered by daily data starting at `from`.
fn first_full_month(from: Date) -> Date {
    if from.day() == 1 {
        from
    } else {
        dates::add_months(from, 1)
    }
}

struct Run<'a, S> {
    db: &'a Mutex<Storage>,
    source: &'a S,
    account_id: &'a str,
    report: SyncReport,
    names: BTreeMap<String, Option<String>>,
}

impl<S: CostSource> Run<'_, S> {
    fn db<T>(&self, f: impl FnOnce(&Storage) -> Result<T>) -> Result<T> {
        f(&self.db.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn count_call(&mut self, operation: &str) -> Result<()> {
        self.report.api_calls += 1;
        self.report.estimated_cost_usd += CE_REQUEST_COST_USD;
        self.db(|db| db.record_api_call(self.account_id, "ce", operation, CE_REQUEST_COST_USD))
    }

    /// Fetches all pages of `[start, end)` and replaces the stored rows.
    async fn fetch(
        &mut self,
        granularity: Granularity,
        dimension: &Dimension,
        start: Date,
        end: Date,
    ) -> Result<()> {
        if start >= end {
            return Ok(());
        }
        let (start, end) = (dates::format(start), dates::format(end));
        let mut req = CostRequest {
            granularity,
            dimension: dimension.clone(),
            start: start.clone(),
            end: end.clone(),
            next_token: None,
        };
        let mut rows: Vec<CostRow> = Vec::new();
        loop {
            let page = self.source.cost_and_usage(&req).await?;
            self.count_call("GetCostAndUsage")?;
            for (id, name) in page.account_names {
                self.names.insert(id, Some(name));
            }
            for r in &page.rows {
                self.names.entry(r.linked_account.clone()).or_insert(None);
            }
            rows.extend(page.rows);
            match page.next_token {
                Some(t) => req.next_token = Some(t),
                None => break,
            }
        }
        self.report.rows += rows.len();
        self.db(|db| {
            db.replace_cost_rows(self.account_id, granularity, dimension, &start, &end, &rows)
        })
    }
}

/// Syncs Cost Explorer data for one connected account.
pub async fn sync_account<S: CostSource>(
    db: &Mutex<Storage>,
    source: &S,
    account_id: &str,
    settings: &Settings,
    today: Date,
) -> Result<SyncReport> {
    let mut run = Run {
        db,
        source,
        account_id,
        report: SyncReport::default(),
        names: BTreeMap::new(),
    };

    for dimension in datasets(settings) {
        let state = run.db(|db| db.cost_sync_state(account_id, &dimension))?;
        let (daily_from, fetch_from) = match state {
            None => {
                let monthly_from =
                    dates::add_months(today, -(settings.monthly_history_months as i32));
                let daily_from = dates::add_days(today, -(settings.daily_history_days as i64))
                    .min(dates::add_months(today, -1));
                run.fetch(
                    Granularity::Monthly,
                    &dimension,
                    monthly_from,
                    first_full_month(daily_from),
                )
                .await?;
                (daily_from, daily_from)
            }
            Some((from, through)) => {
                let from = dates::parse(&from)?;
                let refresh = dates::add_days(dates::parse(&through)?, -REFRESH_DAYS).max(from);
                (from, refresh)
            }
        };
        run.fetch(Granularity::Daily, &dimension, fetch_from, today)
            .await?;

        let rollup_from = first_full_month(daily_from).max(dates::first_of_month(fetch_from));
        let names: Vec<(String, Option<String>)> =
            std::mem::take(&mut run.names).into_iter().collect();
        run.db(|db| {
            db.upsert_linked_accounts(account_id, &names)?;
            db.rollup_monthly(account_id, &dimension, &dates::format(rollup_from))?;
            db.set_cost_sync_state(
                account_id,
                &dimension,
                &dates::format(daily_from),
                &dates::format(today),
            )
        })?;
    }

    // Month-end forecast (FR-10): AWS forecasts from today to the end of the month.
    let (start, end) = (
        dates::format(today),
        dates::format(dates::add_months(today, 1)),
    );
    for metric in CostMetric::ALL {
        match source.forecast(metric, &start, &end).await {
            Ok(forecast) => {
                run.count_call("GetCostForecast")?;
                if forecast.is_none() {
                    run.report
                        .warnings
                        .push("AWS has not enough history to forecast yet".into());
                }
                run.db(|db| db.save_forecast(account_id, metric, &start, &end, forecast.as_ref()))?;
            }
            Err(e) => run
                .report
                .warnings
                .push(format!("forecast unavailable: {e}")),
        }
    }
    run.report.warnings.dedup();
    Ok(run.report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::source::{CostPage, Forecast};
    use crate::cost::{CostFilter, CostQuery, GroupBy};
    use crate::profiles::ProfileKind;
    use std::sync::Mutex as StdMutex;

    /// Fake Cost Explorer: $1/day per (account, key) with two pages per call.
    struct FakeCe {
        accounts: Vec<(&'static str, &'static str)>,
        requests: StdMutex<Vec<CostRequest>>,
    }

    impl FakeCe {
        fn new(accounts: Vec<(&'static str, &'static str)>) -> Self {
            Self {
                accounts,
                requests: StdMutex::default(),
            }
        }
    }

    impl CostSource for FakeCe {
        async fn cost_and_usage(&self, req: &CostRequest) -> Result<CostPage> {
            self.requests.lock().unwrap().push(req.clone());
            let keys: &[&str] = match req.dimension {
                Dimension::Service => &["Amazon EC2", "Amazon S3"],
                Dimension::Region => &["us-east-1"],
                Dimension::UsageType => &["BoxUsage"],
                Dimension::Tag(_) => &["prod", ""],
            };
            // Page 1 has the first key, page 2 the rest.
            let (page_keys, next) = match req.next_token.as_deref() {
                None => (&keys[..1], (keys.len() > 1).then(|| "p2".to_owned())),
                Some(_) => (&keys[1..], None),
            };
            let mut rows = Vec::new();
            let mut d = dates::parse(&req.start)?;
            let end = dates::parse(&req.end)?;
            while d < end {
                let next_period = match req.granularity {
                    Granularity::Daily => dates::add_days(d, 1),
                    Granularity::Monthly => dates::add_months(d, 1).min(end),
                };
                let days = (next_period - d).whole_days() as f64;
                for (acct, _) in &self.accounts {
                    for k in page_keys {
                        rows.push(CostRow {
                            period_start: dates::format(d),
                            linked_account: acct.to_string(),
                            key: k.to_string(),
                            unblended: days,
                            amortized: days * 2.0,
                            net_unblended: days * 0.5,
                            unit: "USD".into(),
                            estimated: false,
                        });
                    }
                }
                d = next_period;
            }
            Ok(CostPage {
                rows,
                account_names: self
                    .accounts
                    .iter()
                    .map(|(a, n)| (a.to_string(), n.to_string()))
                    .collect(),
                next_token: next,
            })
        }

        async fn forecast(
            &self,
            _: CostMetric,
            start: &str,
            end: &str,
        ) -> Result<Option<Forecast>> {
            let days = (dates::parse(end)? - dates::parse(start)?).whole_days() as f64;
            Ok(Some(Forecast {
                amount: days * 100.0,
                unit: "USD".into(),
            }))
        }
    }

    fn setup() -> (Mutex<Storage>, String, String) {
        let db = Storage::open_in_memory().unwrap();
        let ws = db.create_workspace("W").unwrap();
        let acc = db
            .add_account(&ws.id, "Payer", "payer", ProfileKind::Sso)
            .unwrap();
        (Mutex::new(db), ws.id, acc.id)
    }

    fn query(ws: &str, start: &str, end: &str, g: Granularity, by: GroupBy) -> CostQuery {
        CostQuery {
            workspace_id: ws.into(),
            start: start.into(),
            end: end.into(),
            granularity: g,
            metric: CostMetric::Unblended,
            group_by: by,
            filter: CostFilter::default(),
        }
    }

    #[tokio::test]
    async fn initial_then_incremental_sync() {
        let (db, ws, acc) = setup();
        let ce = FakeCe::new(vec![("111111111111", "prod"), ("222222222222", "dev")]);
        let today = dates::parse("2026-10-13").unwrap();
        let report = sync_account(&db, &ce, &acc, &Settings::default(), today)
            .await
            .unwrap();

        // 3 datasets × (monthly + daily); SERVICE has 2 pages each; + 3 forecasts.
        assert_eq!(report.api_calls, 2 + 2 + 1 + 1 + 1 + 1 + 3);
        let reqs = ce.requests.lock().unwrap().clone();
        let monthly = reqs
            .iter()
            .find(|r| r.granularity == Granularity::Monthly)
            .unwrap();
        assert_eq!(
            (monthly.start.as_str(), monthly.end.as_str()),
            ("2025-10-01", "2026-09-01")
        );
        let daily = reqs
            .iter()
            .find(|r| r.granularity == Granularity::Daily)
            .unwrap();
        // 60 days back is 2026-08-14, which is before the previous month's start.
        assert_eq!(
            (daily.start.as_str(), daily.end.as_str()),
            ("2026-08-14", "2026-10-13")
        );

        {
            let storage = db.lock().unwrap();
            // Month to date: Oct 1..12 = 12 days × 2 accounts × 2 services × $1.
            let o = storage
                .cost_overview(&ws, CostMetric::Unblended, &[], today)
                .unwrap();
            assert_eq!(o.month_to_date, 48.0);
            assert_eq!(o.previous_month_to_date, 48.0);
            assert_eq!(o.last_month_total, 30.0 * 4.0);
            assert_eq!(o.month_over_month_pct, Some(0.0));
            // MTD actuals + forecast for Oct 13..31 (19 days × $100).
            assert_eq!(o.forecast_month_end, Some(48.0 + 1900.0));
            assert_eq!(o.data_through.as_deref(), Some("2026-10-12"));

            // Monthly view: September comes from the daily rollup, August partly daily.
            let s = storage
                .cost_series(&query(
                    &ws,
                    "2026-01-01",
                    "2026-10-13",
                    Granularity::Monthly,
                    GroupBy::Account,
                ))
                .unwrap();
            let sep = s.periods.iter().position(|p| p == "2026-09-01").unwrap();
            assert_eq!(s.period_totals[sep], 30.0 * 4.0);
            assert_eq!(s.groups[0].label, "dev (222222222222)");
        }

        // Incremental sync next day re-fetches only the last days.
        ce.requests.lock().unwrap().clear();
        let tomorrow = dates::add_days(today, 1);
        sync_account(&db, &ce, &acc, &Settings::default(), tomorrow)
            .await
            .unwrap();
        let reqs = ce.requests.lock().unwrap().clone();
        assert!(reqs.iter().all(|r| r.granularity == Granularity::Daily));
        assert!(
            reqs.iter()
                .all(|r| r.start == "2026-10-08" && r.end == "2026-10-14")
        );

        let storage = db.lock().unwrap();
        let o = storage
            .cost_overview(&ws, CostMetric::Unblended, &[], tomorrow)
            .unwrap();
        assert_eq!(o.month_to_date, 52.0, "no duplicated days after refresh");
        assert!(storage.api_usage("2000-01-01").unwrap()[0].calls > 0);
    }

    #[tokio::test]
    async fn overlapping_sources_are_not_double_counted() {
        let (db, ws, payer) = setup();
        let member = db
            .lock()
            .unwrap()
            .add_account(&ws, "Member", "member", ProfileKind::Sso)
            .unwrap()
            .id;
        let today = dates::parse("2026-10-13").unwrap();
        let payer_ce = FakeCe::new(vec![("111111111111", "prod"), ("222222222222", "dev")]);
        let member_ce = FakeCe::new(vec![("222222222222", "dev")]);
        sync_account(&db, &payer_ce, &payer, &Settings::default(), today)
            .await
            .unwrap();
        sync_account(&db, &member_ce, &member, &Settings::default(), today)
            .await
            .unwrap();

        let storage = db.lock().unwrap();
        let o = storage
            .cost_overview(&ws, CostMetric::Unblended, &[], today)
            .unwrap();
        assert_eq!(o.month_to_date, 48.0);

        let filtered = storage
            .cost_overview(&ws, CostMetric::Unblended, &["222222222222".into()], today)
            .unwrap();
        assert_eq!(filtered.month_to_date, 24.0);
        assert_eq!(filtered.forecast_month_end, None);
    }

    #[tokio::test]
    async fn breakdowns_filters_and_comparison() {
        let (db, ws, acc) = setup();
        let ce = FakeCe::new(vec![("111111111111", "prod")]);
        let today = dates::parse("2026-10-13").unwrap();
        let settings = Settings {
            cost_tag_keys: vec!["env".into()],
            ..Settings::default()
        };
        sync_account(&db, &ce, &acc, &settings, today)
            .await
            .unwrap();
        let storage = db.lock().unwrap();

        let mut q = query(
            &ws,
            "2026-10-01",
            "2026-10-11",
            Granularity::Daily,
            GroupBy::Service,
        );
        q.metric = CostMetric::Amortized;
        let s = storage.cost_series(&q).unwrap();
        assert_eq!(s.periods.len(), 10);
        assert_eq!(s.total, 10.0 * 2.0 * 2.0);
        assert_eq!(s.groups.len(), 2);

        q.filter.services = vec!["Amazon S3".into()];
        assert_eq!(storage.cost_series(&q).unwrap().total, 20.0);

        // Service filter is not available for region breakdowns on CE data.
        q.group_by = GroupBy::Region;
        assert!(storage.cost_series(&q).is_err());

        let tags = storage
            .cost_series(&query(
                &ws,
                "2026-10-01",
                "2026-10-11",
                Granularity::Daily,
                GroupBy::Tag("env".into()),
            ))
            .unwrap();
        let labels: Vec<_> = tags.groups.iter().map(|g| g.label.as_str()).collect();
        assert!(labels.contains(&"(untagged)") && labels.contains(&"prod"));

        let cmp = storage
            .compare_costs(
                &query(
                    &ws,
                    "2026-10-01",
                    "2026-10-11",
                    Granularity::Daily,
                    GroupBy::Service,
                ),
                "2026-09-01",
                "2026-09-06",
            )
            .unwrap();
        assert_eq!(cmp[0].current, 10.0);
        assert_eq!(cmp[0].previous, 5.0);
        assert_eq!(cmp[0].delta_pct, Some(100.0));
    }
}
