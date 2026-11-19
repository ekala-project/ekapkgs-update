/// Message sent from release checker service to updater service
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    pub attr_path: String,
    /// Derivation store path for failure recording. `None` when the request
    /// originates from a source that doesn't have iets eval-jobs output (e.g.
    /// the `watch` command).
    pub drv_path: Option<String>,
    pub current_version: String,
    pub new_version: String,

    // ── Multi-repo routing (all optional for backwards compatibility) ──
    /// Name of the source repo (from config). Empty/None for single-repo mode.
    pub repo_name: Option<String>,
    /// Eval entry point for this repo (absolute path). When set, overrides the
    /// global `eval_entry_point` in `UpdaterServiceConfig`.
    pub eval_entry_point: Option<String>,
    /// PR config for the target repo. When set, overrides the global config.
    pub pr_config: Option<crate::git::PrConfig>,
    /// Fork remote for this repo. When set, overrides the global fork.
    pub fork: Option<String>,
}

impl UpdateRequest {
    /// Repo scope used for database rows (`''` = default / `run` command).
    pub fn repo(&self) -> &str {
        self.repo_name.as_deref().unwrap_or("")
    }
}

/// An update group dispatched as one job: every member is updated in one
/// worktree (one commit each, in dependency order) and proposed as one PR.
#[derive(Debug, Clone)]
pub struct GroupRequest {
    /// The group's root attr path (may be a label that is not a package).
    pub root: String,
    /// Members to update. Routing (repo, entry point, PR config, fork) is
    /// taken from the first member; all members share a repo.
    pub members: Vec<UpdateRequest>,
    /// Attr paths built to verify the group after all members were updated:
    /// every member (updated or not) and the root.
    pub verify: Vec<String>,
}

impl GroupRequest {
    /// The member whose routing the group uses.
    pub fn lead(&self) -> &UpdateRequest {
        &self.members[0]
    }
}

/// A unit of work for the updater service.
#[derive(Debug, Clone)]
pub enum UpdateJob {
    Single(UpdateRequest),
    Group(GroupRequest),
}

impl UpdateJob {
    /// Human-readable label for logs (`attr` or `group <root>`).
    pub fn label(&self) -> String {
        match self {
            UpdateJob::Single(req) => req.attr_path.clone(),
            UpdateJob::Group(group) => format!("group {}", group.root),
        }
    }

    /// Number of package updates this job attempts.
    pub fn attempts(&self) -> usize {
        match self {
            UpdateJob::Single(_) => 1,
            UpdateJob::Group(group) => group.members.len(),
        }
    }
}

/// Result of an update operation
#[derive(Debug)]
pub enum UpdateResult {
    Updated {
        old_version: String,
        new_version: String,
    },
    /// Not attempted / not applicable (e.g. rebuild threshold exceeded,
    /// worktree setup failed). Not recorded as a package failure. Also used
    /// when the tree already has the target version (a sibling attribute
    /// sharing the file was updated first); that case is recorded as an
    /// up-to-date observation.
    Skipped(String),
    /// The update was attempted and failed; already recorded in the
    /// database (failure log, backoff). Carries a one-line summary.
    Failed(String),
    DryRun {
        current_version: String,
        new_version: String,
    },
}
