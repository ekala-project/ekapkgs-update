-- Known vulnerabilities (CVEs and other advisories) affecting tracked
-- packages, from OSV.dev. Written by `cve::scan` (`ekapkgs-update cve-scan`,
-- the end of `run`, and the watch loop); read by the web dashboard.

-- OSV records fetched from `GET /v1/vulns/{id}`, keyed by OSV id. A record is
-- fetched again only when a query reports a different `modified` timestamp.
CREATE TABLE IF NOT EXISTS osv_vulnerabilities (
    id TEXT PRIMARY KEY,             -- OSV id ('CVE-2024-1234', 'GHSA-…', 'PYSEC-…', 'CURL-CVE-…')
    modified TEXT NOT NULL,          -- OSV `modified` of the stored record
    aliases TEXT NOT NULL,           -- JSON array of OSV `aliases`
    summary TEXT,                    -- `summary`, or the first line of `details`
    severity TEXT NOT NULL,          -- 'critical' | 'high' | 'medium' | 'low' | 'unknown'
    cvss_score REAL,                 -- CVSS v3 base score computed from the vector, if any
    cvss_vector TEXT,                -- the CVSS vector the score came from
    published TEXT,
    withdrawn TEXT,                  -- set when OSV withdrew the record (never reported)
    fetched_at TEXT NOT NULL
);

-- Vulnerabilities affecting the version of a package that was scanned, one
-- row per distinct vulnerability: OSV records that alias each other (a CVE,
-- its GHSA and PYSEC entries, curl's CURL-CVE-…) are merged into one row.
-- Rows are replaced on every scan of the package; rows whose `version` is no
-- longer the package's `updates.current_version` are stale (the next scan
-- replaces them) and are not shown.
CREATE TABLE IF NOT EXISTS package_vulnerabilities (
    repo_name TEXT NOT NULL,
    attr_path TEXT NOT NULL,
    vuln_id TEXT NOT NULL,           -- canonical id: the CVE id when the group has one
    osv_ids TEXT NOT NULL,           -- JSON array of the merged OSV record ids
    version TEXT NOT NULL,           -- package version that was scanned
    severity TEXT NOT NULL,          -- highest severity in the group (see osv_vulnerabilities)
    cvss_score REAL,                 -- highest CVSS v3 score in the group
    summary TEXT,
    -- 1: not reported for `latest_version` (updating fixes it); 0: still
    -- affects `latest_version`; NULL: no newer upstream version to check.
    fixed_in_latest INTEGER,
    latest_version TEXT,             -- updates.latest_upstream_version at scan time
    first_seen TEXT NOT NULL,        -- first scan that reported it for this package
    last_seen TEXT NOT NULL,         -- most recent scan that reported it
    PRIMARY KEY (repo_name, attr_path, vuln_id)
);

CREATE INDEX IF NOT EXISTS idx_package_vulnerabilities_severity
    ON package_vulnerabilities(severity);

-- Per-package scan state.
ALTER TABLE updates ADD COLUMN cve_scanned_at TEXT;      -- last completed scan
ALTER TABLE updates ADD COLUMN cve_scanned_version TEXT; -- current_version at that scan
ALTER TABLE updates ADD COLUMN cve_scan_error TEXT;      -- last scan failure; NULL after a success
