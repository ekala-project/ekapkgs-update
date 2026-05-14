use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use ekapkgs_update::database::SessionStatus;
use tokio::time::interval;

use crate::state::AppState;

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_websocket(socket, state))
}

async fn handle_websocket(mut socket: WebSocket, state: AppState) {
    let mut tick = interval(Duration::from_secs(2));

    loop {
        tick.tick().await;

        // Get running sessions and their status
        let running_sessions = state
            .db
            .get_sessions_by_status(SessionStatus::Running)
            .await
            .unwrap_or_default();

        // Prepare update message
        let update = serde_json::json!({
            "type": "update",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "active_sessions": running_sessions.len(),
            "sessions": running_sessions.iter().map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "started_at": s.started_at.to_rfc3339(),
                    "packages_attempted": s.packages_attempted,
                    "packages_succeeded": s.packages_succeeded,
                    "packages_failed": s.packages_failed,
                    "packages_skipped": s.packages_skipped,
                })
            }).collect::<Vec<_>>(),
        });

        let msg = Message::Text(serde_json::to_string(&update).unwrap());

        // Send update to client
        if socket.send(msg).await.is_err() {
            // Client disconnected
            break;
        }
    }
}
