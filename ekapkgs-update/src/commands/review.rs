//! PR review command: checkout, merge base, build changed packages, optionally
//! run tests and a best-practices agent, then merge if everything passes.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::commands::update::build_nix_expr;
use crate::git::{cleanup_worktree, create_worktree};
use crate::github;
use crate::nix::has_passthru_tests;

/// Parsed repository coordinates.
#[derive(Debug, Clone)]
pub struct RepoCoords {
    pub owner: String,
    pub repo: String,
}

/// Configuration for the review command.
#[derive(Debug)]
pub struct ReviewConfig {
    pub repo: RepoCoords,
    pub pr_number: u64,
    pub file: String,
    pub run_tests: bool,
    pub best_practices: bool,
    pub best_practices_agent: String,
    pub best_practices_max_turns: u32,
    pub best_practices_timeout: u64,
    pub merge_method: String,
}

/// Parse a `--repo` value into owner/repo.
///
/// Accepts:
///   - `github.com/owner/repo`
///   - `https://github.com/owner/repo`
///   - `owner/repo`  (assumes github.com)
pub fn parse_repo_arg(input: &str) -> anyhow::Result<RepoCoords> {
    // Try full URL first
    if let Some(gh) = github::parse_github_url(input) {
        return Ok(RepoCoords {
            owner: gh.owner,
            repo: gh.repo,
        });
    }

    // Try owner/repo shorthand
    let parts: Vec<&str> = input.splitn(2, '/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        return Ok(RepoCoords {
            owner: parts[0].to_owned(),
            repo: parts[1].to_owned(),
        });
    }

    anyhow::bail!(
        "Invalid --repo value '{input}': expected 'owner/repo' or 'github.com/owner/repo'"
    )
}

/// Determine the nix attr paths affected by a PR based on the changed `.nix` files.
///
/// Heuristic: for each changed `.nix` file, derive a candidate attr path from
/// the directory structure. For example `pkgs/by-name/he/hello/package.nix`
/// yields `hello`, and `pkgs/tools/misc/foo/default.nix` yields `foo`.
///
/// We then verify each candidate actually evaluates by running `nix-instantiate --eval`.
fn attr_paths_from_changed_files(
    changed_files: &[github::GithubPrFile],
    eval_entry_point: &str,
) -> Vec<String> {
    let mut attr_paths: Vec<String> = Vec::new();

    for file in changed_files {
        if !file.filename.ends_with(".nix") {
            continue;
        }

        let path = Path::new(&file.filename);
        let components: Vec<&str> = path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        // Try by-name convention: pkgs/by-name/<shard>/<pname>/package.nix
        if components.len() >= 4 && components[0] == "pkgs" && components[1] == "by-name" {
            let pname = components[3].to_owned();
            if !attr_paths.contains(&pname) {
                attr_paths.push(pname);
            }
            continue;
        }

        // Generic: take the parent directory name as attr candidate.
        // e.g. pkgs/tools/misc/hello/default.nix → hello
        if let Some(parent) = path.parent() {
            if let Some(dir_name) = parent.file_name().and_then(|n| n.to_str()) {
                // Avoid pushing "pkgs" or the root itself
                if dir_name != "pkgs"
                    && dir_name != "."
                    && !attr_paths.contains(&dir_name.to_owned())
                {
                    attr_paths.push(dir_name.to_owned());
                }
            }
        }
    }

    // Filter out entry point file itself (often "default.nix" at root)
    let ep_stem = Path::new(eval_entry_point)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    attr_paths.retain(|a| a != ep_stem);

    attr_paths
}

/// Validate that an attr path actually exists in the entry point.
async fn validate_attr_path(worktree_entry_point: &str, attr_path: &str) -> bool {
    let expr = format!(
        "let pkgs = import {} {{}}; in pkgs ? {}",
        worktree_entry_point, attr_path
    );

    match Command::new("nix-instantiate")
        .arg("--eval")
        .arg("--json")
        .arg("-E")
        .arg(&expr)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.trim() == "true"
        },
        _ => false,
    }
}

/// Execute the review command.
pub async fn execute(config: ReviewConfig) -> anyhow::Result<()> {
    let token = std::env::var("GITHUB_TOKEN")
        .context("GITHUB_TOKEN environment variable is required for the review command")?;

    let RepoCoords { owner, repo } = &config.repo;

    // 1. Fetch PR details
    info!("Fetching PR #{} from {}/{}", config.pr_number, owner, repo);
    let pr = github::get_pull_request(owner, repo, config.pr_number, Some(&token)).await?;

    if pr.state != "open" {
        anyhow::bail!("PR #{} is not open (state: {})", config.pr_number, pr.state);
    }

    info!(
        "PR #{}: \"{}\" ({} → {})",
        pr.number, pr.title, pr.head.ref_name, pr.base.ref_name
    );

    // 2. Fetch changed files to determine which packages are affected
    let changed_files =
        github::get_pull_request_files(owner, repo, config.pr_number, Some(&token)).await?;
    info!("PR changes {} file(s)", changed_files.len());

    // 3. Clone/prepare the repo in a worktree, checkout PR branch, merge base
    let repo_dir = std::env::current_dir().context("get cwd")?;
    let worktree_label = format!("review-pr-{}", config.pr_number);
    let worktree_path = create_worktree(&repo_dir, &worktree_label).await?;

    // Ensure cleanup on early return
    let cleanup_guard = WorktreeCleanupGuard {
        repo_dir: repo_dir.clone(),
        worktree_path: worktree_path.clone(),
    };

    let result = execute_in_worktree(&config, &token, &pr, &changed_files, &worktree_path).await;

    // Clean up worktree regardless of outcome
    drop(cleanup_guard);
    cleanup_worktree(&repo_dir, &worktree_path).await.ok();

    result
}

/// RAII guard that attempts worktree cleanup on drop (best-effort).
struct WorktreeCleanupGuard {
    repo_dir: PathBuf,
    worktree_path: PathBuf,
}

impl Drop for WorktreeCleanupGuard {
    fn drop(&mut self) {
        // Best-effort synchronous cleanup — the async cleanup in execute() is
        // the primary path, this is a safety net.
        let _ = std::process::Command::new("git")
            .args([
                "-C",
                &self.repo_dir.to_string_lossy(),
                "worktree",
                "remove",
                "--force",
                &self.worktree_path.to_string_lossy(),
            ])
            .output();
    }
}

/// Core review logic that runs inside the worktree.
async fn execute_in_worktree(
    config: &ReviewConfig,
    token: &str,
    pr: &github::GithubPullRequestDetail,
    changed_files: &[github::GithubPrFile],
    worktree_path: &Path,
) -> anyhow::Result<()> {
    let RepoCoords { owner, repo } = &config.repo;

    // Fetch the PR via GitHub's pull ref (works for forks too)
    let pr_ref = format!("refs/pull/{}/head", config.pr_number);
    info!(
        "Fetching PR #{} via {} (branch: '{}')",
        config.pr_number, pr_ref, pr.head.ref_name
    );
    run_git_checked(worktree_path, &["fetch", "origin", &pr_ref]).await?;

    // Delete stale local branch if it exists from a previous run
    let _ = run_git_checked(worktree_path, &["branch", "-D", &pr.head.ref_name]).await;

    // Checkout into a local branch named after the PR branch
    run_git_checked(
        worktree_path,
        &["checkout", "-b", &pr.head.ref_name, "FETCH_HEAD"],
    )
    .await?;

    // Merge the latest base branch (must succeed)
    info!(
        "Merging latest base branch '{}' into PR branch",
        pr.base.ref_name
    );
    run_git_checked(worktree_path, &["fetch", "origin", &pr.base.ref_name]).await?;

    let merge_output = Command::new("git")
        .args([
            "-C",
            &worktree_path.to_string_lossy(),
            "merge",
            &format!("origin/{}", pr.base.ref_name),
            "--no-edit",
        ])
        .output()
        .await
        .context("run git merge")?;

    if !merge_output.status.success() {
        let stderr = String::from_utf8_lossy(&merge_output.stderr);
        // Abort the failed merge so the worktree is clean for cleanup
        let _ = Command::new("git")
            .args(["-C", &worktree_path.to_string_lossy(), "merge", "--abort"])
            .output()
            .await;
        anyhow::bail!(
            "Failed to merge base branch '{}' into PR branch — merge conflicts must be \
             resolved before review.\n{stderr}",
            pr.base.ref_name
        );
    }
    info!("Base branch merged successfully");

    // Determine the eval entry point inside the worktree
    let worktree_entry_point = worktree_path.join(&config.file);
    let worktree_entry_str = worktree_entry_point.to_string_lossy().to_string();

    if !worktree_entry_point.exists() {
        anyhow::bail!(
            "Eval entry point '{}' not found in worktree",
            worktree_entry_str
        );
    }

    // 4. Determine affected attr paths
    let candidate_attrs = attr_paths_from_changed_files(changed_files, &config.file);
    info!(
        "Candidate attr paths from changed files: {:?}",
        candidate_attrs
    );

    let mut attr_paths = Vec::new();
    for attr in &candidate_attrs {
        if validate_attr_path(&worktree_entry_str, attr).await {
            attr_paths.push(attr.clone());
        } else {
            debug!(
                "Attr path '{}' does not exist in entry point, skipping",
                attr
            );
        }
    }

    if attr_paths.is_empty() {
        anyhow::bail!(
            "Could not determine any valid package attr paths from the PR's changed files.\n\
             Changed files: {:?}\n\
             Candidates tried: {:?}",
            changed_files
                .iter()
                .map(|f| &f.filename)
                .collect::<Vec<_>>(),
            candidate_attrs,
        );
    }

    info!(
        "Will build {} package(s): {:?}",
        attr_paths.len(),
        attr_paths
    );

    // 5. Build each package
    let mut build_failures: Vec<(String, String)> = Vec::new();
    let mut test_failures: Vec<(String, String)> = Vec::new();
    let mut successful_builds: Vec<String> = Vec::new();

    for attr in &attr_paths {
        info!("Building {}", attr);
        match build_nix_expr(&worktree_entry_str, attr, None).await {
            Ok((true, _stdout, _stderr)) => {
                info!("  ✓ {} built successfully", attr);
                successful_builds.push(attr.clone());
            },
            Ok((false, _stdout, stderr)) => {
                warn!("  ✗ {} build failed", attr);
                build_failures.push((attr.clone(), stderr));
            },
            Err(e) => {
                warn!("  ✗ {} build error: {}", attr, e);
                build_failures.push((attr.clone(), format!("{e:#}")));
            },
        }
    }

    // 6. Run passthru.tests for successfully built packages (if --tests)
    if config.run_tests {
        for attr in &successful_builds {
            let has_tests = has_passthru_tests(&worktree_entry_str, attr)
                .await
                .unwrap_or(false);

            if !has_tests {
                info!("  - {}: no passthru.tests, skipping", attr);
                continue;
            }

            info!("  Testing {}.passthru.tests", attr);
            match build_nix_expr(&worktree_entry_str, attr, Some("passthru.tests")).await {
                Ok((true, _stdout, _stderr)) => {
                    info!("  ✓ {}: tests passed", attr);
                },
                Ok((false, _stdout, stderr)) => {
                    warn!("  ✗ {}: tests failed", attr);
                    test_failures.push((attr.clone(), stderr));
                },
                Err(e) => {
                    warn!("  ✗ {}: test error: {}", attr, e);
                    test_failures.push((attr.clone(), format!("{e:#}")));
                },
            }
        }
    }

    // 7. Run best-practices agent review (if --best-practices)
    let mut agent_findings: Vec<BestPracticesFinding> = Vec::new();
    if config.best_practices {
        info!(
            "Running best-practices review (agent: {})",
            config.best_practices_agent
        );

        let nix_files_in_pr: Vec<&str> = changed_files
            .iter()
            .filter(|f| f.filename.ends_with(".nix"))
            .map(|f| f.filename.as_str())
            .collect();

        match run_best_practices_review(
            worktree_path,
            &nix_files_in_pr,
            &attr_paths,
            &config.best_practices_agent,
            config.best_practices_max_turns,
            config.best_practices_timeout,
        )
        .await
        {
            Ok(BestPracticesResult::Pass) => {
                info!("  ✓ Best-practices review passed");
            },
            Ok(BestPracticesResult::Fail { findings }) => {
                warn!("  ✗ Best-practices review found issues");
                agent_findings = findings;
            },
            Ok(BestPracticesResult::AgentNotAvailable) => {
                anyhow::bail!(
                    "Best-practices agent '{}' is not available on PATH",
                    config.best_practices_agent
                );
            },
            Err(e) => {
                warn!("  ✗ Best-practices review error: {}", e);
                agent_findings.push(BestPracticesFinding {
                    message: format!("Agent error: {e:#}"),
                    file: None,
                    line: None,
                });
            },
        }
    }

    // 8. Report results
    println!();
    println!("═══ Review Results for PR #{} ═══", pr.number);
    println!();

    // Build results
    for attr in &successful_builds {
        println!("  ✓ {attr}: build passed");
    }
    for (attr, err) in &build_failures {
        println!("  ✗ {attr}: build failed");
        let tail: String = err
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        println!("    {tail}");
    }

    // Test results
    if config.run_tests {
        if test_failures.is_empty() && !successful_builds.is_empty() {
            println!("  ✓ All passthru.tests passed");
        }
        for (attr, _) in &test_failures {
            println!("  ✗ {attr}: passthru.tests failed");
        }
    }

    // Agent review results
    if config.best_practices {
        if agent_findings.is_empty() {
            println!("  ✓ Best-practices review passed");
        } else {
            println!("  ✗ Best-practices review found issues:");
            for finding in &agent_findings {
                match (&finding.file, finding.line) {
                    (Some(f), Some(l)) => println!("    - {f}:{l}: {}", finding.message),
                    (Some(f), None) => println!("    - {f}: {}", finding.message),
                    _ => println!("    - {}", finding.message),
                }
            }
        }
    }

    println!();

    // 9. Post review and merge (or request changes)
    if !build_failures.is_empty() {
        // Build failures are posted immediately as REQUEST_CHANGES — no prompt
        let review_body = format_build_failure_review(&build_failures);

        info!("Posting REQUEST_CHANGES review on PR #{}", pr.number);
        github::create_pull_request_review(
            owner,
            repo,
            config.pr_number,
            &review_body,
            "REQUEST_CHANGES",
            token,
        )
        .await?;

        anyhow::bail!("Review failed: {} build failure(s)", build_failures.len(),);
    }

    if !agent_findings.is_empty() {
        // Best-practices failures are posted immediately with inline comments
        let review_body = format_best_practices_review(&agent_findings);
        let inline_comments = findings_to_review_comments(&agent_findings);

        info!(
            "Posting REQUEST_CHANGES review on PR #{} ({} inline comment(s))",
            pr.number,
            inline_comments.len()
        );
        github::create_pull_request_review_with_comments(
            owner,
            repo,
            config.pr_number,
            &review_body,
            "REQUEST_CHANGES",
            &inline_comments,
            token,
        )
        .await?;

        anyhow::bail!(
            "Review failed: {} best-practices issue(s)",
            agent_findings.len(),
        );
    }

    if !test_failures.is_empty() {
        let review_body = format_failure_review(&test_failures);

        // Prompt user: post as-is, edit, or skip
        let final_body = prompt_failure_review(&review_body)?;

        if let Some(body) = final_body {
            info!("Posting REQUEST_CHANGES review on PR #{}", pr.number);
            github::create_pull_request_review(
                owner,
                repo,
                config.pr_number,
                &body,
                "REQUEST_CHANGES",
                token,
            )
            .await?;
        } else {
            info!("Skipping review post");
        }

        anyhow::bail!("Review failed: {} test failure(s)", test_failures.len(),);
    }

    // All checks passed — approve and merge
    let approve_body = format!(
        "All checks passed.\n\n\
         - {} package(s) built successfully: {}\n\
         {}\
         {}\
         \n🤖 Reviewed by ekapkgs-update",
        successful_builds.len(),
        successful_builds.join(", "),
        if config.run_tests {
            "- passthru.tests: passed\n"
        } else {
            ""
        },
        if config.best_practices {
            "- best-practices review: passed\n"
        } else {
            ""
        },
    );

    info!("Posting APPROVE review on PR #{}", pr.number);
    github::create_pull_request_review(
        owner,
        repo,
        config.pr_number,
        &approve_body,
        "APPROVE",
        token,
    )
    .await?;

    if pr.draft {
        println!(
            "✓ PR #{} review passed (draft PR — skipping merge)",
            pr.number
        );
    } else {
        info!(
            "Merging PR #{} (method: {})",
            pr.number, config.merge_method
        );
        github::merge_pull_request(owner, repo, config.pr_number, &config.merge_method, token)
            .await?;
        println!("✓ PR #{} merged successfully", pr.number);
    }

    Ok(())
}

/// Build a review body for build failures (last 20 lines per package).
fn format_build_failure_review(build_failures: &[(String, String)]) -> String {
    let mut body = String::from("## ekapkgs-update review: build failed\n\n");

    for (attr, stderr) in build_failures {
        body.push_str(&format!("**{attr}**:\n```\n"));
        let tail: String = last_n_lines(stderr, 20);
        body.push_str(&tail);
        body.push_str("\n```\n\n");
    }

    body.push_str("---\n🤖 Reviewed by ekapkgs-update\n");
    body
}

/// Build a review body for test failures.
fn format_failure_review(test_failures: &[(String, String)]) -> String {
    let mut body = String::from("## ekapkgs-update review: test failures\n\n");

    for (attr, stderr) in test_failures {
        body.push_str(&format!("**{attr}**:\n```\n"));
        body.push_str(&last_n_lines(stderr, 20));
        body.push_str("\n```\n\n");
    }

    body.push_str("---\n🤖 Reviewed by ekapkgs-update\n");
    body
}

/// Build a review body summarizing best-practices findings.
fn format_best_practices_review(findings: &[BestPracticesFinding]) -> String {
    let mut body = String::from("## ekapkgs-update review: best-practices issues\n\n");

    for finding in findings {
        match (&finding.file, finding.line) {
            (Some(f), Some(l)) => body.push_str(&format!("- `{f}:{l}`: {}\n", finding.message)),
            (Some(f), None) => body.push_str(&format!("- `{f}`: {}\n", finding.message)),
            _ => body.push_str(&format!("- {}\n", finding.message)),
        }
    }

    body.push_str("\n---\n🤖 Reviewed by ekapkgs-update\n");
    body
}

/// Convert best-practices findings into GitHub inline review comments.
///
/// Only findings that have both a file path and line number become inline
/// comments. The rest are captured in the review body only.
fn findings_to_review_comments(findings: &[BestPracticesFinding]) -> Vec<github::ReviewComment> {
    findings
        .iter()
        .filter_map(|f| {
            Some(github::ReviewComment {
                path: f.file.clone()?,
                line: f.line?,
                body: f.message.clone(),
            })
        })
        .collect()
}

/// Prompt the user to post, edit, or skip the failure review.
///
/// Returns `Some(body)` if the review should be posted (possibly edited),
/// or `None` if the user chose to skip.
fn prompt_failure_review(review_body: &str) -> anyhow::Result<Option<String>> {
    use std::io::{self, Write};

    println!("--- Proposed PR review ---");
    println!("{review_body}");
    println!("--------------------------");
    println!();
    println!("  [p]ost   - Post this review as REQUEST_CHANGES");
    println!("  [e]dit   - Edit the review message before posting");
    println!("  [s]kip   - Skip posting, just exit with failure");
    println!();
    print!("Action [p/e/s]: ");
    io::stdout().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    match response.trim().to_lowercase().as_str() {
        "p" | "post" => Ok(Some(review_body.to_owned())),
        "e" | "edit" => {
            let edited = open_in_editor(review_body)?;
            if edited.trim().is_empty() {
                println!("Empty message — skipping review post.");
                Ok(None)
            } else {
                Ok(Some(edited))
            }
        },
        "s" | "skip" | "" => Ok(None),
        other => {
            println!("Unknown option '{other}', skipping review post.");
            Ok(None)
        },
    }
}

/// Open `content` in the user's `$EDITOR` (falling back to `vi`) and return
/// the edited text.
fn open_in_editor(content: &str) -> anyhow::Result<String> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_owned());

    // Write content to a temporary file
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("ekapkgs-update-review.md");
    std::fs::write(&tmp_path, content)
        .with_context(|| format!("write temp file {}", tmp_path.display()))?;

    // Open editor (blocking — this is intentionally synchronous)
    let status = std::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .with_context(|| format!("launch editor '{editor}'"))?;

    if !status.success() {
        anyhow::bail!("Editor exited with status {status}");
    }

    let edited = std::fs::read_to_string(&tmp_path)
        .with_context(|| format!("read edited file {}", tmp_path.display()))?;

    // Clean up
    let _ = std::fs::remove_file(&tmp_path);

    Ok(edited)
}

// ── Best-practices agent ────────────────────────────────────────────────

/// A single best-practices finding, optionally tied to a file and line.
#[derive(Debug, Clone)]
struct BestPracticesFinding {
    /// The issue description.
    message: String,
    /// File path relative to the repo root (if the agent provided one).
    file: Option<String>,
    /// Line number in the file (if the agent provided one).
    line: Option<u64>,
}

enum BestPracticesResult {
    Pass,
    Fail { findings: Vec<BestPracticesFinding> },
    AgentNotAvailable,
}

/// Invoke an agent CLI (default: `claude`) to perform a best-practices review
/// of the Nix files touched in the PR.
async fn run_best_practices_review(
    worktree_path: &Path,
    nix_files: &[&str],
    attr_paths: &[String],
    agent_cmd: &str,
    max_turns: u32,
    timeout_secs: u64,
) -> anyhow::Result<BestPracticesResult> {
    // Check agent availability
    match Command::new(agent_cmd).arg("--version").output().await {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            debug!("Best-practices agent found: {}", version.trim());
        },
        _ => return Ok(BestPracticesResult::AgentNotAvailable),
    }

    let prompt = build_best_practices_prompt(worktree_path, nix_files, attr_paths).await;

    let max_turns_str = max_turns.to_string();
    let mut child = Command::new(agent_cmd)
        .args([
            "--print",
            "--dangerously-skip-permissions",
            "--max-turns",
            &max_turns_str,
            "--allowedTools",
            "Read,Grep,Glob,Bash",
        ])
        .current_dir(worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn best-practices agent")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .context("write prompt to agent stdin")?;
        drop(stdin);
    }

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => anyhow::bail!("agent process error: {e}"),
        Err(_) => anyhow::bail!("best-practices agent timed out after {timeout_secs}s"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    // Parse the agent's output for PASS/FAIL verdict.
    // Convention: the agent should end with a line starting with "VERDICT: PASS"
    // or "VERDICT: FAIL" followed by bullet-point suggestions.
    Ok(parse_best_practices_output(&stdout))
}

/// Build the prompt for the best-practices review agent.
async fn build_best_practices_prompt(
    worktree_path: &Path,
    nix_files: &[&str],
    attr_paths: &[String],
) -> String {
    let mut prompt = String::with_capacity(8_000);
    prompt.push_str("# Task: Nix Package Best-Practices Review\n\n");
    prompt.push_str(
        "You are reviewing Nix package files for best-practices compliance. \
         Check each file for common issues and output a verdict.\n\n",
    );

    // Include project-specific best-practices skill if it exists
    let skill_path = worktree_path.join(".agents/skills/best-practices/SKILL.md");
    if let Ok(skill_content) = tokio::fs::read_to_string(&skill_path).await {
        prompt.push_str("## Project best-practices guidelines\n\n");
        prompt.push_str(
            "The following project-specific guidelines take precedence over \
             the general checks below.\n\n",
        );
        prompt.push_str(&skill_content);
        prompt.push_str("\n\n");
    }

    prompt.push_str("## Packages under review\n\n");
    for attr in attr_paths {
        prompt.push_str(&format!("- `{attr}`\n"));
    }
    prompt.push('\n');

    prompt.push_str("## Changed Nix files\n\n");
    for file in nix_files {
        prompt.push_str(&format!("- `{file}`\n"));

        // Include file content so the agent has full context
        let full_path = worktree_path.join(file);
        if let Ok(content) = tokio::fs::read_to_string(&full_path).await {
            prompt.push_str(&format!("\n```nix\n// {file}\n{content}\n```\n\n"));
        }
    }

    prompt.push_str("## Checks to perform\n\n");
    prompt.push_str(
        "1. **Correct use of build inputs**: Are dependencies in the right list \
                      (`nativeBuildInputs` for build-time, `buildInputs` for runtime)?\n",
    );
    prompt.push_str(
        "2. **License and metadata**: Is `meta.license`, `meta.description`, \
                      and `meta.homepage` set?\n",
    );
    prompt.push_str(
        "3. **No maintainers/teams**: `meta.maintainers` and `meta.teams` must NOT be set. \
                      Flag any package that includes either attribute.\n",
    );
    prompt.push_str("4. **Hash format**: Are hashes in SRI format (`sha256-...`)?\n");
    prompt.push_str(
        "5. **Deprecated patterns**: Does it use deprecated Nix patterns \
                      (e.g., `stdenv.lib` instead of `lib`, `with pkgs;` at top level)?\n",
    );
    prompt
        .push_str("6. **Patch hygiene**: Are patches well-documented with upstream references?\n");
    prompt.push_str(
        "7. **Phase overrides**: Are `postPatch`, `postInstall`, etc. clean and \
                      necessary?\n",
    );
    prompt.push_str(
        "8. **Version string consistency**: Does the version in the expression \
                      match the source URL/tag?\n\n",
    );

    prompt.push_str("## Output format\n\n");
    prompt.push_str(
        "After your analysis, you MUST end your response with exactly one of:\n\n\
         ```\n\
         VERDICT: PASS\n\
         ```\n\n\
         or:\n\n\
         ```\n\
         VERDICT: FAIL\n\
         - file.nix:42: Issue description for a specific line\n\
         - file.nix: Issue description for the whole file\n\
         - Issue description without a file reference\n\
         ```\n\n\
         Each issue on its own line, prefixed with `- `.\n\
         When possible, include `filename:line:` at the start of each issue \
         so the feedback can be attached as an inline comment on the PR.\n\
         Only fail for genuine problems, not stylistic preferences.\n",
    );

    prompt
}

/// Parse the agent's stdout for the VERDICT line.
fn parse_best_practices_output(output: &str) -> BestPracticesResult {
    // Search for the VERDICT line from the end
    let lines: Vec<&str> = output.lines().collect();

    let mut verdict_idx = None;
    for (i, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with("VERDICT:") {
            verdict_idx = Some(i);
            break;
        }
    }

    let Some(idx) = verdict_idx else {
        return BestPracticesResult::Fail {
            findings: vec![BestPracticesFinding {
                message: format!(
                    "Agent did not produce a VERDICT line. Raw output (last 500 chars): {}",
                    &output[output.len().saturating_sub(500)..]
                ),
                file: None,
                line: None,
            }],
        };
    };

    let verdict_line = lines[idx].trim();
    if verdict_line.contains("PASS") {
        return BestPracticesResult::Pass;
    }

    let mut findings: Vec<BestPracticesFinding> = Vec::new();

    // First check if there are items on the same line as VERDICT
    if let Some(after_fail) = verdict_line.strip_prefix("VERDICT: FAIL") {
        let trimmed = after_fail.trim();
        if !trimmed.is_empty() {
            findings.push(parse_finding_line(trimmed.trim_start_matches("- ")));
        }
    }
    // Then collect subsequent lines that start with "- "
    for line in &lines[idx + 1..] {
        let trimmed = line.trim();
        if let Some(item) = trimmed.strip_prefix("- ") {
            findings.push(parse_finding_line(item));
        } else if trimmed.starts_with("```") {
            break;
        } else if !trimmed.is_empty() {
            findings.push(parse_finding_line(trimmed));
        }
    }

    if findings.is_empty() {
        findings.push(BestPracticesFinding {
            message: "Best-practices review failed (no specific issues listed)".to_owned(),
            file: None,
            line: None,
        });
    }

    BestPracticesResult::Fail { findings }
}

/// Parse a finding line that may start with `file.nix:line:` or `file.nix:`.
///
/// Accepted formats:
///   - `pkgs/foo/default.nix:42: description` → file + line + message
///   - `pkgs/foo/default.nix: description`     → file + message (no line)
///   - `description`                           → message only
fn parse_finding_line(line: &str) -> BestPracticesFinding {
    // Try "file.nix:line: message"
    if let Some(colon1) = line.find(':') {
        let candidate_file = &line[..colon1];
        if candidate_file.ends_with(".nix") {
            let rest = &line[colon1 + 1..];
            // Try to parse a line number after the first colon
            if let Some(colon2) = rest.find(':') {
                let maybe_line = rest[..colon2].trim();
                if let Ok(line_num) = maybe_line.parse::<u64>() {
                    let message = rest[colon2 + 1..].trim().to_owned();
                    return BestPracticesFinding {
                        message,
                        file: Some(candidate_file.to_owned()),
                        line: Some(line_num),
                    };
                }
            }
            // No line number — just "file.nix: message"
            let message = rest.trim().to_owned();
            return BestPracticesFinding {
                message,
                file: Some(candidate_file.to_owned()),
                line: None,
            };
        }
    }

    BestPracticesFinding {
        message: line.to_owned(),
        file: None,
        line: None,
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Return the last `n` lines of `text`, preserving original order.
fn last_n_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

async fn run_git_checked(cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd.as_os_str())
        .args(args)
        .output()
        .await
        .with_context(|| format!("git {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repo_arg_owner_repo() {
        let coords = parse_repo_arg("ekala-project/ekapkgs").unwrap();
        assert_eq!(coords.owner, "ekala-project");
        assert_eq!(coords.repo, "ekapkgs");
    }

    #[test]
    fn test_parse_repo_arg_full_url() {
        let coords = parse_repo_arg("https://github.com/ekala-project/ekapkgs").unwrap();
        assert_eq!(coords.owner, "ekala-project");
        assert_eq!(coords.repo, "ekapkgs");
    }

    #[test]
    fn test_parse_repo_arg_github_url_no_scheme() {
        let coords = parse_repo_arg("github.com/ekala-project/ekapkgs").unwrap();
        assert_eq!(coords.owner, "ekala-project");
        assert_eq!(coords.repo, "ekapkgs");
    }

    #[test]
    fn test_parse_repo_arg_invalid() {
        assert!(parse_repo_arg("just-one-word").is_err());
        assert!(parse_repo_arg("").is_err());
    }

    #[test]
    fn test_attr_paths_from_changed_files_by_name() {
        let files = vec![github::GithubPrFile {
            filename: "pkgs/by-name/he/hello/package.nix".to_owned(),
            status: "modified".to_owned(),
        }];
        let result = attr_paths_from_changed_files(&files, "default.nix");
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_attr_paths_from_changed_files_generic() {
        let files = vec![github::GithubPrFile {
            filename: "pkgs/tools/misc/ripgrep/default.nix".to_owned(),
            status: "modified".to_owned(),
        }];
        let result = attr_paths_from_changed_files(&files, "default.nix");
        assert_eq!(result, vec!["ripgrep"]);
    }

    #[test]
    fn test_attr_paths_skips_non_nix() {
        let files = vec![github::GithubPrFile {
            filename: "README.md".to_owned(),
            status: "modified".to_owned(),
        }];
        let result = attr_paths_from_changed_files(&files, "default.nix");
        assert!(result.is_empty());
    }

    #[test]
    fn test_attr_paths_deduplicates() {
        let files = vec![
            github::GithubPrFile {
                filename: "pkgs/tools/misc/hello/default.nix".to_owned(),
                status: "modified".to_owned(),
            },
            github::GithubPrFile {
                filename: "pkgs/tools/misc/hello/update.nix".to_owned(),
                status: "modified".to_owned(),
            },
        ];
        let result = attr_paths_from_changed_files(&files, "default.nix");
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_parse_best_practices_pass() {
        let output = "Looks good!\n\nVERDICT: PASS\n";
        let result = parse_best_practices_output(output);
        assert!(matches!(result, BestPracticesResult::Pass));
    }

    #[test]
    fn test_parse_best_practices_fail() {
        let output = "Issues found.\n\nVERDICT: FAIL\n- Missing meta.license\n- Deprecated stdenv.lib usage\n";
        let result = parse_best_practices_output(output);
        match result {
            BestPracticesResult::Fail { findings } => {
                assert_eq!(findings.len(), 2);
                assert!(findings[0].message.contains("meta.license"));
                assert!(findings[1].message.contains("stdenv.lib"));
            },
            _ => panic!("Expected Fail"),
        }
    }

    #[test]
    fn test_parse_best_practices_fail_with_file_line() {
        let output = "VERDICT: FAIL\n- pkgs/foo/default.nix:42: Missing license\n- pkgs/bar/default.nix: No description\n- General issue\n";
        let result = parse_best_practices_output(output);
        match result {
            BestPracticesResult::Fail { findings } => {
                assert_eq!(findings.len(), 3);
                assert_eq!(findings[0].file.as_deref(), Some("pkgs/foo/default.nix"));
                assert_eq!(findings[0].line, Some(42));
                assert_eq!(findings[0].message, "Missing license");
                assert_eq!(findings[1].file.as_deref(), Some("pkgs/bar/default.nix"));
                assert_eq!(findings[1].line, None);
                assert_eq!(findings[1].message, "No description");
                assert!(findings[2].file.is_none());
                assert!(findings[2].line.is_none());
            },
            _ => panic!("Expected Fail"),
        }
    }

    #[test]
    fn test_parse_best_practices_no_verdict() {
        let output = "Some random agent output with no verdict line";
        let result = parse_best_practices_output(output);
        assert!(matches!(result, BestPracticesResult::Fail { .. }));
    }
}
