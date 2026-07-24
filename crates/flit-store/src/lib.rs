use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use flit_protocol::{
    EventEnvelope, EventProtocolVersion, EventSource, EventSourceKind, MAX_JSON_SAFE_INTEGER,
    NullableSessionId, UnsequencedEventEnvelope,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

mod projects;
mod writer;

pub use projects::{
    Project, ProjectDirectoryInspection, ProjectIdentity, ProjectInspectionError,
    ProjectRegistration, ProjectRegistrationOutcome, ProjectTrustConfirmation, ProjectTrustOutcome,
};

pub use writer::{
    CheckpointAck, CheckpointFailure, CheckpointReceipt, DurableEventAck,
    EVENT_WRITER_QUEUE_CAPACITY, EVENT_WRITER_THREAD_NAME, EventCommitPriority, EventWriteFailure,
    EventWriteReceipt, EventWriter, EventWriterHandle, EventWriterShutdownError,
    EventWriterStartError, NORMAL_EVENT_BATCH_WAIT, event_commit_priority,
};

const INITIAL_MIGRATION_VERSION: i64 = 1;
const INITIAL_MIGRATION_NAME: &str = "initial";
const INITIAL_MIGRATION_SQL: &str = include_str!("../migrations/0001_initial.sql");
const PROJECT_FILESYSTEM_IDENTITY_MIGRATION_VERSION: i64 = 2;
const PROJECT_FILESYSTEM_IDENTITY_MIGRATION_NAME: &str = "project_filesystem_identity";
const PROJECT_FILESYSTEM_IDENTITY_MIGRATION_SQL: &str =
    include_str!("../migrations/0002_project_filesystem_identity.sql");
const MAX_EVENT_READ_LIMIT: usize = 1_000;
pub const MAX_EVENT_APPEND_BATCH: usize = 50;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionPolicy {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub synchronous: i64,
    pub busy_timeout_ms: i64,
    pub temp_store: i64,
    pub wal_autocheckpoint_pages: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointReport {
    pub busy: i64,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

pub struct Store {
    connection: Connection,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppendEventOutcome {
    Inserted(EventEnvelope),
    Duplicate(EventEnvelope),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunSnapshotDraft {
    pub run_id: String,
    pub version: u64,
    pub lifecycle: String,
    pub activity: String,
    pub activity_confidence: f64,
    pub attention_level: String,
    pub dashboard_bucket: String,
    pub last_progress_at: Option<String>,
    pub last_liveness_at: Option<String>,
    pub snapshot: Map<String, Value>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunSnapshot {
    pub run_id: String,
    pub version: u64,
    pub lifecycle: String,
    pub activity: String,
    pub activity_confidence: f64,
    pub attention_level: String,
    pub dashboard_bucket: String,
    pub last_progress_at: Option<String>,
    pub last_liveness_at: Option<String>,
    pub snapshot: Map<String, Value>,
    pub updated_at: String,
}

impl From<RunSnapshotDraft> for RunSnapshot {
    fn from(snapshot: RunSnapshotDraft) -> Self {
        Self {
            run_id: snapshot.run_id,
            version: snapshot.version,
            lifecycle: snapshot.lifecycle,
            activity: snapshot.activity,
            activity_confidence: snapshot.activity_confidence,
            attention_level: snapshot.attention_level,
            dashboard_bucket: snapshot.dashboard_bucket,
            last_progress_at: snapshot.last_progress_at,
            last_liveness_at: snapshot.last_liveness_at,
            snapshot: snapshot.snapshot,
            updated_at: snapshot.updated_at,
        }
    }
}

impl From<RunSnapshot> for RunSnapshotDraft {
    fn from(snapshot: RunSnapshot) -> Self {
        Self {
            run_id: snapshot.run_id,
            version: snapshot.version,
            lifecycle: snapshot.lifecycle,
            activity: snapshot.activity,
            activity_confidence: snapshot.activity_confidence,
            attention_level: snapshot.attention_level,
            dashboard_bucket: snapshot.dashboard_bucket,
            last_progress_at: snapshot.last_progress_at,
            last_liveness_at: snapshot.last_liveness_at,
            snapshot: snapshot.snapshot,
            updated_at: snapshot.updated_at,
        }
    }
}

fn validate_project_registration(registration: &ProjectRegistration) -> Result<(), StoreError> {
    if registration.id.trim().is_empty() {
        return Err(StoreError::InvalidProjectRegistration { field: "id" });
    }
    if registration.display_name.trim().is_empty() {
        return Err(StoreError::InvalidProjectRegistration {
            field: "display_name",
        });
    }
    if registration.created_at.trim().is_empty() {
        return Err(StoreError::InvalidProjectRegistration {
            field: "created_at",
        });
    }
    if registration.selected_path.as_os_str().is_empty() {
        return Err(StoreError::InvalidProjectRegistration {
            field: "selected_path",
        });
    }
    Ok(())
}

fn validate_project_trust_confirmation(
    confirmation: &ProjectTrustConfirmation,
) -> Result<(), StoreError> {
    if confirmation.project_id.trim().is_empty() {
        return Err(StoreError::InvalidProjectTrustConfirmation {
            field: "project_id",
        });
    }
    if confirmation.selected_path.as_os_str().is_empty() {
        return Err(StoreError::InvalidProjectTrustConfirmation {
            field: "selected_path",
        });
    }
    if confirmation.confirmed_at.trim().is_empty() {
        return Err(StoreError::InvalidProjectTrustConfirmation {
            field: "confirmed_at",
        });
    }
    Ok(())
}

fn transaction_project_id_for_canonical_path(
    transaction: &Transaction<'_>,
    canonical_path: &Path,
) -> Result<Option<String>, StoreError> {
    transaction
        .query_row(
            "SELECT id FROM projects WHERE canonical_path = ?1",
            [canonical_path
                .to_str()
                .ok_or(StoreError::InvalidProjectRegistration {
                    field: "canonical_path",
                })?],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

fn transaction_project_id_for_filesystem_id(
    transaction: &Transaction<'_>,
    filesystem_id: &str,
) -> Result<Option<String>, StoreError> {
    transaction
        .query_row(
            "SELECT id FROM projects WHERE filesystem_id = ?1",
            [filesystem_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

fn transaction_project_exists(
    transaction: &Transaction<'_>,
    project_id: &str,
) -> Result<bool, StoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [project_id],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)
}

fn project_by_id(connection: &Connection, project_id: &str) -> Result<Option<Project>, StoreError> {
    connection
        .query_row(
            "SELECT id, display_name, canonical_path, filesystem_id, trusted, default_provider, notification_policy_json, created_at, updated_at FROM projects WHERE id = ?1",
            [project_id],
            |row| {
                let trusted: i64 = row.get(4)?;
                Ok(Project {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    canonical_path: PathBuf::from(row.get::<_, String>(2)?),
                    filesystem_id: row.get(3)?,
                    trusted: trusted == 1,
                    default_provider: row.get(5)?,
                    notification_policy_json: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

#[derive(Clone, Debug, PartialEq)]
pub enum WriteRunSnapshotOutcome {
    Inserted(RunSnapshot),
    Replaced(RunSnapshot),
    Duplicate(RunSnapshot),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunEventPage {
    pub upper_bound: u64,
    pub events: Vec<EventEnvelope>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>, migration_applied_at: &str) -> Result<Self, StoreError> {
        if migration_applied_at.trim().is_empty() {
            return Err(StoreError::InvalidMigrationAppliedAt);
        }

        let mut connection = Connection::open(path).map_err(StoreError::Sqlite)?;
        let needs_bootstrap = preflight_database(&connection)?;
        configure_connection(&connection)?;
        if needs_bootstrap {
            apply_pending_migrations(&mut connection, migration_applied_at, 0)?;
        } else {
            let applied_count = applied_migration_count(&connection)?;
            apply_pending_migrations(&mut connection, migration_applied_at, applied_count)?;
        }
        validate_schema(&connection)?;
        validate_integrity(&connection)?;
        validate_connection_policy(&connection)?;
        Ok(Self { connection })
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .map_err(StoreError::Sqlite)
    }

    pub fn connection_policy(&self) -> Result<ConnectionPolicy, StoreError> {
        Ok(ConnectionPolicy {
            foreign_keys: pragma_i64(&self.connection, "foreign_keys")? == 1,
            journal_mode: pragma_string(&self.connection, "journal_mode")?,
            synchronous: pragma_i64(&self.connection, "synchronous")?,
            busy_timeout_ms: pragma_i64(&self.connection, "busy_timeout")?,
            temp_store: pragma_i64(&self.connection, "temp_store")?,
            wal_autocheckpoint_pages: pragma_i64(&self.connection, "wal_autocheckpoint")?,
        })
    }

    pub fn quick_check(&self) -> Result<String, StoreError> {
        pragma_string(&self.connection, "quick_check")
    }

    pub fn passive_checkpoint(&mut self) -> Result<CheckpointReport, StoreError> {
        let (busy, log_frames, checkpointed_frames) = self
            .connection
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(StoreError::Sqlite)?;
        Self::validated_checkpoint_report(busy, log_frames, checkpointed_frames)
    }

    pub fn register_project(
        &mut self,
        registration: ProjectRegistration,
    ) -> Result<ProjectRegistrationOutcome, StoreError> {
        validate_project_registration(&registration)?;
        let inspection = ProjectDirectoryInspection::inspect(&registration.selected_path)
            .map_err(StoreError::ProjectInspection)?;
        let identity = inspection.identity;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::Sqlite)?;
        if let Some(existing_project_id) =
            transaction_project_id_for_canonical_path(&transaction, &identity.canonical_path)?
        {
            return Ok(ProjectRegistrationOutcome::DuplicateCanonicalPath {
                existing_project_id,
            });
        }
        if let Some(existing_project_id) =
            transaction_project_id_for_filesystem_id(&transaction, &identity.filesystem_id)?
        {
            return Ok(ProjectRegistrationOutcome::DuplicateFilesystemIdentity {
                existing_project_id,
            });
        }
        if transaction_project_exists(&transaction, &registration.id)? {
            return Err(StoreError::ProjectIdConflict {
                project_id: registration.id,
            });
        }

        transaction
            .execute(
                "INSERT INTO projects(id, display_name, canonical_path, filesystem_id, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, ?2, ?3, ?4, 0, '{}', ?5, ?5)",
                params![
                    registration.id,
                    registration.display_name,
                    identity
                        .canonical_path
                        .to_str()
                        .ok_or(StoreError::InvalidProjectRegistration {
                            field: "canonical_path",
                        })?,
                    identity.filesystem_id,
                    registration.created_at,
                ],
            )
            .map_err(StoreError::Sqlite)?;
        transaction.commit().map_err(StoreError::Sqlite)?;
        let project = project_by_id(&self.connection, &registration.id)?.ok_or_else(|| {
            StoreError::MissingProject {
                project_id: registration.id,
            }
        })?;
        Ok(ProjectRegistrationOutcome::Registered(project))
    }

    pub fn project(&self, project_id: &str) -> Result<Option<Project>, StoreError> {
        project_by_id(&self.connection, project_id)
    }

    pub fn confirm_project_trust(
        &mut self,
        confirmation: ProjectTrustConfirmation,
    ) -> Result<ProjectTrustOutcome, StoreError> {
        validate_project_trust_confirmation(&confirmation)?;
        let inspection = ProjectDirectoryInspection::inspect(&confirmation.selected_path)
            .map_err(StoreError::ProjectInspection)?;
        let canonical_path = inspection.identity.canonical_path.to_str().ok_or(
            StoreError::InvalidProjectTrustConfirmation {
                field: "canonical_path",
            },
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::Sqlite)?;
        let stored = transaction
            .query_row(
                "SELECT canonical_path, filesystem_id, trusted FROM projects WHERE id = ?1",
                [&confirmation.project_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StoreError::Sqlite)?
            .ok_or_else(|| StoreError::MissingProject {
                project_id: confirmation.project_id.clone(),
            })?;
        let filesystem_id =
            stored
                .1
                .ok_or_else(|| StoreError::ProjectFilesystemIdentityUnavailable {
                    project_id: confirmation.project_id.clone(),
                })?;
        if stored.0 != canonical_path || filesystem_id != inspection.identity.filesystem_id {
            return Err(StoreError::ProjectIdentityMismatch {
                project_id: confirmation.project_id,
            });
        }
        if stored.2 == 1 {
            drop(transaction);
            let project =
                project_by_id(&self.connection, &confirmation.project_id)?.ok_or_else(|| {
                    StoreError::MissingProject {
                        project_id: confirmation.project_id,
                    }
                })?;
            return Ok(ProjectTrustOutcome::AlreadyTrusted(project));
        }
        transaction
            .execute(
                "UPDATE projects SET trusted = 1, updated_at = ?1 WHERE id = ?2",
                params![confirmation.confirmed_at, confirmation.project_id],
            )
            .map_err(StoreError::Sqlite)?;
        transaction.commit().map_err(StoreError::Sqlite)?;
        let project =
            project_by_id(&self.connection, &confirmation.project_id)?.ok_or_else(|| {
                StoreError::MissingProject {
                    project_id: confirmation.project_id,
                }
            })?;
        Ok(ProjectTrustOutcome::Trusted(project))
    }

    fn validated_checkpoint_report(
        busy: i64,
        log_frames: i64,
        checkpointed_frames: i64,
    ) -> Result<CheckpointReport, StoreError> {
        let report = CheckpointReport {
            busy,
            log_frames,
            checkpointed_frames,
        };
        if report.busy < 0
            || report.log_frames < 0
            || report.checkpointed_frames < 0
            || report.checkpointed_frames > report.log_frames
        {
            return Err(StoreError::InvalidCheckpointReport(report));
        }
        Ok(report)
    }

    pub fn append_event(
        &mut self,
        event: UnsequencedEventEnvelope,
    ) -> Result<AppendEventOutcome, StoreError> {
        validate_event(&event)?;
        let mut outcomes = self.append_event_batch(vec![event])?;
        Ok(outcomes
            .pop()
            .expect("one event input must produce one append outcome"))
    }

    pub fn append_event_batch(
        &mut self,
        events: Vec<UnsequencedEventEnvelope>,
    ) -> Result<Vec<AppendEventOutcome>, StoreError> {
        if !(1..=MAX_EVENT_APPEND_BATCH).contains(&events.len()) {
            return Err(StoreError::InvalidEventBatchSize {
                count: events.len(),
                max: MAX_EVENT_APPEND_BATCH,
            });
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::Sqlite)?;
        let mut outcomes = Vec::with_capacity(events.len());

        for event in events {
            validate_event(&event)?;
            if let Some(ingest_seq) = event_ingest_seq(&transaction, &event.event_id)? {
                let existing = load_event(&transaction, ingest_seq)?;
                if UnsequencedEventEnvelope::from(existing.clone()) == event {
                    outcomes.push(AppendEventOutcome::Duplicate(existing));
                    continue;
                }
                return Err(StoreError::EventIdentityConflict {
                    event_id: event.event_id,
                });
            }

            if let NullableSessionId::Id(session_id) = &event.session_id
                && let Some(existing_event_id) =
                    event_id_for_stream(&transaction, session_id, event.stream_seq)?
            {
                return Err(StoreError::StreamSequenceConflict {
                    session_id: session_id.clone(),
                    stream_seq: event.stream_seq,
                    existing_event_id,
                });
            }

            validate_event_session(&transaction, &event)?;
            validate_event_evidence(&transaction, &event)?;
            let source_json = serde_json::to_string(&event.source).map_err(StoreError::Json)?;
            let payload_json = serde_json::to_string(&event.payload).map_err(StoreError::Json)?;
            let extensions_json =
                serde_json::to_string(&event.extensions).map_err(StoreError::Json)?;
            let session_id = match &event.session_id {
                NullableSessionId::Id(session_id) => Some(session_id.as_str()),
                NullableSessionId::Null => None,
            };
            transaction
                .execute(
                    "INSERT INTO events(event_id, protocol_version, event_type, run_id, session_id, stream_seq, occurred_at, observed_at, source_json, confidence, payload_version, payload_json, extensions_json) VALUES(?1, '1.0', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11)",
                    params![
                        event.event_id,
                        event.event_type,
                        event.run_id,
                        session_id,
                        event.stream_seq as i64,
                        event.occurred_at,
                        event.observed_at,
                        source_json,
                        event.confidence,
                        payload_json,
                        extensions_json,
                    ],
                )
                .map_err(StoreError::Sqlite)?;
            let ingest_seq = assigned_sequence(transaction.last_insert_rowid())?;
            for (ordinal, evidence_id) in event.evidence_ids.iter().enumerate() {
                transaction
                    .execute(
                        "INSERT INTO event_evidence(event_id, evidence_id, ordinal) VALUES(?1, ?2, ?3)",
                        params![event.event_id, evidence_id, ordinal as i64],
                    )
                    .map_err(StoreError::Sqlite)?;
            }
            outcomes.push(AppendEventOutcome::Inserted(
                event.with_ingest_seq(ingest_seq),
            ));
        }
        transaction.commit().map_err(StoreError::Sqlite)?;
        Ok(outcomes)
    }

    pub fn events_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, StoreError> {
        if cursor > MAX_JSON_SAFE_INTEGER || !(1..=MAX_EVENT_READ_LIMIT).contains(&limit) {
            return Err(StoreError::InvalidEventReadRange { cursor, limit });
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT ingest_seq FROM events WHERE ingest_seq > ?1 ORDER BY ingest_seq LIMIT ?2",
            )
            .map_err(StoreError::Sqlite)?;
        let ingest_sequences = statement
            .query_map(params![cursor as i64, limit as i64], |row| row.get(0))
            .map_err(StoreError::Sqlite)?
            .collect::<Result<Vec<i64>, _>>()
            .map_err(StoreError::Sqlite)?;
        drop(statement);
        ingest_sequences
            .into_iter()
            .map(|ingest_seq| load_event(&self.connection, ingest_seq))
            .collect()
    }

    pub fn write_run_snapshot(
        &mut self,
        draft: RunSnapshotDraft,
    ) -> Result<WriteRunSnapshotOutcome, StoreError> {
        validate_snapshot(&draft)?;
        let snapshot_json = serde_json::to_string(&draft.snapshot).map_err(StoreError::Json)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::Sqlite)?;
        validate_snapshot_version(&transaction, &draft.run_id, draft.version)?;

        let existing = load_run_snapshot(&transaction, &draft.run_id)?;
        if let Some(existing) = existing {
            if draft.version < existing.version {
                return Err(StoreError::StaleRunSnapshot {
                    run_id: draft.run_id,
                    stored_version: existing.version,
                    received_version: draft.version,
                });
            }
            if draft.version == existing.version {
                if RunSnapshotDraft::from(existing.clone()) == draft {
                    return Ok(WriteRunSnapshotOutcome::Duplicate(existing));
                }
                return Err(StoreError::RunSnapshotConflict {
                    run_id: draft.run_id,
                    version: draft.version,
                });
            }
            let changed = transaction
                .execute(
                    "UPDATE run_snapshots SET version = ?2, lifecycle = ?3, activity = ?4, activity_confidence = ?5, attention_level = ?6, dashboard_bucket = ?7, last_progress_at = ?8, last_liveness_at = ?9, snapshot_json = ?10, updated_at = ?11 WHERE run_id = ?1 AND version = ?12",
                    params![
                        draft.run_id,
                        draft.version as i64,
                        draft.lifecycle,
                        draft.activity,
                        draft.activity_confidence,
                        draft.attention_level,
                        draft.dashboard_bucket,
                        draft.last_progress_at,
                        draft.last_liveness_at,
                        snapshot_json,
                        draft.updated_at,
                        existing.version as i64,
                    ],
                )
                .map_err(StoreError::Sqlite)?;
            if changed != 1 {
                return Err(StoreError::RunSnapshotConcurrentChange {
                    run_id: draft.run_id,
                });
            }
            let snapshot = RunSnapshot::from(draft);
            transaction.commit().map_err(StoreError::Sqlite)?;
            return Ok(WriteRunSnapshotOutcome::Replaced(snapshot));
        }

        transaction
            .execute(
                "INSERT INTO run_snapshots(run_id, version, lifecycle, activity, activity_confidence, attention_level, dashboard_bucket, last_progress_at, last_liveness_at, snapshot_json, updated_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    draft.run_id,
                    draft.version as i64,
                    draft.lifecycle,
                    draft.activity,
                    draft.activity_confidence,
                    draft.attention_level,
                    draft.dashboard_bucket,
                    draft.last_progress_at,
                    draft.last_liveness_at,
                    snapshot_json,
                    draft.updated_at,
                ],
            )
            .map_err(StoreError::Sqlite)?;
        let snapshot = RunSnapshot::from(draft);
        transaction.commit().map_err(StoreError::Sqlite)?;
        Ok(WriteRunSnapshotOutcome::Inserted(snapshot))
    }

    pub fn run_snapshot(&self, run_id: &str) -> Result<Option<RunSnapshot>, StoreError> {
        if run_id.trim().is_empty() {
            return Err(StoreError::InvalidRunSnapshot { field: "run_id" });
        }
        load_run_snapshot(&self.connection, run_id)
    }

    pub fn latest_ingest_seq(&self) -> Result<u64, StoreError> {
        let latest = self
            .connection
            .query_row("SELECT MAX(ingest_seq) FROM events", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(StoreError::Sqlite)?;
        latest.map_or(Ok(0), assigned_sequence)
    }

    pub fn run_events_through(
        &self,
        run_id: &str,
        cursor: u64,
        upper_bound: u64,
        limit: usize,
    ) -> Result<RunEventPage, StoreError> {
        if run_id.trim().is_empty()
            || cursor > upper_bound
            || upper_bound > MAX_JSON_SAFE_INTEGER
            || !(1..=MAX_EVENT_READ_LIMIT).contains(&limit)
        {
            return Err(StoreError::InvalidRunEventRange {
                cursor,
                upper_bound,
                limit,
            });
        }
        if !run_exists(&self.connection, run_id)? {
            return Err(StoreError::MissingRun {
                run_id: run_id.to_owned(),
            });
        }
        let latest = self.latest_ingest_seq()?;
        if upper_bound > latest {
            return Err(StoreError::InvalidRunEventRange {
                cursor,
                upper_bound,
                limit,
            });
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT ingest_seq FROM events WHERE run_id = ?1 AND ingest_seq > ?2 AND ingest_seq <= ?3 ORDER BY ingest_seq LIMIT ?4",
            )
            .map_err(StoreError::Sqlite)?;
        let ingest_sequences = statement
            .query_map(
                params![run_id, cursor as i64, upper_bound as i64, limit as i64],
                |row| row.get(0),
            )
            .map_err(StoreError::Sqlite)?
            .collect::<Result<Vec<i64>, _>>()
            .map_err(StoreError::Sqlite)?;
        drop(statement);
        let events = ingest_sequences
            .into_iter()
            .map(|ingest_seq| load_event(&self.connection, ingest_seq))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RunEventPage {
            upper_bound,
            events,
        })
    }
}

fn validate_snapshot(snapshot: &RunSnapshotDraft) -> Result<(), StoreError> {
    for (field, value) in [
        ("run_id", snapshot.run_id.as_str()),
        ("lifecycle", snapshot.lifecycle.as_str()),
        ("activity", snapshot.activity.as_str()),
        ("attention_level", snapshot.attention_level.as_str()),
        ("dashboard_bucket", snapshot.dashboard_bucket.as_str()),
        ("updated_at", snapshot.updated_at.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::InvalidRunSnapshot { field });
        }
    }
    if snapshot.version == 0 || snapshot.version > MAX_JSON_SAFE_INTEGER {
        return Err(StoreError::InvalidRunSnapshot { field: "version" });
    }
    if !snapshot.activity_confidence.is_finite()
        || !(0.0..=1.0).contains(&snapshot.activity_confidence)
    {
        return Err(StoreError::InvalidRunSnapshot {
            field: "activity_confidence",
        });
    }
    for (field, value) in [
        ("last_progress_at", snapshot.last_progress_at.as_deref()),
        ("last_liveness_at", snapshot.last_liveness_at.as_deref()),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(StoreError::InvalidRunSnapshot { field });
        }
    }
    validate_snapshot_json(snapshot)
}

fn validate_snapshot_json(snapshot: &RunSnapshotDraft) -> Result<(), StoreError> {
    let string_matches = |field: &'static str, expected: &str| {
        if snapshot.snapshot.get(field).and_then(Value::as_str) == Some(expected) {
            Ok(())
        } else {
            Err(StoreError::InvalidRunSnapshot { field })
        }
    };
    string_matches("run_id", &snapshot.run_id)?;
    if snapshot.snapshot.get("version").and_then(Value::as_u64) != Some(snapshot.version) {
        return Err(StoreError::InvalidRunSnapshot { field: "version" });
    }
    string_matches("lifecycle", &snapshot.lifecycle)?;
    let activity = snapshot
        .snapshot
        .get("activity")
        .and_then(Value::as_object)
        .ok_or(StoreError::InvalidRunSnapshot { field: "activity" })?;
    if activity.get("kind").and_then(Value::as_str) != Some(snapshot.activity.as_str()) {
        return Err(StoreError::InvalidRunSnapshot {
            field: "activity.kind",
        });
    }
    if activity.get("confidence").and_then(Value::as_f64) != Some(snapshot.activity_confidence) {
        return Err(StoreError::InvalidRunSnapshot {
            field: "activity.confidence",
        });
    }
    let attention = snapshot
        .snapshot
        .get("attention")
        .and_then(Value::as_object)
        .ok_or(StoreError::InvalidRunSnapshot { field: "attention" })?;
    if attention.get("level").and_then(Value::as_str) != Some(snapshot.attention_level.as_str()) {
        return Err(StoreError::InvalidRunSnapshot {
            field: "attention.level",
        });
    }
    string_matches("dashboard_bucket", &snapshot.dashboard_bucket)?;
    validate_optional_snapshot_field(
        &snapshot.snapshot,
        "last_progress_at",
        snapshot.last_progress_at.as_deref(),
    )?;
    validate_optional_snapshot_field(
        &snapshot.snapshot,
        "last_liveness_at",
        snapshot.last_liveness_at.as_deref(),
    )
}

fn validate_optional_snapshot_field(
    snapshot: &Map<String, Value>,
    field: &'static str,
    expected: Option<&str>,
) -> Result<(), StoreError> {
    let matches = match (snapshot.get(field), expected) {
        (Some(Value::Null), None) => true,
        (Some(value), Some(expected)) => value.as_str() == Some(expected),
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(StoreError::InvalidRunSnapshot { field })
    }
}

fn validate_snapshot_version(
    connection: &Connection,
    run_id: &str,
    version: u64,
) -> Result<(), StoreError> {
    if !run_exists(connection, run_id)? {
        return Err(StoreError::MissingRun {
            run_id: run_id.to_owned(),
        });
    }
    let owned = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE run_id = ?1 AND ingest_seq = ?2)",
            params![run_id, version as i64],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StoreError::Sqlite)?;
    if !owned {
        return Err(StoreError::RunSnapshotVersionNotOwned {
            run_id: run_id.to_owned(),
            version,
        });
    }
    Ok(())
}

fn run_exists(connection: &Connection, run_id: &str) -> Result<bool, StoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
            [run_id],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)
}

fn load_run_snapshot(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<RunSnapshot>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT version, lifecycle, activity, activity_confidence, attention_level, dashboard_bucket, last_progress_at, last_liveness_at, snapshot_json, updated_at FROM run_snapshots WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok(StoredRunSnapshot {
                    version: row.get(0)?,
                    lifecycle: row.get(1)?,
                    activity: row.get(2)?,
                    activity_confidence: row.get(3)?,
                    attention_level: row.get(4)?,
                    dashboard_bucket: row.get(5)?,
                    last_progress_at: row.get(6)?,
                    last_liveness_at: row.get(7)?,
                    snapshot_json: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let version =
        assigned_sequence(stored.version).map_err(|_| StoreError::StoredRunSnapshotInvalid {
            run_id: run_id.to_owned(),
            field: "version",
        })?;
    let snapshot =
        serde_json::from_str::<Map<String, Value>>(&stored.snapshot_json).map_err(|source| {
            StoreError::StoredRunSnapshotJson {
                run_id: run_id.to_owned(),
                source,
            }
        })?;
    let record = RunSnapshot {
        run_id: run_id.to_owned(),
        version,
        lifecycle: stored.lifecycle,
        activity: stored.activity,
        activity_confidence: stored.activity_confidence,
        attention_level: stored.attention_level,
        dashboard_bucket: stored.dashboard_bucket,
        last_progress_at: stored.last_progress_at,
        last_liveness_at: stored.last_liveness_at,
        snapshot,
        updated_at: stored.updated_at,
    };
    let draft = RunSnapshotDraft::from(record.clone());
    validate_snapshot(&draft).map_err(|_| StoreError::StoredRunSnapshotInvalid {
        run_id: run_id.to_owned(),
        field: "snapshot",
    })?;
    validate_snapshot_version(connection, run_id, version).map_err(|_| {
        StoreError::StoredRunSnapshotInvalid {
            run_id: run_id.to_owned(),
            field: "version",
        }
    })?;
    Ok(Some(record))
}

struct StoredRunSnapshot {
    version: i64,
    lifecycle: String,
    activity: String,
    activity_confidence: f64,
    attention_level: String,
    dashboard_bucket: String,
    last_progress_at: Option<String>,
    last_liveness_at: Option<String>,
    snapshot_json: String,
    updated_at: String,
}

fn validate_event(event: &UnsequencedEventEnvelope) -> Result<(), StoreError> {
    for (field, value) in [
        ("event_id", event.event_id.as_str()),
        ("run_id", event.run_id.as_str()),
        ("occurred_at", event.occurred_at.as_str()),
        ("observed_at", event.observed_at.as_str()),
        ("type", event.event_type.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::InvalidEvent { field });
        }
    }
    if let NullableSessionId::Id(session_id) = &event.session_id
        && session_id.trim().is_empty()
    {
        return Err(StoreError::InvalidEvent {
            field: "session_id",
        });
    }
    if event.stream_seq == 0 || event.stream_seq > MAX_JSON_SAFE_INTEGER {
        return Err(StoreError::InvalidEvent {
            field: "stream_seq",
        });
    }
    if !event.confidence.is_finite() || !(0.0..=1.0).contains(&event.confidence) {
        return Err(StoreError::InvalidEvent {
            field: "confidence",
        });
    }
    validate_extension_keys(
        &event.extensions,
        &[
            "protocol_version",
            "event_id",
            "run_id",
            "session_id",
            "stream_seq",
            "ingest_seq",
            "occurred_at",
            "observed_at",
            "type",
            "source",
            "confidence",
            "evidence_ids",
            "payload",
        ],
        "extensions",
    )?;
    validate_extension_keys(
        &event.source.extensions,
        &["kind", "provider", "contract_version"],
        "source.extensions",
    )?;

    let mut evidence_ids = BTreeSet::new();
    for evidence_id in &event.evidence_ids {
        if evidence_id.trim().is_empty() || !evidence_ids.insert(evidence_id.as_str()) {
            return Err(StoreError::InvalidEvent {
                field: "evidence_ids",
            });
        }
    }
    if event.source.kind == EventSourceKind::Classifier && event.evidence_ids.is_empty() {
        return Err(StoreError::InvalidEvent {
            field: "evidence_ids",
        });
    }
    Ok(())
}

fn validate_extension_keys(
    extensions: &BTreeMap<String, Value>,
    reserved: &[&str],
    field: &'static str,
) -> Result<(), StoreError> {
    if extensions
        .keys()
        .any(|key| reserved.contains(&key.as_str()))
    {
        return Err(StoreError::InvalidEvent { field });
    }
    Ok(())
}

fn event_ingest_seq(connection: &Connection, event_id: &str) -> Result<Option<i64>, StoreError> {
    connection
        .query_row(
            "SELECT ingest_seq FROM events WHERE event_id = ?1",
            [event_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

fn event_id_for_stream(
    connection: &Connection,
    session_id: &str,
    stream_seq: u64,
) -> Result<Option<String>, StoreError> {
    connection
        .query_row(
            "SELECT event_id FROM events WHERE session_id = ?1 AND stream_seq = ?2",
            params![session_id, stream_seq as i64],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)
}

fn validate_event_evidence(
    connection: &Connection,
    event: &UnsequencedEventEnvelope,
) -> Result<(), StoreError> {
    for evidence_id in &event.evidence_ids {
        let evidence_run_id = connection
            .query_row(
                "SELECT run_id FROM evidence WHERE id = ?1",
                [evidence_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::Sqlite)?;
        let Some(evidence_run_id) = evidence_run_id else {
            return Err(StoreError::MissingEvidence {
                evidence_id: evidence_id.clone(),
            });
        };
        if evidence_run_id != event.run_id {
            return Err(StoreError::EvidenceRunMismatch {
                evidence_id: evidence_id.clone(),
                event_run_id: event.run_id.clone(),
                evidence_run_id,
            });
        }
    }
    Ok(())
}

fn validate_event_session(
    connection: &Connection,
    event: &UnsequencedEventEnvelope,
) -> Result<(), StoreError> {
    let NullableSessionId::Id(session_id) = &event.session_id else {
        return Ok(());
    };
    let session_run_id = connection
        .query_row(
            "SELECT run_id FROM agent_sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)?;
    let Some(session_run_id) = session_run_id else {
        return Err(StoreError::MissingSession {
            session_id: session_id.clone(),
        });
    };
    if session_run_id != event.run_id {
        return Err(StoreError::SessionRunMismatch {
            session_id: session_id.clone(),
            event_run_id: event.run_id.clone(),
            session_run_id,
        });
    }
    Ok(())
}

fn assigned_sequence(value: i64) -> Result<u64, StoreError> {
    let sequence =
        u64::try_from(value).map_err(|_| StoreError::AssignedSequenceOutOfRange(value))?;
    if sequence == 0 || sequence > MAX_JSON_SAFE_INTEGER {
        return Err(StoreError::AssignedSequenceOutOfRange(value));
    }
    Ok(sequence)
}

fn load_event(connection: &Connection, ingest_seq: i64) -> Result<EventEnvelope, StoreError> {
    let stored = connection
        .query_row(
            "SELECT protocol_version, event_id, run_id, session_id, stream_seq, occurred_at, observed_at, event_type, source_json, confidence, payload_version, payload_json, extensions_json FROM events WHERE ingest_seq = ?1",
            [ingest_seq],
            |row| {
                Ok(StoredEvent {
                    protocol_version: row.get(0)?,
                    event_id: row.get(1)?,
                    run_id: row.get(2)?,
                    session_id: row.get(3)?,
                    stream_seq: row.get(4)?,
                    occurred_at: row.get(5)?,
                    observed_at: row.get(6)?,
                    event_type: row.get(7)?,
                    source_json: row.get(8)?,
                    confidence: row.get(9)?,
                    payload_version: row.get(10)?,
                    payload_json: row.get(11)?,
                    extensions_json: row.get(12)?,
                })
            },
        )
        .map_err(StoreError::Sqlite)?;
    let assigned_ingest_seq = assigned_sequence(ingest_seq)?;
    if stored.protocol_version != "1.0" || stored.payload_version != 1 {
        return Err(StoreError::StoredEventInvalid {
            ingest_seq: assigned_ingest_seq,
            field: if stored.protocol_version != "1.0" {
                "protocol_version"
            } else {
                "payload_version"
            },
        });
    }
    let stream_seq = stored
        .stream_seq
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0 && *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(StoreError::StoredEventInvalid {
            ingest_seq: assigned_ingest_seq,
            field: "stream_seq",
        })?;
    let source =
        stored_json::<EventSource>(assigned_ingest_seq, "source_json", &stored.source_json)?;
    let payload = stored_json::<Map<String, Value>>(
        assigned_ingest_seq,
        "payload_json",
        &stored.payload_json,
    )?;
    let extensions = stored_json::<BTreeMap<String, Value>>(
        assigned_ingest_seq,
        "extensions_json",
        &stored.extensions_json,
    )?;
    let evidence_ids = event_evidence_ids(connection, assigned_ingest_seq, &stored.event_id)?;
    let envelope = EventEnvelope {
        protocol_version: EventProtocolVersion::V1_0,
        event_id: stored.event_id,
        run_id: stored.run_id,
        session_id: stored
            .session_id
            .map_or(NullableSessionId::Null, NullableSessionId::Id),
        stream_seq,
        ingest_seq: assigned_ingest_seq,
        occurred_at: stored.occurred_at,
        observed_at: stored.observed_at,
        event_type: stored.event_type,
        source,
        confidence: stored.confidence,
        evidence_ids,
        payload,
        extensions,
    };
    let unsequenced = UnsequencedEventEnvelope::from(envelope.clone());
    validate_event(&unsequenced).map_err(|_| StoreError::StoredEventInvalid {
        ingest_seq: assigned_ingest_seq,
        field: "envelope",
    })?;
    validate_event_session(connection, &unsequenced).map_err(|error| match error {
        StoreError::MissingSession { .. } | StoreError::SessionRunMismatch { .. } => {
            StoreError::StoredEventInvalid {
                ingest_seq: assigned_ingest_seq,
                field: "session_id",
            }
        }
        error => error,
    })?;
    validate_event_evidence(connection, &unsequenced).map_err(|error| match error {
        StoreError::MissingEvidence { .. } | StoreError::EvidenceRunMismatch { .. } => {
            StoreError::StoredEventInvalid {
                ingest_seq: assigned_ingest_seq,
                field: "evidence_ids",
            }
        }
        error => error,
    })?;
    Ok(envelope)
}

fn stored_json<T: serde::de::DeserializeOwned>(
    ingest_seq: u64,
    field: &'static str,
    json: &str,
) -> Result<T, StoreError> {
    serde_json::from_str(json).map_err(|source| StoreError::StoredJson {
        ingest_seq,
        field,
        source,
    })
}

fn event_evidence_ids(
    connection: &Connection,
    ingest_seq: u64,
    event_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT evidence_id, ordinal FROM event_evidence WHERE event_id = ?1 ORDER BY ordinal, evidence_id",
        )
        .map_err(StoreError::Sqlite)?;
    let rows = statement
        .query_map([event_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sqlite)?;
    rows.into_iter()
        .enumerate()
        .map(|(expected, (evidence_id, ordinal))| {
            if ordinal != expected as i64 {
                return Err(StoreError::StoredEventInvalid {
                    ingest_seq,
                    field: "evidence_ids",
                });
            }
            Ok(evidence_id)
        })
        .collect()
}

struct StoredEvent {
    protocol_version: String,
    event_id: String,
    run_id: String,
    session_id: Option<String>,
    stream_seq: Option<i64>,
    occurred_at: String,
    observed_at: String,
    event_type: String,
    source_json: String,
    confidence: f64,
    payload_version: i64,
    payload_json: String,
    extensions_json: String,
}

#[must_use]
pub fn initial_migration_checksum() -> String {
    migration_checksum(INITIAL_MIGRATION_SQL)
}

#[must_use]
pub fn project_filesystem_identity_migration_checksum() -> String {
    migration_checksum(PROJECT_FILESYSTEM_IDENTITY_MIGRATION_SQL)
}

fn migration_checksum(sql: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(sql.as_bytes());
    format!("{:x}", digest.finalize())
}

fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(StoreError::Sqlite)?;
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "temp_store", "MEMORY")
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "wal_autocheckpoint", 1_000_i64)
        .map_err(StoreError::Sqlite)
}

fn preflight_database(connection: &Connection) -> Result<bool, StoreError> {
    let objects = schema_objects(connection)?;
    let has_registry = objects
        .iter()
        .any(|object| object.kind == "table" && object.name == "schema_migrations");

    if !has_registry {
        let unmanaged = objects
            .iter()
            .filter(|object| !object.name.starts_with("sqlite_"))
            .map(|object| object.name.clone())
            .collect::<Vec<_>>();
        if !unmanaged.is_empty() {
            return Err(StoreError::UnmanagedDatabase { objects: unmanaged });
        }
        return Ok(true);
    }

    validate_migration_registry(connection)?;
    validate_schema_for_migration_count(connection, applied_migration_count(connection)?)?;
    validate_integrity(connection)?;
    Ok(false)
}

fn apply_pending_migrations(
    connection: &mut Connection,
    applied_at: &str,
    applied_count: usize,
) -> Result<(), StoreError> {
    for migration in migrations().iter().skip(applied_count) {
        if migration.version == PROJECT_FILESYSTEM_IDENTITY_MIGRATION_VERSION {
            validate_legacy_project_filesystem_ids(connection)?;
        }
        apply_migration(
            connection,
            migration.version,
            migration.name,
            &migration_checksum(migration.sql),
            applied_at,
            migration.sql,
        )?;
    }
    Ok(())
}

fn apply_migration(
    connection: &mut Connection,
    version: i64,
    name: &str,
    checksum: &str,
    applied_at: &str,
    sql: &str,
) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StoreError::Sqlite)?;
    transaction.execute_batch(sql).map_err(StoreError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(?1, ?2, ?3, ?4)",
            params![version, name, checksum, applied_at],
        )
        .map_err(StoreError::Sqlite)?;
    transaction.commit().map_err(StoreError::Sqlite)
}

fn validate_migration_registry(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .map_err(StoreError::Sqlite)?;
    let records = statement
        .query_map([], |row| {
            Ok(MigrationRecord {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })
        .map_err(StoreError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sqlite)?;

    if records.is_empty() {
        return Err(StoreError::MissingMigration {
            version: INITIAL_MIGRATION_VERSION,
        });
    }
    let known = migrations();
    for (index, record) in records.iter().enumerate() {
        let Some(expected) = known.get(index) else {
            return Err(StoreError::UnsupportedMigration {
                version: record.version,
            });
        };
        if record.version != expected.version {
            return Err(StoreError::MissingMigration {
                version: expected.version,
            });
        }
        if record.name != expected.name {
            return Err(StoreError::MigrationNameMismatch {
                version: record.version,
                expected: expected.name.to_owned(),
                actual: record.name.clone(),
            });
        }
        let expected_checksum = migration_checksum(expected.sql);
        if record.checksum != expected_checksum {
            return Err(StoreError::MigrationChecksumMismatch {
                version: record.version,
                expected: expected_checksum,
                actual: record.checksum.clone(),
            });
        }
    }
    Ok(())
}

fn applied_migration_count(connection: &Connection) -> Result<usize, StoreError> {
    connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count as usize)
        .map_err(StoreError::Sqlite)
}

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

fn migrations() -> [Migration; 2] {
    [
        Migration {
            version: INITIAL_MIGRATION_VERSION,
            name: INITIAL_MIGRATION_NAME,
            sql: INITIAL_MIGRATION_SQL,
        },
        Migration {
            version: PROJECT_FILESYSTEM_IDENTITY_MIGRATION_VERSION,
            name: PROJECT_FILESYSTEM_IDENTITY_MIGRATION_NAME,
            sql: PROJECT_FILESYSTEM_IDENTITY_MIGRATION_SQL,
        },
    ]
}

fn validate_schema(connection: &Connection) -> Result<(), StoreError> {
    validate_schema_for_migration_count(connection, migrations().len())
}

fn validate_schema_for_migration_count(
    connection: &Connection,
    migration_count: usize,
) -> Result<(), StoreError> {
    let expected_connection = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
    for migration in migrations().iter().take(migration_count) {
        expected_connection
            .execute_batch(migration.sql)
            .map_err(StoreError::Sqlite)?;
    }
    let expected = schema_objects(&expected_connection)?;
    let actual = schema_objects(connection)?;
    if actual != expected {
        return Err(StoreError::SchemaDrift {
            expected: schema_signature(&expected),
            actual: schema_signature(&actual),
        });
    }
    Ok(())
}

fn validate_legacy_project_filesystem_ids(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT id, filesystem_id FROM projects WHERE filesystem_id IS NOT NULL ORDER BY id",
        )
        .map_err(StoreError::Sqlite)?;
    let records = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(StoreError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sqlite)?;
    for (project_id, filesystem_id) in records {
        if !projects::is_valid_filesystem_id(&filesystem_id) {
            return Err(StoreError::InvalidStoredProjectFilesystemIdentity { project_id });
        }
    }
    Ok(())
}

fn validate_integrity(connection: &Connection) -> Result<(), StoreError> {
    let result = pragma_string(connection, "quick_check")?;
    if result != "ok" {
        return Err(StoreError::IntegrityCheckFailed(result));
    }
    Ok(())
}

fn validate_connection_policy(connection: &Connection) -> Result<(), StoreError> {
    let actual = read_connection_policy(connection)?;
    let expected = ConnectionPolicy {
        foreign_keys: true,
        journal_mode: "wal".to_owned(),
        synchronous: 1,
        busy_timeout_ms: 5_000,
        temp_store: 2,
        wal_autocheckpoint_pages: 1_000,
    };
    if actual != expected {
        return Err(StoreError::ConnectionPolicyMismatch {
            expected: Box::new(expected),
            actual: Box::new(actual),
        });
    }
    Ok(())
}

fn read_connection_policy(connection: &Connection) -> Result<ConnectionPolicy, StoreError> {
    Ok(ConnectionPolicy {
        foreign_keys: pragma_i64(connection, "foreign_keys")? == 1,
        journal_mode: pragma_string(connection, "journal_mode")?,
        synchronous: pragma_i64(connection, "synchronous")?,
        busy_timeout_ms: pragma_i64(connection, "busy_timeout")?,
        temp_store: pragma_i64(connection, "temp_store")?,
        wal_autocheckpoint_pages: pragma_i64(connection, "wal_autocheckpoint")?,
    })
}

fn pragma_i64(connection: &Connection, pragma: &str) -> Result<i64, StoreError> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .map_err(StoreError::Sqlite)
}

fn pragma_string(connection: &Connection, pragma: &str) -> Result<String, StoreError> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .map_err(StoreError::Sqlite)
}

fn schema_objects(connection: &Connection) -> Result<Vec<SchemaObject>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type, name",
        )
        .map_err(StoreError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok(SchemaObject {
                kind: row.get(0)?,
                name: row.get(1)?,
                table_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })
        .map_err(StoreError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sqlite)
}

fn schema_signature(objects: &[SchemaObject]) -> Vec<String> {
    objects
        .iter()
        .map(|object| {
            format!(
                "{}:{}:{}:{}",
                object.kind, object.name, object.table_name, object.sql
            )
        })
        .collect()
}

#[derive(Clone, Debug)]
struct MigrationRecord {
    version: i64,
    name: String,
    checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SchemaObject {
    kind: String,
    name: String,
    table_name: String,
    sql: String,
}

#[derive(Debug)]
pub enum StoreError {
    InvalidProjectTrustConfirmation {
        field: &'static str,
    },
    ProjectFilesystemIdentityUnavailable {
        project_id: String,
    },
    ProjectIdentityMismatch {
        project_id: String,
    },
    InvalidStoredProjectFilesystemIdentity {
        project_id: String,
    },
    ProjectInspection(ProjectInspectionError),
    InvalidProjectRegistration {
        field: &'static str,
    },
    ProjectIdConflict {
        project_id: String,
    },
    MissingProject {
        project_id: String,
    },
    InvalidCheckpointReport(CheckpointReport),
    InvalidEventBatchSize {
        count: usize,
        max: usize,
    },
    InvalidRunSnapshot {
        field: &'static str,
    },
    MissingRun {
        run_id: String,
    },
    RunSnapshotVersionNotOwned {
        run_id: String,
        version: u64,
    },
    StaleRunSnapshot {
        run_id: String,
        stored_version: u64,
        received_version: u64,
    },
    RunSnapshotConflict {
        run_id: String,
        version: u64,
    },
    RunSnapshotConcurrentChange {
        run_id: String,
    },
    StoredRunSnapshotInvalid {
        run_id: String,
        field: &'static str,
    },
    StoredRunSnapshotJson {
        run_id: String,
        source: serde_json::Error,
    },
    InvalidRunEventRange {
        cursor: u64,
        upper_bound: u64,
        limit: usize,
    },
    InvalidEvent {
        field: &'static str,
    },
    InvalidEventReadRange {
        cursor: u64,
        limit: usize,
    },
    EventIdentityConflict {
        event_id: String,
    },
    StreamSequenceConflict {
        session_id: String,
        stream_seq: u64,
        existing_event_id: String,
    },
    MissingEvidence {
        evidence_id: String,
    },
    MissingSession {
        session_id: String,
    },
    SessionRunMismatch {
        session_id: String,
        event_run_id: String,
        session_run_id: String,
    },
    EvidenceRunMismatch {
        evidence_id: String,
        event_run_id: String,
        evidence_run_id: String,
    },
    AssignedSequenceOutOfRange(i64),
    StoredEventInvalid {
        ingest_seq: u64,
        field: &'static str,
    },
    StoredJson {
        ingest_seq: u64,
        field: &'static str,
        source: serde_json::Error,
    },
    Json(serde_json::Error),
    InvalidMigrationAppliedAt,
    UnmanagedDatabase {
        objects: Vec<String>,
    },
    MissingMigration {
        version: i64,
    },
    UnsupportedMigration {
        version: i64,
    },
    MigrationNameMismatch {
        version: i64,
        expected: String,
        actual: String,
    },
    MigrationChecksumMismatch {
        version: i64,
        expected: String,
        actual: String,
    },
    SchemaDrift {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    IntegrityCheckFailed(String),
    ConnectionPolicyMismatch {
        expected: Box<ConnectionPolicy>,
        actual: Box<ConnectionPolicy>,
    },
    Sqlite(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProjectTrustConfirmation { field } => {
                write!(
                    formatter,
                    "invalid Project trust confirmation field: {field}"
                )
            }
            Self::ProjectFilesystemIdentityUnavailable { project_id } => {
                write!(
                    formatter,
                    "Project has no filesystem identity: {project_id}"
                )
            }
            Self::ProjectIdentityMismatch { project_id } => {
                write!(
                    formatter,
                    "Project identity no longer matches: {project_id}"
                )
            }
            Self::InvalidStoredProjectFilesystemIdentity { project_id } => write!(
                formatter,
                "stored Project has an invalid filesystem identity: {project_id}"
            ),
            Self::ProjectInspection(error) => {
                write!(formatter, "Project inspection failed: {error}")
            }
            Self::InvalidProjectRegistration { field } => {
                write!(formatter, "invalid Project registration field: {field}")
            }
            Self::ProjectIdConflict { project_id } => {
                write!(formatter, "Project ID already exists: {project_id}")
            }
            Self::MissingProject { project_id } => {
                write!(
                    formatter,
                    "Project does not exist after registration: {project_id}"
                )
            }
            Self::InvalidCheckpointReport(report) => write!(
                formatter,
                "invalid PASSIVE checkpoint report: busy {}, log frames {}, checkpointed frames {}",
                report.busy, report.log_frames, report.checkpointed_frames
            ),
            Self::InvalidEventBatchSize { count, max } => {
                write!(
                    formatter,
                    "invalid event batch size {count}; expected 1..={max}"
                )
            }
            Self::InvalidRunSnapshot { field } => {
                write!(formatter, "invalid Run snapshot field: {field}")
            }
            Self::MissingRun { run_id } => write!(formatter, "Run does not exist: {run_id}"),
            Self::RunSnapshotVersionNotOwned { run_id, version } => write!(
                formatter,
                "Run snapshot version {version} is not an event owned by {run_id}"
            ),
            Self::StaleRunSnapshot {
                run_id,
                stored_version,
                received_version,
            } => write!(
                formatter,
                "Run snapshot {run_id} is stale: stored {stored_version}, received {received_version}"
            ),
            Self::RunSnapshotConflict { run_id, version } => write!(
                formatter,
                "Run snapshot {run_id} conflicts at version {version}"
            ),
            Self::RunSnapshotConcurrentChange { run_id } => write!(
                formatter,
                "Run snapshot changed during replacement: {run_id}"
            ),
            Self::StoredRunSnapshotInvalid { run_id, field } => write!(
                formatter,
                "stored Run snapshot {run_id} has an invalid {field} field"
            ),
            Self::StoredRunSnapshotJson { run_id, source } => {
                write!(
                    formatter,
                    "stored Run snapshot {run_id} has invalid JSON: {source}"
                )
            }
            Self::InvalidRunEventRange {
                cursor,
                upper_bound,
                limit,
            } => write!(
                formatter,
                "invalid Run event range: cursor {cursor}, upper bound {upper_bound}, limit {limit}"
            ),
            Self::InvalidEvent { field } => write!(formatter, "invalid event field: {field}"),
            Self::InvalidEventReadRange { cursor, limit } => write!(
                formatter,
                "invalid event read range: cursor {cursor}, limit {limit}"
            ),
            Self::EventIdentityConflict { event_id } => {
                write!(
                    formatter,
                    "event identity conflicts with stored event: {event_id}"
                )
            }
            Self::StreamSequenceConflict {
                session_id,
                stream_seq,
                existing_event_id,
            } => write!(
                formatter,
                "session stream sequence {session_id}/{stream_seq} belongs to {existing_event_id}"
            ),
            Self::MissingEvidence { evidence_id } => {
                write!(formatter, "event evidence does not exist: {evidence_id}")
            }
            Self::MissingSession { session_id } => {
                write!(formatter, "event session does not exist: {session_id}")
            }
            Self::SessionRunMismatch {
                session_id,
                event_run_id,
                session_run_id,
            } => write!(
                formatter,
                "event session {session_id} belongs to Run {session_run_id}, not {event_run_id}"
            ),
            Self::EvidenceRunMismatch {
                evidence_id,
                event_run_id,
                evidence_run_id,
            } => write!(
                formatter,
                "event evidence {evidence_id} belongs to Run {evidence_run_id}, not {event_run_id}"
            ),
            Self::AssignedSequenceOutOfRange(sequence) => {
                write!(
                    formatter,
                    "assigned ingest sequence is out of range: {sequence}"
                )
            }
            Self::StoredEventInvalid { ingest_seq, field } => write!(
                formatter,
                "stored event {ingest_seq} has an invalid {field} field"
            ),
            Self::StoredJson {
                ingest_seq,
                field,
                source,
            } => write!(
                formatter,
                "stored event {ingest_seq} has invalid {field}: {source}"
            ),
            Self::Json(error) => write!(formatter, "event JSON error: {error}"),
            Self::InvalidMigrationAppliedAt => {
                formatter.write_str("migration applied_at must not be empty")
            }
            Self::UnmanagedDatabase { objects } => {
                write!(formatter, "database has no migration registry: {objects:?}")
            }
            Self::MissingMigration { version } => {
                write!(formatter, "required migration {version} is missing")
            }
            Self::UnsupportedMigration { version } => {
                write!(formatter, "database migration {version} is not supported")
            }
            Self::MigrationNameMismatch {
                version,
                expected,
                actual,
            } => write!(
                formatter,
                "migration {version} name mismatch: expected {expected}, found {actual}"
            ),
            Self::MigrationChecksumMismatch {
                version,
                expected,
                actual,
            } => write!(
                formatter,
                "migration {version} checksum mismatch: expected {expected}, found {actual}"
            ),
            Self::SchemaDrift { expected, actual } => write!(
                formatter,
                "database schema drift: expected {expected:?}, found {actual:?}"
            ),
            Self::IntegrityCheckFailed(result) => {
                write!(formatter, "SQLite quick_check failed: {result}")
            }
            Self::ConnectionPolicyMismatch { expected, actual } => write!(
                formatter,
                "SQLite connection policy mismatch: expected {expected:?}, found {actual:?}"
            ),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ProjectInspection(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::StoredJson { source, .. } => Some(source),
            Self::StoredRunSnapshotJson { source, .. } => Some(source),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TemporaryDirectory {
        path: PathBuf,
    }

    impl TemporaryDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("flit-store-{label}-{}-{nonce}", process::id()));
            fs::create_dir(&path).expect("unique temporary directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!(
                    "failed to remove test directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    #[test]
    fn checkpoint_report_rejects_negative_and_impossible_frame_counts() {
        for raw in [(-1, 0, 0), (0, -1, 0), (0, 1, -1), (0, 1, 2)] {
            assert!(matches!(
                Store::validated_checkpoint_report(raw.0, raw.1, raw.2),
                Err(StoreError::InvalidCheckpointReport(_))
            ));
        }
        assert_eq!(
            Store::validated_checkpoint_report(0, 3, 2).expect("valid checkpoint report"),
            CheckpointReport {
                busy: 0,
                log_frames: 3,
                checkpointed_frames: 2,
            }
        );
    }

    #[test]
    fn failed_migration_rolls_back_all_ddl_and_allows_clean_bootstrap() {
        let directory = TemporaryDirectory::new("rollback");
        let path = directory.path().join("flit.sqlite3");
        let mut connection = Connection::open(&path).expect("rollback database");
        configure_connection(&connection).expect("connection policy");
        let failing_sql = "
            CREATE TABLE schema_migrations (
              version INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              checksum TEXT NOT NULL,
              applied_at TEXT NOT NULL
            ) STRICT;
            CREATE TABLE partial_table(id INTEGER PRIMARY KEY) STRICT;
            INSERT INTO table_that_does_not_exist(id) VALUES(1);
        ";
        assert!(matches!(
            apply_migration(&mut connection, 1, "failing", "failing", "now", failing_sql),
            Err(StoreError::Sqlite(_))
        ));
        assert!(
            schema_objects(&connection)
                .expect("rolled back schema")
                .is_empty()
        );
        assert_eq!(
            pragma_string(&connection, "quick_check").expect("quick check"),
            "ok"
        );

        apply_pending_migrations(&mut connection, "now", 0).expect("clean retry");
        validate_migration_registry(&connection).expect("migration registry");
        validate_schema(&connection).expect("initial schema");
    }

    #[test]
    fn temporary_directory_is_removed_during_panic_unwind() {
        let mut observed_path = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let directory = TemporaryDirectory::new("panic-cleanup");
            observed_path = Some(directory.path().to_owned());
            panic!("intentional cleanup control");
        }));
        assert!(result.is_err());
        assert!(
            !observed_path
                .expect("panic fixture path")
                .try_exists()
                .expect("inspect cleanup path")
        );
    }
}
