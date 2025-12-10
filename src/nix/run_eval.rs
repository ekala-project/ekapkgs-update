use futures::stream::Stream;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use tracing::warn;

use super::nix_eval_jobs::NixEvalItem;

pub fn run_nix_eval_jobs(file_path: String) -> impl Stream<Item = anyhow::Result<NixEvalItem>> {
    async_stream::stream! {
        let mut cmd = match Command::new("nix-eval-jobs")
            .arg("--show-input-drvs")
            .arg(&file_path)
            .stdout(std::process::Stdio::piped())
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
