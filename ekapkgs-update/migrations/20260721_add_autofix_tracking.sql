-- Autofix queue: tracks packages awaiting or undergoing LLM fix attempts
CREATE TABLE IF NOT EXISTS autofix_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    attr_path TEXT NOT NULL,
    session_id TEXT NOT NULL,
    error_type TEXT NOT NULL,
    failed_phase TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',  -- queued|processing|fixed|escalated|skipped
    priority INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 0,
    max_attempts INTEGER DEFAULT 3,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    fixed_at TEXT,
    artifacts_path TEXT,
    UNIQUE(attr_path, session_id)
);

-- Autofix attempts: records each LLM interaction attempt
CREATE TABLE IF NOT EXISTS autofix_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    queue_id INTEGER NOT NULL REFERENCES autofix_queue(id),
    attempt_number INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    prompt_text TEXT,
    response_text TEXT,
    changes_json TEXT,
    changes_applied INTEGER DEFAULT 0,
    build_success INTEGER,
    build_stderr TEXT,
    status TEXT NOT NULL DEFAULT 'pending',  -- pending|llm_error|parse_error|apply_error|build_failed|success
    error_message TEXT
);

-- Autofix embeddings: stores vector embeddings of error contexts for RAG retrieval
CREATE TABLE IF NOT EXISTS autofix_embeddings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id INTEGER NOT NULL REFERENCES autofix_attempts(id),
    error_type TEXT NOT NULL,
    error_summary TEXT NOT NULL,              -- Short text that was embedded
    embedding TEXT NOT NULL,                  -- JSON array of f32 values
    fix_json TEXT,                            -- The successful fix (NULL if failed)
    build_success INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_autofix_queue_status ON autofix_queue(status);
CREATE INDEX IF NOT EXISTS idx_autofix_queue_attr ON autofix_queue(attr_path);
CREATE INDEX IF NOT EXISTS idx_autofix_attempts_queue ON autofix_attempts(queue_id);
CREATE INDEX IF NOT EXISTS idx_autofix_embeddings_error ON autofix_embeddings(error_type);
CREATE INDEX IF NOT EXISTS idx_autofix_embeddings_success ON autofix_embeddings(build_success);
