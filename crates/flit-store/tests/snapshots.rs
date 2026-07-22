use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_protocol::{
    EventProtocolVersion, EventSource, EventSourceKind, NullableSessionId, UnsequencedEventEnvelope,
};
use flit_store::{
    AppendEventOutcome, RunSnapshot, RunSnapshotDraft, Store, StoreError, WriteRunSnapshotOutcome,
};
use rusqlite::{Connection, params};
use serde_json::{Map, json};

const APPLIED_AT: &str = "2026-07-23T00:00:00.000Z";
const PROJECT_ID: &str = "project-snapshots";
const RUN_A: &str = "run-snapshot-a";
const RUN_B: &str = "run-snapshot-b";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("flit-snapshots-{label}-{}-{nonce}", process::id()));
        fs::create_dir(&directory).expect("unique test directory");
        let path = directory.join("flit.sqlite3");
        let database = Self { directory, path };
        let store = Store::open(&database.path, APPLIED_AT).expect("bootstrap store");
        drop(store);
        seed_runs(&database.path);
        database
    }

    fn open(&self) -> Store {
        Store::open(&self.path, APPLIED_AT).expect("open test store")
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "failed to remove test directory {}: {error}",
                self.directory.display()
            );
        }
    }
}

#[test]
fn snapshot_insert_duplicate_replace_and_reopen_are_monotonic() {
    let database = TestDatabase::new("monotonic");
    let mut store = database.open();
    let first_event = append(&mut store, event(RUN_A, "event-a-1", 1));
    assert_eq!(first_event, 1);
    let other_event = append(&mut store, event(RUN_B, "event-b-1", 1));
    assert_eq!(other_event, 2);

    let first = snapshot(RUN_A, first_event, "Running", "Editing", 0.9);
    let first_record = RunSnapshot::from(first.clone());
    assert_eq!(
        store
            .write_run_snapshot(first.clone())
            .expect("insert snapshot"),
        WriteRunSnapshotOutcome::Inserted(first_record.clone())
    );
    assert_eq!(
        store
            .write_run_snapshot(first.clone())
            .expect("duplicate snapshot"),
        WriteRunSnapshotOutcome::Duplicate(first_record.clone())
    );

    let third_event = append(&mut store, event(RUN_A, "event-a-2", 2));
    assert_eq!(third_event, 3);
    let interleaved_event = append(&mut store, event(RUN_B, "event-b-2", 2));
    assert_eq!(interleaved_event, 4);
    let fifth_event = append(&mut store, event(RUN_A, "event-a-3", 3));
    assert_eq!(fifth_event, 5);
    let sixth_event = append(&mut store, event(RUN_A, "event-a-4", 4));
    assert_eq!(sixth_event, 6);
    let upper_bound = store.latest_ingest_seq().expect("fixed upper bound");
    assert_eq!(upper_bound, 6);
    let later_event = append(&mut store, event(RUN_A, "event-a-5", 5));
    assert_eq!(later_event, 7);
    let first_page = store
        .run_events_through(RUN_A, first_event, upper_bound, 2)
        .expect("first fixed tail page");
    assert_eq!(first_page.upper_bound, 6);
    assert_eq!(
        first_page
            .events
            .iter()
            .map(|event| event.ingest_seq)
            .collect::<Vec<_>>(),
        [third_event, fifth_event]
    );
    let second_page = store
        .run_events_through(RUN_A, fifth_event, upper_bound, 2)
        .expect("second fixed tail page");
    assert_eq!(second_page.upper_bound, 6);
    assert_eq!(
        second_page
            .events
            .iter()
            .map(|event| event.ingest_seq)
            .collect::<Vec<_>>(),
        [sixth_event]
    );

    let newer = snapshot(RUN_A, third_event, "Running", "Testing", 1.0);
    let newer_record = RunSnapshot::from(newer.clone());
    assert_eq!(
        store
            .write_run_snapshot(newer.clone())
            .expect("replace snapshot"),
        WriteRunSnapshotOutcome::Replaced(newer_record.clone())
    );
    assert!(matches!(
        store.write_run_snapshot(first),
        Err(StoreError::StaleRunSnapshot {
            stored_version: 3,
            received_version: 1,
            ..
        })
    ));

    let mut conflict = newer;
    conflict.activity = "Reviewing".to_owned();
    conflict
        .snapshot
        .get_mut("activity")
        .and_then(serde_json::Value::as_object_mut)
        .expect("activity object")
        .insert("kind".to_owned(), json!("Reviewing"));
    assert!(matches!(
        store.write_run_snapshot(conflict),
        Err(StoreError::RunSnapshotConflict { version: 3, .. })
    ));
    assert_eq!(
        store.run_snapshot(RUN_A).expect("snapshot"),
        Some(newer_record.clone())
    );
    drop(store);

    let reopened = database.open();
    assert_eq!(
        reopened.run_snapshot(RUN_A).expect("reopened snapshot"),
        Some(newer_record)
    );
}

#[test]
fn snapshot_version_and_content_validation_fail_before_mutation() {
    let database = TestDatabase::new("invalid");
    let mut store = database.open();
    let run_a_event = append(&mut store, event(RUN_A, "event-a", 1));
    let run_b_event = append(&mut store, event(RUN_B, "event-b", 1));
    assert_eq!((run_a_event, run_b_event), (1, 2));

    let mut zero = snapshot(RUN_A, 1, "Running", "Editing", 0.9);
    zero.version = 0;
    zero.snapshot.insert("version".to_owned(), json!(0));
    assert!(matches!(
        store.write_run_snapshot(zero),
        Err(StoreError::InvalidRunSnapshot { field: "version" })
    ));

    assert!(matches!(
        store.write_run_snapshot(snapshot(RUN_A, run_b_event, "Running", "Editing", 0.9)),
        Err(StoreError::RunSnapshotVersionNotOwned { version: 2, .. })
    ));
    assert!(matches!(
        store.write_run_snapshot(snapshot(RUN_A, 99, "Running", "Editing", 0.9)),
        Err(StoreError::RunSnapshotVersionNotOwned { version: 99, .. })
    ));
    assert!(matches!(
        store.write_run_snapshot(snapshot("run-missing", 1, "Running", "Editing", 0.9)),
        Err(StoreError::MissingRun { .. })
    ));

    let mut invalid_confidence = snapshot(RUN_A, 1, "Running", "Editing", 0.9);
    invalid_confidence.activity_confidence = f64::NAN;
    assert!(matches!(
        store.write_run_snapshot(invalid_confidence),
        Err(StoreError::InvalidRunSnapshot {
            field: "activity_confidence"
        })
    ));
    let mut mismatched_lifecycle = snapshot(RUN_A, 1, "Running", "Editing", 0.9);
    mismatched_lifecycle
        .snapshot
        .insert("lifecycle".to_owned(), json!("Failed"));
    assert!(matches!(
        store.write_run_snapshot(mismatched_lifecycle),
        Err(StoreError::InvalidRunSnapshot { field: "lifecycle" })
    ));
    let mut mismatched_activity = snapshot(RUN_A, 1, "Running", "Editing", 0.9);
    mismatched_activity
        .snapshot
        .get_mut("activity")
        .and_then(serde_json::Value::as_object_mut)
        .expect("activity object")
        .insert("confidence".to_owned(), json!(0.8));
    assert!(matches!(
        store.write_run_snapshot(mismatched_activity),
        Err(StoreError::InvalidRunSnapshot {
            field: "activity.confidence"
        })
    ));
    let mut mismatched_attention = snapshot(RUN_A, 1, "Running", "Editing", 0.9);
    mismatched_attention
        .snapshot
        .get_mut("attention")
        .and_then(serde_json::Value::as_object_mut)
        .expect("attention object")
        .insert("level".to_owned(), json!("ActionRequired"));
    assert!(matches!(
        store.write_run_snapshot(mismatched_attention),
        Err(StoreError::InvalidRunSnapshot {
            field: "attention.level"
        })
    ));
    assert_eq!(store.run_snapshot(RUN_A).expect("no snapshot"), None);
}

#[test]
fn malformed_stored_snapshot_fails_closed_without_repair() {
    let database = TestDatabase::new("corruption");
    let mut store = database.open();
    let version = append(&mut store, event(RUN_A, "event-a", 1));
    let valid = snapshot(RUN_A, version, "Running", "Editing", 0.9);
    store
        .write_run_snapshot(valid.clone())
        .expect("insert snapshot");
    drop(store);

    let connection = Connection::open(database.path()).expect("corrupt snapshot");
    connection
        .execute(
            "UPDATE run_snapshots SET snapshot_json = '[]' WHERE run_id = ?1",
            [RUN_A],
        )
        .expect("corrupt JSON shape");
    drop(connection);
    let reopened = database.open();
    assert!(matches!(
        reopened.run_snapshot(RUN_A),
        Err(StoreError::StoredRunSnapshotJson { .. })
    ));
    drop(reopened);

    let connection = Connection::open(database.path()).expect("corrupt normalized column");
    connection
        .execute(
            "UPDATE run_snapshots SET snapshot_json = ?2, lifecycle = 'Failed' WHERE run_id = ?1",
            params![
                RUN_A,
                serde_json::to_string(&valid.snapshot).expect("snapshot JSON")
            ],
        )
        .expect("create normalized mismatch");
    drop(connection);
    let reopened = database.open();
    assert!(matches!(
        reopened.run_snapshot(RUN_A),
        Err(StoreError::StoredRunSnapshotInvalid { .. })
    ));
    drop(reopened);

    let connection = Connection::open(database.path()).expect("inspect original row");
    let lifecycle: String = connection
        .query_row(
            "SELECT lifecycle FROM run_snapshots WHERE run_id = ?1",
            [RUN_A],
            |row| row.get(0),
        )
        .expect("stored lifecycle");
    assert_eq!(lifecycle, "Failed");
}

#[test]
fn run_tail_rejects_invalid_bounds_and_missing_runs() {
    let database = TestDatabase::new("tail-bounds");
    let mut store = database.open();
    append(&mut store, event(RUN_A, "event-a", 1));
    assert!(matches!(
        store.run_events_through(RUN_A, 2, 1, 10),
        Err(StoreError::InvalidRunEventRange { .. })
    ));
    assert!(matches!(
        store.run_events_through(RUN_A, 0, 2, 10),
        Err(StoreError::InvalidRunEventRange { .. })
    ));
    assert!(matches!(
        store.run_events_through(RUN_A, 0, 1, 0),
        Err(StoreError::InvalidRunEventRange { .. })
    ));
    assert!(matches!(
        store.run_events_through("run-missing", 0, 1, 10),
        Err(StoreError::MissingRun { .. })
    ));
}

fn append(store: &mut Store, event: UnsequencedEventEnvelope) -> u64 {
    match store.append_event(event).expect("append event") {
        AppendEventOutcome::Inserted(event) => event.ingest_seq,
        AppendEventOutcome::Duplicate(_) => panic!("expected inserted event"),
    }
}

fn event(run_id: &str, event_id: &str, stream_seq: u64) -> UnsequencedEventEnvelope {
    UnsequencedEventEnvelope {
        protocol_version: EventProtocolVersion::V1_0,
        event_id: event_id.to_owned(),
        run_id: run_id.to_owned(),
        session_id: NullableSessionId::Null,
        stream_seq,
        occurred_at: APPLIED_AT.to_owned(),
        observed_at: APPLIED_AT.to_owned(),
        event_type: "run.event_observed".to_owned(),
        source: EventSource {
            kind: EventSourceKind::Core,
            provider: None,
            contract_version: None,
            extensions: BTreeMap::new(),
        },
        confidence: 1.0,
        evidence_ids: Vec::new(),
        payload: Map::new(),
        extensions: BTreeMap::new(),
    }
}

fn snapshot(
    run_id: &str,
    version: u64,
    lifecycle: &str,
    activity: &str,
    activity_confidence: f64,
) -> RunSnapshotDraft {
    let last_progress_at = Some(APPLIED_AT.to_owned());
    let last_liveness_at = Some(APPLIED_AT.to_owned());
    let value = json!({
        "run_id": run_id,
        "version": version,
        "lifecycle": lifecycle,
        "activity": {
            "kind": activity,
            "confidence": activity_confidence
        },
        "attention": {
            "level": "None",
            "open_count": 0
        },
        "dashboard_bucket": "Working",
        "last_progress_at": last_progress_at,
        "last_liveness_at": last_liveness_at,
        "future_projection_field": { "kept": true }
    });
    RunSnapshotDraft {
        run_id: run_id.to_owned(),
        version,
        lifecycle: lifecycle.to_owned(),
        activity: activity.to_owned(),
        activity_confidence,
        attention_level: "None".to_owned(),
        dashboard_bucket: "Working".to_owned(),
        last_progress_at,
        last_liveness_at,
        snapshot: value.as_object().expect("snapshot object").clone(),
        updated_at: APPLIED_AT.to_owned(),
    }
}

fn seed_runs(path: &Path) {
    let connection = Connection::open(path).expect("seed connection");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, 'Snapshots', '/private/tmp/flit-snapshots', 1, '{}', ?2, ?2)",
            params![PROJECT_ID, APPLIED_AT],
        )
        .expect("seed project");
    for run_id in [RUN_A, RUN_B] {
        connection
            .execute(
                "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES(?1, ?2, ?1, 'codex', '{}', ?3)",
                params![run_id, PROJECT_ID, APPLIED_AT],
            )
            .expect("seed Run");
    }
}
