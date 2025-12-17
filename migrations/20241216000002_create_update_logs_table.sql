CREATE TABLE IF NOT EXISTS update_logs (
    drv_path TEXT PRIMARY KEY,
    attr_path TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    status TEXT NOT NULL,
    error_log TEXT NOT NULL,
    old_version TEXT,
    new_version TEXT
);

CREATE INDEX IF NOT EXISTS idx_update_logs_attr_path ON update_logs(attr_path);
CREATE INDEX IF NOT EXISTS idx_update_logs_timestamp ON update_logs(timestamp DESC);
