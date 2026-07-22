use std::{error::Error, fmt};

use crate::{
    permission_mode::{PermissionMode, PermissionModeSnapshot},
    request::RequestId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderPolicyValue(String);

impl ProviderPolicyValue {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderPolicyValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ProviderPolicyValueError::BlankValue);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPolicyValueError {
    BlankValue,
    InvalidRequestVersion,
}

impl fmt::Display for ProviderPolicyValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankValue => formatter.write_str("provider policy value must not be blank"),
            Self::InvalidRequestVersion => {
                formatter.write_str("provider policy request version must be greater than zero")
            }
        }
    }
}

impl Error for ProviderPolicyValueError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionRisk {
    Low,
    Destructive,
    Credential,
    PublishDeploy,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeBoundary {
    InProject,
    OutOfProject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPolicyDecision {
    Allowed,
    Denied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderTerminalOutcome {
    RequestResolved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderPolicyRequestContext {
    session_key: ProviderPolicyValue,
    request_id: RequestId,
    request_version: u64,
    bound_mode: PermissionModeSnapshot,
    displayed_action_fingerprint: ProviderPolicyValue,
    displayed_scope_fingerprint: ProviderPolicyValue,
    action_risk: ActionRisk,
    scope_boundary: ScopeBoundary,
}

impl ProviderPolicyRequestContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_key: ProviderPolicyValue,
        request_id: RequestId,
        request_version: u64,
        bound_mode: PermissionModeSnapshot,
        displayed_action_fingerprint: ProviderPolicyValue,
        displayed_scope_fingerprint: ProviderPolicyValue,
        action_risk: ActionRisk,
        scope_boundary: ScopeBoundary,
    ) -> Result<Self, ProviderPolicyValueError> {
        if request_version == 0 {
            return Err(ProviderPolicyValueError::InvalidRequestVersion);
        }
        Ok(Self {
            session_key,
            request_id,
            request_version,
            bound_mode,
            displayed_action_fingerprint,
            displayed_scope_fingerprint,
            action_risk,
            scope_boundary,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderPolicyObservation {
    pub session_key: Option<ProviderPolicyValue>,
    pub request_id: Option<RequestId>,
    pub request_version: Option<u64>,
    pub action_fingerprint: Option<ProviderPolicyValue>,
    pub scope_fingerprint: Option<ProviderPolicyValue>,
    pub bound_mode: Option<PermissionModeSnapshot>,
    pub policy_source: Option<ProviderPolicyValue>,
    pub policy_version: Option<ProviderPolicyValue>,
    pub policy_fingerprint: Option<ProviderPolicyValue>,
    pub decision_id: Option<ProviderPolicyValue>,
    pub decision: Option<ProviderPolicyDecision>,
    pub terminal_outcome: Option<ProviderTerminalOutcome>,
    pub captured_at_ms: Option<u64>,
    pub evidence_id: Option<ProviderPolicyValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedProviderPolicyOutcome {
    pub session_key: ProviderPolicyValue,
    pub request_id: RequestId,
    pub request_version: u64,
    pub action_fingerprint: ProviderPolicyValue,
    pub scope_fingerprint: ProviderPolicyValue,
    pub bound_mode: PermissionModeSnapshot,
    pub policy_source: ProviderPolicyValue,
    pub policy_version: ProviderPolicyValue,
    pub policy_fingerprint: ProviderPolicyValue,
    pub decision_id: ProviderPolicyValue,
    pub decision: ProviderPolicyDecision,
    pub terminal_outcome: ProviderTerminalOutcome,
    pub captured_at_ms: u64,
    pub evidence_id: ProviderPolicyValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderPolicyClassification {
    InformationalResolve(VerifiedProviderPolicyOutcome),
    ResolvedWithPolicyViolation {
        outcome: VerifiedProviderPolicyOutcome,
        reasons: Vec<PolicyViolationReason>,
    },
    ProviderOutcomeUnknown {
        missing: Vec<MissingProviderPolicyField>,
    },
    UnboundSessionCapabilityDegrade,
    AuditOnly(ProviderPolicyAuditReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingProviderPolicyField {
    RequestVersion,
    SessionKey,
    ActionFingerprint,
    ScopeFingerprint,
    BoundMode,
    PolicySource,
    PolicyVersion,
    PolicyFingerprint,
    DecisionId,
    Decision,
    TerminalOutcome,
    CapturedAt,
    EvidenceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPolicyAuditReason {
    SessionMismatch,
    RequestMismatch,
    RequestVersionMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyViolationReason {
    ManualMode,
    UnknownMode,
    BoundModeMismatch,
    PolicyFingerprintMismatch,
    ActionFingerprintMismatch,
    ScopeFingerprintMismatch,
    DestructiveAction,
    CredentialAction,
    PublishDeployAction,
    UnknownAction,
    OutOfProjectScope,
}

#[must_use]
pub fn classify_provider_policy_outcome(
    context: &ProviderPolicyRequestContext,
    observation: &ProviderPolicyObservation,
) -> ProviderPolicyClassification {
    if observation
        .session_key
        .as_ref()
        .is_some_and(|session_key| session_key != &context.session_key)
    {
        return ProviderPolicyClassification::AuditOnly(ProviderPolicyAuditReason::SessionMismatch);
    }
    let Some(request_id) = observation.request_id.as_ref() else {
        return ProviderPolicyClassification::UnboundSessionCapabilityDegrade;
    };
    if request_id != &context.request_id {
        return ProviderPolicyClassification::AuditOnly(ProviderPolicyAuditReason::RequestMismatch);
    }
    if observation
        .request_version
        .is_some_and(|version| version != context.request_version)
    {
        return ProviderPolicyClassification::AuditOnly(
            ProviderPolicyAuditReason::RequestVersionMismatch,
        );
    }
    let missing = missing_fields(observation);
    if !missing.is_empty() {
        return ProviderPolicyClassification::ProviderOutcomeUnknown { missing };
    }
    let outcome = complete_outcome(observation);
    let reasons = violation_reasons(context, &outcome);
    if reasons.is_empty() {
        ProviderPolicyClassification::InformationalResolve(outcome)
    } else {
        ProviderPolicyClassification::ResolvedWithPolicyViolation { outcome, reasons }
    }
}

fn missing_fields(observation: &ProviderPolicyObservation) -> Vec<MissingProviderPolicyField> {
    let mut missing = Vec::new();
    let fields = [
        (
            observation.request_version.is_none(),
            MissingProviderPolicyField::RequestVersion,
        ),
        (
            observation.session_key.is_none(),
            MissingProviderPolicyField::SessionKey,
        ),
        (
            observation.action_fingerprint.is_none(),
            MissingProviderPolicyField::ActionFingerprint,
        ),
        (
            observation.scope_fingerprint.is_none(),
            MissingProviderPolicyField::ScopeFingerprint,
        ),
        (
            observation.bound_mode.is_none(),
            MissingProviderPolicyField::BoundMode,
        ),
        (
            observation.policy_source.is_none(),
            MissingProviderPolicyField::PolicySource,
        ),
        (
            observation.policy_version.is_none(),
            MissingProviderPolicyField::PolicyVersion,
        ),
        (
            observation.policy_fingerprint.is_none(),
            MissingProviderPolicyField::PolicyFingerprint,
        ),
        (
            observation.decision_id.is_none(),
            MissingProviderPolicyField::DecisionId,
        ),
        (
            observation.decision.is_none(),
            MissingProviderPolicyField::Decision,
        ),
        (
            observation.terminal_outcome.is_none(),
            MissingProviderPolicyField::TerminalOutcome,
        ),
        (
            observation.captured_at_ms.is_none(),
            MissingProviderPolicyField::CapturedAt,
        ),
        (
            observation.evidence_id.is_none(),
            MissingProviderPolicyField::EvidenceId,
        ),
    ];
    for (is_missing, field) in fields {
        if is_missing {
            missing.push(field);
        }
    }
    missing
}

fn complete_outcome(observation: &ProviderPolicyObservation) -> VerifiedProviderPolicyOutcome {
    VerifiedProviderPolicyOutcome {
        session_key: observation
            .session_key
            .clone()
            .expect("completeness checked"),
        request_id: observation.request_id.clone().expect("identity checked"),
        request_version: observation.request_version.expect("completeness checked"),
        action_fingerprint: observation
            .action_fingerprint
            .clone()
            .expect("completeness checked"),
        scope_fingerprint: observation
            .scope_fingerprint
            .clone()
            .expect("completeness checked"),
        bound_mode: observation
            .bound_mode
            .clone()
            .expect("completeness checked"),
        policy_source: observation
            .policy_source
            .clone()
            .expect("completeness checked"),
        policy_version: observation
            .policy_version
            .clone()
            .expect("completeness checked"),
        policy_fingerprint: observation
            .policy_fingerprint
            .clone()
            .expect("completeness checked"),
        decision_id: observation
            .decision_id
            .clone()
            .expect("completeness checked"),
        decision: observation.decision.expect("completeness checked"),
        terminal_outcome: observation.terminal_outcome.expect("completeness checked"),
        captured_at_ms: observation.captured_at_ms.expect("completeness checked"),
        evidence_id: observation
            .evidence_id
            .clone()
            .expect("completeness checked"),
    }
}

fn violation_reasons(
    context: &ProviderPolicyRequestContext,
    outcome: &VerifiedProviderPolicyOutcome,
) -> Vec<PolicyViolationReason> {
    let mut reasons = Vec::new();
    match context.bound_mode.mode() {
        PermissionMode::Manual => reasons.push(PolicyViolationReason::ManualMode),
        PermissionMode::Unknown => reasons.push(PolicyViolationReason::UnknownMode),
        PermissionMode::ApproveForMe => {}
    }
    if outcome.bound_mode != context.bound_mode {
        reasons.push(PolicyViolationReason::BoundModeMismatch);
    }
    if context
        .bound_mode
        .policy_fingerprint()
        .map(|value| value.as_str())
        != Some(outcome.policy_fingerprint.as_str())
    {
        reasons.push(PolicyViolationReason::PolicyFingerprintMismatch);
    }
    if outcome.action_fingerprint != context.displayed_action_fingerprint {
        reasons.push(PolicyViolationReason::ActionFingerprintMismatch);
    }
    if outcome.scope_fingerprint != context.displayed_scope_fingerprint {
        reasons.push(PolicyViolationReason::ScopeFingerprintMismatch);
    }
    match context.action_risk {
        ActionRisk::Low => {}
        ActionRisk::Destructive => reasons.push(PolicyViolationReason::DestructiveAction),
        ActionRisk::Credential => reasons.push(PolicyViolationReason::CredentialAction),
        ActionRisk::PublishDeploy => reasons.push(PolicyViolationReason::PublishDeployAction),
        ActionRisk::Unknown => reasons.push(PolicyViolationReason::UnknownAction),
    }
    if context.scope_boundary == ScopeBoundary::OutOfProject {
        reasons.push(PolicyViolationReason::OutOfProjectScope);
    }
    reasons
}
