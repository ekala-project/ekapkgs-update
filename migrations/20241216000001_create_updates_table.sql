CREATE TABLE IF NOT EXISTS updates (
    attr_path TEXT PRIMARY KEY,
    last_attempted TEXT,
    next_attempt TEXT,
    current_version TEXT,
    proposed_version TEXT,
    latest_upstream_version TEXT
);
