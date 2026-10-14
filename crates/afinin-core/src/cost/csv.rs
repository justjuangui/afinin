// SPDX-License-Identifier: MIT OR Apache-2.0

//! CSV export of cost views (FR-16).

use super::{CostComparisonRow, CostSeries};

fn field(s: &str) -> String {
    // Quote when needed; prefix formula-like values so spreadsheets don't evaluate them.
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_owned()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

fn number(v: f64) -> String {
    format!("{v:.2}")
}

fn line(cells: impl IntoIterator<Item = String>) -> String {
    let mut l = cells.into_iter().collect::<Vec<_>>().join(",");
    l.push_str("\r\n");
    l
}

/// One row per group, one column per period, plus the total.
pub fn series_to_csv(s: &CostSeries) -> String {
    let mut out = line(
        ["Group".to_owned(), "Key".to_owned()]
            .into_iter()
            .chain(s.periods.iter().map(|p| field(p)))
            .chain([format!("Total ({})", s.unit)]),
    );
    for g in &s.groups {
        out += &line(
            [field(&g.label), field(&g.key)]
                .into_iter()
                .chain(g.values.iter().map(|v| number(*v)))
                .chain([number(g.total)]),
        );
    }
    out += &line(
        ["Total".to_owned(), String::new()]
            .into_iter()
            .chain(s.period_totals.iter().map(|v| number(*v)))
            .chain([number(s.total)]),
    );
    out
}

pub fn comparison_to_csv(rows: &[CostComparisonRow]) -> String {
    let mut out =
        line(["Group", "Key", "Current", "Previous", "Change", "Change %"].map(String::from));
    for r in rows {
        out += &line([
            field(&r.label),
            field(&r.key),
            number(r.current),
            number(r.previous),
            number(r.delta),
            r.delta_pct.map(number).unwrap_or_default(),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::CostGroup;

    #[test]
    fn escapes_fields() {
        let s = CostSeries {
            periods: vec!["2026-10-01".into()],
            groups: vec![CostGroup {
                key: "k".into(),
                label: "Acme, \"Inc\"".into(),
                values: vec![1.234],
                total: 1.234,
            }],
            period_totals: vec![1.234],
            total: 1.234,
            unit: "USD".into(),
            estimated_periods: vec![],
        };
        let csv = series_to_csv(&s);
        assert_eq!(
            csv,
            "Group,Key,2026-10-01,Total (USD)\r\n\"Acme, \"\"Inc\"\"\",k,1.23,1.23\r\nTotal,,1.23,1.23\r\n"
        );
        assert_eq!(field("=cmd()"), "'=cmd()");
    }
}
