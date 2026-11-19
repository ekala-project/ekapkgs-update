//! Update groups (`updates.group_parent` / `updates.group_root`).
//!
//! Declarations are resolved by [`crate::groups`]; this module stores the
//! result and answers "which group is this package in, and what are the
//! other members' versions" at dispatch time.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use sqlx::Row;

use super::{Database, version_at_least};

/// A member of an update group, as stored in `updates`.
#[derive(Debug, Clone)]
pub struct GroupMember {
    pub attr_path: String,
    pub current_version: Option<String>,
    pub latest_upstream_version: Option<String>,
}

impl GroupMember {
    /// `(current, latest)` when the latest known upstream release is newer
    /// than the tree's version.
    pub fn pending_update(&self) -> Option<(&str, &str)> {
        let current = self.current_version.as_deref()?;
        let latest = self.latest_upstream_version.as_deref()?;
        (!version_at_least(current, latest)).then_some((current, latest))
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
            "SELECT attr_path, current_version, latest_upstream_version \
             FROM updates WHERE repo_name = ? AND group_root = ? ORDER BY attr_path",
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
                })
            })
            .collect()
    }
}
