use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use flit_protocol::{
    EventProtocolVersion, EventSource, EventSourceKind, NullableSessionId, UnsequencedEventEnvelope,
};
use flit_store::{
    AppendEventOutcome, CheckpointFailure, EVENT_WRITER_THREAD_NAME, EventWriter, Store,
};
use rusqlite::{Connection, params};
use serde_json::Map;

const APPLIED_AT: &str = "2026-07-23T00:00:00.000Z";
const PROJECT_ID: &str = "project-checkpoints";
const RUN_ID: &str = "run-checkpoints";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "flit-checkpoints-{label}-{}-{nonce}",
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
        Store::open(&self.path, APPLIED_AT).expect("open store")
    }

    fn writer(&self) -> EventWriter {
        EventWriter::start(&self.path, APPLIED_AT).expect("start writer")
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
fn passive_checkpoint_is_partial_with_a_pinned_reader_then_catches_up() {
    let database = TestDatabase::new("pinned-reader");
    let mut store = database.open();
    inserted(
        store
            .append_event(event("event-before-reader", 1))
            .expect("append baseline event"),
    );
    let baseline = store.passive_checkpoint().expect("baseline checkpoint");
    assert!(baseline.checkpointed_frames <= baseline.log_frames);

    let reader = Connection::open(database.path()).expect("open pinned reader");
    reader
        .execute_batch("BEGIN")
        .expect("begin read transaction");
    let observed: i64 = reader
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .expect("pin read snapshot");
    assert_eq!(observed, 1);

    let events = (2..=20_u64)
        .map(|sequence| event(&format!("event-after-reader-{sequence}"), sequence))
        .collect();
    store
        .append_event_batch(events)
        .expect("append WAL frames behind reader");
    let pinned = store.passive_checkpoint().expect("non-blocking checkpoint");
    assert!(pinned.log_frames > 0);
    assert!(pinned.checkpointed_frames < pinned.log_frames);

    reader
        .execute_batch("ROLLBACK")
        .expect("release read transaction");
    drop(reader);
    let caught_up = store.passive_checkpoint().expect("catch-up checkpoint");
    assert_eq!(caught_up.checkpointed_frames, caught_up.log_frames);
    assert_eq!(store.quick_check().expect("quick check"), "ok");
    let policy = store.connection_policy().expect("connection policy");
    assert_eq!(policy.journal_mode, "wal");
    assert_eq!(policy.synchronous, 1);
    assert_eq!(policy.wal_autocheckpoint_pages, 1_000);
    assert_eq!(store.events_after(0, 100).expect("all events").len(), 20);
}

#[test]
fn actor_checkpoint_flushes_pending_event_and_runs_on_the_writer_thread() {
    let database = TestDatabase::new("actor-ordering");
    let writer = database.writer();
    let handle = writer.handle();
    let event_receipt = handle
        .submit(event("event-before-checkpoint", 1))
        .expect("submit pending normal event");
    let checkpoint_receipt = handle.checkpoint_idle().expect("queue idle checkpoint");

    let event_ack = event_receipt
        .wait_timeout(Duration::from_secs(2))
        .expect("event durable before checkpoint");
    let checkpoint_ack = checkpoint_receipt
        .wait_timeout(Duration::from_secs(2))
        .expect("checkpoint report");
    assert_eq!(inserted(event_ack.outcome), 1);
    assert_eq!(checkpoint_ack.writer_thread_name, EVENT_WRITER_THREAD_NAME);
    assert!(checkpoint_ack.report.busy >= 0);
    assert!(checkpoint_ack.report.log_frames >= 0);
    assert!(checkpoint_ack.report.checkpointed_frames <= checkpoint_ack.report.log_frames);
    writer.shutdown().expect("shutdown writer");
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("durable event")
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        ["event-before-checkpoint"]
    );
    assert!(matches!(
        handle.checkpoint_idle(),
        Err(CheckpointFailure::WriterClosed)
    ));
}

#[test]
fn checkpoint_timeout_does_not_cancel_command_or_poison_writer() {
    let database = TestDatabase::new("timeout-isolation");
    let writer = database.writer();
    let handle = writer.handle();
    let lock = hold_write_lock(database.path());
    let event_receipt = handle
        .submit(event("event-before-timeout", 1))
        .expect("submit blocked event");
    let checkpoint_receipt = handle.checkpoint_idle().expect("queue checkpoint");

    assert!(matches!(
        checkpoint_receipt.wait_timeout(Duration::from_millis(10)),
        Err(CheckpointFailure::TimedOut)
    ));
    release_write_lock(lock);
    assert_eq!(
        inserted(
            event_receipt
                .wait_timeout(Duration::from_secs(2))
                .expect("preceding event commits")
                .outcome
        ),
        1
    );
    let checkpoint = checkpoint_receipt
        .wait_timeout(Duration::from_secs(2))
        .expect("same checkpoint receipt completes");
    assert_eq!(checkpoint.writer_thread_name, EVENT_WRITER_THREAD_NAME);
    let second = handle
        .append(event("event-after-timeout", 2))
        .expect("writer remains usable");
    assert_eq!(inserted(second.outcome), 2);
    writer.shutdown().expect("shutdown writer");
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("both events durable")
            .len(),
        2
    );
}

fn inserted(outcome: AppendEventOutcome) -> u64 {
    match outcome {
        AppendEventOutcome::Inserted(event) => event.ingest_seq,
        AppendEventOutcome::Duplicate(_) => panic!("expected inserted event"),
    }
}

fn event(event_id: &str, stream_seq: u64) -> UnsequencedEventEnvelope {
    UnsequencedEventEnvelope {
        protocol_version: EventProtocolVersion::V1_0,
        event_id: event_id.to_owned(),
        run_id: RUN_ID.to_owned(),
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

fn seed_registry(path: &Path) {
    let connection = Connection::open(path).expect("seed connection");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, 'Checkpoints', '/private/tmp/flit-checkpoints', 1, '{}', ?2, ?2)",
            params![PROJECT_ID, APPLIED_AT],
        )
        .expect("seed project");
    connection
        .execute(
            "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES(?1, ?2, 'Checkpoint Run', 'codex', '{}', ?3)",
            params![RUN_ID, PROJECT_ID, APPLIED_AT],
        )
        .expect("seed run");
}

fn hold_write_lock(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open competing writer");
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold SQLite write lock");
    connection
}

fn release_write_lock(connection: Connection) {
    connection
        .execute_batch("ROLLBACK")
        .expect("release SQLite write lock");
}
