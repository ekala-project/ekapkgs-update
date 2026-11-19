use anyhow::Context;
use tracing::info;

use crate::commands::run::preservation::repo_from_label;
use crate::database::Database;

/// Whether `identifier` names a derivation (`/nix/store/…` or `….drv`)
/// rather than an attr path. Attr paths often contain `-` (`aws-c-common`).
fn is_drv_identifier(identifier: &str) -> bool {
    identifier.starts_with("/nix/store/") || identifier.ends_with(".drv")
}

pub async fn show_log(
    database_path: String,
    identifier: String,
    repo: Option<String>,
) -> anyhow::Result<()> {
    // Expand tilde in database path
    let expanded_db_path = shellexpand::tilde(&database_path).to_string();

    // Initialize database
    let db = Database::new(&expanded_db_path).await?;
    let repo_name = repo.as_deref().map(repo_from_label);

    if is_drv_identifier(&identifier) {
        return show_log_by_drv(&db, &identifier).await;
    }

    let logs = db
        .get_all_failed_logs_by_attr(repo_name, &identifier)
        .await?;
    if logs.is_empty() {
        // Not an attr path with logs: maybe a log key without `.drv`
        // (`<repo>:<attr>@<version>`).
        if let Some(log) = db.get_log_by_drv(&identifier).await? {
            print_log_entry(&log, true);
            return Ok(());
        }
        info!("No failed update logs found for {}", identifier);
        return Ok(());
    }
    show_logs_by_attr(&identifier, &logs);
    Ok(())
}

async fn show_log_by_drv(db: &Database, drv_identifier: &str) -> anyhow::Result<()> {
    let log = db
        .get_log_by_drv(drv_identifier)
        .await?
        .context("No log found for the specified drv path")?;

    print_log_entry(&log, true);
    Ok(())
}

fn show_logs_by_attr(attr_path: &str, logs: &[crate::database::UpdateLog]) {
    // Show the latest log in detail
    info!("Showing most recent failed update for: {}", attr_path);
    info!("");
    print_log_entry(&logs[0], true);

    // If there are multiple failed attempts, list them
    if logs.len() > 1 {
        info!("");
        info!("Previous failed attempts:");
        for (i, log) in logs.iter().skip(1).enumerate() {
            info!(
                "  {}. {} ({})",
                i + 2,
                extract_drv_name(&log.drv_path),
                log.timestamp_as_datetime().format("%Y-%m-%d %H:%M:%S")
            );
        }
        info!("");
        info!("Use 'ekapkgs-update log <drv-path>' to view details of a specific attempt");
    }
}

fn print_log_entry(log: &crate::database::UpdateLog, show_full_log: bool) {
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("Failed Update Log");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("");
    if !log.repo_name.is_empty() {
        info!("Repository:     {}", log.repo_name);
    }
    info!("Attribute Path: {}", log.attr_path);
    info!("Derivation:     {}", log.drv_path);
    info!(
        "Timestamp:      {}",
        log.timestamp_as_datetime().format("%Y-%m-%d %H:%M:%S %Z")
    );

    if let (Some(old), Some(new)) = (&log.old_version, &log.new_version) {
        info!("Version:        {} → {}", old, new);
    } else if let Some(version) = &log.old_version {
        info!("Version:        {}", version);
    }

    info!("Status:         {}", log.status);
    if let Some(phase) = &log.phase {
        info!("Failed Phase:   {}", phase);
    }
    if let Some(error_type) = &log.error_type {
        info!("Error Type:     {}", error_type);
    }
    if let Some(session) = &log.session_id {
        info!("Session:        {}", session);
    }
    info!("");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("Error Log");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("");

    if show_full_log {
        // Print the error log, preserving formatting
        for line in log.error_log.lines() {
            info!("{}", line);
        }
    } else {
        // Show truncated version (first 20 lines)
        let lines: Vec<&str> = log.error_log.lines().collect();
        let truncated = lines.len() > 20;

        for line in lines.iter().take(20) {
            info!("{}", line);
        }

        if truncated {
            info!("");
            info!("... ({} more lines)", lines.len() - 20);
            info!("Use full drv path to see complete log");
        }
    }

    info!("");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

fn extract_drv_name(drv_path: &str) -> &str {
    // Extract just the drv name from full path
    // E.g., "/nix/store/abc123-python-setuptools-1.2.3.drv" -> "abc123-python-setuptools-1.2.3.drv"
    drv_path.rsplit('/').next().unwrap_or(drv_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drv_identifiers_are_store_paths_or_drv_files() {
        for (identifier, expected) in [
            ("/nix/store/abc123-cmake-3.29.6.drv", true),
            ("abc123-cmake-3.29.6.drv", true),
            ("/nix/store/abc123-cmake-3.29.6", true),
            ("aws-c-common", false),
            ("python3Packages.setuptools-scm", false),
            ("hello", false),
            ("repo:hello@1.1", false),
        ] {
            assert_eq!(is_drv_identifier(identifier), expected, "{identifier}");
        }
    }

    #[test]
    fn test_extract_drv_name_full_store_path() {
        let input = "/nix/store/abc123-python-setuptools-1.2.3.drv";
        let expected = "abc123-python-setuptools-1.2.3.drv";
        assert_eq!(extract_drv_name(input), expected);
    }

    #[test]
    fn test_extract_drv_name_short_name() {
        let input = "abc123-python-setuptools-1.2.3.drv";
        let expected = "abc123-python-setuptools-1.2.3.drv";
        assert_eq!(extract_drv_name(input), expected);
    }

    #[test]
    fn test_extract_drv_name_multiple_slashes() {
        let input = "/nix/store/subdir/hash-name.drv";
        let expected = "hash-name.drv";
        assert_eq!(extract_drv_name(input), expected);
    }

    #[test]
    fn test_extract_drv_name_no_slash() {
        let input = "just-a-name.drv";
        let expected = "just-a-name.drv";
        assert_eq!(extract_drv_name(input), expected);
    }

    #[test]
    fn test_extract_drv_name_empty_string() {
        let input = "";
        let expected = "";
        assert_eq!(extract_drv_name(input), expected);
    }

    #[test]
    fn test_extract_drv_name_trailing_slash() {
        let input = "/nix/store/abc123-pkg.drv/";
        let expected = "";
        assert_eq!(extract_drv_name(input), expected);
    }

    #[test]
    fn test_extract_drv_name_real_world_example() {
        let input = "/nix/store/3fr8b3xlygv2a64ff7fq7564j4sxv4lc-cmake-3.29.6.drv";
        let expected = "3fr8b3xlygv2a64ff7fq7564j4sxv4lc-cmake-3.29.6.drv";
        assert_eq!(extract_drv_name(input), expected);
    }
}
