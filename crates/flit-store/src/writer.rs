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

use crate::{AppendEventOutcome, MAX_EVENT_APPEND_BATCH, Store, StoreError};

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
    Shutdown(SyncSender<()>),
    #[cfg(test)]
    Panic,
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
            Ok(WriterCommand::Shutdown(reply)) => {
                let _ = reply.send(());
                return;
            }
            #[cfg(test)]
            Ok(WriterCommand::Panic) => panic!("injected event writer failure"),
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
            #[cfg(test)]
            Ok(WriterCommand::Panic) => panic!("injected event writer failure"),
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

    use flit_protocol::{EventEnvelope, UnsequencedEventEnvelope};

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
}
