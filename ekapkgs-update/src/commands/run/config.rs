//! Configuration structures for run mode operations

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::info;

use crate::cli::CommitStrategy;
use crate::commands::pr_enhancements::PrEnhancementsConfig;
use crate::database::{Database, SessionCounter, SessionMode};
use crate::git::PrConfig;

use super::types::UpdateJob;

/// Number of updates the updater service may run at once for `commit_strategy`.
///
/// Branch mode always runs one update at a time, whatever was requested.
/// All branch-mode updates share one working tree. The git mutex serializes
/// only `git add` / `commit` / `checkout`; rewriting, building and
/// `--claude-fix` edits run outside it. `branch_git::DirtySnapshot` treats
/// every path dirtied after an update started as that update's own. With two
/// updates in flight, one would sweep the other's files into its commit, or
/// delete and check them out mid-build when it reverts. Attributing paths per
/// update cannot be made exact: two updates can edit the same shared file, and
/// a fix may touch any path. Serializing whole updates is the only safe
/// choice. Worktree mode isolates each update and keeps the requested
/// concurrency.
#[must_use]
pub fn updater_concurrency(commit_strategy: CommitStrategy, requested: usize) -> usize {
    match commit_strategy {
        CommitStrategy::Branch => 1,
        CommitStrategy::Worktrees => requested.max(1),
    }
}

/// Warn when branch mode ignores a requested `--concurrent-updates` above 1.
pub fn warn_if_branch_concurrency_ignored(commit_strategy: CommitStrategy, requested: usize) {
    if updater_concurrency(commit_strategy, requested) < requested {
        tracing::warn!(
            "--concurrent-updates {} ignored: branch mode runs one update at a time, because \
             concurrent updates in the shared working tree would commit or revert each \
             other's files (use --commit-strategy worktrees for concurrency)",
            requested
        );
    }
}

/// Configuration for automated run mode
#[derive(Debug, Clone, Serialize)]
pub struct RunConfig {
    /// Nix file entry point enumerated with iets eval-jobs
    pub file: String,

    /// iets eval-jobs `--select` applied to `file` before traversal
    pub select: Option<String>,

    /// Nix file attr paths are evaluated, located and built against.
    /// Defaults to `file`; required when `select` is set.
    pub eval_file: Option<String>,

    /// Path to SQLite database for tracking updates
    pub database_path: String,

    /// Upstream git remote for PR creation
    pub upstream: Option<String>,

    /// Fork git remote for PR creation
    pub fork: String,

    /// Whether to run passthru.tests for packages
    pub run_passthru_tests: bool,

    /// Dry-run mode (don't actually perform updates)
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrent_updates: Option<usize>,

    /// Skip packages marked as unstable
    pub skip_unstable: bool,

    /// PR enhancement configuration (CVE, Repology, directory diff, rebuilds)
    pub pr_enhancements: PrEnhancementsConfig,

    /// Interactive mode - prompt before submitting PRs
    pub interactive: bool,

    /// Preserve failed worktrees and artifacts for later inspection
    pub preserve_failures: bool,

    /// Strategy for committing updates
    pub commit_strategy: CommitStrategy,

    /// Invoke Claude Code CLI on build/test failure to attempt a fix
    pub claude_fix: bool,

    /// Maximum agent turns per Claude fix attempt
    pub claude_fix_max_turns: u32,

    /// Timeout in seconds for each Claude fix attempt
    pub claude_fix_timeout: u64,
}

impl RunConfig {
    /// Build a [`RunConfig`] from the flat set of CLI flags accepted by the
    /// `Run` subcommand.
    ///
    /// This keeps `Commands::execute` in `cli.rs` to a tight dispatcher by
    /// pushing the field-shuffling out of the CLI module.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub fn from_args(
        file: String,
        select: Option<String>,
        eval_file: Option<String>,
        database_path: String,
        upstream: Option<String>,
        fork: String,
        run_passthru_tests: bool,
        dry_run: bool,
        concurrent_updates: Option<usize>,
        skip_unstable: bool,
        analyze_rebuilds: bool,
        max_rebuilds: Option<usize>,
        skip_cve: bool,
        skip_repology: bool,
        skip_directory_diff: bool,
        skip_cachix: bool,
        cachix_cache: Option<String>,
        interactive: bool,
        preserve_failures: bool,
        commit_strategy: CommitStrategy,
        audit: bool,
        no_security: bool,
        claude_fix: bool,
        claude_fix_max_turns: u32,
        claude_fix_timeout: u64,
    ) -> Self {
        // Resolve cache precedence: CLI flag → env var → None. The CLI value
        // is moved when present; any whitespace-only value is treated as
        // unset and falls through to `CACHIX_CACHE_NAME`.
        let cachix_cache = match cachix_cache {
            Some(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            _ => crate::cachix::resolve_cache_name(None),
        };
        Self {
            file,
            select,
            eval_file,
            database_path,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            skip_unstable,
            pr_enhancements: PrEnhancementsConfig {
                directory_diff: !skip_directory_diff,
                skip_cve_check: skip_cve,
                skip_repology,
                analyze_rebuilds,
                max_rebuilds,
                skip_cachix,
                cachix_cache,
                run_audit: audit,
                skip_security: no_security,
            },
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
        }
    }

    /// Execute the automated update process
    pub async fn execute(self) -> anyhow::Result<()> {
        // Serialize config before destructuring
        let config_json = serde_json::to_string(&self).ok();

        let RunConfig {
            file,
            select,
            eval_file,
            database_path,
            upstream,
            fork,
            run_passthru_tests,
            dry_run,
            concurrent_updates,
            skip_unstable,
            pr_enhancements,
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
        } = self;

        if select.is_some() && eval_file.is_none() {
            anyhow::bail!(
                "--select requires --eval-file (attribute paths from the selected set must be \
                 evaluated against a file where they resolve, e.g. default.nix)"
            );
        }
        let eval_file = eval_file.unwrap_or_else(|| file.clone());

        info!(
            "Running iets eval-jobs on: {}{}",
            file,
            select
                .as_deref()
                .map(|s| format!(" (select: {s})"))
                .unwrap_or_default()
        );
        if eval_file != file {
            info!("Evaluating packages against: {}", eval_file);
        }

        if claude_fix {
            super::warn_if_claude_unavailable().await;
        }

        // Expand tilde in database path
        let expanded_db_path = shellexpand::tilde(&database_path).to_string();

        // Initialize database
        let db = Database::new(&expanded_db_path).await?;
        info!("Database initialized at: {}", expanded_db_path);

        // Close `run` sessions left `running` by a process that is gone
        // (killed, OOM, crash) so they don't stay running on the dashboard.
        match db.recover_stale_run_sessions().await {
            Ok(0) => {},
            Ok(n) => info!("Closed {} stale run session(s) of exited processes", n),
            Err(e) => tracing::warn!("Failed to close stale run sessions: {:#}", e),
        }

        // Create session for tracking this run
        let session_id = db.create_session(SessionMode::Run, config_json).await?;
        info!("Created session: {}", session_id);

        // Calculate concurrency: use provided value or default to CPU cores / 4 (minimum 1)
        // In interactive mode, force single-threaded execution
        let is_branch_mode = commit_strategy == CommitStrategy::Branch;
        let concurrency = if interactive {
            1
        } else if is_branch_mode {
            // Updates are clamped to 1 in `run_service` (see
            // `updater_concurrency`); the requested value still sizes the
            // release checker.
            concurrent_updates.unwrap_or(1)
        } else {
            concurrent_updates.unwrap_or_else(|| {
                let cpus = num_cpus::get();
                std::cmp::max(1, cpus / 4)
            })
        };
        if is_branch_mode {
            warn_if_branch_concurrency_ignored(commit_strategy, concurrency);
            info!("Running in branch mode (one update at a time, direct commits)");
        } else if interactive {
            info!("Running in interactive mode (single-threaded)");
        } else {
            info!("Running with concurrency level: {}", concurrency);
        }

        // Determine PR configuration: use CLI override or auto-detect from git,
        // inspecting the repository that contains the entry point (which need
        // not be the working directory, e.g. under systemd).
        let entry_dir = std::path::Path::new(&eval_file)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf);
        let pr_config = if let Some(remote_name) = upstream {
            crate::git::PrConfig::from_remote_in(entry_dir.as_deref(), &remote_name).await
        } else {
            crate::git::PrConfig::from_git_upstream_in(entry_dir.as_deref()).await
        };
        let pr_config = match pr_config {
            Ok(config) => Some(config),
            Err(e) => {
                // Worktree updates without a PR are built and then dropped
                // (reported as skipped, packages left `outdated`).
                if !is_branch_mode && !dry_run {
                    tracing::warn!(
                        "No PR configuration ({:#}): updates will be built but not proposed, \
                         and their packages stay outdated. Pass --upstream <remote> or set \
                         an upstream tracking branch to create PRs.",
                        e
                    );
                }
                None
            },
        };

        // Checker → grouper → updater. Requests for update-group members are
        // held back by the grouper until the checker finished (see
        // `grouping::RunGroups`); the rest pass straight through.
        let (tx, mut checker_rx) = mpsc::unbounded_channel::<super::types::UpdateRequest>();
        let (job_tx, rx) = mpsc::unbounded_channel::<UpdateJob>();
        let groups = Arc::new(super::grouping::RunGroups::load(&db, "").await);
        let grouper_groups = Arc::clone(&groups);
        let db_grouper = db.clone();
        let grouper_handle = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Some(req) = checker_rx.recv().await {
                if grouper_groups.should_hold(&req.attr_path) {
                    held.push(req);
                } else if job_tx.send(UpdateJob::Single(req)).is_err() {
                    break;
                }
            }
            for job in grouper_groups.finish(&db_grouper, "", held).await {
                if job_tx.send(job).is_err() {
                    break;
                }
            }
        });

        // Clone data for services
        let db_checker = db.clone();
        let db_updater = db.clone();
        let file_checker = file.clone();
        let eval_file_checker = eval_file.clone();
        let file_updater = eval_file.clone();

        // Spawn release checker service
        let skip_repology = pr_enhancements.skip_repology;
        let skip_cve_scan = pr_enhancements.skip_cve_check;
        let checker_concurrency = concurrency;
        let checker_session = session_id.clone();
        let checker_handle = tokio::spawn(async move {
            // Always limit checker concurrency to avoid OOM from unbounded
            // parallel nix-instantiate calls. Each checker task spawns up to
            // ~16 nix-instantiate processes for metadata extraction, so even
            // modest concurrency can exhaust memory on constrained machines.
            let max_checkers = std::cmp::max(4, checker_concurrency * 2);
            let max_eval_workers = std::cmp::max(1, checker_concurrency);
            super::checker::release_checker_service_with_limits(
                file_checker,
                select,
                eval_file_checker,
                db_checker,
                tx,
                groups,
                skip_unstable,
                skip_repology,
                Some(max_checkers),
                Some(max_eval_workers),
                Some(checker_session),
            )
            .await
        });

        // Spawn updater service
        let updater_config = UpdaterServiceConfig {
            session_id: session_id.clone(),
            eval_entry_point: Arc::from(file_updater), // Convert String to Arc<str>
            pr_config,
            fork: Arc::from(fork), // Convert String to Arc<str>
            run_passthru_tests,
            dry_run,
            concurrency,
            pr_enhancements,
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
            #[cfg(test)]
            executor: None,
        };
        let updater_handle =
            tokio::spawn(async move { updater_config.run_service(rx, db_updater).await });
        // Wait for all services, or stop on SIGINT / SIGTERM.
        let (checker_counts, stats) = supervise_services(
            &db,
            &session_id,
            checker_handle,
            grouper_handle,
            updater_handle,
            shutdown_signal(),
        )
        .await?;
        let (checked_count, skipped_count, error_count) = checker_counts;
        let failed_count = stats.failed;
        let updated_count = stats.updated;
        let packages_attempted = stats.attempted();

        // Refresh known vulnerabilities of the packages this run checked
        // (versions may have changed). Failures never fail the run.
        if !skip_cve_scan {
            let options = crate::cve::scan::ScanOptions {
                repo: Some(String::new()),
                ..crate::cve::scan::ScanOptions::default()
            };
            tokio::select! {
                result = crate::cve::scan::scan_due(db.pool(), &options) => {
                    if let Err(e) = result {
                        tracing::warn!("CVE scan failed: {:#}", e);
                    }
                },
                signal = shutdown_signal() => {
                    tracing::warn!("Received {}, skipping the CVE scan", signal);
                },
            }
        }

        // Display comprehensive summary
        info!("============================================================");
        info!("All services complete!");
        info!("============================================================");
        info!("");
        if dry_run {
            info!("Update Summary (DRY-RUN - no changes made):");
        } else {
            info!("Update Summary:");
        }
        info!("");
        info!("  Packages evaluated:      {:>6}", checked_count);
        if error_count > 0 {
            info!("  Evaluation errors:       {:>6}", error_count);
        }
        info!("");
        info!("  Breakdown:");
        info!("    Skipped (backoff):     {:>6}", skipped_count);
        info!("    Attempted:             {:>6}", packages_attempted);
        info!("    Succeeded:             {:>6}", updated_count);
        info!("    Failed:                {:>6}", failed_count);
        // Counted as skipped in the session, like the updater's skips.
        if stats.dry_run > 0 {
            info!("    Dry-run (not run):     {:>6}", stats.dry_run);
        }
        if stats.skipped > 0 {
            info!("    Skipped (updater):     {:>6}", stats.skipped);
        }
        info!("");

        // Success rate of the updates that were actually performed.
        let performed = updated_count + failed_count;
        if performed > 0 {
            let success_rate = (updated_count as f64 / performed as f64) * 100.0;
            info!(
                "  Success rate:            {:>5.1}% ({}/{})",
                success_rate, updated_count, performed
            );
            info!("");
        }

        info!("  Session ID: {}", session_id);
        info!("  Database: {}", expanded_db_path);
        info!("============================================================");

        // Additional warnings/notices
        if skipped_count > packages_attempted * 10 {
            info!("");
            info!(
                "NOTICE: {} packages were skipped due to backoff cooldown",
                skipped_count
            );
            info!("These packages have recently failed and are in cooldown period");
        }

        if failed_count > 0 && preserve_failures {
            info!("");
            info!(
                "NOTICE: {} updates failed - artifacts preserved for inspection",
                failed_count
            );
            info!("Location: ~/.cache/ekapkgs-update/failed/{}/", session_id);
        }

        Ok(())
    }
}

/// Packages evaluated, skipped by backoff and failed to evaluate (checker).
type CheckerCounts = (usize, usize, usize);

/// How long a shutdown waits for aborted services to stop.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Wait for the services of a run and close its session.
///
/// When `shutdown` resolves first (SIGINT / SIGTERM, see [`shutdown_signal`]),
/// dispatch and the updates in flight are aborted and the session is closed
/// as `cancelled` (instead of staying `running` forever). A panicked service
/// closes it as `failed`. Otherwise it is finalized as `completed`, or
/// `failed` when an update failed. Outcome counters were maintained live and
/// are kept either way.
///
/// # Errors
/// On a shutdown signal or a panicked checker / updater.
async fn supervise_services(
    db: &Database,
    session_id: &str,
    mut checker_handle: tokio::task::JoinHandle<CheckerCounts>,
    mut grouper_handle: tokio::task::JoinHandle<()>,
    mut updater_handle: tokio::task::JoinHandle<UpdaterStats>,
    shutdown: impl std::future::Future<Output = &'static str>,
) -> anyhow::Result<(CheckerCounts, UpdaterStats)> {
    use crate::database::SessionStatus;

    let finished = tokio::select! {
        results = async {
            tokio::join!(&mut checker_handle, &mut grouper_handle, &mut updater_handle)
        } => Ok(results),
        signal = shutdown => Err(signal),
    };
    let (checker_result, grouper_result, updater_result) = match finished {
        Ok(results) => results,
        Err(signal) => {
            tracing::warn!("Received {}, stopping the run", signal);
            checker_handle.abort();
            grouper_handle.abort();
            updater_handle.abort();
            // Aborting the updater drops its tasks (and their worktrees'
            // processes); wait for that before closing the session, but
            // not forever: a task blocked outside an await point (the
            // `--interactive` prompt) only stops when it yields.
            let stopped = async {
                let _ = tokio::join!(checker_handle, grouper_handle, updater_handle);
            };
            if tokio::time::timeout(SHUTDOWN_GRACE, stopped).await.is_err() {
                tracing::warn!("Some update tasks did not stop within {:?}", SHUTDOWN_GRACE);
            }
            if let Err(e) = db.close_session(session_id, SessionStatus::Cancelled).await {
                tracing::warn!("Failed to close interrupted session: {}", e);
            }
            anyhow::bail!("run interrupted by {signal} (session {session_id} cancelled)");
        },
    };
    if let Err(e) = grouper_result {
        tracing::warn!("Update grouping task failed: {}", e);
    }

    // A failed (panicked) service still closes the session.
    let checker_counts = checker_result
        .map_err(|e| tracing::warn!("Release checker task failed: {}", e))
        .ok();
    let stats = updater_result
        .map_err(|e| tracing::warn!("Updater task failed: {}", e))
        .ok();
    let session_status = match (checker_counts, stats) {
        (Some(_), Some(stats)) if stats.failed == 0 => SessionStatus::Completed,
        _ => SessionStatus::Failed,
    };

    let finalized = match checker_counts {
        Some((checked, skipped, _)) => {
            db.finalize_session(session_id, session_status, checked, skipped)
                .await
        },
        None => db.close_session(session_id, session_status).await,
    };
    if let Err(e) = finalized {
        tracing::warn!("Failed to finalize session: {}", e);
    }
    match (checker_counts, stats) {
        (Some(counts), Some(stats)) => Ok((counts, stats)),
        _ => anyhow::bail!("run failed: a service task panicked (session {session_id})"),
    }
}

/// Wait for SIGINT (Ctrl-C) or SIGTERM (systemd stop); returns its name.
///
/// A signal whose handler cannot be installed is never reported.
pub async fn shutdown_signal() -> &'static str {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            },
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => "SIGINT",
        () = terminate => "SIGTERM",
    }
}

/// Configuration for the updater service
#[derive(Debug, Clone)]
pub struct UpdaterServiceConfig {
    /// Session ID for tracking this run
    pub session_id: String,

    /// Nix file entry point (Arc for cheap cloning in async tasks)
    pub eval_entry_point: Arc<str>,

    /// Pull request configuration (if enabled)
    pub pr_config: Option<PrConfig>,

    /// Fork git remote (Arc for cheap cloning in async tasks)
    pub fork: Arc<str>,

    /// Whether to run passthru.tests
    pub run_passthru_tests: bool,

    /// Dry-run mode
    pub dry_run: bool,

    /// Number of concurrent update workers
    pub concurrency: usize,

    /// PR enhancement configuration (CVE, Repology, directory diff, rebuilds)
    pub pr_enhancements: PrEnhancementsConfig,

    /// Interactive mode - prompt before submitting PRs
    pub interactive: bool,

    /// Preserve failed worktrees and artifacts for later inspection
    pub preserve_failures: bool,

    /// Strategy for committing updates
    pub commit_strategy: CommitStrategy,

    /// Invoke Claude Code CLI on build/test failure to attempt a fix
    pub claude_fix: bool,

    /// Maximum agent turns per Claude fix attempt
    pub claude_fix_max_turns: u32,

    /// Timeout in seconds for each Claude fix attempt
    pub claude_fix_timeout: u64,

    /// Runs every job in place of the update workflow (tests only).
    #[cfg(test)]
    pub(crate) executor: Option<JobExecutor>,
}

/// Test seam of [`UpdaterServiceConfig::run_service`]: performs a job (as a
/// task of the service) instead of the update workflow, so tests can hold an
/// update in flight.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct JobExecutor(pub(crate) Arc<dyn Fn(UpdateJob) -> JobFuture + Send + Sync>);

#[cfg(test)]
pub(crate) type JobFuture = std::pin::Pin<Box<dyn std::future::Future<Output = TaskOutput> + Send>>;

#[cfg(test)]
impl std::fmt::Debug for JobExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JobExecutor")
    }
}

/// Outcome counts of an updater service run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdaterStats {
    /// Updates built and committed / proposed.
    pub updated: usize,
    /// Dry-run reports (not performed).
    pub dry_run: usize,
    /// Attempts that failed (including unexpected errors and panics).
    pub failed: usize,
    /// Requests that were not attempted (setup failures, rebuild threshold).
    pub skipped: usize,
}

impl UpdaterStats {
    /// Total requests processed.
    pub fn attempted(&self) -> usize {
        self.updated + self.dry_run + self.failed + self.skipped
    }

    /// Count a finished request and return the session counter to bump.
    fn count(&mut self, result: &anyhow::Result<super::types::UpdateResult>) -> SessionCounter {
        use super::types::UpdateResult;
        match result {
            Ok(UpdateResult::Updated { .. }) => {
                self.updated += 1;
                SessionCounter::Succeeded
            },
            Ok(UpdateResult::DryRun { .. }) => {
                self.dry_run += 1;
                SessionCounter::Skipped
            },
            Ok(UpdateResult::Skipped(_)) => {
                self.skipped += 1;
                SessionCounter::Skipped
            },
            Ok(UpdateResult::Failed(_)) | Err(_) => {
                self.failed += 1;
                SessionCounter::Failed
            },
        }
    }

    /// Count a job whose task panicked: every package update it attempted
    /// (one per group member) failed. Returns the session counters to bump,
    /// matching the `attempted` increments made when the job started.
    fn count_panicked(&mut self, attempts: usize) -> Vec<SessionCounter> {
        self.failed += attempts;
        vec![SessionCounter::Failed; attempts]
    }
}

/// Results of a job run in dry-run mode: one [`UpdateResult::DryRun`] per
/// package, whatever the commit strategy (nothing is edited or committed).
fn dry_run_outputs(job: &UpdateJob) -> TaskOutput {
    job.requests()
        .iter()
        .map(|req| {
            (
                Ok(super::types::UpdateResult::dry_run(req)),
                req.attr_path.clone(),
            )
        })
        .collect()
}

/// Results of one job: one `(result, attr_path)` per attempted package.
pub(crate) type TaskOutput = Vec<(anyhow::Result<super::types::UpdateResult>, String)>;

/// Account for a finished update task: local stats, live session counter
/// and logging.
///
/// `in_flight` maps running tasks to the number of package updates they
/// attempt, so a panicked group job is counted once per member.
async fn finish_task(
    db: &Database,
    session_id: &str,
    stats: &mut UpdaterStats,
    in_flight: &mut HashMap<tokio::task::Id, usize>,
    task_result: Result<(tokio::task::Id, TaskOutput), tokio::task::JoinError>,
) {
    let counters = match task_result {
        Ok((id, outputs)) => {
            in_flight.remove(&id);
            outputs
                .into_iter()
                .map(|(result, attr_path)| {
                    let counter = stats.count(&result);
                    super::updater::handle_result(result, &attr_path);
                    counter
                })
                .collect()
        },
        Err(e) => {
            tracing::warn!("Task panicked: {}", e);
            let attempts = in_flight.remove(&e.id()).unwrap_or(1);
            stats.count_panicked(attempts)
        },
    };
    for counter in counters {
        if let Err(e) = db.increment_session_counter(session_id, counter).await {
            tracing::warn!("Failed to update session counters: {}", e);
        }
    }
}

impl UpdaterServiceConfig {
    /// Run the updater service that processes package update requests.
    ///
    /// Session counters (`packages_attempted` and the outcome counters) are
    /// updated live as requests start and finish.
    pub async fn run_service(
        self,
        mut rx: mpsc::UnboundedReceiver<super::types::UpdateJob>,
        db: Database,
    ) -> UpdaterStats {
        use tokio::task::JoinSet;
        use tracing::warn;

        let UpdaterServiceConfig {
            session_id,
            eval_entry_point,
            pr_config,
            fork,
            run_passthru_tests,
            dry_run,
            concurrency,
            pr_enhancements,
            interactive,
            preserve_failures,
            commit_strategy,
            claude_fix,
            claude_fix_max_turns,
            claude_fix_timeout,
            #[cfg(test)]
            executor,
        } = self;

        // Build Claude fix config if enabled
        let claude_fix_config = if claude_fix {
            Some(Arc::new(super::claude_fix::ClaudeFixConfig {
                max_turns: claude_fix_max_turns,
                timeout_secs: claude_fix_timeout,
            }))
        } else {
            None
        };

        let concurrency = updater_concurrency(commit_strategy, concurrency);

        // Serializes git operations in branch mode. Updates are serialized
        // there too (see `updater_concurrency`); the mutex stays as a
        // second guard for the git commands themselves.
        let git_mutex = Arc::new(tokio::sync::Mutex::new(()));

        let mut join_set: JoinSet<TaskOutput> = JoinSet::new();
        let mut stats = UpdaterStats::default();
        let mut in_flight: HashMap<tokio::task::Id, usize> = HashMap::new();

        loop {
            tokio::select! {
                // Receive update requests from release checker
                update_req = rx.recv() => {
                    // `None`: channel closed - no more updates coming
                    let Some(job) = update_req else { break };

                    // Wait if we've reached the concurrency limit
                    while join_set.len() >= concurrency {
                        if let Some(task_result) = join_set.join_next_with_id().await {
                            finish_task(&db, &session_id, &mut stats, &mut in_flight, task_result)
                                .await;
                        }
                    }

                    for _ in 0..job.attempts() {
                        if let Err(e) = db
                            .increment_session_counter(&session_id, SessionCounter::Attempted)
                            .await
                        {
                            warn!("Failed to update session counters: {}", e);
                        }
                    }

                    // Clone data needed for the async task
                    let db_clone = db.clone();
                    let session_id_clone = session_id.clone();
                    let pr_enhancements_clone = pr_enhancements.clone();
                    let claude_fix_clone = claude_fix_config.clone();

                    // Use per-request overrides when present (multi-repo routing),
                    // falling back to the global config. A group is routed by
                    // its first member (all members share a repo).
                    let lead = match &job {
                        UpdateJob::Single(req) => req,
                        UpdateJob::Group(group) => group.lead(),
                    };
                    let effective_entry_point: Arc<str> = match lead.eval_entry_point.as_deref() {
                        Some(ep) => Arc::from(ep),
                        None => Arc::clone(&eval_entry_point),
                    };
                    let effective_pr_config = lead.pr_config.clone().or_else(|| pr_config.clone());
                    let effective_fork: Arc<str> = match lead.fork.as_deref() {
                        Some(f) => Arc::from(f),
                        None => Arc::clone(&fork),
                    };

                    // Spawn the update task
                    let is_branch_mode = commit_strategy == CommitStrategy::Branch;
                    let git_mutex_clone = Arc::clone(&git_mutex);
                    let attempts = job.attempts();
                    #[cfg(test)]
                    let executor = executor.clone();
                    let handle = join_set.spawn(async move {
                        // Dry-run reports every update without performing
                        // it, in branch and worktree mode alike.
                        if dry_run {
                            return dry_run_outputs(&job);
                        }
                        #[cfg(test)]
                        if let Some(executor) = executor {
                            return (executor.0)(job).await;
                        }
                        let req = match job {
                            UpdateJob::Group(group) => {
                                let opts = super::group_update::GroupUpdateOptions {
                                    db: &db_clone,
                                    session_id: &session_id_clone,
                                    eval_entry_point: &effective_entry_point,
                                    pr_config: effective_pr_config.as_ref(),
                                    fork: &effective_fork,
                                    run_passthru_tests,
                                    dry_run,
                                    pr_enhancements: &pr_enhancements_clone,
                                    preserve_failures,
                                    claude_fix: claude_fix_clone.as_deref(),
                                };
                                // Branch mode commits the group on the
                                // current branch (no PR).
                                return if is_branch_mode {
                                    super::group_update::perform_branch_group_update(
                                        &opts,
                                        &group,
                                        &git_mutex_clone,
                                    )
                                    .await
                                } else {
                                    super::group_update::perform_group_update(&opts, &group).await
                                };
                            },
                            UpdateJob::Single(req) => req,
                        };
                        let result = if is_branch_mode {
                            super::updater::perform_direct_update(
                                &db_clone,
                                &session_id_clone,
                                &effective_entry_point,
                                &req,
                                run_passthru_tests,
                                Arc::clone(&git_mutex_clone),
                                claude_fix_clone.as_deref(),
                            )
                            .await
                        } else {
                            super::updater::perform_update(
                                &db_clone,
                                &session_id_clone,
                                &effective_entry_point,
                                &req,
                                effective_pr_config.as_ref(),
                                &effective_fork,
                                run_passthru_tests,
                                dry_run,
                                &pr_enhancements_clone,
                                interactive,
                                preserve_failures,
                                claude_fix_clone.as_deref(),
                            )
                            .await
                        };
                        vec![(result, req.attr_path)]
                    });
                    in_flight.insert(handle.id(), attempts);
                },
                // Also drain completed tasks while waiting for new requests
                Some(task_result) = join_set.join_next_with_id(), if !join_set.is_empty() => {
                    finish_task(&db, &session_id, &mut stats, &mut in_flight, task_result).await;
                },
            }
        }

        // Wait for all remaining tasks to complete
        while let Some(task_result) = join_set.join_next_with_id().await {
            finish_task(&db, &session_id, &mut stats, &mut in_flight, task_result).await;
        }

        info!("Updater service complete");
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::UpdateResult;
    use super::*;

    #[test]
    fn branch_mode_runs_updates_one_at_a_time() {
        for requested in [0, 1, 2, 8] {
            assert_eq!(updater_concurrency(CommitStrategy::Branch, requested), 1);
        }
    }

    #[test]
    fn worktree_mode_keeps_requested_concurrency() {
        assert_eq!(updater_concurrency(CommitStrategy::Worktrees, 0), 1);
        assert_eq!(updater_concurrency(CommitStrategy::Worktrees, 1), 1);
        assert_eq!(updater_concurrency(CommitStrategy::Worktrees, 8), 8);
    }

    #[test]
    fn updater_stats_count_outcomes() {
        let mut stats = UpdaterStats::default();
        let results: Vec<anyhow::Result<UpdateResult>> = vec![
            Ok(UpdateResult::Updated {
                old_version: "1".into(),
                new_version: "2".into(),
            }),
            Ok(UpdateResult::DryRun {
                current_version: "1".into(),
                new_version: "2".into(),
            }),
            Ok(UpdateResult::Skipped("x".into())),
            Ok(UpdateResult::Failed("x".into())),
            Err(anyhow::anyhow!("boom")),
        ];
        let counters: Vec<SessionCounter> = results.iter().map(|r| stats.count(r)).collect();
        assert_eq!(
            counters,
            vec![
                SessionCounter::Succeeded,
                SessionCounter::Skipped,
                SessionCounter::Skipped,
                SessionCounter::Failed,
                SessionCounter::Failed,
            ]
        );
        assert_eq!(
            stats,
            UpdaterStats {
                updated: 1,
                dry_run: 1,
                failed: 2,
                skipped: 1,
            }
        );
        assert_eq!(stats.attempted(), 5);
    }

    #[test]
    fn panicked_group_counts_every_member() {
        let mut stats = UpdaterStats::default();
        assert_eq!(
            stats.count_panicked(3),
            vec![SessionCounter::Failed; 3],
            "one failed counter per attempted member"
        );
        assert_eq!(stats.failed, 3);
        assert_eq!(stats.attempted(), 3);
    }

    fn req(attr: &str) -> super::super::types::UpdateRequest {
        super::super::types::UpdateRequest {
            attr_path: attr.to_owned(),
            drv_path: None,
            current_version: "1".to_owned(),
            new_version: "2".to_owned(),
            repo_name: None,
            eval_entry_point: None,
            pr_config: None,
            fork: None,
        }
    }

    #[test]
    fn dry_run_reports_every_member() {
        let single = UpdateJob::Single(req("a"));
        let group = UpdateJob::Group(super::super::types::GroupRequest {
            root: "r".to_owned(),
            members: vec![req("a"), req("b")],
            verify: vec![],
        });
        for (job, attrs) in [(single, vec!["a"]), (group, vec!["a", "b"])] {
            let outputs = dry_run_outputs(&job);
            assert_eq!(
                outputs.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>(),
                attrs
            );
            assert!(
                outputs
                    .iter()
                    .all(|(r, _)| matches!(r, Ok(UpdateResult::DryRun { .. })))
            );
            // Counted like the session does: skipped, not succeeded.
            let mut stats = UpdaterStats::default();
            for (r, _) in &outputs {
                assert_eq!(stats.count(r), SessionCounter::Skipped);
            }
            assert_eq!(stats.updated, 0);
        }
    }

    async fn setup_db() -> Database {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        Database::from(pool)
    }

    /// `(status, packages_evaluated, packages_attempted, completed)` of a session.
    async fn session_row(db: &Database, id: &str) -> (String, usize, usize, bool) {
        let session = db.get_session(id).await.unwrap().unwrap();
        (
            session.status.as_str().to_owned(),
            session.packages_evaluated,
            session.packages_attempted,
            session.completed_at.is_some(),
        )
    }

    fn pending<T: Send + 'static>() -> tokio::task::JoinHandle<T> {
        tokio::spawn(std::future::pending())
    }

    #[tokio::test]
    async fn shutdown_cancels_run_session_and_aborts_services() {
        let db = setup_db().await;
        let session = db.create_session(SessionMode::Run, None).await.unwrap();
        db.increment_session_counter(&session, SessionCounter::Attempted)
            .await
            .unwrap();
        let updater = pending::<UpdaterStats>();
        let updater_abort = updater.abort_handle();

        let err = supervise_services(
            &db,
            &session,
            pending(),
            pending(),
            updater,
            std::future::ready("SIGTERM"),
        )
        .await
        .unwrap_err();

        assert!(format!("{err}").contains("interrupted by SIGTERM"), "{err}");
        assert!(updater_abort.is_finished(), "updater aborted");
        assert_eq!(
            session_row(&db, &session).await,
            ("cancelled".to_owned(), 0, 1, true),
            "cancelled, counters kept"
        );
    }

    #[tokio::test]
    async fn panicked_service_fails_run_session() {
        for checker_panics in [true, false] {
            let db = setup_db().await;
            let session = db.create_session(SessionMode::Run, None).await.unwrap();
            let checker = tokio::spawn(async move {
                assert!(!checker_panics, "checker panic");
                (5, 1, 0)
            });
            let updater = tokio::spawn(async move {
                assert!(checker_panics, "updater panic");
                UpdaterStats::default()
            });

            let err = supervise_services(
                &db,
                &session,
                checker,
                tokio::spawn(async {}),
                updater,
                std::future::pending(),
            )
            .await
            .unwrap_err();

            assert!(format!("{err}").contains("panicked"), "{err}");
            let evaluated = if checker_panics { 0 } else { 5 };
            assert_eq!(
                session_row(&db, &session).await,
                ("failed".to_owned(), evaluated, 0, true),
                "checker panics: {checker_panics}"
            );
        }
    }

    #[tokio::test]
    async fn finished_services_finalize_run_session() {
        let cases = [
            (UpdaterStats::default(), "completed"),
            (
                UpdaterStats {
                    failed: 1,
                    ..UpdaterStats::default()
                },
                "failed",
            ),
        ];
        for (stats, status) in cases {
            let db = setup_db().await;
            let session = db.create_session(SessionMode::Run, None).await.unwrap();

            let (counts, returned) = supervise_services(
                &db,
                &session,
                tokio::spawn(async { (7, 2, 0) }),
                tokio::spawn(async {}),
                tokio::spawn(async move { stats }),
                std::future::pending(),
            )
            .await
            .unwrap();

            assert_eq!((counts, returned), ((7, 2, 0), stats));
            assert_eq!(
                session_row(&db, &session).await,
                (status.to_owned(), 7, 0, true)
            );
        }
    }
}
