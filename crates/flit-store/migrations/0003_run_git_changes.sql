CREATE TABLE run_git_change_sets (
  run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
  terminal_event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id) ON DELETE CASCADE,
  attribution TEXT NOT NULL CHECK (attribution IN ('exact', 'observed_during_run')),
  baseline_head TEXT,
  terminal_head TEXT,
  project_filesystem_id TEXT NOT NULL,
  repository_root BLOB NOT NULL CHECK (length(repository_root) BETWEEN 1 AND 16384),
  repository_root_filesystem_id TEXT NOT NULL,
  git_directory BLOB NOT NULL CHECK (length(git_directory) BETWEEN 1 AND 16384),
  git_directory_filesystem_id TEXT NOT NULL,
  common_directory BLOB NOT NULL CHECK (length(common_directory) BETWEEN 1 AND 16384),
  common_directory_filesystem_id TEXT NOT NULL,
  file_count INTEGER NOT NULL CHECK (file_count >= 0),
  insertions INTEGER CHECK (insertions IS NULL OR insertions >= 0),
  deletions INTEGER CHECK (deletions IS NULL OR deletions >= 0),
  CHECK ((insertions IS NULL) = (deletions IS NULL))
) STRICT;

CREATE TABLE run_git_file_changes (
  run_id TEXT NOT NULL REFERENCES run_git_change_sets(run_id) ON DELETE CASCADE,
  change_id TEXT NOT NULL,
  raw_path BLOB NOT NULL CHECK (length(raw_path) BETWEEN 1 AND 16384),
  display_path TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('added', 'modified', 'deleted', 'type_changed', 'untracked')),
  committed INTEGER NOT NULL CHECK (committed IN (0, 1)),
  staged INTEGER NOT NULL CHECK (staged IN (0, 1)),
  unstaged INTEGER NOT NULL CHECK (unstaged IN (0, 1)),
  binary INTEGER NOT NULL CHECK (binary IN (0, 1)),
  insertions INTEGER CHECK (insertions IS NULL OR insertions >= 0),
  deletions INTEGER CHECK (deletions IS NULL OR deletions >= 0),
  project_scope TEXT NOT NULL CHECK (project_scope IN ('inside_project', 'outside_project')),
  PRIMARY KEY (run_id, change_id),
  UNIQUE (run_id, raw_path),
  CHECK (committed = 1 OR staged = 1 OR unstaged = 1),
  CHECK ((insertions IS NULL) = (deletions IS NULL)),
  CHECK (binary = 0 OR insertions IS NULL)
) STRICT;

CREATE INDEX run_git_file_changes_by_path
ON run_git_file_changes(run_id, raw_path, change_id);
