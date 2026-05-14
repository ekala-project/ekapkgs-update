use anyhow::Context;
use tracing::info;

use crate::database::Database;

pub async fn show_log(database_path: String, identifier: String) -> anyhow::Result<()> {
    // Expand tilde in database path
    let expanded_db_path = shellexpand::tilde(&database_path).to_string();

    // Initialize database
    let db = Database::new(&expanded_db_path).await?;

    // Determine if identifier is a drv_path or attr_path
    let is_drv_path = identifier.starts_with("/nix/store/")
        || identifier.contains(".drv")
        || identifier.contains('-');

    if is_drv_path {
        // Query by drv_path
        show_log_by_drv(&db, &identifier).await
    } else {
        // Query by attr_path
        show_logs_by_attr(&db, &identifier).await
    }
}

async fn show_log_by_drv(db: &Database, drv_identifier: &str) -> anyhow::Result<()> {
    let log = db
        .get_log_by_drv(drv_identifier)
        .await?
        .context("No log found for the specified drv path")?;

    print_log_entry(&log, true);
    Ok(())
}

async fn show_logs_by_attr(db: &Database, attr_path: &str) -> anyhow::Result<()> {
    let logs = db.get_all_failed_logs_by_attr(attr_path).await?;

    if logs.is_empty() {
        info!("No failed update logs found for {}", attr_path);
        return Ok(());
    }

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

    Ok(())
}

fn print_log_entry(log: &crate::database::UpdateLog, show_full_log: bool) {
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("Failed Update Log");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("");
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
