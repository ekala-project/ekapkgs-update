//! Streaming wrapper around `nix-eval-jobs`.
//!
//! Each invocation writes the full stderr to
//! `~/.cache/ekapkgs-update/logs/nix-eval-jobs.<entry-point>.stderr.log`
//! (truncated per run) and keeps the last few lines in memory so failures can
//! be reported in the regular log instead of disappearing into a file.

use std::collections::VecDeque;
use std::path::PathBuf;

use futures::stream::Stream;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use super::nix_eval_jobs::NixEvalItem;

/// Number of trailing stderr lines kept for error reporting.
const STDERR_TAIL_LINES: usize = 20;

/// Options for a `nix-eval-jobs` invocation.
#[derive(Debug, Clone, Default)]
pub struct EvalJobsOptions {
    /// `--workers` value. `None` uses the nix-eval-jobs default.
    pub max_workers: Option<usize>,
    /// `--select` function applied to the evaluation root before traversal,
    /// e.g. `x: x.buildable`. Dotted keys in the selected set are treated as
    /// attribute paths (see [`NixEvalItem::flatten_dotted_attrs`]).
    pub select: Option<String>,
    /// `--apply` function applied to every derivation. Its JSON result is
    /// returned in `NixEvalDrv::extra_value`.
    pub apply: Option<String>,
}

/// Path of the stderr log for evaluating `file_path`.
///
/// One file per entry point so concurrent evaluations of different entry
/// points (e.g. the `run` and `watch` services) don't clobber each other.
async fn get_stderr_log_path(file_path: &str) -> anyhow::Result<PathBuf> {
    let logs_dir = crate::paths::logs_dir()?;
    fs::create_dir_all(&logs_dir).await?;

    Ok(logs_dir
        .join(stderr_log_file_name(file_path))
        .into_std_path_buf())
}

/// Turn an entry point path into a stable, filesystem-safe log file name.
fn stderr_log_file_name(file_path: &str) -> String {
    const MAX_LEN: usize = 120;
    let sanitized: String = file_path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    // Keep the tail: the end of the path is the distinguishing part.
    let start = trimmed.len().saturating_sub(MAX_LEN);
    let tail = trimmed.get(start..).unwrap_or(trimmed);
    format!("nix-eval-jobs.{tail}.stderr.log")
}

pub fn run_nix_eval_jobs(file_path: String) -> impl Stream<Item = anyhow::Result<NixEvalItem>> {
    run_nix_eval_jobs_with_options(file_path, EvalJobsOptions::default())
}

/// Run `nix-eval-jobs` and stream its results.
///
/// The stream ends with an `Err` item if nix-eval-jobs exits unsuccessfully,
/// carrying the tail of its stderr. A successful run that produced no jobs at
/// all is logged as a warning, since that almost always means the entry point
/// returns nested attribute sets that need a `select`.
pub fn run_nix_eval_jobs_with_options(
    file_path: String,
    options: EvalJobsOptions,
) -> impl Stream<Item = anyhow::Result<NixEvalItem>> {
    async_stream::stream! {
        let log_path = match get_stderr_log_path(&file_path).await {
            Ok(path) => path,
            Err(e) => {
                yield Err(anyhow::anyhow!("Failed to get stderr log path: {e}"));
                return;
            }
        };

        // Truncate: the log only needs to describe the most recent run.
        let mut log_file = match fs::File::create(&log_path).await {
            Ok(file) => file,
            Err(e) => {
                yield Err(anyhow::anyhow!(
                    "Failed to open stderr log file {}: {e}",
                    log_path.display()
                ));
                return;
            }
        };

        debug!("nix-eval-jobs stderr logging to: {:?}", log_path);

        let mut nix_eval_cmd = Command::new("nix-eval-jobs");
        nix_eval_cmd.arg("--show-input-drvs");
        if let Some(workers) = options.max_workers {
            nix_eval_cmd.arg("--workers").arg(workers.to_string());
        }
        if let Some(select) = &options.select {
            nix_eval_cmd.arg("--select").arg(select);
        }
        if let Some(apply) = &options.apply {
            nix_eval_cmd.arg("--apply").arg(apply);
        }
        nix_eval_cmd
            .arg(&file_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = match nix_eval_cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                yield Err(anyhow::anyhow!("Failed to spawn nix-eval-jobs: {e}"));
                return;
            }
        };

        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            yield Err(anyhow::anyhow!("nix-eval-jobs stdout/stderr were not piped"));
            return;
        };

        // Drain stderr concurrently: write everything to the log file and keep
        // a short tail in memory for error messages.
        let stderr_task = tokio::spawn(async move {
            let mut tail: VecDeque<String> = VecDeque::with_capacity(STDERR_TAIL_LINES);
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if log_file.write_all(line.as_bytes()).await.is_ok() {
                    let _ = log_file.write_all(b"\n").await;
                }
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            let _ = log_file.flush().await;
            tail
        });

        let mut stdout_lines = BufReader::new(stdout).lines();
        let mut jobs = 0usize;

        while let Some(line) = stdout_lines.next_line().await.transpose() {
            let line = match line {
                Ok(line) => line,
                Err(e) => {
                    yield Err(anyhow::anyhow!("Error reading line: {e}"));
                    continue;
                }
            };

            match serde_json::from_str::<NixEvalItem>(&line) {
                Ok(mut item) => {
                    if options.select.is_some() {
                        item.flatten_dotted_attrs();
                    }
                    jobs += 1;
                    yield Ok(item);
                },
                Err(e) => {
                    warn!(
                        "Encountered error when deserializing nix-eval-jobs output: {:?}",
                        e
                    );
                    continue;
                }
            };
        }

        let status = child.wait().await;
        let tail = stderr_task.await.unwrap_or_default();
        let tail_text = tail.into_iter().collect::<Vec<_>>().join("\n");
        let select_desc = options
            .select
            .as_deref()
            .map(|s| format!(" (select: {s})"))
            .unwrap_or_default();

        match status {
            Ok(status) if status.success() => {
                if jobs == 0 {
                    warn!(
                        "nix-eval-jobs produced no jobs for {}{}. If this file returns nested \
                         attribute sets (e.g. `{{ buildable = ...; }}`), configure a select \
                         expression such as `x: x.buildable`. stderr log: {}{}",
                        file_path,
                        select_desc,
                        log_path.display(),
                        if tail_text.is_empty() {
                            String::new()
                        } else {
                            format!("\n{tail_text}")
                        }
                    );
                } else {
                    debug!("nix-eval-jobs produced {} jobs for {}", jobs, file_path);
                }
            },
            Ok(status) => {
                yield Err(anyhow::anyhow!(
                    "nix-eval-jobs exited with {status} evaluating {file_path}{select_desc} \
                     (stderr log: {}):\n{tail_text}",
                    log_path.display()
                ));
            },
            Err(e) => {
                yield Err(anyhow::anyhow!("Failed to wait for nix-eval-jobs: {e}"));
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stderr_log_file_name() {
        assert_eq!(
            stderr_log_file_name("/var/lib/ekapkgs-update/repos/corepkgs/ci/packages.nix"),
            "nix-eval-jobs.var_lib_ekapkgs-update_repos_corepkgs_ci_packages.nix.stderr.log"
        );
        assert_eq!(
            stderr_log_file_name("default.nix"),
            "nix-eval-jobs.default.nix.stderr.log"
        );

        let long = format!("/{}/default.nix", "a".repeat(300));
        let name = stderr_log_file_name(&long);
        assert!(name.ends_with("_default.nix.stderr.log"));
        assert!(name.len() <= "nix-eval-jobs..stderr.log".len() + 120);
    }
}
