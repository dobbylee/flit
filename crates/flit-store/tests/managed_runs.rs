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
    AppendEventOutcome, InitialManagedSessionConnection, InitialManagedSessionOutcome,
    MAX_LIVE_MANAGED_SESSIONS, ManagedPermissionDecision, ManagedPermissionDeliveryUnknownReason,
    ManagedPermissionResolutionKind, ManagedPermissionResponseAttempt,
    ManagedPermissionResponseAttemptOutcome, ManagedPermissionResponseResult,
    ManagedPermissionResponseResultKind, ManagedProviderDecision, ManagedProviderObservation,
    ManagedProviderObservationKind, ManagedProviderOutcome, ManagedProviderOutcomeCommit,
    ManagedProviderTerminalOutcome, ManagedReconciliationState, ManagedRunIntent,
    ManagedRunIntentOutcome, ManagedRunStartFailure, ManagedRunStartFailureOutcome,
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
    assert_eq!(failed_event.ingest_seq, 3);
    assert_eq!(failed_event.stream_seq, 3);
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
        ManagedRunStartFailureOutcome::Duplicate { event, .. } if event.ingest_seq == 3
    ));
    assert_eq!(store.latest_ingest_seq().expect("duplicate cursor"), 3);

    let mut mismatch = start_failure("run-failed-start", "event-other-terminal");
    mismatch.reason = "different_failure".to_owned();
    assert!(matches!(
        store.fail_managed_run_start(mismatch),
        Err(StoreError::ManagedRunTerminalConflict { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("conflict cursor"), 3);
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
        ["run.created", "run.start_requested"]
    );
    assert_eq!(
        created_events
            .iter()
            .map(|event| event.ingest_seq)
            .collect::<Vec<_>>(),
        [1, 2]
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
        kind: ManagedProviderObservationKind::TurnCompleted,
    };
    let terminal_event = store
        .append_managed_provider_observation(terminal.clone())
        .expect("terminal observation");
    assert!(matches!(
        terminal_event,
        AppendEventOutcome::Inserted(ref terminal) if terminal.stream_seq == 4
    ));
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
    assert_eq!(store.latest_ingest_seq().expect("stable cursor"), 5);

    drop(store);
    let reopened = Store::open(&database, ENDED_AT).expect("reopen Store");
    let events = reopened
        .run_events_through("run-1", 0, 5, 10)
        .expect("reopened events");
    assert_eq!(
        events
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            "run.created",
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
    assert_eq!(request.ingest_seq, 4);

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
    assert_eq!(submitted.payload["request_version"], 4);
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
    assert_eq!(store.latest_ingest_seq().expect("stable event cursor"), 6);

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
    assert_eq!(store.latest_ingest_seq().expect("unchanged cursor"), 4);

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
        .run_events_through("run-1", 0, 6, 10)
        .expect("delivery unknown events");
    assert_eq!(
        events.events[5].payload["delivery_plan_fingerprint"],
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
        6
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
        3 + (128 * 3) + 3
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
            kind: ManagedProviderObservationKind::TurnCompleted,
        })
        .expect("terminal observation");
    assert!(matches!(
        store.submit_managed_permission_response(attempt),
        Err(StoreError::ManagedPermissionRequestStale { .. }
            | StoreError::ManagedSessionNotLive { .. })
    ));
    assert_eq!(store.latest_ingest_seq().expect("terminal cursor"), 5);

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
        5
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
    assert!(matches!(
        store
            .connect_initial_managed_session(initial_connection.clone())
            .expect("terminal session-connect replay"),
        InitialManagedSessionOutcome::Duplicate { event, .. } if event.ingest_seq == 3
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
    assert!(matches!(
        reopened
            .connect_initial_managed_session(initial_connection)
            .expect("reopened terminal session-connect replay"),
        InitialManagedSessionOutcome::Duplicate { event, .. } if event.ingest_seq == 3
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
        baseline_head: None,
        created_at: CREATED_AT.to_owned(),
        run_created_event_id: created_event_id.to_owned(),
        start_requested_event_id: requested_event_id.to_owned(),
    }
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
