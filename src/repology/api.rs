use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use tracing::debug;

use super::types::{RepologyInfo, RepologyPackage};

/// Rate limiter for Repology API (max 1 request per second)
struct RateLimiter {
    last_request: Option<Instant>,
    min_interval: Duration,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            last_request: None,
            min_interval: Duration::from_secs(1), // 1 request per second
        }
    }

    async fn wait_if_needed(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                let wait_time = self.min_interval - elapsed;
                debug!("Rate limiting: waiting {:?}", wait_time);
                tokio::time::sleep(wait_time).await;
            }
        }
        self.last_request = Some(Instant::now());
    }
}

/// Repology API client with rate limiting
#[derive(Clone)]
pub struct RepologyClient {
    client: reqwest::Client,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    user_agent: String,
}

impl RepologyClient {
    /// Create a new Repology API client
    ///
    /// The client automatically enforces Repology's 1 request/second rate limit.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            user_agent: "ekapkgs-update (https://github.com/your-org/ekapkgs-update)".to_string(),
        }
    }

    /// Fetch information about a project from Repology
    ///
    /// # Arguments
    /// * `project_name` - The project name in Repology (e.g., "firefox", "python")
    ///
    /// # Returns
    /// RepologyInfo containing all packages for this project across distributions
    ///
    /// # Errors
    /// Returns an error if the API request fails or returns invalid data
    ///
    /// # Rate Limiting
    /// This function automatically enforces Repology's 1 req/sec rate limit
    pub async fn get_project(&self, project_name: &str) -> Result<RepologyInfo> {
        // Enforce rate limit
        {
            let mut limiter = self.rate_limiter.lock().await;
            limiter.wait_if_needed().await;
        }

        let url = format!("https://repology.org/api/v1/project/{}", project_name);

        debug!("Fetching Repology data for project: {}", project_name);

        let response = self
            .client
            .get(&url)
            .header("User-Agent", &self.user_agent)
            .send()
            .await
            .with_context(|| format!("GET {}", url))?;

        if !response.status().is_success() {
            if response.status().as_u16() == 404 {
                // Project not found in Repology
                debug!("Project not found in Repology: {}", project_name);
                return Ok(RepologyInfo {
                    project_name: project_name.to_string(),
                    packages: Vec::new(),
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                });
            }
            anyhow::bail!(
                "Repology API request failed with status: {}",
                response.status()
            );
        }

        let packages: Vec<RepologyPackage> = response
            .json()
            .await
            .with_context(|| format!("decode JSON from {}", url))?;

        debug!(
            "Found {} packages for {} in Repology",
            packages.len(),
            project_name
        );

        Ok(RepologyInfo {
            project_name: project_name.to_string(),
            packages,
            fetched_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Try to normalize a package name for Repology lookup
    ///
    /// Repology uses different naming conventions than nixpkgs. This function
    /// attempts to convert nixpkgs package names to Repology project names.
    ///
    /// Common transformations:
    /// - Remove language prefixes: python3-foo -> foo, node-foo -> foo
    /// - Convert underscores to hyphens in some cases
    /// - Remove version suffixes: foo_1_2 -> foo
    pub fn normalize_package_name(nixpkgs_name: &str) -> Vec<String> {
        let mut candidates = vec![nixpkgs_name.to_string()];

        // Try without common prefixes
        for prefix in &["python3-", "python-", "node-", "ruby-", "perl-"] {
            if let Some(stripped) = nixpkgs_name.strip_prefix(prefix) {
                candidates.push(stripped.to_string());
            }
        }

        // Try with underscores -> hyphens
        if nixpkgs_name.contains('_') {
            candidates.push(nixpkgs_name.replace('_', "-"));
        }

        // Try with hyphens -> underscores
        if nixpkgs_name.contains('-') {
            candidates.push(nixpkgs_name.replace('-', "_"));
        }

        candidates
    }
}

impl Default for RepologyClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_package_name() {
        let candidates = RepologyClient::normalize_package_name("python3-requests");
        assert!(candidates.contains(&"python3-requests".to_string()));
        assert!(candidates.contains(&"requests".to_string()));

        let candidates = RepologyClient::normalize_package_name("foo_bar");
        assert!(candidates.contains(&"foo_bar".to_string()));
        assert!(candidates.contains(&"foo-bar".to_string()));
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let mut limiter = RateLimiter::new();

        let start = Instant::now();

        // First request should be immediate
        limiter.wait_if_needed().await;
        let first_elapsed = start.elapsed();
        assert!(first_elapsed < Duration::from_millis(100));

        // Second request should wait ~1 second
        limiter.wait_if_needed().await;
        let second_elapsed = start.elapsed();
        assert!(second_elapsed >= Duration::from_secs(1));
        assert!(second_elapsed < Duration::from_millis(1100));
    }

    // Integration test - requires network access
    #[tokio::test]
    #[ignore]
    async fn test_get_project_real_api() {
        let client = RepologyClient::new();

        // Test with a well-known package
        let result = client.get_project("firefox").await;
        assert!(result.is_ok());

        let info = result.unwrap();
        assert_eq!(info.project_name, "firefox");
        assert!(!info.packages.is_empty());

        // Should have packages from multiple repositories
        let repos: Vec<_> = info.packages.iter().map(|p| &p.repo).collect();
        assert!(repos.len() > 5);
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_project_not_found() {
        let client = RepologyClient::new();

        let result = client
            .get_project("this-package-definitely-does-not-exist-12345")
            .await;
        assert!(result.is_ok());

        let info = result.unwrap();
        assert!(info.packages.is_empty());
    }
}
