use flit_core::{
    permission_mode::{PermissionMode, PermissionModeSnapshot, ProviderConfigurationIdentity},
    provider_outcome::{
        MissingProviderOutcomeField, ProviderDecision, ProviderOutcomeAuditReason,
        ProviderOutcomeClassification, ProviderOutcomeObservation, ProviderOutcomeRequestContext,
        ProviderOutcomeValue, ProviderOutcomeValueError, ProviderTerminalOutcome,
        classify_provider_outcome,
    },
    request::RequestId,
};

fn value(input: &str) -> ProviderOutcomeValue {
    ProviderOutcomeValue::new(input).expect("test provider outcome value must be valid")
}

fn request_id(input: &str) -> RequestId {
    RequestId::new(input).expect("test request ID must be valid")
}

fn permission_mode(mode: PermissionMode, version: u64) -> PermissionModeSnapshot {
    PermissionModeSnapshot::new(
        mode,
        version,
        (!matches!(mode, PermissionMode::Unknown)).then(|| {
            ProviderConfigurationIdentity::new("provider-config")
                .expect("test provider configuration identity must be valid")
        }),
    )
    .expect("test permission mode must be valid")
}

fn context() -> ProviderOutcomeRequestContext {
    ProviderOutcomeRequestContext::new(
        value("session-1"),
        request_id("request-1"),
        7,
        permission_mode(PermissionMode::ProviderAuto, 3),
    )
    .expect("test context must be valid")
}

fn complete_observation() -> ProviderOutcomeObservation {
    ProviderOutcomeObservation {
        session_key: Some(value("session-1")),
        request_id: Some(request_id("request-1")),
        request_version: Some(7),
        decision_id: Some(value("decision-1")),
        decision: Some(ProviderDecision::Allowed),
        terminal_outcome: Some(ProviderTerminalOutcome::RequestResolved),
        captured_at_ms: Some(1_000),
        evidence_id: Some(value("evidence-1")),
    }
}

#[test]
fn values_and_context_reject_blank_values_and_zero_request_version() {
    assert_eq!(
        ProviderOutcomeValue::new("\n\t"),
        Err(ProviderOutcomeValueError::BlankValue)
    );
    assert_eq!(value("value").as_str(), "value");
    assert_eq!(
        ProviderOutcomeRequestContext::new(
            value("session-1"),
            request_id("request-1"),
            0,
            permission_mode(PermissionMode::ProviderAuto, 3),
        ),
        Err(ProviderOutcomeValueError::InvalidRequestVersion)
    );
    assert_eq!(
        context().permission_mode(),
        &permission_mode(PermissionMode::ProviderAuto, 3)
    );
}

#[test]
fn manual_or_unknown_request_binding_never_accepts_a_provider_owned_outcome() {
    for mode in [PermissionMode::Manual, PermissionMode::Unknown] {
        let context = ProviderOutcomeRequestContext::new(
            value("session-1"),
            request_id("request-1"),
            7,
            permission_mode(mode, 9),
        )
        .expect("test context must be valid");
        assert_eq!(
            classify_provider_outcome(&context, &complete_observation()),
            ProviderOutcomeClassification::AuditOnly(
                ProviderOutcomeAuditReason::RequestModeDoesNotAllowProviderOutcome
            )
        );
    }
}

#[test]
fn missing_request_identity_degrades_without_targeting_an_open_request() {
    let observation = ProviderOutcomeObservation {
        session_key: Some(value("session-1")),
        ..ProviderOutcomeObservation::default()
    };
    assert_eq!(
        classify_provider_outcome(&context(), &observation),
        ProviderOutcomeClassification::UnboundSessionCapabilityDegrade
    );
}

#[test]
fn stale_request_and_session_identity_are_audit_only() {
    let mut observation = complete_observation();
    observation.request_id = Some(request_id("request-old"));
    assert_eq!(
        classify_provider_outcome(&context(), &observation),
        ProviderOutcomeClassification::AuditOnly(ProviderOutcomeAuditReason::RequestMismatch)
    );

    observation.request_id = Some(request_id("request-1"));
    observation.request_version = Some(6);
    assert_eq!(
        classify_provider_outcome(&context(), &observation),
        ProviderOutcomeClassification::AuditOnly(
            ProviderOutcomeAuditReason::RequestVersionMismatch
        )
    );

    observation.request_version = Some(7);
    observation.session_key = Some(value("session-other"));
    assert_eq!(
        classify_provider_outcome(&context(), &observation),
        ProviderOutcomeClassification::AuditOnly(ProviderOutcomeAuditReason::SessionMismatch)
    );
}

#[test]
fn exact_request_with_incomplete_provider_facts_reports_every_missing_field() {
    let observation = ProviderOutcomeObservation {
        request_id: Some(request_id("request-1")),
        ..ProviderOutcomeObservation::default()
    };
    assert_eq!(
        classify_provider_outcome(&context(), &observation),
        ProviderOutcomeClassification::OutcomeUnknown {
            missing: vec![
                MissingProviderOutcomeField::RequestVersion,
                MissingProviderOutcomeField::SessionKey,
                MissingProviderOutcomeField::DecisionId,
                MissingProviderOutcomeField::Decision,
                MissingProviderOutcomeField::TerminalOutcome,
                MissingProviderOutcomeField::CapturedAt,
                MissingProviderOutcomeField::EvidenceId,
            ],
        }
    );
}

#[test]
fn exact_provider_allow_and_deny_are_factual_resolutions_without_flit_policy() {
    for decision in [ProviderDecision::Allowed, ProviderDecision::Denied] {
        let mut observation = complete_observation();
        observation.decision = Some(decision);
        let ProviderOutcomeClassification::Resolved(outcome) =
            classify_provider_outcome(&context(), &observation)
        else {
            panic!("exact provider outcome must resolve");
        };
        assert_eq!(outcome.request_id.as_str(), "request-1");
        assert_eq!(outcome.request_version, 7);
        assert_eq!(outcome.decision_id.as_str(), "decision-1");
        assert_eq!(outcome.decision, decision);
        assert_eq!(outcome.evidence_id.as_str(), "evidence-1");
    }
}
