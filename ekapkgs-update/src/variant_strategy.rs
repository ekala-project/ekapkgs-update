//! Strategy inference for mkManyVariants package variants

use crate::vcs_sources::SemverStrategy;

/// Parse version components from a variant attribute name
///
/// Examples:
/// - "v1_2" -> Some(vec![1, 2])
/// - "v0_20" -> Some(vec![0, 20])
/// - "v1_2_3" -> Some(vec![1, 2, 3])
/// - "latest" -> None
/// - "default" -> None
pub fn parse_variant_components(variant_name: &str) -> Option<Vec<u32>> {
    // Strip 'v' prefix if present
    let name = variant_name.strip_prefix('v').unwrap_or(variant_name);

    // Try to parse components separated by underscores
    let components: Result<Vec<u32>, _> = name.split('_').map(str::parse::<u32>).collect();

    components.ok()
}

/// Check if a variant should be considered "pinned" (exact version, no auto-updates)
///
/// Variants with 3 or more version components (e.g., v1_2_3) are considered pinned.
///
/// Examples:
/// - "v1_2_3" -> true (pinned)
/// - "v1_2_3_4" -> true (pinned)
/// - "v1_2" -> false (not pinned)
/// - "v1" -> false (not pinned)
pub fn is_variant_pinned(variant_name: &str) -> bool {
    match parse_variant_components(variant_name) {
        Some(components) => components.len() >= 3,
        None => false,
    }
}

/// Infer semver update strategy from variant attribute name
///
/// Strategy inference rules:
/// - Two components (e.g., "v1_2"): Patch - update to latest 1.2.x
/// - One component (e.g., "v1"): Minor - update to latest 1.x.x
/// - Three+ components (e.g., "v1_2_3"): None - pinned, no auto-update
/// - Non-versioned (e.g., "latest", "default"): None
///
/// Returns None if the variant is pinned or doesn't follow version naming.
pub fn infer_strategy_from_variant(variant_name: &str) -> Option<SemverStrategy> {
    match parse_variant_components(variant_name) {
        Some(components) => match components.len() {
            1 => Some(SemverStrategy::Minor), // v1 -> update 1.x.x
            2 => Some(SemverStrategy::Patch), // v1_2 -> update 1.2.x
            _ => None,                        // v1_2_3+ -> pinned, no auto-update
        },
        None => None, // Non-version names like "latest", "default"
    }
}

/// Result of inferring a strategy from an attr_path's version suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrPathStrategy {
    /// Use this semver strategy for updates
    Strategy(SemverStrategy),
    /// The package appears pinned to a specific version — skip updates
    Pinned,
}

/// Infer semver strategy from a package's attr_path by examining trailing
/// version-like suffixes.
///
/// For example, `flex_2_5_39` has suffix `_2_5_39` which parses as 3 components
/// (pinned). `libffi_3_3` has suffix `_3_3` which parses as 2 components (Patch).
/// `elixir_1_17` has suffix `_1_17` → 2 components → Patch strategy.
///
/// Returns `None` if the attr_path does not end with a version-like suffix.
pub fn infer_strategy_from_attr_path(attr_path: &str) -> Option<AttrPathStrategy> {
    // Get the last segment after any '.' (e.g., "python312Packages.cmake" -> "cmake")
    let name = attr_path.rsplit('.').next().unwrap_or(attr_path);

    // Find the longest trailing _N(_N)* suffix
    // Work backwards from the end of the name collecting _digit segments
    let parts: Vec<&str> = name.split('_').collect();
    if parts.len() < 2 {
        return None;
    }

    // Count how many trailing segments are purely numeric
    let mut numeric_suffix_count = 0;
    for part in parts.iter().rev() {
        if part.parse::<u32>().is_ok() {
            numeric_suffix_count += 1;
        } else {
            break;
        }
    }

    if numeric_suffix_count == 0 {
        return None;
    }

    match numeric_suffix_count {
        1 => Some(AttrPathStrategy::Strategy(SemverStrategy::Minor)), // foo_3 -> 3.x
        2 => Some(AttrPathStrategy::Strategy(SemverStrategy::Patch)), // foo_3_3 -> 3.3.x
        _ => Some(AttrPathStrategy::Pinned),                          // foo_2_5_39 -> pinned
    }
}

/// Outcome of deciding how far an automated update may move a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStrategyInference {
    /// Set explicitly via `passthru.ekapkgs-update.semver-strategy`.
    Explicit(SemverStrategy),
    /// Inferred from a variant attribute (`foo.v1_2`) or a version-suffixed
    /// attr path (`foo_3_3`).
    Inferred(SemverStrategy),
    /// The attr path names an exact version (`foo.v1_2_3`, `foo_2_5_39`); the
    /// package must not be auto-updated.
    Pinned,
    /// No signal. Callers fall back to `Minor` for mkManyVariants packages and
    /// `Latest` otherwise.
    Unknown,
}

impl UpdateStrategyInference {
    /// Resolve to a concrete strategy, applying the default for packages
    /// without a signal (`Minor` for mkManyVariants packages, else `Latest`).
    /// Returns `None` for pinned packages.
    pub fn resolve(self, is_many_variants: bool) -> Option<SemverStrategy> {
        match self {
            Self::Explicit(strategy) | Self::Inferred(strategy) => Some(strategy),
            Self::Pinned => None,
            Self::Unknown if is_many_variants => Some(SemverStrategy::Minor),
            Self::Unknown => Some(SemverStrategy::Latest),
        }
    }
}

/// Decide the semver strategy for `attr_path`.
///
/// Precedence: explicit passthru strategy, then the variant name when the attr
/// path selects a mkManyVariants variant (`cmake.v4` → Minor, `foo.v1_2` →
/// Patch, `foo.v1_2_3` → pinned), then a numeric attr-path suffix
/// (`libffi_3_3` → Patch).
pub fn infer_update_strategy(
    explicit: Option<SemverStrategy>,
    attr_path: &str,
) -> UpdateStrategyInference {
    if let Some(strategy) = explicit {
        return UpdateStrategyInference::Explicit(strategy);
    }

    // Only treat the last segment as a variant when it is nested under a
    // package (`cmake.v4`); a top-level package can legitimately be called
    // `v8`.
    if let Some((_, last)) = attr_path.rsplit_once('.') {
        if is_variant_name(last) {
            return match infer_strategy_from_variant(last) {
                Some(strategy) => UpdateStrategyInference::Inferred(strategy),
                None => UpdateStrategyInference::Pinned,
            };
        }
    }

    match infer_strategy_from_attr_path(attr_path) {
        Some(AttrPathStrategy::Strategy(strategy)) => UpdateStrategyInference::Inferred(strategy),
        Some(AttrPathStrategy::Pinned) => UpdateStrategyInference::Pinned,
        None => UpdateStrategyInference::Unknown,
    }
}

/// Whether `segment` looks like a versioned variant name (`v4`, `v1_2`, `v0_20_1`).
fn is_variant_name(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        && parse_variant_components(segment).is_some()
}

/// Determine the semver strategy used for automatic updates of a package,
/// evaluating `pkg ? variants` only when [`infer_update_strategy`] has no
/// signal.
///
/// Returns `None` when the package is pinned and must not be auto-updated.
/// Used where no batch evaluation has already reported mkManyVariants (the
/// watch indexer gets that from its `nix-eval-jobs --apply` pass instead), so
/// every caller picks the same release for a package.
pub async fn effective_strategy(
    eval_entry_point: &str,
    attr_path: &str,
    explicit: Option<SemverStrategy>,
) -> Option<SemverStrategy> {
    let inference = infer_update_strategy(explicit, attr_path);
    let many_variants = inference == UpdateStrategyInference::Unknown
        && crate::nix::is_many_variants_package(eval_entry_point, attr_path)
            .await
            .unwrap_or(false);
    let strategy = inference.resolve(many_variants);
    tracing::debug!(
        "{}: semver strategy {:?} ({:?})",
        attr_path,
        strategy,
        inference
    );
    strategy
}
/// Extract version constraint pattern from variant name for matching
///
/// Examples:
/// - "v1_2" -> Some("1.2")
/// - "v0_20" -> Some("0.20")
/// - "v1" -> Some("1")
/// - "v1_2_3" -> Some("1.2.3")
pub fn extract_version_prefix(variant_name: &str) -> Option<String> {
    parse_variant_components(variant_name).map(|components| {
        components
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(".")
    })
}

/// Extract the first N dot-separated components from a version string
///
/// Used to group upstream releases by version series, matching the
/// granularity of existing variant names.
///
/// Examples:
/// - `version_series_key("0.27.3", 2)` -> Some("0.27")
/// - `version_series_key("3.1.0", 1)` -> Some("3")
/// - `version_series_key("1.2", 2)` -> Some("1.2")
/// - `version_series_key("invalid", 2)` -> None
pub fn version_series_key(version: &str, component_count: usize) -> Option<String> {
    if component_count == 0 {
        return None;
    }

    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < component_count {
        return None;
    }

    // Verify all requested components are numeric
    for part in &parts[..component_count] {
        if part.parse::<u32>().is_err() {
            return None;
        }
    }

    Some(parts[..component_count].join("."))
}

/// Build a variant attribute name from a version series key
///
/// This is the inverse of [`extract_version_prefix`]: replaces `.` with `_`
/// and prepends `v`.
///
/// Examples:
/// - `"0.27"` -> `"v0_27"`
/// - `"3"` -> `"v3"`
pub fn variant_name_from_series_key(series_key: &str) -> String {
    format!("v{}", series_key.replace('.', "_"))
}

/// Determine the component count used by existing variant names
///
/// Returns `Some(N)` when all parseable variant names have exactly N
/// components (1 or 2). Returns `None` when the set is empty, contains
/// no parseable variants, or uses mixed component counts.
///
/// Non-versioned variants (e.g., "latest", "default") and pinned variants
/// (3+ components) are ignored when inferring the count.
pub fn infer_variant_component_count(variants: &[String]) -> Option<usize> {
    let mut counts = std::collections::HashSet::new();

    for name in variants {
        if let Some(components) = parse_variant_components(name) {
            let len = components.len();
            // Only consider 1 or 2 component variants (not pinned 3+)
            if len <= 2 {
                counts.insert(len);
            }
        }
    }

    if counts.len() == 1 {
        counts.into_iter().next()
    } else {
        None
    }
}

/// Convert a keep-mode string to the number of version components each
/// variant entry should have.
///
/// - `"patch"` → 3 (e.g., `v1_26_8`)
/// - `"minor"` → 2 (e.g., `v1_26`)
/// - `"major"` → 1 (e.g., `v1`)
///
/// Returns `None` for unrecognised values.
pub fn keep_component_count(keep: &str) -> Option<usize> {
    match keep {
        "patch" => Some(3),
        "minor" => Some(2),
        "major" => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_update_strategy() {
        use UpdateStrategyInference::{Explicit, Inferred, Pinned, Unknown};

        let cases: &[(Option<SemverStrategy>, &str, UpdateStrategyInference)] = &[
            // Explicit passthru strategy always wins.
            (
                Some(SemverStrategy::Major),
                "cmake.v4",
                Explicit(SemverStrategy::Major),
            ),
            // Variant attributes.
            (None, "cmake.v4", Inferred(SemverStrategy::Minor)),
            (
                None,
                "abseil-cpp.v20230802",
                Inferred(SemverStrategy::Minor),
            ),
            (None, "foo.v1_2", Inferred(SemverStrategy::Patch)),
            (None, "foo.v1_2_3", Pinned),
            // Numeric attr-path suffixes.
            (None, "libffi_3_3", Inferred(SemverStrategy::Patch)),
            (None, "flex_2_5_39", Pinned),
            // No signal.
            (None, "zlib", Unknown),
            (None, "python3.v3_12.pkgs.requests", Unknown),
            // A top-level package named like a variant is not a variant.
            (None, "v8", Unknown),
            // Non-version suffixes are not variants.
            (None, "valgrind.light", Unknown),
            (None, "foo.vendor", Unknown),
        ];

        for (explicit, attr_path, expected) in cases {
            assert_eq!(
                infer_update_strategy(*explicit, attr_path),
                *expected,
                "{attr_path} (explicit: {explicit:?})"
            );
        }
    }

    #[test]
    fn test_parse_variant_components() {
        assert_eq!(parse_variant_components("v1_2"), Some(vec![1, 2]));
        assert_eq!(parse_variant_components("v0_20"), Some(vec![0, 20]));
        assert_eq!(parse_variant_components("v1_2_3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_variant_components("v1"), Some(vec![1]));
        assert_eq!(parse_variant_components("v3_0"), Some(vec![3, 0]));

        // Without 'v' prefix
        assert_eq!(parse_variant_components("1_2"), Some(vec![1, 2]));

        // Non-version names
        assert_eq!(parse_variant_components("latest"), None);
        assert_eq!(parse_variant_components("default"), None);
        assert_eq!(parse_variant_components("stable"), None);
    }

    #[test]
    fn test_is_variant_pinned() {
        // Pinned (3+ components)
        assert!(is_variant_pinned("v1_2_3"));
        assert!(is_variant_pinned("v0_20_1"));
        assert!(is_variant_pinned("v1_2_3_4"));

        // Not pinned (1-2 components)
        assert!(!is_variant_pinned("v1_2"));
        assert!(!is_variant_pinned("v0_20"));
        assert!(!is_variant_pinned("v1"));

        // Non-version names
        assert!(!is_variant_pinned("latest"));
        assert!(!is_variant_pinned("default"));
    }

    #[test]
    fn test_infer_strategy_from_variant() {
        // Two components -> Patch
        assert_eq!(
            infer_strategy_from_variant("v1_2"),
            Some(SemverStrategy::Patch)
        );
        assert_eq!(
            infer_strategy_from_variant("v0_20"),
            Some(SemverStrategy::Patch)
        );

        // One component -> Minor
        assert_eq!(
            infer_strategy_from_variant("v1"),
            Some(SemverStrategy::Minor)
        );
        assert_eq!(
            infer_strategy_from_variant("v2"),
            Some(SemverStrategy::Minor)
        );

        // Three+ components -> None (pinned)
        assert_eq!(infer_strategy_from_variant("v1_2_3"), None);
        assert_eq!(infer_strategy_from_variant("v0_20_1"), None);

        // Non-version names -> None
        assert_eq!(infer_strategy_from_variant("latest"), None);
        assert_eq!(infer_strategy_from_variant("default"), None);
    }

    #[test]
    fn test_extract_version_prefix() {
        assert_eq!(extract_version_prefix("v1_2"), Some("1.2".to_owned()));
        assert_eq!(extract_version_prefix("v0_20"), Some("0.20".to_owned()));
        assert_eq!(extract_version_prefix("v1"), Some("1".to_owned()));
        assert_eq!(extract_version_prefix("v1_2_3"), Some("1.2.3".to_owned()));
        assert_eq!(extract_version_prefix("latest"), None);
    }

    #[test]
    fn test_version_series_key() {
        // Two components
        assert_eq!(version_series_key("0.27.3", 2), Some("0.27".to_owned()));
        assert_eq!(version_series_key("1.2.0", 2), Some("1.2".to_owned()));

        // One component
        assert_eq!(version_series_key("3.1.0", 1), Some("3".to_owned()));
        assert_eq!(version_series_key("0.27.3", 1), Some("0".to_owned()));

        // Exact component count
        assert_eq!(version_series_key("1.2", 2), Some("1.2".to_owned()));
        assert_eq!(version_series_key("3", 1), Some("3".to_owned()));

        // Not enough components
        assert_eq!(version_series_key("3", 2), None);
        assert_eq!(version_series_key("", 1), None);

        // Non-numeric
        assert_eq!(version_series_key("abc.def", 2), None);
        assert_eq!(version_series_key("1.beta", 2), None);

        // Zero component count
        assert_eq!(version_series_key("1.2.3", 0), None);
    }

    #[test]
    fn test_variant_name_from_series_key() {
        assert_eq!(variant_name_from_series_key("0.27"), "v0_27");
        assert_eq!(variant_name_from_series_key("3"), "v3");
        assert_eq!(variant_name_from_series_key("1.18"), "v1_18");
    }

    #[test]
    fn test_infer_variant_component_count() {
        // Homogeneous 2-component
        assert_eq!(
            infer_variant_component_count(&[
                "v0_20".to_owned(),
                "v0_23".to_owned(),
                "v0_27".to_owned(),
            ]),
            Some(2)
        );

        // Homogeneous 1-component
        assert_eq!(
            infer_variant_component_count(&["v1".to_owned(), "v2".to_owned(), "v3".to_owned()]),
            Some(1)
        );

        // Mixed → None
        assert_eq!(
            infer_variant_component_count(&["v1".to_owned(), "v1_2".to_owned()]),
            None
        );

        // Empty → None
        assert_eq!(infer_variant_component_count(&[]), None);

        // Only non-parseable → None
        assert_eq!(
            infer_variant_component_count(&["latest".to_owned(), "default".to_owned()]),
            None
        );

        // Pinned (3+) variants are ignored, remaining are homogeneous
        assert_eq!(
            infer_variant_component_count(&[
                "v0_20".to_owned(),
                "v0_23".to_owned(),
                "v0_20_1".to_owned(), // pinned, ignored
            ]),
            Some(2)
        );

        // Only pinned variants → None
        assert_eq!(
            infer_variant_component_count(&["v1_2_3".to_owned(), "v1_2_4".to_owned()]),
            None
        );
    }

    #[test]
    fn test_infer_strategy_from_attr_path() {
        // Version-pinned attr_paths (3+ trailing numeric segments -> Pinned)
        assert_eq!(
            infer_strategy_from_attr_path("flex_2_5_39"),
            Some(AttrPathStrategy::Pinned)
        );

        // Two trailing numeric segments -> Patch
        assert_eq!(
            infer_strategy_from_attr_path("libffi_3_3"),
            Some(AttrPathStrategy::Strategy(SemverStrategy::Patch))
        );
        assert_eq!(
            infer_strategy_from_attr_path("elixir_1_17"),
            Some(AttrPathStrategy::Strategy(SemverStrategy::Patch))
        );

        // One trailing numeric segment -> Minor
        assert_eq!(
            infer_strategy_from_attr_path("catch2_3"),
            Some(AttrPathStrategy::Strategy(SemverStrategy::Minor))
        );

        // No trailing numeric segments -> None (use default)
        assert_eq!(infer_strategy_from_attr_path("hello"), None);
        assert_eq!(infer_strategy_from_attr_path("aws-c-auth"), None);
        assert_eq!(
            infer_strategy_from_attr_path("python312Packages.requests"),
            None
        );

        // Dotted attr_paths: only look at the last segment
        assert_eq!(
            infer_strategy_from_attr_path("llvmPackages_19.libllvm"),
            None
        );
    }

    #[test]
    fn test_keep_component_count() {
        assert_eq!(keep_component_count("patch"), Some(3));
        assert_eq!(keep_component_count("minor"), Some(2));
        assert_eq!(keep_component_count("major"), Some(1));
        assert_eq!(keep_component_count("invalid"), None);
        assert_eq!(keep_component_count(""), None);
    }
}
