use std::{collections::BTreeSet, fs, path::PathBuf};

use flit_protocol::{
    CapabilityStatus, CommandError, DashboardReadRequest, DashboardReadResponse,
    EVENT_PROTOCOL_VERSION, EventEnvelope, EventProtocolVersion, GitBaselinePayload,
    GitObservationRequest, GitObservationResponse, MAX_JSON_SAFE_INTEGER, ManagedRunObserveRequest,
    ManagedRunObserveResponse, ManagedRunOpenInProviderRequest, ManagedRunPermissionRespondRequest,
    ManagedRunPermissionRespondResponse, ManagedRunStartRequest, ManagedRunStartResponse,
    PROTOCOL_VERSION, ProjectInspectionRequest, ProjectInspectionResponse,
    ProjectRegistrationRequest, ProjectRegistrationResponse, ProjectTrustRequest,
    ProjectTrustResponse, ProjectsListRequest, ProjectsListResponse, ProviderCompatibility,
    ProviderDiagnosticsRequest, ProviderDiagnosticsResponse, ProviderExecutionAfterQuit,
    ProviderUnavailableReason, QuitImpactReason, QuitImpactRequest, QuitImpactResponse,
    RunDetailReadRequest, RunDetailReadResponse, SystemHealthRequest, SystemHealthResponse,
    event_schema_id, event_schema_relative_path, generated_swift_command_contract,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

fn repository_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn assert_fixture_round_trip<T>(relative: &str)
where
    T: DeserializeOwned + Serialize,
{
    let fixture =
        fs::read_to_string(repository_path(relative)).expect("fixture should be readable");
    let expected: serde_json::Value =
        serde_json::from_str(&fixture).expect("fixture should contain valid JSON");
    let decoded: T =
        serde_json::from_value(expected.clone()).expect("fixture should match Rust type");
    let actual = serde_json::to_value(decoded).expect("Rust type should serialize");
    assert_eq!(actual, expected);
}

#[derive(Clone, Debug, Deserialize)]
struct CommandCompatibilityManifest {
    current: CommandFixtureReference,
    previous_minor: Option<CommandFixtureReference>,
}

#[derive(Clone, Debug, Deserialize)]
struct CommandFixtureReference {
    version: String,
    directory: String,
}

fn read_command_compatibility_manifest() -> CommandCompatibilityManifest {
    serde_json::from_str(
        &fs::read_to_string(repository_path(
            "fixtures/protocol/commands/compatibility.json",
        ))
        .expect("command compatibility manifest should be readable"),
    )
    .expect("command compatibility manifest should contain valid JSON")
}

fn command_fixture(reference: &CommandFixtureReference, name: &str) -> String {
    format!("{}/{name}", reference.directory)
}

#[test]
fn checked_in_event_schema_matches_the_rust_source() {
    let manifest = read_compatibility_manifest();
    assert_eq!(manifest.current.schema, event_schema_relative_path());
    let generated = fs::read_to_string(repository_path(&manifest.current.schema))
        .expect("generated event schema should be checked in");

    assert_eq!(generated, flit_protocol::generated_event_schema());
}

#[test]
fn current_system_health_fixtures_round_trip() {
    let manifest = read_command_compatibility_manifest();
    assert_fixture_round_trip::<SystemHealthRequest>(&command_fixture(
        &manifest.current,
        "system_health.request.json",
    ));
    assert_fixture_round_trip::<SystemHealthResponse>(&command_fixture(
        &manifest.current,
        "system_health.response.json",
    ));
    assert_fixture_round_trip::<SystemHealthResponse>(&command_fixture(
        &manifest.current,
        "system_health.providers_ready.response.json",
    ));
    assert_fixture_round_trip::<SystemHealthResponse>(&command_fixture(
        &manifest.current,
        "system_health.providers_unavailable.response.json",
    ));
    assert_fixture_round_trip::<CommandError>(&command_fixture(
        &manifest.current,
        "protocol_mismatch.error.json",
    ));
}

#[test]
fn fixtures_are_bound_to_the_current_protocol_version() {
    let manifest = read_command_compatibility_manifest();
    let request: SystemHealthRequest = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            &manifest.current,
            "system_health.request.json",
        )))
        .expect("request fixture should be readable"),
    )
    .expect("request fixture should match Rust type");
    let response: SystemHealthResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            &manifest.current,
            "system_health.response.json",
        )))
        .expect("response fixture should be readable"),
    )
    .expect("response fixture should match Rust type");

    assert_eq!(manifest.current.version, PROTOCOL_VERSION);
    assert_eq!(request.client_protocol_version, PROTOCOL_VERSION);
    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
}

#[test]
fn current_project_command_fixtures_round_trip_every_shape() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<ProjectInspectionRequest>(&command_fixture(
        current,
        "project_inspect.request.json",
    ));
    assert_fixture_round_trip::<ProjectInspectionResponse>(&command_fixture(
        current,
        "project_inspect.response.json",
    ));
    assert_fixture_round_trip::<ProjectRegistrationRequest>(&command_fixture(
        current,
        "project_register.request.json",
    ));
    for name in [
        "project_register.registered.response.json",
        "project_register.duplicate_canonical_path.response.json",
        "project_register.duplicate_filesystem_identity.response.json",
    ] {
        assert_fixture_round_trip::<ProjectRegistrationResponse>(&command_fixture(current, name));
    }
    assert_fixture_round_trip::<ProjectTrustRequest>(&command_fixture(
        current,
        "project_trust.request.json",
    ));
    for name in [
        "project_trust.trusted.response.json",
        "project_trust.already_trusted.response.json",
    ] {
        assert_fixture_round_trip::<ProjectTrustResponse>(&command_fixture(current, name));
    }
    assert_fixture_round_trip::<ProjectsListRequest>(&command_fixture(
        current,
        "projects_list.request.json",
    ));
    assert_fixture_round_trip::<ProjectsListResponse>(&command_fixture(
        current,
        "projects_list.response.json",
    ));
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        current,
        "project_errors.json",
    ));
    let errors: Vec<CommandError> = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "project_errors.json",
        )))
        .expect("Project error fixture should be readable"),
    )
    .expect("Project error fixture should match Rust types");
    for error in errors {
        assert_eq!(error, CommandError::for_code(error.code));
    }
}

#[test]
fn current_git_observation_fixtures_are_exhaustive_and_fail_closed() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<GitObservationRequest>(&command_fixture(
        current,
        "git_observe.request.json",
    ));
    for name in [
        "git_observe.not_repository.response.json",
        "git_observe.bare_repository.response.json",
        "git_observe.unborn.response.json",
        "git_observe.repository.response.json",
        "git_observe.runner_unavailable.response.json",
        "git_observe.git_unavailable.response.json",
        "git_observe.project_changed.response.json",
        "git_observe.process_unavailable.response.json",
        "git_observe.malformed_output.response.json",
    ] {
        assert_fixture_round_trip::<GitObservationResponse>(&command_fixture(current, name));
    }

    let mut mixed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "git_observe.unborn.response.json",
        )))
        .expect("unborn Git fixture should be readable"),
    )
    .expect("unborn Git fixture should be JSON");
    mixed["head"]["oid"] = serde_json::json!("invented");
    assert!(serde_json::from_value::<GitObservationResponse>(mixed).is_err());

    let mut wrong_variant: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "git_observe.runner_unavailable.response.json",
        )))
        .expect("unavailable Git fixture should be readable"),
    )
    .expect("unavailable Git fixture should be JSON");
    wrong_variant["dirty"] = serde_json::json!({
        "staged": 0,
        "unstaged": 0,
        "untracked": 0,
        "entries": 0
    });
    assert!(serde_json::from_value::<GitObservationResponse>(wrong_variant).is_err());
}

#[test]
fn current_dashboard_read_fixtures_round_trip_every_delivery() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    for name in [
        "dashboard_read.initial.request.json",
        "dashboard_read.delta.request.json",
    ] {
        assert_fixture_round_trip::<DashboardReadRequest>(&command_fixture(current, name));
    }
    for name in [
        "dashboard_read.initial.response.json",
        "dashboard_read.unavailable_changes.response.json",
        "dashboard_read.delta.response.json",
        "dashboard_read.resync.response.json",
    ] {
        assert_fixture_round_trip::<DashboardReadResponse>(&command_fixture(current, name));
    }
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        current,
        "dashboard_read_errors.json",
    ));

    let mut missing_identity: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "dashboard_read.initial.response.json",
        )))
        .expect("Dashboard snapshot fixture should be readable"),
    )
    .expect("Dashboard snapshot fixture should be JSON");
    missing_identity
        .as_object_mut()
        .expect("Dashboard snapshot should be an object")
        .remove("core_instance_id");
    assert!(serde_json::from_value::<DashboardReadResponse>(missing_identity).is_err());

    let mut unknown_reason: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "dashboard_read.resync.response.json",
        )))
        .expect("Dashboard resync fixture should be readable"),
    )
    .expect("Dashboard resync fixture should be JSON");
    unknown_reason["reason"] = serde_json::json!("silently_continue");
    assert!(serde_json::from_value::<DashboardReadResponse>(unknown_reason).is_err());

    for required in ["attention_open_count", "changes"] {
        let mut missing_projection: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(repository_path(&command_fixture(
                current,
                "dashboard_read.initial.response.json",
            )))
            .expect("Dashboard projection fixture should be readable"),
        )
        .expect("Dashboard projection fixture should be JSON");
        missing_projection["runs"][0]
            .as_object_mut()
            .expect("Dashboard Run should be an object")
            .remove(required);
        assert!(
            serde_json::from_value::<DashboardReadResponse>(missing_projection).is_err(),
            "Dashboard Run must require {required}"
        );
    }

    let unavailable: DashboardReadResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "dashboard_read.unavailable_changes.response.json",
        )))
        .expect("unavailable changes fixture should be readable"),
    )
    .expect("unavailable changes fixture should match Rust types");
    assert!(matches!(
        unavailable,
        DashboardReadResponse::Snapshot { runs, .. }
            if matches!(
                &runs[0].changes,
                flit_protocol::DashboardChangeSummary::Unavailable { reason }
                    if reason == "git_observation_not_configured"
            )
    ));

    let delta: DashboardReadResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "dashboard_read.delta.response.json",
        )))
        .expect("projection-bearing delta fixture should be readable"),
    )
    .expect("projection-bearing delta should match Rust types");
    assert!(matches!(
        delta,
        DashboardReadResponse::Delta {
            next_cursor,
            events,
            runs,
            ..
        } if events.len() == 1
            && runs.len() == 1
            && runs[0].run_id == events[0].run_id
            && runs[0].version == next_cursor
    ));

    let mut missing_delta_runs: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "dashboard_read.delta.response.json",
        )))
        .expect("projection-bearing delta fixture should be readable"),
    )
    .expect("projection-bearing delta fixture should be JSON");
    missing_delta_runs
        .as_object_mut()
        .expect("Dashboard delta should be an object")
        .remove("runs");
    assert!(serde_json::from_value::<DashboardReadResponse>(missing_delta_runs).is_err());

    for invalid_changes in [
        serde_json::json!({
            "availability": "estimated",
            "reason": "not_exact"
        }),
        serde_json::json!({
            "availability": "available",
            "insertions": 1,
            "deletions": 1
        }),
        serde_json::json!({
            "availability": "unavailable",
            "reason": "git_observation_not_configured",
            "files": 0
        }),
    ] {
        let mut invalid: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(repository_path(&command_fixture(
                current,
                "dashboard_read.initial.response.json",
            )))
            .expect("Dashboard snapshot fixture should be readable"),
        )
        .expect("Dashboard snapshot fixture should be JSON");
        invalid["runs"][0]["changes"] = invalid_changes;
        assert!(
            serde_json::from_value::<DashboardReadResponse>(invalid).is_err(),
            "Dashboard changes availability must fail closed"
        );
    }
}

#[test]
fn current_run_detail_and_provider_open_fixtures_are_exact_and_path_free() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<RunDetailReadRequest>(&command_fixture(
        current,
        "run_detail_read.request.json",
    ));
    assert_fixture_round_trip::<RunDetailReadResponse>(&command_fixture(
        current,
        "run_detail_read.response.json",
    ));
    assert_fixture_round_trip::<ManagedRunOpenInProviderRequest>(&command_fixture(
        current,
        "managed_run_open_in_provider.request.json",
    ));
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        current,
        "run_detail_and_provider_open_errors.json",
    ));
    let response: RunDetailReadResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "run_detail_read.response.json",
        )))
        .expect("Run detail response should be readable"),
    )
    .expect("Run detail response should match Rust types");
    assert_eq!(response.history_status, CapabilityStatus::Unsupported);
    assert_eq!(
        response.open_in_provider_status,
        CapabilityStatus::Unsupported
    );
    assert_eq!(response.events.len(), 2);
    let rendered = serde_json::to_string(&response).expect("Run detail should serialize");
    for forbidden in [
        "\"payload\"",
        "\"source\"",
        "canonical_path",
        "executable_path",
        "provider_thread_id",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "Run detail must not expose {forbidden}"
        );
    }
}

#[test]
fn current_provider_diagnostics_fixtures_are_exhaustive_and_path_free() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<ProviderDiagnosticsRequest>(&command_fixture(
        current,
        "provider_diagnostics.request.json",
    ));
    for name in [
        "provider_diagnostics.supported.response.json",
        "provider_diagnostics.unknown.response.json",
        "provider_diagnostics.unavailable.response.json",
    ] {
        assert_fixture_round_trip::<ProviderDiagnosticsResponse>(&command_fixture(current, name));
    }

    let read = |name: &str| -> ProviderDiagnosticsResponse {
        serde_json::from_str(
            &fs::read_to_string(repository_path(&command_fixture(current, name)))
                .expect("provider diagnostics fixture should be readable"),
        )
        .expect("provider diagnostics fixture should match Rust types")
    };
    let supported = read("provider_diagnostics.supported.response.json");
    let unknown = read("provider_diagnostics.unknown.response.json");
    let unavailable = read("provider_diagnostics.unavailable.response.json");

    for response in [&supported, &unknown, &unavailable] {
        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.capabilities.len(), 16);
        assert_eq!(
            response
                .capabilities
                .iter()
                .map(|entry| entry.capability)
                .collect::<BTreeSet<_>>()
                .len(),
            16
        );
    }
    assert_eq!(supported.compatibility, ProviderCompatibility::Supported);
    assert!(
        supported
            .capabilities
            .iter()
            .any(|entry| entry.status == CapabilityStatus::Degraded)
    );
    assert!(
        supported
            .capabilities
            .iter()
            .any(|entry| entry.status == CapabilityStatus::Unsupported)
    );
    assert_eq!(unknown.compatibility, ProviderCompatibility::Unknown);
    assert_eq!(unknown.fingerprint_mismatches.len(), 8);
    assert!(
        unknown
            .capabilities
            .iter()
            .all(|entry| entry.status == CapabilityStatus::Unknown)
    );
    assert_eq!(
        unavailable.compatibility,
        ProviderCompatibility::Unavailable
    );
    assert_eq!(
        unavailable.unavailable_reason,
        Some(ProviderUnavailableReason::ExecutableNotFound)
    );
    assert!(
        unavailable
            .capabilities
            .iter()
            .all(|entry| entry.status == CapabilityStatus::Unavailable)
    );

    let rendered =
        serde_json::to_string(&[supported, unknown, unavailable]).expect("diagnostics serialize");
    for forbidden in [
        "canonical_path",
        "selected_path",
        "stderr",
        "stdout",
        "combined_schema_sha256\":",
        "v2_schema_sha256\":",
        "executable_sha256\":",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "diagnostics response must not expose {forbidden}"
        );
    }
}

#[test]
fn current_quit_impact_fixtures_are_exact_bounded_and_path_free() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<QuitImpactRequest>(&command_fixture(
        current,
        "quit_impact.request.json",
    ));
    assert_fixture_round_trip::<QuitImpactResponse>(&command_fixture(
        current,
        "quit_impact.response.json",
    ));
    let response: QuitImpactResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "quit_impact.response.json",
        )))
        .expect("Quit impact fixture should be readable"),
    )
    .expect("Quit impact fixture should match Rust types");
    assert!(response.flit_monitoring_stops);
    assert!(response.flit_notifications_stop);
    assert_eq!(
        response
            .runs
            .iter()
            .map(|run| (run.execution_after_quit, run.reason))
            .collect::<Vec<_>>(),
        [
            (
                ProviderExecutionAfterQuit::Continues,
                QuitImpactReason::CapabilitySupported,
            ),
            (
                ProviderExecutionAfterQuit::Stops,
                QuitImpactReason::CapabilityUnsupported,
            ),
            (
                ProviderExecutionAfterQuit::Unknown,
                QuitImpactReason::CapabilityMissing,
            ),
        ]
    );
    let rendered = serde_json::to_string(&response).expect("Quit impact should serialize");
    for forbidden in [
        "canonical_path",
        "executable",
        "session_id",
        "provider_thread",
        "capabilities",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "Quit impact fixture must not expose {forbidden}"
        );
    }

    for required in [
        "core_instance_id",
        "cursor",
        "flit_monitoring_stops",
        "flit_notifications_stop",
        "runs",
    ] {
        let mut missing: serde_json::Value =
            serde_json::from_str(&rendered).expect("Quit impact fixture should be JSON");
        missing
            .as_object_mut()
            .expect("Quit impact should be an object")
            .remove(required);
        assert!(
            serde_json::from_value::<QuitImpactResponse>(missing).is_err(),
            "Quit impact must require {required}"
        );
    }
    let mut invented: serde_json::Value =
        serde_json::from_str(&rendered).expect("Quit impact fixture should be JSON");
    invented["runs"][0]["execution_after_quit"] = serde_json::json!("probably_continues");
    assert!(serde_json::from_value::<QuitImpactResponse>(invented).is_err());
}

#[test]
fn current_managed_run_start_fixtures_round_trip_every_shape() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    for name in [
        "managed_run_start.request.json",
        "managed_run_start.provider_auto.request.json",
    ] {
        assert_fixture_round_trip::<ManagedRunStartRequest>(&command_fixture(current, name));
    }
    let request_path = command_fixture(current, "managed_run_start.request.json");
    let request: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&request_path))
            .expect("managed Run request fixture should be readable"),
    )
    .expect("managed Run request fixture should be JSON");
    for required in ["git_baseline_observed_at", "git_baseline_event_id"] {
        let mut missing = request.clone();
        missing
            .as_object_mut()
            .expect("managed Run request should be an object")
            .remove(required);
        assert!(
            serde_json::from_value::<ManagedRunStartRequest>(missing).is_err(),
            "managed Run start must require {required}"
        );
    }
    for (name, provider_configuration) in [
        (
            "managed_run_start.response.json",
            "readOnly+on-request+user",
        ),
        (
            "managed_run_start.provider_auto.response.json",
            "readOnly+on-request+auto_review",
        ),
    ] {
        let fixture = command_fixture(current, name);
        assert_fixture_round_trip::<ManagedRunStartResponse>(&fixture);
        let response: ManagedRunStartResponse = serde_json::from_str(
            &fs::read_to_string(repository_path(&fixture))
                .expect("managed Run response fixture should be readable"),
        )
        .expect("managed Run response fixture should match Rust types");
        assert_eq!(response.provider_configuration, provider_configuration);
    }
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        current,
        "managed_run_errors.json",
    ));
    let errors: Vec<CommandError> = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "managed_run_errors.json",
        )))
        .expect("managed Run error fixture should be readable"),
    )
    .expect("managed Run error fixture should match Rust types");
    for error in errors {
        assert_eq!(error, CommandError::for_code(error.code));
    }
}

#[test]
fn current_managed_run_observe_fixtures_round_trip_every_shape() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<ManagedRunObserveRequest>(&command_fixture(
        current,
        "managed_run_observe.request.json",
    ));
    for name in [
        "managed_run_observe.permission_requested.response.json",
        "managed_run_observe.provider_outcome_resolved.response.json",
        "managed_run_observe.turn_completed.response.json",
        "managed_run_observe.turn_interrupted.response.json",
    ] {
        assert_fixture_round_trip::<ManagedRunObserveResponse>(&command_fixture(current, name));
    }
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        current,
        "managed_run_observe_errors.json",
    ));

    let provider_auto_start: ManagedRunStartResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "managed_run_start.provider_auto.response.json",
        )))
        .expect("ProviderAuto start fixture should be readable"),
    )
    .expect("ProviderAuto start fixture should match Rust types");
    let provider_auto_observation: ManagedRunObserveResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "managed_run_observe.provider_outcome_resolved.response.json",
        )))
        .expect("ProviderAuto observation fixture should be readable"),
    )
    .expect("ProviderAuto observation fixture should match Rust types");
    match provider_auto_observation {
        ManagedRunObserveResponse::ProviderOutcomeResolved {
            run_id,
            session_id,
            request_version,
            event_version,
            ..
        } => {
            assert_eq!(run_id, provider_auto_start.run_id);
            assert_eq!(session_id, provider_auto_start.session_id);
            assert_eq!(request_version, 5);
            assert_eq!(event_version, 6);
        }
        _ => panic!("ProviderAuto observation fixture should be a resolved outcome"),
    }

    for (name, expected_version) in [
        ("managed_run_observe.permission_requested.response.json", 5),
        ("managed_run_observe.turn_completed.response.json", 5),
        ("managed_run_observe.turn_interrupted.response.json", 5),
    ] {
        let response: ManagedRunObserveResponse = serde_json::from_str(
            &fs::read_to_string(repository_path(&command_fixture(current, name)))
                .expect("managed observation fixture should be readable"),
        )
        .expect("managed observation fixture should match Rust types");
        let version = match response {
            ManagedRunObserveResponse::PermissionRequested {
                request_version, ..
            } => request_version,
            ManagedRunObserveResponse::TurnCompleted { event_version, .. }
            | ManagedRunObserveResponse::TurnInterrupted { event_version, .. } => event_version,
            ManagedRunObserveResponse::ProviderOutcomeResolved { .. } => {
                panic!("fixture should represent the named observation shape")
            }
        };
        assert_eq!(version, expected_version, "stale version in {name}");
    }
}

#[test]
fn current_managed_permission_response_fixtures_round_trip_every_shape() {
    let manifest = read_command_compatibility_manifest();
    let current = &manifest.current;
    assert_fixture_round_trip::<ManagedRunPermissionRespondRequest>(&command_fixture(
        current,
        "managed_run_permission_respond.request.json",
    ));
    for name in [
        "managed_run_permission_respond.delivered.response.json",
        "managed_run_permission_respond.delivery_unknown.response.json",
    ] {
        assert_fixture_round_trip::<ManagedRunPermissionRespondResponse>(&command_fixture(
            current, name,
        ));
    }
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        current,
        "managed_run_permission_respond_errors.json",
    ));

    let request: ManagedRunPermissionRespondRequest = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            current,
            "managed_run_permission_respond.request.json",
        )))
        .expect("permission response request fixture should be readable"),
    )
    .expect("permission response request fixture should match Rust types");
    assert_eq!(request.request_version, 5);
    for name in [
        "managed_run_permission_respond.delivered.response.json",
        "managed_run_permission_respond.delivery_unknown.response.json",
    ] {
        let response: ManagedRunPermissionRespondResponse = serde_json::from_str(
            &fs::read_to_string(repository_path(&command_fixture(current, name)))
                .expect("permission response fixture should be readable"),
        )
        .expect("permission response fixture should match Rust types");
        let (request_version, submitted_version, outcome_version) = match response {
            ManagedRunPermissionRespondResponse::Delivered {
                request_version,
                submitted_version,
                outcome_version,
                ..
            }
            | ManagedRunPermissionRespondResponse::DeliveryUnknown {
                request_version,
                submitted_version,
                outcome_version,
                ..
            } => (request_version, submitted_version, outcome_version),
        };
        assert_eq!(request_version, 5, "stale request version in {name}");
        assert_eq!(submitted_version, 6, "stale submit version in {name}");
        assert_eq!(outcome_version, 7, "stale outcome version in {name}");
    }
}

#[test]
fn command_manifest_retains_the_exact_previous_minor_health_contract() {
    let manifest = read_command_compatibility_manifest();
    assert_eq!(manifest.current.version, PROTOCOL_VERSION);
    assert_eq!(
        manifest.current.directory,
        format!("fixtures/protocol/commands/v{PROTOCOL_VERSION}")
    );
    let (major, minor) = PROTOCOL_VERSION
        .split_once('.')
        .expect("command protocol version should be major.minor");
    let expected_previous = format!(
        "{major}.{}",
        minor
            .parse::<u64>()
            .expect("minor should be numeric")
            .checked_sub(1)
            .expect("current command protocol should have a previous minor")
    );
    let previous = manifest
        .previous_minor
        .expect("non-initial command minor should retain its predecessor");
    assert_eq!(previous.version, expected_previous);
    assert_eq!(
        previous.directory,
        format!("fixtures/protocol/commands/v{expected_previous}")
    );
    assert_fixture_round_trip::<SystemHealthRequest>(&command_fixture(
        &previous,
        "system_health.request.json",
    ));
    assert_fixture_round_trip::<SystemHealthResponse>(&command_fixture(
        &previous,
        "system_health.response.json",
    ));
    assert_fixture_round_trip::<CommandError>(&command_fixture(
        &previous,
        "protocol_mismatch.error.json",
    ));
    assert_fixture_round_trip::<ProjectInspectionRequest>(&command_fixture(
        &previous,
        "project_inspect.request.json",
    ));
    assert_fixture_round_trip::<ProjectInspectionResponse>(&command_fixture(
        &previous,
        "project_inspect.response.json",
    ));
    assert_fixture_round_trip::<ProjectRegistrationRequest>(&command_fixture(
        &previous,
        "project_register.request.json",
    ));
    for name in [
        "project_register.registered.response.json",
        "project_register.duplicate_canonical_path.response.json",
        "project_register.duplicate_filesystem_identity.response.json",
    ] {
        assert_fixture_round_trip::<ProjectRegistrationResponse>(&command_fixture(&previous, name));
    }
    assert_fixture_round_trip::<ProjectTrustRequest>(&command_fixture(
        &previous,
        "project_trust.request.json",
    ));
    for name in [
        "project_trust.trusted.response.json",
        "project_trust.already_trusted.response.json",
    ] {
        assert_fixture_round_trip::<ProjectTrustResponse>(&command_fixture(&previous, name));
    }
    assert_fixture_round_trip::<ProjectsListRequest>(&command_fixture(
        &previous,
        "projects_list.request.json",
    ));
    assert_fixture_round_trip::<ProjectsListResponse>(&command_fixture(
        &previous,
        "projects_list.response.json",
    ));
    assert_fixture_round_trip::<Vec<CommandError>>(&command_fixture(
        &previous,
        "project_errors.json",
    ));
}

#[test]
fn generated_swift_project_contract_is_current_and_required_fields_fail_closed() {
    let generated = generated_swift_command_contract();
    assert!(generated.contains(&format!(
        "let flitClientProtocolVersion = \"{PROTOCOL_VERSION}\""
    )));
    assert!(generated.contains(&format!(
        "let flitEventSchemaVersion = \"{EVENT_PROTOCOL_VERSION}\""
    )));
    for type_name in [
        "FlitProjectInspectionRequest",
        "FlitProjectInspectionResponse",
        "FlitProjectRegistrationRequest",
        "FlitProjectRegistrationResponse",
        "FlitProjectTrustRequest",
        "FlitProjectTrustResponse",
        "FlitProjectsListRequest",
        "FlitProjectsListResponse",
        "FlitGitObservationRequest",
        "FlitGitObservationResponse",
        "FlitGitObservationUnavailableReason",
        "FlitGitHead",
        "FlitGitDirtySummary",
        "FlitDashboardReadRequest",
        "FlitDashboardDelivery",
        "FlitDashboardSnapshotReason",
        "FlitDashboardRunRecord",
        "FlitDashboardChangeSummary",
        "FlitDashboardEventRecord",
        "FlitDashboardReadResponse",
        "FlitRunDetailReadRequest",
        "FlitRunEvidenceRecord",
        "FlitRunDetailReadResponse",
        "FlitManagedRunOpenInProviderRequest",
        "FlitCommandError",
        "FlitProviderDiagnosticsRequest",
        "FlitProviderDiagnosticsResponse",
        "FlitQuitImpactRequest",
        "FlitProviderExecutionAfterQuit",
        "FlitQuitImpactReason",
        "FlitQuitImpactRun",
        "FlitQuitImpactResponse",
        "FlitManagedRunPermissionMode",
        "FlitManagedRunStartRequest",
        "FlitManagedRunStartResponse",
        "FlitManagedRunObserveRequest",
        "FlitManagedRunObservationStatus",
        "FlitManagedRunProviderDecision",
        "FlitManagedRunProviderTerminalOutcome",
        "FlitManagedRunObserveResponse",
        "FlitManagedRunPermissionDecision",
        "FlitManagedRunPermissionRespondRequest",
        "FlitManagedRunPermissionResponseStatus",
        "FlitManagedRunPermissionRespondResponse",
    ] {
        assert!(
            generated.contains(type_name),
            "generated Swift contract should contain {type_name}"
        );
    }
    assert!(generated.contains("case providerAuto = \"provider_auto\""));
    assert!(generated.contains("case providerOutcomeResolved = \"provider_outcome_resolved\""));
    assert!(generated.contains("case permissionModeConfigure = \"permission_mode_configure\""));
    assert!(generated.contains("case providerOutcomeObserve = \"provider_outcome_observe\""));
    assert!(generated.contains("let providerConfiguration: String"));
    assert!(!generated.contains("approveForMe"));
    assert!(!generated.contains("providerPolicy"));

    let manifest = read_command_compatibility_manifest();
    let mut drifted: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            &manifest.current,
            "project_inspect.response.json",
        )))
        .expect("inspection fixture should be readable"),
    )
    .expect("inspection fixture should be JSON");
    drifted
        .as_object_mut()
        .expect("inspection response should be an object")
        .remove("filesystem_id");
    assert!(serde_json::from_value::<ProjectInspectionResponse>(drifted).is_err());

    let mut unknown_status: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            &manifest.current,
            "project_register.registered.response.json",
        )))
        .expect("registration fixture should be readable"),
    )
    .expect("registration fixture should be JSON");
    unknown_status["status"] = serde_json::json!("silently_invented");
    assert!(serde_json::from_value::<ProjectRegistrationResponse>(unknown_status).is_err());

    let mut unknown_permission_status: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&command_fixture(
            &manifest.current,
            "managed_run_permission_respond.delivered.response.json",
        )))
        .expect("permission response fixture should be readable"),
    )
    .expect("permission response fixture should be JSON");
    unknown_permission_status["status"] = serde_json::json!("silently_delivered");
    assert!(
        serde_json::from_value::<ManagedRunPermissionRespondResponse>(unknown_permission_status)
            .is_err()
    );
}

#[test]
fn current_event_fixture_round_trips_without_losing_unknown_fields() {
    let manifest = read_compatibility_manifest();
    let fixture = fs::read_to_string(repository_path(&manifest.current.fixture))
        .expect("fixture should be readable");
    let expected: serde_json::Value =
        serde_json::from_str(&fixture).expect("fixture should contain valid JSON");
    let decoded: EventEnvelope =
        serde_json::from_value(expected.clone()).expect("fixture should match event envelope");

    assert_eq!(expected["protocol_version"], EVENT_PROTOCOL_VERSION);
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn current_git_baseline_event_payload_is_exact_and_mixed_variants_fail_closed() {
    let manifest = read_compatibility_manifest();
    let fixture: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.fixture))
            .expect("current event fixture should be readable"),
    )
    .expect("current event fixture should contain valid JSON");
    let payload = fixture["payload"].clone();
    let decoded: GitBaselinePayload =
        serde_json::from_value(payload.clone()).expect("Git baseline payload");
    assert_eq!(serde_json::to_value(decoded).unwrap(), payload);

    let mut mixed = payload.clone();
    mixed["reason"] = serde_json::json!("process_unavailable");
    assert!(serde_json::from_value::<GitBaselinePayload>(mixed).is_err());

    let unavailable = serde_json::json!({
        "availability": "unavailable",
        "project_id": "project-1",
        "reason": "not_repository"
    });
    assert!(serde_json::from_value::<GitBaselinePayload>(unavailable.clone()).is_ok());
    let mut mixed_unavailable = unavailable;
    mixed_unavailable["dirty"] = serde_json::json!({
        "staged": 0,
        "unstaged": 0,
        "untracked": 0,
        "entries": 0
    });
    assert!(serde_json::from_value::<GitBaselinePayload>(mixed_unavailable).is_err());
}

#[test]
fn event_schema_accepts_current_fixture_and_rejects_invalid_boundaries() {
    let manifest = read_compatibility_manifest();
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.schema))
            .expect("event schema should be readable"),
    )
    .expect("event schema should contain valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("event schema should compile");
    let fixture: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.fixture))
            .expect("event fixture should be readable"),
    )
    .expect("event fixture should contain valid JSON");

    assert!(validator.is_valid(&fixture));

    let mut invalid_confidence = fixture.clone();
    invalid_confidence["confidence"] = serde_json::json!(1.01);
    assert!(!validator.is_valid(&invalid_confidence));

    let mut invalid_stream_sequence = fixture.clone();
    invalid_stream_sequence["stream_seq"] = serde_json::json!(0);
    assert!(!validator.is_valid(&invalid_stream_sequence));

    let mut invalid_payload = fixture;
    invalid_payload["payload"] = serde_json::json!("not an object");
    assert!(!validator.is_valid(&invalid_payload));

    let mut unsafe_json_sequence = invalid_payload;
    unsafe_json_sequence["payload"] = serde_json::json!({});
    unsafe_json_sequence["ingest_seq"] = serde_json::json!(MAX_JSON_SAFE_INTEGER + 1);
    assert!(!validator.is_valid(&unsafe_json_sequence));

    let mut missing_session_id = unsafe_json_sequence;
    missing_session_id["ingest_seq"] = serde_json::json!(1);
    missing_session_id
        .as_object_mut()
        .expect("fixture should be an object")
        .remove("session_id");
    assert!(!validator.is_valid(&missing_session_id));
    assert!(serde_json::from_value::<EventEnvelope>(missing_session_id).is_err());
}

#[derive(Clone, Debug, Deserialize)]
struct CompatibilityManifest {
    current: FixtureReference,
    previous_minor: Option<FixtureReference>,
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureReference {
    version: String,
    schema: String,
    fixture: String,
}

fn read_compatibility_manifest() -> CompatibilityManifest {
    serde_json::from_str(
        &fs::read_to_string(repository_path(
            "fixtures/protocol/events/compatibility.json",
        ))
        .expect("compatibility manifest should be readable"),
    )
    .expect("compatibility manifest should contain valid JSON")
}

#[derive(Clone)]
struct CurrentContractSnapshot {
    expected_version: String,
    serialized_version: String,
    generated_schema_path: String,
    schema_id: String,
    fixture_version: String,
    manifest_version: String,
    manifest_schema_path: String,
}

fn validate_current_contract(snapshot: &CurrentContractSnapshot) -> Result<(), String> {
    let expected_path = format!(
        "schemas/protocol/events/v{}/event.schema.json",
        snapshot.expected_version
    );
    let expected_id = format!("urn:flit:protocol:event:{}", snapshot.expected_version);
    if snapshot.serialized_version != snapshot.expected_version {
        return Err("serialized event version must match current".to_owned());
    }
    if snapshot.generated_schema_path != expected_path {
        return Err("generated schema path must match current".to_owned());
    }
    if snapshot.schema_id != expected_id {
        return Err("schema ID must match current".to_owned());
    }
    if snapshot.fixture_version != snapshot.expected_version {
        return Err("fixture event version must match current".to_owned());
    }
    if snapshot.manifest_version != snapshot.expected_version {
        return Err("manifest event version must match current".to_owned());
    }
    if snapshot.manifest_schema_path != expected_path {
        return Err("manifest schema path must match generated path".to_owned());
    }
    Ok(())
}

fn current_contract_snapshot() -> CurrentContractSnapshot {
    let manifest = read_compatibility_manifest();
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.schema))
            .expect("current schema should be readable"),
    )
    .expect("current schema should contain valid JSON");
    let fixture: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.fixture))
            .expect("current fixture should be readable"),
    )
    .expect("current fixture should contain valid JSON");
    let serialized_version = serde_json::to_value(EventProtocolVersion::V1_1)
        .expect("event version should serialize")
        .as_str()
        .expect("event version should serialize as a string")
        .to_owned();

    CurrentContractSnapshot {
        expected_version: EVENT_PROTOCOL_VERSION.to_owned(),
        serialized_version,
        generated_schema_path: event_schema_relative_path(),
        schema_id: schema["$id"]
            .as_str()
            .expect("event schema should declare an ID")
            .to_owned(),
        fixture_version: fixture["protocol_version"]
            .as_str()
            .expect("event fixture should declare a protocol version")
            .to_owned(),
        manifest_version: manifest.current.version,
        manifest_schema_path: manifest.current.schema,
    }
}

#[test]
fn current_event_version_sources_cannot_drift_independently() {
    let current = current_contract_snapshot();
    validate_current_contract(&current).expect("current event version sources should agree");
    assert_eq!(current.schema_id, event_schema_id());

    for mutate in [
        |snapshot: &mut CurrentContractSnapshot| snapshot.serialized_version = "0.9".to_owned(),
        |snapshot: &mut CurrentContractSnapshot| {
            snapshot.schema_id = "urn:flit:protocol:event:0.9".to_owned()
        },
        |snapshot: &mut CurrentContractSnapshot| {
            snapshot.generated_schema_path =
                "schemas/protocol/events/v0.9/event.schema.json".to_owned()
        },
        |snapshot: &mut CurrentContractSnapshot| snapshot.fixture_version = "0.9".to_owned(),
        |snapshot: &mut CurrentContractSnapshot| snapshot.manifest_version = "0.9".to_owned(),
        |snapshot: &mut CurrentContractSnapshot| {
            snapshot.manifest_schema_path =
                "schemas/protocol/events/v0.9/event.schema.json".to_owned()
        },
    ] {
        let mut stale = current.clone();
        mutate(&mut stale);
        assert!(validate_current_contract(&stale).is_err());
    }
}

fn validate_compatibility_manifest(
    manifest: &CompatibilityManifest,
    expected_current: &str,
    require_files: bool,
) -> Result<(), String> {
    if manifest.current.version != expected_current {
        return Err("current event version must match the Rust protocol version".to_owned());
    }

    let (major, minor) = manifest
        .current
        .version
        .split_once('.')
        .ok_or_else(|| "event version must be major.minor".to_owned())?;
    let minor = minor
        .parse::<u64>()
        .map_err(|_| "event minor version must be numeric".to_owned())?;

    match (minor.checked_sub(1), &manifest.previous_minor) {
        (None, None) => {}
        (None, Some(_)) => {
            return Err("initial minor must not invent a previous fixture".to_owned());
        }
        (Some(_), None) => return Err("non-initial minor requires a previous fixture".to_owned()),
        (Some(previous_minor), Some(previous)) => {
            let expected = format!("{major}.{previous_minor}");
            if previous.version != expected {
                return Err("previous fixture must be the exact preceding minor".to_owned());
            }
        }
    }

    for reference in std::iter::once(&manifest.current).chain(manifest.previous_minor.as_ref()) {
        let version_segment = format!("/v{}/", reference.version);
        if !reference.schema.contains(&version_segment)
            || !reference.fixture.contains(&version_segment)
        {
            return Err("manifest paths must be bound to their declared version".to_owned());
        }
    }

    if require_files {
        for reference in std::iter::once(&manifest.current).chain(manifest.previous_minor.as_ref())
        {
            let schema_path = repository_path(&reference.schema);
            let fixture_path = repository_path(&reference.fixture);
            if !schema_path.is_file() || !fixture_path.is_file() {
                return Err("manifest schema and fixture paths must exist".to_owned());
            }

            let schema: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(schema_path)
                    .map_err(|_| "manifest schema must be readable".to_owned())?,
            )
            .map_err(|_| "manifest schema must contain valid JSON".to_owned())?;
            let fixture: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(fixture_path)
                    .map_err(|_| "manifest fixture must be readable".to_owned())?,
            )
            .map_err(|_| "manifest fixture must contain valid JSON".to_owned())?;
            let validator = jsonschema::validator_for(&schema)
                .map_err(|_| "manifest schema must compile".to_owned())?;
            if !validator.is_valid(&fixture) {
                return Err("manifest fixture must validate against its schema".to_owned());
            }
        }
    }

    Ok(())
}

#[test]
fn compatibility_manifest_enforces_initial_and_future_minor_rules() {
    let manifest = read_compatibility_manifest();
    validate_compatibility_manifest(&manifest, EVENT_PROTOCOL_VERSION, true)
        .expect("current manifest should be valid");

    let future_without_previous: CompatibilityManifest =
        serde_json::from_value(serde_json::json!({
            "current": {
                "version": "1.1",
                "schema": "schemas/protocol/events/v1.1/event.schema.json",
                "fixture": "fixtures/protocol/events/v1.1/permission.requested.json"
            },
            "previous_minor": null
        }))
        .unwrap();
    assert!(validate_compatibility_manifest(&future_without_previous, "1.1", false).is_err());

    let future_with_wrong_previous: CompatibilityManifest =
        serde_json::from_value(serde_json::json!({
            "current": {
                "version": "1.1",
                "schema": "schemas/protocol/events/v1.1/event.schema.json",
                "fixture": "fixtures/protocol/events/v1.1/permission.requested.json"
            },
            "previous_minor": {
                "version": "0.9",
                "schema": "schemas/protocol/events/v0.9/event.schema.json",
                "fixture": "fixtures/protocol/events/v0.9/permission.requested.json"
            }
        }))
        .unwrap();
    assert!(validate_compatibility_manifest(&future_with_wrong_previous, "1.1", false).is_err());

    let future_with_previous: CompatibilityManifest = serde_json::from_value(serde_json::json!({
        "current": {
            "version": "1.1",
            "schema": "schemas/protocol/events/v1.1/event.schema.json",
            "fixture": "fixtures/protocol/events/v1.1/permission.requested.json"
        },
        "previous_minor": {
            "version": "1.0",
            "schema": "schemas/protocol/events/v1.0/event.schema.json",
            "fixture": "fixtures/protocol/events/v1.0/permission.requested.json"
        }
    }))
    .unwrap();
    validate_compatibility_manifest(&future_with_previous, "1.1", false)
        .expect("future manifest should accept the exact preceding minor");
}
