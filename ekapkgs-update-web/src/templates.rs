use askama_axum::Template;
use chrono::{DateTime, Utc};
use ekapkgs_update::database::{PhaseRecord, UpdateSession};

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub stats: DashboardStats,
    pub recent_sessions: Vec<UpdateSession>,
    pub active_session: Option<UpdateSession>,
}

pub struct DashboardStats {
    pub total_packages: usize,
    pub success_rate: String, // Pre-formatted for display
    pub active_updates: usize,
    pub total_sessions: usize,
}

#[derive(Template)]
#[template(path = "sessions.html")]
pub struct SessionsTemplate {
    pub sessions: Vec<UpdateSession>,
    pub filter_status: Option<String>,
}

#[derive(Template)]
#[template(path = "session_detail.html")]
pub struct SessionDetailTemplate {
    pub session: UpdateSession,
    pub success_phases: Vec<PhaseRecord>,
    pub failed_phases: Vec<PhaseRecord>,
}

#[derive(Template)]
#[template(path = "packages.html")]
pub struct PackagesTemplate {
    pub packages: Vec<PackageInfo>,
    pub search: Option<String>,
}

pub struct PackageInfo {
    pub attr_path: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub status: String,
    pub last_attempted: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
    pub pr_url: Option<String>,
}

#[derive(Template)]
#[template(path = "analytics.html")]
pub struct AnalyticsTemplate {
    pub error_distribution: Vec<ErrorTypeCount>,
    pub total_errors: usize,
    pub phase_stats: Vec<PhaseStats>,
    pub success_rate_trend: Vec<SuccessRateTrend>,
}

pub struct ErrorTypeCount {
    pub error_type: String,
    pub count: usize,
}

pub struct PhaseStats {
    pub phase: String,
    pub success_count: usize,
    pub failure_count: usize,
    pub avg_duration_ms: i64,
}

pub struct SuccessRateTrend {
    pub date: String,
    pub success_rate: f64,
    pub success_rate_display: String, // Pre-formatted for display
}
