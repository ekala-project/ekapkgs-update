-- Cached OSV records are only refetched when OSV reports a new `modified`, so
-- a change in how records are rated needs the affected ones dropped from the
-- cache; the next scan refetches and rescores them, and each package's rows
-- follow at its next scan.
--
-- CVSS v4.0 vectors are now scored (`cve::cvss4`): records cached without a
-- CVSS score (rated from a label, or `unknown`, possibly v4-only) may change.
-- A CVSS score of 0.0 now takes the record's label rating instead of
-- `unknown`. Records rated from a CVSS v3 score are unaffected.
DELETE FROM osv_vulnerabilities WHERE cvss_vector IS NULL OR severity = 'unknown';
