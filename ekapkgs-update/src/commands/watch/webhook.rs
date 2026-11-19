//! GitHub webhook endpoint: `POST /github/webhook`, served by `watch
//! --webhook-listen` behind a TLS-terminating reverse proxy.
//!
//! Every delivery is checked against the webhook secret
//! (`X-Hub-Signature-256`) and recorded in `webhook_deliveries` by its
//! `X-GitHub-Delivery` id, so a redelivery is not processed twice. Handling
//! is quick and answered well within GitHub's 10 second limit:
//!
//! - a push to a watched repo's base branch, or a merged PR, asks the head
//!   watcher ([`super::refresh`]) to sync and re-index that repo now;
//! - a PR closed without merging withdraws its proposal
//!   ([`Database::record_pr_closed`]).
//!
//! GitHub does not retry failed deliveries; the periodic head check catches
//! up on anything missed.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::ResolvedRepo;
use crate::database::Database;
use crate::github::webhook::{WebhookEvent, parse_event, payload_action, verify_signature};

/// Path the endpoint is served at.
pub const WEBHOOK_PATH: &str = "/github/webhook";
/// Webhook deliveries are kept this long.
const DELIVERY_RETENTION_DAYS: i64 = 7;

#[derive(Clone)]
pub(super) struct WebhookState {
    secret: Arc<[u8]>,
    db: Database,
    repos: Arc<Vec<ResolvedRepo>>,
    /// Repo names for the head watcher to check now.
    triggers: mpsc::UnboundedSender<String>,
}

impl WebhookState {
    pub(super) fn new(
        secret: Vec<u8>,
        db: Database,
        repos: Arc<Vec<ResolvedRepo>>,
        triggers: mpsc::UnboundedSender<String>,
    ) -> Self {
        Self {
            secret: secret.into(),
            db,
            repos,
            triggers,
        }
    }

    /// Act on one (new, authenticated) delivery; returns a one-line outcome
    /// and whether anything was done.
    async fn handle(&self, event: WebhookEvent) -> (bool, String) {
        let repository = event.repository().map(str::to_owned);
        let repo = repository.as_deref().and_then(|r| self.repo_for(r));
        match (event, repo) {
            (WebhookEvent::Ping, _) => (false, "pong".to_owned()),
            (WebhookEvent::Ignored(why), _) => (false, format!("ignored: {why}")),
            (_, None) => (
                false,
                format!(
                    "ignored: {} is not a watched repository",
                    repository.unwrap_or_default()
                ),
            ),
            (WebhookEvent::Push { branch, .. }, Some(repo)) => {
                let base = repo.pr_config.as_ref().map(|c| c.base_branch.as_str());
                if base == Some(branch.as_str()) {
                    self.trigger(repo);
                    (true, format!("push to {branch}: checking {}", repo.label()))
                } else {
                    (false, format!("ignored: push to {branch}"))
                }
            },
            (
                WebhookEvent::PullRequestClosed {
                    number,
                    merged: true,
                    ..
                },
                Some(repo),
            ) => {
                self.trigger(repo);
                (
                    true,
                    format!("PR #{number} merged: checking {}", repo.label()),
                )
            },
            (
                WebhookEvent::PullRequestClosed {
                    number,
                    merged: false,
                    ..
                },
                Some(repo),
            ) => match self.db.record_pr_closed(&repo.name, number).await {
                Ok(Some((attr_path, version))) => (
                    true,
                    format!(
                        "PR #{number} closed: withdrew {attr_path} {version} (retried after the \
                         failure backoff)"
                    ),
                ),
                Ok(None) => (
                    false,
                    format!("PR #{number} closed: not a pending proposal"),
                ),
                Err(e) => (false, format!("PR #{number} closed: {e:#}")),
            },
        }
    }

    /// The watched repo whose upstream is `full_name` (`owner/name`).
    fn repo_for(&self, full_name: &str) -> Option<&ResolvedRepo> {
        self.repos.iter().find(|r| {
            r.pr_config
                .as_ref()
                .is_some_and(|c| format!("{}/{}", c.owner, c.repo).eq_ignore_ascii_case(full_name))
        })
    }

    fn trigger(&self, repo: &ResolvedRepo) {
        if self.triggers.send(repo.name.clone()).is_err() {
            warn!(
                "[{}] Head watcher is not running; webhook not acted on",
                repo.label()
            );
        }
    }
}

pub(super) fn router(state: WebhookState) -> Router {
    Router::new()
        .route(WEBHOOK_PATH, post(receive))
        .with_state(state)
}

/// Bind `addr` and serve the endpoint in the background.
pub(super) async fn serve(
    addr: SocketAddr,
    state: WebhookState,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind webhook listener on {addr}"))?;
    info!("Receiving GitHub webhooks on http://{addr}{WEBHOOK_PATH}");
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router(state)).await {
            warn!("Webhook listener stopped: {e:#}");
        }
    }))
}

async fn receive(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if !verify_signature(&state.secret, &body, header("x-hub-signature-256")) {
        warn!("Rejected a webhook delivery with a missing or invalid signature");
        return (StatusCode::UNAUTHORIZED, "invalid signature".to_owned());
    }
    let (Some(event), Some(delivery)) = (header("x-github-event"), header("x-github-delivery"))
    else {
        return (
            StatusCode::BAD_REQUEST,
            "missing X-GitHub-Event or X-GitHub-Delivery".to_owned(),
        );
    };
    let parsed = match parse_event(event, &body) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!("Webhook {event} {delivery}: unreadable payload: {e:#}");
            return (
                StatusCode::BAD_REQUEST,
                format!("unreadable {event} payload"),
            );
        },
    };

    match state
        .db
        .record_webhook_delivery(
            delivery,
            event,
            payload_action(&body).as_deref(),
            parsed.repository(),
        )
        .await
    {
        Ok(true) => {},
        Ok(false) => {
            debug!("Webhook {event} {delivery}: already processed");
            return (StatusCode::OK, "duplicate delivery".to_owned());
        },
        // Acting without a record beats dropping the delivery.
        Err(e) => warn!("Webhook {event} {delivery}: not recorded: {e:#}"),
    }

    let (acted, outcome) = state.handle(parsed).await;
    if acted {
        info!("Webhook {event} {delivery}: {outcome}");
    } else {
        debug!("Webhook {event} {delivery}: {outcome}");
    }
    if let Err(e) = state.db.set_webhook_outcome(delivery, &outcome).await {
        debug!("Webhook {delivery}: outcome not recorded: {e:#}");
    }
    if let Err(e) = state
        .db
        .prune_webhook_deliveries(DELIVERY_RETENTION_DAYS)
        .await
    {
        debug!("Failed to prune webhook deliveries: {e:#}");
    }
    let status = if acted {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    (status, outcome)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::git::PrConfig;
    use crate::github::webhook::signature;

    const SECRET: &[u8] = b"s3cret";

    async fn setup() -> (Router, Database, mpsc::UnboundedReceiver<String>) {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let db = Database::from(pool);
        let repo = ResolvedRepo {
            name: "corepkgs".into(),
            enumerate_entry_point: "/r/default.nix".into(),
            select: None,
            eval_entry_point: "/r/default.nix".into(),
            repo_dir: PathBuf::from("/r"),
            pr_config: Some(PrConfig {
                owner: "ekala-project".into(),
                repo: "corepkgs".into(),
                base_branch: "master".into(),
            }),
            fork: "origin".into(),
            upstream_url: Some("https://github.com/ekala-project/corepkgs".into()),
            managed: true,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let state = WebhookState::new(SECRET.to_vec(), db.clone(), Arc::new(vec![repo]), tx);
        (router(state), db, rx)
    }

    async fn deliver(
        app: &Router,
        event: &str,
        delivery: &str,
        body: &str,
        sig: Option<String>,
    ) -> (StatusCode, String) {
        let mut request = Request::post(WEBHOOK_PATH)
            .header("x-github-event", event)
            .header("x-github-delivery", delivery);
        if let Some(sig) = sig {
            request = request.header("x-hub-signature-256", sig);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn signed(app: &Router, event: &str, delivery: &str, body: &str) -> (StatusCode, String) {
        deliver(
            app,
            event,
            delivery,
            body,
            Some(signature(SECRET, body.as_bytes()).unwrap()),
        )
        .await
    }

    fn push(repo: &str, git_ref: &str) -> String {
        format!(r#"{{"ref":"{git_ref}","deleted":false,"repository":{{"full_name":"{repo}"}}}}"#)
    }

    fn pr_closed(number: i64, merged: bool) -> String {
        format!(
            r#"{{"action":"closed","number":{number},"pull_request":{{"merged":{merged}}},
               "repository":{{"full_name":"ekala-project/corepkgs"}}}}"#
        )
    }

    #[tokio::test]
    async fn rejects_unsigned_and_forged_deliveries() {
        let (app, db, mut rx) = setup().await;
        let body = push("ekala-project/corepkgs", "refs/heads/master");
        let (status, _) = deliver(&app, "push", "d1", &body, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let forged = signature(b"wrong", body.as_bytes()).unwrap();
        let (status, _) = deliver(&app, "push", "d1", &body, Some(forged)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(rx.try_recv().is_err());
        let recorded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(recorded, 0);
    }

    #[tokio::test]
    async fn push_to_base_branch_triggers_a_check_once() {
        let (app, db, mut rx) = setup().await;
        let (status, _) = signed(&app, "ping", "d0", r#"{"zen":"hi"}"#).await;
        assert_eq!(status, StatusCode::OK);

        let body = push("Ekala-Project/CorePkgs", "refs/heads/master");
        let (status, outcome) = signed(&app, "push", "d1", &body).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{outcome}");
        assert_eq!(rx.try_recv().unwrap(), "corepkgs");

        // A redelivery keeps its id and is not acted on again.
        let (status, outcome) = signed(&app, "push", "d1", &body).await;
        assert_eq!(
            (status, outcome.as_str()),
            (StatusCode::OK, "duplicate delivery")
        );
        assert!(rx.try_recv().is_err());

        let row: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT event, repository, outcome FROM webhook_deliveries WHERE delivery_id = 'd1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.0, "push");
        assert_eq!(row.1.as_deref(), Some("Ekala-Project/CorePkgs"));
        assert!(row.2.unwrap().starts_with("push to master"));
    }

    #[tokio::test]
    async fn other_branches_and_repos_are_ignored() {
        let (app, _db, mut rx) = setup().await;
        for (delivery, body) in [
            (
                "d1",
                push("ekala-project/corepkgs", "refs/heads/update/grpc/1.85.3"),
            ),
            ("d2", push("ekala-project/corepkgs", "refs/tags/v1")),
            ("d3", push("someone/else", "refs/heads/master")),
        ] {
            let (status, outcome) = signed(&app, "push", delivery, &body).await;
            assert_eq!(status, StatusCode::OK, "{outcome}");
            assert!(outcome.starts_with("ignored"), "{outcome}");
        }
        assert!(rx.try_recv().is_err());
        let (status, _) = signed(&app, "push", "d4", "not json").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn closed_pull_requests() {
        let (app, db, mut rx) = setup().await;
        sqlx::query(
            "INSERT INTO updates (repo_name, attr_path, status, current_version, \
             proposed_version, pr_number, pr_url) \
             VALUES ('corepkgs', 'grpc', 'proposed', '1.85.1', '1.85.3', 302, 'u')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let (status, _) = signed(&app, "pull_request", "d1", &pr_closed(302, true)).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap(), "corepkgs");

        let (status, outcome) = signed(&app, "pull_request", "d2", &pr_closed(302, false)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{outcome}");
        let record = db
            .get_update_record("corepkgs", "grpc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.status.as_str(), "failed");
        assert_eq!(record.proposed_version, None);
        assert_eq!(record.failed_version.as_deref(), Some("1.85.3"));
        assert_eq!(record.failure_count, 1);
        assert!(record.in_backoff(chrono::Utc::now()));

        // Unknown or already handled PRs change nothing.
        let (status, outcome) = signed(&app, "pull_request", "d3", &pr_closed(302, false)).await;
        assert_eq!(status, StatusCode::OK, "{outcome}");
        let (status, _) = signed(&app, "pull_request", "d4", &pr_closed(999, false)).await;
        assert_eq!(status, StatusCode::OK);
    }
}
