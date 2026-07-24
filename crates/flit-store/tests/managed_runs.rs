use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_store::{
    InitialManagedSessionConnection, InitialManagedSessionOutcome, ManagedRunIntent,
    ManagedRunIntentOutcome, ProjectRegistration, ProjectTrustConfirmation, Store, StoreError,
};
use serde_json::{Map, Value, json};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
const CREATED_AT: &str = "2026-07-24T10:00:00Z";
const STARTED_AT: &str = "2026-07-24T10:00:01Z";

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
