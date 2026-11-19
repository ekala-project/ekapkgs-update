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
/// only their current `HEAD` is recorded. Takes the checkout lock
/// ([`crate::git::checkout_lock`]); callers already holding it use
/// [`sync_repo_locked`].
pub async fn sync_repo(repo: &ResolvedRepo, db: &Database) -> RepoSyncStatus {
    let lock = crate::git::checkout_lock(&repo.repo_dir);
    let _checkout = lock.lock().await;
    sync_repo_locked(repo, db).await
}

/// [`sync_repo`] for a caller that holds the checkout lock.
pub async fn sync_repo_locked(repo: &ResolvedRepo, db: &Database) -> RepoSyncStatus {
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

/// The branch a managed checkout follows.
fn base_branch(repo: &ResolvedRepo) -> &str {
    repo.pr_config
        .as_ref()
        .map_or("master", |c| c.base_branch.as_str())
}

/// The commit the upstream base branch points at, asked from the remote
/// (`git ls-remote`, no fetch). `None` for unmanaged checkouts, which follow
/// no upstream.
pub async fn remote_head(repo: &ResolvedRepo) -> anyhow::Result<Option<String>> {
    if !repo.managed {
        return Ok(None);
    }
    let remote = upstream_remote(repo).await;
    let refname = format!("refs/heads/{}", base_branch(repo));
    let out = run_git_in(&repo.repo_dir, &["ls-remote", &remote, &refname]).await?;
    let sha = parse_ls_remote(&out, &refname)
        .ok_or_else(|| anyhow::anyhow!("{remote} has no {refname}"))?;
    Ok(Some(sha))
}

/// The sha `ls-remote` output lists for exactly `refname`.
fn parse_ls_remote(output: &str, refname: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (sha, name) = line.split_once('\t')?;
        (name.trim() == refname && !sha.is_empty()).then(|| sha.to_owned())
    })
}

/// The checkout's `HEAD` commit.
pub async fn local_head(dir: &Path) -> Option<String> {
    run_git_in(dir, &["rev-parse", "HEAD"]).await.ok()
}

async fn fast_forward(repo: &ResolvedRepo) -> anyhow::Result<Synced> {
    let dir = repo.repo_dir.as_path();
    let base = base_branch(repo);

    let remote = upstream_remote(repo).await;
    fetch_with_retry(repo, &remote).await?;

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

/// Delays between `git fetch` attempts. At boot the service can start before
/// the network is routable even after `network-online.target`; a failed
/// startup sync would otherwise leave the checkout stale until the next
/// re-index.
const FETCH_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(15),
    std::time::Duration::from_secs(45),
];

async fn fetch_with_retry(repo: &ResolvedRepo, remote: &str) -> anyhow::Result<()> {
    let dir = repo.repo_dir.as_path();
    let mut delays = FETCH_RETRY_DELAYS.iter();
    loop {
        match run_git_in(dir, &["fetch", "--prune", remote]).await {
            Ok(_) => return Ok(()),
            Err(e) => match delays.next() {
                Some(delay) => {
                    warn!(
                        "[{}] git fetch {} failed ({:#}); retrying in {}s",
                        repo.label(),
                        remote,
                        e,
                        delay.as_secs()
                    );
                    tokio::time::sleep(*delay).await;
                },
                None => return Err(e),
            },
        }
    }
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
            enumerate_entry_point: dir.join("default.nix").to_string_lossy().to_string(),
            select: None,
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

    #[test]
    fn ls_remote_parsing() {
        let out = "abc\trefs/heads/master\ndef\trefs/heads/master-old\n";
        assert_eq!(
            parse_ls_remote(out, "refs/heads/master").as_deref(),
            Some("abc")
        );
        assert_eq!(
            parse_ls_remote(out, "refs/heads/master-old").as_deref(),
            Some("def")
        );
        assert_eq!(parse_ls_remote(out, "refs/heads/main"), None);
        assert_eq!(parse_ls_remote("", "refs/heads/master"), None);
    }

    #[tokio::test]
    async fn remote_head_reads_upstream_without_fetching() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let (_tmp, upstream, clone) = setup().await;
        let repo = resolved(clone.clone());
        let first = git(&upstream, &["rev-parse", "HEAD"]).await;
        assert_eq!(
            remote_head(&repo).await.unwrap().as_deref(),
            Some(first.as_str())
        );

        commit(&upstream, "b", "two").await;
        let second = git(&upstream, &["rev-parse", "HEAD"]).await;
        assert_eq!(
            remote_head(&repo).await.unwrap().as_deref(),
            Some(second.as_str())
        );
        // The checkout itself only moves when synced.
        assert_eq!(local_head(&clone).await.as_deref(), Some(first.as_str()));

        let unmanaged = ResolvedRepo {
            managed: false,
            ..resolved(clone)
        };
        assert_eq!(remote_head(&unmanaged).await.unwrap(), None);
    }
}
