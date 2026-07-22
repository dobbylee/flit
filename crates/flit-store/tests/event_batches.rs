use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_protocol::{EventEnvelope, NullableSessionId, UnsequencedEventEnvelope};
use flit_store::{AppendEventOutcome, MAX_EVENT_APPEND_BATCH, Store, StoreError};
use rusqlite::{Connection, params};
use serde_json::json;

const APPLIED_AT: &str = "2026-07-23T00:00:00.000Z";
const PROJECT_ID: &str = "project-event-batches";
const RUN_ID: &str = "run_01JZ8Y60R5M6V3Y2S0VJ3G8K1C";
const SESSION_ID: &str = "ses_01JZ8Y62E8FVDMZ00HBFV3N6XP";
const EVIDENCE_ID: &str = "evd_01JZ8Y6A7S6KZ0B2E9WP76M44X";
const OTHER_SESSION_ID: &str = "ses-event-batch-other";
const OTHER_EVIDENCE_ID: &str = "evd-event-batch-other";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "flit-event-batches-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir(&directory).expect("unique test directory");
        let path = directory.join("flit.sqlite3");
        let database = Self { directory, path };
        let store = Store::open(&database.path, APPLIED_AT).expect("bootstrap store");
        drop(store);
        seed_registry(&database.path);
        database
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
fn batch_preserves_input_order_and_duplicates_do_not_consume_cursors() {
    let database = TestDatabase::new("ordered");
    let mut store = database.open();
    let existing = inserted(
        store
            .append_event(event("event-existing", 42))
            .expect("append existing event"),
    );
    let first_unique = event("event-batch-first", 43);
    let second_unique = event("event-batch-second", 44);
    let outcomes = store
        .append_event_batch(vec![
            UnsequencedEventEnvelope::from(existing.clone()),
            first_unique.clone(),
            first_unique,
            second_unique,
        ])
        .expect("append ordered batch");

    assert_eq!(outcomes.len(), 4);
    assert_eq!(outcome(&outcomes[0]), (false, 1, "event-existing"));
    assert_eq!(outcome(&outcomes[1]), (true, 2, "event-batch-first"));
    assert_eq!(outcome(&outcomes[2]), (false, 2, "event-batch-first"));
    assert_eq!(outcome(&outcomes[3]), (true, 3, "event-batch-second"));
    assert_eq!(
        store
            .events_after(0, 10)
            .expect("stored unique events")
            .iter()
            .map(|event| (event.ingest_seq, event.event_id.as_str()))
            .collect::<Vec<_>>(),
        [
            (1, "event-existing"),
            (2, "event-batch-first"),
            (3, "event-batch-second")
        ]
    );
}

#[derive(Clone, Copy, Debug)]
enum LateFailure {
    InvalidEnvelope,
    IdentityConflict,
    StreamConflict,
    SessionMismatch,
    EvidenceMismatch,
    ForeignKey,
}

#[test]
fn every_late_failure_rolls_back_earlier_inserts_without_a_cursor_hole() {
    for failure in [
        LateFailure::InvalidEnvelope,
        LateFailure::IdentityConflict,
        LateFailure::StreamConflict,
        LateFailure::SessionMismatch,
        LateFailure::EvidenceMismatch,
        LateFailure::ForeignKey,
    ] {
        let database = TestDatabase::new(&format!("rollback-{failure:?}"));
        let mut store = database.open();
        let existing_draft = event("event-existing", 42);
        let existing = inserted(
            store
                .append_event(existing_draft.clone())
                .expect("append existing event"),
        );
        let first = event("event-rolled-back-first", 43);
        let late = late_failure(failure, existing_draft);

        let error = store
            .append_event_batch(vec![first, late])
            .expect_err("late batch failure");
        assert_expected_failure(failure, &error);
        assert_eq!(
            store.events_after(0, 10).expect("preserved baseline"),
            [existing]
        );

        let next = inserted(
            store
                .append_event(event("event-after-rollback", 43))
                .expect("append after rollback"),
        );
        assert_eq!(next.ingest_seq, 2, "failure: {failure:?}");
    }
}

#[test]
fn input_order_makes_an_earlier_conflict_win_over_later_invalid_input() {
    let database = TestDatabase::new("ordered-failure");
    let mut store = database.open();
    let existing_draft = event("event-existing", 42);
    let existing = inserted(
        store
            .append_event(existing_draft.clone())
            .expect("append existing event"),
    );
    let mut conflict = existing_draft;
    conflict.payload.insert("blocking".to_owned(), json!(false));
    let mut invalid = event(" ", 43);
    invalid.evidence_ids.clear();

    assert!(matches!(
        store.append_event_batch(vec![conflict, invalid]),
        Err(StoreError::EventIdentityConflict { .. })
    ));
    assert_eq!(
        store.events_after(0, 10).expect("preserved baseline"),
        [existing]
    );
    let next = inserted(
        store
            .append_event(event("event-after-ordered-failure", 43))
            .expect("append after ordered failure"),
    );
    assert_eq!(next.ingest_seq, 2);
}

#[test]
fn empty_and_oversized_batches_fail_before_mutation() {
    let database = TestDatabase::new("bounds");
    let mut store = database.open();
    assert!(matches!(
        store.append_event_batch(Vec::new()),
        Err(StoreError::InvalidEventBatchSize {
            count: 0,
            max: MAX_EVENT_APPEND_BATCH
        })
    ));
    assert!(matches!(
        store.append_event_batch(vec![
            event("event-repeated", 42);
            MAX_EVENT_APPEND_BATCH + 1
        ]),
        Err(StoreError::InvalidEventBatchSize {
            count: 51,
            max: MAX_EVENT_APPEND_BATCH
        })
    ));
    assert!(store.events_after(0, 10).expect("no events").is_empty());
}

fn outcome(outcome: &AppendEventOutcome) -> (bool, u64, &str) {
    match outcome {
        AppendEventOutcome::Inserted(event) => (true, event.ingest_seq, &event.event_id),
        AppendEventOutcome::Duplicate(event) => (false, event.ingest_seq, &event.event_id),
    }
}

fn inserted(outcome: AppendEventOutcome) -> EventEnvelope {
    match outcome {
        AppendEventOutcome::Inserted(event) => event,
        AppendEventOutcome::Duplicate(_) => panic!("expected inserted event"),
    }
}

fn event(event_id: &str, stream_seq: u64) -> UnsequencedEventEnvelope {
    let event: EventEnvelope = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/events/v1.0/permission.requested.json"
    ))
    .expect("current event fixture");
    let mut event = UnsequencedEventEnvelope::from(event);
    event.event_id = event_id.to_owned();
    event.stream_seq = stream_seq;
    event
}

fn late_failure(
    failure: LateFailure,
    existing: UnsequencedEventEnvelope,
) -> UnsequencedEventEnvelope {
    match failure {
        LateFailure::InvalidEnvelope => {
            let mut event = event(" ", 44);
            event.evidence_ids.clear();
            event
        }
        LateFailure::IdentityConflict => {
            let mut event = existing;
            event.payload.insert("blocking".to_owned(), json!(false));
            event
        }
        LateFailure::StreamConflict => event("event-stream-conflict", 42),
        LateFailure::SessionMismatch => {
            let mut event = event("event-session-mismatch", 44);
            event.session_id = NullableSessionId::Id(OTHER_SESSION_ID.to_owned());
            event
        }
        LateFailure::EvidenceMismatch => {
            let mut event = event("event-evidence-mismatch", 44);
            event.evidence_ids = vec![OTHER_EVIDENCE_ID.to_owned()];
            event
        }
        LateFailure::ForeignKey => {
            let mut event = event("event-missing-run", 44);
            event.run_id = "run-missing".to_owned();
            event.session_id = NullableSessionId::Null;
            event.evidence_ids.clear();
            event
        }
    }
}

fn assert_expected_failure(failure: LateFailure, error: &StoreError) {
    let matches = match failure {
        LateFailure::InvalidEnvelope => {
            matches!(error, StoreError::InvalidEvent { field: "event_id" })
        }
        LateFailure::IdentityConflict => {
            matches!(error, StoreError::EventIdentityConflict { .. })
        }
        LateFailure::StreamConflict => {
            matches!(error, StoreError::StreamSequenceConflict { .. })
        }
        LateFailure::SessionMismatch => {
            matches!(error, StoreError::SessionRunMismatch { .. })
        }
        LateFailure::EvidenceMismatch => {
            matches!(error, StoreError::EvidenceRunMismatch { .. })
        }
        LateFailure::ForeignKey => matches!(error, StoreError::Sqlite(_)),
    };
    assert!(matches, "unexpected {failure:?} error: {error}");
}

fn seed_registry(path: &Path) {
    let connection = Connection::open(path).expect("seed connection");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, 'Event Batches', '/private/tmp/flit-event-batches', 1, '{}', ?2, ?2)",
            params![PROJECT_ID, APPLIED_AT],
        )
        .expect("seed project");
    connection
        .execute(
            "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES(?1, ?2, 'Batch Run', 'codex', '{}', ?3)",
            params![RUN_ID, PROJECT_ID, APPLIED_AT],
        )
        .expect("seed run");
    connection
        .execute(
            "INSERT INTO agent_sessions(id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint, cwd, capabilities_json, started_at) VALUES(?1, ?2, 1, 'codex', 'external-session', 'fixture-v1', '/private/tmp/flit-event-batches', '{}', ?3)",
            params![SESSION_ID, RUN_ID, APPLIED_AT],
        )
        .expect("seed session");
    connection
        .execute(
            "INSERT INTO evidence(id, run_id, session_id, kind, locator_json, created_at) VALUES(?1, ?2, ?3, 'provider_history', '{}', ?4)",
            params![EVIDENCE_ID, RUN_ID, SESSION_ID, APPLIED_AT],
        )
        .expect("seed evidence");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES('project-other', 'Other', '/private/tmp/flit-event-batches-other', 1, '{}', ?1, ?1)",
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
            "INSERT INTO agent_sessions(id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint, cwd, capabilities_json, started_at) VALUES(?1, 'run-other', 1, 'codex', 'external-other', 'fixture-v1', '/private/tmp/flit-event-batches-other', '{}', ?2)",
            params![OTHER_SESSION_ID, APPLIED_AT],
        )
        .expect("seed other session");
    connection
        .execute(
            "INSERT INTO evidence(id, run_id, kind, locator_json, created_at) VALUES(?1, 'run-other', 'provider_history', '{}', ?2)",
            params![OTHER_EVIDENCE_ID, APPLIED_AT],
        )
        .expect("seed other evidence");
}
