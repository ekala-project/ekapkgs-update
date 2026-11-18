//! `cve-scan`: scan tracked packages for known vulnerabilities (OSV.dev).

use crate::cve::scan::{ScanOptions, scan_due};
use crate::cve::store::high_severity_packages;
use crate::database::Database;

/// Packages listed after a scan.
const LISTED: usize = 25;

/// Scan the packages that are due (or all of them with `force`) and list
/// those with high or critical vulnerabilities.
pub async fn cve_scan(
    database_path: String,
    repo: Option<String>,
    attrs: Vec<String>,
    max_age_hours: u32,
    force: bool,
) -> anyhow::Result<()> {
    let expanded_path = shellexpand::tilde(&database_path).to_string();
    let db = Database::new(&expanded_path).await?;
    // The dashboard labels the '' repo "default".
    let repo = repo.map(|r| if r == "default" { String::new() } else { r });
    let options = ScanOptions {
        repo: repo.clone(),
        attrs,
        max_age: chrono::Duration::hours(i64::from(max_age_hours)),
        force,
    };
    let summary = scan_due(db.pool(), &options).await?;
    println!(
        "Scanned {} packages ({} failed): {} with known vulnerabilities, {} high or critical.",
        summary.scanned, summary.failed, summary.vulnerable, summary.high_or_critical
    );

    let packages = high_severity_packages(db.pool(), repo.as_deref()).await?;
    if packages.is_empty() {
        println!("No tracked package has a known high or critical vulnerability.");
        return Ok(());
    }
    println!(
        "\nPackages with high or critical vulnerabilities ({}):",
        packages.len()
    );
    println!(
        "{:<44} {:>8} {:>4} {:>4} {:>5}  fixed by update",
        "attribute", "version", "crit", "high", "score"
    );
    for p in packages.iter().take(LISTED) {
        let attr = if p.repo_name.is_empty() {
            p.attr_path.clone()
        } else {
            format!("{}:{}", p.repo_name, p.attr_path)
        };
        let fixed = match p.latest_upstream_version.as_deref() {
            Some(latest) if latest != p.current_version => {
                format!("{}/{} → {latest}", p.fixed_by_update, p.critical + p.high)
            },
            _ => "-".to_owned(),
        };
        println!(
            "{attr:<44} {:>8} {:>4} {:>4} {:>5}  {fixed}",
            p.current_version,
            p.critical,
            p.high,
            p.max_score
                .map_or_else(|| "-".to_owned(), |s| format!("{s:.1}")),
        );
    }
    if packages.len() > LISTED {
        println!(
            "… and {} more (see the dashboard's packages page).",
            packages.len() - LISTED
        );
    }
    Ok(())
}
