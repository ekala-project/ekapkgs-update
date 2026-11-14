-- nix-eval-jobs derivation path recorded by the watch indexer. Watch
-- dispatches carry it so failure logs (update_logs.drv_path) are keyed by the
-- derivation instead of the synthetic '<repo>:<attr_path>@<version>' key.
-- (pname / semver_strategy / version_regex / include_prereleases were added
-- by 20261111_dashboard_data.)
ALTER TABLE upstream_index ADD COLUMN drv_path TEXT;
