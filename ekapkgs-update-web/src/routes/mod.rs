pub mod analytics;
pub mod dashboard;
pub mod packages;
pub mod sessions;
pub mod ws;

#[cfg(test)]
mod tests;

/// Number of recent sessions shown on the dashboard
pub const DASHBOARD_RECENT_LIMIT: usize = 10;
/// Number of recent sessions sampled for success rate calculation
pub const STATS_SESSIONS_SAMPLE: usize = 50;
/// Default session list page size
pub const SESSION_LIST_LIMIT: usize = 100;
/// Default package search result limit
pub const PACKAGE_SEARCH_LIMIT: i64 = 200;
/// Error type distribution limit for analytics
pub const ERROR_DISTRIBUTION_LIMIT: i64 = 10;
/// Number of days for success rate trend
pub const SUCCESS_RATE_TREND_DAYS: i64 = 30;
