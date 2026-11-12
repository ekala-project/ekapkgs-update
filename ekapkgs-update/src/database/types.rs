use chrono::{DateTime, Utc};

use crate::vcs_sources::compare_version_components;

/// Lifecycle status of a package row in the `updates` table.
///
/// Stored as the snake_case string returned by [`UpdateStatus::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Known (e.g. indexed by `watch`) but never compared against upstream.
    Pending,
    /// The current version is the best compatible upstream release.
    UpToDate,
    /// A newer compatible release exists and has not been proposed yet.
    Outdated,
    /// A newer version was built and proposed (PR opened or commit created).
    Proposed,
    /// The most recent update attempt failed.
    Failed,
    /// Fetching or selecting upstream releases failed.
    UpstreamError,
}

impl UpdateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::UpToDate => "up_to_date",
            Self::Outdated => "outdated",
            Self::Proposed => "proposed",
            Self::Failed => "failed",
            Self::UpstreamError => "upstream_error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "up_to_date" => Some(Self::UpToDate),
            "outdated" => Some(Self::Outdated),
            "proposed" => Some(Self::Proposed),
            "failed" => Some(Self::Failed),
            "upstream_error" => Some(Self::UpstreamError),
            _ => None,
        }
    }

    /// Derive the status of a package from a fresh upstream observation.
    ///
    /// `latest` is the best compatible upstream version. A previously proposed
    /// or failed attempt for exactly that version keeps its status so the
    /// dashboard does not flip back to "outdated" while a PR is open or a
    /// failure is in backoff.
    pub fn observed(
        current: &str,
        latest: &str,
        proposed: Option<&str>,
        failed_version: Option<&str>,
    ) -> Self {
        if current == latest || compare_version_components(latest, current).is_le() {
            Self::UpToDate
        } else if proposed == Some(latest) {
            Self::Proposed
        } else if failed_version == Some(latest) {
            Self::Failed
        } else {
            Self::Outdated
        }
    }
}

impl std::fmt::Display for UpdateStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A package row from the `updates` table (scoped by repo).
#[derive(Debug, Clone)]
pub struct UpdateRecord {
    #[allow(dead_code)] // read by tests and library users
    pub status: UpdateStatus,
    #[allow(dead_code)] // read by tests and library users
    pub current_version: Option<String>,
    pub latest_upstream_version: Option<String>,
    pub proposed_version: Option<String>,
    /// Upstream version recorded by the first `watch` observation.
    pub baseline_version: Option<String>,
    /// Target version of the last failed attempt.
    pub failed_version: Option<String>,
    /// Consecutive failed attempts (reset on success).
    pub failure_count: i64,
    /// One-line summary of the last failure / upstream error.
    pub last_error: Option<String>,
    pub last_checked: Option<DateTime<Utc>>,
    pub last_attempted: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
}

impl UpdateRecord {
    /// Whether `next_attempt` is still in the future at `now`.
    pub fn in_backoff(&self, now: DateTime<Utc>) -> bool {
        self.next_attempt.is_some_and(|next| next > now)
    }
}

/// Cached package metadata stored alongside the update record so the web
/// dashboard can display it without evaluating Nix.
#[derive(Debug, Clone, Default)]
pub struct PackageInfo {
    pub pname: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub upstream_type: Option<String>,
    pub upstream_key: Option<String>,
    pub upstream_instance: Option<String>,
    pub semver_strategy: Option<String>,
}

impl PackageInfo {
    /// Build from extracted metadata and (optionally) the resolved upstream.
    pub fn from_metadata(
        metadata: &crate::package::PackageMetadata,
        upstream: Option<&crate::vcs_sources::UpstreamSource>,
        strategy: Option<crate::vcs_sources::SemverStrategy>,
    ) -> Self {
        Self {
            pname: metadata.pname.clone(),
            description: metadata.description.clone(),
            homepage: metadata.homepage.clone(),
            upstream_type: upstream.map(|u| u.source_type().to_owned()),
            upstream_key: upstream.map(crate::vcs_sources::UpstreamSource::source_key),
            upstream_instance: upstream.and_then(|u| u.instance().map(str::to_owned)),
            semver_strategy: strategy.map(|s| s.to_string()),
        }
    }
}

/// A failed update attempt to record in `update_logs` and `updates`.
#[derive(Debug, Clone, Default)]
pub struct FailedUpdate<'a> {
    pub repo_name: &'a str,
    pub attr_path: &'a str,
    /// Derivation path, when the request came from `nix-eval-jobs`. A
    /// synthetic `<repo>:<attr>@<version>` key is used otherwise.
    pub drv_path: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub old_version: Option<&'a str>,
    pub new_version: Option<&'a str>,
    pub error_log: &'a str,
    /// `UpdatePhase::as_str()` of the failed phase, when known.
    pub phase: Option<&'a str>,
    /// `UpdateError::error_type_name()`, when known.
    pub error_type: Option<&'a str>,
}

impl FailedUpdate<'_> {
    /// Primary key for the `update_logs` row.
    pub fn log_key(&self) -> String {
        if let Some(drv) = self.drv_path.filter(|d| !d.is_empty() && *d != "unknown") {
            return drv.to_owned();
        }
        let repo = if self.repo_name.is_empty() {
            "default"
        } else {
            self.repo_name
        };
        format!(
            "{repo}:{}@{}",
            self.attr_path,
            self.new_version.unwrap_or("unknown")
        )
    }
}

/// Represents a failed update log entry in the database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdateLog {
    pub drv_path: String,
    pub attr_path: String,
    pub timestamp: String,
    pub status: String,
    pub error_log: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub repo_name: String,
    pub session_id: Option<String>,
    pub phase: Option<String>,
    pub error_type: Option<String>,
}

impl UpdateLog {
    /// Parse the timestamp string as a DateTime<Utc>
    pub fn timestamp_as_datetime(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    }
}

/// Reduce a (possibly multi-kilobyte) error log to a one-line summary for
/// the `updates.last_error` column.
pub fn summarize_error(log: &str) -> String {
    const MAX_LEN: usize = 300;
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.to_ascii_lowercase().contains("error"))
        .or_else(|| lines.last())
        .copied()
        .unwrap_or("");
    if line.chars().count() > MAX_LEN {
        let truncated: String = line.chars().take(MAX_LEN).collect();
        format!("{truncated}…")
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips() {
        for status in [
            UpdateStatus::Pending,
            UpdateStatus::UpToDate,
            UpdateStatus::Outdated,
            UpdateStatus::Proposed,
            UpdateStatus::Failed,
            UpdateStatus::UpstreamError,
        ] {
            assert_eq!(UpdateStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(UpdateStatus::parse("bogus"), None);
    }

    #[test]
    fn observed_status_table() {
        let cases = [
            // (current, latest, proposed, failed, expected)
            ("1.0", "1.0", None, None, UpdateStatus::UpToDate),
            ("1.1", "1.0", None, None, UpdateStatus::UpToDate),
            ("1.0", "1.1", None, None, UpdateStatus::Outdated),
            ("1.0", "1.1", Some("1.1"), None, UpdateStatus::Proposed),
            ("1.0", "1.2", Some("1.1"), None, UpdateStatus::Outdated),
            ("1.0", "1.1", None, Some("1.1"), UpdateStatus::Failed),
            ("1.0", "1.2", None, Some("1.1"), UpdateStatus::Outdated),
            ("1.1", "1.1", Some("1.1"), None, UpdateStatus::UpToDate),
        ];
        for (current, latest, proposed, failed, expected) in cases {
            assert_eq!(
                UpdateStatus::observed(current, latest, proposed, failed),
                expected,
                "current={current} latest={latest} proposed={proposed:?} failed={failed:?}"
            );
        }
    }

    #[test]
    fn log_key_prefers_drv_path() {
        let base = FailedUpdate {
            repo_name: "",
            attr_path: "hello",
            new_version: Some("2.0"),
            ..FailedUpdate::default()
        };
        assert_eq!(base.log_key(), "default:hello@2.0");

        let with_repo = FailedUpdate {
            repo_name: "ekapkgs",
            ..base.clone()
        };
        assert_eq!(with_repo.log_key(), "ekapkgs:hello@2.0");

        let unknown_drv = FailedUpdate {
            drv_path: Some("unknown"),
            ..base.clone()
        };
        assert_eq!(unknown_drv.log_key(), "default:hello@2.0");

        let with_drv = FailedUpdate {
            drv_path: Some("/nix/store/abc-hello.drv"),
            ..base
        };
        assert_eq!(with_drv.log_key(), "/nix/store/abc-hello.drv");
    }

    #[test]
    fn summarize_error_picks_last_error_line() {
        let log = "building...\nerror: hash mismatch\n  got: sha256-x\nnote: something";
        assert_eq!(summarize_error(log), "error: hash mismatch");
        assert_eq!(summarize_error("just one line\n"), "just one line");
        assert_eq!(summarize_error(""), "");
        let long = "error ".repeat(100);
        assert!(summarize_error(&long).ends_with('…'));
    }
}
