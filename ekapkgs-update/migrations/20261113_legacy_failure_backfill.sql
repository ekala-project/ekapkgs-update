-- Backfill failure state for packages that failed before 20261111.
--
-- Before the dashboard data model, a failed update attempt was written only
-- to `update_logs`. The `updates` row was not touched. A later check that
-- found nothing to do, or a successful update, set `updates.last_attempted`.
-- So a package whose newest failure log is newer than its `last_attempted`
-- was still failing when the database was upgraded. 20261111 marked it
-- 'pending'/'outdated' with failure_count = 0.
--
-- For those rows this sets what `record_failed_update` would have written:
--   status          'failed'
--   failed_version  new_version of the newest failure log
--   failure_count   failure logs newer than last_attempted (one per drv,
--                   since update_logs is keyed by drv_path)
--   last_session_id session of the newest failure log
--   last_attempted  timestamp of the newest failure log
-- next_attempt is kept. last_error stays NULL. `Database::new` fills it in
-- with `summarize_error` (a Rust function) after the migrations run.
--
-- Rows the current code has written are never matched: its failures set
-- status = 'failed' with failure_count >= 1, and set last_attempted to the
-- failure log's own timestamp, so no log is ever strictly newer.

UPDATE updates
SET
    status = 'failed',
    failed_version = (
        SELECT l.new_version FROM update_logs l
        WHERE l.repo_name = updates.repo_name AND l.attr_path = updates.attr_path
          AND l.status = 'failed'
        ORDER BY julianday(l.timestamp) DESC LIMIT 1
    ),
    failure_count = (
        SELECT COUNT(*) FROM update_logs l
        WHERE l.repo_name = updates.repo_name AND l.attr_path = updates.attr_path
          AND l.status = 'failed'
          AND (updates.last_attempted IS NULL
               OR julianday(l.timestamp) > julianday(updates.last_attempted))
    ),
    last_session_id = (
        SELECT l.session_id FROM update_logs l
        WHERE l.repo_name = updates.repo_name AND l.attr_path = updates.attr_path
          AND l.status = 'failed'
        ORDER BY julianday(l.timestamp) DESC LIMIT 1
    ),
    last_attempted = (
        SELECT l.timestamp FROM update_logs l
        WHERE l.repo_name = updates.repo_name AND l.attr_path = updates.attr_path
          AND l.status = 'failed'
        ORDER BY julianday(l.timestamp) DESC LIMIT 1
    ),
    last_error = NULL
WHERE repo_name = ''
  AND status != 'failed'
  AND failure_count = 0
  AND EXISTS (
      SELECT 1 FROM update_logs l
      WHERE l.repo_name = updates.repo_name AND l.attr_path = updates.attr_path
        AND l.status = 'failed'
        AND (updates.last_attempted IS NULL
             OR julianday(l.timestamp) > julianday(updates.last_attempted))
  );
