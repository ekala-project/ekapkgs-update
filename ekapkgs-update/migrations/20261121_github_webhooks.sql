-- GitHub webhook deliveries received by `ekapkgs-update watch
-- --webhook-listen`. GitHub keeps the delivery id when a delivery is
-- redelivered, so a delivery is processed at most once. Pruned after 7 days.
CREATE TABLE IF NOT EXISTS webhook_deliveries (
    delivery_id TEXT PRIMARY KEY,     -- X-GitHub-Delivery
    event TEXT NOT NULL,              -- X-GitHub-Event ('push', 'pull_request', ...)
    action TEXT,                      -- payload `action`, when it has one
    repository TEXT,                  -- 'owner/name' the event is about
    received_at TEXT NOT NULL,
    outcome TEXT                      -- what watch did with it (one line)
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_received ON webhook_deliveries(received_at);

-- Closed-PR events are matched to packages by PR number.
CREATE INDEX IF NOT EXISTS idx_updates_pr ON updates(repo_name, pr_number);
