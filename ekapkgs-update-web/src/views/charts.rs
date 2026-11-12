//! Chart geometry for the pure-CSS charts (no JavaScript, no CDN).
//!
//! Charts follow the dashboard's data-viz rules: ranked bars use a single
//! hue (bar length already encodes the value), status meaning uses status
//! tokens with a label, every chart has a table-view twin, and tooltips are
//! an enhancement (values are also printed or tabulated).

use chrono::{Duration, NaiveDate};

use crate::format::{Badge, compact, css_pct, pct_text, ratio_pct, thousands};

/// A horizontal bar in a ranked bar list.
#[derive(Debug, Clone)]
pub struct Bar {
    pub label: String,
    /// Optional status badge shown instead of the plain label.
    pub badge: Option<Badge>,
    pub href: Option<String>,
    pub value_text: String,
    pub share: String,
    /// CSS width relative to the largest bar.
    pub width: String,
    pub tip: String,
}

/// Ranked bars (largest first is the caller's ordering), one hue.
pub fn ranked_bars(items: &[(String, i64)]) -> Vec<Bar> {
    let max = items.iter().map(|(_, v)| *v).max().unwrap_or(0);
    let total: i64 = items.iter().map(|(_, v)| *v).sum();
    items
        .iter()
        .map(|(label, value)| {
            let share = pct_text(ratio_pct(*value, total));
            Bar {
                label: label.clone(),
                badge: None,
                href: None,
                value_text: thousands(*value),
                width: css_pct(*value as f64, max as f64),
                tip: format!("{label}: {} ({share})", thousands(*value)),
                share,
            }
        })
        .collect()
}

/// A failure-rate meter: fill tone carries severity.
#[derive(Debug, Clone)]
pub struct Meter {
    pub width: String,
    pub tone: &'static str,
    pub text: String,
}

impl Meter {
    /// Meter for `num` out of `den`, toned by thresholds on the rate.
    pub fn failure_rate(num: i64, den: i64) -> Self {
        let rate = ratio_pct(num, den);
        let tone = match rate {
            Some(r) if r >= 30.0 => "critical",
            Some(r) if r >= 10.0 => "warn",
            _ => "info",
        };
        Self {
            width: css_pct(num as f64, den as f64),
            tone,
            text: pct_text(rate),
        }
    }
}

/// One day in the stacked column chart.
#[derive(Debug, Clone)]
pub struct Column {
    pub date: String,
    /// Axis label (only every few days).
    pub label: String,
    /// Stack height relative to the axis maximum.
    pub height: String,
    pub succeeded: i64,
    pub failed: i64,
    pub succeeded_text: String,
    pub failed_text: String,
    pub rate: String,
    pub tip: String,
    pub has_value: bool,
}

/// A horizontal gridline with its tick label.
#[derive(Debug, Clone)]
pub struct Tick {
    pub label: String,
    pub bottom: String,
}

/// Daily outcome columns: succeeded (de-emphasised) and failed (critical).
#[derive(Debug, Clone, Default)]
pub struct ColumnChart {
    pub columns: Vec<Column>,
    pub ticks: Vec<Tick>,
    pub has_data: bool,
    pub caption: String,
}

/// A "nice" tick step (1/2/5 × 10ⁿ) giving at most `target` intervals.
pub fn nice_step(max: i64, target: i64) -> i64 {
    if max <= 0 || target <= 0 {
        return 1;
    }
    let raw = max as f64 / target as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let factor = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    ((factor * mag).round() as i64).max(1)
}

/// Build the daily chart for `days` days ending at `today` (inclusive).
///
/// `data` holds `(YYYY-MM-DD, succeeded, failed)`; missing days are zero so
/// the time axis stays continuous.
pub fn daily_columns(data: &[(String, i64, i64)], today: NaiveDate, days: i64) -> ColumnChart {
    let days = days.max(1);
    let start = today - Duration::days(days - 1);
    let label_every = match days {
        0..=10 => 1,
        11..=45 => 7,
        _ => 14,
    };
    let lookup = |d: &str| {
        data.iter()
            .find(|(day, _, _)| day == d)
            .map_or((0, 0), |(_, s, f)| (*s, *f))
    };

    let raw: Vec<(NaiveDate, i64, i64)> = (0..days)
        .map(|i| {
            let date = start + Duration::days(i);
            let (s, f) = lookup(&date.format("%Y-%m-%d").to_string());
            (date, s, f)
        })
        .collect();

    let max_total = raw.iter().map(|(_, s, f)| s + f).max().unwrap_or(0);
    let step = nice_step(max_total, 4);
    let axis_max = if max_total == 0 {
        step
    } else {
        ((max_total + step - 1) / step) * step
    };

    let columns = raw
        .iter()
        .enumerate()
        .map(|(i, (date, s, f))| {
            let total = s + f;
            let rate = pct_text(ratio_pct(*s, total));
            let pretty = date.format("%b %-d").to_string();
            // Anchor labels on the newest day and count backwards, so the
            // last column is always labelled and labels never collide.
            let is_labelled = (days - 1 - i as i64) % label_every == 0;
            Column {
                date: date.format("%Y-%m-%d").to_string(),
                label: if is_labelled {
                    pretty.clone()
                } else {
                    String::new()
                },
                height: css_pct(total as f64, axis_max as f64),
                succeeded: *s,
                failed: *f,
                succeeded_text: thousands(*s),
                failed_text: thousands(*f),
                tip: if total == 0 {
                    format!("{pretty}: no finished updates")
                } else {
                    format!(
                        "{pretty}: {} succeeded · {} failed · {rate} success",
                        thousands(*s),
                        thousands(*f)
                    )
                },
                rate,
                has_value: total > 0,
            }
        })
        .collect();

    let ticks = (0..=axis_max / step)
        .map(|k| Tick {
            label: compact(k * step),
            bottom: css_pct((k * step) as f64, axis_max as f64),
        })
        .collect();

    let (sum_s, sum_f) = raw.iter().fold((0, 0), |(a, b), (_, s, f)| (a + s, b + f));
    ColumnChart {
        columns,
        ticks,
        has_data: max_total > 0,
        caption: format!(
            "{} succeeded, {} failed over {days} days ({} success)",
            thousands(sum_s),
            thousands(sum_f),
            pct_text(ratio_pct(sum_s, sum_s + sum_f))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps() {
        let cases = [
            (0, 1),
            (3, 1),
            (8, 2),
            (17, 5),
            (40, 10),
            (95, 50),
            (1_234, 500),
        ];
        for (max, expected) in cases {
            assert_eq!(nice_step(max, 4), expected, "{max}");
        }
    }

    #[test]
    fn ranked_bars_scale_to_largest() {
        let bars = ranked_bars(&[("a".to_owned(), 10), ("b".to_owned(), 5)]);
        assert_eq!(bars[0].width, "100.00%");
        assert_eq!(bars[1].width, "50.00%");
        assert_eq!(bars[1].share, "33.3%");
    }

    #[test]
    fn daily_columns_fill_gaps() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let data = vec![
            ("2026-11-10".to_owned(), 8, 2),
            ("2026-11-11".to_owned(), 3, 0),
        ];
        let chart = daily_columns(&data, today, 7);
        assert_eq!(chart.columns.len(), 7);
        assert_eq!(chart.columns[0].date, "2026-11-05");
        assert!(!chart.columns[0].has_value);
        assert_eq!(chart.columns[5].succeeded, 8);
        assert_eq!(chart.columns[5].rate, "80.0%");
        assert!(chart.has_data);
        // max 10 → step 5 → axis 0, 5, 10
        assert_eq!(chart.ticks.len(), 3);
        assert_eq!(chart.columns[5].height, "100.00%");

        let empty = daily_columns(&[], today, 30);
        assert!(!empty.has_data);
        assert_eq!(empty.columns.len(), 30);
    }

    #[test]
    fn meter_tones() {
        assert_eq!(Meter::failure_rate(0, 10).tone, "info");
        assert_eq!(Meter::failure_rate(2, 10).tone, "warn");
        assert_eq!(Meter::failure_rate(5, 10).tone, "critical");
        assert_eq!(Meter::failure_rate(0, 0).text, "—");
    }
}
