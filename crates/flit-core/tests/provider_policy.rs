use flit_core::{
    permission_mode::{PermissionMode, PermissionModeSnapshot, PolicyFingerprint},
    provider_policy::{
        ActionRisk, MissingProviderPolicyField, PolicyViolationReason, ProviderPolicyAuditReason,
        ProviderPolicyClassification, ProviderPolicyDecision, ProviderPolicyObservation,
        ProviderPolicyRequestContext, ProviderPolicyValue, ProviderPolicyValueError,
        ProviderTerminalOutcome, ScopeBoundary, classify_provider_policy_outcome,
    },
    request::RequestId,
};

fn value(input: &str) -> ProviderPolicyValue {
    ProviderPolicyValue::new(input).expect("test provider policy value must be valid")
}

fn request_id(input: &str) -> RequestId {
    RequestId::new(input).expect("test request ID must be valid")
}

fn mode(mode: PermissionMode, version: u64, fingerprint: Option<&str>) -> PermissionModeSnapshot {
    PermissionModeSnapshot::new(
        mode,
        version,
        fingerprint.map(|input| {
            PolicyFingerprint::new(input).expect("test policy fingerprint must be valid")
        }),
    )
    .expect("test mode snapshot must be valid")
}

fn context(
    bound_mode: PermissionModeSnapshot,
    risk: ActionRisk,
    scope: ScopeBoundary,
) -> ProviderPolicyRequestContext {
    ProviderPolicyRequestContext::new(
        value("session-1"),
        request_id("request-1"),
        7,
        bound_mode,
        value("action-fp"),
        value("scope-fp"),
        risk,
        scope,
    )
    .expect("test context must be valid")
}

fn complete_observation(bound_mode: PermissionModeSnapshot) -> ProviderPolicyObservation {
    ProviderPolicyObservation {
        session_key: Some(value("session-1")),
        request_id: Some(request_id("request-1")),
        request_version: Some(7),
        action_fingerprint: Some(value("action-fp")),
        scope_fingerprint: Some(value("scope-fp")),
        bound_mode: Some(bound_mode),
        policy_source: Some(value("provider-native-policy")),
        policy_version: Some(value("policy-v1")),
        policy_fingerprint: Some(value("policy-fp")),
        decision_id: Some(value("decision-1")),
        decision: Some(ProviderPolicyDecision::Allowed),
        terminal_outcome: Some(ProviderTerminalOutcome::RequestResolved),
        captured_at_ms: Some(1_000),
        evidence_id: Some(value("evidence-1")),
    }
}

fn approved_mode() -> PermissionModeSnapshot {
    mode(PermissionMode::ApproveForMe, 3, Some("policy-fp"))
}

#[test]
fn value_and_context_reject_blank_values_and_zero_request_version() {
    assert_eq!(
        ProviderPolicyValue::new("\n\t"),
        Err(ProviderPolicyValueError::BlankValue)
    );
    assert_eq!(value("value").as_str(), "value");
    assert_eq!(
        ProviderPolicyRequestContext::new(
            value("session-1"),
            request_id("request-1"),
            0,
            approved_mode(),
            value("action-fp"),
            value("scope-fp"),
            ActionRisk::Low,
            ScopeBoundary::InProject,
        ),
        Err(ProviderPolicyValueError::InvalidRequestVersion)
    );
}

#[test]
fn missing_request_identity_degrades_session_without_targeting_an_open_request() {
    let context = context(approved_mode(), ActionRisk::Low, ScopeBoundary::InProject);
    let observation = ProviderPolicyObservation {
        session_key: Some(value("session-1")),
        ..ProviderPolicyObservation::default()
    };

    assert_eq!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::UnboundSessionCapabilityDegrade
    );

    let mismatched_session = ProviderPolicyObservation {
        session_key: Some(value("session-other")),
        ..ProviderPolicyObservation::default()
    };
    assert_eq!(
        classify_provider_policy_outcome(&context, &mismatched_session),
        ProviderPolicyClassification::AuditOnly(ProviderPolicyAuditReason::SessionMismatch)
    );
}

#[test]
fn stale_request_and_session_identity_are_audit_only() {
    let context = context(approved_mode(), ActionRisk::Low, ScopeBoundary::InProject);
    let mut observation = complete_observation(approved_mode());
    observation.request_id = Some(request_id("request-old"));
    assert_eq!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::AuditOnly(ProviderPolicyAuditReason::RequestMismatch)
    );

    observation.request_id = Some(request_id("request-1"));
    observation.request_version = Some(6);
    assert_eq!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::AuditOnly(ProviderPolicyAuditReason::RequestVersionMismatch)
    );

    observation.request_version = Some(7);
    observation.session_key = Some(value("session-other"));
    assert_eq!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::AuditOnly(ProviderPolicyAuditReason::SessionMismatch)
    );
}

#[test]
fn exact_request_with_incomplete_provenance_reports_every_missing_field_in_order() {
    let context = context(approved_mode(), ActionRisk::Low, ScopeBoundary::InProject);
    let observation = ProviderPolicyObservation {
        request_id: Some(request_id("request-1")),
        ..ProviderPolicyObservation::default()
    };

    assert_eq!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::ProviderOutcomeUnknown {
            missing: vec![
                MissingProviderPolicyField::RequestVersion,
                MissingProviderPolicyField::SessionKey,
                MissingProviderPolicyField::ActionFingerprint,
                MissingProviderPolicyField::ScopeFingerprint,
                MissingProviderPolicyField::BoundMode,
                MissingProviderPolicyField::PolicySource,
                MissingProviderPolicyField::PolicyVersion,
                MissingProviderPolicyField::PolicyFingerprint,
                MissingProviderPolicyField::DecisionId,
                MissingProviderPolicyField::Decision,
                MissingProviderPolicyField::TerminalOutcome,
                MissingProviderPolicyField::CapturedAt,
                MissingProviderPolicyField::EvidenceId,
            ],
        }
    );
}

#[test]
fn exact_complete_low_risk_approve_for_me_outcome_is_informational() {
    let bound_mode = approved_mode();
    let context = context(
        bound_mode.clone(),
        ActionRisk::Low,
        ScopeBoundary::InProject,
    );
    let observation = complete_observation(bound_mode);

    let ProviderPolicyClassification::InformationalResolve(outcome) =
        classify_provider_policy_outcome(&context, &observation)
    else {
        panic!("safe exact outcome must be informational");
    };
    assert_eq!(outcome.request_id.as_str(), "request-1");
    assert_eq!(outcome.request_version, 7);
    assert_eq!(outcome.decision_id.as_str(), "decision-1");
    assert_eq!(outcome.decision, ProviderPolicyDecision::Allowed);
    assert_eq!(outcome.evidence_id.as_str(), "evidence-1");
}

#[test]
fn safe_provider_denial_preserves_the_same_informational_provenance() {
    let bound_mode = approved_mode();
    let context = context(
        bound_mode.clone(),
        ActionRisk::Low,
        ScopeBoundary::InProject,
    );
    let mut observation = complete_observation(bound_mode);
    observation.decision = Some(ProviderPolicyDecision::Denied);

    assert!(matches!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::InformationalResolve(outcome)
            if outcome.decision == ProviderPolicyDecision::Denied
    ));
}

#[test]
fn manual_and_unknown_request_modes_are_policy_violations() {
    for (bound_mode, expected) in [
        (
            mode(PermissionMode::Manual, 3, Some("policy-fp")),
            vec![PolicyViolationReason::ManualMode],
        ),
        (
            mode(PermissionMode::Unknown, 3, None),
            vec![
                PolicyViolationReason::UnknownMode,
                PolicyViolationReason::PolicyFingerprintMismatch,
            ],
        ),
    ] {
        let context = context(
            bound_mode.clone(),
            ActionRisk::Low,
            ScopeBoundary::InProject,
        );
        let observation = complete_observation(bound_mode);
        assert!(matches!(
            classify_provider_policy_outcome(&context, &observation),
            ProviderPolicyClassification::ResolvedWithPolicyViolation { reasons, .. }
                if reasons == expected
        ));
    }
}

#[test]
fn mode_policy_action_and_scope_mismatches_accumulate_deterministic_violations() {
    let context = context(approved_mode(), ActionRisk::Low, ScopeBoundary::InProject);
    let mut observation = complete_observation(mode(
        PermissionMode::ApproveForMe,
        2,
        Some("stale-policy-fp"),
    ));
    observation.policy_fingerprint = Some(value("different-policy-fp"));
    observation.action_fingerprint = Some(value("different-action"));
    observation.scope_fingerprint = Some(value("different-scope"));

    assert!(matches!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::ResolvedWithPolicyViolation { reasons, .. }
            if reasons == vec![
                PolicyViolationReason::BoundModeMismatch,
                PolicyViolationReason::PolicyFingerprintMismatch,
                PolicyViolationReason::ActionFingerprintMismatch,
                PolicyViolationReason::ScopeFingerprintMismatch,
            ]
    ));
}

#[test]
fn every_forbidden_risk_and_external_scope_produces_a_critical_violation_reason() {
    let cases = [
        (
            ActionRisk::Destructive,
            PolicyViolationReason::DestructiveAction,
        ),
        (
            ActionRisk::Credential,
            PolicyViolationReason::CredentialAction,
        ),
        (
            ActionRisk::PublishDeploy,
            PolicyViolationReason::PublishDeployAction,
        ),
        (ActionRisk::Unknown, PolicyViolationReason::UnknownAction),
    ];
    for (risk, expected) in cases {
        let bound_mode = approved_mode();
        let context = context(bound_mode.clone(), risk, ScopeBoundary::InProject);
        let observation = complete_observation(bound_mode);
        assert!(matches!(
            classify_provider_policy_outcome(&context, &observation),
            ProviderPolicyClassification::ResolvedWithPolicyViolation { reasons, .. }
                if reasons == vec![expected]
        ));
    }

    let bound_mode = approved_mode();
    let context = context(
        bound_mode.clone(),
        ActionRisk::Low,
        ScopeBoundary::OutOfProject,
    );
    let observation = complete_observation(bound_mode);
    assert!(matches!(
        classify_provider_policy_outcome(&context, &observation),
        ProviderPolicyClassification::ResolvedWithPolicyViolation { reasons, .. }
            if reasons == vec![PolicyViolationReason::OutOfProjectScope]
    ));
}
