-- Scope the autofix tables by repository, like `updates`.
--
-- repo_name: '' for `run` and single-repo watch, the [[repos]] name in
-- multi-repo watch. autofix_attempts.repo_name always equals its queue row's
-- repo_name (copied on insert) so attempts can be filtered without a join.
--
-- The old UNIQUE(attr_path, session_id) let two repos that failed the same
-- attr in one watch session share a queue row, so the tables are rebuilt.
-- Migrations run in a transaction with foreign_keys=ON, so the parents are
-- created first and the old tables dropped child-first; renaming the new
-- tables rewrites the REFERENCES clauses to the final names.

CREATE TABLE autofix_queue_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_name TEXT NOT NULL DEFAULT '',
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
    UNIQUE(repo_name, attr_path, session_id)
);

INSERT INTO autofix_queue_new
    (id, repo_name, attr_path, session_id, error_type, failed_phase, status,
     priority, attempts, max_attempts, created_at, updated_at, fixed_at, artifacts_path)
SELECT id, '', attr_path, session_id, error_type, failed_phase, status,
       priority, attempts, max_attempts, created_at, updated_at, fixed_at, artifacts_path
FROM autofix_queue;

CREATE TABLE autofix_attempts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    queue_id INTEGER NOT NULL REFERENCES autofix_queue_new(id),
    repo_name TEXT NOT NULL DEFAULT '',
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

INSERT INTO autofix_attempts_new
    (id, queue_id, repo_name, attempt_number, started_at, completed_at,
     prompt_tokens, completion_tokens, prompt_text, response_text, changes_json,
     changes_applied, build_success, build_stderr, status, error_message)
SELECT id, queue_id, '', attempt_number, started_at, completed_at,
       prompt_tokens, completion_tokens, prompt_text, response_text, changes_json,
       changes_applied, build_success, build_stderr, status, error_message
FROM autofix_attempts;

-- Unchanged, but it references autofix_attempts and must be rebuilt with it.
CREATE TABLE autofix_embeddings_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id INTEGER NOT NULL REFERENCES autofix_attempts_new(id),
    error_type TEXT NOT NULL,
    error_summary TEXT NOT NULL,              -- Short text that was embedded
    embedding TEXT NOT NULL,                  -- JSON array of f32 values
    fix_json TEXT,                            -- The successful fix (NULL if failed)
    build_success INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

INSERT INTO autofix_embeddings_new
    (id, attempt_id, error_type, error_summary, embedding, fix_json, build_success, created_at)
SELECT id, attempt_id, error_type, error_summary, embedding, fix_json, build_success, created_at
FROM autofix_embeddings;

DROP TABLE autofix_embeddings;
DROP TABLE autofix_attempts;
DROP TABLE autofix_queue;

ALTER TABLE autofix_queue_new RENAME TO autofix_queue;
ALTER TABLE autofix_attempts_new RENAME TO autofix_attempts;
ALTER TABLE autofix_embeddings_new RENAME TO autofix_embeddings;

CREATE INDEX idx_autofix_queue_status ON autofix_queue(status);
CREATE INDEX idx_autofix_queue_attr ON autofix_queue(repo_name, attr_path);
CREATE INDEX idx_autofix_attempts_queue ON autofix_attempts(queue_id);
CREATE INDEX idx_autofix_attempts_repo ON autofix_attempts(repo_name);
CREATE INDEX idx_autofix_embeddings_error ON autofix_embeddings(error_type);
CREATE INDEX idx_autofix_embeddings_success ON autofix_embeddings(build_success);
