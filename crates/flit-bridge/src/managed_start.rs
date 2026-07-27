use std::{collections::BTreeMap, ffi::OsStr, path::Path};

use flit_protocol::{
    ManagedRunObserveRequest, ManagedRunObserveResponse, ManagedRunPermissionDecision,
    ManagedRunPermissionMode, ManagedRunPermissionRespondRequest, ManagedRunStartRequest,
    ManagedRunStartResponse, PROTOCOL_VERSION,
};
use flit_providers::{
    CapabilityStatus, CodexAppServer, CodexAppServerError, CodexContractError,
    CodexManagedThreadId, CodexManualStartedThread, CodexStartedTurn, CodexTurnObservation,
    CodexTurnTerminalOutcome, ProviderCapability, ProviderCompatibility, ProviderFingerprint,
    classify_codex,
};
use flit_store::{
    AppendEventOutcome, InitialManagedSessionConnection, ManagedPermissionDecision,
    ManagedPermissionResolutionKind, ManagedPermissionResponseAttempt,
    ManagedPermissionResponseResult, ManagedPermissionResponseResultKind,
    ManagedProviderObservation, ManagedProviderObservationKind, ManagedReconciliationState,
    ManagedRunIntent, ManagedRunIntentOutcome, ManagedRunStartFailure,
    ManagedSessionReconciliation, Store, StoreError,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const MAX_MANAGED_ID_BYTES: usize = 1_024;
const MAX_MANAGED_TITLE_BYTES: usize = 4 * 1_024;
const MAX_MANAGED_GOAL_BYTES: usize = 64 * 1_024;
const MAX_MANAGED_TIMESTAMP_BYTES: usize = 128;
const MANUAL_MODE_VERSION: u64 = 1;
const MANUAL_PROVIDER_CONFIGURATION: &str = "readOnly+on-request+user";
#[cfg(test)]
const MAX_MANAGED_OBSERVATIONS_PER_CALL: usize =
    flit_providers::MAX_CODEX_COMMAND_STARTS_PER_TURN + 1;

pub(crate) trait ManagedCodexConnector {
    fn connect(&self, path_environment: Option<&OsStr>)
    -> Result<Box<dyn ManagedCodexRuntime>, ()>;
}

pub(crate) trait ManagedCodexRuntime: Send {
    fn validated_profile(&self) -> Option<&ProviderFingerprint>;
    fn start_manual(
        &mut self,
        cwd: &Path,
    ) -> Result<CodexManualStartedThread, ProviderStartAttemptError>;
    fn start_turn(
        &mut self,
        thread_id: &CodexManagedThreadId,
        prompt: &str,
    ) -> Result<CodexStartedTurn, ()>;
    fn wait_for_turn_observation(&mut self) -> Result<CodexTurnObservation, ()>;
    fn respond_to_file_change_permission(
        &mut self,
        request: &flit_providers::CodexPermissionRequest,
        decision: flit_providers::CodexPermissionDecision,
    ) -> Result<flit_providers::CodexPermissionDelivery, ()>;
    fn delete_started_thread(self: Box<Self>, thread_id: &CodexManagedThreadId) -> Result<(), ()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderStartAttemptError {
    Rejected,
    Unknown,
}

pub(crate) struct ProductionCodexConnector;

impl ManagedCodexConnector for ProductionCodexConnector {
    fn connect(
        &self,
        path_environment: Option<&OsStr>,
    ) -> Result<Box<dyn ManagedCodexRuntime>, ()> {
        CodexAppServer::connect_on_path(path_environment)
            .map(|runtime| Box::new(runtime) as Box<dyn ManagedCodexRuntime>)
            .map_err(|_| ())
    }
}

impl ManagedCodexRuntime for CodexAppServer {
    fn validated_profile(&self) -> Option<&ProviderFingerprint> {
        self.validated_profile()
    }

    fn start_manual(
        &mut self,
        cwd: &Path,
    ) -> Result<CodexManualStartedThread, ProviderStartAttemptError> {
        CodexAppServer::start_manual(self, cwd).map_err(|error| match error {
            CodexAppServerError::ManualPolicyUnavailable
            | CodexAppServerError::Contract(CodexContractError::ServerError) => {
                ProviderStartAttemptError::Rejected
            }
            _ => ProviderStartAttemptError::Unknown,
        })
    }

    fn start_turn(
        &mut self,
        thread_id: &CodexManagedThreadId,
        prompt: &str,
    ) -> Result<CodexStartedTurn, ()> {
        CodexAppServer::start_turn(self, thread_id, prompt).map_err(|_| ())
    }

    fn wait_for_turn_observation(&mut self) -> Result<CodexTurnObservation, ()> {
        CodexAppServer::wait_for_turn_observation(self).map_err(|_| ())
    }

    fn respond_to_file_change_permission(
        &mut self,
        request: &flit_providers::CodexPermissionRequest,
        decision: flit_providers::CodexPermissionDecision,
    ) -> Result<flit_providers::CodexPermissionDelivery, ()> {
        CodexAppServer::respond_to_file_change_permission(self, request, decision).map_err(|_| ())
    }

    fn delete_started_thread(self: Box<Self>, thread_id: &CodexManagedThreadId) -> Result<(), ()> {
        (*self)
            .delete_started_thread(thread_id)
            .map(|_| ())
            .map_err(|_| ())
    }
}

pub(crate) struct RetainedManagedRun {
    request_digest: String,
    response: ManagedRunStartResponse,
    provider: Box<dyn ManagedCodexRuntime>,
    active_permission: Option<RetainedPermission>,
}

pub(crate) struct RetainedPermission {
    response: ManagedRunObserveResponse,
    native_request: flit_providers::CodexPermissionRequest,
}

impl RetainedManagedRun {
    pub(crate) fn cached_permission(&self) -> Option<ManagedRunObserveResponse> {
        self.active_permission
            .as_ref()
            .map(|permission| permission.response.clone())
    }

    pub(crate) fn active_permission(
        &self,
    ) -> Option<(
        &ManagedRunObserveResponse,
        &flit_providers::CodexPermissionRequest,
    )> {
        self.active_permission
            .as_ref()
            .map(|permission| (&permission.response, &permission.native_request))
    }

    pub(crate) fn clear_active_permission(&mut self) {
        self.active_permission = None;
    }

    pub(crate) fn respond_to_active_permission(
        &mut self,
        decision: flit_providers::CodexPermissionDecision,
    ) -> Result<flit_providers::CodexPermissionDelivery, ()> {
        let request = self
            .active_permission
            .as_ref()
            .ok_or(())?
            .native_request
            .clone();
        self.provider
            .respond_to_file_change_permission(&request, decision)
    }

    pub(crate) fn start_response(&self) -> ManagedRunStartResponse {
        self.response.clone()
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        response: ManagedRunStartResponse,
        provider: Box<dyn ManagedCodexRuntime>,
    ) -> Self {
        Self {
            request_digest: "test-runtime".to_owned(),
            response,
            provider,
            active_permission: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedStartError {
    InvalidRequest,
    RunConflict,
    ProjectNotFound,
    ProjectNotTrusted,
    ProjectIdentityMismatch,
    ProviderUnavailable,
    ProviderStartFailed,
    ProviderStartUnknown,
    StorageUnavailable,
    RunNotActive,
    ProviderObservationUnknown,
    PermissionRequestStale,
    PermissionResponseConflict,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedPermissionResponse {
    pub(crate) attempt: ManagedPermissionResponseAttempt,
    pub(crate) provider_decision: flit_providers::CodexPermissionDecision,
    pub(crate) resolution: ManagedPermissionResolutionKind,
}

pub(crate) fn prepare_permission_response(
    retained: &RetainedManagedRun,
    request: &ManagedRunPermissionRespondRequest,
) -> Result<PreparedPermissionResponse, ManagedStartError> {
    validate_permission_response_request(request)?;
    let Some((response, native)) = retained.active_permission() else {
        return Err(ManagedStartError::PermissionRequestStale);
    };
    let ManagedRunObserveResponse::PermissionRequested {
        run_id,
        session_id,
        provider_thread_id,
        provider_turn_id,
        provider_item_id,
        provider_request_id,
        request_id,
        request_version,
        event_id,
        ..
    } = response
    else {
        return Err(ManagedStartError::PermissionRequestStale);
    };
    if run_id != &request.run_id
        || request_id != &request.request_id
        || *request_version != request.request_version
        || provider_thread_id != native.thread_id.as_str()
        || provider_turn_id != native.turn_id.as_str()
        || provider_item_id != native.item_id.as_str()
        || *provider_request_id != native.provider_request_id
    {
        return Err(ManagedStartError::PermissionRequestStale);
    }
    let (provider_decision, store_decision, resolution) = match request.decision {
        ManagedRunPermissionDecision::AllowOnce => (
            flit_providers::CodexPermissionDecision::Accept,
            ManagedPermissionDecision::AllowOnce,
            ManagedPermissionResolutionKind::AcceptedCompleted,
        ),
        ManagedRunPermissionDecision::Deny => (
            flit_providers::CodexPermissionDecision::Decline,
            ManagedPermissionDecision::Deny,
            ManagedPermissionResolutionKind::Declined,
        ),
    };
    let delivery_plan_fingerprint =
        permission_delivery_plan_fingerprint(request, session_id, native, store_decision);
    Ok(PreparedPermissionResponse {
        attempt: ManagedPermissionResponseAttempt {
            run_id: request.run_id.clone(),
            session_id: session_id.clone(),
            external_session_key: native.thread_id.as_str().to_owned(),
            provider_turn_id: native.turn_id.as_str().to_owned(),
            provider_item_id: native.item_id.as_str().to_owned(),
            provider_request_id: native.provider_request_id,
            request_id: request.request_id.clone(),
            request_version: request.request_version,
            request_event_id: event_id.clone(),
            response_attempt_id: request.response_attempt_id.clone(),
            decision: store_decision,
            delivery_plan_fingerprint,
            contract_version: "codex-app-server/0.145.0".to_owned(),
            submitted_at: request.submitted_at.clone(),
            submitted_event_id: request.submitted_event_id.clone(),
        },
        provider_decision,
        resolution,
    })
}

pub(crate) fn permission_response_result(
    prepared: &PreparedPermissionResponse,
    request: &ManagedRunPermissionRespondRequest,
    kind: ManagedPermissionResponseResultKind,
) -> ManagedPermissionResponseResult {
    let outcome_event_id = match kind {
        ManagedPermissionResponseResultKind::Resolved(_) => request.resolved_event_id.clone(),
        ManagedPermissionResponseResultKind::DeliveryUnknown(_) => {
            request.delivery_unknown_event_id.clone()
        }
    };
    ManagedPermissionResponseResult {
        run_id: prepared.attempt.run_id.clone(),
        session_id: prepared.attempt.session_id.clone(),
        external_session_key: prepared.attempt.external_session_key.clone(),
        provider_turn_id: prepared.attempt.provider_turn_id.clone(),
        provider_item_id: prepared.attempt.provider_item_id.clone(),
        provider_request_id: prepared.attempt.provider_request_id,
        request_id: prepared.attempt.request_id.clone(),
        request_version: prepared.attempt.request_version,
        response_attempt_id: prepared.attempt.response_attempt_id.clone(),
        decision: prepared.attempt.decision,
        delivery_plan_fingerprint: prepared.attempt.delivery_plan_fingerprint.clone(),
        contract_version: prepared.attempt.contract_version.clone(),
        finished_at: request.finished_at.clone(),
        outcome_event_id,
        kind,
    }
}

pub(crate) fn validate_permission_response_request(
    request: &ManagedRunPermissionRespondRequest,
) -> Result<(), ManagedStartError> {
    for value in [
        request.run_id.as_str(),
        request.request_id.as_str(),
        request.response_attempt_id.as_str(),
        request.submitted_event_id.as_str(),
        request.resolved_event_id.as_str(),
        request.delivery_unknown_event_id.as_str(),
    ] {
        validate_text(value, MAX_MANAGED_ID_BYTES)?;
    }
    validate_token(&request.submitted_at, MAX_MANAGED_TIMESTAMP_BYTES)?;
    validate_token(&request.finished_at, MAX_MANAGED_TIMESTAMP_BYTES)?;
    if request.request_version == 0
        || request.request_version > flit_protocol::MAX_JSON_SAFE_INTEGER
        || request.submitted_event_id == request.resolved_event_id
        || request.submitted_event_id == request.delivery_unknown_event_id
        || request.resolved_event_id == request.delivery_unknown_event_id
    {
        return Err(ManagedStartError::InvalidRequest);
    }
    Ok(())
}

fn permission_delivery_plan_fingerprint(
    request: &ManagedRunPermissionRespondRequest,
    session_id: &str,
    native: &flit_providers::CodexPermissionRequest,
    decision: ManagedPermissionDecision,
) -> String {
    let provider_request_id = native.provider_request_id.to_string();
    let request_version = request.request_version.to_string();
    let decision = match decision {
        ManagedPermissionDecision::AllowOnce => "allow_once",
        ManagedPermissionDecision::Deny => "deny",
    };
    let parts = [
        "codex-app-server/0.145.0",
        request.run_id.as_str(),
        session_id,
        native.thread_id.as_str(),
        native.turn_id.as_str(),
        native.item_id.as_str(),
        provider_request_id.as_str(),
        request.request_id.as_str(),
        request_version.as_str(),
        request.response_attempt_id.as_str(),
        decision,
    ];
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub(crate) fn start_managed_run(
    store: &mut Store,
    runtimes: &mut BTreeMap<String, RetainedManagedRun>,
    connector: &dyn ManagedCodexConnector,
    path_environment: Option<&OsStr>,
    request: ManagedRunStartRequest,
) -> Result<ManagedRunStartResponse, ManagedStartError> {
    validate_request(&request)?;
    let request_digest = request_digest(&request)?;
    if let Some(existing) = runtimes.get(&request.run_id) {
        return if existing.request_digest == request_digest {
            Ok(existing.response.clone())
        } else {
            Err(ManagedStartError::RunConflict)
        };
    }

    let project = store
        .project(&request.project_id)
        .map_err(|_| ManagedStartError::StorageUnavailable)?
        .ok_or(ManagedStartError::ProjectNotFound)?;
    if !project.trusted {
        return Err(ManagedStartError::ProjectNotTrusted);
    }
    validate_project_identity(&project)?;

    let start_request = json!({
        "goal_sha256": sha256_hex(request.goal.as_bytes()),
        "permission_mode": "manual",
        "permission_mode_version": MANUAL_MODE_VERSION,
        "provider": "codex",
        "protocol_version": PROTOCOL_VERSION,
    })
    .as_object()
    .expect("object literal")
    .clone();
    let intent = ManagedRunIntent {
        id: request.run_id.clone(),
        project_id: request.project_id.clone(),
        title: request.title.clone(),
        goal: Some(request.goal.clone()),
        start_request,
        baseline_head: None,
        created_at: request.created_at.clone(),
        run_created_event_id: request.run_created_event_id.clone(),
        start_requested_event_id: request.start_requested_event_id.clone(),
    };
    match store.create_managed_run_intent(intent) {
        Ok(ManagedRunIntentOutcome::Created { .. }) => {}
        Ok(ManagedRunIntentOutcome::Duplicate { run, .. }) => {
            return if run.started_at.is_none() && run.ended_at.is_some() {
                Err(ManagedStartError::ProviderStartFailed)
            } else {
                Err(ManagedStartError::ProviderStartUnknown)
            };
        }
        Err(error) => return Err(map_intent_error(error)),
    }

    let mut provider = match connector.connect(path_environment) {
        Ok(provider) => provider,
        Err(()) => {
            fail_unstarted_run(store, &request, "provider_unavailable", "unavailable")?;
            return Err(ManagedStartError::ProviderUnavailable);
        }
    };
    let Some(profile) = provider.validated_profile().cloned() else {
        fail_unstarted_run(
            store,
            &request,
            "missing_validated_profile",
            "codex-app-server/unknown",
        )?;
        return Err(ManagedStartError::ProviderUnavailable);
    };
    if profile.executable_version != "0.145.0" {
        fail_unstarted_run(
            store,
            &request,
            "unsupported_provider_profile",
            &contract_version(&profile),
        )?;
        return Err(ManagedStartError::ProviderUnavailable);
    }
    let capabilities = match managed_capabilities(&profile) {
        Ok(capabilities) => capabilities,
        Err(error) => {
            drop(provider);
            fail_unstarted_run(
                store,
                &request,
                "manual_capability_unavailable",
                &contract_version(&profile),
            )?;
            return Err(error);
        }
    };
    if validate_project_identity(&project).is_err() {
        drop(provider);
        fail_unstarted_run(
            store,
            &request,
            "project_identity_changed",
            &contract_version(&profile),
        )?;
        return Err(ManagedStartError::ProjectIdentityMismatch);
    }

    let started = match provider.start_manual(&project.canonical_path) {
        Ok(started) => started,
        Err(ProviderStartAttemptError::Rejected) => {
            fail_unstarted_run(
                store,
                &request,
                "manual_provider_start_failed",
                &contract_version(&profile),
            )?;
            return Err(ManagedStartError::ProviderStartFailed);
        }
        Err(ProviderStartAttemptError::Unknown) => {
            return Err(ManagedStartError::ProviderStartUnknown);
        }
    };
    if started.provider_configuration != MANUAL_PROVIDER_CONFIGURATION {
        if provider
            .delete_started_thread(&started.thread.thread_id)
            .is_err()
        {
            return Err(ManagedStartError::ProviderStartUnknown);
        }
        fail_unstarted_run(
            store,
            &request,
            "manual_policy_mismatch",
            &contract_version(&profile),
        )?;
        return Err(ManagedStartError::ProviderStartFailed);
    }

    let connection = InitialManagedSessionConnection {
        id: request.session_id.clone(),
        run_id: request.run_id.clone(),
        external_session_key: started.thread.thread_id.as_str().to_owned(),
        session_fingerprint: session_fingerprint(&profile),
        executable_path: Some(profile.canonical_executable.clone()),
        executable_version: Some(profile.executable_version.clone()),
        cwd: project.canonical_path.clone(),
        capabilities,
        contract_version: contract_version(&profile),
        started_at: request.started_at.clone(),
        connected_event_id: request.session_connected_event_id.clone(),
    };
    if let Err(error) = store.connect_initial_managed_session(connection) {
        if matches!(error, StoreError::ExternalSessionAlreadyClaimed { .. }) {
            drop(provider);
            return Err(ManagedStartError::ProviderStartUnknown);
        }
        if provider
            .delete_started_thread(&started.thread.thread_id)
            .is_err()
        {
            return Err(ManagedStartError::ProviderStartUnknown);
        }
        fail_unstarted_run(
            store,
            &request,
            "session_ownership_failed",
            &contract_version(&profile),
        )?;
        return Err(ManagedStartError::ProviderStartFailed);
    }

    let turn = match provider.start_turn(&started.thread.thread_id, &request.goal) {
        Ok(turn) => turn,
        Err(()) => {
            record_start_unknown(store, &request, &started.thread.thread_id, &profile)?;
            return Err(ManagedStartError::ProviderStartUnknown);
        }
    };
    let response = ManagedRunStartResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: request.run_id.clone(),
        session_id: request.session_id,
        provider_thread_id: started.thread.thread_id.as_str().to_owned(),
        provider_turn_id: turn.turn_id.as_str().to_owned(),
        permission_mode: ManagedRunPermissionMode::Manual,
        permission_mode_version: MANUAL_MODE_VERSION,
        provider_configuration: started.provider_configuration.to_owned(),
    };
    runtimes.insert(
        request.run_id,
        RetainedManagedRun {
            request_digest,
            response: response.clone(),
            provider,
            active_permission: None,
        },
    );
    Ok(response)
}

#[cfg(test)]
pub(crate) fn observe_managed_run(
    store: &mut Store,
    runtimes: &mut BTreeMap<String, RetainedManagedRun>,
    request: ManagedRunObserveRequest,
) -> Result<ManagedRunObserveResponse, ManagedStartError> {
    validate_observe_request(&request)?;
    let (result, start_response) = {
        let retained = runtimes
            .get_mut(&request.run_id)
            .ok_or(ManagedStartError::RunNotActive)?;
        if let Some(permission) = &retained.active_permission {
            return Ok(permission.response.clone());
        }
        (
            observe_retained_runtime(store, retained, &request),
            retained.response.clone(),
        )
    };
    let result = if result == Err(ManagedStartError::ProviderObservationUnknown) {
        record_observation_unknown(store, &request, &start_response)?;
        Err(ManagedStartError::ProviderObservationUnknown)
    } else {
        result
    };
    if matches!(
        result,
        Ok(ManagedRunObserveResponse::TurnCompleted { .. })
            | Ok(ManagedRunObserveResponse::TurnInterrupted { .. })
            | Err(ManagedStartError::ProviderObservationUnknown)
    ) {
        runtimes.remove(&request.run_id);
    }
    result
}

#[cfg(test)]
fn observe_retained_runtime(
    store: &mut Store,
    retained: &mut RetainedManagedRun,
    request: &ManagedRunObserveRequest,
) -> Result<ManagedRunObserveResponse, ManagedStartError> {
    for _ in 0..MAX_MANAGED_OBSERVATIONS_PER_CALL {
        let observation = wait_managed_observation(retained)?;
        match commit_managed_observation(store, retained, request, observation)? {
            ManagedObservationCommit::Continue => {}
            ManagedObservationCommit::Complete(response) => return Ok(*response),
        }
    }
    Err(ManagedStartError::ProviderObservationUnknown)
}

pub(crate) enum ManagedObservationCommit {
    Continue,
    Complete(Box<ManagedRunObserveResponse>),
}

pub(crate) fn wait_managed_observation(
    retained: &mut RetainedManagedRun,
) -> Result<CodexTurnObservation, ManagedStartError> {
    retained
        .provider
        .wait_for_turn_observation()
        .map_err(|_| ManagedStartError::ProviderObservationUnknown)
}

pub(crate) fn commit_managed_observation(
    store: &mut Store,
    retained: &mut RetainedManagedRun,
    request: &ManagedRunObserveRequest,
    observation: CodexTurnObservation,
) -> Result<ManagedObservationCommit, ManagedStartError> {
    match observation {
        CodexTurnObservation::CommandStarted {
            thread_id,
            turn_id,
            item_id,
        } => {
            validate_observation_identity(&retained.response, &thread_id, &turn_id)?;
            let event_id = observation_id(
                "evt_codex_command_",
                &[
                    request.run_id.as_str(),
                    thread_id.as_str(),
                    turn_id.as_str(),
                    item_id.as_str(),
                ],
            );
            store
                .append_managed_provider_observation(ManagedProviderObservation {
                    run_id: request.run_id.clone(),
                    session_id: retained.response.session_id.clone(),
                    external_session_key: thread_id.as_str().to_owned(),
                    provider_turn_id: turn_id.as_str().to_owned(),
                    contract_version: "codex-app-server/0.145.0".to_owned(),
                    event_id,
                    observed_at: request.observed_at.clone(),
                    kind: ManagedProviderObservationKind::CommandStarted {
                        provider_item_id: item_id.as_str().to_owned(),
                    },
                })
                .map_err(map_observation_store_error)?;
            Ok(ManagedObservationCommit::Continue)
        }
        CodexTurnObservation::PermissionRequested(permission) => {
            validate_observation_identity(
                &retained.response,
                &permission.thread_id,
                &permission.turn_id,
            )?;
            let provider_request_id = permission.provider_request_id.to_string();
            let axes = [
                request.run_id.as_str(),
                permission.thread_id.as_str(),
                permission.turn_id.as_str(),
                permission.item_id.as_str(),
                provider_request_id.as_str(),
            ];
            let request_id = observation_id("req_codex_", &axes);
            let event_id = observation_id("evt_codex_permission_", &axes);
            let outcome = store
                .append_managed_provider_observation(ManagedProviderObservation {
                    run_id: request.run_id.clone(),
                    session_id: retained.response.session_id.clone(),
                    external_session_key: permission.thread_id.as_str().to_owned(),
                    provider_turn_id: permission.turn_id.as_str().to_owned(),
                    contract_version: "codex-app-server/0.145.0".to_owned(),
                    event_id: event_id.clone(),
                    observed_at: request.observed_at.clone(),
                    kind: ManagedProviderObservationKind::PermissionRequested {
                        request_id: request_id.clone(),
                        provider_request_id: permission.provider_request_id,
                        provider_item_id: permission.item_id.as_str().to_owned(),
                        provider_started_at_ms: permission.started_at_ms,
                    },
                })
                .map_err(map_observation_store_error)?;
            let request_version = appended_event(&outcome).ingest_seq;
            let response = ManagedRunObserveResponse::PermissionRequested {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: request.run_id.clone(),
                session_id: retained.response.session_id.clone(),
                provider_thread_id: permission.thread_id.as_str().to_owned(),
                provider_turn_id: permission.turn_id.as_str().to_owned(),
                provider_item_id: permission.item_id.as_str().to_owned(),
                provider_request_id: permission.provider_request_id,
                request_id,
                request_version,
                event_id,
            };
            retained.active_permission = Some(RetainedPermission {
                response: response.clone(),
                native_request: permission,
            });
            Ok(ManagedObservationCommit::Complete(Box::new(response)))
        }
        CodexTurnObservation::Terminal {
            thread_id,
            turn_id,
            outcome,
        } => {
            validate_observation_identity(&retained.response, &thread_id, &turn_id)?;
            let terminal_outcome = outcome;
            let (prefix, kind) = match terminal_outcome {
                CodexTurnTerminalOutcome::Completed => (
                    "evt_codex_completed_",
                    ManagedProviderObservationKind::TurnCompleted,
                ),
                CodexTurnTerminalOutcome::Interrupted => (
                    "evt_codex_interrupted_",
                    ManagedProviderObservationKind::TurnInterrupted,
                ),
            };
            let event_id = observation_id(
                prefix,
                &[
                    request.run_id.as_str(),
                    thread_id.as_str(),
                    turn_id.as_str(),
                ],
            );
            let outcome = store
                .append_managed_provider_observation(ManagedProviderObservation {
                    run_id: request.run_id.clone(),
                    session_id: retained.response.session_id.clone(),
                    external_session_key: thread_id.as_str().to_owned(),
                    provider_turn_id: turn_id.as_str().to_owned(),
                    contract_version: "codex-app-server/0.145.0".to_owned(),
                    event_id: event_id.clone(),
                    observed_at: request.observed_at.clone(),
                    kind,
                })
                .map_err(map_observation_store_error)?;
            let event_version = appended_event(&outcome).ingest_seq;
            Ok(ManagedObservationCommit::Complete(Box::new(
                match terminal_outcome {
                    CodexTurnTerminalOutcome::Completed => {
                        ManagedRunObserveResponse::TurnCompleted {
                            protocol_version: PROTOCOL_VERSION.to_owned(),
                            run_id: request.run_id.clone(),
                            session_id: retained.response.session_id.clone(),
                            provider_thread_id: thread_id.as_str().to_owned(),
                            provider_turn_id: turn_id.as_str().to_owned(),
                            event_id,
                            event_version,
                        }
                    }
                    CodexTurnTerminalOutcome::Interrupted => {
                        ManagedRunObserveResponse::TurnInterrupted {
                            protocol_version: PROTOCOL_VERSION.to_owned(),
                            run_id: request.run_id.clone(),
                            session_id: retained.response.session_id.clone(),
                            provider_thread_id: thread_id.as_str().to_owned(),
                            provider_turn_id: turn_id.as_str().to_owned(),
                            event_id,
                            event_version,
                        }
                    }
                },
            )))
        }
    }
}

fn validate_request(request: &ManagedRunStartRequest) -> Result<(), ManagedStartError> {
    if request.client_protocol_version != PROTOCOL_VERSION
        || request.provider != flit_protocol::ProviderKind::Codex
        || request.permission_mode != ManagedRunPermissionMode::Manual
        || request.permission_mode_version != MANUAL_MODE_VERSION
    {
        return Err(ManagedStartError::InvalidRequest);
    }
    for value in [
        &request.run_id,
        &request.session_id,
        &request.project_id,
        &request.run_created_event_id,
        &request.start_requested_event_id,
        &request.session_connected_event_id,
        &request.start_failed_event_id,
        &request.start_unknown_event_id,
    ] {
        validate_token(value, MAX_MANAGED_ID_BYTES)?;
    }
    let event_ids = [
        &request.run_created_event_id,
        &request.start_requested_event_id,
        &request.session_connected_event_id,
        &request.start_failed_event_id,
        &request.start_unknown_event_id,
    ];
    for (index, event_id) in event_ids.iter().enumerate() {
        if event_ids[..index].contains(event_id) {
            return Err(ManagedStartError::InvalidRequest);
        }
    }
    validate_text(&request.title, MAX_MANAGED_TITLE_BYTES)?;
    validate_text(&request.goal, MAX_MANAGED_GOAL_BYTES)?;
    validate_token(&request.created_at, MAX_MANAGED_TIMESTAMP_BYTES)?;
    validate_token(&request.started_at, MAX_MANAGED_TIMESTAMP_BYTES)
}

pub(crate) fn validate_observe_request(
    request: &ManagedRunObserveRequest,
) -> Result<(), ManagedStartError> {
    if request.client_protocol_version != PROTOCOL_VERSION {
        return Err(ManagedStartError::InvalidRequest);
    }
    validate_token(&request.run_id, MAX_MANAGED_ID_BYTES)?;
    validate_token(&request.observed_at, MAX_MANAGED_TIMESTAMP_BYTES)
}

fn validate_observation_identity(
    response: &ManagedRunStartResponse,
    thread_id: &CodexManagedThreadId,
    turn_id: &flit_providers::CodexManagedTurnId,
) -> Result<(), ManagedStartError> {
    if response.provider_thread_id != thread_id.as_str()
        || response.provider_turn_id != turn_id.as_str()
    {
        return Err(ManagedStartError::ProviderObservationUnknown);
    }
    Ok(())
}

fn appended_event(outcome: &AppendEventOutcome) -> &flit_protocol::EventEnvelope {
    match outcome {
        AppendEventOutcome::Inserted(event) | AppendEventOutcome::Duplicate(event) => event,
    }
}

fn observation_id(prefix: &str, axes: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for axis in axes {
        hasher.update(axis.len().to_be_bytes());
        hasher.update(axis.as_bytes());
    }
    format!("{prefix}{:x}", hasher.finalize())
}

pub(crate) fn record_observation_unknown(
    store: &mut Store,
    request: &ManagedRunObserveRequest,
    response: &ManagedRunStartResponse,
) -> Result<(), ManagedStartError> {
    store
        .reconcile_managed_session(ManagedSessionReconciliation {
            run_id: request.run_id.clone(),
            session_id: response.session_id.clone(),
            external_session_key: response.provider_thread_id.clone(),
            state: ManagedReconciliationState::Unknown,
            latest_turn_id: Some(response.provider_turn_id.clone()),
            contract_version: "codex-app-server/0.145.0".to_owned(),
            observed_at: request.observed_at.clone(),
            gap_event_id: observation_id(
                "evt_codex_observation_unknown_",
                &[
                    request.run_id.as_str(),
                    response.provider_thread_id.as_str(),
                    response.provider_turn_id.as_str(),
                    request.observed_at.as_str(),
                ],
            ),
            terminal_event_id: None,
        })
        .map(|_| ())
        .map_err(|_| ManagedStartError::StorageUnavailable)
}

fn validate_project_identity(project: &flit_store::Project) -> Result<(), ManagedStartError> {
    let inspection = flit_store::ProjectDirectoryInspection::inspect(&project.canonical_path)
        .map_err(|_| ManagedStartError::ProjectIdentityMismatch)?;
    if inspection.identity.canonical_path != project.canonical_path
        || project.filesystem_id.as_deref() != Some(inspection.identity.filesystem_id.as_str())
    {
        return Err(ManagedStartError::ProjectIdentityMismatch);
    }
    Ok(())
}

fn validate_token(value: &str, max_bytes: usize) -> Result<(), ManagedStartError> {
    validate_text(value, max_bytes)?;
    if value.chars().any(char::is_control) {
        return Err(ManagedStartError::InvalidRequest);
    }
    Ok(())
}

fn validate_text(value: &str, max_bytes: usize) -> Result<(), ManagedStartError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(ManagedStartError::InvalidRequest);
    }
    Ok(())
}

fn request_digest(request: &ManagedRunStartRequest) -> Result<String, ManagedStartError> {
    serde_json::to_vec(request)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|_| ManagedStartError::InvalidRequest)
}

fn session_fingerprint(profile: &ProviderFingerprint) -> String {
    let axes = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        profile.canonical_executable.display(),
        profile.executable_version,
        profile.executable_sha256,
        profile.combined_schema_sha256,
        profile.v2_schema_sha256,
        profile.method_allowlist_sha256,
        profile.fixture_sha256,
        profile.smoke_run_id,
    );
    sha256_hex(axes.as_bytes())
}

fn managed_capabilities(
    profile: &ProviderFingerprint,
) -> Result<Map<String, Value>, ManagedStartError> {
    let snapshot = classify_codex(profile);
    if snapshot.compatibility != ProviderCompatibility::Supported
        || snapshot.status(ProviderCapability::Launch) != CapabilityStatus::Supported
        || snapshot.status(ProviderCapability::PermissionModeConfigure)
            != CapabilityStatus::Supported
        || snapshot.status(ProviderCapability::PermissionRespond) != CapabilityStatus::Unsupported
    {
        return Err(ManagedStartError::ProviderUnavailable);
    }
    let mut capabilities = Map::new();
    for entry in snapshot.capabilities {
        let capability = serde_json::to_value(super::protocol_capability(entry.capability))
            .expect("fixed protocol capability must serialize")
            .as_str()
            .expect("fixed protocol capability must serialize as a string")
            .to_owned();
        let status = serde_json::to_value(super::protocol_capability_status(entry.status))
            .expect("fixed capability status must serialize");
        capabilities.insert(capability, status);
    }
    Ok(capabilities)
}

fn contract_version(profile: &ProviderFingerprint) -> String {
    format!("codex-app-server/{}", profile.executable_version)
}

fn fail_unstarted_run(
    store: &mut Store,
    request: &ManagedRunStartRequest,
    reason: &str,
    contract_version: &str,
) -> Result<(), ManagedStartError> {
    store
        .fail_managed_run_start(ManagedRunStartFailure {
            run_id: request.run_id.clone(),
            reason: reason.to_owned(),
            contract_version: contract_version.to_owned(),
            failed_at: request.started_at.clone(),
            failed_event_id: request.start_failed_event_id.clone(),
        })
        .map(|_| ())
        .map_err(|_| ManagedStartError::StorageUnavailable)
}

fn record_start_unknown(
    store: &mut Store,
    request: &ManagedRunStartRequest,
    thread_id: &CodexManagedThreadId,
    profile: &ProviderFingerprint,
) -> Result<(), ManagedStartError> {
    store
        .reconcile_managed_session(ManagedSessionReconciliation {
            run_id: request.run_id.clone(),
            session_id: request.session_id.clone(),
            external_session_key: thread_id.as_str().to_owned(),
            state: ManagedReconciliationState::Unknown,
            latest_turn_id: None,
            contract_version: contract_version(profile),
            observed_at: request.started_at.clone(),
            gap_event_id: request.start_unknown_event_id.clone(),
            terminal_event_id: None,
        })
        .map(|_| ())
        .map_err(|_| ManagedStartError::StorageUnavailable)
}

fn map_intent_error(error: StoreError) -> ManagedStartError {
    match error {
        StoreError::MissingProject { .. } => ManagedStartError::ProjectNotFound,
        StoreError::UntrustedProject { .. } | StoreError::ArchivedProject { .. } => {
            ManagedStartError::ProjectNotTrusted
        }
        StoreError::ManagedRunIdentityConflict { .. }
        | StoreError::ManagedRunAlreadyStarted { .. }
        | StoreError::ManagedRunTerminalConflict { .. } => ManagedStartError::RunConflict,
        StoreError::InvalidManagedRunIntent { .. } => ManagedStartError::InvalidRequest,
        _ => ManagedStartError::StorageUnavailable,
    }
}

fn map_observation_store_error(error: StoreError) -> ManagedStartError {
    match error {
        StoreError::InvalidManagedProviderObservation { .. } => {
            ManagedStartError::ProviderObservationUnknown
        }
        _ => ManagedStartError::StorageUnavailable,
    }
}

pub(crate) fn map_permission_store_error(error: StoreError) -> ManagedStartError {
    match error {
        StoreError::InvalidManagedPermissionResponse { .. } => ManagedStartError::InvalidRequest,
        StoreError::ManagedPermissionRequestStale { .. }
        | StoreError::ManagedPermissionRequestMismatch { .. }
        | StoreError::ManagedSessionNotLive { .. } => ManagedStartError::PermissionRequestStale,
        StoreError::ManagedPermissionResponseConflict { .. }
        | StoreError::ManagedPermissionResponseNotSubmitted { .. }
        | StoreError::EventIdentityConflict { .. } => ManagedStartError::PermissionResponseConflict,
        _ => ManagedStartError::StorageUnavailable,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    use flit_providers::{
        CodexManagedItemId, CodexManagedTurnId, CodexPermissionRequest, CodexStartedThread,
        validated_codex_0_145_0_fingerprint,
    };
    use flit_store::{
        InitialManagedSessionConnection, ManagedRunIntent, ProjectRegistration,
        ProjectRegistrationOutcome, ProjectTrustConfirmation,
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
    const CREATED_AT: &str = "2026-07-27T12:00:00Z";
    const STARTED_AT: &str = "2026-07-27T12:00:01Z";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-managed-start-{label}-{}-{nonce}",
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

    #[derive(Clone, Copy)]
    enum FakeBehavior {
        Success,
        ConnectFailure,
        ManualFailure,
        ManualUnknown,
        TurnFailure,
        CleanupFailure,
        CommandThenPermission,
        TerminalCompleted,
        ObservationFailure,
        WrongObservationIdentity,
        OversizedObservationTimestamp,
    }

    #[derive(Default)]
    struct FakeCalls {
        connects: usize,
        manual_starts: usize,
        turns: usize,
        deletes: usize,
        observations: usize,
    }

    struct FakeConnector {
        behavior: FakeBehavior,
        calls: Arc<Mutex<FakeCalls>>,
        thread_id: String,
        replace_project_on_connect: Option<PathBuf>,
    }

    struct FakeRuntime {
        behavior: FakeBehavior,
        calls: Arc<Mutex<FakeCalls>>,
        profile: ProviderFingerprint,
        thread_id: String,
    }

    impl ManagedCodexConnector for FakeConnector {
        fn connect(
            &self,
            _path_environment: Option<&OsStr>,
        ) -> Result<Box<dyn ManagedCodexRuntime>, ()> {
            self.calls.lock().expect("calls").connects += 1;
            if let Some(project) = &self.replace_project_on_connect {
                let moved = project.with_extension("replaced-during-connect");
                fs::rename(project, moved).expect("replace Project during connect");
                fs::create_dir(project).expect("replacement Project during connect");
            }
            if matches!(self.behavior, FakeBehavior::ConnectFailure) {
                return Err(());
            }
            Ok(Box::new(FakeRuntime {
                behavior: self.behavior,
                calls: Arc::clone(&self.calls),
                profile: validated_codex_0_145_0_fingerprint(),
                thread_id: self.thread_id.clone(),
            }))
        }
    }

    impl ManagedCodexRuntime for FakeRuntime {
        fn validated_profile(&self) -> Option<&ProviderFingerprint> {
            Some(&self.profile)
        }

        fn start_manual(
            &mut self,
            cwd: &Path,
        ) -> Result<CodexManualStartedThread, ProviderStartAttemptError> {
            self.calls.lock().expect("calls").manual_starts += 1;
            if matches!(self.behavior, FakeBehavior::ManualFailure) {
                return Err(ProviderStartAttemptError::Rejected);
            }
            if matches!(self.behavior, FakeBehavior::ManualUnknown) {
                return Err(ProviderStartAttemptError::Unknown);
            }
            Ok(CodexManualStartedThread {
                thread: CodexStartedThread {
                    thread_id: CodexManagedThreadId::new(self.thread_id.clone())
                        .expect("thread ID"),
                    canonical_cwd: cwd.to_owned(),
                },
                provider_configuration: MANUAL_PROVIDER_CONFIGURATION,
            })
        }

        fn start_turn(
            &mut self,
            thread_id: &CodexManagedThreadId,
            _prompt: &str,
        ) -> Result<CodexStartedTurn, ()> {
            self.calls.lock().expect("calls").turns += 1;
            if matches!(self.behavior, FakeBehavior::TurnFailure) {
                return Err(());
            }
            Ok(CodexStartedTurn {
                thread_id: thread_id.clone(),
                turn_id: CodexManagedTurnId::new("turn-1").expect("turn ID"),
            })
        }

        fn wait_for_turn_observation(&mut self) -> Result<CodexTurnObservation, ()> {
            let mut calls = self.calls.lock().expect("calls");
            calls.observations += 1;
            let observation_index = calls.observations;
            drop(calls);
            match self.behavior {
                FakeBehavior::CommandThenPermission if observation_index == 1 => {
                    Ok(CodexTurnObservation::CommandStarted {
                        thread_id: CodexManagedThreadId::new(self.thread_id.clone())
                            .expect("thread ID"),
                        turn_id: CodexManagedTurnId::new("turn-1").expect("turn ID"),
                        item_id: CodexManagedItemId::new("command-item").expect("item ID"),
                    })
                }
                FakeBehavior::CommandThenPermission => Ok(
                    CodexTurnObservation::PermissionRequested(CodexPermissionRequest {
                        provider_request_id: 0,
                        thread_id: CodexManagedThreadId::new(self.thread_id.clone())
                            .expect("thread ID"),
                        turn_id: CodexManagedTurnId::new("turn-1").expect("turn ID"),
                        item_id: CodexManagedItemId::new("permission-item").expect("item ID"),
                        started_at_ms: 17,
                    }),
                ),
                FakeBehavior::TerminalCompleted => Ok(CodexTurnObservation::Terminal {
                    thread_id: CodexManagedThreadId::new(self.thread_id.clone())
                        .expect("thread ID"),
                    turn_id: CodexManagedTurnId::new("turn-1").expect("turn ID"),
                    outcome: CodexTurnTerminalOutcome::Completed,
                }),
                FakeBehavior::WrongObservationIdentity => Ok(
                    CodexTurnObservation::PermissionRequested(CodexPermissionRequest {
                        provider_request_id: 0,
                        thread_id: CodexManagedThreadId::new("wrong-thread").expect("thread ID"),
                        turn_id: CodexManagedTurnId::new("turn-1").expect("turn ID"),
                        item_id: CodexManagedItemId::new("permission-item").expect("item ID"),
                        started_at_ms: 17,
                    }),
                ),
                FakeBehavior::OversizedObservationTimestamp => Ok(
                    CodexTurnObservation::PermissionRequested(CodexPermissionRequest {
                        provider_request_id: 0,
                        thread_id: CodexManagedThreadId::new(self.thread_id.clone())
                            .expect("thread ID"),
                        turn_id: CodexManagedTurnId::new("turn-1").expect("turn ID"),
                        item_id: CodexManagedItemId::new("permission-item").expect("item ID"),
                        started_at_ms: flit_protocol::MAX_JSON_SAFE_INTEGER + 1,
                    }),
                ),
                FakeBehavior::ObservationFailure => Err(()),
                FakeBehavior::Success
                | FakeBehavior::ConnectFailure
                | FakeBehavior::ManualFailure
                | FakeBehavior::ManualUnknown
                | FakeBehavior::TurnFailure
                | FakeBehavior::CleanupFailure => Err(()),
            }
        }

        fn respond_to_file_change_permission(
            &mut self,
            request: &CodexPermissionRequest,
            decision: flit_providers::CodexPermissionDecision,
        ) -> Result<flit_providers::CodexPermissionDelivery, ()> {
            Ok(flit_providers::CodexPermissionDelivery {
                provider_request_id: request.provider_request_id,
                thread_id: request.thread_id.clone(),
                turn_id: request.turn_id.clone(),
                item_id: request.item_id.clone(),
                decision,
            })
        }

        fn delete_started_thread(
            self: Box<Self>,
            _thread_id: &CodexManagedThreadId,
        ) -> Result<(), ()> {
            self.calls.lock().expect("calls").deletes += 1;
            if matches!(self.behavior, FakeBehavior::CleanupFailure) {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    fn store_and_project(label: &str, trusted: bool) -> (TestDirectory, Store, PathBuf) {
        let directory = TestDirectory::new(label);
        let selected_project = directory.0.join("project");
        fs::create_dir(&selected_project).expect("Project directory");
        let project = fs::canonicalize(&selected_project).expect("canonical Project");
        let mut store = Store::open(directory.0.join("flit.sqlite3"), CREATED_AT).expect("Store");
        assert!(matches!(
            store
                .register_project(ProjectRegistration {
                    id: "project-1".to_owned(),
                    display_name: "Project One".to_owned(),
                    selected_path: project.clone(),
                    created_at: CREATED_AT.to_owned(),
                })
                .expect("register Project"),
            ProjectRegistrationOutcome::Registered(_)
        ));
        if trusted {
            store
                .confirm_project_trust(ProjectTrustConfirmation {
                    project_id: "project-1".to_owned(),
                    selected_path: project.clone(),
                    confirmed_at: CREATED_AT.to_owned(),
                })
                .expect("trust Project");
        }
        (directory, store, project)
    }

    fn request() -> ManagedRunStartRequest {
        ManagedRunStartRequest {
            run_id: "run-1".to_owned(),
            session_id: "session-1".to_owned(),
            project_id: "project-1".to_owned(),
            title: "Implement the requested change".to_owned(),
            goal: "Update the Project and verify the result.".to_owned(),
            provider: flit_protocol::ProviderKind::Codex,
            permission_mode: ManagedRunPermissionMode::Manual,
            permission_mode_version: MANUAL_MODE_VERSION,
            created_at: CREATED_AT.to_owned(),
            started_at: STARTED_AT.to_owned(),
            run_created_event_id: "event-run-created".to_owned(),
            start_requested_event_id: "event-start-requested".to_owned(),
            session_connected_event_id: "event-session-connected".to_owned(),
            start_failed_event_id: "event-start-failed".to_owned(),
            start_unknown_event_id: "event-start-unknown".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        }
    }

    fn observe_request() -> ManagedRunObserveRequest {
        ManagedRunObserveRequest {
            run_id: "run-1".to_owned(),
            observed_at: "2026-07-27T12:00:02Z".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        }
    }

    fn connector(
        behavior: FakeBehavior,
        thread_id: &str,
    ) -> (FakeConnector, Arc<Mutex<FakeCalls>>) {
        let calls = Arc::new(Mutex::new(FakeCalls::default()));
        (
            FakeConnector {
                behavior,
                calls: Arc::clone(&calls),
                thread_id: thread_id.to_owned(),
                replace_project_on_connect: None,
            },
            calls,
        )
    }

    #[test]
    fn exact_manual_start_is_owned_idempotent_and_prompt_safe() {
        let (_directory, mut store, project) = store_and_project("success", true);
        let (connector, calls) = connector(FakeBehavior::Success, "thread-1");
        let mut runtimes = BTreeMap::new();
        let start_request = request();
        let response = start_managed_run(
            &mut store,
            &mut runtimes,
            &connector,
            None,
            start_request.clone(),
        )
        .expect("managed start");
        assert_eq!(response.run_id, "run-1");
        assert_eq!(response.session_id, "session-1");
        assert_eq!(response.provider_thread_id, "thread-1");
        assert_eq!(response.provider_turn_id, "turn-1");
        assert_eq!(
            response.provider_configuration,
            MANUAL_PROVIDER_CONFIGURATION
        );
        assert_eq!(runtimes.len(), 1);

        let run = store.managed_run("run-1").expect("Run read").expect("Run");
        assert_eq!(run.started_at.as_deref(), Some(STARTED_AT));
        assert_eq!(run.ended_at, None);
        assert_eq!(run.goal.as_deref(), Some(start_request.goal.as_str()));
        let start_request_json = serde_json::to_string(&run.start_request).expect("start request");
        assert!(!start_request_json.contains(&start_request.goal));
        assert_eq!(
            run.start_request["goal_sha256"],
            sha256_hex(start_request.goal.as_bytes())
        );
        let intent_events = store
            .run_events_through("run-1", 0, 2, 10)
            .expect("intent events");
        let intent_event_json =
            serde_json::to_string(&intent_events.events).expect("intent event JSON");
        assert!(!intent_event_json.contains(&start_request.goal));
        assert_eq!(
            intent_events.events[0].payload["goal_sha256"],
            sha256_hex(start_request.goal.as_bytes())
        );
        let session = store
            .managed_session("session-1")
            .expect("session read")
            .expect("session");
        assert_eq!(session.external_session_key, "thread-1");
        assert_eq!(session.cwd, project);
        assert_eq!(
            session.capabilities["permission_respond"],
            Value::String("unsupported".to_owned())
        );
        assert_eq!(session.capabilities.len(), ProviderCapability::ALL.len());
        assert_eq!(
            session.capabilities["permission_detect"],
            Value::String("degraded".to_owned())
        );

        assert_eq!(
            start_managed_run(
                &mut store,
                &mut runtimes,
                &connector,
                None,
                start_request.clone(),
            )
            .expect("exact duplicate"),
            response
        );
        let mut conflict = start_request;
        conflict.title.push_str(" conflict");
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, conflict),
            Err(ManagedStartError::RunConflict)
        );
        let calls = calls.lock().expect("calls");
        assert_eq!(calls.connects, 1);
        assert_eq!(calls.manual_starts, 1);
        assert_eq!(calls.turns, 1);
        assert_eq!(calls.deletes, 0);
    }

    #[test]
    fn prevalidation_rejects_before_provider_side_effects() {
        let (_directory, mut store, project) = store_and_project("prevalidation", false);
        let (connector, calls) = connector(FakeBehavior::Success, "thread-1");
        let mut runtimes = BTreeMap::new();
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, request(),),
            Err(ManagedStartError::ProjectNotTrusted)
        );

        store
            .confirm_project_trust(ProjectTrustConfirmation {
                project_id: "project-1".to_owned(),
                selected_path: project.clone(),
                confirmed_at: CREATED_AT.to_owned(),
            })
            .expect("trust Project");
        let moved = project.with_extension("moved");
        fs::rename(&project, &moved).expect("move Project");
        fs::create_dir(&project).expect("replace Project");
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, request(),),
            Err(ManagedStartError::ProjectIdentityMismatch)
        );

        let mut automatic = request();
        automatic.permission_mode = ManagedRunPermissionMode::ProviderAuto;
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, automatic,),
            Err(ManagedStartError::InvalidRequest)
        );
        assert_eq!(calls.lock().expect("calls").connects, 0);
        assert!(store.managed_run("run-1").expect("Run read").is_none());
    }

    #[test]
    fn provider_failures_before_session_ownership_are_durable_and_not_retried() {
        for (label, behavior, expected) in [
            (
                "connect-failure",
                FakeBehavior::ConnectFailure,
                ManagedStartError::ProviderUnavailable,
            ),
            (
                "manual-failure",
                FakeBehavior::ManualFailure,
                ManagedStartError::ProviderStartFailed,
            ),
        ] {
            let (_directory, mut store, _project) = store_and_project(label, true);
            let (connector, calls) = connector(behavior, "thread-1");
            let mut runtimes = BTreeMap::new();
            let start_request = request();
            assert_eq!(
                start_managed_run(
                    &mut store,
                    &mut runtimes,
                    &connector,
                    None,
                    start_request.clone(),
                ),
                Err(expected)
            );
            let run = store.managed_run("run-1").expect("Run read").expect("Run");
            assert_eq!(run.started_at, None);
            assert_eq!(run.ended_at.as_deref(), Some(STARTED_AT));
            assert_eq!(
                start_managed_run(&mut store, &mut runtimes, &connector, None, start_request,),
                Err(ManagedStartError::ProviderStartFailed)
            );
            assert_eq!(calls.lock().expect("calls").connects, 1);
        }
    }

    #[test]
    fn project_identity_is_rechecked_after_provider_probe_before_thread_start() {
        let (_directory, mut store, project) = store_and_project("identity-race", true);
        let (mut connector, calls) = connector(FakeBehavior::Success, "thread-1");
        connector.replace_project_on_connect = Some(project);
        let mut runtimes = BTreeMap::new();
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, request(),),
            Err(ManagedStartError::ProjectIdentityMismatch)
        );
        let run = store.managed_run("run-1").expect("Run read").expect("Run");
        assert_eq!(run.started_at, None);
        assert_eq!(run.ended_at.as_deref(), Some(STARTED_AT));
        let calls = calls.lock().expect("calls");
        assert_eq!(calls.connects, 1);
        assert_eq!(calls.manual_starts, 0);
    }

    #[test]
    fn ambiguous_manual_start_is_not_terminalized_or_retried() {
        let (_directory, mut store, _project) = store_and_project("manual-unknown", true);
        let (connector, calls) = connector(FakeBehavior::ManualUnknown, "thread-unknown");
        let mut runtimes = BTreeMap::new();
        let start_request = request();
        assert_eq!(
            start_managed_run(
                &mut store,
                &mut runtimes,
                &connector,
                None,
                start_request.clone(),
            ),
            Err(ManagedStartError::ProviderStartUnknown)
        );
        let run = store.managed_run("run-1").expect("Run read").expect("Run");
        assert_eq!(run.started_at, None);
        assert_eq!(run.ended_at, None);
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, start_request,),
            Err(ManagedStartError::ProviderStartUnknown)
        );
        assert_eq!(calls.lock().expect("calls").connects, 1);
    }

    #[test]
    fn turn_start_uncertainty_is_durable_and_never_retried() {
        let (_directory, mut store, _project) = store_and_project("turn-unknown", true);
        let (connector, calls) = connector(FakeBehavior::TurnFailure, "thread-1");
        let mut runtimes = BTreeMap::new();
        let start_request = request();
        assert_eq!(
            start_managed_run(
                &mut store,
                &mut runtimes,
                &connector,
                None,
                start_request.clone(),
            ),
            Err(ManagedStartError::ProviderStartUnknown)
        );
        let run = store.managed_run("run-1").expect("Run read").expect("Run");
        assert_eq!(run.started_at.as_deref(), Some(STARTED_AT));
        assert_eq!(run.ended_at, None);
        assert!(runtimes.is_empty());
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, start_request,),
            Err(ManagedStartError::ProviderStartUnknown)
        );
        let calls = calls.lock().expect("calls");
        assert_eq!(calls.connects, 1);
        assert_eq!(calls.turns, 1);
    }

    fn create_claimed_thread(store: &mut Store, project: &Path, session_id: &str, thread_id: &str) {
        let intent = ManagedRunIntent {
            id: "prior-run".to_owned(),
            project_id: "project-1".to_owned(),
            title: "Prior Run".to_owned(),
            goal: Some("Prior goal".to_owned()),
            start_request: Map::new(),
            baseline_head: None,
            created_at: CREATED_AT.to_owned(),
            run_created_event_id: "prior-created".to_owned(),
            start_requested_event_id: "prior-start-requested".to_owned(),
        };
        store
            .create_managed_run_intent(intent)
            .expect("prior Run intent");
        store
            .connect_initial_managed_session(InitialManagedSessionConnection {
                id: session_id.to_owned(),
                run_id: "prior-run".to_owned(),
                external_session_key: thread_id.to_owned(),
                session_fingerprint: "prior-fingerprint".to_owned(),
                executable_path: Some(PathBuf::from("/private/tmp/codex")),
                executable_version: Some("0.145.0".to_owned()),
                cwd: project.to_owned(),
                capabilities: Map::new(),
                contract_version: "codex-app-server/0.145.0".to_owned(),
                started_at: STARTED_AT.to_owned(),
                connected_event_id: "prior-session-connected".to_owned(),
            })
            .expect("prior session");
    }

    #[test]
    fn failed_session_composition_cleans_before_terminal_failure() {
        for (label, behavior, expected, terminal) in [
            (
                "cleanup-success",
                FakeBehavior::Success,
                ManagedStartError::ProviderStartFailed,
                true,
            ),
            (
                "cleanup-unknown",
                FakeBehavior::CleanupFailure,
                ManagedStartError::ProviderStartUnknown,
                false,
            ),
        ] {
            let (_directory, mut store, project) = store_and_project(label, true);
            create_claimed_thread(&mut store, &project, "session-1", "prior-thread");
            let (connector, calls) = connector(behavior, "new-thread");
            let mut runtimes = BTreeMap::new();
            assert_eq!(
                start_managed_run(&mut store, &mut runtimes, &connector, None, request(),),
                Err(expected)
            );
            let run = store.managed_run("run-1").expect("Run read").expect("Run");
            assert_eq!(run.started_at, None);
            assert_eq!(run.ended_at.is_some(), terminal);
            let calls = calls.lock().expect("calls");
            assert_eq!(calls.manual_starts, 1);
            assert_eq!(calls.turns, 0);
            assert_eq!(calls.deletes, 1);
        }
    }

    #[test]
    fn provider_identity_collision_is_unknown_and_never_deleted() {
        let (_directory, mut store, project) =
            store_and_project("provider-identity-collision", true);
        create_claimed_thread(
            &mut store,
            &project,
            "prior-session",
            "already-owned-thread",
        );
        let (connector, calls) = connector(FakeBehavior::Success, "already-owned-thread");
        let mut runtimes = BTreeMap::new();
        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, request(),),
            Err(ManagedStartError::ProviderStartUnknown)
        );
        let run = store.managed_run("run-1").expect("Run read").expect("Run");
        assert_eq!(run.started_at, None);
        assert_eq!(run.ended_at, None);
        let calls = calls.lock().expect("calls");
        assert_eq!(calls.manual_starts, 1);
        assert_eq!(calls.turns, 0);
        assert_eq!(calls.deletes, 0);
    }

    #[test]
    fn combined_session_and_provider_identity_collision_is_unknown_and_never_deleted() {
        let (_directory, mut store, project) =
            store_and_project("combined-provider-identity-collision", true);
        create_claimed_thread(&mut store, &project, "session-1", "already-owned-thread");
        let (connector, calls) = connector(FakeBehavior::Success, "already-owned-thread");
        let mut runtimes = BTreeMap::new();

        assert_eq!(
            start_managed_run(&mut store, &mut runtimes, &connector, None, request(),),
            Err(ManagedStartError::ProviderStartUnknown)
        );
        let prior_session = store
            .managed_session("session-1")
            .expect("prior session read")
            .expect("prior session");
        assert_eq!(prior_session.run_id, "prior-run");
        assert_eq!(prior_session.external_session_key, "already-owned-thread");
        let calls = calls.lock().expect("calls");
        assert_eq!(calls.manual_starts, 1);
        assert_eq!(calls.turns, 0);
        assert_eq!(calls.deletes, 0);
    }

    #[test]
    fn command_then_permission_is_durable_content_safe_and_cached_while_open() {
        let (_directory, mut store, _project) = store_and_project("observe-permission", true);
        let (connector, calls) = connector(FakeBehavior::CommandThenPermission, "thread-1");
        let mut runtimes = BTreeMap::new();
        start_managed_run(&mut store, &mut runtimes, &connector, None, request())
            .expect("managed start");

        let permission =
            observe_managed_run(&mut store, &mut runtimes, observe_request()).expect("permission");
        let ManagedRunObserveResponse::PermissionRequested {
            ref provider_thread_id,
            ref provider_turn_id,
            ref provider_item_id,
            provider_request_id,
            request_version,
            ref request_id,
            ref event_id,
            ..
        } = permission
        else {
            panic!("expected permission response");
        };
        assert_eq!(provider_thread_id, "thread-1");
        assert_eq!(provider_turn_id, "turn-1");
        assert_eq!(provider_item_id, "permission-item");
        assert_eq!(provider_request_id, 0);
        assert_eq!(request_version, 5);
        assert!(request_id.starts_with("req_codex_"));
        assert!(event_id.starts_with("evt_codex_permission_"));

        let events = store
            .run_events_through("run-1", 0, request_version, 10)
            .expect("Run events");
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
                "command.started",
                "permission.requested",
            ]
        );
        let rendered = serde_json::to_string(&events.events).expect("event JSON");
        for raw in [
            "sensitive reason",
            "secret command",
            "/private/secret",
            "Update the Project and verify the result.",
        ] {
            assert!(!rendered.contains(raw));
        }

        let mut retry = observe_request();
        retry.observed_at = "2026-07-27T12:00:03Z".to_owned();
        assert_eq!(
            observe_managed_run(&mut store, &mut runtimes, retry).expect("cached permission"),
            permission
        );
        assert_eq!(calls.lock().expect("calls").observations, 2);
        assert_eq!(runtimes.len(), 1);
    }

    #[test]
    fn terminal_and_unknown_observations_close_or_degrade_without_false_permission() {
        for (label, behavior, expected_terminal, expected_error) in [
            (
                "observe-terminal",
                FakeBehavior::TerminalCompleted,
                true,
                None,
            ),
            (
                "observe-failure",
                FakeBehavior::ObservationFailure,
                false,
                Some(ManagedStartError::ProviderObservationUnknown),
            ),
            (
                "observe-wrong-identity",
                FakeBehavior::WrongObservationIdentity,
                false,
                Some(ManagedStartError::ProviderObservationUnknown),
            ),
            (
                "observe-oversized-timestamp",
                FakeBehavior::OversizedObservationTimestamp,
                false,
                Some(ManagedStartError::ProviderObservationUnknown),
            ),
        ] {
            let (_directory, mut store, _project) = store_and_project(label, true);
            let (connector, _calls) = connector(behavior, "thread-1");
            let mut runtimes = BTreeMap::new();
            start_managed_run(&mut store, &mut runtimes, &connector, None, request())
                .expect("managed start");
            let observed = observe_managed_run(&mut store, &mut runtimes, observe_request());
            if expected_terminal {
                assert!(matches!(
                    observed,
                    Ok(ManagedRunObserveResponse::TurnCompleted { .. })
                ));
                assert!(
                    store
                        .managed_run("run-1")
                        .expect("Run")
                        .expect("Run")
                        .ended_at
                        .is_some()
                );
            } else {
                assert_eq!(observed, Err(expected_error.expect("expected error")));
                let events = store
                    .run_events_through(
                        "run-1",
                        0,
                        store.latest_ingest_seq().expect("latest cursor"),
                        10,
                    )
                    .expect("unknown events");
                assert_eq!(
                    events.events.last().expect("gap").event_type,
                    "diagnostic.sequence_gap"
                );
                assert!(
                    events
                        .events
                        .iter()
                        .all(|event| event.event_type != "permission.requested")
                );
            }
            assert!(runtimes.is_empty());
        }
    }
}
