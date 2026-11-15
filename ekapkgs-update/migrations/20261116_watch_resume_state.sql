-- What a repo's last index build was produced from: the ekapkgs-update
-- binary, enumeration entry point, select expression and evaluation entry
-- point. On startup, watch reuses a recent successful index only when this
-- still matches, so a config change or a newly deployed build re-indexes.
ALTER TABLE watched_repos ADD COLUMN index_config TEXT;
