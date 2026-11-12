//! Phase instrumentation for tracking update workflow execution
//!
//! [`PhaseTracker`] records one `update_phases` row per workflow phase of a
//! single package update attempt (start time, duration, status and a
//! structured [`UpdateError`] on failure). A disabled tracker (the default,
//! used by the standalone `update` command) is a no-op.
//!
//! Database errors while recording phases are logged and never fail the
//! update itself.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::{debug, warn};

use super::errors::UpdateError;
use super::types::UpdatePhase;
use crate::database::Database;

struct TrackerInner {
    db: Database,
    session_id: String,
    repo_name: String,
    attr_path: String,
    last_failure: Mutex<Option<UpdateError>>,
}

/// Records workflow phases for one package update attempt.
///
/// Cheap to clone (shared state behind an `Arc`).
#[derive(Clone, Default)]
pub struct PhaseTracker {
    inner: Option<Arc<TrackerInner>>,
}

impl std::fmt::Debug for PhaseTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Some(inner) => f
                .debug_struct("PhaseTracker")
                .field("session_id", &inner.session_id)
                .field("repo_name", &inner.repo_name)
                .field("attr_path", &inner.attr_path)
                .finish(),
            None => f.write_str("PhaseTracker(disabled)"),
        }
    }
}

impl PhaseTracker {
    /// A tracker that records nothing.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// A tracker writing phases for `attr_path` in `repo_name` to `session_id`.
    pub fn new(db: Database, session_id: &str, repo_name: &str, attr_path: &str) -> Self {
        Self {
            inner: Some(Arc::new(TrackerInner {
                db,
                session_id: session_id.to_owned(),
                repo_name: repo_name.to_owned(),
                attr_path: attr_path.to_owned(),
                last_failure: Mutex::new(None),
            })),
        }
    }

    /// The most recent phase failure recorded by this tracker.
    pub fn last_failure(&self) -> Option<UpdateError> {
        let inner = self.inner.as_ref()?;
        inner
            .last_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Start a phase. Finish it with [`PhaseHandle::success`],
    /// [`PhaseHandle::skipped`] or [`PhaseHandle::fail`].
    pub async fn start(&self, phase: UpdatePhase) -> PhaseHandle<'_> {
        let id = match &self.inner {
            Some(inner) => {
                match inner
                    .db
                    .record_phase_start(
                        &inner.session_id,
                        &inner.repo_name,
                        &inner.attr_path,
                        phase,
                    )
                    .await
                {
                    Ok(id) => Some(id),
                    Err(e) => {
                        warn!(
                            "{}: failed to record start of phase {}: {}",
                            inner.attr_path, phase, e
                        );
                        None
                    },
                }
            },
            None => None,
        };
        PhaseHandle {
            tracker: self,
            id,
            phase,
            started: Instant::now(),
        }
    }

    /// Run `fut` as `phase`, recording success or a classified failure.
    /// The future's result is returned unchanged.
    pub async fn run<T, F>(&self, phase: UpdatePhase, fut: F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>>,
    {
        let handle = self.start(phase).await;
        match fut.await {
            Ok(value) => {
                handle.success().await;
                Ok(value)
            },
            Err(e) => {
                let attr_path = self.inner.as_ref().map_or("", |i| i.attr_path.as_str());
                handle
                    .fail(UpdateError::classify(phase, attr_path, &e))
                    .await;
                Err(e)
            },
        }
    }

    /// Record an already-finished failed phase (e.g. a failure detected
    /// outside of a tracked future).
    pub async fn record_failure(&self, error: UpdateError) {
        self.start(error.phase()).await.fail(error).await;
    }
}

/// An in-progress phase started by [`PhaseTracker::start`].
pub struct PhaseHandle<'a> {
    tracker: &'a PhaseTracker,
    id: Option<i64>,
    phase: UpdatePhase,
    started: Instant,
}

impl PhaseHandle<'_> {
    pub async fn success(self) {
        let (Some(inner), Some(id)) = (&self.tracker.inner, self.id) else {
            return;
        };
        let elapsed = self.started.elapsed();
        debug!(
            "{}: {} succeeded in {:?}",
            inner.attr_path, self.phase, elapsed
        );
        if let Err(e) = inner.db.record_phase_success(id, elapsed).await {
            warn!("{}: failed to record phase success: {}", inner.attr_path, e);
        }
    }

    pub async fn skipped(self) {
        let (Some(inner), Some(id)) = (&self.tracker.inner, self.id) else {
            return;
        };
        if let Err(e) = inner
            .db
            .record_phase_skipped(id, self.started.elapsed())
            .await
        {
            warn!("{}: failed to record skipped phase: {}", inner.attr_path, e);
        }
    }

    pub async fn fail(self, error: UpdateError) {
        let Some(inner) = &self.tracker.inner else {
            return;
        };
        debug!(
            "{}: {} failed: {}",
            inner.attr_path,
            self.phase,
            error.summary()
        );
        if let Some(id) = self.id {
            if let Err(e) = inner
                .db
                .record_phase_failure(id, self.started.elapsed(), &error, None)
                .await
            {
                warn!("{}: failed to record phase failure: {}", inner.attr_path, e);
            }
        }
        *inner
            .last_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Database {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        Database::from(pool)
    }

    #[tokio::test]
    async fn tracker_records_phases() {
        let db = setup_db().await;
        let session = db
            .create_session(crate::database::SessionMode::Run, None)
            .await
            .unwrap();
        let tracker = PhaseTracker::new(db.clone(), &session, "", "hello");

        let ok: anyhow::Result<u8> = tracker
            .run(UpdatePhase::MetadataExtraction, async { Ok(1) })
            .await;
        assert_eq!(ok.unwrap(), 1);
        let err: anyhow::Result<()> = tracker
            .run(UpdatePhase::Build, async {
                Err(anyhow::anyhow!("builder for '/nix/store/x.drv' failed"))
            })
            .await;
        assert!(err.is_err());

        let phases = db.get_phases_by_attr("hello").await.unwrap();
        let statuses: Vec<(&str, &str)> = phases
            .iter()
            .rev()
            .map(|p| (p.phase.as_str(), p.status.as_str()))
            .collect();
        assert_eq!(
            statuses,
            vec![("metadata_extraction", "success"), ("build", "failed")]
        );
        assert_eq!(phases[0].error_type.as_deref(), Some("BuildError"));
        assert_eq!(
            tracker.last_failure().map(|e| e.phase()),
            Some(UpdatePhase::Build)
        );
    }

    #[tokio::test]
    async fn disabled_tracker_is_noop() {
        let tracker = PhaseTracker::disabled();
        let res: anyhow::Result<()> = tracker
            .run(UpdatePhase::Build, async { Err(anyhow::anyhow!("x")) })
            .await;
        assert!(res.is_err());
        assert!(tracker.last_failure().is_none());
    }
}
