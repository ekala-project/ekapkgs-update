-- Add phase tracking and session management for supervised updates
--
-- This migration adds tables to track:
-- 1. Update sessions - entire run contexts
-- 2. Update phases - individual steps within updates
-- 3. Links existing update_logs to sessions

-- Track entire run sessions
CREATE TABLE IF NOT EXISTS update_sessions (
    id TEXT PRIMARY KEY,                -- UUID
    started_at TEXT NOT NULL,           -- ISO 8601 timestamp
    completed_at TEXT,                  -- NULL if still running
    status TEXT NOT NULL,               -- 'running', 'completed', 'failed', 'cancelled'
    packages_attempted INTEGER DEFAULT 0,
    packages_succeeded INTEGER DEFAULT 0,
    packages_failed INTEGER DEFAULT 0,
    packages_skipped INTEGER DEFAULT 0,
    config_json TEXT                    -- JSON of RunConfig for reproducibility
);

-- Track individual phases within update attempts
CREATE TABLE IF NOT EXISTS update_phases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,           -- Links to update_sessions table
    attr_path TEXT NOT NULL,
    phase TEXT NOT NULL,                -- UpdatePhase enum name (e.g., 'build', 'testing')
    started_at TEXT NOT NULL,           -- ISO 8601 timestamp
    completed_at TEXT,                  -- NULL if still running
    duration_ms INTEGER,                -- Duration in milliseconds
    status TEXT NOT NULL,               -- 'success', 'failed', 'skipped', 'running'
    error_type TEXT,                    -- UpdateError enum variant name (e.g., 'BuildError')
    error_details TEXT,                 -- JSON serialized UpdateError with full context
    artifacts_path TEXT,                -- Path to preserved artifacts if failure preserved
    FOREIGN KEY (session_id) REFERENCES update_sessions(id)
);

-- Update existing update_logs to link to sessions
ALTER TABLE update_logs ADD COLUMN session_id TEXT;

-- Indexes for fast queries
CREATE INDEX IF NOT EXISTS idx_sessions_status ON update_sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_started ON update_sessions(started_at DESC);

CREATE INDEX IF NOT EXISTS idx_phases_session ON update_phases(session_id);
CREATE INDEX IF NOT EXISTS idx_phases_attr ON update_phases(attr_path);
CREATE INDEX IF NOT EXISTS idx_phases_status ON update_phases(status);
CREATE INDEX IF NOT EXISTS idx_phases_error_type ON update_phases(error_type);
CREATE INDEX IF NOT EXISTS idx_phases_started ON update_phases(started_at DESC);

CREATE INDEX IF NOT EXISTS idx_logs_session ON update_logs(session_id);
