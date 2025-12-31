use std::path::PathBuf;

use futures::stream::Stream;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use super::nix_eval_jobs::NixEvalItem;

/// Get the path to the nix-eval-jobs stderr log file in XDG cache directory
async fn get_stderr_log_path() -> anyhow::Result<PathBuf> {
    let cache_dir = directories::ProjectDirs::from("", "", "ekapkgs-update")
        .ok_or_else(|| anyhow::anyhow!("Failed to determine cache directory"))?
        .cache_dir()
        .to_path_buf();

    let logs_dir = cache_dir.join("logs");
    fs::create_dir_all(&logs_dir).await?;

    Ok(logs_dir.join("nix-eval-jobs.stderr.log"))
}

pub fn run_nix_eval_jobs(file_path: String) -> impl Stream<Item = anyhow::Result<NixEvalItem>> {
    async_stream::stream! {
        // Set up stderr logging to XDG cache directory
        let log_path = match get_stderr_log_path().await {
            Ok(path) => path,
            Err(e) => {
                yield Err(anyhow::anyhow!("Failed to get stderr log path: {}", e));
                return;
            }
        };

        let stderr_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(e) => {
                yield Err(anyhow::anyhow!("Failed to open stderr log file: {}", e));
                return;
            }
        };

        debug!("nix-eval-jobs stderr logging to: {:?}", log_path);

        let mut cmd = match Command::new("nix-eval-jobs")
            .arg("--show-input-drvs")
            .arg(&file_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::from(stderr_file))
            .spawn()
        {
            Ok(cmd) => cmd,
            Err(e) => {
                yield Err(anyhow::anyhow!("Failed to spawn nix-eval-jobs: {}", e));
                return;
            }
        };

        // TODO: handle failure case more nicely
        let stdout = cmd.stdout.take().unwrap();
        // Create a stream, so that we can pass through values as they are produced
        let stdout_reader = BufReader::new(stdout);
        let mut stdout_lines = stdout_reader.lines();

        while let Some(line) = stdout_lines.next_line().await.transpose() {
            let line = match line {
                Ok(line) => line,
                Err(e) => {
                    yield Err(anyhow::anyhow!("Error reading line: {}", e));
                    continue;
                }
            };

            match serde_json::from_str::<NixEvalItem>(&line) {
                Ok(item) => {
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
    }
}
