//! `/ws/live`: tells browsers when something on screen may have changed.
//!
//! Each connection polls a cheap database fingerprint every
//! [`POLL_INTERVAL`] and sends `{"type":"live","v":"<hash>"}` only when it
//! changes (plus once on connect). `static/js/app.js` forwards a changed
//! value to the Elm app (the `liveUpdate` port), which re-fetches the data
//! of the page on screen.

use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use tokio::time::{Instant, MissedTickBehavior, interval};

use crate::queries::live::fingerprint;
use crate::state::AppState;

/// How often each connection re-reads the fingerprint.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Send a ping when nothing else was sent for this long (keeps proxies from
/// closing idle connections).
const PING_AFTER: Duration = Duration::from_secs(30);

/// `GET /ws/live`
pub async fn live(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state))
}

/// The message for a fingerprint: a short hash, so nothing about the
/// database leaks and messages stay tiny.
pub fn message(fp: &str) -> String {
    format!(
        r#"{{"type":"live","v":"{:016x}"}}"#,
        crate::assets::fnv1a(fp.as_bytes())
    )
}

async fn serve(mut socket: WebSocket, state: AppState) {
    let mut tick = interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last: Option<String> = None;
    let mut last_send = Instant::now();

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let fp = match fingerprint(state.db.pool()).await {
                    Ok(fp) => fp,
                    Err(e) => {
                        tracing::warn!("live fingerprint query failed: {e}");
                        continue;
                    },
                };
                let outgoing = if last.as_deref() == Some(fp.as_str()) {
                    (last_send.elapsed() >= PING_AFTER).then(|| Message::Ping(Vec::new()))
                } else {
                    let msg = Message::Text(message(&fp));
                    last = Some(fp);
                    Some(msg)
                };
                if let Some(msg) = outgoing {
                    if socket.send(msg).await.is_err() {
                        break;
                    }
                    last_send = Instant::now();
                }
            },
            incoming = socket.recv() => {
                match incoming {
                    // Pongs and stray client messages are ignored.
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {},
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::message;

    #[test]
    fn message_is_small_and_stable() {
        let a = message("1|0||0:0:|");
        assert_eq!(a, message("1|0||0:0:|"));
        assert_ne!(a, message("2|0||0:0:|"));
        assert!(a.starts_with(r#"{"type":"live","v":""#));
        assert!(a.len() < 48);
    }
}
