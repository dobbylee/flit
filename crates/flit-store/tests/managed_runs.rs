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
    InitialManagedSessionConnection, InitialManagedSessionOutcome, MAX_LIVE_MANAGED_SESSIONS,
    ManagedReconciliationState, ManagedRunIntent, ManagedRunIntentOutcome,
    ManagedSessionReconciliation, ManagedSessionReconciliationOutcome, ManagedSessionTermination,
    ManagedSessionTerminationOutcome, ManagedTurnTerminalOutcome, ProjectRegistration,
    ProjectTrustConfirmation, Store, StoreError,
};
use serde_json::{Map, Value, json};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
const CREATED_AT: &str = "2026-07-24T10:00:00Z";
const STARTED_AT: &str = "2026-07-24T10:00:01Z";
const ENDED_AT: &str = "2026-07-24T10:05:00Z";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "flit-managed-runs-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn managed_run_and_exact_session_are_atomic_idempotent_and_reopen() {
    let directory = TestDirectory::new("reopen");
    let (mut store, database, project_path) = trusted_store(&directory);
    let intent = run_intent("run-1", "event-run-created", "event-start-requested");

    let created = store
        .create_managed_run_intent(intent.clone())
        .expect("create Run intent");
    let (created_run, created_events) = match created {
        ManagedRunIntentOutcome::Created { run, events } => (run, events),
        other => panic!("unexpected initial outcome: {other:?}"),
    };
    assert_eq!(created_run.started_at, None);
    assert_eq!(
        created_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        ["run.created", "run.start_requested"]
    );
    assert_eq!(
        created_events
            .iter()
            .map(|event| event.ingest_seq)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    let duplicate = store
        .create_managed_run_intent(intent)
        .expect("duplicate Run intent");
    assert!(matches!(
        duplicate,
        ManagedRunIntentOutcome::Duplicate { ref events, .. }
            if events.iter().map(|event| event.ingest_seq).collect::<Vec<_>>() == [1, 2]
    ));
    for (created_event_id, requested_event_id) in [
        ("event-run-created-retry", "event-start-requested"),
        (
            "event-run-created-retry-both",
            "event-start-requested-retry-both",
        ),
    ] {
        assert!(matches!(
            store.create_managed_run_intent(run_intent(
                "run-1",
                created_event_id,
                requested_event_id
            )),
            Err(StoreError::ManagedRunIdentityConflict { .. })
        ));
        assert_eq!(store.latest_ingest_seq().expect("retry cursor"), 2);
        assert_eq!(
            store
                .run_events_through("run-1", 0, 2, 10)
                .expect("original Run events")
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-run-created", "event-start-requested"]
        );
    }

    let connection = session_connection("session-1", "run-1", "codex-thread-1", &project_path);
    let connected = store
        .connect_initial_managed_session(connection.clone())
        .expect("connect initial session");
    let (session, connected_event) = match connected {
        InitialManagedSessionOutcome::Connected { session, event } => (session, event),
        other => panic!("unexpected session outcome: {other:?}"),
    };
    assert_eq!(session.ordinal, 1);
    assert_eq!(session.external_session_key, "codex-thread-1");
    assert_eq!(connected_event.event_type, "session.connected");
    assert_eq!(connected_event.ingest_seq, 3);
    assert_eq!(
        store
            .managed_run("run-1")
            .expect("read started Run")
            .expect("Run"),
        flit_store::ManagedRun {
            started_at: Some(STARTED_AT.to_owned()),
            ..created_run
        }
    );
    assert!(matches!(
        store
            .connect_initial_managed_session(connection)
            .expect("duplicate session"),
        InitialManagedSessionOutcome::Duplicate { event, .. } if event.ingest_seq == 3
    ));

    drop(store);
    let reopened = Store::open(&database, CREATED_AT).expect("reopen Store");
    assert_eq!(
        reopened
            .managed_session("session-1")
            .expect("reopened session"),
        Some(session)
    );
    assert_eq!(
        reopened
            .managed_run("run-1")
            .expect("reopened Run")
            .expect("Run")
            .started_at
            .as_deref(),
        Some(STARTED_AT)
    );
    assert_eq!(
        reopened
            .run_events_through("run-1", 0, 3, 10)
            .expect("reopened event page")
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        ["run.created", "run.start_requested", "session.connected"]
    );
}

#[test]
fn untrusted_archived_and_oversized_intents_fail_before_mutation() {
    let directory = TestDirectory::new("project-gates");
    let database = directory.0.join("flit.sqlite3");
    let project_path = directory.0.join("project");
    fs::create_dir(&project_path).expect("Project directory");
    let mut store = Store::open(&database, CREATED_AT).expect("open Store");
    register_project(&mut store, &project_path, "project-1");
    assert!(matches!(
        store.create_managed_run_intent(run_intent(
            "run-untrusted",
            "event-untrusted-created",
            "event-untrusted-requested"
        )),
        Err(StoreError::UntrustedProject { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("empty event cursor"), 0);
    assert_eq!(
        store
            .managed_run("run-untrusted")
            .expect("missing untrusted Run"),
        None
    );

    trust_project(&mut store, &project_path, "project-1");
    let mut oversized = run_intent(
        "run-oversized",
        "event-oversized-created",
        "event-oversized-requested",
    );
    oversized
        .start_request
        .insert("large".to_owned(), Value::String("x".repeat(256 * 1024)));
    assert!(matches!(
        store.create_managed_run_intent(oversized),
        Err(StoreError::InvalidManagedRunIntent {
            field: "start_request"
        })
    ));
    assert_eq!(store.latest_ingest_seq().expect("still empty"), 0);

    let mut over_depth = run_intent(
        "run-over-depth",
        "event-depth-created",
        "event-depth-requested",
    );
    over_depth
        .start_request
        .insert("nested".to_owned(), nested_value(33));
    assert!(matches!(
        store.create_managed_run_intent(over_depth),
        Err(StoreError::InvalidManagedRunIntent {
            field: "start_request"
        })
    ));
    assert_eq!(
        store
            .managed_run("run-over-depth")
            .expect("missing over-depth Run"),
        None
    );
    assert_eq!(store.latest_ingest_seq().expect("depth cursor"), 0);

    store
        .create_managed_run_intent(run_intent(
            "run-session-depth",
            "event-session-depth-created",
            "event-session-depth-requested",
        ))
        .expect("Run for session depth");
    let canonical_project_path = store
        .project("project-1")
        .expect("read Project")
        .expect("Project")
        .canonical_path;
    let mut over_depth_session = session_connection(
        "session-over-depth",
        "run-session-depth",
        "thread-over-depth",
        &canonical_project_path,
    );
    over_depth_session
        .capabilities
        .insert("nested".to_owned(), nested_value(33));
    assert!(matches!(
        store.connect_initial_managed_session(over_depth_session),
        Err(StoreError::InvalidInitialManagedSession {
            field: "capabilities"
        })
    ));
    assert_eq!(
        store
            .managed_session("session-over-depth")
            .expect("missing over-depth session"),
        None
    );
    assert_eq!(
        store
            .managed_run("run-session-depth")
            .expect("unstarted depth Run")
            .expect("Run")
            .started_at,
        None
    );
    assert_eq!(store.latest_ingest_seq().expect("session depth cursor"), 2);

    drop(store);
    let raw = rusqlite::Connection::open(&database).expect("raw database");
    raw.execute(
        "UPDATE projects SET archived_at = ?1 WHERE id = 'project-1'",
        [STARTED_AT],
    )
    .expect("archive Project");
    drop(raw);
    let mut store = Store::open(&database, CREATED_AT).expect("reopen archived Store");
    assert!(matches!(
        store.create_managed_run_intent(run_intent(
            "run-archived",
            "event-archived-created",
            "event-archived-requested"
        )),
        Err(StoreError::ArchivedProject { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("archived cursor"), 2);
}

#[test]
fn late_event_conflicts_roll_back_run_and_session_rows() {
    let directory = TestDirectory::new("rollback");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-existing",
            "event-existing-created",
            "event-existing-requested",
        ))
        .expect("existing Run");
    let cursor = store.latest_ingest_seq().expect("existing cursor");

    let conflicting = run_intent(
        "run-rollback",
        "event-new-before-conflict",
        "event-existing-created",
    );
    assert!(matches!(
        store.create_managed_run_intent(conflicting),
        Err(StoreError::EventIdentityConflict { .. })
    ));
    assert_eq!(
        store.managed_run("run-rollback").expect("rolled back Run"),
        None
    );
    assert_eq!(store.latest_ingest_seq().expect("unchanged cursor"), cursor);

    let mut connection = session_connection(
        "session-rollback",
        "run-existing",
        "thread-rollback",
        &project_path,
    );
    connection.connected_event_id = "event-existing-created".to_owned();
    assert!(matches!(
        store.connect_initial_managed_session(connection),
        Err(StoreError::EventIdentityConflict { .. })
    ));
    assert_eq!(
        store
            .managed_session("session-rollback")
            .expect("rolled back session"),
        None
    );
    assert_eq!(
        store
            .managed_run("run-existing")
            .expect("unstated Run")
            .expect("Run")
            .started_at,
        None
    );
    assert_eq!(
        store.latest_ingest_seq().expect("session rollback cursor"),
        cursor
    );
}

#[test]
fn external_identity_cwd_live_session_and_retry_conflicts_fail_closed() {
    let directory = TestDirectory::new("ownership");
    let (mut store, _database, project_path) = trusted_store(&directory);
    for (run, created, requested) in [
        ("run-1", "event-run-1-created", "event-run-1-requested"),
        ("run-2", "event-run-2-created", "event-run-2-requested"),
    ] {
        store
            .create_managed_run_intent(run_intent(run, created, requested))
            .expect("managed Run");
    }
    let connection = session_connection("session-1", "run-1", "thread-shared", &project_path);
    store
        .connect_initial_managed_session(connection.clone())
        .expect("initial session");

    let mut identity_conflict = connection.clone();
    identity_conflict.session_fingerprint = "different-fingerprint".to_owned();
    assert!(matches!(
        store.connect_initial_managed_session(identity_conflict),
        Err(StoreError::ManagedSessionIdentityConflict { .. })
    ));

    assert!(matches!(
        store.connect_initial_managed_session(session_connection(
            "session-other-run",
            "run-2",
            "thread-shared",
            &project_path
        )),
        Err(StoreError::ExternalSessionAlreadyClaimed { .. })
    ));
    assert!(matches!(
        store.connect_initial_managed_session(session_connection(
            "session-second-live",
            "run-1",
            "thread-other",
            &project_path
        )),
        Err(StoreError::LiveManagedSessionExists { .. })
    ));

    let canonical = project_path.to_str().expect("UTF-8 Project path");
    let parent = project_path
        .parent()
        .and_then(Path::to_str)
        .expect("UTF-8 Project parent");
    let leaf = project_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("UTF-8 Project name");
    for (label, noncanonical_cwd) in [
        ("dot", PathBuf::from(format!("{canonical}/./"))),
        ("repeated", PathBuf::from(format!("{parent}//{leaf}"))),
        ("trailing", PathBuf::from(format!("{canonical}/"))),
    ] {
        let session_id = format!("session-{label}-cwd");
        assert!(matches!(
            store.connect_initial_managed_session(session_connection(
                &session_id,
                "run-2",
                &format!("thread-{label}-cwd"),
                &noncanonical_cwd
            )),
            Err(StoreError::ManagedSessionCwdMismatch { .. })
        ));
        assert_eq!(
            store
                .managed_session(&session_id)
                .expect("missing noncanonical-cwd session"),
            None
        );
        assert_eq!(
            store
                .managed_run("run-2")
                .expect("read unchanged Run")
                .expect("unchanged Run")
                .started_at,
            None
        );
        assert_eq!(
            store.latest_ingest_seq().expect("unchanged event cursor"),
            5
        );
    }

    let wrong_cwd = directory.0.join("other-project");
    assert!(matches!(
        store.connect_initial_managed_session(session_connection(
            "session-wrong-cwd",
            "run-2",
            "thread-wrong-cwd",
            &wrong_cwd
        )),
        Err(StoreError::ManagedSessionCwdMismatch { .. })
    ));
    assert_eq!(
        store
            .managed_session("session-wrong-cwd")
            .expect("missing wrong-cwd session"),
        None
    );
    assert_eq!(
        store
            .managed_run("run-2")
            .expect("read unchanged Run")
            .expect("unchanged Run")
            .started_at,
        None
    );
    assert_eq!(
        store.latest_ingest_seq().expect("unchanged event cursor"),
        5
    );
}

#[test]
fn managed_terminal_closes_session_and_run_atomically_idempotently_and_reopens() {
    let directory = TestDirectory::new("terminal-reopen");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-terminal",
            "event-terminal-created",
            "event-terminal-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-terminal",
            "run-terminal",
            "thread-terminal",
            &project_path,
        ))
        .expect("managed session");
    let termination = session_termination(
        "run-terminal",
        "session-terminal",
        "thread-terminal",
        "turn-terminal",
        "event-terminal-completed",
        2,
        ManagedTurnTerminalOutcome::Completed,
    );

    let outcome = store
        .terminate_managed_session(termination.clone())
        .expect("terminate managed session");
    let (run, session, event) = match outcome {
        ManagedSessionTerminationOutcome::Terminated {
            run,
            session,
            event,
        } => (run, session, event),
        other => panic!("unexpected terminal outcome: {other:?}"),
    };
    assert_eq!(run.ended_at.as_deref(), Some(ENDED_AT));
    assert_eq!(session.ended_at.as_deref(), Some(ENDED_AT));
    assert_eq!(session.end_reason.as_deref(), Some("completed"));
    assert_eq!(event.ingest_seq, 4);
    assert_eq!(event.stream_seq, 2);
    assert_eq!(event.event_type, "run.completed");
    assert_eq!(event.payload["outcome"], "completed");
    assert_eq!(event.payload["provider_session_key"], "thread-terminal");
    assert_eq!(event.payload["provider_turn_id"], "turn-terminal");
    assert!(matches!(
        store
            .terminate_managed_session(termination)
            .expect("exact terminal retry"),
        ManagedSessionTerminationOutcome::Duplicate {
            event: duplicate,
            ..
        } if duplicate == event
    ));
    assert_eq!(store.latest_ingest_seq().expect("duplicate cursor"), 4);

    drop(store);
    let reopened = Store::open(&database, CREATED_AT).expect("reopen Store");
    assert_eq!(
        reopened.managed_run("run-terminal").expect("reopened Run"),
        Some(run)
    );
    assert_eq!(
        reopened
            .managed_session("session-terminal")
            .expect("reopened session"),
        Some(session)
    );
    assert_eq!(
        reopened
            .run_events_through("run-terminal", 0, 4, 10)
            .expect("terminal event page")
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            "run.created",
            "run.start_requested",
            "session.connected",
            "run.completed"
        ]
    );
}

#[test]
fn interrupted_terminal_uses_only_the_exact_provider_locator() {
    let directory = TestDirectory::new("terminal-interrupted");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-interrupted",
            "event-interrupted-created",
            "event-interrupted-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-interrupted",
            "run-interrupted",
            "thread-interrupted",
            &project_path,
        ))
        .expect("managed session");

    let outcome = store
        .terminate_managed_session(session_termination(
            "run-interrupted",
            "session-interrupted",
            "thread-interrupted",
            "turn-interrupted",
            "event-run-interrupted",
            2,
            ManagedTurnTerminalOutcome::Interrupted,
        ))
        .expect("interrupt managed session");
    let (session, event) = match outcome {
        ManagedSessionTerminationOutcome::Terminated { session, event, .. } => (session, event),
        other => panic!("unexpected terminal outcome: {other:?}"),
    };
    assert_eq!(session.end_reason.as_deref(), Some("interrupted"));
    assert_eq!(event.event_type, "run.interrupted");
    assert_eq!(event.payload["reason"], "provider_turn_interrupted");
    assert_eq!(event.payload["provider_session_key"], "thread-interrupted");
    assert_eq!(event.payload["provider_turn_id"], "turn-interrupted");
    assert_eq!(event.payload.len(), 3);
}

#[test]
fn managed_terminal_conflicts_preserve_first_result_and_roll_back_late_failure() {
    let directory = TestDirectory::new("terminal-conflicts");
    let (mut store, _database, project_path) = trusted_store(&directory);
    for (run, session, thread) in [
        ("run-late", "session-late", "thread-late"),
        ("run-first", "session-first", "thread-first"),
        (
            "run-preexisting",
            "session-preexisting",
            "thread-preexisting",
        ),
    ] {
        store
            .create_managed_run_intent(run_intent(
                run,
                &format!("event-{run}-created"),
                &format!("event-{run}-requested"),
            ))
            .expect("managed Run");
        store
            .connect_initial_managed_session(session_connection(
                session,
                run,
                thread,
                &project_path,
            ))
            .expect("managed session");
    }
    let initial_cursor = store.latest_ingest_seq().expect("initial cursor");

    let mut sequence_mismatch = session_termination(
        "run-late",
        "session-late",
        "thread-late",
        "turn-late",
        "event-late-terminal",
        3,
        ManagedTurnTerminalOutcome::Completed,
    );
    assert!(matches!(
        store.terminate_managed_session(sequence_mismatch.clone()),
        Err(StoreError::ManagedSessionStreamSequenceMismatch {
            expected: 2,
            received: 3,
            ..
        })
    ));
    sequence_mismatch.stream_seq = 1;
    assert!(matches!(
        store.terminate_managed_session(sequence_mismatch),
        Err(StoreError::InvalidManagedSessionTermination {
            field: "stream_seq"
        })
    ));
    assert!(matches!(
        store.terminate_managed_session(session_termination(
            "run-late",
            "missing-session",
            "thread-late",
            "turn-late",
            "event-missing-session-terminal",
            2,
            ManagedTurnTerminalOutcome::Completed,
        )),
        Err(StoreError::MissingSession { .. })
    ));
    assert!(matches!(
        store.terminate_managed_session(session_termination(
            "run-late",
            "session-late",
            "different-thread",
            "turn-late",
            "event-wrong-thread-terminal",
            2,
            ManagedTurnTerminalOutcome::Completed,
        )),
        Err(StoreError::ManagedSessionIdentityConflict { .. })
    ));
    assert_terminal_rows_open(&store, "run-late", "session-late");
    assert_eq!(
        store.latest_ingest_seq().expect("pre-mutation cursor"),
        initial_cursor
    );

    store
        .append_event(session_event(
            "event-late-terminal",
            "run-late",
            "session-late",
            2,
            "command.started",
        ))
        .expect("conflicting prior event");
    let cursor_before_late_failure = store.latest_ingest_seq().expect("conflict cursor");
    assert!(matches!(
        store.terminate_managed_session(session_termination(
            "run-late",
            "session-late",
            "thread-late",
            "turn-late",
            "event-late-terminal",
            3,
            ManagedTurnTerminalOutcome::Completed,
        )),
        Err(StoreError::EventIdentityConflict { .. })
    ));
    assert_terminal_rows_open(&store, "run-late", "session-late");
    assert_eq!(
        store.latest_ingest_seq().expect("rolled-back cursor"),
        cursor_before_late_failure
    );

    let first = session_termination(
        "run-first",
        "session-first",
        "thread-first",
        "turn-first",
        "event-first-terminal",
        2,
        ManagedTurnTerminalOutcome::Completed,
    );
    store
        .terminate_managed_session(first.clone())
        .expect("first terminal result");
    let first_cursor = store.latest_ingest_seq().expect("first terminal cursor");
    for conflicting in [
        ManagedSessionTermination {
            terminal_event_id: "event-regenerated-terminal".to_owned(),
            ..first.clone()
        },
        ManagedSessionTermination {
            terminal_event_id: "event-later-interrupted".to_owned(),
            stream_seq: 3,
            outcome: ManagedTurnTerminalOutcome::Interrupted,
            ..first
        },
    ] {
        assert!(matches!(
            store.terminate_managed_session(conflicting),
            Err(StoreError::ManagedRunTerminalConflict { .. })
        ));
        assert_eq!(
            store.latest_ingest_seq().expect("first result cursor"),
            first_cursor
        );
    }
    assert_eq!(
        store
            .managed_session("session-first")
            .expect("first session")
            .expect("first session")
            .end_reason
            .as_deref(),
        Some("completed")
    );

    store
        .append_event(session_event(
            "event-preexisting-failed",
            "run-preexisting",
            "session-preexisting",
            2,
            "run.failed",
        ))
        .expect("preexisting terminal event");
    let preexisting_cursor = store.latest_ingest_seq().expect("preexisting cursor");
    assert!(matches!(
        store.terminate_managed_session(session_termination(
            "run-preexisting",
            "session-preexisting",
            "thread-preexisting",
            "turn-preexisting",
            "event-preexisting-interrupted",
            3,
            ManagedTurnTerminalOutcome::Interrupted,
        )),
        Err(StoreError::ManagedRunTerminalConflict { .. })
    ));
    assert_terminal_rows_open(&store, "run-preexisting", "session-preexisting");
    assert_eq!(
        store.latest_ingest_seq().expect("preexisting cursor"),
        preexisting_cursor
    );
}

#[test]
fn live_managed_sessions_are_stable_bounded_and_exclude_terminal_rows() {
    let directory = TestDirectory::new("live-sessions");
    let (mut store, _database, project_path) = trusted_store(&directory);
    for (run, session, thread) in [
        ("run-z", "session-z", "thread-z"),
        ("run-a", "session-a", "thread-a"),
    ] {
        store
            .create_managed_run_intent(run_intent(
                run,
                &format!("event-{run}-created"),
                &format!("event-{run}-requested"),
            ))
            .expect("managed Run");
        store
            .connect_initial_managed_session(session_connection(
                session,
                run,
                thread,
                &project_path,
            ))
            .expect("managed session");
    }
    assert_eq!(
        store
            .live_managed_sessions(2)
            .expect("live managed sessions")
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["session-a", "session-z"]
    );
    assert_eq!(
        store
            .live_managed_sessions(1)
            .expect("bounded live session")
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["session-a"]
    );
    for limit in [0, MAX_LIVE_MANAGED_SESSIONS + 1] {
        assert!(matches!(
            store.live_managed_sessions(limit),
            Err(StoreError::InvalidLiveManagedSessionLimit { .. })
        ));
    }

    store
        .terminate_managed_session(session_termination(
            "run-a",
            "session-a",
            "thread-a",
            "turn-a",
            "event-run-a-completed",
            2,
            ManagedTurnTerminalOutcome::Completed,
        ))
        .expect("terminal Run");
    assert_eq!(
        store
            .live_managed_sessions(2)
            .expect("remaining live session")
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["session-z"]
    );
}

#[test]
fn gap_only_reconciliation_is_explicit_idempotent_and_never_closes_rows() {
    let directory = TestDirectory::new("reconcile-gaps");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-gap",
            "event-gap-created",
            "event-gap-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-gap",
            "run-gap",
            "thread-gap",
            &project_path,
        ))
        .expect("managed session");

    for (index, state, latest_turn_id, result) in [
        (0_u64, ManagedReconciliationState::NoTurns, None, "no_turns"),
        (
            1,
            ManagedReconciliationState::Unknown,
            Some("turn-unknown"),
            "unknown",
        ),
        (2, ManagedReconciliationState::Missing, None, "missing"),
        (
            3,
            ManagedReconciliationState::ScopeConflict,
            None,
            "scope_conflict",
        ),
    ] {
        let reconciliation = session_reconciliation(
            "run-gap",
            "session-gap",
            "thread-gap",
            state,
            latest_turn_id,
            &format!("event-gap-reconcile-{index}"),
            None,
        );
        let outcome = store
            .reconcile_managed_session(reconciliation.clone())
            .expect("record gap reconciliation");
        let events = match outcome {
            ManagedSessionReconciliationOutcome::Recorded { events, .. } => events,
            other => panic!("unexpected reconciliation outcome: {other:?}"),
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "diagnostic.sequence_gap");
        assert_eq!(events[0].stream_seq, index + 2);
        assert_eq!(events[0].payload["reconciliation_result"], result);
        assert_eq!(
            events[0].payload["gap_reason"],
            "provider_notifications_unavailable_after_restart"
        );
        assert_eq!(events[0].payload["provider_session_key"], "thread-gap");
        match latest_turn_id {
            Some(turn_id) => assert_eq!(events[0].payload["latest_provider_turn_id"], turn_id),
            None => assert!(events[0].payload["latest_provider_turn_id"].is_null()),
        }
        assert!(matches!(
            store
                .reconcile_managed_session(reconciliation)
                .expect("exact gap retry"),
            ManagedSessionReconciliationOutcome::Duplicate {
                events: duplicate,
                ..
            } if duplicate == events
        ));
        assert_terminal_rows_open(&store, "run-gap", "session-gap");
    }
    assert_eq!(store.latest_ingest_seq().expect("gap cursor"), 7);

    let invalid_terminal = session_reconciliation(
        "run-gap",
        "session-gap",
        "thread-gap",
        ManagedReconciliationState::Completed,
        None,
        "event-invalid-terminal-gap",
        Some("event-invalid-terminal"),
    );
    assert!(matches!(
        store.reconcile_managed_session(invalid_terminal),
        Err(StoreError::InvalidManagedSessionReconciliation { field: "state" })
    ));
    let invalid_nonterminal = session_reconciliation(
        "run-gap",
        "session-gap",
        "thread-gap",
        ManagedReconciliationState::Missing,
        Some("invented-turn"),
        "event-invalid-missing-gap",
        None,
    );
    assert!(matches!(
        store.reconcile_managed_session(invalid_nonterminal),
        Err(StoreError::InvalidManagedSessionReconciliation { field: "state" })
    ));
    assert_eq!(store.latest_ingest_seq().expect("invalid cursor"), 7);
}

#[test]
fn exact_terminal_reconciliation_maps_all_states_atomically_and_reopens() {
    let directory = TestDirectory::new("reconcile-terminal");
    let (mut store, database, project_path) = trusted_store(&directory);
    for (index, state, event_type, end_reason) in [
        (
            0,
            ManagedReconciliationState::Completed,
            "run.completed",
            "completed",
        ),
        (
            1,
            ManagedReconciliationState::Failed,
            "run.failed",
            "failed",
        ),
        (
            2,
            ManagedReconciliationState::Interrupted,
            "run.interrupted",
            "interrupted",
        ),
    ] {
        let run_id = format!("run-reconciled-{index}");
        let session_id = format!("session-reconciled-{index}");
        let thread_id = format!("thread-reconciled-{index}");
        let turn_id = format!("turn-reconciled-{index}");
        store
            .create_managed_run_intent(run_intent(
                &run_id,
                &format!("event-{run_id}-created"),
                &format!("event-{run_id}-requested"),
            ))
            .expect("managed Run");
        store
            .connect_initial_managed_session(session_connection(
                &session_id,
                &run_id,
                &thread_id,
                &project_path,
            ))
            .expect("managed session");
        let reconciliation = session_reconciliation(
            &run_id,
            &session_id,
            &thread_id,
            state,
            Some(&turn_id),
            &format!("event-{run_id}-gap"),
            Some(&format!("event-{run_id}-terminal")),
        );
        let outcome = store
            .reconcile_managed_session(reconciliation.clone())
            .expect("terminal reconciliation");
        let (run, session, events) = match outcome {
            ManagedSessionReconciliationOutcome::Recorded {
                run,
                session,
                events,
            } => (run, session, events),
            other => panic!("unexpected reconciliation outcome: {other:?}"),
        };
        assert_eq!(run.ended_at.as_deref(), Some(ENDED_AT));
        assert_eq!(session.ended_at.as_deref(), Some(ENDED_AT));
        assert_eq!(session.end_reason.as_deref(), Some(end_reason));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "diagnostic.sequence_gap");
        assert_eq!(events[0].stream_seq, 2);
        assert_eq!(events[1].event_type, event_type);
        assert_eq!(events[1].stream_seq, 3);
        assert_eq!(events[1].payload["provider_turn_id"], turn_id);
        assert_eq!(events[1].payload["reconciled_after_gap"], true);
        assert!(matches!(
            store
                .reconcile_managed_session(reconciliation)
                .expect("exact terminal retry"),
            ManagedSessionReconciliationOutcome::Duplicate {
                events: duplicate,
                ..
            } if duplicate == events
        ));
    }

    let cursor = store.latest_ingest_seq().expect("terminal cursor");
    drop(store);
    let reopened = Store::open(&database, CREATED_AT).expect("reopen Store");
    assert_eq!(
        reopened.latest_ingest_seq().expect("reopened cursor"),
        cursor
    );
    assert!(
        reopened
            .live_managed_sessions(10)
            .expect("no live reconciled sessions")
            .is_empty()
    );
    assert_eq!(
        reopened
            .managed_session("session-reconciled-1")
            .expect("reopened failed session")
            .expect("failed session")
            .end_reason
            .as_deref(),
        Some("failed")
    );
}

#[test]
fn reconciliation_identity_terminal_and_late_event_conflicts_fail_closed() {
    let directory = TestDirectory::new("reconcile-conflicts");
    let (mut store, _database, project_path) = trusted_store(&directory);
    for (run, session, thread) in [
        (
            "run-reconcile-late",
            "session-reconcile-late",
            "thread-late",
        ),
        (
            "run-reconcile-first",
            "session-reconcile-first",
            "thread-first",
        ),
    ] {
        store
            .create_managed_run_intent(run_intent(
                run,
                &format!("event-{run}-created"),
                &format!("event-{run}-requested"),
            ))
            .expect("managed Run");
        store
            .connect_initial_managed_session(session_connection(
                session,
                run,
                thread,
                &project_path,
            ))
            .expect("managed session");
    }
    let initial_cursor = store.latest_ingest_seq().expect("initial cursor");
    assert!(matches!(
        store.reconcile_managed_session(session_reconciliation(
            "run-reconcile-late",
            "session-reconcile-late",
            "wrong-thread",
            ManagedReconciliationState::Unknown,
            None,
            "event-wrong-thread-gap",
            None,
        )),
        Err(StoreError::ManagedSessionIdentityConflict { .. })
    ));
    assert_eq!(
        store.latest_ingest_seq().expect("identity cursor"),
        initial_cursor
    );

    store
        .append_event(session_event(
            "event-late-reconcile-gap",
            "run-reconcile-late",
            "session-reconcile-late",
            2,
            "command.started",
        ))
        .expect("conflicting prior event");
    let late_cursor = store.latest_ingest_seq().expect("late cursor");
    assert!(matches!(
        store.reconcile_managed_session(session_reconciliation(
            "run-reconcile-late",
            "session-reconcile-late",
            "thread-late",
            ManagedReconciliationState::Failed,
            Some("turn-late"),
            "event-late-reconcile-gap",
            Some("event-late-reconcile-terminal"),
        )),
        Err(StoreError::EventIdentityConflict { .. })
    ));
    assert_terminal_rows_open(&store, "run-reconcile-late", "session-reconcile-late");
    assert_eq!(
        store.latest_ingest_seq().expect("rollback cursor"),
        late_cursor
    );

    let first = session_reconciliation(
        "run-reconcile-first",
        "session-reconcile-first",
        "thread-first",
        ManagedReconciliationState::Completed,
        Some("turn-first"),
        "event-first-reconcile-gap",
        Some("event-first-reconcile-terminal"),
    );
    store
        .reconcile_managed_session(first)
        .expect("first terminal reconciliation");
    let first_cursor = store.latest_ingest_seq().expect("first cursor");
    assert!(matches!(
        store.reconcile_managed_session(session_reconciliation(
            "run-reconcile-first",
            "session-reconcile-first",
            "thread-first",
            ManagedReconciliationState::Failed,
            Some("turn-later"),
            "event-later-reconcile-gap",
            Some("event-later-reconcile-terminal"),
        )),
        Err(StoreError::ManagedRunTerminalConflict { .. })
    ));
    assert_eq!(
        store.latest_ingest_seq().expect("preserved first cursor"),
        first_cursor
    );
    assert_eq!(
        store
            .managed_session("session-reconcile-first")
            .expect("first session")
            .expect("first session")
            .end_reason
            .as_deref(),
        Some("completed")
    );
}

fn trusted_store(directory: &TestDirectory) -> (Store, PathBuf, PathBuf) {
    let database = directory.0.join("flit.sqlite3");
    let project_path = directory.0.join("project");
    fs::create_dir(&project_path).expect("Project directory");
    let mut store = Store::open(&database, CREATED_AT).expect("open Store");
    register_project(&mut store, &project_path, "project-1");
    trust_project(&mut store, &project_path, "project-1");
    let canonical_project_path = store
        .project("project-1")
        .expect("read Project")
        .expect("Project")
        .canonical_path;
    (store, database, canonical_project_path)
}

fn register_project(store: &mut Store, path: &Path, project_id: &str) {
    store
        .register_project(ProjectRegistration {
            id: project_id.to_owned(),
            display_name: "Managed Project".to_owned(),
            selected_path: path.to_owned(),
            created_at: CREATED_AT.to_owned(),
        })
        .expect("register Project");
}

fn trust_project(store: &mut Store, path: &Path, project_id: &str) {
    store
        .confirm_project_trust(ProjectTrustConfirmation {
            project_id: project_id.to_owned(),
            selected_path: path.to_owned(),
            confirmed_at: CREATED_AT.to_owned(),
        })
        .expect("trust Project");
}

fn run_intent(run_id: &str, created_event_id: &str, requested_event_id: &str) -> ManagedRunIntent {
    ManagedRunIntent {
        id: run_id.to_owned(),
        project_id: "project-1".to_owned(),
        title: format!("Run {run_id}"),
        goal: Some("Respond with the requested result.".to_owned()),
        start_request: object(json!({
            "permission_mode": "manual",
            "prompt_sha256": "fixture-prompt-digest"
        })),
        baseline_head: None,
        created_at: CREATED_AT.to_owned(),
        run_created_event_id: created_event_id.to_owned(),
        start_requested_event_id: requested_event_id.to_owned(),
    }
}

fn session_connection(
    session_id: &str,
    run_id: &str,
    external_session_key: &str,
    cwd: &Path,
) -> InitialManagedSessionConnection {
    InitialManagedSessionConnection {
        id: session_id.to_owned(),
        run_id: run_id.to_owned(),
        external_session_key: external_session_key.to_owned(),
        session_fingerprint: "codex-0.144.6-exact-profile".to_owned(),
        executable_path: Some(PathBuf::from(
            "/opt/homebrew/Caskroom/codex/0.144.6/codex-aarch64-apple-darwin",
        )),
        executable_version: Some("0.144.6".to_owned()),
        cwd: cwd.to_owned(),
        capabilities: object(json!({
            "completion_detect": "supported",
            "structured_activity": "degraded",
            "stop": "supported"
        })),
        contract_version: "codex-app-server/0.144.6".to_owned(),
        started_at: STARTED_AT.to_owned(),
        connected_event_id: format!("event-{session_id}-connected"),
    }
}

fn session_termination(
    run_id: &str,
    session_id: &str,
    external_session_key: &str,
    provider_turn_id: &str,
    terminal_event_id: &str,
    stream_seq: u64,
    outcome: ManagedTurnTerminalOutcome,
) -> ManagedSessionTermination {
    ManagedSessionTermination {
        run_id: run_id.to_owned(),
        session_id: session_id.to_owned(),
        external_session_key: external_session_key.to_owned(),
        provider_turn_id: provider_turn_id.to_owned(),
        contract_version: "codex-app-server/0.144.6".to_owned(),
        stream_seq,
        ended_at: ENDED_AT.to_owned(),
        terminal_event_id: terminal_event_id.to_owned(),
        outcome,
    }
}

fn session_reconciliation(
    run_id: &str,
    session_id: &str,
    external_session_key: &str,
    state: ManagedReconciliationState,
    latest_turn_id: Option<&str>,
    gap_event_id: &str,
    terminal_event_id: Option<&str>,
) -> ManagedSessionReconciliation {
    ManagedSessionReconciliation {
        run_id: run_id.to_owned(),
        session_id: session_id.to_owned(),
        external_session_key: external_session_key.to_owned(),
        state,
        latest_turn_id: latest_turn_id.map(str::to_owned),
        contract_version: "codex-app-server/0.144.6".to_owned(),
        observed_at: ENDED_AT.to_owned(),
        gap_event_id: gap_event_id.to_owned(),
        terminal_event_id: terminal_event_id.map(str::to_owned),
    }
}

fn session_event(
    event_id: &str,
    run_id: &str,
    session_id: &str,
    stream_seq: u64,
    event_type: &str,
) -> UnsequencedEventEnvelope {
    UnsequencedEventEnvelope {
        protocol_version: EventProtocolVersion::V1_0,
        event_id: event_id.to_owned(),
        run_id: run_id.to_owned(),
        session_id: NullableSessionId::Id(session_id.to_owned()),
        stream_seq,
        occurred_at: ENDED_AT.to_owned(),
        observed_at: ENDED_AT.to_owned(),
        event_type: event_type.to_owned(),
        source: EventSource {
            kind: EventSourceKind::ProviderAdapter,
            provider: Some("codex".to_owned()),
            contract_version: Some("codex-app-server/0.144.6".to_owned()),
            extensions: BTreeMap::new(),
        },
        confidence: 1.0,
        evidence_ids: Vec::new(),
        payload: object(json!({"fixture": true})),
        extensions: BTreeMap::new(),
    }
}

fn assert_terminal_rows_open(store: &Store, run_id: &str, session_id: &str) {
    assert_eq!(
        store
            .managed_run(run_id)
            .expect("read open Run")
            .expect("open Run")
            .ended_at,
        None
    );
    let session = store
        .managed_session(session_id)
        .expect("read live session")
        .expect("live session");
    assert_eq!(session.ended_at, None);
    assert_eq!(session.end_reason, None);
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().expect("object fixture").clone()
}

fn nested_value(depth: usize) -> Value {
    let mut value = Value::String("leaf".to_owned());
    for _ in 0..depth {
        value = json!({"nested": value});
    }
    value
}
