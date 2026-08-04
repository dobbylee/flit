use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use flit_protocol::{
    DashboardReadRequest, DashboardReadResponse, GitBaselinePayload, GitBaselineUnavailableReason,
    ManagedRunObserveRequest, ManagedRunObserveResponse, ManagedRunPermissionDecision,
    ManagedRunPermissionMode, ManagedRunPermissionRespondRequest,
    ManagedRunPermissionRespondResponse, ManagedRunProviderDecision,
    ManagedRunProviderTerminalOutcome, ManagedRunStartRequest, ManagedRunStartResponse,
    ProviderKind,
};
use flit_providers::{
    CodexManagedItemId, CodexManagedScope, CodexManagedThreadId, CodexManagedThreads,
    CodexManagedTurnId, CodexManualStartedThread, CodexPermissionDelivery, CodexPermissionRequest,
    CodexProviderAutoStartedThread, CodexStartedThread, CodexStartedTurn, CodexThreadRead,
    CodexThreadState, CodexTurnObservation, ProviderFingerprint,
    validated_codex_0_145_0_fingerprint,
};
use flit_store::{
    ProjectRegistration, ProjectRegistrationOutcome, ProjectTrustConfirmation, ProjectTrustOutcome,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    BridgeError, CoreManager, InitializationOutcome, PROTOCOL_VERSION, dashboard_read_with,
    managed_run_observe_with, managed_run_permission_respond_with, managed_start,
    start_managed_run_in_core,
};
use crate::codex_recovery::{
    CodexRecoveryAttempt, CodexRecoveryConnector, CodexRecoveryProvider,
    CodexRecoveryProviderError, observe_codex_sessions, persist_codex_recovery_observations,
};

const PROJECT_ID: &str = "project-phase2-fake";
const CREATED_AT: &str = "2026-07-28T00:00:00Z";
const STARTED_AT: &str = "2026-07-28T00:00:01Z";
const OBSERVED_AT: &str = "2026-07-28T00:00:02Z";
const RESPONDED_AT: &str = "2026-07-28T00:00:03Z";
const COMPLETED_AT: &str = "2026-07-28T00:00:04Z";
const DASHBOARD_SAMPLE_COUNT: usize = 40;
const DASHBOARD_P95_LIMIT: Duration = Duration::from_millis(500);
const MAX_FAKE_SCENARIO_BYTES: usize = 64 * 1024;
const EXPECTED_SCENARIO_VERSION: u64 = 1;
const EXPECTED_DATASET_SEED: u64 = 240_204;
const FAKE_SCENARIO_JSON: &str = include_str!("../fixtures/phase2-fake-v1.json");

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("flit-phase2-journey-{}-{nonce}", process::id()));
        fs::create_dir(&path).expect("Phase 2 test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug, Deserialize)]
struct FakeScenario {
    scenario_version: u64,
    dataset_seed: u64,
    runs: Vec<FakeRun>,
}

#[derive(Clone, Debug, Deserialize)]
struct FakeRun {
    run_id: String,
    session_id: String,
    title: String,
    permission_mode: FakePermissionMode,
    thread_id: String,
    turn_id: String,
    observation: FakeObservation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum FakePermissionMode {
    Manual,
    ProviderAuto,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FakeObservation {
    ManualPermission {
        command_item_id: String,
        permission_item_id: String,
        provider_request_id: u64,
        started_at_ms: u64,
    },
    ProviderAutoOutcome {
        review_id: String,
        target_item_id: String,
    },
    Idle,
}

#[derive(Default)]
struct FakeReceipt {
    connected_runs: Vec<String>,
    permission_responses: Vec<(String, u64)>,
}

struct ScriptedConnector {
    pending: Mutex<VecDeque<FakeRun>>,
    receipt: Arc<Mutex<FakeReceipt>>,
}

impl ScriptedConnector {
    fn new(runs: Vec<FakeRun>) -> (Self, Arc<Mutex<FakeReceipt>>) {
        let receipt = Arc::new(Mutex::new(FakeReceipt::default()));
        (
            Self {
                pending: Mutex::new(runs.into()),
                receipt: Arc::clone(&receipt),
            },
            receipt,
        )
    }
}

impl managed_start::ManagedCodexConnector for ScriptedConnector {
    fn connect(
        &self,
        _path_environment: Option<&OsStr>,
    ) -> Result<Box<dyn managed_start::ManagedCodexRuntime>, ()> {
        let run = self
            .pending
            .lock()
            .expect("script queue")
            .pop_front()
            .ok_or(())?;
        self.receipt
            .lock()
            .expect("fake receipt")
            .connected_runs
            .push(run.run_id.clone());
        Ok(Box::new(ScriptedRuntime {
            profile: validated_codex_0_145_0_fingerprint(),
            observations: scripted_observations(&run)?,
            run,
            receipt: Arc::clone(&self.receipt),
        }))
    }
}

struct ScriptedRuntime {
    profile: ProviderFingerprint,
    observations: VecDeque<CodexTurnObservation>,
    run: FakeRun,
    receipt: Arc<Mutex<FakeReceipt>>,
}

impl managed_start::ManagedCodexRuntime for ScriptedRuntime {
    fn validated_profile(&self) -> Option<&ProviderFingerprint> {
        Some(&self.profile)
    }

    fn start_manual(
        &mut self,
        cwd: &Path,
    ) -> Result<CodexManualStartedThread, managed_start::ProviderStartAttemptError> {
        if self.run.permission_mode != FakePermissionMode::Manual {
            return Err(managed_start::ProviderStartAttemptError::Rejected);
        }
        Ok(CodexManualStartedThread {
            thread: started_thread(&self.run, cwd)?,
            provider_configuration: "readOnly+on-request+user",
        })
    }

    fn start_provider_auto(
        &mut self,
        cwd: &Path,
    ) -> Result<CodexProviderAutoStartedThread, managed_start::ProviderStartAttemptError> {
        if self.run.permission_mode != FakePermissionMode::ProviderAuto {
            return Err(managed_start::ProviderStartAttemptError::Rejected);
        }
        Ok(CodexProviderAutoStartedThread {
            thread: started_thread(&self.run, cwd)?,
            provider_configuration: "readOnly+on-request+auto_review",
        })
    }

    fn start_turn(
        &mut self,
        thread_id: &CodexManagedThreadId,
        _prompt: &str,
    ) -> Result<CodexStartedTurn, ()> {
        if thread_id.as_str() != self.run.thread_id {
            return Err(());
        }
        Ok(CodexStartedTurn {
            thread_id: thread_id.clone(),
            turn_id: CodexManagedTurnId::new(self.run.turn_id.clone()).map_err(|_| ())?,
        })
    }

    fn wait_for_turn_observation(&mut self) -> Result<CodexTurnObservation, ()> {
        self.observations.pop_front().ok_or(())
    }

    fn respond_to_file_change_permission(
        &mut self,
        request: &CodexPermissionRequest,
        decision: flit_providers::CodexPermissionDecision,
    ) -> Result<CodexPermissionDelivery, ()> {
        if decision != flit_providers::CodexPermissionDecision::Accept
            || request.thread_id.as_str() != self.run.thread_id
            || request.turn_id.as_str() != self.run.turn_id
        {
            return Err(());
        }
        self.receipt
            .lock()
            .expect("fake receipt")
            .permission_responses
            .push((self.run.run_id.clone(), request.provider_request_id));
        Ok(CodexPermissionDelivery {
            provider_request_id: request.provider_request_id,
            thread_id: request.thread_id.clone(),
            turn_id: request.turn_id.clone(),
            item_id: request.item_id.clone(),
            decision,
        })
    }

    fn delete_started_thread(self: Box<Self>, _thread_id: &CodexManagedThreadId) -> Result<(), ()> {
        Ok(())
    }
}

fn started_thread(
    run: &FakeRun,
    cwd: &Path,
) -> Result<CodexStartedThread, managed_start::ProviderStartAttemptError> {
    Ok(CodexStartedThread {
        thread_id: CodexManagedThreadId::new(run.thread_id.clone())
            .map_err(|_| managed_start::ProviderStartAttemptError::Rejected)?,
        canonical_cwd: cwd.to_owned(),
    })
}

fn scripted_observations(run: &FakeRun) -> Result<VecDeque<CodexTurnObservation>, ()> {
    let thread_id = CodexManagedThreadId::new(run.thread_id.clone()).map_err(|_| ())?;
    let turn_id = CodexManagedTurnId::new(run.turn_id.clone()).map_err(|_| ())?;
    let observations = match &run.observation {
        FakeObservation::ManualPermission {
            command_item_id,
            permission_item_id,
            provider_request_id,
            started_at_ms,
        } => vec![
            CodexTurnObservation::CommandStarted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item_id: CodexManagedItemId::new(command_item_id.clone()).map_err(|_| ())?,
            },
            CodexTurnObservation::PermissionRequested(CodexPermissionRequest {
                provider_request_id: *provider_request_id,
                thread_id,
                turn_id,
                item_id: CodexManagedItemId::new(permission_item_id.clone()).map_err(|_| ())?,
                started_at_ms: *started_at_ms,
            }),
        ],
        FakeObservation::ProviderAutoOutcome {
            review_id,
            target_item_id,
        } => vec![CodexTurnObservation::ProviderAutoOutcome {
            thread_id,
            turn_id,
            review_id: CodexManagedItemId::new(review_id.clone()).map_err(|_| ())?,
            target_item_id: CodexManagedItemId::new(target_item_id.clone()).map_err(|_| ())?,
        }],
        FakeObservation::Idle => Vec::new(),
    };
    Ok(observations.into())
}

struct RecoveryConnector {
    profile: ProviderFingerprint,
    turns_by_thread: BTreeMap<String, String>,
}

struct RecoveryProvider {
    profile: ProviderFingerprint,
    turns_by_thread: BTreeMap<String, String>,
}

impl CodexRecoveryConnector for RecoveryConnector {
    type Provider = RecoveryProvider;

    fn connect(&mut self, executable: &Path) -> Result<Self::Provider, CodexRecoveryProviderError> {
        if executable != self.profile.canonical_executable {
            return Err(CodexRecoveryProviderError);
        }
        Ok(RecoveryProvider {
            profile: self.profile.clone(),
            turns_by_thread: self.turns_by_thread.clone(),
        })
    }
}

impl CodexRecoveryProvider for RecoveryProvider {
    fn validated_profile(&self) -> Option<&ProviderFingerprint> {
        Some(&self.profile)
    }

    fn list_managed(
        &mut self,
        scope: &CodexManagedScope,
    ) -> Result<CodexManagedThreads, CodexRecoveryProviderError> {
        let expected = scope
            .exact_thread_ids()
            .iter()
            .map(|thread| thread.as_str())
            .collect::<BTreeSet<_>>();
        if expected
            != self
                .turns_by_thread
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        {
            return Err(CodexRecoveryProviderError);
        }
        Ok(CodexManagedThreads {
            matched_thread_ids: scope.exact_thread_ids().iter().cloned().collect(),
            conflicting_threads: Vec::new(),
            missing_thread_ids: Vec::new(),
            unrelated_thread_count: 0,
            page_count: 1,
        })
    }

    fn read_managed(
        &mut self,
        thread_id: &CodexManagedThreadId,
    ) -> Result<CodexThreadRead, CodexRecoveryProviderError> {
        let turn_id = self
            .turns_by_thread
            .get(thread_id.as_str())
            .ok_or(CodexRecoveryProviderError)?;
        Ok(CodexThreadRead {
            thread_id: thread_id.clone(),
            latest_turn_id: Some(turn_id.clone()),
            state: CodexThreadState::Completed,
        })
    }
}

#[test]
fn phase2_journey_and_four_run_latency_gate() {
    assert!(FAKE_SCENARIO_JSON.len() <= MAX_FAKE_SCENARIO_BYTES);
    let scenario: FakeScenario =
        serde_json::from_str(FAKE_SCENARIO_JSON).expect("versioned Phase 2 fake scenario");
    assert_eq!(scenario.scenario_version, EXPECTED_SCENARIO_VERSION);
    assert_eq!(scenario.dataset_seed, EXPECTED_DATASET_SEED);
    assert_eq!(scenario.runs.len(), 4);
    assert_eq!(
        scenario
            .runs
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        scenario.runs.len(),
        "fake Run identities must be unique"
    );

    let directory = TestDirectory::new();
    let data_directory = directory.0.join("data");
    let project_directory = directory.0.join("project");
    fs::create_dir(&project_directory).expect("fake Project");
    let canonical_project = fs::canonicalize(&project_directory).expect("canonical fake Project");

    let manager = CoreManager::default();
    assert_eq!(
        manager
            .initialize(data_directory.to_str().expect("UTF-8 data directory"))
            .expect("initialize fake Core"),
        InitializationOutcome::Initialized
    );
    seed_trusted_project(&manager, &canonical_project);

    let (connector, receipt) = ScriptedConnector::new(scenario.runs.clone());
    let starts = start_runs(&manager, &connector, &scenario);
    assert_eq!(
        starts.keys().cloned().collect::<BTreeSet<_>>(),
        scenario.runs.iter().map(|run| run.run_id.clone()).collect()
    );

    let manual = scenario
        .runs
        .iter()
        .find(|run| matches!(run.observation, FakeObservation::ManualPermission { .. }))
        .expect("Manual permission scenario");
    let permission = observe(&manager, manual);
    let (permission_request_id, permission_request_version, provider_request_id, permission_run_id) =
        match permission {
            ManagedRunObserveResponse::PermissionRequested {
                request_id,
                request_version,
                provider_request_id,
                run_id,
                ..
            } => (request_id, request_version, provider_request_id, run_id),
            response => panic!("expected exact Manual permission, got {response:?}"),
        };
    let permission_response = respond_to_permission(
        &manager,
        &permission_run_id,
        &permission_request_id,
        permission_request_version,
    );
    assert!(matches!(
        permission_response,
        ManagedRunPermissionRespondResponse::Delivered {
            decision: ManagedRunPermissionDecision::AllowOnce,
            ..
        }
    ));

    let provider_auto = scenario
        .runs
        .iter()
        .find(|run| matches!(run.observation, FakeObservation::ProviderAutoOutcome { .. }))
        .expect("ProviderAuto scenario");
    let provider_outcome = observe(&manager, provider_auto);
    assert!(matches!(
        provider_outcome,
        ManagedRunObserveResponse::ProviderOutcomeResolved {
            provider_decision: ManagedRunProviderDecision::Allowed,
            terminal_outcome: ManagedRunProviderTerminalOutcome::RequestResolved,
            ..
        }
    ));

    let fake_receipt = receipt.lock().expect("fake receipt");
    assert_eq!(fake_receipt.connected_runs.len(), 4);
    assert_eq!(
        fake_receipt.permission_responses,
        vec![(manual.run_id.clone(), provider_request_id)]
    );
    drop(fake_receipt);

    manager
        .with_ready_core(|core| {
            let manual_events = event_types(&core.store, &manual.run_id);
            assert!(manual_events.contains(&"permission.response_submitted".to_owned()));
            assert!(manual_events.contains(&"permission.resolved".to_owned()));
            let provider_auto_events = event_types(&core.store, &provider_auto.run_id);
            assert!(
                provider_auto_events.contains(&"permission.provider_outcome_resolved".to_owned())
            );
            assert!(
                !provider_auto_events.contains(&"permission.response_submitted".to_owned()),
                "ProviderAuto factual outcome must not create a Flit response attempt"
            );
            Ok(())
        })
        .expect("inspect exact event ownership");

    let (_, _, run_ids) = initial_dashboard(&manager);
    assert_eq!(
        run_ids,
        scenario
            .runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<BTreeSet<_>>(),
        "the four-Run Dashboard must not hide or truncate a live Run"
    );
    let snapshot_p95 = measure_dashboard_p95(
        &manager,
        DashboardReadRequest {
            expected_core_instance_id: None,
            after_cursor: None,
            requested_event_limit: 50,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        true,
        &run_ids,
    );
    assert!(
        snapshot_p95 <= DASHBOARD_P95_LIMIT,
        "four-Run Dashboard snapshot p95 {snapshot_p95:?} exceeds {DASHBOARD_P95_LIMIT:?}"
    );

    drop(manager);

    let restarted = CoreManager::default();
    assert_eq!(
        restarted
            .initialize(data_directory.to_str().expect("UTF-8 data directory"))
            .expect("restart fake Core"),
        InitializationOutcome::Initialized
    );
    let (_, startup_observed_at, sessions) = restarted
        .take_startup_recovery()
        .expect("bounded startup recovery snapshot");
    assert_eq!(sessions.len(), 4);
    restarted
        .with_ready_core(|core| {
            for run in &scenario.runs {
                assert!(
                    event_types(&core.store, &run.run_id)
                        .contains(&"diagnostic.sequence_gap".to_owned()),
                    "restart must persist durable Unknown before provider observation"
                );
            }
            Ok(())
        })
        .expect("inspect startup Unknown events");
    let (restarted_core_instance_id, unknown_cursor, unknown_run_ids) =
        initial_dashboard(&restarted);
    assert_eq!(unknown_run_ids, run_ids);

    let profile = validated_codex_0_145_0_fingerprint();
    let mut recovery = RecoveryConnector {
        profile,
        turns_by_thread: scenario
            .runs
            .iter()
            .map(|run| (run.thread_id.clone(), run.turn_id.clone()))
            .collect(),
    };
    let observations =
        observe_codex_sessions(sessions, &mut recovery).expect("exact fake recovery observation");
    let summary = restarted
        .with_ready_core(|core| {
            persist_codex_recovery_observations(
                &mut core.store,
                &CodexRecoveryAttempt {
                    id: "phase2-fake-exact".to_owned(),
                    observed_at: startup_observed_at,
                },
                observations,
            )
            .map_err(|_| BridgeError::StorageFailure)
        })
        .expect("persist exact fake recovery");
    assert_eq!(summary.examined, 4);
    assert_eq!(summary.completed, 4);
    assert_eq!(summary.unknown, 0);

    let delta_p95 = measure_dashboard_p95(
        &restarted,
        DashboardReadRequest {
            expected_core_instance_id: Some(restarted_core_instance_id),
            after_cursor: Some(unknown_cursor),
            requested_event_limit: 50,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        },
        false,
        &run_ids,
    );
    assert!(
        delta_p95 <= DASHBOARD_P95_LIMIT,
        "four-Run projection-upsert delta p95 {delta_p95:?} exceeds {DASHBOARD_P95_LIMIT:?}"
    );

    let completed_dashboard = read_dashboard(
        &restarted,
        &DashboardReadRequest {
            expected_core_instance_id: None,
            after_cursor: None,
            requested_event_limit: 50,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        },
    );
    let DashboardReadResponse::Snapshot {
        runs: completed_runs,
        has_more,
        ..
    } = completed_dashboard
    else {
        panic!("completed Dashboard must be a snapshot");
    };
    assert!(!has_more);
    assert_eq!(
        completed_runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<BTreeSet<_>>(),
        run_ids
    );
    assert!(
        completed_runs
            .iter()
            .all(|run| run.lifecycle == "Finished" && run.dashboard_bucket == "Finished"),
        "exact recovery completion must converge every Dashboard Run to Finished"
    );
    restarted
        .with_ready_core(|core| {
            for run in &scenario.runs {
                let stored = core
                    .store
                    .managed_run(&run.run_id)
                    .expect("completed Run")
                    .expect("completed Run");
                assert!(stored.ended_at.is_some());
                let session = core
                    .store
                    .managed_session(&run.session_id)
                    .expect("completed session")
                    .expect("completed session");
                assert_eq!(session.end_reason.as_deref(), Some("completed"));
            }
            Ok(())
        })
        .expect("inspect completed fake Runs");

    println!(
        "phase2_fake_receipt={}",
        json!({
            "architecture": std::env::consts::ARCH,
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "dataset_seed": scenario.dataset_seed,
            "dashboard_delta_kind": "four_finished_upserts",
            "dashboard_delta_p95_us": delta_p95.as_micros(),
            "dashboard_snapshot_p95_us": snapshot_p95.as_micros(),
            "os": std::env::consts::OS,
            "run_count": scenario.runs.len(),
            "sample_count_per_read": DASHBOARD_SAMPLE_COUNT,
            "scenario_version": scenario.scenario_version,
            "threshold_ms": DASHBOARD_P95_LIMIT.as_millis(),
        })
    );
}

fn seed_trusted_project(manager: &CoreManager, project: &Path) {
    manager
        .with_ready_core(|core| {
            assert!(matches!(
                core.store
                    .register_project(ProjectRegistration {
                        id: PROJECT_ID.to_owned(),
                        display_name: "Phase 2 Fake Project".to_owned(),
                        selected_path: project.to_owned(),
                        created_at: CREATED_AT.to_owned(),
                    })
                    .expect("register fake Project"),
                ProjectRegistrationOutcome::Registered(_)
            ));
            assert!(matches!(
                core.store
                    .confirm_project_trust(ProjectTrustConfirmation {
                        project_id: PROJECT_ID.to_owned(),
                        selected_path: project.to_owned(),
                        confirmed_at: CREATED_AT.to_owned(),
                    })
                    .expect("trust fake Project"),
                ProjectTrustOutcome::Trusted(_)
            ));
            Ok(())
        })
        .expect("seed trusted Project");
}

fn start_runs(
    manager: &CoreManager,
    connector: &ScriptedConnector,
    scenario: &FakeScenario,
) -> BTreeMap<String, ManagedRunStartResponse> {
    scenario
        .runs
        .iter()
        .enumerate()
        .map(|(index, run)| {
            let request = ManagedRunStartRequest {
                run_id: run.run_id.clone(),
                session_id: run.session_id.clone(),
                project_id: PROJECT_ID.to_owned(),
                title: run.title.clone(),
                goal: format!("Execute deterministic fake journey {}", run.run_id),
                provider: ProviderKind::Codex,
                permission_mode: match run.permission_mode {
                    FakePermissionMode::Manual => ManagedRunPermissionMode::Manual,
                    FakePermissionMode::ProviderAuto => ManagedRunPermissionMode::ProviderAuto,
                },
                permission_mode_version: 1,
                created_at: CREATED_AT.to_owned(),
                git_baseline_observed_at: CREATED_AT.to_owned(),
                started_at: STARTED_AT.to_owned(),
                run_created_event_id: format!("event-{}-created", run.run_id),
                git_baseline_event_id: format!("event-{}-git-baseline", run.run_id),
                start_requested_event_id: format!("event-{}-start-requested", run.run_id),
                session_connected_event_id: format!("event-{}-connected", run.run_id),
                start_failed_event_id: format!("event-{}-start-failed", run.run_id),
                start_unknown_event_id: format!("event-{}-start-unknown", run.run_id),
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            };
            let rendered = manager
                .with_ready_core(|core| {
                    start_managed_run_in_core(
                        core,
                        connector,
                        None,
                        GitBaselinePayload::Unavailable {
                            project_id: PROJECT_ID.to_owned(),
                            reason: GitBaselineUnavailableReason::RunnerUnavailable,
                        },
                        managed_start::RetainedGitChangeBaseline::Unavailable(
                            "git_baseline_observation_unavailable".to_owned(),
                        ),
                        request,
                    )
                })
                .unwrap_or_else(|error| panic!("start fake Run {index}: {error:?}"));
            let response: ManagedRunStartResponse =
                serde_json::from_str(&rendered).expect("managed start response");
            assert_eq!(response.provider_thread_id, run.thread_id);
            assert_eq!(response.provider_turn_id, run.turn_id);
            match run.permission_mode {
                FakePermissionMode::Manual => {
                    assert_eq!(response.permission_mode, ManagedRunPermissionMode::Manual);
                    assert_eq!(response.provider_configuration, "readOnly+on-request+user");
                }
                FakePermissionMode::ProviderAuto => {
                    assert_eq!(
                        response.permission_mode,
                        ManagedRunPermissionMode::ProviderAuto
                    );
                    assert_eq!(
                        response.provider_configuration,
                        "readOnly+on-request+auto_review"
                    );
                }
            }
            (run.run_id.clone(), response)
        })
        .collect()
}

fn observe(manager: &CoreManager, run: &FakeRun) -> ManagedRunObserveResponse {
    let rendered = managed_run_observe_with(
        manager,
        serde_json::to_string(&ManagedRunObserveRequest {
            run_id: run.run_id.clone(),
            observed_at: OBSERVED_AT.to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("observe request"),
        managed_start::wait_managed_observation,
        managed_start::commit_managed_observation,
    )
    .expect("observe fake Run");
    serde_json::from_str(&rendered).expect("observe response")
}

fn respond_to_permission(
    manager: &CoreManager,
    run_id: &str,
    request_id: &str,
    request_version: u64,
) -> ManagedRunPermissionRespondResponse {
    let rendered = managed_run_permission_respond_with(
        manager,
        serde_json::to_string(&ManagedRunPermissionRespondRequest {
            run_id: run_id.to_owned(),
            request_id: request_id.to_owned(),
            request_version,
            response_attempt_id: "attempt-phase2-fake-manual".to_owned(),
            decision: ManagedRunPermissionDecision::AllowOnce,
            submitted_at: RESPONDED_AT.to_owned(),
            finished_at: COMPLETED_AT.to_owned(),
            submitted_event_id: "event-phase2-fake-manual-submitted".to_owned(),
            resolved_event_id: "event-phase2-fake-manual-resolved".to_owned(),
            delivery_unknown_event_id: "event-phase2-fake-manual-unknown".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("permission response request"),
        |runtime, decision| runtime.respond_to_active_permission(decision),
    )
    .expect("respond to fake permission");
    serde_json::from_str(&rendered).expect("permission response")
}

fn initial_dashboard(manager: &CoreManager) -> (String, u64, BTreeSet<String>) {
    let response = read_dashboard(
        manager,
        &DashboardReadRequest {
            expected_core_instance_id: None,
            after_cursor: None,
            requested_event_limit: 50,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        },
    );
    let DashboardReadResponse::Snapshot {
        core_instance_id,
        next_cursor,
        has_more,
        runs,
        ..
    } = response
    else {
        panic!("initial Dashboard must be a snapshot");
    };
    assert!(!has_more);
    (
        core_instance_id,
        next_cursor,
        runs.into_iter().map(|run| run.run_id).collect(),
    )
}

fn measure_dashboard_p95(
    manager: &CoreManager,
    request: DashboardReadRequest,
    expect_snapshot: bool,
    expected_runs: &BTreeSet<String>,
) -> Duration {
    let mut samples = Vec::with_capacity(DASHBOARD_SAMPLE_COUNT);
    for _ in 0..DASHBOARD_SAMPLE_COUNT {
        let started = Instant::now();
        let response = read_dashboard(manager, &request);
        samples.push(started.elapsed());
        match response {
            DashboardReadResponse::Snapshot { runs, has_more, .. } => {
                assert!(expect_snapshot);
                assert!(!has_more);
                assert_eq!(
                    runs.into_iter()
                        .map(|run| run.run_id)
                        .collect::<BTreeSet<_>>(),
                    *expected_runs
                );
            }
            DashboardReadResponse::Delta {
                events,
                runs,
                has_more,
                ..
            } => {
                assert!(!expect_snapshot);
                assert!(!events.is_empty());
                assert!(!has_more);
                assert_eq!(
                    runs.iter()
                        .map(|run| run.run_id.clone())
                        .collect::<BTreeSet<_>>(),
                    *expected_runs
                );
                assert!(
                    runs.iter().all(
                        |run| run.lifecycle == "Finished" && run.dashboard_bucket == "Finished"
                    ),
                    "non-empty Dashboard delta must converge all four projection upserts"
                );
            }
        }
    }
    samples.sort_unstable();
    samples[(DASHBOARD_SAMPLE_COUNT * 95).div_ceil(100) - 1]
}

fn read_dashboard(manager: &CoreManager, request: &DashboardReadRequest) -> DashboardReadResponse {
    let rendered = dashboard_read_with(
        manager,
        &serde_json::to_string(request).expect("Dashboard request"),
    )
    .expect("Dashboard response");
    serde_json::from_str(&rendered).expect("Dashboard response JSON")
}

fn event_types(store: &flit_store::Store, run_id: &str) -> Vec<String> {
    let latest = store.latest_ingest_seq().expect("latest event cursor");
    store
        .run_events_through(run_id, 0, latest, 100)
        .expect("Run events")
        .events
        .into_iter()
        .map(|event| event.event_type)
        .collect()
}
