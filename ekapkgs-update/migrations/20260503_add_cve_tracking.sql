-- Add CVE cache table for storing vulnerability data
CREATE TABLE IF NOT EXISTS cve_cache (
    package_key TEXT PRIMARY KEY,
    vulnerabilities TEXT NOT NULL,
    cached_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

-- Index for efficient cleanup of expired cache entries
CREATE INDEX IF NOT EXISTS idx_cve_cache_expires ON cve_cache(expires_at);

-- Add CVE statistics columns to updates table
ALTER TABLE updates ADD COLUMN cve_resolved_count INTEGER DEFAULT 0;
ALTER TABLE updates ADD COLUMN cve_introduced_count INTEGER DEFAULT 0;
