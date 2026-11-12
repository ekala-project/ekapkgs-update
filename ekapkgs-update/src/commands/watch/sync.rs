//! Keep watched checkouts current with their upstream (fast-forward only).
//!
//! Before every index build, a managed checkout is fetched from the remote
//! pointing at its upstream repository and fast-forwarded to
//! `<remote>/<base_branch>`. Update worktrees are created from `HEAD`, so
//! this is what keeps proposed updates based on recent upstream state.
//!
//! The sync never rewrites local history: when the checkout has diverged
//! (e.g. branch-mode commits that upstream hasn't merged while upstream
//! moved on), a warning is logged, the stale tree is kept and the repo is
//! recorded as `diverged` for the dashboard.

use std::path::Path;

use tracing::{debug, info, warn};

use super::{ResolvedRepo, run_git_in};
use crate::database::{Database, RepoSyncStatus};
use crate::github::parse_github_url;

/// Fetch and fast-forward a repository checkout, recording the result.
///
/// Unmanaged (legacy single-repo) checkouts are never fetched or merged;
/// only their current `HEAD` is recorded.
pub async fn sync_repo(repo: &ResolvedRepo, db: &Database) -> RepoSyncStatus {
    if !repo.managed {
        record_unmanaged(repo, db).await;
        return RepoSyncStatus::Unmanaged;
    }

    let (status, error) = match fast_forward(repo).await {
        Ok(Synced::UpToDate) => {
            debug!("[{}] Checkout is up to date", repo.label());
            (RepoSyncStatus::Ok, None)
        },
        Ok(Synced::FastForwarded { from, to }) => {
            info!(
                "[{}] Fast-forwarded {} -> {}",
                repo.label(),
                short(&from),
                short(&to)
            );
            (RepoSyncStatus::Ok, None)
        },
        Ok(Synced::Ahead) => {
            info!(
                "[{}] Checkout has local commits not yet upstream; not fast-forwarding",
                repo.label()
            );
            (RepoSyncStatus::Ok, None)
        },
        Ok(Synced::Refused(reason)) => {
            warn!(
                "[{}] {}; keeping the current (possibly stale) tree",
                repo.label(),
                reason
            );
            (RepoSyncStatus::Diverged, Some(reason))
        },
        Err(e) => {
            let message = format!("{e:#}");
            warn!("[{}] Failed to sync checkout: {}", repo.label(), message);
            (RepoSyncStatus::Error, Some(message))
        },
    };

    let head = head_commit(&repo.repo_dir).await;
    if let Err(e) = db
        .record_repo_sync(
            &repo.name,
            status,
            error.as_deref(),
            head.as_ref().map(|(sha, _)| sha.as_str()),
            head.as_ref().map(|(_, subject)| subject.as_str()),
        )
        .await
    {
        warn!("[{}] Failed to record sync status: {:#}", repo.label(), e);
    }
    status
}

/// Record an unmanaged checkout's `HEAD` (no fetch / merge).
async fn record_unmanaged(repo: &ResolvedRepo, db: &Database) {
    let head = head_commit(&repo.repo_dir).await;
    if let Err(e) = db
        .record_repo_sync(
            &repo.name,
            RepoSyncStatus::Unmanaged,
            None,
            head.as_ref().map(|(sha, _)| sha.as_str()),
            head.as_ref().map(|(_, subject)| subject.as_str()),
        )
        .await
    {
        warn!(
            "[{}] Failed to record checkout state: {:#}",
            repo.label(),
            e
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Synced {
    UpToDate,
    FastForwarded {
        from: String,
        to: String,
    },
    /// Local commits on top of upstream; nothing to do.
    Ahead,
    /// Not fast-forwardable; carries the reason.
    Refused(String),
}

async fn fast_forward(repo: &ResolvedRepo) -> anyhow::Result<Synced> {
    let dir = repo.repo_dir.as_path();
    let base = repo
        .pr_config
        .as_ref()
        .map_or("master", |c| c.base_branch.as_str());

    let remote = upstream_remote(repo).await;
    run_git_in(dir, &["fetch", "--prune", &remote]).await?;

    let target = format!("{remote}/{base}");
    let target_sha = run_git_in(
        dir,
        &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
    )
    .await
    .map_err(|e| anyhow::anyhow!("upstream branch {target} not found: {e:#}"))?;
    let head_sha = run_git_in(dir, &["rev-parse", "HEAD"]).await?;

    // Never fast-forward some other branch someone checked out by hand.
    if let Ok(branch) = run_git_in(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await {
        if branch != base {
            return Ok(Synced::Refused(format!(
                "checkout is on branch '{branch}', expected '{base}'"
            )));
        }
    }

    if head_sha == target_sha {
        return Ok(Synced::UpToDate);
    }
    if is_ancestor(dir, &head_sha, &target_sha).await? {
        run_git_in(dir, &["merge", "--ff-only", "--quiet", &target_sha]).await?;
        return Ok(Synced::FastForwarded {
            from: head_sha,
            to: target_sha,
        });
    }
    if is_ancestor(dir, &target_sha, &head_sha).await? {
        return Ok(Synced::Ahead);
    }
    Ok(Synced::Refused(format!(
        "local {base} has diverged from {target}"
    )))
}

/// The remote whose URL points at the configured upstream (falls back to
/// `origin`).
async fn upstream_remote(repo: &ResolvedRepo) -> String {
    let fallback = || "origin".to_owned();
    let Some(cfg) = repo.pr_config.as_ref() else {
        return fallback();
    };
    let Ok(remotes) = run_git_in(&repo.repo_dir, &["remote"]).await else {
        return fallback();
    };
    for name in remotes.lines().map(str::trim).filter(|n| !n.is_empty()) {
        let Ok(url) = run_git_in(&repo.repo_dir, &["remote", "get-url", name]).await else {
            continue;
        };
        if parse_github_url(&url).is_some_and(|gh| {
            gh.owner.eq_ignore_ascii_case(&cfg.owner) && gh.repo.eq_ignore_ascii_case(&cfg.repo)
        }) {
            return name.to_owned();
        }
    }
    fallback()
}

/// `git merge-base --is-ancestor a b`: exit 0 = yes, 1 = no, else error.
async fn is_ancestor(dir: &Path, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
    let output = tokio::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(dir)
        .output()
        .await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "git merge-base --is-ancestor failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// `HEAD` commit sha and subject.
async fn head_commit(dir: &Path) -> Option<(String, String)> {
    let out = run_git_in(dir, &["log", "-1", "--format=%H%x00%s"])
        .await
        .ok()?;
    let (sha, subject) = out.split_once('\0')?;
    Some((sha.to_owned(), subject.to_owned()))
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::git::PrConfig;

    async fn git(dir: &Path, args: &[&str]) -> String {
        run_git_in(dir, args).await.unwrap()
    }

    async fn commit(dir: &Path, file: &str, msg: &str) {
        tokio::fs::write(dir.join(file), msg).await.unwrap();
        git(dir, &["add", file]).await;
        git(dir, &["commit", "--quiet", "-m", msg]).await;
    }

    async fn configure(dir: &Path) {
        git(dir, &["config", "user.email", "t@example.com"]).await;
        git(dir, &["config", "user.name", "t"]).await;
        git(dir, &["config", "commit.gpgsign", "false"]).await;
    }

    fn resolved(dir: PathBuf) -> ResolvedRepo {
        ResolvedRepo {
            name: "core".into(),
            eval_entry_point: dir.join("default.nix").to_string_lossy().to_string(),
            repo_dir: dir,
            pr_config: Some(PrConfig {
                owner: "o".into(),
                repo: "r".into(),
                base_branch: "master".into(),
            }),
            fork: "origin".into(),
            upstream_url: Some("https://github.com/o/r".into()),
            managed: true,
        }
    }

    /// Creates an "upstream" repo and a clone of it.
    async fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("upstream");
        let clone = tmp.path().join("clone");
        tokio::fs::create_dir_all(&upstream).await.unwrap();
        git(&upstream, &["init", "--quiet", "-b", "master"]).await;
        configure(&upstream).await;
        commit(&upstream, "a", "one").await;
        git(
            tmp.path(),
            &[
                "clone",
                "--quiet",
                &upstream.to_string_lossy(),
                &clone.to_string_lossy(),
            ],
        )
        .await;
        configure(&clone).await;
        (tmp, upstream, clone)
    }

    #[tokio::test]
    async fn fast_forward_outcomes() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let (_tmp, upstream, clone) = setup().await;
        let repo = resolved(clone.clone());

        assert_eq!(fast_forward(&repo).await.unwrap(), Synced::UpToDate);

        commit(&upstream, "b", "two").await;
        assert!(matches!(
            fast_forward(&repo).await.unwrap(),
            Synced::FastForwarded { .. }
        ));
        assert_eq!(
            git(&clone, &["rev-parse", "HEAD"]).await,
            git(&upstream, &["rev-parse", "HEAD"]).await
        );

        commit(&clone, "c", "local").await;
        assert_eq!(fast_forward(&repo).await.unwrap(), Synced::Ahead);

        commit(&upstream, "d", "upstream moved").await;
        let local_head = git(&clone, &["rev-parse", "HEAD"]).await;
        assert!(matches!(
            fast_forward(&repo).await.unwrap(),
            Synced::Refused(_)
        ));
        // The stale tree is kept.
        assert_eq!(git(&clone, &["rev-parse", "HEAD"]).await, local_head);

        let (sha, subject) = head_commit(&clone).await.unwrap();
        assert_eq!(sha, local_head);
        assert_eq!(subject, "local");
    }
}
