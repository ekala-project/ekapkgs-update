-- The commit a watched repo's upstream index was last built from (HEAD of the
-- checkout at the start of a successful build). Watch re-indexes when the
-- checkout or its upstream branch moves past it, e.g. after a PR was merged,
-- instead of waiting for the periodic refresh. NULL until the first
-- successful build with this column; a failed build keeps the previous value.
ALTER TABLE watched_repos ADD COLUMN indexed_commit TEXT;
