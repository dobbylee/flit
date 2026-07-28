use std::{
    env, fs, process,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use flit_bridge::{core_construction_count, dashboard_read_json, initialize_core};
use flit_protocol::{DashboardReadRequest, DashboardReadResponse, PROTOCOL_VERSION};
use flit_store::{
    InitialManagedSessionConnection, ManagedRunIntent, ProjectRegistration,
    ProjectTrustConfirmation, Store,
};
use serde_json::{Map, Value, json};

const CREATED_AT: &str = "2026-07-28T10:00:00Z";
const STARTED_AT: &str = "2026-07-28T10:00:01Z";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[test]
fn startup_persists_unknown_before_ready_then_reconciles_once_in_background() {
    if let Some(path) = env::var_os("FLIT_EMPTY_STARTUP_RECOVERY_PATH") {
        let data_directory = std::path::PathBuf::from(path);
        fs::create_dir(&data_directory).expect("empty data directory");
        initialize_core(
            data_directory.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("empty Core initialization");
        match initial_dashboard() {
            DashboardReadResponse::Snapshot {
                next_cursor, runs, ..
            } => {
                assert_eq!(next_cursor, 0);
                assert!(runs.is_empty());
            }
            DashboardReadResponse::Delta { .. } => {
                panic!("empty initial Dashboard read must be a snapshot");
            }
        }
        initialize_core(
            data_directory.to_string_lossy().into_owned(),
            PROTOCOL_VERSION.to_owned(),
        )
        .expect("empty repeat initialization");
        assert_eq!(core_construction_count(), 1);
        fs::remove_dir_all(data_directory).expect("remove empty test directory");
        return;
    }

    let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let data_directory = std::env::temp_dir().join(format!(
        "flit-bridge-startup-recovery-{}-{nonce}",
        process::id()
    ));
    fs::create_dir(&data_directory).expect("data directory");
    let project_path = data_directory.join("project");
    fs::create_dir(&project_path).expect("Project directory");
    let database_path = data_directory.join("flit.sqlite3");
    let mut store = Store::open(&database_path, CREATED_AT).expect("seed Store");
    store
        .register_project(ProjectRegistration {
            id: "project-startup".to_owned(),
            display_name: "Startup Recovery".to_owned(),
            selected_path: project_path.clone(),
            created_at: CREATED_AT.to_owned(),
        })
        .expect("register Project");
    store
        .confirm_project_trust(ProjectTrustConfirmation {
            project_id: "project-startup".to_owned(),
            selected_path: project_path,
            confirmed_at: CREATED_AT.to_owned(),
        })
        .expect("trust Project");
    let canonical_path = store
        .project("project-startup")
        .expect("Project read")
        .expect("Project")
        .canonical_path;
    store
        .create_managed_run_intent(ManagedRunIntent {
            id: "run-startup".to_owned(),
            project_id: "project-startup".to_owned(),
            title: "Recover at startup".to_owned(),
            goal: Some("Recover this exact managed Run.".to_owned()),
            start_request: object(json!({"prompt_sha256": "startup-digest"})),
            baseline_head: None,
            created_at: CREATED_AT.to_owned(),
            run_created_event_id: "event-startup-created".to_owned(),
            start_requested_event_id: "event-startup-requested".to_owned(),
        })
        .expect("managed Run");
    store
        .connect_initial_managed_session(InitialManagedSessionConnection {
            id: "session-startup".to_owned(),
            run_id: "run-startup".to_owned(),
            external_session_key: "thread-startup".to_owned(),
            session_fingerprint: "missing-executable-evidence".to_owned(),
            executable_path: None,
            executable_version: None,
            cwd: canonical_path,
            capabilities: object(json!({"reconcile": "unknown"})),
            contract_version: "codex-app-server/unknown".to_owned(),
            started_at: STARTED_AT.to_owned(),
            connected_event_id: "event-startup-connected".to_owned(),
        })
        .expect("managed session");
    let before_recovery_cursor = store.latest_ingest_seq().expect("seed cursor");
    drop(store);

    initialize_core(
        data_directory.to_string_lossy().into_owned(),
        PROTOCOL_VERSION.to_owned(),
    )
    .expect("Core initialization with recovery");
    assert_eq!(core_construction_count(), 1);

    let first = initial_dashboard();
    let (core_instance_id, first_cursor, first_activity) = snapshot_facts(first);
    assert!(first_cursor > before_recovery_cursor);
    assert_eq!(first_activity, "Unknown");

    let expected_background_cursor = before_recovery_cursor + 2;
    let mut recovered_cursor = first_cursor;
    let deadline = Instant::now() + Duration::from_secs(5);
    while recovered_cursor != expected_background_cursor && Instant::now() < deadline {
        thread::yield_now();
        let (_, cursor, _) = snapshot_facts(initial_dashboard());
        recovered_cursor = cursor;
    }
    assert_eq!(recovered_cursor, expected_background_cursor);

    let delta = dashboard(DashboardReadRequest {
        expected_core_instance_id: Some(core_instance_id),
        after_cursor: Some(before_recovery_cursor),
        requested_event_limit: 50,
        client_protocol_version: PROTOCOL_VERSION.to_owned(),
    });
    match delta {
        DashboardReadResponse::Delta {
            next_cursor,
            events,
            runs,
            ..
        } => {
            assert_eq!(next_cursor, expected_background_cursor);
            assert_eq!(events.len(), 2);
            assert!(
                events
                    .iter()
                    .all(|event| event.event_type == "diagnostic.sequence_gap")
            );
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].version, expected_background_cursor);
            assert_eq!(runs[0].activity, "Unknown");
        }
        DashboardReadResponse::Snapshot { .. } => {
            panic!("exact startup cursor must return a delta");
        }
    }

    initialize_core(
        data_directory.to_string_lossy().into_owned(),
        PROTOCOL_VERSION.to_owned(),
    )
    .expect("same-process initialization is idempotent");
    let (_, repeated_cursor, _) = snapshot_facts(initial_dashboard());
    assert_eq!(repeated_cursor, expected_background_cursor);
    assert_eq!(core_construction_count(), 1);

    let empty_directory = data_directory.with_extension("empty");
    let child_status = Command::new(env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "startup_persists_unknown_before_ready_then_reconciles_once_in_background",
        ])
        .env("FLIT_EMPTY_STARTUP_RECOVERY_PATH", &empty_directory)
        .status()
        .expect("launch empty startup recovery probe");
    assert!(child_status.success());

    fs::remove_dir_all(data_directory).expect("remove test directory");
}

fn initial_dashboard() -> DashboardReadResponse {
    dashboard(DashboardReadRequest {
        expected_core_instance_id: None,
        after_cursor: None,
        requested_event_limit: 50,
        client_protocol_version: PROTOCOL_VERSION.to_owned(),
    })
}

fn dashboard(request: DashboardReadRequest) -> DashboardReadResponse {
    let rendered = dashboard_read_json(serde_json::to_string(&request).expect("request JSON"))
        .expect("Dashboard read");
    serde_json::from_str(&rendered).expect("Dashboard response")
}

fn snapshot_facts(response: DashboardReadResponse) -> (String, u64, String) {
    match response {
        DashboardReadResponse::Snapshot {
            core_instance_id,
            next_cursor,
            runs,
            ..
        } => {
            assert_eq!(runs.len(), 1);
            (core_instance_id, next_cursor, runs[0].activity.clone())
        }
        DashboardReadResponse::Delta { .. } => {
            panic!("initial Dashboard read must be a snapshot");
        }
    }
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().expect("object fixture").clone()
}
