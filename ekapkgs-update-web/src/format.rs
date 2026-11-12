//! Display helpers shared by every page.
//!
//! Templates receive pre-formatted view models instead of raw database rows,
//! so all the formatting logic (relative times, durations, truncation, badge
//! tones) lives here where it can be unit-tested.

use chrono::{DateTime, NaiveDateTime, Utc};

/// Placeholder rendered for missing values.
pub const DASH: &str = "—";

/// Label used for the empty `repo_name` (the `run` command / legacy watch).
pub const DEFAULT_REPO_LABEL: &str = "default";

/// Parse a stored timestamp.
///
/// Writers use chrono's `to_rfc3339()`, but SQLite's own `datetime()` format
/// (`YYYY-MM-DD HH:MM:SS`, implicitly UTC) is accepted too.
pub fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(raw, fmt).ok())
        .map(|naive| naive.and_utc())
}

/// Human relative time: "just now", "5m ago", "3h ago", "in 2d".
pub fn relative(ts: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - ts).num_seconds();
    let future = secs < 0;
    let abs = secs.unsigned_abs();
    if abs < 45 {
        return if future { "in a moment" } else { "just now" }.to_owned();
    }
    let body = if abs < 3_600 {
        format!("{}m", (abs + 30) / 60)
    } else if abs < 86_400 {
        format!("{}h", abs / 3_600)
    } else if abs < 86_400 * 30 {
        format!("{}d", abs / 86_400)
    } else if abs < 86_400 * 365 {
        format!("{}mo", abs / (86_400 * 30))
    } else {
        format!("{}y", abs / (86_400 * 365))
    };
    if future {
        format!("in {body}")
    } else {
        format!("{body} ago")
    }
}

/// Absolute UTC time, second precision: `2026-11-11 08:40:12 UTC`.
pub fn absolute(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// A timestamp prepared for display: relative text plus the absolute time
/// for the `title` attribute. Rendered by the `ts` macro in `macros.html`.
#[derive(Debug, Clone, Default)]
pub struct Ts {
    /// Machine-readable value for `<time datetime>`; empty when missing.
    pub iso: String,
    /// "3h ago"
    pub rel: String,
    /// "2026-11-11 08:40:12 UTC"
    pub abs: String,
}

impl Ts {
    pub fn new(raw: Option<&str>, now: DateTime<Utc>) -> Self {
        let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
            return Self::default();
        };
        match parse_ts(raw) {
            Some(dt) => Self {
                iso: dt.to_rfc3339(),
                rel: relative(dt, now),
                abs: absolute(dt),
            },
            // Unparseable: show the raw value rather than hiding it.
            None => Self {
                iso: String::new(),
                rel: raw.to_owned(),
                abs: raw.to_owned(),
            },
        }
    }

    pub fn is_set(&self) -> bool {
        !self.rel.is_empty()
    }
}

/// Milliseconds as a compact duration: `850ms`, `4.2s`, `1m 23s`, `2h 5m`.
pub fn duration_ms(ms: i64) -> String {
    if ms < 0 {
        return DASH.to_owned();
    }
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    if ms < 10_000 {
        return format!("{:.1}s", ms as f64 / 1_000.0);
    }
    let s = ms / 1_000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    } else {
        format!("{}d {}h", s / 86_400, (s % 86_400) / 3_600)
    }
}

/// Optional duration; missing values render as a dash.
pub fn duration_opt(ms: Option<i64>) -> String {
    ms.map_or_else(|| DASH.to_owned(), duration_ms)
}

/// Elapsed time between two stored timestamps (or from `start` to `now`).
pub fn elapsed_ms(start: &str, end: Option<&str>, now: DateTime<Utc>) -> Option<i64> {
    let start = parse_ts(start)?;
    let end = match end {
        Some(e) => parse_ts(e)?,
        None => now,
    };
    Some((end - start).num_milliseconds().max(0))
}

/// Truncate to at most `max` characters, appending an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Short display form of a version string.
///
/// Commit-hash versions are cut to 10 characters, very long versions are
/// truncated; everything else is returned unchanged.
pub fn short_version(v: &str) -> String {
    let looks_like_hash = v.len() >= 16 && v.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_hash {
        return v.chars().take(10).collect();
    }
    truncate(v, 24)
}

/// Short form of a session id (UUID): the first 8 characters.
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Short form of a git commit hash.
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// Integer with thousands separators: `1,284`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Compact count for stat tiles: full under 10,000, then `12.9K`, `4.2M`.
pub fn compact(n: i64) -> String {
    let abs = n.unsigned_abs();
    if abs < 10_000 {
        thousands(n)
    } else if abs < 1_000_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// `num / den` as a percentage, `None` when the denominator is zero.
pub fn ratio_pct(num: i64, den: i64) -> Option<f64> {
    (den > 0).then(|| num as f64 * 100.0 / den as f64)
}

/// Percentage with one decimal place, or a dash.
pub fn pct_text(p: Option<f64>) -> String {
    p.map_or_else(|| DASH.to_owned(), |p| format!("{p:.1}%"))
}

/// A CSS percentage for inline `width`/`height` styles, clamped to 0–100.
pub fn css_pct(num: f64, den: f64) -> String {
    if den <= 0.0 {
        return "0%".to_owned();
    }
    format!("{:.2}%", (num / den * 100.0).clamp(0.0, 100.0))
}

/// Pretty-print a JSON string; returns the input unchanged if it isn't JSON.
pub fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| raw.to_owned())
}

/// Human byte size: `812 B`, `4.2 KB`, `1.3 MB`.
pub fn bytes(n: i64) -> String {
    if n < 1_024 {
        format!("{n} B")
    } else if n < 1_024 * 1_024 {
        format!("{:.1} KB", n as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1_024.0 * 1_024.0))
    }
}

/// Display label for a repository name.
pub fn repo_label(name: &str) -> &str {
    if name.is_empty() {
        DEFAULT_REPO_LABEL
    } else {
        name
    }
}

/// Inverse of [`repo_label`] for query parameters: `"default"` → `""`.
pub fn repo_from_param(param: &str) -> String {
    if param == DEFAULT_REPO_LABEL {
        String::new()
    } else {
        param.to_owned()
    }
}

/// Humanise a snake_case identifier: `up_to_date` → `up to date`.
pub fn humanize(s: &str) -> String {
    s.replace('_', " ")
}

/// Percent-encode a string for use in a URL path segment or query value.
///
/// Only RFC 3986 unreserved characters are left as-is.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Build a query string from key/value pairs, skipping empty values.
///
/// The result has no leading `?`; values are percent-encoded.
pub fn query_string(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Join a path and a query string (omitting `?` when the query is empty).
pub fn href(path: &str, query: &str) -> String {
    if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{query}")
    }
}

/// Link to a package detail page.
pub fn package_href(repo_name: &str, attr_path: &str) -> String {
    href(
        &format!("/packages/{}", url_encode(attr_path)),
        &query_string(&[("repo", repo_label(repo_name))]),
    )
}

/// Link to a session detail page.
pub fn session_href(id: &str) -> String {
    format!("/sessions/{}", url_encode(id))
}

/// Return the URL only when it is a plain http(s) link (safe for `href`).
pub fn safe_url(url: Option<&str>) -> Option<String> {
    let url = url?.trim();
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_owned())
}

/// Web page for an upstream source, when one can be derived.
pub fn upstream_url(kind: &str, key: &str, instance: Option<&str>) -> Option<String> {
    let instance = instance.filter(|s| !s.is_empty());
    let url = match kind {
        "github" => format!("https://github.com/{key}/releases"),
        "gitlab" => {
            let host = instance.unwrap_or("gitlab.com");
            let host = host
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            format!("https://{host}/{key}/-/tags")
        },
        "sourcehut" => format!("https://git.sr.ht/{key}/refs"),
        "pypi" => format!("https://pypi.org/project/{key}/#history"),
        "directory" => key.to_owned(),
        _ => return None,
    };
    safe_url(Some(&url))
}

/// Visual tone for a status value. Maps to a `tone-*` CSS class.
///
/// Every status string written by the CLI (package, session, phase, watch,
/// autofix) has an explicit mapping; unknown values are neutral.
pub fn tone(status: &str) -> &'static str {
    match status {
        "up_to_date" | "completed" | "success" | "ok" | "fixed" => "good",
        "outdated" | "backlog" | "diverged" | "stale" => "warn",
        "upstream_error" | "escalated" | "interrupted" | "llm_error" | "parse_error"
        | "apply_error" => "serious",
        "failed" | "error" | "build_failed" => "critical",
        "proposed" | "running" | "indexing" | "processing" | "in_flight" => "info",
        _ => "neutral",
    }
}

/// A status badge: label plus tone. Rendered by the `badge` macro.
#[derive(Debug, Clone)]
pub struct Badge {
    pub label: String,
    pub tone: &'static str,
}

impl Badge {
    /// Badge for a raw status value, labelled with the humanised value.
    pub fn status(status: &str) -> Self {
        Self {
            label: humanize(status),
            tone: tone(status),
        }
    }

    /// Badge with an explicit label and tone.
    pub fn new(label: impl Into<String>, tone: &'static str) -> Self {
        Self {
            label: label.into(),
            tone,
        }
    }

    /// Badge for a package `updates.status`, marking backlog packages.
    pub fn package(status: &str, backlog: bool) -> Self {
        if backlog {
            Self::new("outdated · backlog", "warn")
        } else {
            Self::status(status)
        }
    }
}

/// A version prepared for display. Rendered by the `ver` macro.
#[derive(Debug, Clone, Default)]
pub struct Version {
    pub short: String,
    pub full: String,
}

impl Version {
    pub fn new(v: Option<&str>) -> Self {
        match v.filter(|s| !s.is_empty()) {
            Some(v) => Self {
                short: short_version(v),
                full: v.to_owned(),
            },
            None => Self::default(),
        }
    }

    pub fn is_set(&self) -> bool {
        !self.full.is_empty()
    }
}

/// An `<option>` for a filter `<select>`.
#[derive(Debug, Clone)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

impl SelectOption {
    pub fn new(value: &str, label: &str, current: &str) -> Self {
        Self {
            value: value.to_owned(),
            label: label.to_owned(),
            selected: value == current,
        }
    }
}

/// A filter chip / tab link with its active state.
#[derive(Debug, Clone)]
pub struct Chip {
    pub label: String,
    pub count: Option<String>,
    pub href: String,
    pub active: bool,
    pub tone: &'static str,
}

/// Pagination state with ready-made links.
#[derive(Debug, Clone)]
pub struct Pager {
    pub page: i64,
    pub pages: i64,
    pub total: i64,
    pub total_text: String,
    pub first_item: i64,
    pub last_item: i64,
    pub prev_href: Option<String>,
    pub next_href: Option<String>,
}

impl Pager {
    /// `base_query` holds the active filters (no `page`), already encoded.
    pub fn new(path: &str, base_query: &str, page: i64, per_page: i64, total: i64) -> Self {
        let pages = ((total + per_page - 1) / per_page).max(1);
        let page = page.clamp(1, pages);
        let link = |p: i64| {
            let page_param = format!("page={p}");
            let query = if base_query.is_empty() {
                page_param
            } else {
                format!("{base_query}&{page_param}")
            };
            href(path, &query)
        };
        Self {
            page,
            pages,
            total,
            total_text: thousands(total),
            first_item: if total == 0 {
                0
            } else {
                (page - 1) * per_page + 1
            },
            last_item: (page * per_page).min(total),
            prev_href: (page > 1).then(|| link(page - 1)),
            next_href: (page < pages).then(|| link(page + 1)),
        }
    }

    /// SQL `OFFSET` for this page.
    pub fn offset(&self, per_page: i64) -> i64 {
        (self.page - 1) * per_page
    }

    pub fn is_multi(&self) -> bool {
        self.pages > 1
    }
}

/// Parse a 1-based page number leniently (missing/invalid → 1).
pub fn parse_page(raw: Option<&str>) -> i64 {
    raw.and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(1)
}

/// Treat empty/whitespace query values as absent.
pub fn non_empty(raw: Option<&String>) -> Option<String> {
    raw.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn parse_ts_accepts_writer_formats() {
        let expected = at(2026, 11, 11, 8, 40, 12);
        let cases = [
            "2026-11-11T08:40:12+00:00",
            "2026-11-11T08:40:12.123456789+00:00",
            "2026-11-11T08:40:12Z",
            "2026-11-11 08:40:12",
            "2026-11-11T10:40:12+02:00",
        ];
        for raw in cases {
            let parsed = parse_ts(raw).unwrap_or_else(|| panic!("failed to parse {raw}"));
            assert_eq!(parsed.timestamp(), expected.timestamp(), "{raw}");
        }
        assert!(parse_ts("not a date").is_none());
    }

    #[test]
    fn relative_time_table() {
        let now = at(2026, 11, 11, 12, 0, 0);
        let cases = [
            (at(2026, 11, 11, 11, 59, 50), "just now"),
            (at(2026, 11, 11, 11, 59, 0), "1m ago"),
            (at(2026, 11, 11, 11, 35, 0), "25m ago"),
            (at(2026, 11, 11, 9, 0, 0), "3h ago"),
            (at(2026, 11, 9, 12, 0, 0), "2d ago"),
            (at(2026, 9, 1, 12, 0, 0), "2mo ago"),
            (at(2024, 11, 1, 12, 0, 0), "2y ago"),
            (at(2026, 11, 13, 12, 0, 0), "in 2d"),
            (at(2026, 11, 11, 12, 0, 20), "in a moment"),
        ];
        for (ts, expected) in cases {
            assert_eq!(relative(ts, now), expected, "{ts}");
        }
    }

    #[test]
    fn ts_view_model() {
        let now = at(2026, 11, 11, 12, 0, 1);
        let ts = Ts::new(Some("2026-11-11T09:00:00.5+00:00"), now);
        assert!(ts.is_set());
        assert_eq!(ts.rel, "3h ago");
        assert_eq!(ts.abs, "2026-11-11 09:00:00 UTC");
        assert!(ts.iso.starts_with("2026-11-11T09:00:00"));

        assert!(!Ts::new(None, now).is_set());
        assert!(!Ts::new(Some(""), now).is_set());

        let garbage = Ts::new(Some("yesterday"), now);
        assert_eq!(garbage.rel, "yesterday");
        assert!(garbage.iso.is_empty());
    }

    #[test]
    fn duration_table() {
        let cases = [
            (-5, "—"),
            (0, "0ms"),
            (850, "850ms"),
            (4_200, "4.2s"),
            (12_000, "12s"),
            (83_000, "1m 23s"),
            (3_600_000 + 5 * 60_000, "1h 5m"),
            (2 * 86_400_000 + 3 * 3_600_000, "2d 3h"),
        ];
        for (ms, expected) in cases {
            assert_eq!(duration_ms(ms), expected, "{ms}");
        }
        assert_eq!(duration_opt(None), "—");
        assert_eq!(duration_opt(Some(1_500)), "1.5s");
    }

    #[test]
    fn elapsed_between_timestamps() {
        let now = at(2026, 11, 11, 12, 0, 0);
        assert_eq!(
            elapsed_ms(
                "2026-11-11T11:00:00+00:00",
                Some("2026-11-11T11:01:30+00:00"),
                now
            ),
            Some(90_000)
        );
        assert_eq!(
            elapsed_ms("2026-11-11T11:59:00+00:00", None, now),
            Some(60_000)
        );
        assert_eq!(elapsed_ms("garbage", None, now), None);
    }

    #[test]
    fn truncation_and_versions() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(short_version("1.2.3"), "1.2.3");
        assert_eq!(
            short_version("0123456789abcdef0123456789abcdef01234567"),
            "0123456789"
        );
        assert_eq!(
            short_version("0-unstable-2026-11-11-with-a-very-long-suffix"),
            "0-unstable-2026-11-11-w…"
        );
        assert_eq!(short_id("1b4e28ba-2fa1-11d2-883f-0016d3cca427"), "1b4e28ba");
    }

    #[test]
    fn number_formatting() {
        let cases = [
            (0, "0", "0"),
            (999, "999", "999"),
            (1_284, "1,284", "1,284"),
            (12_900, "12,900", "12.9K"),
            (-1_234_567, "-1,234,567", "-1.2M"),
            (4_200_000, "4,200,000", "4.2M"),
        ];
        for (n, full, short) in cases {
            assert_eq!(thousands(n), full, "{n}");
            assert_eq!(compact(n), short, "{n}");
        }
        assert_eq!(pct_text(ratio_pct(1, 3)), "33.3%");
        assert_eq!(pct_text(ratio_pct(1, 0)), "—");
        assert_eq!(css_pct(5.0, 10.0), "50.00%");
        assert_eq!(css_pct(15.0, 10.0), "100.00%");
        assert_eq!(css_pct(1.0, 0.0), "0%");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(4_300), "4.2 KB");
    }

    #[test]
    fn pretty_json_falls_back_to_raw() {
        assert_eq!(pretty_json(r#"{"a":1}"#), "{\n  \"a\": 1\n}");
        assert_eq!(pretty_json("not json"), "not json");
    }

    #[test]
    fn urls_and_repos() {
        assert_eq!(
            url_encode("python3Packages.foo-bar_1"),
            "python3Packages.foo-bar_1"
        );
        assert_eq!(url_encode("a b/c&d"), "a%20b%2Fc%26d");
        assert_eq!(
            query_string(&[("q", "a b"), ("repo", ""), ("page", "2")]),
            "q=a%20b&page=2"
        );
        assert_eq!(href("/x", ""), "/x");
        assert_eq!(href("/x", "a=1"), "/x?a=1");
        assert_eq!(package_href("", "hello"), "/packages/hello?repo=default");
        assert_eq!(package_href("core", "gtk+3"), "/packages/gtk%2B3?repo=core");
        assert_eq!(repo_label(""), "default");
        assert_eq!(repo_label("core"), "core");
        assert_eq!(repo_from_param("default"), "");
        assert_eq!(repo_from_param("core"), "core");
        assert_eq!(safe_url(Some("javascript:alert(1)")), None);
        assert_eq!(
            safe_url(Some("https://example.org")),
            Some("https://example.org".to_owned())
        );
        assert_eq!(
            upstream_url("github", "NixOS/nix", None).as_deref(),
            Some("https://github.com/NixOS/nix/releases")
        );
        assert_eq!(
            upstream_url("gitlab", "a/b", Some("gitlab.freedesktop.org")).as_deref(),
            Some("https://gitlab.freedesktop.org/a/b/-/tags")
        );
        assert_eq!(upstream_url("script", "/x.sh", None), None);
        assert_eq!(upstream_url("directory", "ftp://x", None), None);
    }

    #[test]
    fn tones_cover_every_status() {
        let cases = [
            ("pending", "neutral"),
            ("up_to_date", "good"),
            ("outdated", "warn"),
            ("proposed", "info"),
            ("failed", "critical"),
            ("upstream_error", "serious"),
            ("running", "info"),
            ("completed", "good"),
            ("cancelled", "neutral"),
            ("success", "good"),
            ("skipped", "neutral"),
            ("diverged", "warn"),
            ("error", "critical"),
            ("unmanaged", "neutral"),
            ("escalated", "serious"),
            ("build_failed", "critical"),
            ("something_new", "neutral"),
        ];
        for (status, expected) in cases {
            assert_eq!(tone(status), expected, "{status}");
        }
        let b = Badge::package("outdated", true);
        assert_eq!(b.label, "outdated · backlog");
        assert_eq!(Badge::status("up_to_date").label, "up to date");
    }

    #[test]
    fn pager_links() {
        let p = Pager::new("/packages", "status=failed", 2, 50, 120);
        assert_eq!(p.pages, 3);
        assert_eq!(p.first_item, 51);
        assert_eq!(p.last_item, 100);
        assert_eq!(p.offset(50), 50);
        assert_eq!(
            p.prev_href.as_deref(),
            Some("/packages?status=failed&page=1")
        );
        assert_eq!(
            p.next_href.as_deref(),
            Some("/packages?status=failed&page=3")
        );

        let empty = Pager::new("/packages", "", 7, 50, 0);
        assert_eq!(empty.page, 1);
        assert_eq!(empty.first_item, 0);
        assert!(empty.prev_href.is_none() && empty.next_href.is_none());

        assert_eq!(parse_page(Some("3")), 3);
        assert_eq!(parse_page(Some("0")), 1);
        assert_eq!(parse_page(Some("abc")), 1);
        assert_eq!(parse_page(None), 1);
    }
}
