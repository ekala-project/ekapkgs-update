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
    let components: Result<Vec<u32>, _> = name.split('_').map(|s| s.parse::<u32>()).collect();

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
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(".")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(is_variant_pinned("v1_2_3"), true);
        assert_eq!(is_variant_pinned("v0_20_1"), true);
        assert_eq!(is_variant_pinned("v1_2_3_4"), true);

        // Not pinned (1-2 components)
        assert_eq!(is_variant_pinned("v1_2"), false);
        assert_eq!(is_variant_pinned("v0_20"), false);
        assert_eq!(is_variant_pinned("v1"), false);

        // Non-version names
        assert_eq!(is_variant_pinned("latest"), false);
        assert_eq!(is_variant_pinned("default"), false);
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
        assert_eq!(extract_version_prefix("v1_2"), Some("1.2".to_string()));
        assert_eq!(extract_version_prefix("v0_20"), Some("0.20".to_string()));
        assert_eq!(extract_version_prefix("v1"), Some("1".to_string()));
        assert_eq!(extract_version_prefix("v1_2_3"), Some("1.2.3".to_string()));
        assert_eq!(extract_version_prefix("latest"), None);
    }
}
