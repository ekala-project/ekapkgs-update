//! PyPI (Python Package Index) API integration

use std::collections::HashMap;

use serde::Deserialize;
use tracing::debug;

/// PyPI release information from the API
#[derive(Debug, Deserialize)]
pub struct PypiResponse {
    #[allow(dead_code)]
    pub info: PypiInfo,
    pub releases: HashMap<String, Vec<PypiArtifact>>,
}

/// Package metadata from PyPI
#[derive(Debug, Deserialize)]
pub struct PypiInfo {
    #[allow(dead_code)]
    pub version: String,
}

/// Individual release artifact
#[derive(Debug, Deserialize)]
pub struct PypiArtifact {
    pub yanked: bool,
}

/// Fetch all releases from PyPI API
///
/// Retrieves all releases for a given Python package from PyPI.
/// The releases are returned as a HashMap where keys are version strings.
///
/// # Arguments
/// * `pname` - Python package name (e.g., "requests", "django")
///
/// # Returns
/// A PypiResponse containing all releases and package info
///
/// # Example
/// ```no_run
/// use ekapkgs_update::pypi::fetch_pypi_releases;
///
/// # async fn example() -> anyhow::Result<()> {
/// let response = fetch_pypi_releases("requests").await?;
/// println!("Latest version: {}", response.info.version);
/// # Ok(())
/// # }
/// ```
pub async fn fetch_pypi_releases(pname: &str) -> anyhow::Result<PypiResponse> {
    let url = format!("https://pypi.org/pypi/{}/json", pname);

    debug!("Fetching PyPI releases from {}", url);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "ekapkgs-update")
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("PyPI API request failed with status: {}", response.status());
    }

    let pypi_response: PypiResponse = response.json().await?;
    Ok(pypi_response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pypi_response_structure() {
        // This test just verifies that the structures are defined correctly
        // Actual API integration tests would require network access
        let _response: Option<PypiResponse> = None;
        assert!(true);
    }
}
