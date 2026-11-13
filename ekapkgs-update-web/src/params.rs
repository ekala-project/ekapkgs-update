//! Small helpers shared by the API handlers and the queries.

/// Display name of the unnamed repository (`repo_name = ''`, used by `run`
/// and legacy single-repo watch). `ConfigFile::validate` rejects it as a
/// configured repo name, so it is unambiguous.
pub const DEFAULT_REPO_LABEL: &str = "default";

/// Inverse of the frontend's repo label for query parameters:
/// `"default"` → `""`.
pub fn repo_from_param(param: &str) -> String {
    if param == DEFAULT_REPO_LABEL {
        String::new()
    } else {
        param.to_owned()
    }
}

/// `?page=`: a positive page number, defaulting to 1.
pub fn parse_page(raw: Option<&str>) -> i64 {
    raw.and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(1)
}

/// Clamp a requested page to the last page of `total` rows at `limit` per
/// page (`max(1, ceil(total / limit))`), so a stale `?page=` lands on the
/// last page instead of an empty one. An empty result set has one page.
pub fn clamp_page(page: i64, total: i64, limit: i64) -> i64 {
    let limit = limit.max(1);
    let last = ((total.max(0) + limit - 1) / limit).max(1);
    page.clamp(1, last)
}

/// Treat empty/whitespace query values as absent.
pub fn non_empty(raw: Option<&String>) -> Option<String> {
    raw.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// `num / den` as a percentage, or `None` when `den` is zero.
#[allow(clippy::cast_precision_loss)] // counts are far below 2^52
pub fn ratio_pct(num: i64, den: i64) -> Option<f64> {
    (den > 0).then(|| num as f64 * 100.0 / den as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_default_to_one() {
        assert_eq!(parse_page(None), 1);
        assert_eq!(parse_page(Some("3")), 3);
        assert_eq!(parse_page(Some(" 2 ")), 2);
        assert_eq!(parse_page(Some("0")), 1);
        assert_eq!(parse_page(Some("-4")), 1);
        assert_eq!(parse_page(Some("x")), 1);
    }

    #[test]
    fn pages_clamp_to_the_last_page() {
        assert_eq!(clamp_page(1, 0, 50), 1);
        assert_eq!(clamp_page(99, 0, 50), 1);
        assert_eq!(clamp_page(2, 50, 50), 1);
        assert_eq!(clamp_page(2, 51, 50), 2);
        assert_eq!(clamp_page(2, 120, 50), 2);
        assert_eq!(clamp_page(3, 120, 50), 3);
        assert_eq!(clamp_page(99, 120, 50), 3);
        assert_eq!(clamp_page(0, 120, 50), 1);
        assert_eq!(clamp_page(5, 10, 0), 5);
    }

    #[test]
    fn repo_params_map_default_to_empty() {
        assert_eq!(repo_from_param("default"), "");
        assert_eq!(repo_from_param("ekapkgs"), "ekapkgs");
    }

    #[test]
    fn empty_params_are_absent() {
        assert_eq!(non_empty(Some(&"  ".to_owned())), None);
        assert_eq!(non_empty(Some(&" a ".to_owned())), Some("a".to_owned()));
        assert_eq!(non_empty(None), None);
    }

    #[test]
    fn ratios() {
        assert_eq!(ratio_pct(1, 0), None);
        assert_eq!(ratio_pct(1, 4), Some(25.0));
    }
}
