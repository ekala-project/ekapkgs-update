use axum::extract::State;
use axum::response::IntoResponse;
use sqlx;

use crate::state::AppState;
use crate::templates::{AnalyticsTemplate, ErrorTypeCount, PhaseStats, SuccessRateTrend};

pub async fn index(State(state): State<AppState>) -> impl IntoResponse {
    // Get error distribution
    let error_distribution = get_error_distribution(&state).await;
    let total_errors: usize = error_distribution.iter().map(|e| e.count).sum();

    // Get phase statistics
    let phase_stats = get_phase_stats(&state).await;

    // Get success rate trend
    let success_rate_trend = get_success_rate_trend(&state).await;

    AnalyticsTemplate {
        error_distribution,
        total_errors,
        phase_stats,
        success_rate_trend,
    }
}

async fn get_error_distribution(state: &AppState) -> Vec<ErrorTypeCount> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT error_type, COUNT(*) as count
         FROM update_phases
         WHERE error_type IS NOT NULL
         GROUP BY error_type
         ORDER BY count DESC
         LIMIT ?1",
    )
    .bind(super::ERROR_DISTRIBUTION_LIMIT)
    .fetch_all(state.db.pool())
    .await
    .inspect_err(|e| tracing::warn!("Failed to get error distribution: {e}"))
    .unwrap_or_default();

    rows.into_iter()
        .map(|(error_type, count)| ErrorTypeCount {
            error_type,
            count: count as usize,
        })
        .collect()
}

async fn get_phase_stats(state: &AppState) -> Vec<PhaseStats> {
    let rows: Vec<(String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT
             phase,
             SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) as success_count,
             SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) as failure_count,
             AVG(duration_ms) as avg_duration
         FROM update_phases
         WHERE completed_at IS NOT NULL
         GROUP BY phase
         ORDER BY phase",
    )
    .fetch_all(state.db.pool())
    .await
    .inspect_err(|e| tracing::warn!("Failed to get phase stats: {e}"))
    .unwrap_or_default();

    rows.into_iter()
        .map(
            |(phase, success_count, failure_count, avg_duration)| PhaseStats {
                phase,
                success_count: success_count as usize,
                failure_count: failure_count as usize,
                avg_duration_ms: avg_duration.unwrap_or(0.0) as i64,
            },
        )
        .collect()
}

async fn get_success_rate_trend(state: &AppState) -> Vec<SuccessRateTrend> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT
             DATE(started_at) as date,
             CAST(SUM(packages_succeeded) AS REAL) / NULLIF(SUM(packages_attempted), 0) * 100 as \
         success_rate
         FROM update_sessions
         WHERE status = 'completed'
         GROUP BY DATE(started_at)
         ORDER BY date DESC
         LIMIT ?1",
    )
    .bind(super::SUCCESS_RATE_TREND_DAYS)
    .fetch_all(state.db.pool())
    .await
    .inspect_err(|e| tracing::warn!("Failed to get success rate trend: {e}"))
    .unwrap_or_default();

    rows.into_iter()
        .map(|(date, success_rate)| SuccessRateTrend {
            date,
            success_rate,
            success_rate_display: format!("{:.1}", success_rate),
        })
        .collect()
}
