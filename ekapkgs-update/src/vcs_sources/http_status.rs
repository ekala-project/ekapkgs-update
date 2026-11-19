//! Typed HTTP status failures from upstream release endpoints.
//!
//! Release fetchers return this (wrapped in `anyhow::Error`) for a non-success
//! response, so callers can tell a missing project (404/410) from a transient
//! failure without matching on message text.

use std::fmt;

use reqwest::StatusCode;

/// A release endpoint answered with a non-success HTTP status.
#[derive(Debug, Clone)]
pub struct HttpStatusError {
    /// What was requested, e.g. `"GitHub tags API request"`.
    pub request: &'static str,
    pub status: StatusCode,
}

impl HttpStatusError {
    pub fn new(request: &'static str, status: StatusCode) -> Self {
        Self { request, status }
    }

    /// The project does not exist at the requested location.
    pub fn is_not_found(&self) -> bool {
        matches!(self.status, StatusCode::NOT_FOUND | StatusCode::GONE)
    }

    /// The platform is throttling us (429, or 403, which GitHub answers when
    /// the API rate limit is exhausted).
    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self.status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::FORBIDDEN
        )
    }

    /// The first `HttpStatusError` in an error's chain.
    pub fn find(err: &anyhow::Error) -> Option<&Self> {
        err.chain().find_map(|e| e.downcast_ref::<Self>())
    }
}

impl fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed with status: {}", self.request, self.status)
    }
}

impl std::error::Error for HttpStatusError {}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn found_through_context_and_classified() {
        for (status, not_found) in [
            (StatusCode::NOT_FOUND, true),
            (StatusCode::GONE, true),
            (StatusCode::FORBIDDEN, false),
            (StatusCode::TOO_MANY_REQUESTS, false),
            (StatusCode::SERVICE_UNAVAILABLE, false),
        ] {
            let err: anyhow::Result<()> =
                Err(HttpStatusError::new("GitHub tags API request", status).into());
            let err = err.context("API poll failed for github/o/r").unwrap_err();
            let found = HttpStatusError::find(&err).expect("status error in chain");
            assert_eq!(found.is_not_found(), not_found, "{status}");
            assert_eq!(
                found.is_rate_limited(),
                matches!(
                    status,
                    StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
                ),
                "{status}"
            );
        }
        assert!(HttpStatusError::find(&anyhow::anyhow!("timeout")).is_none());
        assert_eq!(
            HttpStatusError::new("PyPI API request", StatusCode::NOT_FOUND).to_string(),
            "PyPI API request failed with status: 404 Not Found"
        );
    }
}
