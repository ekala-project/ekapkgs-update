//! Update groups (`updates.group_parent` / `updates.group_root`).
//!
//! Declarations are resolved by [`crate::groups`]; this module stores the
//! result and answers "which group is this package in, and what are the
//! other members' versions" at dispatch time.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{Database, parse_rfc3339, version_at_least};

/// A member of an update group, as stored in `updates`.
#[derive(Debug, Clone)]
pub struct GroupMember {
    pub attr_path: String,
    pub current_version: Option<String>,
    pub latest_upstream_version: Option<String>,
    /// The version whose update last failed (`updates.failed_version`).
    pub failed_version: Option<String>,
    /// End of the failure backoff (`updates.next_attempt`).
    pub next_attempt: Option<DateTime<Utc>>,
}

impl GroupMember {
    /// `(current, latest)` when the latest known upstream release is newer
    /// than the tree's version and not blocked at `now`.
    ///
    /// A member whose update to that same version failed and is still in
    /// backoff is left out: otherwise one broken member would fail its group
    /// (and back off every other member) on every attempt, forever. Callers
    /// add the requests that triggered the job regardless.
    pub fn pending_update(&self, now: DateTime<Utc>) -> Option<(&str, &str)> {
        let current = self.current_version.as_deref()?;
        let latest = self.latest_upstream_version.as_deref()?;
        let blocked = self.failed_version.as_deref() == Some(latest)
            && self.next_attempt.is_some_and(|next| next > now);
        (!blocked && !version_at_least(current, latest)).then_some((current, latest))
    }
}

impl Database {
    /// Replace a repo's group assignments.
    ///
    /// `parents` holds every declared (resolved) parent and `roots` every
    /// package's group root (see [`crate::groups::resolve_roots`]). Packages
    /// not listed lose their group. Rows that don't exist (e.g. a root that
    /// is only a label, or a package that is not tracked) are not created.
    pub async fn replace_groups(
        &self,
        repo_name: &str,
        parents: &BTreeMap<String, String>,
        roots: &BTreeMap<String, String>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("begin group transaction")?;
        sqlx::query(
            "UPDATE updates SET group_parent = NULL, group_root = NULL \
             WHERE repo_name = ? AND (group_parent IS NOT NULL OR group_root IS NOT NULL)",
        )
        .bind(repo_name)
        .execute(&mut *tx)
        .await
        .context("clear groups")?;
        for (attr, parent) in parents {
            sqlx::query(
                "UPDATE updates SET group_parent = ? WHERE repo_name = ? AND attr_path = ?",
            )
            .bind(parent)
            .bind(repo_name)
            .bind(attr)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("set group parent of {attr}"))?;
        }
        for (attr, root) in roots {
            sqlx::query("UPDATE updates SET group_root = ? WHERE repo_name = ? AND attr_path = ?")
                .bind(root)
                .bind(repo_name)
                .bind(attr)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("set group root of {attr}"))?;
        }
        tx.commit().await.context("commit groups")?;
        Ok(())
    }

    /// Declared (resolved) parents of a repo's packages: child → parent.
    pub async fn group_parents(&self, repo_name: &str) -> Result<BTreeMap<String, String>> {
        let rows = sqlx::query(
            "SELECT attr_path, group_parent FROM updates \
             WHERE repo_name = ? AND group_parent IS NOT NULL",
        )
        .bind(repo_name)
        .fetch_all(&self.pool)
        .await
        .context("load group parents")?;
        rows.iter()
            .map(|row| Ok((row.try_get("attr_path")?, row.try_get("group_parent")?)))
            .collect()
    }

    /// Every grouped package of a repo: attr path → group root.
    pub async fn group_roots(&self, repo_name: &str) -> Result<BTreeMap<String, String>> {
        let rows = sqlx::query(
            "SELECT attr_path, group_root FROM updates \
             WHERE repo_name = ? AND group_root IS NOT NULL",
        )
        .bind(repo_name)
        .fetch_all(&self.pool)
        .await
        .context("load group roots")?;
        rows.iter()
            .map(|row| Ok((row.try_get("attr_path")?, row.try_get("group_root")?)))
            .collect()
    }

    /// The tracked members of a group (the root included when it is tracked),
    /// ordered by attr path.
    pub async fn group_members(&self, repo_name: &str, root: &str) -> Result<Vec<GroupMember>> {
        let rows = sqlx::query(
            "SELECT attr_path, current_version, latest_upstream_version, failed_version, \
             next_attempt FROM updates WHERE repo_name = ? AND group_root = ? ORDER BY attr_path",
        )
        .bind(repo_name)
        .bind(root)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("load members of group {root}"))?;
        rows.iter()
            .map(|row| {
                Ok(GroupMember {
                    attr_path: row.try_get("attr_path")?,
                    current_version: row.try_get("current_version")?,
                    latest_upstream_version: row.try_get("latest_upstream_version")?,
                    failed_version: row.try_get("failed_version")?,
                    next_attempt: parse_rfc3339(row.try_get("next_attempt")?),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    #[test]
    fn pending_update_skips_members_backed_off_for_the_same_version() {
        let now = Utc::now();
        let member =
            |latest: &str, failed: Option<&str>, next: Option<DateTime<Utc>>| GroupMember {
                attr_path: "aws-c-io".to_owned(),
                current_version: Some("1.0".to_owned()),
                latest_upstream_version: Some(latest.to_owned()),
                failed_version: failed.map(str::to_owned),
                next_attempt: next,
            };
        let later = Some(now + Duration::days(2));
        let earlier = Some(now - Duration::hours(1));
        for (m, expected) in [
            (member("1.1", None, None), Some(("1.0", "1.1"))),
            // Failed on 1.1, still in backoff: left out.
            (member("1.1", Some("1.1"), later), None),
            // Backoff over: retried.
            (member("1.1", Some("1.1"), earlier), Some(("1.0", "1.1"))),
            // A newer release than the one that failed.
            (member("1.2", Some("1.1"), later), Some(("1.0", "1.2"))),
            // Up to date.
            (member("1.0", None, None), None),
        ] {
            assert_eq!(m.pending_update(now), expected, "{m:?}");
        }
    }
}
