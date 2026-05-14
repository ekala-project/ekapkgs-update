use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use ekapkgs_update::database::SessionStatus;
use sqlx;

use crate::state::AppState;
use crate::templates::{DashboardStats, DashboardTemplate};

pub async fn index(State(state): State<AppState>) -> impl IntoResponse {
    // Get statistics
    let stats = get_dashboard_stats(&state).await;

    // Get recent sessions (last 10)
    let recent_sessions = state.db.get_recent_sessions(10).await.unwrap_or_default();

    // Get active session if any
    let active_session = state
        .db
        .get_sessions_by_status(SessionStatus::Running)
        .await
        .ok()
        .and_then(|sessions| sessions.into_iter().next());

    let template = DashboardTemplate {
        stats,
        recent_sessions,
        active_session,
    };

    template
}

pub async fn stats_json(State(state): State<AppState>) -> impl IntoResponse {
    let stats = get_dashboard_stats(&state).await;
    Json(serde_json::json!({
        "total_packages": stats.total_packages,
        "success_rate": stats.success_rate,
        "active_updates": stats.active_updates,
        "total_sessions": stats.total_sessions,
    }))
}

async fn get_dashboard_stats(state: &AppState) -> DashboardStats {
    // Get total number of packages from updates table
    let total_packages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM updates")
        .fetch_one(state.db.pool())
        .await
        .unwrap_or(0);

    // Get success rate from recent sessions
    let recent_sessions = state.db.get_recent_sessions(50).await.unwrap_or_default();

    let total_attempted: usize = recent_sessions.iter().map(|s| s.packages_attempted).sum();

    let total_succeeded: usize = recent_sessions.iter().map(|s| s.packages_succeeded).sum();

    let success_rate = if total_attempted > 0 {
        (total_succeeded as f64 / total_attempted as f64) * 100.0
    } else {
        0.0
    };

    // Get active updates
    let active_updates = state
        .db
        .get_sessions_by_status(SessionStatus::Running)
        .await
        .ok()
        .map(|sessions| {
            sessions
                .iter()
                .map(|s| {
                    s.packages_attempted
                        - s.packages_succeeded
                        - s.packages_failed
                        - s.packages_skipped
                })
                .sum()
        })
        .unwrap_or(0);

    // Get total sessions
    let total_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM update_sessions")
        .fetch_one(state.db.pool())
        .await
        .unwrap_or(0);

    DashboardStats {
        total_packages: total_packages as usize,
        success_rate,
        active_updates,
        total_sessions: total_sessions as usize,
    }
}
