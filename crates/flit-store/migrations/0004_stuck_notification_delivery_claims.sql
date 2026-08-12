CREATE TABLE stuck_notification_delivery_claims (
  run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
  run_version INTEGER NOT NULL CHECK (run_version > 0),
  occurrence_id TEXT NOT NULL UNIQUE CHECK (length(occurrence_id) BETWEEN 1 AND 256),
  platform_id TEXT NOT NULL UNIQUE CHECK (length(platform_id) BETWEEN 1 AND 256),
  claimed_at TEXT NOT NULL CHECK (length(claimed_at) BETWEEN 1 AND 64)
) STRICT;
