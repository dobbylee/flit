use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use flit_protocol::{EventEnvelope, NullableSessionId, UnsequencedEventEnvelope};
use flit_store::{
    AppendEventOutcome, EVENT_WRITER_QUEUE_CAPACITY, EVENT_WRITER_THREAD_NAME, EventCommitPriority,
    EventWriteFailure, EventWriteReceipt, EventWriter, EventWriterStartError,
    NORMAL_EVENT_BATCH_WAIT, Store, StoreError, event_commit_priority,
};
use rusqlite::{Connection, params};

const APPLIED_AT: &str = "2026-07-23T00:00:00.000Z";
const PROJECT_ID: &str = "project-event-writer";
const RUN_ID: &str = "run_01JZ8Y60R5M6V3Y2S0VJ3G8K1C";
const SESSION_ID: &str = "ses_01JZ8Y62E8FVDMZ00HBFV3N6XP";
const EVIDENCE_ID: &str = "evd_01JZ8Y6A7S6KZ0B2E9WP76M44X";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "flit-event-writer-{label}-{}-{nonce}",
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

    fn writer(&self) -> EventWriter {
        EventWriter::start(&self.path, APPLIED_AT).expect("start writer")
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Store {
        Store::open(&self.path, APPLIED_AT).expect("open store")
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
fn one_normal_submission_flushes_on_the_fixed_deadline_after_durable_commit() {
    let database = TestDatabase::new("deadline");
    let writer = database.writer();
    let handle = writer.handle();
    let first_event = event("event-deadline", 1, "run.event_observed");
    let started = Instant::now();
    let receipt = handle.submit(first_event).expect("submit normal event");
    let ack = wait(receipt).expect("normal durable ack");
    let elapsed = started.elapsed();

    assert!(elapsed >= Duration::from_millis(10), "elapsed: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "elapsed: {elapsed:?}");
    assert_eq!(NORMAL_EVENT_BATCH_WAIT, Duration::from_millis(20));
    assert_eq!(ack.priority, EventCommitPriority::Normal);
    assert_eq!(ack.group_size, 1);
    assert_eq!(ack.writer_thread_name, EVENT_WRITER_THREAD_NAME);
    assert_eq!(outcome_seq(&ack.outcome), 1);
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("durable row")
            .len(),
        1
    );
    writer.shutdown().expect("shutdown writer");
}

#[test]
fn trickle_arrivals_do_not_extend_the_first_normal_deadline() {
    let database = TestDatabase::new("fixed-deadline");
    let writer = database.writer();
    let handle = writer.handle();
    let mut receipts = vec![
        handle
            .submit(event("event-trickle-1", 1, "run.event_observed"))
            .expect("submit first trickle event"),
    ];
    for index in 2..=8_u64 {
        thread::sleep(Duration::from_millis(5));
        receipts.push(
            handle
                .submit(event(
                    &format!("event-trickle-{index}"),
                    index,
                    "run.event_observed",
                ))
                .expect("submit later trickle event"),
        );
    }

    let acknowledgements = receipts
        .into_iter()
        .map(|receipt| wait(receipt).expect("trickle durable ack"))
        .collect::<Vec<_>>();
    assert!(acknowledgements[0].group_size < acknowledgements.len());
    assert!(
        acknowledgements
            .iter()
            .any(|ack| ack.commit_group != acknowledgements[0].commit_group)
    );
    assert_eq!(
        acknowledgements
            .iter()
            .map(|ack| outcome_seq(&ack.outcome))
            .collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
    writer.shutdown().expect("shutdown writer");
}

#[test]
fn fifty_normal_submissions_share_one_commit_group_across_cloned_handles() {
    let database = TestDatabase::new("batch-size");
    let writer = database.writer();
    let first_handle = writer.handle();
    let second_handle = first_handle.clone();
    let mut receipts = Vec::new();
    for index in 1..=50_u64 {
        let handle = if index % 2 == 0 {
            &first_handle
        } else {
            &second_handle
        };
        receipts.push(
            handle
                .submit(event(
                    &format!("event-batch-{index}"),
                    index,
                    "run.event_observed",
                ))
                .expect("submit batched event"),
        );
    }

    let acknowledgements = receipts
        .into_iter()
        .map(|receipt| wait(receipt).expect("batched durable ack"))
        .collect::<Vec<_>>();
    let commit_group = acknowledgements[0].commit_group;
    for (index, ack) in acknowledgements.iter().enumerate() {
        assert_eq!(ack.commit_group, commit_group);
        assert_eq!(ack.group_size, 50);
        assert_eq!(ack.priority, EventCommitPriority::Normal);
        assert_eq!(outcome_seq(&ack.outcome), index as u64 + 1);
        assert_eq!(ack.writer_thread_name, EVENT_WRITER_THREAD_NAME);
    }
    assert_eq!(
        database
            .open()
            .events_after(0, 100)
            .expect("durable batch")
            .len(),
        50
    );
    writer.shutdown().expect("shutdown writer");
}

#[test]
fn urgent_arrival_flushes_pending_normal_then_commits_separately() {
    let database = TestDatabase::new("urgent");
    let writer = database.writer();
    let handle = writer.handle();
    let normal = handle
        .submit(event("event-normal", 1, "run.event_observed"))
        .expect("submit normal");
    let urgent = handle
        .submit(event("event-urgent", 2, "permission.requested"))
        .expect("submit urgent");

    let normal_ack = wait(normal).expect("normal durable ack");
    let urgent_ack = wait(urgent).expect("urgent durable ack");
    assert_eq!(normal_ack.priority, EventCommitPriority::Normal);
    assert_eq!(urgent_ack.priority, EventCommitPriority::Urgent);
    assert_eq!(normal_ack.group_size, 1);
    assert_eq!(urgent_ack.group_size, 1);
    assert_eq!(urgent_ack.commit_group, normal_ack.commit_group + 1);
    assert_eq!(outcome_seq(&normal_ack.outcome), 1);
    assert_eq!(outcome_seq(&urgent_ack.outcome), 2);
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("ordered durable rows")
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        ["event-normal", "event-urgent"]
    );
    writer.shutdown().expect("shutdown writer");
}

#[test]
fn urgent_commit_is_attempted_after_pending_normal_failure() {
    let database = TestDatabase::new("urgent-after-failure");
    let writer = database.writer();
    let handle = writer.handle();
    let normal = handle
        .submit(event(" ", 1, "run.event_observed"))
        .expect("submit invalid normal");
    let urgent = handle
        .submit(event("event-urgent", 1, "question.requested"))
        .expect("submit urgent");

    assert!(matches!(
        wait(normal),
        Err(EventWriteFailure::Store(error))
            if matches!(error.as_ref(), StoreError::InvalidEvent { field: "event_id" })
    ));
    let urgent_ack = wait(urgent).expect("urgent still commits");
    assert_eq!(urgent_ack.priority, EventCommitPriority::Urgent);
    assert_eq!(outcome_seq(&urgent_ack.outcome), 1);
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("only urgent row")
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        ["event-urgent"]
    );
    writer.shutdown().expect("shutdown writer");
}

#[test]
fn timed_out_receipt_does_not_cancel_the_locked_write() {
    let database = TestDatabase::new("timeout-non-cancellation");
    let writer = database.writer();
    let handle = writer.handle();
    let lock = hold_write_lock(database.path());
    let receipt = handle
        .submit(event("event-timeout", 1, "permission.requested"))
        .expect("submit locked urgent event");

    assert!(matches!(
        receipt.wait_timeout(Duration::from_millis(10)),
        Err(EventWriteFailure::TimedOut)
    ));
    release_write_lock(lock);
    writer.shutdown().expect("join after releasing lock");
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("timed-out write became durable")
            .iter()
            .map(|event| (event.ingest_seq, event.event_id.as_str()))
            .collect::<Vec<_>>(),
        [(1, "event-timeout")]
    );
}

#[test]
fn bounded_queue_blocks_the_next_producer_at_capacity() {
    let database = TestDatabase::new("bounded-backpressure");
    let writer = database.writer();
    let handle = writer.handle();
    let lock = hold_write_lock(database.path());
    let urgent = handle
        .submit(event("event-blocked-urgent", 1, "permission.requested"))
        .expect("submit blocked urgent event");
    for index in 2..=EVENT_WRITER_QUEUE_CAPACITY as u64 {
        let _ = handle
            .submit(event(
                &format!("event-queued-{index}"),
                index,
                "run.event_observed",
            ))
            .expect("fill bounded queue");
    }

    let producer_handle = handle.clone();
    let (stage_sender, stage_receiver) = mpsc::sync_channel(2);
    let producer = thread::spawn(move || {
        let _ = producer_handle
            .submit(event(
                "event-queued-1001",
                EVENT_WRITER_QUEUE_CAPACITY as u64 + 1,
                "run.event_observed",
            ))
            .expect("fill final queue slot");
        stage_sender.send(1).expect("first producer stage");
        let _ = producer_handle
            .submit(event(
                "event-queued-1002",
                EVENT_WRITER_QUEUE_CAPACITY as u64 + 2,
                "run.event_observed",
            ))
            .expect("submit after backpressure");
        stage_sender.send(2).expect("second producer stage");
    });

    assert_eq!(
        stage_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("producer fills exact capacity"),
        1
    );
    assert!(matches!(
        stage_receiver.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release_write_lock(lock);
    assert_eq!(
        stage_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("producer resumes after capacity frees"),
        2
    );
    producer.join().expect("join producer");
    assert_eq!(
        outcome_seq(&wait(urgent).expect("blocked urgent ack").outcome),
        1
    );
    writer.shutdown().expect("drain queue and join writer");
    assert_eq!(
        database
            .open()
            .latest_ingest_seq()
            .expect("all queued writes"),
        EVENT_WRITER_QUEUE_CAPACITY as u64 + 2
    );
}

#[test]
fn shutdown_flushes_pending_normal_and_closes_every_handle() {
    let database = TestDatabase::new("shutdown");
    let writer = database.writer();
    let handle = writer.handle();
    let receipt = handle
        .submit(event("event-shutdown", 1, "run.event_observed"))
        .expect("submit pending normal");
    writer.shutdown().expect("flush and join");

    assert_eq!(
        outcome_seq(&wait(receipt).expect("shutdown durable ack").outcome),
        1
    );
    assert!(matches!(
        handle.submit(event("event-after-shutdown", 2, "run.event_observed")),
        Err(EventWriteFailure::WriterClosed)
    ));
    assert_eq!(
        database
            .open()
            .events_after(0, 10)
            .expect("shutdown row")
            .len(),
        1
    );
}

#[test]
fn urgent_type_matrix_and_startup_failure_are_typed() {
    for event_type in [
        "permission.requested",
        "question.requested",
        "run.completed",
        "run.failed",
        "run.stopped",
        "run.interrupted",
        "run.resume_failed",
    ] {
        assert_eq!(
            event_commit_priority(event_type),
            EventCommitPriority::Urgent,
            "event type: {event_type}"
        );
    }
    for event_type in ["run.created", "run.event_observed", "future.event"] {
        assert_eq!(
            event_commit_priority(event_type),
            EventCommitPriority::Normal,
            "event type: {event_type}"
        );
    }

    let directory = std::env::temp_dir().join(format!(
        "flit-event-writer-invalid-start-{}-{}",
        process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).expect("startup failure directory");
    let path = directory.join("flit.sqlite3");
    assert!(matches!(
        EventWriter::start(&path, ""),
        Err(EventWriterStartError::Store(
            StoreError::InvalidMigrationAppliedAt
        ))
    ));
    assert!(!path.exists());
    fs::remove_dir_all(directory).expect("remove startup failure directory");
}

fn outcome_seq(outcome: &AppendEventOutcome) -> u64 {
    match outcome {
        AppendEventOutcome::Inserted(event) | AppendEventOutcome::Duplicate(event) => {
            event.ingest_seq
        }
    }
}

fn wait(receipt: EventWriteReceipt) -> Result<flit_store::DurableEventAck, EventWriteFailure> {
    receipt.wait_timeout(Duration::from_secs(2))
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

fn event(event_id: &str, stream_seq: u64, event_type: &str) -> UnsequencedEventEnvelope {
    let event: EventEnvelope = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/events/v1.0/permission.requested.json"
    ))
    .expect("current event fixture");
    let mut event = UnsequencedEventEnvelope::from(event);
    event.event_id = event_id.to_owned();
    event.run_id = RUN_ID.to_owned();
    event.session_id = NullableSessionId::Id(SESSION_ID.to_owned());
    event.stream_seq = stream_seq;
    event.event_type = event_type.to_owned();
    event.evidence_ids = vec![EVIDENCE_ID.to_owned()];
    event
}

fn seed_registry(path: &Path) {
    let connection = Connection::open(path).expect("seed connection");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, 'Event Writer', '/private/tmp/flit-event-writer', 1, '{}', ?2, ?2)",
            params![PROJECT_ID, APPLIED_AT],
        )
        .expect("seed project");
    connection
        .execute(
            "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES(?1, ?2, 'Writer Run', 'codex', '{}', ?3)",
            params![RUN_ID, PROJECT_ID, APPLIED_AT],
        )
        .expect("seed run");
    connection
        .execute(
            "INSERT INTO agent_sessions(id, run_id, ordinal, provider_kind, external_session_key, session_fingerprint, cwd, capabilities_json, started_at) VALUES(?1, ?2, 1, 'codex', 'external-session', 'fixture-v1', '/private/tmp/flit-event-writer', '{}', ?3)",
            params![SESSION_ID, RUN_ID, APPLIED_AT],
        )
        .expect("seed session");
    connection
        .execute(
            "INSERT INTO evidence(id, run_id, session_id, kind, locator_json, created_at) VALUES(?1, ?2, ?3, 'provider_history', '{}', ?4)",
            params![EVIDENCE_ID, RUN_ID, SESSION_ID, APPLIED_AT],
        )
        .expect("seed evidence");
}
