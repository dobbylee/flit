CREATE TABLE notification_deliveries (
  notification_id TEXT PRIMARY KEY CHECK (length(notification_id) BETWEEN 1 AND 96),
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('permission', 'question', 'failure', 'completion', 'stuck')),
  item_id TEXT NOT NULL CHECK (length(item_id) BETWEEN 1 AND 256),
  item_version INTEGER NOT NULL CHECK (item_version > 0),
  platform_id TEXT NOT NULL UNIQUE CHECK (length(platform_id) BETWEEN 1 AND 256),
  state TEXT NOT NULL CHECK (state IN ('suppressed', 'claimed', 'delivered')),
  suppression_reason TEXT CHECK (suppression_reason IS NULL OR suppression_reason IN ('policy', 'quiet_hours')),
  suppressed_at TEXT CHECK (suppressed_at IS NULL OR length(suppressed_at) BETWEEN 1 AND 64),
  claimed_at TEXT CHECK (claimed_at IS NULL OR length(claimed_at) BETWEEN 1 AND 64),
  delivered_at TEXT CHECK (delivered_at IS NULL OR length(delivered_at) BETWEEN 1 AND 64),
  CHECK (
    (state = 'suppressed' AND suppression_reason IS NOT NULL AND suppressed_at IS NOT NULL AND claimed_at IS NULL AND delivered_at IS NULL)
    OR (state = 'claimed' AND claimed_at IS NOT NULL AND delivered_at IS NULL)
    OR (state = 'delivered' AND claimed_at IS NOT NULL AND delivered_at IS NOT NULL)
  ),
  UNIQUE(run_id, kind, item_id)
) STRICT;

CREATE INDEX notification_deliveries_project_state_idx
  ON notification_deliveries(project_id, state, kind, notification_id);
