use chrono::{DateTime, Utc};

/// Represents a package update record in the database
#[derive(Debug, Clone)]
pub struct UpdateRecord {
    pub _attr_path: String,
    pub last_attempted: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
    pub _current_version: Option<String>,
    pub proposed_version: Option<String>,
    pub _latest_upstream_version: Option<String>,
}

/// Represents a failed update log entry in the database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdateLog {
    pub drv_path: String,
    pub attr_path: String,
    pub timestamp: String,
    pub status: String,
    pub error_log: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
}

impl UpdateLog {
    /// Parse the timestamp string as a DateTime<Utc>
    pub fn timestamp_as_datetime(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    }
}

/// Database statistics
#[derive(Debug)]
pub struct _DatabaseStatistics {
    pub total_packages: i64,
    pub packages_with_proposed_updates: i64,
    pub packages_in_backoff: i64,
}
