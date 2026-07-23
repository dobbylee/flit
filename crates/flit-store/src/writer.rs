use std::{
    error::Error,
    fmt, io,
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use flit_protocol::UnsequencedEventEnvelope;

use crate::{AppendEventOutcome, CheckpointReport, MAX_EVENT_APPEND_BATCH, Store, StoreError};

pub const EVENT_WRITER_QUEUE_CAPACITY: usize = 1_000;
pub const EVENT_WRITER_THREAD_NAME: &str = "flit-store-writer";
pub const NORMAL_EVENT_BATCH_WAIT: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventCommitPriority {
    Normal,
    Urgent,
}

pub fn event_commit_priority(event_type: &str) -> EventCommitPriority {
    match event_type {
        "permission.requested"
        | "question.requested"
        | "run.completed"
        | "run.failed"
        | "run.stopped"
        | "run.interrupted"
        | "run.resume_failed" => EventCommitPriority::Urgent,
        _ => EventCommitPriority::Normal,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DurableEventAck {
    pub outcome: AppendEventOutcome,
    pub commit_group: u64,
    pub group_size: usize,
    pub priority: EventCommitPriority,
    pub writer_thread_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointAck {
    pub report: CheckpointReport,
    pub writer_thread_name: String,
}

#[derive(Clone, Debug)]
pub enum CheckpointFailure {
    Store(Arc<StoreError>),
    WriterClosed,
    TimedOut,
}

impl fmt::Display for CheckpointFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "PASSIVE checkpoint failed: {error}"),
            Self::WriterClosed => formatter.write_str("event writer is closed"),
            Self::TimedOut => formatter.write_str("checkpoint receipt timed out"),
        }
    }
}

impl Error for CheckpointFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error.as_ref()),
            Self::WriterClosed | Self::TimedOut => None,
        }
    }
}

pub struct CheckpointReceipt {
    receiver: Receiver<Result<CheckpointAck, CheckpointFailure>>,
}

impl CheckpointReceipt {
    pub fn wait(self) -> Result<CheckpointAck, CheckpointFailure> {
        self.receiver
            .recv()
            .map_err(|_| CheckpointFailure::WriterClosed)?
    }

    pub fn wait_timeout(&self, timeout: Duration) -> Result<CheckpointAck, CheckpointFailure> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(CheckpointFailure::TimedOut),
            Err(RecvTimeoutError::Disconnected) => Err(CheckpointFailure::WriterClosed),
        }
    }
}

#[derive(Clone, Debug)]
pub enum EventWriteFailure {
    Store(Arc<StoreError>),
    WriterClosed,
    TimedOut,
    OutcomeCount { expected: usize, actual: usize },
}

impl fmt::Display for EventWriteFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "event store write failed: {error}"),
            Self::WriterClosed => formatter.write_str("event writer is closed"),
            Self::TimedOut => formatter.write_str("event writer receipt timed out"),
            Self::OutcomeCount { expected, actual } => write!(
                formatter,
                "event writer received {actual} outcomes for {expected} submissions"
            ),
        }
    }
}

impl Error for EventWriteFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error.as_ref()),
            Self::WriterClosed | Self::TimedOut | Self::OutcomeCount { .. } => None,
        }
    }
}

pub struct EventWriteReceipt {
    receiver: Receiver<Result<DurableEventAck, EventWriteFailure>>,
}

impl EventWriteReceipt {
    pub fn wait(self) -> Result<DurableEventAck, EventWriteFailure> {
        self.receiver
            .recv()
            .map_err(|_| EventWriteFailure::WriterClosed)?
    }

    pub fn wait_timeout(self, timeout: Duration) -> Result<DurableEventAck, EventWriteFailure> {
        // Timing out only stops this wait; it does not cancel the queued durable write.
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(EventWriteFailure::TimedOut),
            Err(RecvTimeoutError::Disconnected) => Err(EventWriteFailure::WriterClosed),
        }
    }
}

#[derive(Clone)]
pub struct EventWriterHandle {
    sender: SyncSender<WriterCommand>,
}

impl EventWriterHandle {
    pub fn submit(
        &self,
        event: UnsequencedEventEnvelope,
    ) -> Result<EventWriteReceipt, EventWriteFailure> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterCommand::Append(Box::new(Submission {
                event,
                reply,
                submitted_at: Instant::now(),
            })))
            .map_err(|_| EventWriteFailure::WriterClosed)?;
        Ok(EventWriteReceipt { receiver })
    }

    pub fn append(
        &self,
        event: UnsequencedEventEnvelope,
    ) -> Result<DurableEventAck, EventWriteFailure> {
        self.submit(event)?.wait()
    }

    pub fn checkpoint_idle(&self) -> Result<CheckpointReceipt, CheckpointFailure> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterCommand::Checkpoint(reply))
            .map_err(|_| CheckpointFailure::WriterClosed)?;
        Ok(CheckpointReceipt { receiver })
    }
}

pub struct EventWriter {
    handle: EventWriterHandle,
    join: Option<JoinHandle<()>>,
}

impl EventWriter {
    pub fn start(
        path: impl AsRef<Path>,
        migration_applied_at: &str,
    ) -> Result<Self, EventWriterStartError> {
        let path = path.as_ref().to_owned();
        let migration_applied_at = migration_applied_at.to_owned();
        let (sender, receiver) = mpsc::sync_channel(EVENT_WRITER_QUEUE_CAPACITY);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name(EVENT_WRITER_THREAD_NAME.to_owned())
            .spawn(move || match Store::open(path, &migration_applied_at) {
                Ok(store) => {
                    if startup_sender.send(Ok(())).is_ok() {
                        run_writer(store, receiver);
                    }
                }
                Err(error) => {
                    let _ = startup_sender.send(Err(error));
                }
            })
            .map_err(EventWriterStartError::Spawn)?;

        match startup_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                handle: EventWriterHandle { sender },
                join: Some(join),
            }),
            Ok(Err(error)) => {
                join.join()
                    .map_err(|_| EventWriterStartError::WorkerPanicked)?;
                Err(EventWriterStartError::Store(error))
            }
            Err(_) => {
                join.join()
                    .map_err(|_| EventWriterStartError::WorkerPanicked)?;
                Err(EventWriterStartError::StartupChannelClosed)
            }
        }
    }

    pub fn handle(&self) -> EventWriterHandle {
        self.handle.clone()
    }

    pub fn shutdown(mut self) -> Result<(), EventWriterShutdownError> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> Result<(), EventWriterShutdownError> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let send_result = self.sender().send(WriterCommand::Shutdown(reply));
        let acknowledgement = if send_result.is_ok() {
            receiver
                .recv()
                .map_err(|_| EventWriterShutdownError::WorkerClosed)
        } else {
            Err(EventWriterShutdownError::WorkerClosed)
        };
        join.join()
            .map_err(|_| EventWriterShutdownError::WorkerPanicked)?;
        if send_result.is_err() {
            return Err(EventWriterShutdownError::WorkerClosed);
        }
        acknowledgement
    }

    fn sender(&self) -> &SyncSender<WriterCommand> {
        &self.handle.sender
    }
}

impl Drop for EventWriter {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

#[derive(Debug)]
pub enum EventWriterStartError {
    Spawn(io::Error),
    Store(StoreError),
    StartupChannelClosed,
    WorkerPanicked,
}

impl fmt::Display for EventWriterStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "failed to spawn event writer: {error}"),
            Self::Store(error) => write!(formatter, "failed to open event store: {error}"),
            Self::StartupChannelClosed => {
                formatter.write_str("event writer startup channel closed")
            }
            Self::WorkerPanicked => formatter.write_str("event writer panicked during startup"),
        }
    }
}

impl Error for EventWriterStartError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Spawn(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::StartupChannelClosed | Self::WorkerPanicked => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventWriterShutdownError {
    WorkerClosed,
    WorkerPanicked,
}

impl fmt::Display for EventWriterShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerClosed => formatter.write_str("event writer closed before shutdown ack"),
            Self::WorkerPanicked => formatter.write_str("event writer panicked during shutdown"),
        }
    }
}

impl Error for EventWriterShutdownError {}

struct Submission {
    event: UnsequencedEventEnvelope,
    reply: SyncSender<Result<DurableEventAck, EventWriteFailure>>,
    submitted_at: Instant,
}

enum WriterCommand {
    Append(Box<Submission>),
    Checkpoint(SyncSender<Result<CheckpointAck, CheckpointFailure>>),
    Shutdown(SyncSender<()>),
    #[cfg(test)]
    Panic,
    #[cfg(test)]
    CheckpointFailure(SyncSender<Result<CheckpointAck, CheckpointFailure>>),
}

fn run_writer(mut store: Store, receiver: Receiver<WriterCommand>) {
    let mut next_commit_group = 1_u64;
    loop {
        match receiver.recv() {
            Ok(WriterCommand::Append(submission)) => {
                if event_commit_priority(&submission.event.event_type)
                    == EventCommitPriority::Urgent
                {
                    flush_group(
                        &mut store,
                        vec![*submission],
                        EventCommitPriority::Urgent,
                        &mut next_commit_group,
                    );
                } else if !collect_normal_group(
                    &mut store,
                    *submission,
                    &receiver,
                    &mut next_commit_group,
                ) {
                    return;
                }
            }
            Ok(WriterCommand::Checkpoint(reply)) => run_checkpoint(&mut store, reply),
            Ok(WriterCommand::Shutdown(reply)) => {
                let _ = reply.send(());
                return;
            }
            #[cfg(test)]
            Ok(WriterCommand::Panic) => panic!("injected event writer failure"),
            #[cfg(test)]
            Ok(WriterCommand::CheckpointFailure(reply)) => run_injected_checkpoint_failure(reply),
            Err(_) => return,
        }
    }
}

fn collect_normal_group(
    store: &mut Store,
    first: Submission,
    receiver: &Receiver<WriterCommand>,
    next_commit_group: &mut u64,
) -> bool {
    let deadline = first.submitted_at + NORMAL_EVENT_BATCH_WAIT;
    let mut pending = Vec::with_capacity(MAX_EVENT_APPEND_BATCH);
    pending.push(first);

    loop {
        if pending.len() == MAX_EVENT_APPEND_BATCH {
            flush_group(
                store,
                pending,
                EventCommitPriority::Normal,
                next_commit_group,
            );
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(WriterCommand::Append(submission)) => {
                if event_commit_priority(&submission.event.event_type)
                    == EventCommitPriority::Urgent
                {
                    flush_group(
                        store,
                        pending,
                        EventCommitPriority::Normal,
                        next_commit_group,
                    );
                    flush_group(
                        store,
                        vec![*submission],
                        EventCommitPriority::Urgent,
                        next_commit_group,
                    );
                    return true;
                }
                pending.push(*submission);
            }
            Ok(WriterCommand::Shutdown(reply)) => {
                flush_group(
                    store,
                    pending,
                    EventCommitPriority::Normal,
                    next_commit_group,
                );
                let _ = reply.send(());
                return false;
            }
            Ok(WriterCommand::Checkpoint(reply)) => {
                flush_group(
                    store,
                    pending,
                    EventCommitPriority::Normal,
                    next_commit_group,
                );
                run_checkpoint(store, reply);
                return true;
            }
            #[cfg(test)]
            Ok(WriterCommand::Panic) => panic!("injected event writer failure"),
            #[cfg(test)]
            Ok(WriterCommand::CheckpointFailure(reply)) => {
                flush_group(
                    store,
                    pending,
                    EventCommitPriority::Normal,
                    next_commit_group,
                );
                run_injected_checkpoint_failure(reply);
                return true;
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_group(
                    store,
                    pending,
                    EventCommitPriority::Normal,
                    next_commit_group,
                );
                return true;
            }
            Err(RecvTimeoutError::Disconnected) => {
                flush_group(
                    store,
                    pending,
                    EventCommitPriority::Normal,
                    next_commit_group,
                );
                return false;
            }
        }
    }
}

fn run_checkpoint(store: &mut Store, reply: SyncSender<Result<CheckpointAck, CheckpointFailure>>) {
    let result = store.passive_checkpoint().map_or_else(
        |error| Err(CheckpointFailure::Store(Arc::new(error))),
        |report| {
            Ok(CheckpointAck {
                report,
                writer_thread_name: thread::current().name().unwrap_or_default().to_owned(),
            })
        },
    );
    let _ = reply.send(result);
}

#[cfg(test)]
fn run_injected_checkpoint_failure(reply: SyncSender<Result<CheckpointAck, CheckpointFailure>>) {
    let report = CheckpointReport {
        busy: 0,
        log_frames: -1,
        checkpointed_frames: 0,
    };
    let _ = reply.send(Err(CheckpointFailure::Store(Arc::new(
        StoreError::InvalidCheckpointReport(report),
    ))));
}

fn flush_group(
    store: &mut Store,
    submissions: Vec<Submission>,
    priority: EventCommitPriority,
    next_commit_group: &mut u64,
) {
    let commit_group = *next_commit_group;
    *next_commit_group = next_commit_group.saturating_add(1);
    let group_size = submissions.len();
    let (events, replies): (Vec<_>, Vec<_>) = submissions
        .into_iter()
        .map(|submission| (submission.event, submission.reply))
        .unzip();
    match store.append_event_batch(events) {
        Ok(outcomes) if outcomes.len() == group_size => {
            for (outcome, reply) in outcomes.into_iter().zip(replies) {
                let _ = reply.send(Ok(DurableEventAck {
                    outcome,
                    commit_group,
                    group_size,
                    priority,
                    writer_thread_name: thread::current().name().unwrap_or_default().to_owned(),
                }));
            }
        }
        Ok(outcomes) => {
            let failure = EventWriteFailure::OutcomeCount {
                expected: group_size,
                actual: outcomes.len(),
            };
            for reply in replies {
                let _ = reply.send(Err(failure.clone()));
            }
        }
        Err(error) => {
            let failure = EventWriteFailure::Store(Arc::new(error));
            for reply in replies {
                let _ = reply.send(Err(failure.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use flit_protocol::{
        EventEnvelope, EventSourceKind, NullableSessionId, UnsequencedEventEnvelope,
    };
    use rusqlite::{Connection, params};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("flit-event-writer-panic-{}-{nonce}", process::id()));
            fs::create_dir(&path).expect("unique test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!(
                    "failed to remove test directory {}: {error}",
                    self.0.display()
                );
            }
        }
    }

    #[test]
    fn worker_panic_closes_outstanding_receipt_and_is_reported_by_shutdown() {
        let directory = TestDirectory::new();
        let path = directory.0.join("flit.sqlite3");
        let writer =
            EventWriter::start(&path, "2026-07-23T00:00:00.000Z").expect("start event writer");
        let mut event: UnsequencedEventEnvelope = serde_json::from_str::<EventEnvelope>(
            include_str!("../../../fixtures/protocol/events/v1.0/permission.requested.json"),
        )
        .map(UnsequencedEventEnvelope::from)
        .expect("event fixture");
        event.event_type = "run.event_observed".to_owned();
        let receipt = writer.handle().submit(event).expect("submit normal event");
        writer
            .sender()
            .send(WriterCommand::Panic)
            .expect("inject worker panic");

        assert!(matches!(
            receipt.wait_timeout(Duration::from_secs(2)),
            Err(EventWriteFailure::WriterClosed)
        ));
        assert!(matches!(
            writer.shutdown(),
            Err(EventWriterShutdownError::WorkerPanicked)
        ));
    }

    #[test]
    fn checkpoint_failure_is_isolated_between_durable_event_writes() {
        let directory = TestDirectory::new();
        let path = directory.0.join("flit.sqlite3");
        let store = Store::open(&path, "2026-07-23T00:00:00.000Z").expect("bootstrap store");
        drop(store);
        let connection = Connection::open(&path).expect("seed connection");
        connection
            .execute(
                "INSERT INTO projects(id, display_name, canonical_path, trusted, notification_policy_json, created_at, updated_at) VALUES('project-writer-unit', 'Writer Unit', '/private/tmp/flit-writer-unit', 1, '{}', ?1, ?1)",
                ["2026-07-23T00:00:00.000Z"],
            )
            .expect("seed project");
        connection
            .execute(
                "INSERT INTO runs(id, project_id, title, provider_kind, start_request_json, created_at) VALUES(?1, 'project-writer-unit', 'Writer Unit', 'codex', '{}', ?2)",
                params!["run-writer-unit", "2026-07-23T00:00:00.000Z"],
            )
            .expect("seed run");
        drop(connection);

        let writer =
            EventWriter::start(&path, "2026-07-23T00:00:00.000Z").expect("start event writer");
        let handle = writer.handle();
        let first = handle
            .submit(normal_event("event-before-checkpoint-failure", 1))
            .expect("submit first event");
        let (reply, receiver) = mpsc::sync_channel(1);
        writer
            .sender()
            .send(WriterCommand::CheckpointFailure(reply))
            .expect("inject checkpoint failure");
        let checkpoint = CheckpointReceipt { receiver };

        assert_eq!(
            first
                .wait_timeout(Duration::from_secs(2))
                .map(|ack| match ack.outcome {
                    AppendEventOutcome::Inserted(event) => event.ingest_seq,
                    AppendEventOutcome::Duplicate(_) => 0,
                })
                .expect("first event remains durable"),
            1
        );
        assert!(matches!(
            checkpoint.wait_timeout(Duration::from_secs(2)),
            Err(CheckpointFailure::Store(error))
                if matches!(error.as_ref(), StoreError::InvalidCheckpointReport(_))
        ));
        let second = handle
            .append(normal_event("event-after-checkpoint-failure", 2))
            .expect("writer remains usable");
        assert!(matches!(
            second.outcome,
            AppendEventOutcome::Inserted(ref event) if event.ingest_seq == 2
        ));
        writer.shutdown().expect("shutdown writer");
        assert_eq!(
            Store::open(&path, "2026-07-23T00:00:00.000Z")
                .expect("reopen store")
                .events_after(0, 10)
                .expect("durable events")
                .len(),
            2
        );
    }

    fn normal_event(event_id: &str, stream_seq: u64) -> UnsequencedEventEnvelope {
        let mut event: UnsequencedEventEnvelope = serde_json::from_str::<EventEnvelope>(
            include_str!("../../../fixtures/protocol/events/v1.0/permission.requested.json"),
        )
        .map(UnsequencedEventEnvelope::from)
        .expect("event fixture");
        event.event_id = event_id.to_owned();
        event.run_id = "run-writer-unit".to_owned();
        event.session_id = NullableSessionId::Null;
        event.stream_seq = stream_seq;
        event.event_type = "run.event_observed".to_owned();
        event.source.kind = EventSourceKind::Core;
        event.source.provider = None;
        event.source.contract_version = None;
        event.evidence_ids.clear();
        event
    }
}
