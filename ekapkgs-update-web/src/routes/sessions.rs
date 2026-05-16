use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use ekapkgs_update::database::PhaseRecord;
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::state::AppState;
use crate::templates::{SessionDetailTemplate, SessionsTemplate};

#[derive(Deserialize)]
pub struct SessionsQuery {
    pub status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<SessionsQuery>,
) -> impl IntoResponse {
    let sessions = if let Some(status) = &query.status {
        // Filter by status
        match status.as_str() {
            "running" => state
                .db
                .get_sessions_by_status(ekapkgs_update::database::SessionStatus::Running)
                .await
                .unwrap_or_default(),
            "completed" => state
                .db
                .get_sessions_by_status(ekapkgs_update::database::SessionStatus::Completed)
                .await
                .unwrap_or_default(),
            "failed" => state
                .db
                .get_sessions_by_status(ekapkgs_update::database::SessionStatus::Failed)
                .await
                .unwrap_or_default(),
            "cancelled" => state
                .db
                .get_sessions_by_status(ekapkgs_update::database::SessionStatus::Cancelled)
                .await
                .unwrap_or_default(),
            _ => state.db.get_recent_sessions(100).await.unwrap_or_default(),
        }
    } else {
        state.db.get_recent_sessions(100).await.unwrap_or_default()
    };

    SessionsTemplate {
        sessions,
        filter_status: query.status,
    }
}

pub async fn list_json(State(state): State<AppState>) -> impl IntoResponse {
    let sessions = state.db.get_recent_sessions(100).await.unwrap_or_default();
    Json(sessions)
}

pub async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    // Parse UUID
    let session_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid.to_string(),
        Err(_) => return "Invalid session ID".into_response(),
    };

    // Get session
    let Some(session) = state.db.get_session(&session_id).await.ok().flatten() else {
        return "Session not found".into_response();
    };

    // Get all phases for this session
    let rows = sqlx::query(
        "SELECT id, session_id, attr_path, phase, started_at, completed_at,
                duration_ms, status, error_type, error_details, artifacts_path
         FROM update_phases
         WHERE session_id = ?
         ORDER BY started_at DESC",
    )
    .bind(&session_id)
    .fetch_all(state.db.pool())
    .await
    .unwrap_or_default();

    let phases: Vec<PhaseRecord> = rows
        .into_iter()
        .filter_map(|row| {
            Some(PhaseRecord {
                id: row.get("id"),
                session_id: row.get("session_id"),
                attr_path: row.get("attr_path"),
                phase: row.get("phase"),
                started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                    .ok()?
                    .with_timezone(&Utc),
                completed_at: row
                    .get::<Option<String>, _>("completed_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                duration_ms: row.get("duration_ms"),
                status: row.get("status"),
                error_type: row.get("error_type"),
                error_details: row.get("error_details"),
                artifacts_path: row.get("artifacts_path"),
            })
        })
        .collect();

    // Split into success and failed phases
    let success_phases: Vec<_> = phases
        .iter()
        .filter(|p| p.status == "success")
        .cloned()
        .collect();

    let failed_phases: Vec<_> = phases
        .iter()
        .filter(|p| p.status == "failed")
        .cloned()
        .collect();

    SessionDetailTemplate {
        session,
        success_phases,
        failed_phases,
    }
    .into_response()
}

pub async fn phases_json(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let rows = sqlx::query(
        "SELECT id, session_id, attr_path, phase, started_at, completed_at,
                duration_ms, status, error_type, error_details, artifacts_path
         FROM update_phases
         WHERE session_id = ?
         ORDER BY started_at DESC",
    )
    .bind(&id)
    .fetch_all(state.db.pool())
    .await
    .unwrap_or_default();

    let phases: Vec<PhaseRecord> = rows
        .into_iter()
        .filter_map(|row| {
            Some(PhaseRecord {
                id: row.get("id"),
                session_id: row.get("session_id"),
                attr_path: row.get("attr_path"),
                phase: row.get("phase"),
                started_at: DateTime::parse_from_rfc3339(row.get("started_at"))
                    .ok()?
                    .with_timezone(&Utc),
                completed_at: row
                    .get::<Option<String>, _>("completed_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                duration_ms: row.get("duration_ms"),
                status: row.get("status"),
                error_type: row.get("error_type"),
                error_details: row.get("error_details"),
                artifacts_path: row.get("artifacts_path"),
            })
        })
        .collect();

    Json(phases)
}
