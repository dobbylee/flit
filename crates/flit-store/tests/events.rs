use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_protocol::{EventEnvelope, MAX_JSON_SAFE_INTEGER, UnsequencedEventEnvelope};
use flit_protocol::{EventSourceKind, NullableSessionId};
use flit_store::{AppendEventOutcome, Store, StoreError};
use rusqlite::{Connection, params};
use serde_json::json;

const APPLIED_AT: &str = "2026-07-23T00:00:00.000Z";
const PROJECT_ID: &str = "project-event-store";
const RUN_ID: &str = "run_01JZ8Y60R5M6V3Y2S0VJ3G8K1C";
const SESSION_ID: &str = "ses_01JZ8Y62E8FVDMZ00HBFV3N6XP";
const EVIDENCE_ID: &str = "evd_01JZ8Y6A7S6KZ0B2E9WP76M44X";
const SECOND_EVIDENCE_ID: &str = "evd_event_store_second";
const OTHER_RUN_EVIDENCE_ID: &str = "evd_other_run";
const OTHER_SESSION_ID: &str = "ses_other_run";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("flit-events-{label}-{}-{nonce}", process::id()));
        fs::create_dir(&directory).expect("unique test directory");
        let path = directory.join("flit.sqlite3");
        let database = Self { directory, path };
        let store = Store::open(&database.path, APPLIED_AT).expect("bootstrap store");
        drop(store);
        seed_registry(&database.path);
        database
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Store {
        Store::open(&self.path, APPLIED_AT).expect("open test store")
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
fn append_assigns_sequence_and_reopen_replays_lossless_ordered_envelopes() {
    let database = TestDatabase::new("replay");
    let mut store = database.open();
    let first_draft = event_fixture();
    let first = inserted(
        store
            .append_event(first_draft.clone())
            .expect("first append"),
    );
    assert_eq!(first.ingest_seq, 1);
    assert_eq!(
        first.extensions.get("future_envelope_field"),
        Some(&json!("kept"))
    );
    assert_eq!(
        first.source.extensions.get("observation_cursor"),
        Some(&json!("provider-cursor-42"))
    );
    assert_eq!(first.payload["future_payload_field"]["kept"], true);

    let mut second_draft = first_draft;
    second_draft.event_id = "event-second".to_owned();
    second_draft.stream_seq = 43;
    second_draft.event_type = "run.completed".to_owned();
    second_draft.evidence_ids = vec![SECOND_EVIDENCE_ID.to_owned(), EVIDENCE_ID.to_owned()];
    second_draft
        .extensions
        .insert("second".to_owned(), json!(2));
    let second = inserted(store.append_event(second_draft).expect("second append"));
    assert_eq!(second.ingest_seq, 2);
    assert_eq!(
        second.evidence_ids,
        [SECOND_EVIDENCE_ID.to_owned(), EVIDENCE_ID.to_owned()]
    );
    drop(store);

    let reopened = database.open();
    assert_eq!(
        reopened.events_after(0, 10).expect("full replay"),
        [first.clone(), second.clone()]
    );
    assert_eq!(
        reopened.events_after(0, 1).expect("bounded first page"),
        [first]
    );
    assert_eq!(
        reopened.events_after(1, 10).expect("cursor second page"),
        [second]
    );
    assert!(matches!(
        reopened.events_after(0, 0),
        Err(StoreError::InvalidEventReadRange { .. })
    ));
    assert!(matches!(
        reopened.events_after(0, 1_001),
        Err(StoreError::InvalidEventReadRange { .. })
    ));
    assert!(matches!(
        reopened.events_after(MAX_JSON_SAFE_INTEGER + 1, 1),
        Err(StoreError::InvalidEventReadRange { .. })
    ));
}

#[test]
fn exact_duplicate_and_conflicts_do_not_consume_sequence() {
    let database = TestDatabase::new("duplicates");
    let mut store = database.open();
    let draft = event_fixture();
    let first = inserted(store.append_event(draft.clone()).expect("first append"));
    assert_eq!(
        store.append_event(draft.clone()).expect("exact duplicate"),
        AppendEventOutcome::Duplicate(first)
    );

    let mut identity_conflict = draft.clone();
    identity_conflict
        .payload
        .insert("blocking".to_owned(), json!(false));
    assert!(matches!(
        store.append_event(identity_conflict),
        Err(StoreError::EventIdentityConflict { .. })
    ));

    let mut stream_conflict = draft.clone();
    stream_conflict.event_id = "event-stream-conflict".to_owned();
    assert!(matches!(
        store.append_event(stream_conflict),
        Err(StoreError::StreamSequenceConflict { stream_seq: 42, .. })
    ));

    let mut next = draft;
    next.event_id = "event-after-conflicts".to_owned();
    next.stream_seq = 43;
    let next = inserted(store.append_event(next).expect("next unique append"));
    assert_eq!(next.ingest_seq, 2);
    assert_eq!(store.events_after(0, 10).expect("stored events").len(), 2);
}

#[test]
fn invalid_evidence_input_and_foreign_keys_roll_back_without_sequence_holes() {
    let database = TestDatabase::new("failures");
    let mut store = database.open();
    let draft = event_fixture();

    let mut missing = draft.clone();
    missing.event_id = "event-missing-evidence".to_owned();
    missing.evidence_ids = vec!["evd_missing".to_owned()];
    assert!(matches!(
        store.append_event(missing),
        Err(StoreError::MissingEvidence { .. })
    ));

    let mut cross_run = draft.clone();
    cross_run.event_id = "event-cross-run-evidence".to_owned();
    cross_run.evidence_ids = vec![OTHER_RUN_EVIDENCE_ID.to_owned()];
    assert!(matches!(
        store.append_event(cross_run),
        Err(StoreError::EvidenceRunMismatch { .. })
    ));

    let mut duplicate_evidence = draft.clone();
    duplicate_evidence.event_id = "event-duplicate-evidence".to_owned();
    duplicate_evidence.evidence_ids = vec![EVIDENCE_ID.to_owned(), EVIDENCE_ID.to_owned()];
    assert!(matches!(
        store.append_event(duplicate_evidence),
        Err(StoreError::InvalidEvent {
            field: "evidence_ids"
        })
    ));

    let mut missing_run = draft.clone();
    missing_run.event_id = "event-missing-run".to_owned();
    missing_run.run_id = "run_missing".to_owned();
    missing_run.session_id = NullableSessionId::Null;
    missing_run.evidence_ids.clear();
    assert!(matches!(
        store.append_event(missing_run),
        Err(StoreError::Sqlite(_))
    ));

    let mut cross_run_session = draft.clone();
    cross_run_session.event_id = "event-cross-run-session".to_owned();
    cross_run_session.session_id = NullableSessionId::Id(OTHER_SESSION_ID.to_owned());
    assert!(matches!(
        store.append_event(cross_run_session),
        Err(StoreError::SessionRunMismatch { .. })
    ));

    let mut evidence_free_classifier = draft.clone();
    evidence_free_classifier.event_id = "event-evidence-free-classifier".to_owned();
    evidence_free_classifier.source.kind = EventSourceKind::Classifier;
    evidence_free_classifier.evidence_ids.clear();
    assert!(matches!(
        store.append_event(evidence_free_classifier),
        Err(StoreError::InvalidEvent {
            field: "evidence_ids"
        })
    ));

    let inserted = inserted(
        store
            .append_event(draft)
            .expect("valid append after failures"),
    );
    assert_eq!(inserted.ingest_seq, 1);
    assert_eq!(store.events_after(0, 10).expect("one event"), [inserted]);
}

#[test]
fn invalid_envelope_fields_and_reserved_extensions_fail_before_insert() {
    let database = TestDatabase::new("invalid-envelope");
    let mut store = database.open();
    let draft = event_fixture();

    let mut blank = draft.clone();
    blank.event_id = "  ".to_owned();
    assert!(matches!(
        store.append_event(blank),
        Err(StoreError::InvalidEvent { field: "event_id" })
    ));

    let mut invalid_confidence = draft.clone();
    invalid_confidence.confidence = f64::NAN;
    assert!(matches!(
        store.append_event(invalid_confidence),
        Err(StoreError::InvalidEvent {
            field: "confidence"
        })
    ));

    let mut reserved = draft.clone();
    reserved
        .extensions
        .insert("event_id".to_owned(), json!("shadow"));
    assert!(matches!(
        store.append_event(reserved),
        Err(StoreError::InvalidEvent {
            field: "extensions"
        })
    ));

    let mut reserved_source = draft;
    reserved_source
        .source
        .extensions
        .insert("kind".to_owned(), json!("shadow"));
    assert!(matches!(
        store.append_event(reserved_source),
        Err(StoreError::InvalidEvent {
            field: "source.extensions"
        })
    ));
    assert!(store.events_after(0, 10).expect("no events").is_empty());
}

#[test]
fn malformed_stored_json_fails_closed_without_repairing_the_row() {
    let database = TestDatabase::new("stored-corruption");
    let mut store = database.open();
    let event = inserted(store.append_event(event_fixture()).expect("append fixture"));
    drop(store);
    let connection = Connection::open(database.path()).expect("corrupt fixture");
    connection
        .execute(
            "UPDATE events SET extensions_json = '[]' WHERE ingest_seq = ?1",
            [event.ingest_seq as i64],
        )
        .expect("corrupt stored JSON shape");
    drop(connection);

    let reopened = database.open();
    assert!(matches!(
        reopened.events_after(0, 10),
        Err(StoreError::StoredJson {
            ingest_seq: 1,
            field: "extensions_json",
            ..
        })
    ));
    drop(reopened);
    let connection = Connection::open(database.path()).expect("inspect corrupt fixture");
    let stored: String = connection
        .query_row(
            "SELECT extensions_json FROM events WHERE ingest_seq = 1",
            [],
            |row| row.get(0),
        )
        .expect("stored JSON");
    assert_eq!(stored, "[]");
    connection
        .execute(
            "UPDATE events SET extensions_json = '{}' WHERE ingest_seq = 1",
            [],
        )
        .expect("restore extensions fixture");
    connection
        .execute(
            "UPDATE events SET session_id = ?1 WHERE ingest_seq = 1",
            [OTHER_SESSION_ID],
        )
        .expect("corrupt session ownership");
    drop(connection);
    let reopened = database.open();
    assert!(matches!(
        reopened.events_after(0, 10),
        Err(StoreError::StoredEventInvalid {
            ingest_seq: 1,
            field: "session_id"
        })
    ));
    drop(reopened);
    let connection = Connection::open(database.path()).expect("repair session fixture");
    connection
        .execute(
            "UPDATE events SET session_id = ?1 WHERE ingest_seq = 1",
            [SESSION_ID],
        )
        .expect("restore session ownership");
    connection
        .execute(
            "UPDATE event_evidence SET ordinal = 2 WHERE event_id = ?1",
            [&event.event_id],
        )
        .expect("corrupt evidence ordinal");
    drop(connection);
    let reopened = database.open();
    assert!(matches!(
        reopened.events_after(0, 10),
        Err(StoreError::StoredEventInvalid {
            ingest_seq: 1,
            field: "evidence_ids"
        })
    ));
}

fn inserted(outcome: AppendEventOutcome) -> EventEnvelope {
    match outcome {
        AppendEventOutcome::Inserted(event) => event,
        AppendEventOutcome::Duplicate(_) => panic!("expected inserted event"),
    }
}

fn event_fixture() -> UnsequencedEventEnvelope {
    let event: EventEnvelope = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/events/v1.0/permission.requested.json"
    ))
    .expect("current event fixture");
    UnsequencedEventEnvelope::from(event)
}

fn seed_registry(path: &Path) {
    let connection = Connection::open(path).expect("seed connection");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, 'Event Store', '/private/tmp/flit-event-store', 1, '{}', ?2, ?2)",
            params![PROJECT_ID, APPLIED_AT],
        )
        .expect("seed project");
    connection
        .execute(
            "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES(?1, ?2, 'Event Run', 'codex', '{}', ?3)",
            params![RUN_ID, PROJECT_ID, APPLIED_AT],
        )
        .expect("seed run");
    connection
        .execute(
            "INSERT INTO agent_sessions(id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint, cwd, capabilities_json, started_at) VALUES(?1, ?2, 1, 'codex', 'external-session', 'fixture-v1', '/private/tmp/flit-event-store', '{}', ?3)",
            params![SESSION_ID, RUN_ID, APPLIED_AT],
        )
        .expect("seed session");
    connection
        .execute(
            "INSERT INTO evidence(id, run_id, session_id, kind, locator_json, created_at) VALUES(?1, ?2, ?3, 'provider_history', '{}', ?4)",
            params![EVIDENCE_ID, RUN_ID, SESSION_ID, APPLIED_AT],
        )
        .expect("seed first evidence");
    connection
        .execute(
            "INSERT INTO evidence(id, run_id, session_id, kind, locator_json, created_at) VALUES(?1, ?2, ?3, 'provider_history', '{}', ?4)",
            params![SECOND_EVIDENCE_ID, RUN_ID, SESSION_ID, APPLIED_AT],
        )
        .expect("seed second evidence");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES('project-other', 'Other', '/private/tmp/flit-event-store-other', 1, '{}', ?1, ?1)",
            [APPLIED_AT],
        )
        .expect("seed other project");
    connection
        .execute(
            "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES('run-other', 'project-other', 'Other Run', 'codex', '{}', ?1)",
            [APPLIED_AT],
        )
        .expect("seed other run");
    connection
        .execute(
            "INSERT INTO agent_sessions(id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint, cwd, capabilities_json, started_at) VALUES(?1, 'run-other', 1, 'codex', 'external-other', 'fixture-v1', '/private/tmp/flit-event-store-other', '{}', ?2)",
            params![OTHER_SESSION_ID, APPLIED_AT],
        )
        .expect("seed other session");
    connection
        .execute(
            "INSERT INTO evidence(id, run_id, kind, locator_json, created_at) VALUES(?1, 'run-other', 'provider_history', '{}', ?2)",
            params![OTHER_RUN_EVIDENCE_ID, APPLIED_AT],
        )
        .expect("seed other evidence");
}
