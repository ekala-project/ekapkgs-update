use serde::{Deserialize, Serialize};

/// Status of a package in Repology
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageStatus {
    /// Newest stable version
    Newest,
    /// Development/unstable version
    Devel,
    /// Unique to this repository
    Unique,
    /// Outdated version
    Outdated,
    /// Legacy/old version
    Legacy,
    /// Rolling release
    Rolling,
    /// No versioning scheme
    Noscheme,
    /// Incorrect version
    Incorrect,
    /// Untrusted source
    Untrusted,
    /// Ignored package
    Ignored,
}

/// A package entry from Repology API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepologyPackage {
    /// Repository name (e.g., "nix_unstable", "arch", "debian_stable")
    pub repo: String,

    /// Package version
    pub version: String,

    /// Status in this repository
    #[serde(default)]
    pub status: Option<PackageStatus>,

    /// Source package name
    #[serde(default)]
    pub srcname: Option<String>,

    /// Binary package name
    #[serde(default)]
    pub binname: Option<String>,

    /// Visible name
    #[serde(default)]
    pub visiblename: Option<String>,

    /// Package summary/description
    #[serde(default)]
    pub summary: Option<String>,

    /// Package categories
    #[serde(default)]
    pub categories: Option<Vec<String>>,

    /// Package licenses
    #[serde(default)]
    pub licenses: Option<Vec<String>>,

    /// Package maintainers
    #[serde(default)]
    pub maintainers: Option<Vec<String>>,
}

/// Result from querying Repology for a project
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepologyInfo {
    /// Project name in Repology
    pub project_name: String,

    /// All packages for this project across repositories
    pub packages: Vec<RepologyPackage>,

    /// When this data was fetched
    pub fetched_at: String,
}

impl RepologyInfo {
    /// Get the newest stable version from Repology
    ///
    /// Looks for packages with status="newest" and returns the most common version
    /// among them. This helps identify what most distributions consider the latest
    /// stable release.
    pub fn get_newest_version(&self) -> Option<String> {
        use std::collections::HashMap;

        // Collect all "newest" versions
        let newest_versions: Vec<String> = self
            .packages
            .iter()
            .filter(|pkg| {
                matches!(
                    pkg.status,
                    Some(PackageStatus::Newest) | Some(PackageStatus::Unique)
                )
            })
            .map(|pkg| pkg.version.clone())
            .collect();

        if newest_versions.is_empty() {
            return None;
        }

        // Count occurrences of each version
        let mut version_counts: HashMap<String, usize> = HashMap::new();
        for version in &newest_versions {
            *version_counts.entry(version.clone()).or_insert(0) += 1;
        }

        // Return the most common "newest" version
        version_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(version, _)| version)
    }

    /// Get all distributions that have this package
    #[allow(dead_code)]
    pub fn get_distributions(&self) -> Vec<String> {
        self.packages.iter().map(|pkg| pkg.repo.clone()).collect()
    }

    /// Check if a specific version is considered newest by any distribution
    #[allow(dead_code)]
    pub fn is_version_newest(&self, version: &str) -> bool {
        self.packages.iter().any(|pkg| {
            pkg.version == version
                && matches!(
                    pkg.status,
                    Some(PackageStatus::Newest) | Some(PackageStatus::Unique)
                )
        })
    }

    /// Get versions considered outdated
    #[allow(dead_code)]
    pub fn get_outdated_versions(&self) -> Vec<String> {
        self.packages
            .iter()
            .filter(|pkg| matches!(pkg.status, Some(PackageStatus::Outdated)))
            .map(|pkg| pkg.version.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_package(repo: &str, version: &str, status: PackageStatus) -> RepologyPackage {
        RepologyPackage {
            repo: repo.to_string(),
            version: version.to_string(),
            status: Some(status),
            srcname: None,
            binname: None,
            visiblename: None,
            summary: None,
            categories: None,
            licenses: None,
            maintainers: None,
        }
    }

    #[test]
    fn test_get_newest_version() {
        let info = RepologyInfo {
            project_name: "test".to_string(),
            packages: vec![
                create_test_package("arch", "1.2.3", PackageStatus::Newest),
                create_test_package("debian", "1.2.3", PackageStatus::Newest),
                create_test_package("fedora", "1.2.3", PackageStatus::Newest),
                create_test_package("ubuntu", "1.2.0", PackageStatus::Outdated),
            ],
            fetched_at: "2026-05-02T00:00:00Z".to_string(),
        };

        assert_eq!(info.get_newest_version(), Some("1.2.3".to_string()));
    }

    #[test]
    fn test_get_newest_version_multiple() {
        let info = RepologyInfo {
            project_name: "test".to_string(),
            packages: vec![
                create_test_package("arch", "1.2.3", PackageStatus::Newest),
                create_test_package("debian", "1.2.2", PackageStatus::Newest),
                create_test_package("fedora", "1.2.3", PackageStatus::Newest),
            ],
            fetched_at: "2026-05-02T00:00:00Z".to_string(),
        };

        // Should return the most common newest version
        assert_eq!(info.get_newest_version(), Some("1.2.3".to_string()));
    }

    #[test]
    fn test_is_version_newest() {
        let info = RepologyInfo {
            project_name: "test".to_string(),
            packages: vec![
                create_test_package("arch", "1.2.3", PackageStatus::Newest),
                create_test_package("ubuntu", "1.2.0", PackageStatus::Outdated),
            ],
            fetched_at: "2026-05-02T00:00:00Z".to_string(),
        };

        assert!(info.is_version_newest("1.2.3"));
        assert!(!info.is_version_newest("1.2.0"));
    }

    #[test]
    fn test_get_distributions() {
        let info = RepologyInfo {
            project_name: "test".to_string(),
            packages: vec![
                create_test_package("arch", "1.2.3", PackageStatus::Newest),
                create_test_package("debian", "1.2.3", PackageStatus::Newest),
            ],
            fetched_at: "2026-05-02T00:00:00Z".to_string(),
        };

        let distros = info.get_distributions();
        assert_eq!(distros.len(), 2);
        assert!(distros.contains(&"arch".to_string()));
        assert!(distros.contains(&"debian".to_string()));
    }
}
