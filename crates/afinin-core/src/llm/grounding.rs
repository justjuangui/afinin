// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checks that the figures in an answer come from the data (FR-46): every
//! amount, percentage or large count must match, after the rounding the
//! answer used, a number in a tool result, the question or the workspace
//! context. The model is told not to compute numbers; this catches when it
//! does anyway.

use serde_json::Value;

/// A figure as written in the answer.
#[derive(Debug, Clone, PartialEq)]
struct Figure {
    text: String,
    value: f64,
    /// Decimal places written, for rounding comparisons.
    decimals: i32,
    /// Written with a k/M/B suffix: compare with a relative tolerance.
    abbreviated: bool,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Figures worth checking: amounts, percentages and numbers of 100 or more.
/// Skips identifiers inside words (EC2, m5.large, USE1), dates and years,
/// long digit runs (account IDs) and citation markers like `[2]`.
fn figures(text: &str) -> Vec<Figure> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit()
            || (i > 0 && (is_word_char(chars[i - 1]) || chars[i - 1] == '.'))
        {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < chars.len()
            && (chars[j].is_ascii_digit()
                || (matches!(chars[j], ',' | '.')
                    && chars.get(j + 1).is_some_and(char::is_ascii_digit)))
        {
            j += 1;
        }
        let raw: String = chars[start..j].iter().collect();
        let prev = start.checked_sub(1).map(|k| chars[k]);
        let next = chars.get(j).copied();
        i = j;

        // Citation markers, dates (2026-09-01), versions and words like 5xx.
        if prev == Some('[') && matches!(next, Some(']' | ',')) {
            continue;
        }
        if matches!(next, Some('-' | '/' | ':'))
            && chars.get(j + 1).is_some_and(char::is_ascii_digit)
        {
            continue;
        }
        if matches!(prev, Some('-' | '/' | ':')) && start >= 2 && chars[start - 2].is_ascii_digit()
        {
            continue;
        }
        let mut suffix_len = 0;
        let mut multiplier = 1.0;
        let mut percent = false;
        match next {
            Some('%') => percent = true,
            Some(c @ ('k' | 'K' | 'M' | 'B'))
                if !chars.get(j + 1).copied().is_some_and(is_word_char) =>
            {
                suffix_len = 1;
                multiplier = match c {
                    'k' | 'K' => 1e3,
                    'M' => 1e6,
                    _ => 1e9,
                };
            }
            Some(c) if is_word_char(c) => continue,
            _ => {}
        }
        let digits = raw.replace(',', "");
        if !raw.contains(['.', ',']) && digits.len() >= 9 {
            continue; // account IDs and other identifiers
        }
        let Ok(value) = digits.parse::<f64>() else {
            continue;
        };
        let money = matches!(prev, Some('$' | '€' | '£')) || {
            let before: String = chars[..start].iter().rev().take(5).collect::<String>();
            before.contains("DSU") // "USD " reversed
        };
        let is_year = !raw.contains(['.', ',']) && (1990.0..=2100.0).contains(&value);
        if !percent && !money && suffix_len == 0 && (value < 100.0 || is_year) {
            continue;
        }
        let decimals = digits.split_once('.').map_or(0, |(_, d)| d.len() as i32);
        out.push(Figure {
            text: format!(
                "{}{raw}{}",
                if money {
                    prev.filter(|c| !c.is_whitespace())
                        .map(String::from)
                        .unwrap_or_default()
                } else {
                    String::new()
                },
                if percent {
                    "%".to_owned()
                } else {
                    chars[j..j + suffix_len].iter().collect()
                }
            ),
            value: value * multiplier,
            decimals,
            abbreviated: suffix_len > 0,
        });
    }
    out
}

/// Every number in a JSON value, including numeric strings.
fn collect_numbers(v: &Value, out: &mut Vec<f64>) {
    match v {
        Value::Number(n) => out.extend(n.as_f64()),
        Value::String(s) => out.extend(figures_loose(s)),
        Value::Array(a) => a.iter().for_each(|x| collect_numbers(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_numbers(x, out)),
        _ => {}
    }
}

/// All plain numbers in free text (question, context), without filtering.
fn figures_loose(text: &str) -> Vec<f64> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .filter_map(|t| {
            t.trim_matches(['.', ','])
                .replace(',', "")
                .parse::<f64>()
                .ok()
        })
        .collect()
}

fn matches(f: &Figure, source: f64) -> bool {
    let s = source.abs();
    if f.abbreviated {
        return s > 0.0 && ((s - f.value).abs() / s) <= 0.05;
    }
    let scale = 10f64.powi(f.decimals);
    // Same value at the precision the answer wrote, or within half a unit
    // of its last digit (truncation and banker's rounding).
    (s * scale).round() == (f.value * scale).round() || (s - f.value).abs() <= 0.5 / scale
}

/// Figures in `answer` that match no number in the sources.
pub fn unverified(answer: &str, results: &[&Value], texts: &[&str]) -> Vec<String> {
    let mut known = Vec::new();
    for r in results {
        collect_numbers(r, &mut known);
    }
    for t in texts {
        known.extend(figures_loose(t));
    }
    let mut out: Vec<String> = Vec::new();
    for f in figures(answer) {
        if !known.iter().any(|&s| matches(&f, s)) && !out.contains(&f.text) {
            out.push(f.text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(answer: &str) -> Vec<String> {
        figures(answer).into_iter().map(|f| f.text).collect()
    }

    #[test]
    fn finds_amounts_and_skips_identifiers() {
        assert_eq!(
            texts(
                "On 2026-09-01 account 123456789012 spent $1,234.56 on EC2 [2], up 12.5% \
                 (USD 980) across 3 m5.large instances in USE1; 2026 total 1.2k."
            ),
            ["$1,234.56", "12.5%", "980", "1.2k"]
        );
    }

    #[test]
    fn matches_rounded_figures_and_flags_invented_ones() {
        let result = json!({"total": 1234.5649, "change_pct": -12.47, "rows": [{"total": "980.1"}], "forecast": 1180.0});
        let answer = "Spend was $1,234.56 [1], down 12.5% from 980 last month; forecast 1.2k. That is $254.46 more.";
        assert_eq!(unverified(answer, &[&result], &[]), ["$254.46"]);
        // Numbers the user gave (a budget in the context) count as known.
        assert!(
            unverified(
                "Budget is USD 5,000.",
                &[&result],
                &["budget USD 5,000/month"]
            )
            .is_empty()
        );
    }
}
