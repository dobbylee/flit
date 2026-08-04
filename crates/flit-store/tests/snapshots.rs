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
    AppendEventOutcome, MAX_DASHBOARD_DELTA_EVENTS, MAX_DASHBOARD_DELTA_RUNS,
    MAX_DASHBOARD_DELTA_SOURCE_BYTES, MAX_DASHBOARD_SNAPSHOT_SOURCE_BYTES,
    MAX_RUN_DETAIL_SOURCE_BYTES, RunSnapshot, RunSnapshotDraft, Store, StoreError,
    WriteRunSnapshotOutcome,
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
    for attribution in [None, Some("guessed")] {
        let mut invalid_attribution = snapshot(RUN_A, 1, "Running", "Editing", 0.9);
        let changes = invalid_attribution
            .snapshot
            .get_mut("changes")
            .and_then(serde_json::Value::as_object_mut)
            .expect("changes object");
        match attribution {
            Some(attribution) => {
                changes.insert("attribution".to_owned(), json!(attribution));
            }
            None => {
                changes.remove("attribution");
            }
        }
        assert!(matches!(
            store.write_run_snapshot(invalid_attribution),
            Err(StoreError::InvalidRunSnapshot { field: "changes" })
        ));
    }
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

#[test]
fn dashboard_snapshot_and_global_delta_share_one_fixed_cursor_order() {
    let database = TestDatabase::new("dashboard-read");
    let mut store = database.open();
    let first = append(&mut store, event(RUN_A, "event-a-1", 1));
    let second = append(&mut store, event(RUN_B, "event-b-1", 1));
    let third = append(&mut store, event(RUN_A, "event-a-2", 2));
    assert_eq!((first, second, third), (1, 2, 3));
    let mut run_a_snapshot = snapshot(RUN_A, third, "Running", "Testing", 1.0);
    run_a_snapshot
        .snapshot
        .get_mut("attention")
        .and_then(serde_json::Value::as_object_mut)
        .expect("attention object")
        .insert("open_count".to_owned(), json!(2));
    run_a_snapshot.snapshot.insert(
        "changes".to_owned(),
        json!({
            "availability": "available",
            "attribution": "observed_during_run",
            "files": 3,
            "insertions": 42,
            "deletions": 7
        }),
    );
    store
        .write_run_snapshot(run_a_snapshot)
        .expect("Run A snapshot");
    store
        .write_run_snapshot(snapshot(RUN_B, second, "Running", "Editing", 0.9))
        .expect("Run B snapshot");

    let through_second = store
        .dashboard_run_snapshots_through(second)
        .expect("fixed Dashboard snapshot");
    assert_eq!(through_second.len(), 1);
    assert_eq!(through_second[0].projection.run_id, RUN_B);
    assert_eq!(through_second[0].project_id, PROJECT_ID);
    assert_eq!(through_second[0].project_display_name, "Snapshots");
    assert_eq!(through_second[0].title, RUN_B);
    assert_eq!(through_second[0].provider_kind, "codex");

    let current = store
        .dashboard_run_snapshots_through(third)
        .expect("current Dashboard snapshot");
    assert_eq!(
        current
            .iter()
            .map(|snapshot| snapshot.projection.run_id.as_str())
            .collect::<Vec<_>>(),
        [RUN_A, RUN_B]
    );
    assert_eq!(current[0].attention_open_count, 2);
    assert_eq!(
        current[0].changes,
        flit_store::DashboardChangeSummary::Available {
            attribution: flit_store::DashboardChangeAttribution::ObservedDuringRun,
            files: 3,
            insertions: 42,
            deletions: 7,
        }
    );

    let first_page = store
        .dashboard_event_locators_through(0, third, 2)
        .expect("first global page");
    assert_eq!(first_page.upper_bound, third);
    assert_eq!(
        first_page
            .events
            .iter()
            .map(|event| event.cursor)
            .collect::<Vec<_>>(),
        [first, second]
    );
    let second_page = store
        .dashboard_event_locators_through(second, third, MAX_DASHBOARD_DELTA_EVENTS)
        .expect("second global page");
    assert_eq!(
        second_page
            .events
            .iter()
            .map(|event| event.cursor)
            .collect::<Vec<_>>(),
        [third]
    );

    let requested = vec![RUN_B.to_owned(), RUN_A.to_owned()];
    let first_page_runs = store
        .dashboard_run_snapshots_for_delta(&requested, 0, second)
        .expect("first page projections");
    assert_eq!(first_page_runs.len(), 1);
    assert_eq!(first_page_runs[0].projection.run_id, RUN_B);
    assert_eq!(first_page_runs[0].projection.version, second);
    let second_page_runs = store
        .dashboard_run_snapshots_for_delta(&requested, second, third)
        .expect("second page projections");
    assert_eq!(second_page_runs.len(), 1);
    assert_eq!(second_page_runs[0].projection.run_id, RUN_A);
    assert_eq!(second_page_runs[0].projection.version, third);
}

#[test]
fn dashboard_reads_reject_future_reversed_and_unbounded_ranges() {
    let database = TestDatabase::new("dashboard-bounds");
    let mut store = database.open();
    append(&mut store, event(RUN_A, "event-a", 1));

    assert!(matches!(
        store.dashboard_run_snapshots_through(2),
        Err(StoreError::InvalidDashboardSnapshotCursor { upper_bound: 2 })
    ));
    for (cursor, upper_bound, limit) in [
        (2, 1, 1),
        (0, 2, 1),
        (0, 1, 0),
        (0, 1, MAX_DASHBOARD_DELTA_EVENTS + 1),
    ] {
        assert!(matches!(
            store.dashboard_event_locators_through(cursor, upper_bound, limit),
            Err(StoreError::InvalidGlobalEventRange { .. })
        ));
    }
    for run_ids in [
        vec![String::new()],
        vec![RUN_A.to_owned(), RUN_A.to_owned()],
        (0..=MAX_DASHBOARD_DELTA_RUNS)
            .map(|index| format!("run-{index}"))
            .collect(),
    ] {
        assert!(matches!(
            store.dashboard_run_snapshots_for_delta(&run_ids, 0, 1),
            Err(StoreError::InvalidDashboardProjectionRequest { field: "run_ids" })
        ));
    }
    assert!(matches!(
        store.dashboard_run_snapshots_for_delta(&[RUN_A.to_owned()], 2, 1),
        Err(StoreError::InvalidDashboardProjectionRequest { field: "cursor" })
    ));
}

#[test]
fn changed_projection_read_ignores_unrelated_corrupt_snapshots() {
    let database = TestDatabase::new("dashboard-changed-projections");
    let mut store = database.open();
    let run_a_version = append(&mut store, event(RUN_A, "event-a", 1));
    let run_b_version = append(&mut store, event(RUN_B, "event-b", 1));
    store
        .write_run_snapshot(snapshot(RUN_A, run_a_version, "Running", "Editing", 0.9))
        .expect("Run A snapshot");
    store
        .write_run_snapshot(snapshot(RUN_B, run_b_version, "Running", "Testing", 1.0))
        .expect("Run B snapshot");

    let connection = Connection::open(database.path()).expect("corruption connection");
    connection
        .execute(
            "UPDATE run_snapshots SET snapshot_json = '{}' WHERE run_id = ?1",
            [RUN_B],
        )
        .expect("corrupt unrelated snapshot");
    drop(connection);

    let changed = store
        .dashboard_run_snapshots_for_delta(
            &[RUN_A.to_owned()],
            0,
            store.latest_ingest_seq().expect("latest cursor"),
        )
        .expect("changed Run projection");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].projection.run_id, RUN_A);
    assert!(matches!(
        store.dashboard_run_snapshots_for_delta(&[RUN_B.to_owned()], 0, run_b_version),
        Err(StoreError::StoredRunSnapshotInvalid { .. })
    ));
    assert!(matches!(
        store.dashboard_run_snapshots_through(run_b_version),
        Err(StoreError::StoredRunSnapshotInvalid { .. })
    ));
}

#[test]
fn dashboard_snapshot_source_bound_fails_before_loading_oversized_json() {
    let database = TestDatabase::new("dashboard-source-bound");
    let mut store = database.open();
    let version = append(&mut store, event(RUN_A, "event-a", 1));
    store
        .write_run_snapshot(snapshot(RUN_A, version, "Running", "Editing", 0.9))
        .expect("valid source snapshot");
    drop(store);

    let oversized = "x".repeat(MAX_DASHBOARD_SNAPSHOT_SOURCE_BYTES + 1);
    let connection = Connection::open(database.path()).expect("oversize snapshot connection");
    connection
        .execute(
            "UPDATE run_snapshots SET snapshot_json = ?2 WHERE run_id = ?1",
            params![RUN_A, oversized],
        )
        .expect("oversize stored snapshot");
    drop(connection);

    let reopened = database.open();
    assert!(matches!(
        reopened.dashboard_run_snapshots_through(version),
        Err(StoreError::DashboardSnapshotReadTooLarge { .. })
    ));
    assert!(matches!(
        reopened.dashboard_run_snapshots_for_delta(&[RUN_A.to_owned()], 0, version),
        Err(StoreError::DashboardSnapshotReadTooLarge { .. })
    ));
    let connection = Connection::open(database.path()).expect("inspect oversized source");
    let stored_bytes: i64 = connection
        .query_row(
            "SELECT LENGTH(CAST(snapshot_json AS BLOB)) FROM run_snapshots WHERE run_id = ?1",
            [RUN_A],
            |row| row.get(0),
        )
        .expect("stored oversized source length");
    assert_eq!(
        stored_bytes,
        i64::try_from(MAX_DASHBOARD_SNAPSHOT_SOURCE_BYTES + 1).expect("source bound fits i64")
    );
}

#[test]
fn dashboard_delta_reads_only_bounded_locators_not_large_event_content() {
    let database = TestDatabase::new("dashboard-locator-only");
    let mut store = database.open();
    let mut oversized = event(RUN_A, "event-large-payload", 1);
    oversized.payload.insert(
        "raw_provider_content".to_owned(),
        serde_json::Value::String("x".repeat(MAX_DASHBOARD_DELTA_SOURCE_BYTES * 2)),
    );
    let cursor = append(&mut store, oversized);

    let page = store
        .dashboard_event_locators_through(0, cursor, MAX_DASHBOARD_DELTA_EVENTS)
        .expect("locator-only Dashboard delta");
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].cursor, cursor);
    assert_eq!(page.events[0].event_id, "event-large-payload");
    assert_eq!(page.events[0].run_id, RUN_A);
    assert_eq!(page.events[0].event_type, "run.event_observed");
}

#[test]
fn run_detail_evidence_is_exact_bounded_and_capability_receipt_owned() {
    let database = TestDatabase::new("run-detail");
    let mut store = database.open();
    let mut first_event = event(RUN_A, "event-a-1", 1);
    first_event.payload.insert(
        "raw_provider_content".to_owned(),
        serde_json::Value::String("secret-not-returned".repeat(10_000)),
    );
    let first = append(&mut store, first_event);
    append(&mut store, event(RUN_B, "event-b-1", 1));
    let third = append(&mut store, event(RUN_A, "event-a-2", 2));
    store
        .write_run_snapshot(snapshot(RUN_A, third, "Running", "Testing", 1.0))
        .expect("Run A snapshot");

    let first_page = store
        .run_evidence_through(RUN_A, 0, third, 1)
        .expect("first evidence page");
    assert!(first_page.has_more);
    assert_eq!(first_page.events.len(), 1);
    assert_eq!(first_page.events[0].cursor, first);
    assert_eq!(first_page.events[0].event_id, "event-a-1");
    assert_eq!(first_page.events[0].source_kind, "core");
    let second_page = store
        .run_evidence_through(RUN_A, first, third, 1)
        .expect("second evidence page");
    assert!(!second_page.has_more);
    assert_eq!(second_page.events.len(), 1);
    assert_eq!(second_page.events[0].cursor, third);
    let empty_page = store
        .run_evidence_through(RUN_A, third, third, 1)
        .expect("empty evidence page");
    assert!(!empty_page.has_more);
    assert!(empty_page.events.is_empty());

    let no_session = store
        .managed_run_detail_context(RUN_A)
        .expect("Run context without session");
    assert_eq!(no_session.run_version, third);
    assert_eq!(no_session.history_status, "unavailable");
    assert_eq!(no_session.open_in_provider_status, "unavailable");

    drop(store);
    let connection = Connection::open(database.path()).expect("session receipt connection");
    connection
        .execute(
            "INSERT INTO agent_sessions(
                id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint,
                executable_version, cwd, capabilities_json, started_at
             ) VALUES(
                'session-detail', ?1, 1, 'codex', 'thread-detail', 'fingerprint-detail',
                '0.145.0', '/private/tmp/flit-snapshots',
                '{\"history\":\"unsupported\",\"open_in_provider\":\"unsupported\"}', ?2
             )",
            params![RUN_A, APPLIED_AT],
        )
        .expect("stored capability receipt");
    drop(connection);

    let reopened = database.open();
    let context = reopened
        .managed_run_detail_context(RUN_A)
        .expect("Run context");
    assert_eq!(context.run_version, third);
    assert_eq!(context.history_status, "unsupported");
    assert_eq!(context.open_in_provider_status, "unsupported");
}

#[test]
fn run_detail_fails_closed_on_oversized_locator_or_malformed_capability() {
    let database = TestDatabase::new("run-detail-corruption");
    let mut store = database.open();
    let version = append(&mut store, event(RUN_A, "event-a", 1));
    store
        .write_run_snapshot(snapshot(RUN_A, version, "Running", "Editing", 0.9))
        .expect("Run A snapshot");
    drop(store);

    let connection = Connection::open(database.path()).expect("corruption connection");
    connection
        .execute(
            "UPDATE events SET event_id = ?2 WHERE run_id = ?1",
            params![RUN_A, "x".repeat(MAX_RUN_DETAIL_SOURCE_BYTES + 1)],
        )
        .expect("oversized evidence locator");
    connection
        .execute(
            "INSERT INTO agent_sessions(
                id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint,
                executable_version, cwd, capabilities_json, started_at
             ) VALUES(
                'session-detail', ?1, 1, 'codex', 'thread-detail', 'fingerprint-detail',
                '0.145.0', '/private/tmp/flit-snapshots',
                '{\"history\":true,\"open_in_provider\":\"unsupported\"}', ?2
             )",
            params![RUN_A, APPLIED_AT],
        )
        .expect("malformed capability receipt");
    drop(connection);

    let reopened = database.open();
    assert!(matches!(
        reopened.run_evidence_through(RUN_A, 0, version, 1),
        Err(StoreError::RunDetailReadTooLarge { .. })
    ));
    assert!(matches!(
        reopened.managed_run_detail_context(RUN_A),
        Err(StoreError::StoredManagedSessionInvalid { .. })
    ));
}

#[test]
fn dashboard_delta_rejects_middle_and_tail_cursor_gaps_without_repair() {
    let database = TestDatabase::new("dashboard-cursor-gaps");
    let mut store = database.open();
    let first = append(&mut store, event(RUN_A, "event-gap-1", 1));
    let second = append(&mut store, event(RUN_A, "event-gap-2", 2));
    let third = append(&mut store, event(RUN_A, "event-gap-3", 3));
    let fourth = append(&mut store, event(RUN_A, "event-gap-4", 4));
    drop(store);

    let connection = Connection::open(database.path()).expect("corrupt event connection");
    connection
        .execute(
            "DELETE FROM events WHERE ingest_seq = ?1",
            [i64::try_from(second).expect("cursor fits i64")],
        )
        .expect("remove middle event");
    drop(connection);

    let reopened = database.open();
    assert!(matches!(
        reopened.dashboard_event_locators_through(first, third, MAX_DASHBOARD_DELTA_EVENTS),
        Err(StoreError::StoredDashboardEventCursorGap {
            expected_cursor,
            actual_cursor: Some(actual_cursor),
        }) if expected_cursor == second && actual_cursor == third
    ));
    drop(reopened);

    let connection = Connection::open(database.path()).expect("corrupt tail connection");
    connection
        .execute(
            "DELETE FROM events WHERE ingest_seq = ?1",
            [i64::try_from(third).expect("cursor fits i64")],
        )
        .expect("remove fixed-bound tail event");
    drop(connection);

    let reopened = database.open();
    assert!(matches!(
        reopened.dashboard_event_locators_through(second, third, MAX_DASHBOARD_DELTA_EVENTS),
        Err(StoreError::StoredDashboardEventCursorGap {
            expected_cursor,
            actual_cursor: None,
        }) if expected_cursor == third
    ));
    assert_eq!(
        reopened
            .latest_ingest_seq()
            .expect("unchanged later cursor"),
        fourth
    );
}

fn append(store: &mut Store, event: UnsequencedEventEnvelope) -> u64 {
    match store.append_event(event).expect("append event") {
        AppendEventOutcome::Inserted(event) => event.ingest_seq,
        AppendEventOutcome::Duplicate(_) => panic!("expected inserted event"),
    }
}

fn event(run_id: &str, event_id: &str, stream_seq: u64) -> UnsequencedEventEnvelope {
    UnsequencedEventEnvelope {
        protocol_version: EventProtocolVersion::V1_1,
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
        "changes": {
            "availability": "available",
            "attribution": "exact",
            "files": 0,
            "insertions": 0,
            "deletions": 0
        },
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
