use anyhow::Result;
use tracing::debug;

use crate::nix::eval_nix_expr;
use crate::vcs_sources::SemverStrategy;

// Data structure for package metadata
#[derive(Debug)]
pub struct PackageMetadata {
    pub version: String,
    pub src_url: Option<String>,
    pub output_hash: Option<String>,
    pub cargo_hash: Option<String>,
    pub vendor_hash: Option<String>,
    pub npm_deps_hash: Option<String>,
    pub nuget_deps_hash: Option<String>,
    pub composer_deps_hash: Option<String>,
    pub pname: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub changelog: Option<String>,
    /// Whether automatic updates should be skipped (from passthru.ekapkgs-update.skip)
    pub skip: Option<bool>,
    /// Semver strategy for version selection (from passthru.ekapkgs-update.semver-strategy)
    pub semver_strategy: Option<SemverStrategy>,
    /// Whether to include prerelease versions (from passthru.ekapkgs-update.include-prereleases)
    pub include_prereleases: Option<bool>,
}

pub struct PackageQuery {
    eval_entry_point: String,
    attr_path: String,
}

impl PackageQuery {
    pub fn new(eval_entry_point: &str, attr_path: &str) -> Self {
        // Normalize the entry point to a valid Nix filepath
        let eval_path = if eval_entry_point.starts_with('/') || eval_entry_point.starts_with('.') {
            eval_entry_point.to_owned()
        } else {
            format!("./{eval_entry_point}")
        };

        Self {
            eval_entry_point: eval_path,
            attr_path: attr_path.to_owned(),
        }
    }

    pub async fn get_attr(&self, attr: &str) -> Option<String> {
        let expr = format!(
            "with import {} {{ }}; {}.{}",
            self.eval_entry_point, self.attr_path, attr
        );

        eval_nix_expr(&expr).await.ok()
    }

    pub async fn get_version(&self) -> Result<String> {
        // Try to get version directly
        let expr = format!(
            "with import {} {{ }}; {}.version or (builtins.parseDrvName {}.name).version",
            self.eval_entry_point, self.attr_path, self.attr_path
        );

        let res = eval_nix_expr(&expr).await?;
        Ok(res)
    }

    pub async fn get_src_url(&self) -> Option<String> {
        // Try to get source URL
        let url_expr = format!(
            "with import {} {{ }}; builtins.toString ({}.src.url or {}.src.urls)",
            self.eval_entry_point, self.attr_path, self.attr_path
        );

        eval_nix_expr(&url_expr).await.ok()
    }
}

impl PackageMetadata {
    /// Extract package metadata from Nix evaluation
    pub async fn from_attr_path(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<Self> {
        debug!("Extracting metadata for {}", attr_path);
        let package = PackageQuery::new(eval_entry_point, attr_path);

        let version = package.get_version().await?;
        let src_url = package.get_src_url().await;
        let output_hash = package.get_attr("src.outputHash").await;
        let cargo_hash = package.get_attr("cargoHash").await;
        let vendor_hash = package.get_attr("vendorHash").await;
        let npm_deps_hash = package.get_attr("npmDepsHash").await;
        let nuget_deps_hash = package.get_attr("nugetDeps").await;
        let composer_deps_hash = package.get_attr("composerDepsHash").await;
        let pname = package.get_attr("pname").await;
        let description = package.get_attr("meta.description").await;
        let homepage = package.get_attr("meta.homepage").await;
        let changelog = package.get_attr("meta.changelog").await;

        // Query passthru.ekapkgs-update.skip attribute
        let skip = package
            .get_attr("passthru.ekapkgs-update.skip or null")
            .await
            .and_then(|s| match s.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });

        // Query passthru.ekapkgs-update.semver-strategy attribute
        let semver_strategy = package
            .get_attr("passthru.ekapkgs-update.semver-strategy or null")
            .await
            .and_then(|s| {
                let trimmed = s.trim().trim_matches('"');
                if trimmed.is_empty() || trimmed == "null" {
                    None
                } else {
                    trimmed.parse::<SemverStrategy>().ok()
                }
            });

        // Query passthru.ekapkgs-update.include-prereleases attribute
        let include_prereleases = package
            .get_attr("passthru.ekapkgs-update.include-prereleases or null")
            .await
            .and_then(|s| match s.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });

        Ok(PackageMetadata {
            version,
            src_url,
            output_hash,
            cargo_hash,
            vendor_hash,
            npm_deps_hash,
            nuget_deps_hash,
            composer_deps_hash,
            pname,
            description,
            homepage,
            changelog,
            skip,
            semver_strategy,
            include_prereleases,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_metadata_has_skip_field() {
        // This test verifies that PackageMetadata has a skip field
        // We can't easily test the Nix evaluation without a full Nix setup,
        // but we can at least verify the struct has the field
        let metadata = PackageMetadata {
            version: "1.0.0".to_string(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: None,
            description: None,
            homepage: None,
            changelog: None,
            skip: Some(true),
            semver_strategy: None,
            include_prereleases: None,
        };

        assert_eq!(metadata.skip, Some(true));

        let metadata_false = PackageMetadata {
            version: "1.0.0".to_string(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: None,
            description: None,
            homepage: None,
            changelog: None,
            skip: Some(false),
            semver_strategy: None,
            include_prereleases: None,
        };

        assert_eq!(metadata_false.skip, Some(false));

        let metadata_none = PackageMetadata {
            version: "1.0.0".to_string(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: None,
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
        };

        assert_eq!(metadata_none.skip, None);
    }

    #[test]
    fn test_package_metadata_has_semver_strategy_field() {
        use crate::vcs_sources::SemverStrategy;

        let metadata = PackageMetadata {
            version: "1.0.0".to_string(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: None,
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: Some(SemverStrategy::Minor),
            include_prereleases: Some(true),
        };

        assert_eq!(metadata.semver_strategy, Some(SemverStrategy::Minor));
        assert_eq!(metadata.include_prereleases, Some(true));
    }
}
