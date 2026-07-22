pub mod checks;
pub mod report;
pub mod types;

use std::path::PathBuf;

use anyhow::Context;
use tokio::process::Command;
use tracing::{debug, info};

pub use types::{AuditConfig, Severity};

/// Output format for the standalone audit command
pub enum OutputFormat {
    Human,
    Json,
    Markdown,
}

impl OutputFormat {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            "markdown" | "md" => Ok(Self::Markdown),
            _ => anyhow::bail!("Unknown output format: {s}. Expected: human, json, or markdown"),
        }
    }
}

/// Parse a severity string from CLI input
pub fn parse_severity(s: &str) -> anyhow::Result<Severity> {
    match s.to_lowercase().as_str() {
        "info" => Ok(Severity::Info),
        "warning" | "warn" => Ok(Severity::Warning),
        "error" => Ok(Severity::Error),
        _ => anyhow::bail!("Unknown severity: {s}. Expected: info, warning, or error"),
    }
}

/// Resolve a target (drv path, store path, or attr path) to output store paths.
///
/// Returns `(output_paths, attr_path_if_known)`.
async fn resolve_target(
    target: &str,
    file: &str,
) -> anyhow::Result<(Vec<(String, PathBuf)>, Option<String>)> {
    if target.ends_with(".drv") {
        // Target is a derivation path -- realise it and get outputs
        resolve_drv(target).await
    } else if target.starts_with("/nix/store/") {
        // Target is an existing store path -- use directly
        if !PathBuf::from(target).exists() {
            anyhow::bail!("Store path does not exist: {target}");
        }
        Ok((vec![("out".to_owned(), PathBuf::from(target))], None))
    } else {
        // Target is an attr path -- build it
        resolve_attr_path(target, file).await
    }
}

/// Resolve a .drv path to its realised outputs
async fn resolve_drv(drv_path: &str) -> anyhow::Result<(Vec<(String, PathBuf)>, Option<String>)> {
    info!("Realising derivation: {}", drv_path);

    // Realise the derivation (build it if needed)
    let realise_output = Command::new("nix-store")
        .arg("--realise")
        .arg(drv_path)
        .output()
        .await
        .context("Failed to run nix-store --realise")?;

    if !realise_output.status.success() {
        let stderr = String::from_utf8_lossy(&realise_output.stderr);
        anyhow::bail!("Failed to realise derivation: {stderr}");
    }

    // Get output names and paths using nix show-derivation
    let show_output = Command::new("nix")
        .args(["show-derivation", drv_path])
        .output()
        .await
        .context("Failed to run nix show-derivation")?;

    if !show_output.status.success() {
        // Fallback: use nix-store --query --outputs
        let query_output = Command::new("nix-store")
            .args(["--query", "--outputs", drv_path])
            .output()
            .await
            .context("Failed to query derivation outputs")?;

        let stdout = String::from_utf8_lossy(&query_output.stdout);
        let outputs: Vec<(String, PathBuf)> = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .enumerate()
            .map(|(i, path)| {
                let name = if i == 0 {
                    "out".to_owned()
                } else {
                    format!("out-{i}")
                };
                (name, PathBuf::from(path))
            })
            .collect();

        return Ok((outputs, None));
    }

    // Parse nix show-derivation JSON output
    let stdout = String::from_utf8_lossy(&show_output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).context("Failed to parse show-derivation JSON")?;

    let mut outputs = Vec::new();

    // The JSON has the drv path as key, with an "outputs" object inside
    if let Some(drv_info) = json.as_object().and_then(|m| m.values().next()) {
        if let Some(drv_outputs) = drv_info.get("outputs").and_then(|o| o.as_object()) {
            for (output_name, output_info) in drv_outputs {
                if let Some(path) = output_info.get("path").and_then(|p| p.as_str()) {
                    let store_path = PathBuf::from(path);
                    if store_path.exists() {
                        outputs.push((output_name.clone(), store_path));
                    } else {
                        debug!("Output {} path does not exist: {}", output_name, path);
                    }
                }
            }
        }
    }

    if outputs.is_empty() {
        anyhow::bail!("No realised outputs found for derivation {drv_path}");
    }

    Ok((outputs, None))
}

/// Resolve an attr path by building it and extracting output paths
async fn resolve_attr_path(
    attr_path: &str,
    file: &str,
) -> anyhow::Result<(Vec<(String, PathBuf)>, Option<String>)> {
    info!("Building {} from {}", attr_path, file);

    let outputs = crate::commands::update::build_and_get_outputs(file, attr_path, None).await?;

    // Clean up result symlinks
    crate::commands::update::cleanup_result_symlinks().ok();

    if outputs.is_empty() {
        anyhow::bail!("Build produced no outputs for {attr_path}");
    }

    Ok((outputs, Some(attr_path.to_owned())))
}

/// Execute the standalone audit command
#[allow(clippy::fn_params_excessive_bools)]
pub async fn execute(
    target: String,
    file: String,
    format: String,
    min_severity: String,
    skip_metadata: bool,
    skip_binary_checks: bool,
) -> anyhow::Result<()> {
    let output_format = OutputFormat::parse(&format)?;
    let severity = parse_severity(&min_severity)?;

    let config = AuditConfig {
        min_severity: severity,
        check_metadata: !skip_metadata,
        check_shared_libs: !skip_binary_checks,
        check_rpaths: !skip_binary_checks,
        ..AuditConfig::default()
    };

    // Resolve the target to store paths
    let (output_paths, attr_path) = resolve_target(&target, &file).await?;

    info!(
        "Auditing {} output(s) for {}",
        output_paths.len(),
        attr_path.as_deref().unwrap_or(&target)
    );

    // Run the audit
    let audit_report = checks::run_audit(
        &output_paths,
        // Only provide eval context if we know the attr path and have a file
        if attr_path.is_some() {
            Some(file.as_str())
        } else {
            None
        },
        attr_path.as_deref(),
        &config,
    )
    .await;

    // Format and print
    let output = match output_format {
        OutputFormat::Human => report::format_terminal(&audit_report),
        OutputFormat::Json => report::format_json(&audit_report),
        OutputFormat::Markdown => report::format_markdown(&audit_report),
    };

    print!("{output}");

    // Exit with code 1 if any errors were found
    if audit_report.has_errors() {
        std::process::exit(1);
    }

    Ok(())
}
