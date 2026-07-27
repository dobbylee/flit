use std::{error::Error, fmt};

use crate::{
    permission_mode::{PermissionMode, PermissionModeSnapshot},
    request::RequestId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOutcomeValue(String);

impl ProviderOutcomeValue {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderOutcomeValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ProviderOutcomeValueError::BlankValue);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderOutcomeValueError {
    BlankValue,
    InvalidRequestVersion,
}

impl fmt::Display for ProviderOutcomeValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankValue => formatter.write_str("provider outcome value must not be blank"),
            Self::InvalidRequestVersion => {
                formatter.write_str("provider outcome request version must be greater than zero")
            }
        }
    }
}

impl Error for ProviderOutcomeValueError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderDecision {
    Allowed,
    Denied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderTerminalOutcome {
    RequestResolved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOutcomeRequestContext {
    session_key: ProviderOutcomeValue,
    request_id: RequestId,
    request_version: u64,
    permission_mode: PermissionModeSnapshot,
}

impl ProviderOutcomeRequestContext {
    pub fn new(
        session_key: ProviderOutcomeValue,
        request_id: RequestId,
        request_version: u64,
        permission_mode: PermissionModeSnapshot,
    ) -> Result<Self, ProviderOutcomeValueError> {
        if request_version == 0 {
            return Err(ProviderOutcomeValueError::InvalidRequestVersion);
        }
        Ok(Self {
            session_key,
            request_id,
            request_version,
            permission_mode,
        })
    }

    #[must_use]
    pub const fn permission_mode(&self) -> &PermissionModeSnapshot {
        &self.permission_mode
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderOutcomeObservation {
    pub session_key: Option<ProviderOutcomeValue>,
    pub request_id: Option<RequestId>,
    pub request_version: Option<u64>,
    pub decision_id: Option<ProviderOutcomeValue>,
    pub decision: Option<ProviderDecision>,
    pub terminal_outcome: Option<ProviderTerminalOutcome>,
    pub captured_at_ms: Option<u64>,
    pub evidence_id: Option<ProviderOutcomeValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedProviderOutcome {
    pub session_key: ProviderOutcomeValue,
    pub request_id: RequestId,
    pub request_version: u64,
    pub decision_id: ProviderOutcomeValue,
    pub decision: ProviderDecision,
    pub terminal_outcome: ProviderTerminalOutcome,
    pub captured_at_ms: u64,
    pub evidence_id: ProviderOutcomeValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderOutcomeClassification {
    Resolved(VerifiedProviderOutcome),
    OutcomeUnknown {
        missing: Vec<MissingProviderOutcomeField>,
    },
    UnboundSessionCapabilityDegrade,
    AuditOnly(ProviderOutcomeAuditReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingProviderOutcomeField {
    RequestVersion,
    SessionKey,
    DecisionId,
    Decision,
    TerminalOutcome,
    CapturedAt,
    EvidenceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderOutcomeAuditReason {
    RequestModeDoesNotAllowProviderOutcome,
    SessionMismatch,
    RequestMismatch,
    RequestVersionMismatch,
}

#[must_use]
pub fn classify_provider_outcome(
    context: &ProviderOutcomeRequestContext,
    observation: &ProviderOutcomeObservation,
) -> ProviderOutcomeClassification {
    if !matches!(context.permission_mode.mode(), PermissionMode::ProviderAuto) {
        return ProviderOutcomeClassification::AuditOnly(
            ProviderOutcomeAuditReason::RequestModeDoesNotAllowProviderOutcome,
        );
    }
    if observation
        .session_key
        .as_ref()
        .is_some_and(|session_key| session_key != &context.session_key)
    {
        return ProviderOutcomeClassification::AuditOnly(
            ProviderOutcomeAuditReason::SessionMismatch,
        );
    }
    let Some(request_id) = observation.request_id.as_ref() else {
        return ProviderOutcomeClassification::UnboundSessionCapabilityDegrade;
    };
    if request_id != &context.request_id {
        return ProviderOutcomeClassification::AuditOnly(
            ProviderOutcomeAuditReason::RequestMismatch,
        );
    }
    if observation
        .request_version
        .is_some_and(|version| version != context.request_version)
    {
        return ProviderOutcomeClassification::AuditOnly(
            ProviderOutcomeAuditReason::RequestVersionMismatch,
        );
    }
    let missing = missing_fields(observation);
    if !missing.is_empty() {
        return ProviderOutcomeClassification::OutcomeUnknown { missing };
    }
    ProviderOutcomeClassification::Resolved(complete_outcome(observation))
}

fn missing_fields(observation: &ProviderOutcomeObservation) -> Vec<MissingProviderOutcomeField> {
    let mut missing = Vec::new();
    let fields = [
        (
            observation.request_version.is_none(),
            MissingProviderOutcomeField::RequestVersion,
        ),
        (
            observation.session_key.is_none(),
            MissingProviderOutcomeField::SessionKey,
        ),
        (
            observation.decision_id.is_none(),
            MissingProviderOutcomeField::DecisionId,
        ),
        (
            observation.decision.is_none(),
            MissingProviderOutcomeField::Decision,
        ),
        (
            observation.terminal_outcome.is_none(),
            MissingProviderOutcomeField::TerminalOutcome,
        ),
        (
            observation.captured_at_ms.is_none(),
            MissingProviderOutcomeField::CapturedAt,
        ),
        (
            observation.evidence_id.is_none(),
            MissingProviderOutcomeField::EvidenceId,
        ),
    ];
    for (is_missing, field) in fields {
        if is_missing {
            missing.push(field);
        }
    }
    missing
}

fn complete_outcome(observation: &ProviderOutcomeObservation) -> VerifiedProviderOutcome {
    VerifiedProviderOutcome {
        session_key: observation
            .session_key
            .clone()
            .expect("completeness checked"),
        request_id: observation.request_id.clone().expect("identity checked"),
        request_version: observation.request_version.expect("completeness checked"),
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
