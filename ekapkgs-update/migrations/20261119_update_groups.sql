-- Update groups: packages that must be updated together (e.g. the aws-c-*
-- libraries and aws-crt-cpp) are proposed as one PR with one commit per
-- member.
--
-- A package joins a group by declaring `passthru.ekapkgs-update.parent =
-- "<attr path>";`. Parents are resolved relative to the child's scope first
-- (`python3Packages.foo` with parent `bar` means `python3Packages.bar` when
-- that exists), and parent chains are followed to the group's root.
ALTER TABLE updates ADD COLUMN group_parent TEXT; -- declared parent, as resolved to an attr path; NULL = none declared
ALTER TABLE updates ADD COLUMN group_root TEXT;   -- root attr path of the package's group (the root itself included); NULL = ungrouped

CREATE INDEX IF NOT EXISTS idx_updates_group ON updates(repo_name, group_root)
    WHERE group_root IS NOT NULL;
