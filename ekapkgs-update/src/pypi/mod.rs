//! PyPI (Python Package Index) API integration

use std::collections::HashMap;

use anyhow::Context;
use serde::Deserialize;
use tracing::debug;

pub mod sdist;

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
    let url = format!("https://pypi.org/pypi/{pname}/json");

    debug!("Fetching PyPI releases from {}", url);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "ekapkgs-update")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("PyPI API request failed with status: {}", response.status());
    }

    let pypi_response: PypiResponse = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(pypi_response)
}

/// A file published for one PyPI release
#[derive(Debug, Deserialize)]
pub struct PypiFile {
    pub filename: String,
    /// `sdist` or `bdist_wheel` (older releases may have other `bdist_*` kinds)
    pub packagetype: String,
    /// `source` for sdists; for wheels the directory the file is served from
    /// (usually its python tag), i.e. `fetchPypi`'s `dist`
    #[serde(default)]
    pub python_version: String,
}

#[derive(Debug, Deserialize)]
struct PypiReleaseResponse {
    urls: Vec<PypiFile>,
}

/// Fetch the files published for one release of a PyPI project
///
/// Uses `https://pypi.org/pypi/{pname}/{version}/json`, whose `urls` field
/// lists that release's files.
pub async fn fetch_release_files(pname: &str, version: &str) -> anyhow::Result<Vec<PypiFile>> {
    let url = format!("https://pypi.org/pypi/{pname}/{version}/json");

    debug!("Fetching PyPI release files from {}", url);

    let response = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "ekapkgs-update")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("PyPI API request failed with status: {}", response.status());
    }

    let release: PypiReleaseResponse = response
        .json()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(release.urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pypi_response_structure() {
        // Compile-only smoke check: verifies the type can be named and is
        // constructible from `None`. Actual API integration tests would require
        // network access.
        let _response: Option<PypiResponse> = None;
    }
}
