CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  checksum TEXT NOT NULL,
  applied_at TEXT NOT NULL
) STRICT;

CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  canonical_path TEXT NOT NULL UNIQUE,
  filesystem_id TEXT,
  trusted INTEGER NOT NULL CHECK (trusted IN (0, 1)),
  default_provider TEXT,
  default_command_json TEXT,
  notification_policy_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT
) STRICT;

CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  title TEXT NOT NULL,
  goal TEXT,
  provider_kind TEXT NOT NULL,
  start_request_json TEXT NOT NULL,
  baseline_head TEXT,
  created_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT,
  deleted_at TEXT
) STRICT;

CREATE TABLE agent_sessions (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL CHECK (ordinal > 0),
  provider_kind TEXT NOT NULL,
  external_session_key TEXT NOT NULL,
  session_fingerprint TEXT NOT NULL,
  executable_path TEXT,
  executable_version TEXT,
  cwd TEXT NOT NULL,
  pid INTEGER,
  capabilities_json TEXT NOT NULL,
  provider_cursor TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  end_reason TEXT,
  UNIQUE (run_id, ordinal)
) STRICT;

CREATE UNIQUE INDEX one_live_session_per_run
ON agent_sessions(run_id)
WHERE ended_at IS NULL;

CREATE TABLE events (
  ingest_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  protocol_version TEXT NOT NULL,
  event_type TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  session_id TEXT REFERENCES agent_sessions(id) ON DELETE CASCADE,
  stream_seq INTEGER,
  occurred_at TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  source_json TEXT NOT NULL,
  confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
  payload_version INTEGER NOT NULL DEFAULT 1,
  payload_json TEXT NOT NULL,
  extensions_json TEXT NOT NULL DEFAULT '{}',
  UNIQUE (session_id, stream_seq)
) STRICT;

CREATE INDEX events_by_run_seq ON events(run_id, ingest_seq);
CREATE INDEX events_by_type_time ON events(event_type, observed_at);

CREATE TABLE evidence (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  session_id TEXT REFERENCES agent_sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  locator_json TEXT NOT NULL,
  content_sha256 TEXT,
  content_size INTEGER,
  created_at TEXT NOT NULL,
  content_deleted_at TEXT
) STRICT;

CREATE TABLE event_evidence (
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  evidence_id TEXT NOT NULL REFERENCES evidence(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  PRIMARY KEY (event_id, evidence_id)
) STRICT;

CREATE TABLE artifacts (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  media_type TEXT NOT NULL,
  display_name TEXT NOT NULL,
  file_path TEXT NOT NULL,
  byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
  sha256 TEXT NOT NULL,
  created_at TEXT NOT NULL,
  content_deleted_at TEXT
) STRICT;

CREATE TABLE permission_rules (
  id TEXT PRIMARY KEY,
  effect TEXT NOT NULL CHECK (effect IN ('allow', 'deny')),
  scope TEXT NOT NULL CHECK (scope IN ('session', 'project', 'global_deny')),
  project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
  session_id TEXT REFERENCES agent_sessions(id) ON DELETE CASCADE,
  provider_kind TEXT NOT NULL,
  matcher_json TEXT NOT NULL,
  created_from_request_id TEXT NOT NULL,
  enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  disabled_at TEXT
) STRICT;

CREATE TABLE run_snapshots (
  run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  lifecycle TEXT NOT NULL,
  activity TEXT NOT NULL,
  activity_confidence REAL NOT NULL,
  attention_level TEXT NOT NULL,
  dashboard_bucket TEXT NOT NULL,
  last_progress_at TEXT,
  last_liveness_at TEXT,
  snapshot_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE INDEX snapshots_by_bucket_progress
ON run_snapshots(dashboard_bucket, last_progress_at DESC);

CREATE TABLE attention_items (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  source_event_id TEXT NOT NULL REFERENCES events(event_id),
  category TEXT NOT NULL,
  severity TEXT NOT NULL,
  blocking INTEGER NOT NULL CHECK (blocking IN (0, 1)),
  status TEXT NOT NULL CHECK (status IN ('open', 'response_pending', 'delivery_unknown', 'resolved', 'acknowledged', 'expired')),
  dedupe_key TEXT NOT NULL,
  version INTEGER NOT NULL,
  item_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  resolved_at TEXT
) STRICT;

CREATE UNIQUE INDEX one_open_attention_per_key
ON attention_items(run_id, dedupe_key)
WHERE status IN ('open', 'response_pending');

CREATE TABLE app_settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;
