use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_protocol::{
    EventProtocolVersion, EventSource, EventSourceKind, GitBaselinePayload,
    GitBaselineUnavailableReason, GitDirtySummary, GitHead, NullableSessionId,
    PossiblyStuckPayload, StuckCauseCode, StuckClearReasonCode, StuckClearedPayload,
    StuckProcessReceipt, UnsequencedEventEnvelope,
};
use flit_store::{
    AppendEventOutcome, DashboardChangeSummary, InitialManagedSessionConnection,
    InitialManagedSessionOutcome, MAX_DASHBOARD_PROJECTION_SOURCE_BYTES, MAX_LIVE_MANAGED_SESSIONS,
    MAX_MANAGED_GIT_CHANGE_PAGE_SIZE, MAX_MANAGED_GIT_CHANGE_PAGE_SOURCE_BYTES,
    MAX_MANAGED_STUCK_ASSESSMENT_RUNS, ManagedGitChangeAttribution, ManagedGitChangeSet,
    ManagedGitChangeSummary, ManagedGitFileChange, ManagedGitFileStatus, ManagedGitProjectScope,
    ManagedGitRepositoryIdentity, ManagedPermissionDecision,
    ManagedPermissionDeliveryUnknownReason, ManagedPermissionResolutionKind,
    ManagedPermissionResponseAttempt, ManagedPermissionResponseAttemptOutcome,
    ManagedPermissionResponseResult, ManagedPermissionResponseResultKind, ManagedProviderDecision,
    ManagedProviderObservation, ManagedProviderObservationKind, ManagedProviderOutcome,
    ManagedProviderOutcomeCommit, ManagedProviderTerminalOutcome, ManagedReconciliationState,
    ManagedRunIntent, ManagedRunIntentOutcome, ManagedRunStartFailure,
    ManagedRunStartFailureOutcome, ManagedSessionReconciliation,
    ManagedSessionReconciliationOutcome, ManagedSessionTermination,
    ManagedSessionTerminationOutcome, ManagedStuckActivity, ManagedStuckAssessment,
    ManagedStuckLifecycle, ManagedStuckTransition, ManagedStuckTransitionOutcome,
    ManagedStuckWaitKind, ManagedTurnTerminalOutcome, ProjectDirectoryInspection,
    ProjectRegistration, ProjectTrustConfirmation, Store, StoreError,
};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
const CREATED_AT: &str = "2026-07-24T10:00:00Z";
const STARTED_AT: &str = "2026-07-24T10:00:01Z";
const ENDED_AT: &str = "2026-07-24T10:05:00Z";
const CHANGE_INSIDE_ID: &str = "11111111111111111111111111111111";
const CHANGE_OUTSIDE_ID: &str = "22222222222222222222222222222222";

#[test]
fn stuck_assessment_contexts_are_bounded_exact_read_only_and_reopen() {
    let directory = TestDirectory::new("stuck-assessment-contexts");
    let (mut store, database, project_path) = trusted_store(&directory);
    for run_id in [
        "run-context-a-starting",
        "run-context-b-blocked",
        "run-context-c-stuck",
    ] {
        store
            .create_managed_run_intent(run_intent(
                run_id,
                &format!("event-{run_id}-created"),
                &format!("event-{run_id}-start-requested"),
            ))
            .expect("managed Run");
    }
    for (run_id, session_id, thread_id) in [
        (
            "run-context-b-blocked",
            "session-context-b",
            "thread-context-b",
        ),
        (
            "run-context-c-stuck",
            "session-context-c",
            "thread-context-c",
        ),
    ] {
        store
            .connect_initial_managed_session(session_connection(
                session_id,
                run_id,
                thread_id,
                &project_path,
            ))
            .expect("managed session");
    }
    store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-context-b-blocked".to_owned(),
            session_id: "session-context-b".to_owned(),
            external_session_key: "thread-context-b".to_owned(),
            provider_turn_id: "turn-context-b".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: "event-context-b-permission".to_owned(),
            observed_at: "2026-07-24T10:00:02Z".to_owned(),
            kind: ManagedProviderObservationKind::PermissionRequested {
                request_id: "request-context-b".to_owned(),
                provider_request_id: 41,
                provider_item_id: "permission-context-b".to_owned(),
                provider_started_at_ms: 42,
            },
        })
        .expect("blocking request");
    let run_c_version = store
        .run_snapshot("run-context-c-stuck")
        .expect("Run C snapshot")
        .expect("Run C projection")
        .version;
    let stuck_version = match store
        .append_managed_stuck_transition(ManagedStuckTransition {
            run_id: "run-context-c-stuck".to_owned(),
            expected_run_version: run_c_version,
            event_id: "event-context-c-stuck".to_owned(),
            observed_at: "2026-07-24T10:02:10Z".to_owned(),
            assessment: ManagedStuckAssessment::PossiblyStuck(PossiblyStuckPayload {
                occurrence_id: "occurrence-context-c".to_owned(),
                cause: StuckCauseCode::Unknown,
                threshold_seconds: 120,
                progress_event_id: "event-run-context-c-stuck-created".to_owned(),
                progress_observed_at: CREATED_AT.to_owned(),
                progress_monotonic_ms: 5_000,
                baseline_monotonic_ms: 5_000,
                stuck_since_monotonic_ms: 125_000,
                process: StuckProcessReceipt::Alive {
                    generation: "process-generation-context-c".to_owned(),
                    observed_monotonic_ms: 130_000,
                },
                evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
            }),
        })
        .expect("stuck occurrence")
    {
        ManagedStuckTransitionOutcome::Appended(event) => event.ingest_seq,
        other => panic!("unexpected stuck context transition: {other:?}"),
    };
    store
        .create_managed_run_intent(run_intent(
            "run-context-d-terminal",
            "event-context-d-created",
            "event-context-d-start-requested",
        ))
        .expect("terminal candidate");
    store
        .fail_managed_run_start(start_failure(
            "run-context-d-terminal",
            "event-context-d-failed",
        ))
        .expect("terminal Run");
    store
        .create_managed_run_intent(run_intent(
            "run-context-e-replay-terminal",
            "event-context-e-created",
            "event-context-e-start-requested",
        ))
        .expect("replay-terminal candidate");
    store
        .connect_initial_managed_session(session_connection(
            "session-context-e",
            "run-context-e-replay-terminal",
            "thread-context-e",
            &project_path,
        ))
        .expect("replay-terminal session");
    store
        .append_event(session_event(
            "event-context-e-completed",
            "run-context-e-replay-terminal",
            "session-context-e",
            2,
            "run.completed",
        ))
        .expect("generic replay terminal");
    assert_eq!(
        store
            .managed_run("run-context-e-replay-terminal")
            .expect("replay-terminal Run")
            .expect("replay-terminal row")
            .ended_at,
        None
    );
    let connection = Connection::open(&database).expect("stale active row connection");
    connection
        .execute(
            "UPDATE runs SET ended_at = ?1 WHERE id = 'run-context-a-starting'",
            [ENDED_AT],
        )
        .expect("install stale active row terminal marker");
    drop(connection);

    let cursor = store.latest_ingest_seq().expect("assessment cursor");
    let contexts = store
        .managed_stuck_assessment_contexts()
        .expect("assessment contexts");
    assert_eq!(
        contexts
            .iter()
            .map(|context| context.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "run-context-a-starting",
            "run-context-b-blocked",
            "run-context-c-stuck"
        ]
    );
    assert_eq!(contexts[0].lifecycle, ManagedStuckLifecycle::Starting);
    assert_eq!(contexts[0].activity, ManagedStuckActivity::Unknown);
    assert_eq!(contexts[0].version, 3);
    assert_eq!(
        contexts[0].progress_event_id,
        "event-run-context-a-starting-created"
    );
    assert_eq!(contexts[0].progress_observed_at, CREATED_AT);
    assert_eq!(contexts[0].active_occurrence_id, None);
    assert_eq!(contexts[1].lifecycle, ManagedStuckLifecycle::Running);
    assert_eq!(contexts[1].activity, ManagedStuckActivity::Waiting);
    assert_eq!(
        contexts[1].wait_kind,
        Some(ManagedStuckWaitKind::BlockingRequest)
    );
    assert!(contexts[1].has_open_blocking_request);
    assert_eq!(contexts[1].active_occurrence_id, None);
    assert_eq!(contexts[2].lifecycle, ManagedStuckLifecycle::Running);
    assert_eq!(contexts[2].activity, ManagedStuckActivity::Unknown);
    assert_eq!(contexts[2].version, stuck_version);
    assert_eq!(
        contexts[2].active_occurrence_id.as_deref(),
        Some("occurrence-context-c")
    );
    assert_eq!(store.latest_ingest_seq().expect("unchanged cursor"), cursor);

    drop(store);
    let reopened = Store::open(&database, "2026-07-24T10:10:00Z").expect("reopen Store");
    assert_eq!(
        reopened
            .managed_stuck_assessment_contexts()
            .expect("reopened contexts"),
        contexts
    );
}

#[test]
fn stuck_assessment_contexts_reject_oversized_sets_and_partial_corrupt_replay() {
    let directory = TestDirectory::new("stuck-assessment-context-bounds");
    let (mut store, database, _project_path) = trusted_store(&directory);
    for index in 0..MAX_MANAGED_STUCK_ASSESSMENT_RUNS {
        store
            .create_managed_run_intent(run_intent(
                &format!("run-context-bound-{index:03}"),
                &format!("event-context-bound-{index:03}-created"),
                &format!("event-context-bound-{index:03}-requested"),
            ))
            .expect("bounded managed Run");
    }
    store
        .create_managed_run_intent(run_intent(
            "run-context-bound-terminal",
            "event-context-bound-terminal-created",
            "event-context-bound-terminal-requested",
        ))
        .expect("replay-terminal bounded Run");
    let project_path = store
        .project("project-1")
        .expect("bounded Project")
        .expect("bounded Project row")
        .canonical_path;
    store
        .connect_initial_managed_session(session_connection(
            "session-context-bound-terminal",
            "run-context-bound-terminal",
            "thread-context-bound-terminal",
            &project_path,
        ))
        .expect("bounded terminal session");
    store
        .append_event(session_event(
            "event-context-bound-terminal-completed",
            "run-context-bound-terminal",
            "session-context-bound-terminal",
            2,
            "run.completed",
        ))
        .expect("bounded generic terminal");
    let bounded = store
        .managed_stuck_assessment_contexts()
        .expect("exactly bounded active contexts");
    assert_eq!(bounded.len(), MAX_MANAGED_STUCK_ASSESSMENT_RUNS);
    assert!(
        bounded
            .iter()
            .all(|context| context.run_id != "run-context-bound-terminal")
    );
    let cursor = store.latest_ingest_seq().expect("bounded cursor");
    let connection = Connection::open(&database).expect("assessment corruption connection");
    connection
        .execute(
            "UPDATE events SET event_type = 'corrupt.created' WHERE event_id = 'event-context-bound-099-created'",
            [],
        )
        .expect("corrupt last ordered Run");
    drop(connection);

    assert!(matches!(
        store.managed_stuck_assessment_contexts(),
        Err(StoreError::DashboardProjection { run_id, .. })
            if run_id == "run-context-bound-099"
    ));
    assert_eq!(store.latest_ingest_seq().expect("corrupt cursor"), cursor);

    let connection = Connection::open(&database).expect("assessment repair connection");
    connection
        .execute(
            "UPDATE events SET event_type = 'run.created' WHERE event_id = 'event-context-bound-099-created'",
            [],
        )
        .expect("restore last ordered Run");
    drop(connection);
    store
        .create_managed_run_intent(run_intent(
            "run-context-bound-overflow",
            "event-context-bound-overflow-created",
            "event-context-bound-overflow-requested",
        ))
        .expect("overflow managed Run");
    let overflow_cursor = store.latest_ingest_seq().expect("overflow cursor");
    assert!(matches!(
        store.managed_stuck_assessment_contexts(),
        Err(StoreError::ManagedStuckAssessmentRunLimitExceeded { count, max })
            if count == MAX_MANAGED_STUCK_ASSESSMENT_RUNS + 1
                && max == MAX_MANAGED_STUCK_ASSESSMENT_RUNS
    ));
    assert_eq!(
        store
            .latest_ingest_seq()
            .expect("unchanged overflow cursor"),
        overflow_cursor
    );
}

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

#[test]
fn stuck_transitions_are_exact_atomic_and_same_state_consumes_no_cursor() {
    let directory = TestDirectory::new("stuck-transitions");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-stuck",
            "event-stuck-run-created",
            "event-stuck-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-stuck",
            "run-stuck",
            "thread-stuck",
            &project_path,
        ))
        .expect("managed session");

    let first_payload = PossiblyStuckPayload {
        occurrence_id: "occurrence-stuck-1".to_owned(),
        cause: StuckCauseCode::Unknown,
        threshold_seconds: 120,
        progress_event_id: "event-stuck-run-created".to_owned(),
        progress_observed_at: CREATED_AT.to_owned(),
        progress_monotonic_ms: 5_000,
        baseline_monotonic_ms: 5_000,
        stuck_since_monotonic_ms: 125_000,
        process: StuckProcessReceipt::Alive {
            generation: "process-generation-stuck-1".to_owned(),
            observed_monotonic_ms: 130_000,
        },
        evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
    };
    let first = ManagedStuckTransition {
        run_id: "run-stuck".to_owned(),
        expected_run_version: 4,
        event_id: "event-stuck-1".to_owned(),
        observed_at: "2026-07-24T10:02:10Z".to_owned(),
        assessment: ManagedStuckAssessment::PossiblyStuck(first_payload.clone()),
    };
    for invalid in [
        {
            let mut invalid = first.clone();
            let ManagedStuckAssessment::PossiblyStuck(payload) = &mut invalid.assessment else {
                unreachable!()
            };
            payload.progress_event_id = "event-missing-progress".to_owned();
            invalid
        },
        {
            let mut invalid = first.clone();
            let ManagedStuckAssessment::PossiblyStuck(payload) = &mut invalid.assessment else {
                unreachable!()
            };
            payload.progress_observed_at = "2026-07-24T10:00:09Z".to_owned();
            invalid
        },
        {
            let mut invalid = first.clone();
            let ManagedStuckAssessment::PossiblyStuck(payload) = &mut invalid.assessment else {
                unreachable!()
            };
            payload.cause = StuckCauseCode::Editing;
            invalid
        },
    ] {
        assert!(matches!(
            store.append_managed_stuck_transition(invalid),
            Err(StoreError::ManagedStuckProgressMismatch { .. })
        ));
        assert_eq!(
            store
                .run_snapshot("run-stuck")
                .expect("unchanged authority snapshot")
                .expect("unchanged authority projection")
                .version,
            4
        );
    }
    let first_event = match store
        .append_managed_stuck_transition(first.clone())
        .expect("first stuck transition")
    {
        ManagedStuckTransitionOutcome::Appended(event) => event,
        other => panic!("unexpected first transition: {other:?}"),
    };
    assert_eq!(first_event.ingest_seq, 5);
    let stuck = store
        .run_snapshot("run-stuck")
        .expect("stuck snapshot")
        .expect("stuck projection");
    assert_eq!(stuck.version, 5);
    assert_eq!(stuck.dashboard_bucket, "PossiblyStuck");
    assert_eq!(stuck.attention_level, "Informational");
    assert_eq!(stuck.snapshot["attention"]["open_count"], 1);

    let mut same_payload = first_payload.clone();
    same_payload.process = StuckProcessReceipt::Alive {
        generation: "process-generation-stuck-1".to_owned(),
        observed_monotonic_ms: 140_000,
    };
    let unchanged = ManagedStuckTransition {
        run_id: "run-stuck".to_owned(),
        expected_run_version: 5,
        event_id: "event-stuck-same-assessment".to_owned(),
        observed_at: "2026-07-24T10:02:20Z".to_owned(),
        assessment: ManagedStuckAssessment::PossiblyStuck(same_payload),
    };
    assert_eq!(
        store
            .append_managed_stuck_transition(unchanged)
            .expect("same assessment"),
        ManagedStuckTransitionOutcome::Unchanged {
            run_id: "run-stuck".to_owned(),
            version: 5,
        }
    );
    assert_eq!(
        store
            .run_snapshot("run-stuck")
            .expect("unchanged snapshot")
            .expect("unchanged projection")
            .version,
        5
    );
    assert!(matches!(
        store.append_managed_stuck_transition(first),
        Err(StoreError::ManagedStuckRunVersionStale {
            expected: 4,
            current: 5,
            ..
        })
    ));

    let mismatched_clear = ManagedStuckTransition {
        run_id: "run-stuck".to_owned(),
        expected_run_version: 5,
        event_id: "event-stuck-clear-stale".to_owned(),
        observed_at: "2026-07-24T10:02:16Z".to_owned(),
        assessment: ManagedStuckAssessment::Clear(StuckClearedPayload {
            occurrence_id: "occurrence-stale".to_owned(),
            reason: StuckClearReasonCode::ProgressObserved,
            process: StuckProcessReceipt::Alive {
                generation: "process-generation-stuck-1".to_owned(),
                observed_monotonic_ms: 136_000,
            },
            evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
        }),
    };
    assert!(matches!(
        store.append_managed_stuck_transition(mismatched_clear),
        Err(StoreError::ManagedStuckOccurrenceMismatch { .. })
    ));

    let clear_first = ManagedStuckTransition {
        run_id: "run-stuck".to_owned(),
        expected_run_version: 5,
        event_id: "event-stuck-clear-1".to_owned(),
        observed_at: "2026-07-24T10:02:17Z".to_owned(),
        assessment: ManagedStuckAssessment::Clear(StuckClearedPayload {
            occurrence_id: "occurrence-stuck-1".to_owned(),
            reason: StuckClearReasonCode::ProgressObserved,
            process: StuckProcessReceipt::Alive {
                generation: "process-generation-stuck-1".to_owned(),
                observed_monotonic_ms: 137_000,
            },
            evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
        }),
    };
    let first_clear_event = match store
        .append_managed_stuck_transition(clear_first)
        .expect("first clear transition")
    {
        ManagedStuckTransitionOutcome::Appended(event) => event,
        other => panic!("unexpected clear: {other:?}"),
    };
    assert_eq!(first_clear_event.ingest_seq, 6);
    let working = store
        .run_snapshot("run-stuck")
        .expect("working snapshot")
        .expect("working projection");
    assert_eq!(working.dashboard_bucket, "Working");
    assert_eq!(working.snapshot["attention"]["open_count"], 0);

    let second_payload = PossiblyStuckPayload {
        occurrence_id: "occurrence-stuck-2".to_owned(),
        process: StuckProcessReceipt::Alive {
            generation: "process-generation-stuck-1".to_owned(),
            observed_monotonic_ms: 145_000,
        },
        ..first_payload
    };
    let second = ManagedStuckTransition {
        run_id: "run-stuck".to_owned(),
        expected_run_version: 6,
        event_id: "event-stuck-2".to_owned(),
        observed_at: "2026-07-24T10:02:25Z".to_owned(),
        assessment: ManagedStuckAssessment::PossiblyStuck(second_payload),
    };
    let second_event = match store
        .append_managed_stuck_transition(second)
        .expect("same facts with a new occurrence identity")
    {
        ManagedStuckTransitionOutcome::Appended(event) => event,
        other => panic!("unexpected second occurrence: {other:?}"),
    };
    assert_eq!(second_event.ingest_seq, 7);
    assert_eq!(
        store
            .run_snapshot("run-stuck")
            .expect("second stuck snapshot")
            .expect("second stuck projection")
            .snapshot["attention"]["open_count"],
        1
    );

    let clear = ManagedStuckTransition {
        run_id: "run-stuck".to_owned(),
        expected_run_version: 7,
        event_id: "event-stuck-clear-2".to_owned(),
        observed_at: "2026-07-24T10:02:27Z".to_owned(),
        assessment: ManagedStuckAssessment::Clear(StuckClearedPayload {
            occurrence_id: "occurrence-stuck-2".to_owned(),
            reason: StuckClearReasonCode::ProgressObserved,
            process: StuckProcessReceipt::Alive {
                generation: "process-generation-stuck-1".to_owned(),
                observed_monotonic_ms: 147_000,
            },
            evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
        }),
    };
    let clear_event = match store
        .append_managed_stuck_transition(clear.clone())
        .expect("second clear transition")
    {
        ManagedStuckTransitionOutcome::Appended(event) => event,
        other => panic!("unexpected second clear: {other:?}"),
    };
    assert_eq!(clear_event.ingest_seq, 8);

    let already_clear = ManagedStuckTransition {
        expected_run_version: 8,
        event_id: "event-stuck-already-clear".to_owned(),
        ..clear
    };
    assert_eq!(
        store
            .append_managed_stuck_transition(already_clear)
            .expect("already clear"),
        ManagedStuckTransitionOutcome::Unchanged {
            run_id: "run-stuck".to_owned(),
            version: 8,
        }
    );
}

#[test]
fn stuck_transition_rejects_cross_run_progress_and_non_core_sources() {
    let directory = TestDirectory::new("stuck-authority");
    let (mut store, database, project_path) = trusted_store(&directory);
    for (run, session, thread) in [
        ("run-stuck-a", "session-stuck-a", "thread-stuck-a"),
        ("run-stuck-b", "session-stuck-b", "thread-stuck-b"),
    ] {
        store
            .create_managed_run_intent(run_intent(
                run,
                &format!("event-{run}-created"),
                &format!("event-{run}-start-requested"),
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
    let run_a_version = store
        .run_snapshot("run-stuck-a")
        .expect("Run A snapshot")
        .expect("Run A projection")
        .version;
    let cross_run = ManagedStuckTransition {
        run_id: "run-stuck-a".to_owned(),
        expected_run_version: run_a_version,
        event_id: "event-cross-run-stuck".to_owned(),
        observed_at: "2026-07-24T10:02:10Z".to_owned(),
        assessment: ManagedStuckAssessment::PossiblyStuck(PossiblyStuckPayload {
            occurrence_id: "occurrence-cross-run".to_owned(),
            cause: StuckCauseCode::Unknown,
            threshold_seconds: 120,
            progress_event_id: "event-run-stuck-b-created".to_owned(),
            progress_observed_at: CREATED_AT.to_owned(),
            progress_monotonic_ms: 5_000,
            baseline_monotonic_ms: 5_000,
            stuck_since_monotonic_ms: 125_000,
            process: StuckProcessReceipt::Alive {
                generation: "process-generation-a".to_owned(),
                observed_monotonic_ms: 130_000,
            },
            evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
        }),
    };
    let mut valid = cross_run.clone();
    valid.event_id = "event-valid-stuck-source".to_owned();
    let ManagedStuckAssessment::PossiblyStuck(payload) = &mut valid.assessment else {
        unreachable!()
    };
    payload.progress_event_id = "event-run-stuck-a-created".to_owned();
    assert!(matches!(
        store.append_managed_stuck_transition(cross_run),
        Err(StoreError::ManagedStuckProgressMismatch { .. })
    ));

    let mut provider_owned = session_event(
        "event-provider-owned-stuck",
        "run-stuck-a",
        "session-stuck-a",
        2,
        "run.possibly_stuck",
    );
    provider_owned.protocol_version = EventProtocolVersion::V1_3;
    assert!(matches!(
        store.append_event(provider_owned),
        Err(StoreError::InvalidEvent {
            field: "stuck_source"
        })
    ));

    let mut ui_owned = session_event(
        "event-ui-owned-clear",
        "run-stuck-a",
        "session-stuck-a",
        2,
        "run.stuck_cleared",
    );
    ui_owned.protocol_version = EventProtocolVersion::V1_3;
    ui_owned.session_id = NullableSessionId::Null;
    ui_owned.source.kind = EventSourceKind::Ui;
    ui_owned.source.provider = None;
    assert!(matches!(
        store.append_event(ui_owned),
        Err(StoreError::InvalidEvent {
            field: "stuck_source"
        })
    ));
    assert_eq!(
        store
            .run_snapshot("run-stuck-a")
            .expect("unchanged source snapshot")
            .expect("unchanged source projection")
            .version,
        run_a_version
    );

    assert!(matches!(
        store
            .append_managed_stuck_transition(valid)
            .expect("valid authoritative stuck transition"),
        ManagedStuckTransitionOutcome::Appended(_)
    ));
    drop(store);

    let connection = Connection::open(&database).expect("stuck source mutation connection");
    let stored_source: String = connection
        .query_row(
            "SELECT source_json FROM events WHERE event_id = 'event-valid-stuck-source'",
            [],
            |row| row.get(0),
        )
        .expect("stored stuck source");
    let mut malformed: Value = serde_json::from_str(&stored_source).expect("source JSON");
    malformed["unexpected"] = json!(true);
    connection
        .execute(
            "UPDATE events SET source_json = ?1 WHERE event_id = 'event-valid-stuck-source'",
            [serde_json::to_string(&malformed).expect("malformed source JSON")],
        )
        .expect("install malformed stuck source");
    drop(connection);

    assert!(matches!(
        Store::open(&database, CREATED_AT),
        Err(StoreError::DashboardProjection { .. })
    ));
}

#[test]
fn persisted_stuck_string_bounds_fail_closed_on_reopen() {
    let directory = TestDirectory::new("stuck-string-bounds");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-stuck-string-bounds",
            "event-stuck-string-bounds-created",
            "event-stuck-string-bounds-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-stuck-string-bounds",
            "run-stuck-string-bounds",
            "thread-stuck-string-bounds",
            &project_path,
        ))
        .expect("managed session");

    let occurrence_id = "occurrence-stuck-string-bounds";
    assert!(matches!(
        store
            .append_managed_stuck_transition(ManagedStuckTransition {
                run_id: "run-stuck-string-bounds".to_owned(),
                expected_run_version: 4,
                event_id: "event-stuck-string-bounds-open".to_owned(),
                observed_at: "2026-07-24T10:02:10Z".to_owned(),
                assessment: ManagedStuckAssessment::PossiblyStuck(PossiblyStuckPayload {
                    occurrence_id: occurrence_id.to_owned(),
                    cause: StuckCauseCode::Unknown,
                    threshold_seconds: 120,
                    progress_event_id: "event-stuck-string-bounds-created".to_owned(),
                    progress_observed_at: CREATED_AT.to_owned(),
                    progress_monotonic_ms: 5_000,
                    baseline_monotonic_ms: 5_000,
                    stuck_since_monotonic_ms: 125_000,
                    process: StuckProcessReceipt::Alive {
                        generation: "process-generation-bounded".to_owned(),
                        observed_monotonic_ms: 130_000,
                    },
                    evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
                }),
            })
            .expect("open stuck occurrence"),
        ManagedStuckTransitionOutcome::Appended(_)
    ));
    assert!(matches!(
        store
            .append_managed_stuck_transition(ManagedStuckTransition {
                run_id: "run-stuck-string-bounds".to_owned(),
                expected_run_version: 5,
                event_id: "event-stuck-string-bounds-clear".to_owned(),
                observed_at: "2026-07-24T10:02:20Z".to_owned(),
                assessment: ManagedStuckAssessment::Clear(StuckClearedPayload {
                    occurrence_id: occurrence_id.to_owned(),
                    reason: StuckClearReasonCode::ProcessUnavailable,
                    process: StuckProcessReceipt::Unavailable {
                        generation: Some("process-generation-bounded".to_owned()),
                        reason: "provider_process_probe_unavailable".to_owned(),
                        observed_monotonic_ms: 140_000,
                    },
                    evidence_unavailable_reason: "raw_provider_content_not_retained".to_owned(),
                }),
            })
            .expect("clear stuck occurrence"),
        ManagedStuckTransitionOutcome::Appended(_)
    ));
    drop(store);

    let connection = Connection::open(&database).expect("stuck string mutation connection");
    let open_source: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE event_id = 'event-stuck-string-bounds-open'",
            [],
            |row| row.get(0),
        )
        .expect("open payload JSON");
    let clear_source: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE event_id = 'event-stuck-string-bounds-clear'",
            [],
            |row| row.get(0),
        )
        .expect("clear payload JSON");
    drop(connection);
    let open_payload: Value = serde_json::from_str(&open_source).expect("open payload");
    let clear_payload: Value = serde_json::from_str(&clear_source).expect("clear payload");

    let mut malformed_cases = Vec::new();
    for (path, value) in [
        ("/occurrence_id", json!("x".repeat(257))),
        ("/progress_event_id", json!("x".repeat(257))),
        ("/progress_observed_at", json!("x".repeat(129))),
        ("/process/generation", json!("x".repeat(257))),
        (
            "/evidence_unavailable_reason",
            json!("x".repeat(4 * 1024 + 1)),
        ),
    ] {
        let mut malformed = open_payload.clone();
        *malformed.pointer_mut(path).expect("open mutation path") = value;
        malformed_cases.push(("event-stuck-string-bounds-open", malformed));
    }
    for (path, value) in [
        ("/occurrence_id", json!("x".repeat(257))),
        ("/process/generation", json!("x".repeat(257))),
        ("/process/reason", json!("x".repeat(4 * 1024 + 1))),
        (
            "/evidence_unavailable_reason",
            json!("x".repeat(4 * 1024 + 1)),
        ),
    ] {
        let mut malformed = clear_payload.clone();
        *malformed.pointer_mut(path).expect("clear mutation path") = value;
        malformed_cases.push(("event-stuck-string-bounds-clear", malformed));
    }

    for (event_id, malformed) in malformed_cases {
        let connection = Connection::open(&database).expect("malformed payload connection");
        connection
            .execute(
                "UPDATE events SET payload_json = ?1 WHERE event_id = ?2",
                params![
                    serde_json::to_string(&malformed).expect("malformed payload JSON"),
                    event_id
                ],
            )
            .expect("install malformed payload");
        drop(connection);
        assert!(matches!(
            Store::open(&database, CREATED_AT),
            Err(StoreError::DashboardProjection { .. })
        ));
        let connection = Connection::open(&database).expect("restore payload connection");
        connection
            .execute(
                "UPDATE events SET payload_json = ?1 WHERE event_id = ?2",
                params![
                    if event_id == "event-stuck-string-bounds-open" {
                        &open_source
                    } else {
                        &clear_source
                    },
                    event_id
                ],
            )
            .expect("restore valid payload");
    }
}

#[test]
fn legacy_same_name_stuck_events_remain_unknown_compatible_across_reopen() {
    let directory = TestDirectory::new("legacy-stuck-names");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-legacy-stuck-names",
            "event-legacy-stuck-run-created",
            "event-legacy-stuck-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-legacy-stuck-names",
            "run-legacy-stuck-names",
            "thread-legacy-stuck-names",
            &project_path,
        ))
        .expect("managed session");
    let initial = store
        .run_snapshot("run-legacy-stuck-names")
        .expect("initial snapshot")
        .expect("initial projection");

    let versions = [
        EventProtocolVersion::V1_0,
        EventProtocolVersion::V1_1,
        EventProtocolVersion::V1_2,
    ];
    let names = ["run.possibly_stuck", "run.stuck_cleared"];
    for (index, (version, name)) in versions
        .into_iter()
        .flat_map(|version| names.into_iter().map(move |name| (version, name)))
        .enumerate()
    {
        let mut event = session_event(
            &format!("event-legacy-stuck-name-{index}"),
            "run-legacy-stuck-names",
            "session-legacy-stuck-names",
            index as u64 + 2,
            name,
        );
        event.protocol_version = version;
        assert!(matches!(
            store.append_event(event).expect("legacy event append"),
            AppendEventOutcome::Inserted(_)
        ));
    }

    let appended = store.events_after(4, 6).expect("read legacy events");
    assert_eq!(appended.len(), 6);
    let projected = store
        .run_snapshot("run-legacy-stuck-names")
        .expect("unknown-compatible snapshot")
        .expect("unknown-compatible projection");
    assert_eq!(projected.lifecycle, initial.lifecycle);
    assert_eq!(projected.activity, initial.activity);
    assert_eq!(projected.dashboard_bucket, initial.dashboard_bucket);
    assert_eq!(projected.attention_level, initial.attention_level);
    assert_eq!(projected.snapshot["attention"]["open_count"], 0);

    drop(store);
    let reopened = Store::open(&database, "2026-07-24T10:10:00Z").expect("reopen Store");
    let reopened_events = reopened.events_after(4, 6).expect("read reopened events");
    assert_eq!(reopened_events, appended);
    assert_eq!(
        reopened
            .run_snapshot("run-legacy-stuck-names")
            .expect("reopened snapshot")
            .expect("reopened projection"),
        projected
    );
}

#[test]
fn managed_events_atomically_advance_truthful_dashboard_projection_and_reopen() {
    let directory = TestDirectory::new("dashboard-projection");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-1",
            "event-run-created",
            "event-start-requested",
        ))
        .expect("managed Run");
    let starting = store
        .run_snapshot("run-1")
        .expect("starting snapshot")
        .expect("starting projection");
    assert_eq!(starting.version, 3);
    assert_eq!(starting.lifecycle, "Starting");
    assert_eq!(starting.activity, "Unknown");
    assert_eq!(starting.dashboard_bucket, "Working");
    assert_eq!(
        starting.snapshot["changes"],
        json!({
            "availability": "unavailable",
            "reason": "git_observation_not_configured"
        })
    );

    store
        .connect_initial_managed_session(session_connection(
            "session-1",
            "run-1",
            "thread-1",
            &project_path,
        ))
        .expect("managed session");
    let running = store
        .run_snapshot("run-1")
        .expect("running snapshot")
        .expect("running projection");
    assert_eq!(running.version, 4);
    assert_eq!(running.lifecycle, "Running");

    let request = match store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-1".to_owned(),
            session_id: "session-1".to_owned(),
            external_session_key: "thread-1".to_owned(),
            provider_turn_id: "turn-1".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: "event-permission-requested".to_owned(),
            observed_at: "2026-07-24T10:00:02Z".to_owned(),
            kind: ManagedProviderObservationKind::PermissionRequested {
                request_id: "request-permission".to_owned(),
                provider_request_id: 17,
                provider_item_id: "permission-item".to_owned(),
                provider_started_at_ms: 42,
            },
        })
        .expect("permission request")
    {
        AppendEventOutcome::Inserted(event) => event,
        other => panic!("unexpected permission request: {other:?}"),
    };
    let waiting = store
        .run_snapshot("run-1")
        .expect("waiting snapshot")
        .expect("waiting projection");
    assert_eq!(waiting.version, request.ingest_seq);
    assert_eq!(waiting.activity, "Waiting");
    assert_eq!(waiting.attention_level, "ActionRequired");
    assert_eq!(waiting.dashboard_bucket, "NeedsAttention");
    assert_eq!(waiting.snapshot["attention"]["open_count"], 1);

    let attempt = permission_attempt(
        &request,
        "attempt-projection",
        ManagedPermissionDecision::Deny,
    );
    store
        .submit_managed_permission_response(attempt.clone())
        .expect("submit response");
    let result = permission_result(
        &attempt,
        "event-response-resolved-projection",
        ManagedPermissionResponseResultKind::Resolved(ManagedPermissionResolutionKind::Declined),
    );
    let resolved = store
        .finish_managed_permission_response(result)
        .expect("finish response");
    let resolved_event = match resolved {
        AppendEventOutcome::Inserted(event) => event,
        other => panic!("unexpected response result: {other:?}"),
    };
    let projection = store
        .run_snapshot("run-1")
        .expect("resolved snapshot")
        .expect("resolved projection");
    assert_eq!(projection.version, resolved_event.ingest_seq);
    assert_eq!(projection.attention_level, "None");
    assert_eq!(projection.dashboard_bucket, "Working");

    let dashboard = store
        .dashboard_run_snapshots_through(resolved_event.ingest_seq)
        .expect("Dashboard projection");
    assert_eq!(
        dashboard[0].changes,
        DashboardChangeSummary::Unavailable {
            reason: "git_observation_not_configured".to_owned(),
        }
    );
    drop(store);

    let reopened = Store::open(&database, CREATED_AT).expect("reopen projection Store");
    assert_eq!(
        reopened
            .run_snapshot("run-1")
            .expect("reopened snapshot")
            .expect("reopened projection"),
        projection
    );
}

#[test]
fn startup_rebuilds_missing_projection_but_same_version_corruption_fails_closed() {
    let directory = TestDirectory::new("dashboard-rebuild");
    let (mut store, database, _project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-rebuild",
            "event-rebuild-created",
            "event-rebuild-requested",
        ))
        .expect("managed Run");
    let expected = store
        .run_snapshot("run-rebuild")
        .expect("projection read")
        .expect("projection");
    drop(store);

    let connection = Connection::open(&database).expect("projection delete connection");
    connection
        .execute("DELETE FROM run_snapshots WHERE run_id = 'run-rebuild'", [])
        .expect("delete derived projection");
    drop(connection);
    let rebuilt = Store::open(&database, CREATED_AT).expect("rebuild missing projection");
    assert_eq!(
        rebuilt
            .run_snapshot("run-rebuild")
            .expect("rebuilt read")
            .expect("rebuilt projection"),
        expected
    );
    drop(rebuilt);

    let connection = Connection::open(&database).expect("projection corruption connection");
    connection
        .execute(
            "UPDATE run_snapshots SET snapshot_json = '{}' WHERE run_id = 'run-rebuild'",
            [],
        )
        .expect("corrupt same-version projection");
    drop(connection);
    assert!(matches!(
        Store::open(&database, CREATED_AT),
        Err(StoreError::StoredRunSnapshotInvalid { .. })
    ));
    let connection = Connection::open(&database).expect("inspect corrupt projection");
    let stored: String = connection
        .query_row(
            "SELECT snapshot_json FROM run_snapshots WHERE run_id = 'run-rebuild'",
            [],
            |row| row.get(0),
        )
        .expect("stored corrupt projection");
    assert_eq!(stored, "{}");
}

#[test]
fn startup_projection_source_bound_counts_utf8_bytes_before_hydration() {
    let directory = TestDirectory::new("dashboard-projection-byte-bound");
    let (mut store, database, _project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-byte-bound",
            "event-byte-bound-created",
            "event-byte-bound-requested",
        ))
        .expect("managed Run");
    drop(store);

    let payload = serde_json::to_string(&json!({
        "large": "é".repeat(MAX_DASHBOARD_PROJECTION_SOURCE_BYTES / 2 + 1)
    }))
    .expect("multibyte payload");
    assert!(payload.chars().count() < MAX_DASHBOARD_PROJECTION_SOURCE_BYTES);
    assert!(payload.len() > MAX_DASHBOARD_PROJECTION_SOURCE_BYTES);
    let connection = Connection::open(&database).expect("projection byte-bound connection");
    connection
        .execute(
            "UPDATE events SET payload_json = ?1 WHERE run_id = 'run-byte-bound' AND event_type = 'run.start_requested'",
            [payload],
        )
        .expect("install multibyte projection payload");
    drop(connection);

    assert!(matches!(
        Store::open(&database, CREATED_AT),
        Err(StoreError::DashboardProjectionReadTooLarge {
            run_id,
            source_bytes,
            ..
        }) if run_id == "run-byte-bound"
            && source_bytes > MAX_DASHBOARD_PROJECTION_SOURCE_BYTES as i64
    ));
}

#[test]
fn startup_projection_bound_does_not_charge_legacy_source_extensions() {
    let directory = TestDirectory::new("legacy-source-projection-byte-bound");
    let (mut store, database, _project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-legacy-source-bound",
            "event-legacy-source-bound-created",
            "event-legacy-source-bound-requested",
        ))
        .expect("managed Run");
    let expected = store
        .run_snapshot("run-legacy-source-bound")
        .expect("projection read")
        .expect("projection");
    drop(store);

    let legacy_source = serde_json::to_string(&json!({
        "kind": "core",
        "legacy_extension": "x".repeat(MAX_DASHBOARD_PROJECTION_SOURCE_BYTES + 1)
    }))
    .expect("large legacy source");
    assert!(legacy_source.len() > MAX_DASHBOARD_PROJECTION_SOURCE_BYTES);
    let connection = Connection::open(&database).expect("legacy source connection");
    connection
        .execute(
            "UPDATE events SET source_json = ?1 WHERE event_id = 'event-legacy-source-bound-created'",
            [legacy_source],
        )
        .expect("install legacy source extension");
    drop(connection);

    let reopened = Store::open(&database, CREATED_AT).expect("reopen legacy source Store");
    assert_eq!(
        reopened
            .run_snapshot("run-legacy-source-bound")
            .expect("reopened projection read")
            .expect("reopened projection"),
        expected
    );
}

#[test]
fn projection_failure_rolls_back_new_event_without_repairing_prior_history() {
    let directory = TestDirectory::new("dashboard-projection-rollback");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-projection-rollback",
            "event-projection-rollback-created",
            "event-projection-rollback-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-projection-rollback",
            "run-projection-rollback",
            "thread-projection-rollback",
            &project_path,
        ))
        .expect("managed session");
    let cursor = store.latest_ingest_seq().expect("projection cursor");

    let connection = Connection::open(&database).expect("history corruption connection");
    connection
        .execute(
            "UPDATE events SET session_id = NULL WHERE run_id = 'run-projection-rollback' AND event_type = 'session.connected'",
            [],
        )
        .expect("corrupt connected identity");
    drop(connection);

    assert!(matches!(
        store.append_event(session_event(
            "event-projection-rollback-command",
            "run-projection-rollback",
            "session-projection-rollback",
            2,
            "command.started",
        )),
        Err(StoreError::DashboardProjection { .. })
    ));
    assert_eq!(
        store.latest_ingest_seq().expect("rolled back cursor"),
        cursor
    );
    let connection = Connection::open(&database).expect("inspect rolled-back event");
    let appended: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM events WHERE event_id = 'event-projection-rollback-command'",
            [],
            |row| row.get(0),
        )
        .expect("rolled-back event count");
    assert_eq!(appended, 0);
}

#[test]
fn managed_start_failure_is_terminal_idempotent_and_reopens_without_a_session() {
    let directory = TestDirectory::new("start-failure");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-failed-start",
            "event-failed-created",
            "event-failed-requested",
        ))
        .expect("create Run intent");
    let failure = start_failure("run-failed-start", "event-failed-terminal");
    let (failed_run, failed_event) = match store
        .fail_managed_run_start(failure.clone())
        .expect("fail start")
    {
        ManagedRunStartFailureOutcome::Failed { run, event } => (run, event),
        other => panic!("unexpected failure outcome: {other:?}"),
    };
    assert_eq!(failed_run.started_at, None);
    assert_eq!(failed_run.ended_at.as_deref(), Some(ENDED_AT));
    assert_eq!(failed_event.event_type, "run.failed");
    assert_eq!(failed_event.ingest_seq, 4);
    assert_eq!(failed_event.stream_seq, 4);
    assert_eq!(failed_event.payload["reason"], "provider_start_failed");
    assert_eq!(failed_event.payload["stage"], "provider_start");
    assert_eq!(failed_event.source.kind, EventSourceKind::ProviderAdapter);
    assert_eq!(
        failed_event.source.contract_version.as_deref(),
        Some("codex-app-server/0.145.0")
    );

    assert!(matches!(
        store
            .fail_managed_run_start(failure)
            .expect("exact duplicate"),
        ManagedRunStartFailureOutcome::Duplicate { event, .. } if event.ingest_seq == 4
    ));
    assert_eq!(store.latest_ingest_seq().expect("duplicate cursor"), 4);

    let mut mismatch = start_failure("run-failed-start", "event-other-terminal");
    mismatch.reason = "different_failure".to_owned();
    assert!(matches!(
        store.fail_managed_run_start(mismatch),
        Err(StoreError::ManagedRunTerminalConflict { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("conflict cursor"), 4);
    assert!(matches!(
        store.connect_initial_managed_session(session_connection(
            "session-after-failure",
            "run-failed-start",
            "thread-after-failure",
            &project_path,
        )),
        Err(StoreError::ManagedRunTerminalConflict { .. })
    ));
    assert_eq!(
        store
            .managed_session("session-after-failure")
            .expect("no session"),
        None
    );

    drop(store);
    let mut reopened = Store::open(&database, CREATED_AT).expect("reopen Store");
    assert_eq!(
        reopened.managed_run("run-failed-start").expect("read Run"),
        Some(failed_run)
    );
    assert!(matches!(
        reopened.connect_initial_managed_session(session_connection(
            "session-after-reopen",
            "run-failed-start",
            "thread-after-reopen",
            &project_path,
        )),
        Err(StoreError::ManagedRunTerminalConflict { .. })
    ));
}

#[test]
fn managed_start_failure_rejects_invalid_or_started_runs_without_mutation() {
    let directory = TestDirectory::new("start-failure-negative");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-started",
            "event-started-created",
            "event-started-requested",
        ))
        .expect("create Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-started",
            "run-started",
            "thread-started",
            &project_path,
        ))
        .expect("connect session");
    assert!(matches!(
        store.fail_managed_run_start(start_failure("run-started", "event-invalid-terminal")),
        Err(StoreError::ManagedRunAlreadyStarted { .. })
    ));
    let cursor = store.latest_ingest_seq().expect("started cursor");

    let mut invalid = start_failure("run-started", "event-invalid-reason");
    invalid.reason.clear();
    assert!(matches!(
        store.fail_managed_run_start(invalid),
        Err(StoreError::InvalidManagedRunStartFailure { field: "reason" })
    ));
    assert_eq!(store.latest_ingest_seq().expect("unchanged cursor"), cursor);

    store
        .create_managed_run_intent(run_intent(
            "run-rollback",
            "event-rollback-created",
            "event-rollback-requested",
        ))
        .expect("create rollback Run");
    let rollback_cursor = store.latest_ingest_seq().expect("rollback cursor");
    assert!(matches!(
        store.fail_managed_run_start(start_failure("run-rollback", "event-started-created")),
        Err(StoreError::EventIdentityConflict { .. })
    ));
    assert_eq!(
        store
            .managed_run("run-rollback")
            .expect("rollback Run")
            .expect("Run")
            .ended_at,
        None
    );
    assert_eq!(
        store.latest_ingest_seq().expect("rolled back cursor"),
        rollback_cursor
    );
    assert!(matches!(
        store
            .fail_managed_run_start(start_failure(
                "run-rollback",
                "event-rollback-terminal"
            ))
            .expect("valid failure after rollback"),
        ManagedRunStartFailureOutcome::Failed { event, .. }
            if event.ingest_seq == rollback_cursor + 1
    ));
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
        [
            "run.created",
            "git.snapshot_recorded",
            "run.start_requested"
        ]
    );
    assert_eq!(
        created_events
            .iter()
            .map(|event| event.ingest_seq)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        created_run.goal.as_deref(),
        Some("Respond with the requested result.")
    );
    let created_payload = serde_json::to_string(&created_events[0].payload)
        .expect("run.created payload should serialize");
    assert!(!created_payload.contains("Respond with the requested result."));
    assert_eq!(
        created_events[0].payload.get("goal_sha256"),
        Some(&serde_json::json!(
            "a243866e405346bd1d66fd94322c0fa65e62cbda534a98cdb60f6a396a3802f9"
        ))
    );
    let duplicate = store
        .create_managed_run_intent(intent)
        .expect("duplicate Run intent");
    assert!(matches!(
        duplicate,
        ManagedRunIntentOutcome::Duplicate { ref events, .. }
            if events.iter().map(|event| event.ingest_seq).collect::<Vec<_>>() == [1, 2, 3]
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
        assert_eq!(store.latest_ingest_seq().expect("retry cursor"), 3);
        assert_eq!(
            store
                .run_events_through("run-1", 0, 3, 10)
                .expect("original Run events")
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            [
                "event-run-created",
                "event-run-created-git-baseline",
                "event-start-requested"
            ]
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
    assert_eq!(connected_event.ingest_seq, 4);
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
        InitialManagedSessionOutcome::Duplicate { event, .. } if event.ingest_seq == 4
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
            .run_events_through("run-1", 0, 4, 10)
            .expect("reopened event page")
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            "run.created",
            "git.snapshot_recorded",
            "run.start_requested",
            "session.connected"
        ]
    );
}

#[test]
fn managed_git_baseline_is_atomic_content_free_and_first_receipt_wins_retries() {
    let directory = TestDirectory::new("git-baseline");
    let (mut store, _database, _project_path) = trusted_store(&directory);
    let mut intent = run_intent(
        "run-git-baseline",
        "event-git-run-created",
        "event-git-start-requested",
    );
    intent.git_baseline = GitBaselinePayload::Available {
        project_id: "project-1".to_owned(),
        head: GitHead::Available {
            oid: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        },
        dirty: GitDirtySummary {
            staged: 1,
            unstaged: 1,
            untracked: 0,
            entries: 1,
        },
    };
    let (run, events) = match store
        .create_managed_run_intent(intent.clone())
        .expect("available baseline intent")
    {
        ManagedRunIntentOutcome::Created { run, events } => (run, events),
        other => panic!("unexpected baseline outcome: {other:?}"),
    };
    assert_eq!(
        run.baseline_head.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(events[1].event_type, "git.snapshot_recorded");
    assert_eq!(events[1].source.kind, EventSourceKind::Core);
    assert_eq!(
        events[1].source.contract_version.as_deref(),
        Some("git-baseline/1.0")
    );
    assert_eq!(events[1].payload["availability"], "available");
    assert_eq!(events[1].payload["dirty"]["entries"], 1);
    let rendered = serde_json::to_string(&events[1]).expect("baseline JSON");
    for forbidden in [
        directory.0.to_string_lossy().as_ref(),
        "stderr",
        "environment",
    ] {
        assert!(!rendered.contains(forbidden));
    }

    intent.git_baseline = GitBaselinePayload::Unavailable {
        project_id: "project-1".to_owned(),
        reason: GitBaselineUnavailableReason::ProcessUnavailable,
    };
    let duplicate = store
        .create_managed_run_intent(intent.clone())
        .expect("retry keeps first baseline");
    assert!(matches!(
        duplicate,
        ManagedRunIntentOutcome::Duplicate { ref events, .. }
            if events[1].payload["availability"] == "available"
                && events[1].ingest_seq == 2
    ));
    assert_eq!(store.latest_ingest_seq().expect("baseline cursor"), 3);

    intent.git_baseline_event_id = "event-conflicting-git-baseline".to_owned();
    assert!(matches!(
        store.create_managed_run_intent(intent),
        Err(StoreError::ManagedRunIdentityConflict { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("conflict cursor"), 3);
}

#[test]
fn managed_provider_observations_are_exact_ordered_idempotent_and_content_safe() {
    let directory = TestDirectory::new("provider-observation");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-1",
            "event-run-created",
            "event-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-1",
            "run-1",
            "thread-1",
            &project_path,
        ))
        .expect("managed session");

    let permission = ManagedProviderObservation {
        run_id: "run-1".to_owned(),
        session_id: "session-1".to_owned(),
        external_session_key: "thread-1".to_owned(),
        provider_turn_id: "turn-1".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-permission".to_owned(),
        observed_at: STARTED_AT.to_owned(),
        kind: ManagedProviderObservationKind::PermissionRequested {
            request_id: "request-permission".to_owned(),
            provider_request_id: 0,
            provider_item_id: "item-1".to_owned(),
            provider_started_at_ms: 17,
        },
    };
    let inserted = store
        .append_managed_provider_observation(permission.clone())
        .expect("permission observation");
    let event = match inserted {
        AppendEventOutcome::Inserted(event) => event,
        other => panic!("unexpected observation outcome: {other:?}"),
    };
    assert_eq!(event.stream_seq, 2);
    assert_eq!(event.event_type, "permission.requested");
    assert_eq!(event.payload["request_id"], "request-permission");
    assert_eq!(
        event.payload["evidence_unavailable_reason"],
        "raw_provider_content_not_retained"
    );
    assert!(event.evidence_ids.is_empty());
    let rendered = serde_json::to_string(&event).expect("event JSON");
    for raw in ["sensitive reason", "secret command", "/private/secret"] {
        assert!(!rendered.contains(raw));
    }
    assert!(matches!(
        store
            .append_managed_provider_observation(permission.clone())
            .expect("exact duplicate"),
        AppendEventOutcome::Duplicate(ref duplicate) if duplicate == &event
    ));

    let mut conflict = permission;
    conflict.kind = ManagedProviderObservationKind::PermissionRequested {
        request_id: "request-permission".to_owned(),
        provider_request_id: 0,
        provider_item_id: "different-item".to_owned(),
        provider_started_at_ms: 17,
    };
    assert!(matches!(
        store.append_managed_provider_observation(conflict),
        Err(StoreError::EventIdentityConflict { .. })
    ));

    let command = ManagedProviderObservation {
        run_id: "run-1".to_owned(),
        session_id: "session-1".to_owned(),
        external_session_key: "thread-1".to_owned(),
        provider_turn_id: "turn-1".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-command".to_owned(),
        observed_at: STARTED_AT.to_owned(),
        kind: ManagedProviderObservationKind::CommandStarted {
            provider_item_id: "command-item".to_owned(),
        },
    };
    assert!(matches!(
        store
            .append_managed_provider_observation(command)
            .expect("command observation"),
        AppendEventOutcome::Inserted(ref command) if command.stream_seq == 3
    ));

    let terminal = ManagedProviderObservation {
        run_id: "run-1".to_owned(),
        session_id: "session-1".to_owned(),
        external_session_key: "thread-1".to_owned(),
        provider_turn_id: "turn-1".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-terminal".to_owned(),
        observed_at: ENDED_AT.to_owned(),
        kind: ManagedProviderObservationKind::TurnCompleted {
            changes: ManagedGitChangeSummary::Exact {
                files: 2,
                insertions: 3,
                deletions: 1,
            },
            change_set: None,
        },
    };
    let terminal_event = store
        .append_managed_provider_observation(terminal.clone())
        .expect("terminal observation");
    assert!(matches!(
        &terminal_event,
        AppendEventOutcome::Inserted(terminal)
            if terminal.stream_seq == 4
                && terminal.protocol_version == EventProtocolVersion::V1_2
                && terminal.payload["changes"] == json!({
                    "availability": "available",
                    "attribution": "exact",
                    "files": 2,
                    "insertions": 3,
                    "deletions": 1
                })
    ));
    assert_eq!(
        store
            .run_snapshot("run-1")
            .expect("terminal snapshot")
            .expect("terminal snapshot")
            .snapshot["changes"],
        json!({
            "availability": "available",
            "attribution": "exact",
            "files": 2,
            "insertions": 3,
            "deletions": 1
        })
    );
    assert_eq!(
        store
            .managed_run("run-1")
            .expect("Run")
            .expect("Run")
            .ended_at
            .as_deref(),
        Some(ENDED_AT)
    );
    assert!(matches!(
        store
            .append_managed_provider_observation(terminal)
            .expect("terminal duplicate"),
        AppendEventOutcome::Duplicate(_)
    ));
}

#[test]
fn terminal_file_changes_are_atomic_sensitive_idempotent_and_restart_durable() {
    let directory = TestDirectory::new("terminal-file-changes");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent_with_clean_baseline(
            "run-changes",
            "event-changes-created",
            "event-changes-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-changes",
            "run-changes",
            "thread-changes",
            &project_path,
        ))
        .expect("managed session");
    let command = ManagedProviderObservation {
        run_id: "run-changes".to_owned(),
        session_id: "session-changes".to_owned(),
        external_session_key: "thread-changes".to_owned(),
        provider_turn_id: "turn-changes".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-changes-command".to_owned(),
        observed_at: STARTED_AT.to_owned(),
        kind: ManagedProviderObservationKind::CommandStarted {
            provider_item_id: "command-changes".to_owned(),
        },
    };
    store
        .append_managed_provider_observation(command.clone())
        .expect("command before terminal");
    let change_set = exact_file_change_set(&project_path);
    let terminal = ManagedProviderObservation {
        run_id: "run-changes".to_owned(),
        session_id: "session-changes".to_owned(),
        external_session_key: "thread-changes".to_owned(),
        provider_turn_id: "turn-changes".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-changes-terminal".to_owned(),
        observed_at: ENDED_AT.to_owned(),
        kind: ManagedProviderObservationKind::TurnCompleted {
            changes: ManagedGitChangeSummary::Exact {
                files: 2,
                insertions: 3,
                deletions: 1,
            },
            change_set: Some(Box::new(change_set.clone())),
        },
    };
    let event = store
        .append_managed_provider_observation(terminal.clone())
        .expect("terminal changes");
    let AppendEventOutcome::Inserted(event) = event else {
        panic!("expected inserted terminal event");
    };
    let event_json = serde_json::to_string(&event).expect("event JSON");
    assert!(!event_json.contains("inside-"));
    assert!(!event_json.contains("outside.txt"));
    assert_eq!(
        event.payload["changes"],
        json!({
            "availability": "available",
            "attribution": "exact",
            "files": 2,
            "insertions": 3,
            "deletions": 1
        })
    );
    let metadata = store
        .managed_git_change_set_metadata("run-changes")
        .expect("change metadata")
        .expect("stored change metadata");
    assert_eq!(metadata.attribution, ManagedGitChangeAttribution::Exact);
    assert_eq!(metadata.baseline_head, change_set.baseline_head);
    assert_eq!(metadata.terminal_head, change_set.terminal_head);
    assert_eq!(metadata.repository_identity, change_set.repository_identity);
    assert_eq!(metadata.files, 2);
    let first_page = store
        .managed_git_change_page("run-changes", None, 1)
        .expect("first change page")
        .expect("available first page");
    assert_eq!(first_page.metadata, metadata);
    assert_eq!(first_page.changes, change_set.changes[..1]);
    assert_eq!(first_page.next_cursor.as_deref(), Some(CHANGE_INSIDE_ID));
    assert!(first_page.has_more);
    let second_page = store
        .managed_git_change_page(
            "run-changes",
            first_page.next_cursor.as_deref(),
            MAX_MANAGED_GIT_CHANGE_PAGE_SIZE,
        )
        .expect("second change page")
        .expect("available second page");
    assert_eq!(second_page.changes, change_set.changes[1..]);
    assert_eq!(second_page.next_cursor.as_deref(), Some(CHANGE_OUTSIDE_ID));
    assert!(!second_page.has_more);
    let exhausted = store
        .managed_git_change_page(
            "run-changes",
            second_page.next_cursor.as_deref(),
            MAX_MANAGED_GIT_CHANGE_PAGE_SIZE,
        )
        .expect("exhausted change page")
        .expect("available exhausted page");
    assert!(exhausted.changes.is_empty());
    assert_eq!(exhausted.next_cursor, second_page.next_cursor);
    assert!(!exhausted.has_more);
    for (cursor, limit) in [
        (Some("not-an-opaque-id"), 1),
        (Some("00000000000000000000000000000000"), 1),
        (None, 0),
        (None, MAX_MANAGED_GIT_CHANGE_PAGE_SIZE + 1),
    ] {
        assert!(matches!(
            store.managed_git_change_page("run-changes", cursor, limit),
            Err(StoreError::InvalidManagedGitChangeRead { .. })
        ));
    }
    assert_eq!(
        store
            .managed_git_file_change("run-changes", CHANGE_INSIDE_ID)
            .expect("file change")
            .expect("stored file change"),
        change_set.changes[0]
    );
    assert!(matches!(
        store
            .append_managed_provider_observation(terminal.clone())
            .expect("exact duplicate"),
        AppendEventOutcome::Duplicate(_)
    ));
    assert!(matches!(
        store
            .append_managed_provider_observation(command)
            .expect("nonterminal duplicate after terminal"),
        AppendEventOutcome::Duplicate(_)
    ));

    let mut downgraded = terminal.clone();
    let ManagedProviderObservationKind::TurnCompleted {
        change_set: downgraded_set,
        ..
    } = &mut downgraded.kind
    else {
        unreachable!()
    };
    *downgraded_set = None;
    assert!(matches!(
        store.append_managed_provider_observation(downgraded),
        Err(StoreError::ManagedGitChangeSetConflict { ref run_id }) if run_id == "run-changes"
    ));

    drop(store);
    let reopened = Store::open(&database, CREATED_AT).expect("reopen Store");
    let stored = reopened
        .managed_git_file_change("run-changes", CHANGE_OUTSIDE_ID)
        .expect("reopened file change")
        .expect("reopened stored file change");
    assert_eq!(stored, change_set.changes[1]);
    assert!(!format!("{stored:?}").contains("outside.txt"));

    let corruption = Connection::open(&database).expect("corruption fixture");
    corruption
        .execute_batch(
            "PRAGMA ignore_check_constraints = ON;
             UPDATE run_git_file_changes SET status = 'corrupt' WHERE run_id = 'run-changes' AND change_id = '22222222222222222222222222222222';",
        )
        .expect("corrupt stored status");
    let error = reopened
        .managed_git_file_change("run-changes", CHANGE_OUTSIDE_ID)
        .expect_err("corrupt file change");
    assert!(matches!(
        error,
        StoreError::StoredManagedGitFileChangeInvalid {
            ref run_id,
            ref change_id,
            field: "status",
        } if run_id == "run-changes" && change_id == CHANGE_OUTSIDE_ID
    ));
    assert!(!error.to_string().contains("outside.txt"));
    corruption
        .execute(
            "UPDATE run_git_file_changes SET status = 'added' WHERE run_id = 'run-changes' AND change_id = '22222222222222222222222222222222'",
            [],
        )
        .expect("restore stored status");
    corruption
        .execute(
            "UPDATE run_git_file_changes SET insertions = 7 WHERE run_id = 'run-changes' AND change_id = '22222222222222222222222222222222'",
            [],
        )
        .expect("corrupt stored aggregate");
    assert!(matches!(
        reopened.managed_git_change_set_metadata("run-changes"),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "aggregate",
        }) if run_id == "run-changes"
    ));
    assert!(matches!(
        reopened.managed_git_file_change("run-changes", CHANGE_INSIDE_ID),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "aggregate",
        }) if run_id == "run-changes"
    ));
    corruption
        .execute(
            "UPDATE run_git_file_changes SET insertions = 1 WHERE run_id = 'run-changes' AND change_id = '22222222222222222222222222222222'",
            [],
        )
        .expect("restore stored aggregate");
    corruption
        .execute(
            "UPDATE run_git_change_sets SET file_count = 10001 WHERE run_id = 'run-changes'",
            [],
        )
        .expect("corrupt stored bound");
    assert!(matches!(
        reopened.managed_git_change_set_metadata("run-changes"),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "metadata",
        }) if run_id == "run-changes"
    ));
    corruption
        .execute(
            "UPDATE run_git_change_sets SET file_count = 2 WHERE run_id = 'run-changes'",
            [],
        )
        .expect("restore stored bound");
    corruption
        .execute(
            "UPDATE run_git_change_sets SET attribution = 'observed_during_run' WHERE run_id = 'run-changes'",
            [],
        )
        .expect("corrupt stored attribution");
    assert!(matches!(
        reopened.managed_git_change_set_metadata("run-changes"),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "terminal_event_changes",
        }) if run_id == "run-changes"
    ));
    corruption
        .execute(
            "UPDATE run_git_change_sets SET attribution = 'exact' WHERE run_id = 'run-changes'",
            [],
        )
        .expect("restore stored attribution");
    corruption
        .execute_batch(
            "UPDATE run_git_file_changes SET insertions = 7 WHERE run_id = 'run-changes' AND change_id = '22222222222222222222222222222222';
             UPDATE run_git_change_sets SET insertions = 9 WHERE run_id = 'run-changes';",
        )
        .expect("corrupt internally consistent totals");
    assert!(matches!(
        reopened.managed_git_change_set_metadata("run-changes"),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "terminal_event_changes",
        }) if run_id == "run-changes"
    ));
    corruption
        .execute_batch(
            "UPDATE run_git_file_changes SET insertions = 1 WHERE run_id = 'run-changes' AND change_id = '22222222222222222222222222222222';
             UPDATE run_git_change_sets SET insertions = 3 WHERE run_id = 'run-changes';",
        )
        .expect("restore stored totals");
    drop(corruption);
    drop(reopened);

    let connection = Connection::open(&database).expect("cascade fixture");
    connection
        .execute_batch("PRAGMA foreign_keys = ON; DELETE FROM runs WHERE id = 'run-changes';")
        .expect("delete Run");
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM run_git_file_changes WHERE run_id = 'run-changes'",
            [],
            |row| row.get(0),
        )
        .expect("remaining locator rows");
    assert_eq!(remaining, 0);
}

#[test]
fn managed_git_change_page_rejects_oversized_source_before_returning_records() {
    let directory = TestDirectory::new("change-page-source-bound");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent_with_clean_baseline(
            "run-large-changes",
            "event-large-changes-created",
            "event-large-changes-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-large-changes",
            "run-large-changes",
            "thread-large-changes",
            &project_path,
        ))
        .expect("managed session");
    assert!(
        store
            .managed_git_change_page("run-large-changes", None, 1)
            .expect("missing change set")
            .is_none()
    );
    assert!(matches!(
        store.managed_git_change_page(
            "run-large-changes",
            Some("00000000000000000000000000000000"),
            1,
        ),
        Err(StoreError::InvalidManagedGitChangeRead { field: "cursor" })
    ));

    let changes = (0_u8..51)
        .map(|index| {
            let mut raw_path = format!("{index:02}-").into_bytes();
            raw_path.resize(16 * 1024, 0xff);
            ManagedGitFileChange {
                change_id: format!("{index:032x}"),
                display_path: String::from_utf8_lossy(&raw_path).into_owned(),
                raw_path,
                status: ManagedGitFileStatus::Modified,
                committed: false,
                staged: true,
                unstaged: false,
                binary: false,
                insertions: Some(1),
                deletions: Some(0),
                project_scope: ManagedGitProjectScope::InsideProject,
            }
        })
        .collect::<Vec<_>>();
    let first_change_id = changes[0].change_id.clone();
    let late_change_id = changes[50].change_id.clone();
    let late_display_path = changes[50].display_path.clone();
    assert!(
        changes
            .iter()
            .map(|change| change.raw_path.len() + change.display_path.len())
            .sum::<usize>()
            > MAX_MANAGED_GIT_CHANGE_PAGE_SOURCE_BYTES
    );
    let file_count = changes.len() as u64;
    let change_set = ManagedGitChangeSet {
        attribution: ManagedGitChangeAttribution::Exact,
        baseline_head: Some("1".repeat(40)),
        terminal_head: Some("2".repeat(40)),
        repository_identity: test_repository_identity(&project_path),
        files: file_count,
        insertions: Some(file_count),
        deletions: Some(0),
        changes,
    };
    store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-large-changes".to_owned(),
            session_id: "session-large-changes".to_owned(),
            external_session_key: "thread-large-changes".to_owned(),
            provider_turn_id: "turn-large-changes".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: "event-large-changes-terminal".to_owned(),
            observed_at: ENDED_AT.to_owned(),
            kind: ManagedProviderObservationKind::TurnCompleted {
                changes: ManagedGitChangeSummary::Exact {
                    files: file_count,
                    insertions: file_count,
                    deletions: 0,
                },
                change_set: Some(Box::new(change_set)),
            },
        })
        .expect("terminal large changes");

    let first = store
        .managed_git_change_page("run-large-changes", None, 1)
        .expect("bounded first page")
        .expect("available first page");
    assert_eq!(first.changes.len(), 1);
    assert!(first.has_more);

    assert!(matches!(
        store.managed_git_change_page(
            "run-large-changes",
            None,
            MAX_MANAGED_GIT_CHANGE_PAGE_SIZE,
        ),
        Err(StoreError::ManagedGitChangeReadTooLarge {
            count: 50,
            source_bytes,
        }) if source_bytes > MAX_MANAGED_GIT_CHANGE_PAGE_SOURCE_BYTES as i64
    ));

    let corruption = Connection::open(&database).expect("late corruption fixture");
    corruption
        .pragma_update(None, "ignore_check_constraints", "ON")
        .expect("allow corruption fixture");
    corruption
        .execute(
            "UPDATE run_git_file_changes SET display_path = 'mismatched-display'
             WHERE run_id = 'run-large-changes' AND change_id = ?1",
            [&late_change_id],
        )
        .expect("corrupt late display path");
    assert!(matches!(
        store.managed_git_change_page("run-large-changes", None, 1),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "aggregate",
        }) if run_id == "run-large-changes"
    ));
    corruption
        .execute(
            "UPDATE run_git_file_changes SET display_path = ?1
             WHERE run_id = 'run-large-changes' AND change_id = ?2",
            params![late_display_path, late_change_id],
        )
        .expect("restore late display path");
    corruption
        .execute(
            "UPDATE run_git_file_changes SET display_path = ?1
             WHERE run_id = 'run-large-changes' AND change_id = ?2",
            params![
                "x".repeat(MAX_MANAGED_GIT_CHANGE_PAGE_SOURCE_BYTES + 1),
                first_change_id,
            ],
        )
        .expect("corrupt cursor display path beyond bounds");
    assert!(matches!(
        store.managed_git_change_page(
            "run-large-changes",
            Some("00000000000000000000000000000000"),
            1,
        ),
        Err(StoreError::StoredManagedGitChangeSetInvalid {
            ref run_id,
            field: "aggregate",
        }) if run_id == "run-large-changes"
    ));
}

#[test]
fn terminal_change_storage_failure_rolls_back_lifecycle_event_and_locators() {
    let directory = TestDirectory::new("terminal-change-rollback");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent_with_clean_baseline(
            "run-rollback",
            "event-rollback-created",
            "event-rollback-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-rollback",
            "run-rollback",
            "thread-rollback",
            &project_path,
        ))
        .expect("managed session");
    let connection = Connection::open(&database).expect("failure fixture");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_change_insert BEFORE INSERT ON run_git_file_changes
             BEGIN SELECT RAISE(ABORT, 'forced change failure'); END;",
        )
        .expect("failure trigger");
    drop(connection);
    let terminal = ManagedProviderObservation {
        run_id: "run-rollback".to_owned(),
        session_id: "session-rollback".to_owned(),
        external_session_key: "thread-rollback".to_owned(),
        provider_turn_id: "turn-rollback".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-rollback-terminal".to_owned(),
        observed_at: ENDED_AT.to_owned(),
        kind: ManagedProviderObservationKind::TurnCompleted {
            changes: ManagedGitChangeSummary::Exact {
                files: 2,
                insertions: 3,
                deletions: 1,
            },
            change_set: Some(Box::new(exact_file_change_set(&project_path))),
        },
    };
    assert!(matches!(
        store.append_managed_provider_observation(terminal),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(
        store
            .managed_run("run-rollback")
            .expect("Run")
            .expect("stored Run")
            .ended_at,
        None
    );
    assert_eq!(
        store
            .managed_session("session-rollback")
            .expect("session")
            .expect("stored session")
            .ended_at,
        None
    );
    assert_eq!(
        store
            .managed_git_change_set_metadata("run-rollback")
            .expect("missing metadata"),
        None
    );
    assert!(
        store
            .run_events_through(
                "run-rollback",
                0,
                store.latest_ingest_seq().expect("latest cursor"),
                10,
            )
            .expect("events")
            .events
            .iter()
            .all(|event| event.event_id != "event-rollback-terminal")
    );
}

#[test]
fn exact_file_changes_require_the_run_clean_baseline_and_matching_oid() {
    let directory = TestDirectory::new("terminal-change-baseline-binding");
    let (mut store, _database, project_path) = trusted_store(&directory);
    for case in ["unavailable", "dirty", "mismatched"] {
        let run_id = format!("run-baseline-{case}");
        let session_id = format!("session-baseline-{case}");
        let thread_id = format!("thread-baseline-{case}");
        let created_event_id = format!("event-baseline-{case}-created");
        let requested_event_id = format!("event-baseline-{case}-requested");
        let mut intent = if case == "unavailable" {
            run_intent(&run_id, &created_event_id, &requested_event_id)
        } else {
            run_intent_with_clean_baseline(&run_id, &created_event_id, &requested_event_id)
        };
        if case == "dirty" {
            let GitBaselinePayload::Available { dirty, .. } = &mut intent.git_baseline else {
                unreachable!()
            };
            dirty.unstaged = 1;
            dirty.entries = 1;
        }
        store
            .create_managed_run_intent(intent)
            .expect("managed Run");
        store
            .connect_initial_managed_session(session_connection(
                &session_id,
                &run_id,
                &thread_id,
                &project_path,
            ))
            .expect("managed session");
        let before = store.latest_ingest_seq().expect("cursor before rejection");
        let mut change_set = exact_file_change_set(&project_path);
        if case == "mismatched" {
            change_set.baseline_head = Some("9".repeat(40));
        }
        let observation = ManagedProviderObservation {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            external_session_key: thread_id,
            provider_turn_id: format!("turn-baseline-{case}"),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: format!("event-baseline-{case}-terminal"),
            observed_at: ENDED_AT.to_owned(),
            kind: ManagedProviderObservationKind::TurnCompleted {
                changes: ManagedGitChangeSummary::Exact {
                    files: 2,
                    insertions: 3,
                    deletions: 1,
                },
                change_set: Some(Box::new(change_set)),
            },
        };
        assert!(matches!(
            store.append_managed_provider_observation(observation),
            Err(StoreError::ManagedGitChangeBaselineMismatch { run_id: rejected })
                if rejected == run_id
        ));
        assert_eq!(store.latest_ingest_seq().expect("unchanged cursor"), before);
        assert_eq!(
            store
                .managed_run(&run_id)
                .expect("Run")
                .expect("stored Run")
                .ended_at,
            None
        );
        assert_eq!(
            store
                .managed_git_change_set_metadata(&run_id)
                .expect("missing change set"),
            None
        );
    }
}

#[test]
fn invalid_terminal_file_change_sets_fail_before_mutation_and_binary_remains_exactly_stored() {
    let directory = TestDirectory::new("terminal-change-validation");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent_with_clean_baseline(
            "run-validation",
            "event-validation-created",
            "event-validation-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-validation",
            "run-validation",
            "thread-validation",
            &project_path,
        ))
        .expect("managed session");
    let initial_cursor = store.latest_ingest_seq().expect("initial cursor");

    let mut invalid_sets = Vec::new();
    let mut invalid_id = exact_file_change_set(&project_path);
    invalid_id.changes[0].change_id = "path-shaped-id".to_owned();
    invalid_sets.push(invalid_id);
    let mut misleading_display = exact_file_change_set(&project_path);
    misleading_display.changes[0].display_path = "different.txt".to_owned();
    invalid_sets.push(misleading_display);
    let mut duplicate_path = exact_file_change_set(&project_path);
    duplicate_path.changes[1].raw_path = duplicate_path.changes[0].raw_path.clone();
    duplicate_path.changes[1].display_path = duplicate_path.changes[0].display_path.clone();
    invalid_sets.push(duplicate_path);
    let mut no_layer = exact_file_change_set(&project_path);
    no_layer.changes[0].committed = false;
    no_layer.changes[0].staged = false;
    no_layer.changes[0].unstaged = false;
    invalid_sets.push(no_layer);
    let mut invalid_binary = exact_file_change_set(&project_path);
    invalid_binary.changes[0].binary = true;
    invalid_sets.push(invalid_binary);
    let mut exact_untracked = exact_file_change_set(&project_path);
    exact_untracked.changes[0].status = ManagedGitFileStatus::Untracked;
    invalid_sets.push(exact_untracked);
    let mut exact_countless = exact_file_change_set(&project_path);
    exact_countless.changes[0].insertions = None;
    exact_countless.changes[0].deletions = None;
    exact_countless.insertions = None;
    exact_countless.deletions = None;
    invalid_sets.push(exact_countless);
    let mut invalid_aggregate = exact_file_change_set(&project_path);
    invalid_aggregate.insertions = Some(4);
    invalid_sets.push(invalid_aggregate);
    let mut unsorted = exact_file_change_set(&project_path);
    unsorted.changes.swap(0, 1);
    invalid_sets.push(unsorted);

    for (index, change_set) in invalid_sets.into_iter().enumerate() {
        let observation = ManagedProviderObservation {
            run_id: "run-validation".to_owned(),
            session_id: "session-validation".to_owned(),
            external_session_key: "thread-validation".to_owned(),
            provider_turn_id: "turn-validation".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: format!("event-invalid-change-set-{index}"),
            observed_at: ENDED_AT.to_owned(),
            kind: ManagedProviderObservationKind::TurnCompleted {
                changes: ManagedGitChangeSummary::Exact {
                    files: 2,
                    insertions: 3,
                    deletions: 1,
                },
                change_set: Some(Box::new(change_set)),
            },
        };
        assert!(matches!(
            store.append_managed_provider_observation(observation),
            Err(StoreError::InvalidManagedProviderObservation {
                field: "change_set"
            })
        ));
    }
    assert_eq!(
        store.latest_ingest_seq().expect("unchanged cursor"),
        initial_cursor
    );
    assert_eq!(
        store
            .managed_run("run-validation")
            .expect("Run")
            .expect("stored Run")
            .ended_at,
        None
    );

    let binary_id = "33333333333333333333333333333333";
    let observed_id = "44444444444444444444444444444444";
    let binary_set = ManagedGitChangeSet {
        attribution: ManagedGitChangeAttribution::ObservedDuringRun,
        baseline_head: None,
        terminal_head: None,
        repository_identity: test_repository_identity(&project_path),
        files: 2,
        insertions: None,
        deletions: None,
        changes: vec![
            ManagedGitFileChange {
                change_id: binary_id.to_owned(),
                raw_path: b"binary.bin".to_vec(),
                display_path: "binary.bin".to_owned(),
                status: ManagedGitFileStatus::Modified,
                committed: false,
                staged: true,
                unstaged: false,
                binary: true,
                insertions: None,
                deletions: None,
                project_scope: ManagedGitProjectScope::InsideProject,
            },
            ManagedGitFileChange {
                change_id: observed_id.to_owned(),
                raw_path: b"observed.txt".to_vec(),
                display_path: "observed.txt".to_owned(),
                status: ManagedGitFileStatus::Untracked,
                committed: false,
                staged: false,
                unstaged: true,
                binary: false,
                insertions: None,
                deletions: None,
                project_scope: ManagedGitProjectScope::InsideProject,
            },
        ],
    };
    let binary = ManagedProviderObservation {
        run_id: "run-validation".to_owned(),
        session_id: "session-validation".to_owned(),
        external_session_key: "thread-validation".to_owned(),
        provider_turn_id: "turn-validation".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        event_id: "event-binary-change-set".to_owned(),
        observed_at: ENDED_AT.to_owned(),
        kind: ManagedProviderObservationKind::TurnCompleted {
            changes: ManagedGitChangeSummary::Unavailable {
                reason: "binary_line_counts_unavailable".to_owned(),
            },
            change_set: Some(Box::new(binary_set.clone())),
        },
    };
    store
        .append_managed_provider_observation(binary)
        .expect("binary detailed change set");
    let metadata = store
        .managed_git_change_set_metadata("run-validation")
        .expect("observed metadata")
        .expect("stored observed metadata");
    assert_eq!(
        metadata.attribution,
        ManagedGitChangeAttribution::ObservedDuringRun
    );
    assert_eq!(metadata.baseline_head, None);
    assert_eq!(metadata.terminal_head, None);
    assert_eq!(metadata.insertions, None);
    assert_eq!(
        store
            .managed_git_file_change("run-validation", binary_id)
            .expect("binary change")
            .expect("stored binary change"),
        binary_set.changes[0]
    );
    assert_eq!(
        store
            .managed_git_file_change("run-validation", observed_id)
            .expect("countless nonbinary change")
            .expect("stored countless nonbinary change"),
        binary_set.changes[1]
    );
}

fn exact_file_change_set(project_path: &Path) -> ManagedGitChangeSet {
    ManagedGitChangeSet {
        attribution: ManagedGitChangeAttribution::Exact,
        baseline_head: Some("1".repeat(40)),
        terminal_head: Some("2".repeat(40)),
        repository_identity: test_repository_identity(project_path),
        files: 2,
        insertions: Some(3),
        deletions: Some(1),
        changes: vec![
            ManagedGitFileChange {
                change_id: CHANGE_INSIDE_ID.to_owned(),
                raw_path: b"inside-\xff.txt".to_vec(),
                display_path: "inside-�.txt".to_owned(),
                status: ManagedGitFileStatus::Modified,
                committed: false,
                staged: true,
                unstaged: true,
                binary: false,
                insertions: Some(2),
                deletions: Some(1),
                project_scope: ManagedGitProjectScope::InsideProject,
            },
            ManagedGitFileChange {
                change_id: CHANGE_OUTSIDE_ID.to_owned(),
                raw_path: b"outside.txt".to_vec(),
                display_path: "outside.txt".to_owned(),
                status: ManagedGitFileStatus::Added,
                committed: true,
                staged: false,
                unstaged: false,
                binary: false,
                insertions: Some(1),
                deletions: Some(0),
                project_scope: ManagedGitProjectScope::OutsideProject,
            },
        ],
    }
}

fn test_repository_identity(project_path: &Path) -> ManagedGitRepositoryIdentity {
    let project = ProjectDirectoryInspection::inspect(project_path)
        .expect("Project identity")
        .identity;
    let root = project
        .canonical_path
        .to_str()
        .expect("UTF-8 test Project path")
        .as_bytes()
        .to_vec();
    let mut git_directory = root.clone();
    git_directory.extend_from_slice(b"/.git");
    ManagedGitRepositoryIdentity {
        project_filesystem_id: project.filesystem_id.clone(),
        repository_root: root,
        repository_root_filesystem_id: project.filesystem_id.clone(),
        git_directory: git_directory.clone(),
        git_directory_filesystem_id: "unix:11:12".to_owned(),
        common_directory: git_directory,
        common_directory_filesystem_id: "unix:11:12".to_owned(),
    }
}

#[test]
fn provider_owned_outcome_atomically_records_request_and_fact_without_response_attempt() {
    let directory = TestDirectory::new("provider-outcome");
    let (mut store, database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-1",
            "event-run-created",
            "event-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-1",
            "run-1",
            "thread-1",
            &project_path,
        ))
        .expect("managed session");

    let outcome = ManagedProviderOutcome {
        run_id: "run-1".to_owned(),
        session_id: "session-1".to_owned(),
        external_session_key: "thread-1".to_owned(),
        provider_turn_id: "turn-1".to_owned(),
        provider_item_id: "item-1".to_owned(),
        provider_decision_id: "review-1".to_owned(),
        request_id: "request-provider-auto-1".to_owned(),
        permission_mode_version: 1,
        provider_configuration: "readOnly+on-request+auto_review".to_owned(),
        decision: ManagedProviderDecision::Allowed,
        terminal_outcome: ManagedProviderTerminalOutcome::RequestResolved,
        contract_version: "codex-app-server/0.145.0".to_owned(),
        observed_at: STARTED_AT.to_owned(),
        request_event_id: "event-provider-auto-request".to_owned(),
        outcome_event_id: "event-provider-auto-resolved".to_owned(),
    };
    let (request, resolved) = match store
        .commit_managed_provider_outcome(outcome.clone())
        .expect("commit provider-owned outcome")
    {
        ManagedProviderOutcomeCommit::Inserted {
            request_event,
            outcome_event,
        } => (request_event, outcome_event),
        other => panic!("unexpected provider outcome: {other:?}"),
    };
    assert_eq!(request.event_type, "permission.requested");
    assert_eq!(request.stream_seq, 2);
    assert_eq!(request.payload["permission_mode"], "provider_auto");
    assert_eq!(request.payload["response_supported"], false);
    assert!(request.payload.get("provider_request_id").is_none());
    assert_eq!(resolved.event_type, "permission.provider_outcome_resolved");
    assert_eq!(resolved.stream_seq, 3);
    assert_eq!(resolved.payload["request_version"], request.ingest_seq);
    assert_eq!(resolved.payload["provider_decision"], "allowed");
    assert_eq!(resolved.payload["provider_decision_id"], "review-1");
    assert_eq!(resolved.payload["terminal_outcome"], "request_resolved");
    let rendered = serde_json::to_string(&[&request, &resolved]).expect("event JSON");
    for forbidden in [
        "response_attempt_id",
        "delivery_plan_fingerprint",
        "risk_level",
        "scope",
        "sensitive reason",
    ] {
        assert!(!rendered.contains(forbidden));
    }

    assert!(matches!(
        store
            .commit_managed_provider_outcome(outcome.clone())
            .expect("exact duplicate"),
        ManagedProviderOutcomeCommit::Duplicate {
            ref request_event,
            ref outcome_event,
        } if request_event == &request && outcome_event == &resolved
    ));
    let mut conflict = outcome;
    conflict.provider_decision_id = "review-conflict".to_owned();
    assert!(matches!(
        store.commit_managed_provider_outcome(conflict),
        Err(StoreError::ManagedProviderOutcomeConflict { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("stable cursor"), 6);

    drop(store);
    let reopened = Store::open(&database, ENDED_AT).expect("reopen Store");
    let events = reopened
        .run_events_through("run-1", 0, 6, 10)
        .expect("reopened events");
    assert_eq!(
        events
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            "run.created",
            "git.snapshot_recorded",
            "run.start_requested",
            "session.connected",
            "permission.requested",
            "permission.provider_outcome_resolved",
        ]
    );
}

#[test]
fn managed_permission_response_attempt_and_resolution_are_exact_durable_and_idempotent() {
    let directory = TestDirectory::new("permission-response-resolved");
    let (mut store, database, project_path) = trusted_store(&directory);
    let request = open_permission_request(&mut store, &project_path);
    assert_eq!(request.ingest_seq, 5);

    let attempt = permission_attempt(&request, "attempt-1", ManagedPermissionDecision::AllowOnce);
    let submitted = match store
        .submit_managed_permission_response(attempt.clone())
        .expect("submit exact response")
    {
        ManagedPermissionResponseAttemptOutcome::Submitted { event } => event,
        other => panic!("unexpected response attempt outcome: {other:?}"),
    };
    assert_eq!(submitted.event_type, "permission.response_submitted");
    assert_eq!(submitted.stream_seq, 3);
    assert_eq!(submitted.payload["request_version"], 5);
    assert_eq!(submitted.payload["decision"], "allow_once");
    assert_eq!(submitted.payload["response_attempt_id"], "attempt-1");
    assert_eq!(
        submitted.payload["delivery_plan_fingerprint"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert!(submitted.evidence_ids.is_empty());

    assert!(matches!(
        store
            .submit_managed_permission_response(attempt.clone())
            .expect("duplicate response attempt"),
        ManagedPermissionResponseAttemptOutcome::Duplicate {
            ref event,
            terminal_event: None,
        } if event == &submitted
    ));

    let result = permission_result(
        &attempt,
        "event-permission-resolved",
        ManagedPermissionResponseResultKind::Resolved(
            ManagedPermissionResolutionKind::AcceptedCompleted,
        ),
    );
    let resolved = match store
        .finish_managed_permission_response(result.clone())
        .expect("resolve exact response")
    {
        AppendEventOutcome::Inserted(event) => event,
        other => panic!("unexpected response result: {other:?}"),
    };
    assert_eq!(resolved.event_type, "permission.resolved");
    assert_eq!(resolved.stream_seq, 4);
    assert_eq!(
        resolved.payload["causal_item_outcome"],
        "accepted_completed"
    );
    assert_eq!(resolved.payload["response_attempt_id"], "attempt-1");
    assert!(resolved.evidence_ids.is_empty());
    let rendered = serde_json::to_string(&[&submitted, &resolved]).expect("event JSON");
    for raw in [
        "sensitive reason",
        "secret command",
        "/private/secret",
        "Respond with the requested result.",
    ] {
        assert!(!rendered.contains(raw));
    }

    assert!(matches!(
        store
            .finish_managed_permission_response(result)
            .expect("duplicate response result"),
        AppendEventOutcome::Duplicate(ref event) if event == &resolved
    ));
    assert!(matches!(
        store
            .submit_managed_permission_response(attempt)
            .expect("duplicate attempt after resolution"),
        ManagedPermissionResponseAttemptOutcome::Duplicate {
            ref event,
            terminal_event: Some(ref terminal),
        } if event == &submitted && terminal.as_ref() == &resolved
    ));
    assert_eq!(store.latest_ingest_seq().expect("stable event cursor"), 7);

    drop(store);
    let reopened = Store::open(&database, ENDED_AT).expect("reopen Store");
    let events = reopened
        .run_events_through("run-1", 0, 7, 10)
        .expect("reopened events");
    assert_eq!(
        events
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            "run.created",
            "git.snapshot_recorded",
            "run.start_requested",
            "session.connected",
            "permission.requested",
            "permission.response_submitted",
            "permission.resolved",
        ]
    );
}

#[test]
fn managed_permission_response_rejects_stale_wrong_and_second_attempts_without_mutation() {
    let directory = TestDirectory::new("permission-response-conflicts");
    let (mut store, database, project_path) = trusted_store(&directory);
    let request = open_permission_request(&mut store, &project_path);
    let attempt = permission_attempt(&request, "attempt-1", ManagedPermissionDecision::Deny);

    let mut wrong = attempt.clone();
    wrong.provider_item_id = "wrong-item".to_owned();
    assert!(matches!(
        store.submit_managed_permission_response(wrong),
        Err(StoreError::ManagedPermissionRequestMismatch { .. })
    ));
    let mut stale = attempt.clone();
    stale.request_version = 3;
    assert!(matches!(
        store.submit_managed_permission_response(stale),
        Err(StoreError::ManagedPermissionRequestMismatch { .. })
    ));
    let mut missing = attempt.clone();
    missing.request_version = 99;
    assert!(matches!(
        store.submit_managed_permission_response(missing),
        Err(StoreError::ManagedPermissionRequestStale { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("unchanged cursor"), 5);

    store
        .submit_managed_permission_response(attempt.clone())
        .expect("first response attempt");
    drop(store);
    let mut store = Store::open(&database, ENDED_AT).expect("reopen pending attempt");
    assert!(matches!(
        store
            .submit_managed_permission_response(attempt.clone())
            .expect("pending duplicate after restart"),
        ManagedPermissionResponseAttemptOutcome::Duplicate {
            terminal_event: None,
            ..
        }
    ));
    let mut second = attempt.clone();
    second.response_attempt_id = "attempt-2".to_owned();
    second.submitted_event_id = "event-response-submitted-2".to_owned();
    assert!(matches!(
        store.submit_managed_permission_response(second),
        Err(StoreError::ManagedPermissionResponseConflict { .. })
    ));

    let wrong_outcome = permission_result(
        &attempt,
        "event-permission-resolved",
        ManagedPermissionResponseResultKind::Resolved(
            ManagedPermissionResolutionKind::AcceptedCompleted,
        ),
    );
    assert!(matches!(
        store.finish_managed_permission_response(wrong_outcome),
        Err(StoreError::InvalidManagedPermissionResponse { field: "kind" })
    ));
    let unknown = permission_result(
        &attempt,
        "event-permission-unknown",
        ManagedPermissionResponseResultKind::DeliveryUnknown(
            ManagedPermissionDeliveryUnknownReason::ProviderOutcomeAmbiguous,
        ),
    );
    store
        .finish_managed_permission_response(unknown)
        .expect("record delivery unknown");
    let events = store
        .run_events_through("run-1", 0, 7, 10)
        .expect("delivery unknown events");
    assert_eq!(
        events.events[6].payload["delivery_plan_fingerprint"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    let resolved_after_unknown = permission_result(
        &attempt,
        "event-permission-resolved-late",
        ManagedPermissionResponseResultKind::Resolved(ManagedPermissionResolutionKind::Declined),
    );
    assert!(matches!(
        store.finish_managed_permission_response(resolved_after_unknown),
        Err(StoreError::ManagedPermissionResponseConflict { .. })
    ));
    assert_eq!(
        store.latest_ingest_seq().expect("one attempt and outcome"),
        7
    );
}

#[test]
fn managed_permission_response_lookup_stays_exact_after_large_unrelated_history() {
    let directory = TestDirectory::new("permission-response-history");
    let (mut store, _database, project_path) = trusted_store(&directory);
    store
        .create_managed_run_intent(run_intent(
            "run-1",
            "event-run-created",
            "event-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-1",
            "run-1",
            "thread-1",
            &project_path,
        ))
        .expect("managed session");

    for index in 0_u64..128 {
        let request = append_permission_request(&mut store, index);
        let attempt = indexed_permission_attempt(&request, index);
        store
            .submit_managed_permission_response(attempt.clone())
            .expect("historical response attempt");
        store
            .finish_managed_permission_response(permission_result(
                &attempt,
                &format!("event-permission-unknown-{index}"),
                ManagedPermissionResponseResultKind::DeliveryUnknown(
                    ManagedPermissionDeliveryUnknownReason::ProviderOutcomeAmbiguous,
                ),
            ))
            .expect("historical delivery unknown");
    }

    let current_request = append_permission_request(&mut store, 128);
    let current_attempt = indexed_permission_attempt(&current_request, 128);
    let current = store
        .submit_managed_permission_response(current_attempt.clone())
        .expect("current response attempt after history");
    assert!(matches!(
        current,
        ManagedPermissionResponseAttemptOutcome::Submitted { ref event }
            if event.payload["request_id"] == "request-permission-128"
    ));
    assert!(matches!(
        store
            .submit_managed_permission_response(current_attempt.clone())
            .expect("exact current duplicate"),
        ManagedPermissionResponseAttemptOutcome::Duplicate {
            terminal_event: None,
            ..
        }
    ));
    append_permission_request(&mut store, 129);
    assert!(matches!(
        store.finish_managed_permission_response(permission_result(
            &current_attempt,
            "event-late-current-outcome",
            ManagedPermissionResponseResultKind::DeliveryUnknown(
                ManagedPermissionDeliveryUnknownReason::ProviderOutcomeAmbiguous,
            ),
        )),
        Err(StoreError::ManagedPermissionRequestStale { .. })
    ));
    assert_eq!(
        store.latest_ingest_seq().expect("large history cursor"),
        4 + (128 * 3) + 3
    );
}

#[test]
fn managed_permission_response_requires_submit_and_a_live_current_request() {
    let directory = TestDirectory::new("permission-response-live");
    let (mut store, _database, project_path) = trusted_store(&directory);
    let request = open_permission_request(&mut store, &project_path);
    let attempt = permission_attempt(&request, "attempt-1", ManagedPermissionDecision::Deny);
    let result = permission_result(
        &attempt,
        "event-permission-unknown",
        ManagedPermissionResponseResultKind::DeliveryUnknown(
            ManagedPermissionDeliveryUnknownReason::CoreRestartedAfterSubmit,
        ),
    );
    assert!(matches!(
        store.finish_managed_permission_response(result),
        Err(StoreError::ManagedPermissionResponseNotSubmitted { .. })
    ));

    store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-1".to_owned(),
            session_id: "session-1".to_owned(),
            external_session_key: "thread-1".to_owned(),
            provider_turn_id: "turn-1".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: "event-terminal".to_owned(),
            observed_at: ENDED_AT.to_owned(),
            kind: ManagedProviderObservationKind::TurnCompleted {
                changes: ManagedGitChangeSummary::Unavailable {
                    reason: "git_terminal_observation_unavailable".to_owned(),
                },
                change_set: None,
            },
        })
        .expect("terminal observation");
    assert!(matches!(
        store.submit_managed_permission_response(attempt),
        Err(StoreError::ManagedPermissionRequestStale { .. }
            | StoreError::ManagedSessionNotLive { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("terminal cursor"), 6);

    let newer_directory = TestDirectory::new("permission-response-newer-request");
    let (mut newer_store, _database, newer_project_path) = trusted_store(&newer_directory);
    let first_request = open_permission_request(&mut newer_store, &newer_project_path);
    let first_attempt = permission_attempt(
        &first_request,
        "attempt-old",
        ManagedPermissionDecision::Deny,
    );
    newer_store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-1".to_owned(),
            session_id: "session-1".to_owned(),
            external_session_key: "thread-1".to_owned(),
            provider_turn_id: "turn-1".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: "event-permission-requested-new".to_owned(),
            observed_at: "2026-07-24T10:00:02Z".to_owned(),
            kind: ManagedProviderObservationKind::PermissionRequested {
                request_id: "request-permission-new".to_owned(),
                provider_request_id: 18,
                provider_item_id: "permission-item-new".to_owned(),
                provider_started_at_ms: 43,
            },
        })
        .expect("newer permission request");
    assert!(matches!(
        newer_store.submit_managed_permission_response(first_attempt),
        Err(StoreError::ManagedPermissionRequestStale { .. })
    ));
    assert_eq!(
        newer_store.latest_ingest_seq().expect("new request cursor"),
        6
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
    assert_eq!(store.latest_ingest_seq().expect("session depth cursor"), 3);

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
    assert_eq!(store.latest_ingest_seq().expect("archived cursor"), 3);
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

    let mut combined_identity_conflict =
        session_connection("session-1", "run-2", "thread-shared", &project_path);
    combined_identity_conflict.session_fingerprint = "different-fingerprint".to_owned();
    assert!(matches!(
        store.connect_initial_managed_session(combined_identity_conflict),
        Err(StoreError::ExternalSessionAlreadyClaimed {
            ref claimed_run_id,
            ref claimed_session_id,
            ..
        }) if claimed_run_id == "run-1" && claimed_session_id == "session-1"
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
            7
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
        7
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
    let initial_connection = session_connection(
        "session-terminal",
        "run-terminal",
        "thread-terminal",
        &project_path,
    );
    store
        .connect_initial_managed_session(initial_connection.clone())
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
    assert_eq!(event.ingest_seq, 5);
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
    assert_eq!(store.latest_ingest_seq().expect("duplicate cursor"), 5);
    assert!(matches!(
        store
            .connect_initial_managed_session(initial_connection.clone())
            .expect("terminal session-connect replay"),
        InitialManagedSessionOutcome::Duplicate { event, .. } if event.ingest_seq == 4
    ));

    drop(store);
    let mut reopened = Store::open(&database, CREATED_AT).expect("reopen Store");
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
            .run_events_through("run-terminal", 0, 5, 10)
            .expect("terminal event page")
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            "run.created",
            "git.snapshot_recorded",
            "run.start_requested",
            "session.connected",
            "run.completed"
        ]
    );
    assert!(matches!(
        reopened
            .connect_initial_managed_session(initial_connection)
            .expect("reopened terminal session-connect replay"),
        InitialManagedSessionOutcome::Duplicate { event, .. } if event.ingest_seq == 4
    ));
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
    assert_eq!(
        event.payload["changes"],
        json!({
            "availability": "unavailable",
            "reason": "git_runtime_baseline_unavailable"
        })
    );
    assert_eq!(event.payload.len(), 4);
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
    assert_eq!(
        store
            .complete_live_managed_sessions(2)
            .expect("complete live session snapshot")
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["session-a", "session-z"]
    );
    assert!(matches!(
        store.complete_live_managed_sessions(1),
        Err(StoreError::LiveManagedSessionSourceLimitExceeded { max: 1 })
    ));
    for limit in [0, MAX_LIVE_MANAGED_SESSIONS + 1] {
        assert!(matches!(
            store.live_managed_sessions(limit),
            Err(StoreError::InvalidLiveManagedSessionLimit { .. })
        ));
        assert!(matches!(
            store.complete_live_managed_sessions(limit),
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
    assert_eq!(
        store
            .complete_live_managed_sessions(1)
            .expect("complete remaining session")
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
    assert_eq!(store.latest_ingest_seq().expect("gap cursor"), 8);

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
    assert_eq!(store.latest_ingest_seq().expect("invalid cursor"), 8);
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
        assert_eq!(events[1].protocol_version, EventProtocolVersion::V1_2);
        assert_eq!(events[1].payload["provider_turn_id"], turn_id);
        assert_eq!(events[1].payload["reconciled_after_gap"], true);
        assert_eq!(
            events[1].payload["changes"],
            json!({
                "availability": "unavailable",
                "reason": "git_runtime_baseline_unavailable_after_restart"
            })
        );
        assert_eq!(
            store
                .run_snapshot(&run_id)
                .expect("reconciled snapshot")
                .expect("reconciled snapshot")
                .snapshot["changes"],
            json!({
                "availability": "unavailable",
                "reason": "git_runtime_baseline_unavailable_after_restart"
            })
        );
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

fn open_permission_request(store: &mut Store, project_path: &Path) -> flit_protocol::EventEnvelope {
    store
        .create_managed_run_intent(run_intent(
            "run-1",
            "event-run-created",
            "event-start-requested",
        ))
        .expect("managed Run");
    store
        .connect_initial_managed_session(session_connection(
            "session-1",
            "run-1",
            "thread-1",
            project_path,
        ))
        .expect("managed session");
    match store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-1".to_owned(),
            session_id: "session-1".to_owned(),
            external_session_key: "thread-1".to_owned(),
            provider_turn_id: "turn-1".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: "event-permission-requested".to_owned(),
            observed_at: STARTED_AT.to_owned(),
            kind: ManagedProviderObservationKind::PermissionRequested {
                request_id: "request-permission".to_owned(),
                provider_request_id: 17,
                provider_item_id: "permission-item".to_owned(),
                provider_started_at_ms: 42,
            },
        })
        .expect("permission request")
    {
        AppendEventOutcome::Inserted(event) => event,
        other => panic!("unexpected permission request: {other:?}"),
    }
}

fn permission_attempt(
    request: &flit_protocol::EventEnvelope,
    response_attempt_id: &str,
    decision: ManagedPermissionDecision,
) -> ManagedPermissionResponseAttempt {
    ManagedPermissionResponseAttempt {
        run_id: "run-1".to_owned(),
        session_id: "session-1".to_owned(),
        external_session_key: "thread-1".to_owned(),
        provider_turn_id: "turn-1".to_owned(),
        provider_item_id: "permission-item".to_owned(),
        provider_request_id: 17,
        request_id: "request-permission".to_owned(),
        request_version: request.ingest_seq,
        request_event_id: request.event_id.clone(),
        response_attempt_id: response_attempt_id.to_owned(),
        decision,
        delivery_plan_fingerprint:
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        submitted_at: "2026-07-24T10:00:02Z".to_owned(),
        submitted_event_id: format!("event-response-submitted-{response_attempt_id}"),
    }
}

fn append_permission_request(store: &mut Store, index: u64) -> flit_protocol::EventEnvelope {
    match store
        .append_managed_provider_observation(ManagedProviderObservation {
            run_id: "run-1".to_owned(),
            session_id: "session-1".to_owned(),
            external_session_key: "thread-1".to_owned(),
            provider_turn_id: "turn-1".to_owned(),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            event_id: format!("event-permission-requested-{index}"),
            observed_at: STARTED_AT.to_owned(),
            kind: ManagedProviderObservationKind::PermissionRequested {
                request_id: format!("request-permission-{index}"),
                provider_request_id: index,
                provider_item_id: format!("permission-item-{index}"),
                provider_started_at_ms: index,
            },
        })
        .expect("indexed permission request")
    {
        AppendEventOutcome::Inserted(event) => event,
        other => panic!("unexpected indexed request: {other:?}"),
    }
}

fn indexed_permission_attempt(
    request: &flit_protocol::EventEnvelope,
    index: u64,
) -> ManagedPermissionResponseAttempt {
    ManagedPermissionResponseAttempt {
        run_id: "run-1".to_owned(),
        session_id: "session-1".to_owned(),
        external_session_key: "thread-1".to_owned(),
        provider_turn_id: "turn-1".to_owned(),
        provider_item_id: format!("permission-item-{index}"),
        provider_request_id: index,
        request_id: format!("request-permission-{index}"),
        request_version: request.ingest_seq,
        request_event_id: request.event_id.clone(),
        response_attempt_id: format!("attempt-{index}"),
        decision: ManagedPermissionDecision::Deny,
        delivery_plan_fingerprint:
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        submitted_at: "2026-07-24T10:00:02Z".to_owned(),
        submitted_event_id: format!("event-response-submitted-{index}"),
    }
}

fn permission_result(
    attempt: &ManagedPermissionResponseAttempt,
    outcome_event_id: &str,
    kind: ManagedPermissionResponseResultKind,
) -> ManagedPermissionResponseResult {
    ManagedPermissionResponseResult {
        run_id: attempt.run_id.clone(),
        session_id: attempt.session_id.clone(),
        external_session_key: attempt.external_session_key.clone(),
        provider_turn_id: attempt.provider_turn_id.clone(),
        provider_item_id: attempt.provider_item_id.clone(),
        provider_request_id: attempt.provider_request_id,
        request_id: attempt.request_id.clone(),
        request_version: attempt.request_version,
        response_attempt_id: attempt.response_attempt_id.clone(),
        decision: attempt.decision,
        delivery_plan_fingerprint: attempt.delivery_plan_fingerprint.clone(),
        contract_version: attempt.contract_version.clone(),
        finished_at: "2026-07-24T10:00:03Z".to_owned(),
        outcome_event_id: outcome_event_id.to_owned(),
        kind,
    }
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
        git_baseline: GitBaselinePayload::Unavailable {
            project_id: "project-1".to_owned(),
            reason: GitBaselineUnavailableReason::RunnerUnavailable,
        },
        git_baseline_observed_at: CREATED_AT.to_owned(),
        created_at: CREATED_AT.to_owned(),
        run_created_event_id: created_event_id.to_owned(),
        git_baseline_event_id: format!("{created_event_id}-git-baseline"),
        start_requested_event_id: requested_event_id.to_owned(),
    }
}

fn run_intent_with_clean_baseline(
    run_id: &str,
    created_event_id: &str,
    requested_event_id: &str,
) -> ManagedRunIntent {
    let mut intent = run_intent(run_id, created_event_id, requested_event_id);
    intent.git_baseline = GitBaselinePayload::Available {
        project_id: "project-1".to_owned(),
        head: flit_protocol::GitHead::Available {
            oid: "1".repeat(40),
        },
        dirty: flit_protocol::GitDirtySummary {
            staged: 0,
            unstaged: 0,
            untracked: 0,
            entries: 0,
        },
    };
    intent
}

fn start_failure(run_id: &str, event_id: &str) -> ManagedRunStartFailure {
    ManagedRunStartFailure {
        run_id: run_id.to_owned(),
        reason: "provider_start_failed".to_owned(),
        contract_version: "codex-app-server/0.145.0".to_owned(),
        failed_at: ENDED_AT.to_owned(),
        failed_event_id: event_id.to_owned(),
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
        protocol_version: EventProtocolVersion::V1_1,
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
