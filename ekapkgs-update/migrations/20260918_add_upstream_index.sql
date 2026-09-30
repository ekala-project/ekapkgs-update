-- Reverse index: maps upstream source identifiers to Nix attr_paths.
-- Used by the `watch` command to look up which packages to update when an
-- RSS/Atom feed reports a new release for a given upstream project.

CREATE TABLE IF NOT EXISTS upstream_index (
    attr_path TEXT NOT NULL,
    upstream_type TEXT NOT NULL,           -- 'github', 'gitlab', 'sourcehut', 'pypi', 'directory'
    upstream_key TEXT NOT NULL,            -- 'owner/repo' for GitHub, 'pname' for PyPI, etc.
    instance TEXT,                         -- NULL for GitHub/PyPI/SourceHut, e.g. 'gitlab.freedesktop.org' for GitLab
    current_version TEXT,
    src_url TEXT,                          -- Original source URL for reference
    indexed_at TEXT NOT NULL,
    PRIMARY KEY (attr_path, upstream_type, upstream_key)
);

CREATE INDEX IF NOT EXISTS idx_upstream_key ON upstream_index(upstream_type, upstream_key, instance);
