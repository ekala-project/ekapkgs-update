use anyhow::{Context, Result};
use tracing::debug;

use crate::nix::eval_nix_expr;
use crate::vcs_sources::{SemverStrategy, UpstreamSource};

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
    /// Custom regex for extracting version from tags (from passthru.ekapkgs-update.version-regex)
    pub version_regex: Option<String>,
    /// Platforms with distinct source hashes (from passthru.ekapkgs-update.platform-hashes)
    pub platform_hashes: Option<Vec<String>>,
    /// Explicit GitHub repo override (from passthru.ekapkgs-update.github-repo, e.g. "owner/repo")
    pub github_repo: Option<String>,
    /// Keep mode: retain every patch/minor/major version as its own variant entry
    /// (from passthru.ekapkgs-update.keep, e.g. "patch", "minor", "major")
    pub keep: Option<String>,
    /// Custom script that lists available upstream versions, one per line
    /// (from passthru.ekapkgs-update.update-script, evaluated to a store path)
    pub update_script: Option<String>,
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
            "let pkgs = import {} {{ }}; in pkgs.{}.{}",
            self.eval_entry_point, self.attr_path, attr
        );

        eval_nix_expr(&expr).await.ok()
    }

    pub async fn get_version(&self) -> Result<String> {
        // Try to get version directly
        let expr = format!(
            "let pkgs = import {} {{ }}; in pkgs.{}.version or (builtins.parseDrvName pkgs.{}.name).version",
            self.eval_entry_point, self.attr_path, self.attr_path
        );

        let res = eval_nix_expr(&expr).await?;
        Ok(res)
    }

    pub async fn get_src_url(&self) -> Option<String> {
        // Source URL(s), space-separated. fetchurl may set `url = ""` and put
        // the real locations in `urls`, so `src.url or src.urls` is not enough.
        let url_expr = format!(
            "let pkgs = import {} {{ }}; in let s = pkgs.{}.src; u = s.url or \"\"; in \
             builtins.concatStringsSep \" \" ((if builtins.isString u && u != \"\" then [ u ] \
             else [ ]) ++ builtins.filter (x: builtins.isString x && x != \"\") (s.urls or [ ]))",
            self.eval_entry_point, self.attr_path
        );

        eval_nix_expr(&url_expr)
            .await
            .ok()
            .filter(|url| !url.trim().is_empty())
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

        // Query passthru.ekapkgs-update.version-regex attribute
        let version_regex = package
            .get_attr("passthru.ekapkgs-update.version-regex or null")
            .await
            .and_then(|s| {
                let trimmed = s.trim().trim_matches('"');
                if trimmed.is_empty() || trimmed == "null" {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            });

        // Query passthru.ekapkgs-update.platform-hashes attribute (list of system strings)
        // Uses a full expression since builtins.concatStringsSep can't be appended via get_attr
        let platform_hashes = {
            let expr = format!(
                "let pkgs = import {} {{ }}; in builtins.concatStringsSep \" \" (pkgs.{}.passthru.ekapkgs-update.platform-hashes or [])",
                package.eval_entry_point, package.attr_path
            );
            eval_nix_expr(&expr).await.ok()
        }
        .and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(
                    trimmed
                        .split_whitespace()
                        .map(String::from)
                        .collect::<Vec<_>>(),
                )
            }
        });

        // Query passthru.ekapkgs-update.github-repo attribute (e.g. "owner/repo")
        let github_repo = package
            .get_attr("passthru.ekapkgs-update.github-repo or null")
            .await
            .and_then(|s| {
                let trimmed = s.trim().trim_matches('"');
                if trimmed.is_empty() || trimmed == "null" {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            });

        // Query passthru.ekapkgs-update.keep attribute (e.g. "patch", "minor", "major")
        let keep = package
            .get_attr("passthru.ekapkgs-update.keep or null")
            .await
            .and_then(|s| {
                let trimmed = s.trim().trim_matches('"');
                match trimmed {
                    "patch" | "minor" | "major" => Some(trimmed.to_owned()),
                    _ => None,
                }
            });

        // Query passthru.ekapkgs-update.update-script attribute (store path of a
        // script that lists available upstream versions, one per line)
        let update_script = package
            .get_attr("passthru.ekapkgs-update.update-script or null")
            .await
            .and_then(|s| {
                let trimmed = s.trim().trim_matches('"');
                if trimmed.is_empty() || trimmed == "null" {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
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
            version_regex,
            platform_hashes,
            github_repo,
            keep,
            update_script,
        })
    }

    /// Resolve the upstream source for this package.
    ///
    /// Priority chain (highest to lowest):
    /// 1. `passthru.ekapkgs-update.update-script` — custom version-listing script
    /// 2. `passthru.ekapkgs-update.github-repo` — explicit `"owner/repo"` override
    /// 3. `src.url` parsing — auto-detect GitHub, GitLab, SourceHut, PyPI, etc.
    /// 4. `pname` — assume PyPI package as last resort
    pub fn resolve_upstream_source(&self) -> anyhow::Result<UpstreamSource> {
        self.resolve_upstream_source_with(true)
    }

    /// [`PackageMetadata::resolve_upstream_source`] with control over the
    /// final "PyPI project named after `pname`" fallback.
    ///
    /// That fallback is only sound for Python packages: for anything else
    /// (wrappers, meta-packages, sources without a URL) it would match an
    /// unrelated PyPI project with the same name and propose bogus versions.
    pub fn resolve_upstream_source_with(
        &self,
        allow_pypi_fallback: bool,
    ) -> anyhow::Result<UpstreamSource> {
        // 1. Explicit update-script
        if let Some(ref script_path) = self.update_script {
            return Ok(UpstreamSource::Script {
                script_path: script_path.clone(),
            });
        }

        // 2. Explicit github-repo
        if let Some(ref gh_repo) = self.github_repo {
            let (owner, repo) = gh_repo.split_once('/').context(format!(
                "Invalid github-repo format '{}': expected 'owner/repo'",
                gh_repo
            ))?;
            return Ok(UpstreamSource::GitHub {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
            });
        }

        // 3. Auto-detect from src.url (possibly several space-separated
        // mirrors: the first recognizable one wins)
        if let Some(ref src_url) = self.src_url {
            if let Some(source) = src_url
                .split_whitespace()
                .find_map(UpstreamSource::from_url)
            {
                return Ok(source);
            }
            anyhow::bail!("Could not parse upstream source from URL: {src_url}");
        }

        // 4. Fall back to PyPI via pname
        if let Some(ref pname) = self.pname {
            if !allow_pypi_fallback {
                anyhow::bail!(
                    "No source URL, github-repo or update-script, and not a Python package \
                     (refusing to guess PyPI project '{pname}')"
                );
            }
            return Ok(UpstreamSource::PyPI {
                pname: pname.clone(),
            });
        }

        anyhow::bail!(
            "No source URL, pname, github-repo, or update-script found — \
             cannot determine upstream source"
        )
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
            version: "1.0.0".to_owned(),
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
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        assert_eq!(metadata.skip, Some(true));

        let metadata_false = PackageMetadata {
            version: "1.0.0".to_owned(),
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
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        assert_eq!(metadata_false.skip, Some(false));

        let metadata_none = PackageMetadata {
            version: "1.0.0".to_owned(),
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
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        assert_eq!(metadata_none.skip, None);
    }

    #[test]
    fn test_package_metadata_has_semver_strategy_field() {
        use crate::vcs_sources::SemverStrategy;

        let metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
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
            version_regex: Some("v(.*)".to_owned()),
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        assert_eq!(metadata.semver_strategy, Some(SemverStrategy::Minor));
        assert_eq!(metadata.include_prereleases, Some(true));
        assert_eq!(metadata.version_regex, Some("v(.*)".to_owned()));
    }

    #[test]
    fn test_resolve_upstream_source_update_script_highest_priority() {
        // update-script takes priority over github-repo and src_url
        let metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
            src_url: Some("https://github.com/owner/repo/archive/v1.0.0.tar.gz".to_owned()),
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("test-pkg".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
            version_regex: None,
            platform_hashes: None,
            github_repo: Some("owner/repo".to_owned()),
            keep: None,
            update_script: Some("/nix/store/abc-update-script".to_owned()),
        };

        let source = metadata.resolve_upstream_source().unwrap();
        assert_eq!(source.source_type(), "script");
        assert_eq!(source.source_key(), "/nix/store/abc-update-script");
    }

    #[test]
    fn test_resolve_upstream_source_github_repo() {
        let metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
            src_url: Some("https://go.dev/dl/go1.25.1.src.tar.gz".to_owned()),
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("go".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
            version_regex: None,
            platform_hashes: None,
            github_repo: Some("golang/go".to_owned()),
            keep: None,
            update_script: None,
        };

        let source = metadata.resolve_upstream_source().unwrap();
        assert_eq!(source.source_type(), "github");
        assert_eq!(source.source_key(), "golang/go");
    }

    #[test]
    fn test_resolve_upstream_source_src_url() {
        let metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
            src_url: Some("https://github.com/example/repo/archive/v1.0.0.tar.gz".to_owned()),
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("example".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        let source = metadata.resolve_upstream_source().unwrap();
        assert_eq!(source.source_type(), "github");
        assert_eq!(source.source_key(), "example/repo");
    }

    #[test]
    fn test_resolve_upstream_source_pname_fallback() {
        let metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
            src_url: None,
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("requests".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        let source = metadata.resolve_upstream_source().unwrap();
        assert_eq!(source.source_type(), "pypi");
        assert_eq!(source.source_key(), "requests");

        // Not a Python package: never guess a PyPI project from the pname.
        assert!(metadata.resolve_upstream_source_with(false).is_err());
    }

    #[test]
    fn test_resolve_upstream_source_tries_each_mirror() {
        let metadata = PackageMetadata {
            version: "8.21.0".to_owned(),
            // corepkgs' fetchurl: unrecognized primary mirror, GitHub second.
            src_url: Some(
                "https://curl.haxx.se/download/curl-8.21.0.tar.xz \
                 https://github.com/curl/curl/releases/download/curl-8_21_0/curl-8.21.0.tar.xz"
                    .to_owned(),
            ),
            output_hash: None,
            cargo_hash: None,
            vendor_hash: None,
            npm_deps_hash: None,
            nuget_deps_hash: None,
            composer_deps_hash: None,
            pname: Some("curl".to_owned()),
            description: None,
            homepage: None,
            changelog: None,
            skip: None,
            semver_strategy: None,
            include_prereleases: None,
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        let source = metadata.resolve_upstream_source_with(false).unwrap();
        assert_eq!(source.source_type(), "github");
        assert_eq!(source.source_key(), "curl/curl");
    }

    #[test]
    fn test_resolve_upstream_source_nothing_available() {
        let metadata = PackageMetadata {
            version: "1.0.0".to_owned(),
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
            version_regex: None,
            platform_hashes: None,
            github_repo: None,
            keep: None,
            update_script: None,
        };

        assert!(metadata.resolve_upstream_source().is_err());
    }
}
