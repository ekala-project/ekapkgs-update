-- Dashboard data model (read by ekapkgs-update-web).
--
-- 1. `updates` becomes repo-scoped: PRIMARY KEY (repo_name, attr_path).
--    repo_name = '' for the `run` command and legacy single-repo watch mode.
--    Adds an explicit `status`, failure/backoff bookkeeping, package metadata
--    and the watch bootstrap `baseline_version`.
-- 2. `update_logs` and `update_phases` gain `repo_name`.
-- 3. `update_sessions` gains `mode` ('run' | 'watch'), extra counters and a
--    heartbeat (`updated_at`).
-- 4. New tables: `watched_repos`, `watch_cycles`, `upstream_sources`.
-- 5. `upstream_index` gains per-package version-selection settings so the
--    watch poller honours semver-strategy / version-regex / include-prereleases.

-- ── 1. updates ─────────────────────────────────────────────────────────────

CREATE TABLE updates_new (
    repo_name TEXT NOT NULL DEFAULT '',
    attr_path TEXT NOT NULL,
    -- 'pending'        known (indexed) but never compared against upstream
    -- 'up_to_date'     current version is the best compatible upstream release
    -- 'outdated'       a newer compatible release exists and has not been proposed
    -- 'proposed'       a newer version was built and proposed (PR opened or committed)
    -- 'failed'         the most recent update attempt failed
    -- 'upstream_error' fetching/selecting upstream releases failed
    status TEXT NOT NULL DEFAULT 'pending',
    current_version TEXT,
    latest_upstream_version TEXT,             -- best compatible upstream version
    proposed_version TEXT,                    -- version successfully built + proposed
    baseline_version TEXT,                    -- watch: upstream version seen at bootstrap
    failed_version TEXT,                      -- target version of the last failed attempt
    failure_count INTEGER NOT NULL DEFAULT 0, -- consecutive failed attempts (drives backoff)
    last_error TEXT,                          -- one-line summary of the last failure/error
    last_session_id TEXT,                     -- session of the last update attempt
    last_checked TEXT,                        -- last time upstream was compared
    last_attempted TEXT,                      -- last update attempt (or legacy check time)
    next_attempt TEXT,                        -- backoff: do not check/attempt before this
    pr_url TEXT,                              -- last PR opened; kept after merge (open iff status = 'proposed')
    pr_number INTEGER,                        -- number of that PR
    rebuild_count INTEGER,
    cve_resolved_count INTEGER DEFAULT 0,
    cve_introduced_count INTEGER DEFAULT 0,
    -- Package metadata (refreshed by the checker and the watch indexer)
    pname TEXT,
    description TEXT,
    homepage TEXT,
    upstream_type TEXT,
    upstream_key TEXT,
    upstream_instance TEXT,
    semver_strategy TEXT,
    PRIMARY KEY (repo_name, attr_path)
);

INSERT INTO updates_new (
    repo_name, attr_path, status, current_version, latest_upstream_version,
    proposed_version, last_checked, last_attempted, next_attempt, pr_url,
    pr_number, rebuild_count, cve_resolved_count, cve_introduced_count
)
SELECT
    '',
    attr_path,
    CASE
        WHEN proposed_version IS NOT NULL
             AND proposed_version != COALESCE(current_version, '') THEN 'proposed'
        WHEN latest_upstream_version IS NULL THEN 'pending'
        WHEN latest_upstream_version = 'unknown' THEN 'upstream_error'
        WHEN latest_upstream_version = current_version THEN 'up_to_date'
        ELSE 'outdated'
    END,
    current_version,
    NULLIF(latest_upstream_version, 'unknown'),
    proposed_version,
    last_attempted,
    last_attempted,
    next_attempt,
    pr_url,
    pr_number,
    rebuild_count,
    COALESCE(cve_resolved_count, 0),
    COALESCE(cve_introduced_count, 0)
FROM updates;

DROP TABLE updates;
ALTER TABLE updates_new RENAME TO updates;

CREATE INDEX IF NOT EXISTS idx_updates_attr ON updates(attr_path);
CREATE INDEX IF NOT EXISTS idx_updates_status ON updates(status);
CREATE INDEX IF NOT EXISTS idx_updates_upstream ON updates(upstream_type, upstream_key);

-- ── 2. update_logs / update_phases ────────────────────────────────────────

-- drv_path stays the primary key. Requests without a derivation (watch mode)
-- use a synthetic key of the form '<repo>:<attr_path>@<new_version>'.
ALTER TABLE update_logs ADD COLUMN repo_name TEXT NOT NULL DEFAULT '';
-- UpdatePhase name (snake_case) in which the attempt failed, when known.
ALTER TABLE update_logs ADD COLUMN phase TEXT;
-- UpdateError variant name (e.g. 'BuildError'), when known.
ALTER TABLE update_logs ADD COLUMN error_type TEXT;

CREATE INDEX IF NOT EXISTS idx_logs_repo_attr ON update_logs(repo_name, attr_path);

ALTER TABLE update_phases ADD COLUMN repo_name TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_phases_session_attr ON update_phases(session_id, attr_path);

-- ── 3. update_sessions ────────────────────────────────────────────────────

-- 'run' (batch checker) or 'watch' (one session per poll cycle that
-- dispatched at least one update).
ALTER TABLE update_sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'run';
-- Packages compared against upstream by the checker (run) / poller (watch).
ALTER TABLE update_sessions ADD COLUMN packages_evaluated INTEGER DEFAULT 0;
-- Packages the checker skipped because they were in a backoff window.
ALTER TABLE update_sessions ADD COLUMN packages_backoff INTEGER DEFAULT 0;
-- Heartbeat: touched whenever a counter changes.
ALTER TABLE update_sessions ADD COLUMN updated_at TEXT;

UPDATE update_sessions SET mode = 'watch' WHERE config_json LIKE '%"mode":%"watch"%';
UPDATE update_sessions SET updated_at = COALESCE(completed_at, started_at);

CREATE INDEX IF NOT EXISTS idx_sessions_mode ON update_sessions(mode);

-- ── 4. watch tables ───────────────────────────────────────────────────────

-- One row per repository watched by `ekapkgs-update watch`.
CREATE TABLE IF NOT EXISTS watched_repos (
    name TEXT PRIMARY KEY,           -- '' = legacy single-repo mode (current directory)
    upstream TEXT,                   -- upstream URL from config (NULL in legacy mode)
    path TEXT NOT NULL,              -- local checkout
    entrypoint TEXT NOT NULL,        -- absolute eval entry point
    base_branch TEXT,
    fork_remote TEXT,
    head_commit TEXT,                -- HEAD after the last sync
    head_subject TEXT,
    sync_status TEXT,                -- 'ok' | 'diverged' | 'error' | 'unmanaged'
    sync_error TEXT,
    last_synced_at TEXT,
    index_status TEXT,               -- 'indexing' | 'ok' | 'error'
    index_error TEXT,
    last_indexed_at TEXT,
    index_duration_ms INTEGER,
    evaluated_count INTEGER,         -- derivations produced by nix-eval-jobs
    indexed_count INTEGER,           -- packages with a resolvable upstream source
    index_skipped INTEGER,           -- skip=true / no upstream / pinned / metadata errors
    index_errors INTEGER,            -- evaluation errors
    bootstrapped_at TEXT,            -- first completed poll (baselines recorded)
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- One row per watch poll cycle or index build.
CREATE TABLE IF NOT EXISTS watch_cycles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,              -- 'poll' | 'index'
    repo_name TEXT,                  -- index: the repo; poll: NULL (all repos)
    started_at TEXT NOT NULL,
    completed_at TEXT,
    duration_ms INTEGER,
    status TEXT NOT NULL,            -- 'running' | 'completed' | 'failed'
    bootstrap INTEGER NOT NULL DEFAULT 0, -- poll: 1 if it recorded at least one baseline
    sources_polled INTEGER NOT NULL DEFAULT 0,
    sources_failed INTEGER NOT NULL DEFAULT 0,
    packages_observed INTEGER NOT NULL DEFAULT 0, -- poll: packages compared; index: indexed
    new_releases INTEGER NOT NULL DEFAULT 0,      -- poll: sources whose latest version changed
    dispatched INTEGER NOT NULL DEFAULT 0,
    updated INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    session_id TEXT,                 -- poll: session created when dispatched > 0
    error TEXT
);

CREATE INDEX IF NOT EXISTS idx_watch_cycles_started ON watch_cycles(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_watch_cycles_kind ON watch_cycles(kind, started_at DESC);

-- Poll health and latest release per unique upstream source.
CREATE TABLE IF NOT EXISTS upstream_sources (
    upstream_type TEXT NOT NULL,
    upstream_key TEXT NOT NULL,
    -- '' when not applicable (NULLs in a SQLite primary key are distinct).
    -- Join with: COALESCE(upstream_index.instance, '') = upstream_sources.instance
    instance TEXT NOT NULL DEFAULT '',
    latest_version TEXT,             -- newest stable release (ignores per-package strategy)
    latest_tag TEXT,
    release_count INTEGER,
    latest_changed_at TEXT,          -- when latest_version last changed
    first_seen_at TEXT NOT NULL,
    last_polled_at TEXT,
    last_success_at TEXT,
    last_error TEXT,                 -- NULL after a successful poll
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (upstream_type, upstream_key, instance)
);

-- ── 5. upstream_index ─────────────────────────────────────────────────────

ALTER TABLE upstream_index ADD COLUMN pname TEXT;
-- 'latest' | 'major' | 'minor' | 'patch' (effective strategy for this package)
ALTER TABLE upstream_index ADD COLUMN semver_strategy TEXT;
ALTER TABLE upstream_index ADD COLUMN version_regex TEXT;
ALTER TABLE upstream_index ADD COLUMN include_prereleases INTEGER NOT NULL DEFAULT 0;
