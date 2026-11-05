-- Add repo_name column to upstream_index for multi-repo support.
-- Existing rows get an empty string (single-repo legacy mode).

-- SQLite does not support ALTER PRIMARY KEY, so we recreate the table.
CREATE TABLE IF NOT EXISTS upstream_index_new (
    repo_name TEXT NOT NULL DEFAULT '',
    attr_path TEXT NOT NULL,
    upstream_type TEXT NOT NULL,
    upstream_key TEXT NOT NULL,
    instance TEXT,
    current_version TEXT,
    src_url TEXT,
    indexed_at TEXT NOT NULL,
    PRIMARY KEY (repo_name, attr_path, upstream_type, upstream_key)
);

INSERT OR IGNORE INTO upstream_index_new
    SELECT '', attr_path, upstream_type, upstream_key, instance,
           current_version, src_url, indexed_at
    FROM upstream_index;

DROP TABLE IF EXISTS upstream_index;
ALTER TABLE upstream_index_new RENAME TO upstream_index;

CREATE INDEX IF NOT EXISTS idx_upstream_key ON upstream_index(upstream_type, upstream_key, instance);
CREATE INDEX IF NOT EXISTS idx_upstream_repo_name ON upstream_index(repo_name);
