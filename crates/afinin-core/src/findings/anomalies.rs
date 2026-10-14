// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cost anomaly detection (FR-30): each account × service's daily spend is
//! compared with its own rolling baseline.

use std::collections::BTreeMap;

use time::Date;

use super::{Finding, FindingKind, Sensitivity, Severity};
use crate::cost::dates;

/// Days in the rolling baseline.
pub const BASELINE_DAYS: i64 = 28;
/// Recent days checked for anomalies.
pub const RECENT_DAYS: i64 = 7;
/// Minimum baseline days with data before judging.
const MIN_HISTORY: usize = 14;

/// One day's cost for a linked account and service.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyCost {
    pub account_id: String,
    pub account_label: String,
    pub service: String,
    pub date: String,
    pub amount: f64,
}

fn mean_std(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

/// Daily values, index of the first day with cost, and account label.
type Series<'a> = (Vec<f64>, Option<usize>, &'a str);

/// Detects anomalies on the `RECENT_DAYS` before `today` (exclusive).
pub fn detect(costs: &[DailyCost], sensitivity: Sensitivity, today: Date) -> Vec<Finding> {
    let (k, min_excess) = sensitivity.thresholds();
    let start = dates::add_days(today, -(BASELINE_DAYS + RECENT_DAYS));
    let days: Vec<String> = (0..BASELINE_DAYS + RECENT_DAYS)
        .map(|i| dates::format(dates::add_days(start, i)))
        .collect();
    let index: BTreeMap<&str, usize> = days
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), i))
        .collect();

    // (account, service) → daily series, zeros where no cost was reported.
    let mut series: BTreeMap<(&str, &str), Series> = BTreeMap::new();
    for c in costs {
        let Some(&i) = index.get(c.date.as_str()) else {
            continue;
        };
        let entry = series
            .entry((c.account_id.as_str(), c.service.as_str()))
            .or_insert_with(|| (vec![0.0; days.len()], None, c.account_label.as_str()));
        entry.0[i] += c.amount;
        entry.1 = Some(entry.1.map_or(i, |f| f.min(i)));
    }

    let mut out = Vec::new();
    for ((account, service), (values, first, label)) in series {
        let Some(first) = first else { continue };
        let mut run: Vec<(usize, f64, f64)> = Vec::new(); // (day, cost, baseline mean)
        let flush = |run: &mut Vec<(usize, f64, f64)>, out: &mut Vec<Finding>| {
            if run.is_empty() {
                return;
            }
            let impact: f64 = run.iter().map(|(_, v, m)| v - m).sum();
            let (peak_day, peak, peak_mean) = run
                .iter()
                .copied()
                .max_by(|a, b| (a.1 - a.2).total_cmp(&(b.1 - b.2)))
                .unwrap();
            let from = &days[run[0].0];
            let to = &days[run.last().unwrap().0];
            let mut f = Finding::new(
                "cost-anomaly",
                FindingKind::Anomaly,
                &format!("{account}:{service}:{from}"),
            )
            .evidence("from", from)
            .evidence("to", to)
            .evidence("peak_day", &days[peak_day])
            .evidence("peak_cost", (peak * 100.0).round() / 100.0)
            .evidence("baseline_daily", (peak_mean * 100.0).round() / 100.0)
            .evidence("service", service)
            .evidence("baseline_days", BASELINE_DAYS);
            f.account_id = account.to_owned();
            f.region = String::new();
            f.impact = Some((impact * 100.0).round() / 100.0);
            f.severity = Severity::from_amount(impact);
            let pct = if peak_mean > 0.0 {
                format!(" (+{:.0}%)", (peak - peak_mean) / peak_mean * 100.0)
            } else {
                String::new()
            };
            f.title = format!("{service} spend spiked in {label}");
            f.summary = if from == to {
                format!(
                    "On {from}, {service} cost {peak:.2} vs a typical {peak_mean:.2} per day{pct}."
                )
            } else {
                format!(
                    "From {from} to {to}, {service} cost {impact:.2} more than usual; peak {peak:.2} on {} vs a typical {peak_mean:.2}{pct}.",
                    days[peak_day]
                )
            };
            f.how_to_fix = "Break the service down by usage type and resource for those days to find what changed \
                            (a new resource, a traffic spike, a pricing change). Dismiss the finding if the increase is expected."
                .into();
            out.push(f);
            run.clear();
        };

        for day in (BASELINE_DAYS as usize)..days.len() {
            let window_start = day - BASELINE_DAYS as usize;
            // Only days after the series' first cost count as history.
            let history = &values[window_start.max(first).min(day)..day];
            if history.len() < MIN_HISTORY {
                flush(&mut run, &mut out);
                continue;
            }
            let (mean, std) = mean_std(history);
            // A floor on the deviation keeps perfectly flat series from flagging tiny changes.
            let threshold = mean + k * std.max(mean * 0.1);
            let v = values[day];
            if v > threshold && v - mean >= min_excess {
                run.push((day, v, mean));
            } else {
                flush(&mut run, &mut out);
            }
        }
        flush(&mut run, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn costs(service: &str, values: &[(i64, f64)], today: Date) -> Vec<DailyCost> {
        values
            .iter()
            .map(|(offset, amount)| DailyCost {
                account_id: "111".into(),
                account_label: "prod (111)".into(),
                service: service.into(),
                date: dates::format(dates::add_days(today, *offset)),
                amount: *amount,
            })
            .collect()
    }

    #[test]
    fn flags_spikes_not_noise() {
        let today = dates::parse("2026-10-14").unwrap();
        // 35 days of ~100/day with mild noise; a spike 3 days ago lasting 2 days.
        let mut ec2: Vec<(i64, f64)> = (-35..0)
            .map(|d| (d, 100.0 + (d % 3) as f64 * 2.0))
            .collect();
        ec2[32].1 = 300.0; // today - 3
        ec2[33].1 = 260.0; // today - 2
        let mut all = costs("Amazon EC2", &ec2, today);
        // Noisy but stable service.
        all.extend(costs(
            "Amazon S3",
            &(-35..0)
                .map(|d| (d, 10.0 + (d % 5) as f64))
                .collect::<Vec<_>>(),
            today,
        ));
        // A brand-new service has no baseline yet.
        all.extend(costs("AWS Glue", &[(-2, 500.0)], today));

        let found = detect(&all, Sensitivity::Medium, today);
        assert_eq!(found.len(), 1, "{found:#?}");
        let f = &found[0];
        assert_eq!(f.id, "cost-anomaly:111:Amazon EC2:2026-10-11");
        assert_eq!(f.evidence["from"], "2026-10-11");
        assert_eq!(f.evidence["to"], "2026-10-12");
        assert!(f.impact.unwrap() > 300.0);
        assert_eq!(f.severity, Severity::High);
    }
}
