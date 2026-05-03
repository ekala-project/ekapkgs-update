-- Add Repology cache table for storing cross-distribution version data
CREATE TABLE IF NOT EXISTS repology_cache (
    project_name TEXT PRIMARY KEY,
    project_data TEXT NOT NULL,
    cached_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

-- Index for efficient cleanup of expired cache entries
CREATE INDEX IF NOT EXISTS idx_repology_cache_expires ON repology_cache(expires_at);
