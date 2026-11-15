-- Classify upstream source health so dead and relocated upstreams stand out
-- from transient poll failures.

-- 'ok'        the last poll succeeded at the indexed location
-- 'error'     the last poll failed for a possibly transient reason
--             (network, rate limit, 5xx, unparsable response)
-- 'not_found' the last poll got HTTP 404/410: the project was deleted,
--             made private, renamed without a redirect, or the package
--             points at the wrong location
-- 'moved'     the last poll succeeded, but only through a redirect to
--             another repository (see moved_to); the package should be
--             pointed at the new location
ALTER TABLE upstream_sources ADD COLUMN health TEXT NOT NULL DEFAULT 'ok';
-- Where a 'moved' source now lives (e.g. 'new-owner/repo'); NULL otherwise.
ALTER TABLE upstream_sources ADD COLUMN moved_to TEXT;

UPDATE upstream_sources
SET health = CASE
    WHEN last_error LIKE '%404 Not Found%' OR last_error LIKE '%410 Gone%' THEN 'not_found'
    ELSE 'error'
END
WHERE consecutive_failures > 0;

-- Sources no package is indexed against any more (e.g. a corrected URL
-- parse) are pruned by the watch indexer from now on; drop existing ones.
DELETE FROM upstream_sources
WHERE NOT EXISTS (
    SELECT 1 FROM upstream_index i
    WHERE i.upstream_type = upstream_sources.upstream_type
      AND i.upstream_key = upstream_sources.upstream_key
      AND COALESCE(i.instance, '') = upstream_sources.instance
);
