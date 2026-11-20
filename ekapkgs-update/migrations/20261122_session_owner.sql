-- Owner of a session: the process that created it. `run` closes sessions
-- left `running` by a process that no longer exists (killed, OOM, crash)
-- when it starts (`Database::recover_stale_run_sessions`); a session whose
-- owner may still be alive is never touched.
ALTER TABLE update_sessions ADD COLUMN owner_host TEXT;   -- hostname of the owning process (/proc/sys/kernel/hostname); NULL = unknown or created before this migration
ALTER TABLE update_sessions ADD COLUMN owner_pid INTEGER; -- PID of the owning process; NULL = created before this migration
