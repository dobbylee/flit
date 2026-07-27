use std::{error::Error, fmt};

use crate::provider_outcome::{
    MissingProviderOutcomeField, ProviderOutcomeClassification, ProviderOutcomeValue,
    VerifiedProviderOutcome,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Result<Self, RequestValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RequestValueError::BlankRequestId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseAttemptId(String);

impl ResponseAttemptId {
    pub fn new(value: impl Into<String>) -> Result<Self, RequestValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RequestValueError::BlankResponseAttemptId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestValueError {
    BlankRequestId,
    BlankResponseAttemptId,
}

impl fmt::Display for RequestValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankRequestId => formatter.write_str("request ID must not be blank"),
            Self::BlankResponseAttemptId => {
                formatter.write_str("response attempt ID must not be blank")
            }
        }
    }
}

impl Error for RequestValueError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    Permission,
    Question,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseAttempt {
    id: ResponseAttemptId,
    submitted_request_version: u64,
}

impl ResponseAttempt {
    #[must_use]
    pub fn id(&self) -> &ResponseAttemptId {
        &self.id
    }

    #[must_use]
    pub const fn submitted_request_version(&self) -> u64 {
        self.submitted_request_version
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOutcomeResolution {
    outcome: VerifiedProviderOutcome,
}

impl ProviderOutcomeResolution {
    #[must_use]
    pub const fn outcome(&self) -> &VerifiedProviderOutcome {
        &self.outcome
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOutcomeUnknown {
    original_request_version: u64,
    missing: Vec<MissingProviderOutcomeField>,
}

impl ProviderOutcomeUnknown {
    #[must_use]
    pub const fn original_request_version(&self) -> u64 {
        self.original_request_version
    }

    #[must_use]
    pub fn missing(&self) -> &[MissingProviderOutcomeField] {
        &self.missing
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestResolution {
    ManualResponse(ResponseAttempt),
    ProviderOutcome(Box<ProviderOutcomeResolution>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestStatus {
    Open,
    ResponsePending(ResponseAttempt),
    DeliveryUnknown(ResponseAttempt),
    Resolved(RequestResolution),
    ProviderOutcomeUnknown(ProviderOutcomeUnknown),
    Expired,
}

impl RequestStatus {
    fn active_attempt(&self) -> Option<&ResponseAttempt> {
        match self {
            Self::ResponsePending(attempt) | Self::DeliveryUnknown(attempt) => Some(attempt),
            Self::Open | Self::Resolved(_) | Self::ProviderOutcomeUnknown(_) | Self::Expired => {
                None
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestProjection {
    request_id: RequestId,
    kind: RequestKind,
    version: u64,
    last_ingest_seq: u64,
    status: RequestStatus,
    used_attempt_ids: Vec<ResponseAttemptId>,
    used_provider_decision_ids: Vec<ProviderOutcomeValue>,
}

impl RequestProjection {
    pub fn new(
        request_id: RequestId,
        kind: RequestKind,
        requested_ingest_seq: u64,
    ) -> Result<Self, RequestError> {
        if requested_ingest_seq == 0 {
            return Err(RequestError::InvalidInitialIngestSequence);
        }
        Ok(Self {
            request_id,
            kind,
            version: requested_ingest_seq,
            last_ingest_seq: requested_ingest_seq,
            status: RequestStatus::Open,
            used_attempt_ids: Vec::new(),
            used_provider_decision_ids: Vec::new(),
        })
    }

    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    #[must_use]
    pub const fn kind(&self) -> RequestKind {
        self.kind
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn last_ingest_seq(&self) -> u64 {
        self.last_ingest_seq
    }

    #[must_use]
    pub const fn status(&self) -> &RequestStatus {
        &self.status
    }

    #[must_use]
    pub fn used_attempt_ids(&self) -> &[ResponseAttemptId] {
        &self.used_attempt_ids
    }

    #[must_use]
    pub fn used_provider_decision_ids(&self) -> &[ProviderOutcomeValue] {
        &self.used_provider_decision_ids
    }

    pub fn apply(
        &mut self,
        ingest_seq: u64,
        event: RequestEvent,
    ) -> Result<RequestDisposition, RequestError> {
        if ingest_seq <= self.last_ingest_seq {
            return Err(RequestError::NonMonotonicIngestSequence {
                current: self.last_ingest_seq,
                received: ingest_seq,
            });
        }

        let disposition = self.apply_ordered(event);
        self.last_ingest_seq = ingest_seq;
        if disposition == RequestDisposition::Applied {
            self.version = ingest_seq;
        }
        debug_assert!(self.invariants_hold());
        Ok(disposition)
    }

    fn apply_ordered(&mut self, event: RequestEvent) -> RequestDisposition {
        match event {
            RequestEvent::ResponseSubmitted {
                expected_version,
                attempt_id,
            } => self.submit_response(expected_version, attempt_id),
            RequestEvent::ResponseResolved { attempt_id } => self.resolve_response(&attempt_id),
            RequestEvent::ResponseFailed { attempt_id } => self.fail_response(&attempt_id),
            RequestEvent::DeliveryUnknown { attempt_id } => self.mark_delivery_unknown(&attempt_id),
            RequestEvent::ProviderOutcomeClassified {
                request_id,
                expected_request_version,
                classification,
            } => {
                self.apply_provider_outcome(&request_id, expected_request_version, *classification)
            }
            RequestEvent::RequestExpired => self.expire(),
        }
    }

    fn submit_response(
        &mut self,
        expected_version: u64,
        attempt_id: ResponseAttemptId,
    ) -> RequestDisposition {
        if expected_version != self.version {
            return RequestDisposition::Ignored(IgnoredRequestReason::StaleExpectedVersion {
                current: self.version,
                received: expected_version,
            });
        }
        if !matches!(self.status, RequestStatus::Open) {
            return RequestDisposition::Ignored(IgnoredRequestReason::ResponseRequiresOpen);
        }
        if self.used_attempt_ids.contains(&attempt_id) {
            return RequestDisposition::Ignored(IgnoredRequestReason::ResponseAttemptAlreadyUsed);
        }

        let attempt = ResponseAttempt {
            id: attempt_id.clone(),
            submitted_request_version: expected_version,
        };
        self.used_attempt_ids.push(attempt_id);
        self.status = RequestStatus::ResponsePending(attempt);
        RequestDisposition::Applied
    }

    fn resolve_response(&mut self, attempt_id: &ResponseAttemptId) -> RequestDisposition {
        let Some(active_attempt) = self.status.active_attempt() else {
            return RequestDisposition::Ignored(
                IgnoredRequestReason::ResolutionRequiresActiveAttempt,
            );
        };
        if active_attempt.id() != attempt_id {
            return RequestDisposition::Ignored(IgnoredRequestReason::MismatchedResponseAttempt);
        }

        self.status =
            RequestStatus::Resolved(RequestResolution::ManualResponse(active_attempt.clone()));
        RequestDisposition::Applied
    }

    fn fail_response(&mut self, attempt_id: &ResponseAttemptId) -> RequestDisposition {
        let RequestStatus::ResponsePending(active_attempt) = &self.status else {
            return RequestDisposition::Ignored(IgnoredRequestReason::FailureRequiresPending);
        };
        if active_attempt.id() != attempt_id {
            return RequestDisposition::Ignored(IgnoredRequestReason::MismatchedResponseAttempt);
        }

        self.status = RequestStatus::Open;
        RequestDisposition::Applied
    }

    fn mark_delivery_unknown(&mut self, attempt_id: &ResponseAttemptId) -> RequestDisposition {
        let RequestStatus::ResponsePending(active_attempt) = &self.status else {
            return RequestDisposition::Ignored(
                IgnoredRequestReason::DeliveryUnknownRequiresPending,
            );
        };
        if active_attempt.id() != attempt_id {
            return RequestDisposition::Ignored(IgnoredRequestReason::MismatchedResponseAttempt);
        }

        self.status = RequestStatus::DeliveryUnknown(active_attempt.clone());
        RequestDisposition::Applied
    }

    fn expire(&mut self) -> RequestDisposition {
        if !matches!(self.status, RequestStatus::Open) {
            return RequestDisposition::Ignored(IgnoredRequestReason::ExpirationRequiresOpen);
        }

        self.status = RequestStatus::Expired;
        RequestDisposition::Applied
    }

    fn apply_provider_outcome(
        &mut self,
        request_id: &RequestId,
        expected_request_version: u64,
        classification: ProviderOutcomeClassification,
    ) -> RequestDisposition {
        if self.kind != RequestKind::Permission {
            return RequestDisposition::Ignored(
                IgnoredRequestReason::ProviderOutcomeRequiresPermission,
            );
        }
        if request_id != &self.request_id {
            return RequestDisposition::Ignored(IgnoredRequestReason::MismatchedProviderRequest);
        }
        match classification {
            ProviderOutcomeClassification::Resolved(outcome) => {
                self.resolve_provider_outcome(expected_request_version, outcome)
            }
            ProviderOutcomeClassification::OutcomeUnknown { missing } => {
                self.lock_provider_outcome_unknown(expected_request_version, missing)
            }
            ProviderOutcomeClassification::UnboundSessionCapabilityDegrade
            | ProviderOutcomeClassification::AuditOnly(_) => RequestDisposition::Ignored(
                IgnoredRequestReason::ProviderOutcomeClassificationNotApplicable,
            ),
        }
    }

    fn resolve_provider_outcome(
        &mut self,
        expected_request_version: u64,
        outcome: VerifiedProviderOutcome,
    ) -> RequestDisposition {
        if outcome.request_id != self.request_id
            || outcome.request_version != expected_request_version
        {
            return RequestDisposition::Ignored(IgnoredRequestReason::MismatchedProviderRequest);
        }
        if self
            .used_provider_decision_ids
            .contains(&outcome.decision_id)
        {
            return RequestDisposition::Ignored(IgnoredRequestReason::ProviderDecisionAlreadyUsed);
        }
        let version_matches = match &self.status {
            RequestStatus::Open => expected_request_version == self.version,
            RequestStatus::ProviderOutcomeUnknown(unknown) => {
                expected_request_version == unknown.original_request_version()
            }
            _ => {
                return RequestDisposition::Ignored(
                    IgnoredRequestReason::ProviderOutcomeRequiresOpenOrUnknown,
                );
            }
        };
        if !version_matches {
            return RequestDisposition::Ignored(IgnoredRequestReason::StaleExpectedVersion {
                current: self.version,
                received: expected_request_version,
            });
        }

        self.used_provider_decision_ids
            .push(outcome.decision_id.clone());
        self.status = RequestStatus::Resolved(RequestResolution::ProviderOutcome(Box::new(
            ProviderOutcomeResolution { outcome },
        )));
        RequestDisposition::Applied
    }

    fn lock_provider_outcome_unknown(
        &mut self,
        expected_request_version: u64,
        missing: Vec<MissingProviderOutcomeField>,
    ) -> RequestDisposition {
        if missing.is_empty() {
            return RequestDisposition::Ignored(
                IgnoredRequestReason::ProviderOutcomeUnknownRequiresMissingFields,
            );
        }
        if !matches!(self.status, RequestStatus::Open) {
            return RequestDisposition::Ignored(
                IgnoredRequestReason::ProviderOutcomeRequiresOpenOrUnknown,
            );
        }
        if expected_request_version != self.version {
            return RequestDisposition::Ignored(IgnoredRequestReason::StaleExpectedVersion {
                current: self.version,
                received: expected_request_version,
            });
        }
        self.status = RequestStatus::ProviderOutcomeUnknown(ProviderOutcomeUnknown {
            original_request_version: expected_request_version,
            missing,
        });
        RequestDisposition::Applied
    }

    fn invariants_hold(&self) -> bool {
        let attempts_unique = self
            .used_attempt_ids
            .iter()
            .enumerate()
            .all(|(index, attempt_id)| !self.used_attempt_ids[..index].contains(attempt_id));
        let active_attempt_known = match &self.status {
            RequestStatus::ResponsePending(attempt) | RequestStatus::DeliveryUnknown(attempt) => {
                self.used_attempt_ids.contains(attempt.id())
                    && attempt.submitted_request_version() <= self.version
            }
            RequestStatus::Resolved(RequestResolution::ManualResponse(attempt)) => {
                self.used_attempt_ids.contains(attempt.id())
            }
            RequestStatus::Resolved(RequestResolution::ProviderOutcome(resolution)) => self
                .used_provider_decision_ids
                .contains(&resolution.outcome().decision_id),
            RequestStatus::ProviderOutcomeUnknown(unknown) => {
                unknown.original_request_version() <= self.version && !unknown.missing().is_empty()
            }
            RequestStatus::Open | RequestStatus::Expired => true,
        };
        let provider_decisions_unique =
            self.used_provider_decision_ids
                .iter()
                .enumerate()
                .all(|(index, decision_id)| {
                    !self.used_provider_decision_ids[..index].contains(decision_id)
                });
        self.version <= self.last_ingest_seq
            && attempts_unique
            && provider_decisions_unique
            && active_attempt_known
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestEvent {
    ResponseSubmitted {
        expected_version: u64,
        attempt_id: ResponseAttemptId,
    },
    ResponseResolved {
        attempt_id: ResponseAttemptId,
    },
    ResponseFailed {
        attempt_id: ResponseAttemptId,
    },
    DeliveryUnknown {
        attempt_id: ResponseAttemptId,
    },
    ProviderOutcomeClassified {
        request_id: RequestId,
        expected_request_version: u64,
        classification: Box<ProviderOutcomeClassification>,
    },
    RequestExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestDisposition {
    Applied,
    Ignored(IgnoredRequestReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoredRequestReason {
    StaleExpectedVersion { current: u64, received: u64 },
    ResponseRequiresOpen,
    ResponseAttemptAlreadyUsed,
    ResolutionRequiresActiveAttempt,
    FailureRequiresPending,
    DeliveryUnknownRequiresPending,
    MismatchedResponseAttempt,
    ExpirationRequiresOpen,
    ProviderOutcomeRequiresPermission,
    MismatchedProviderRequest,
    ProviderDecisionAlreadyUsed,
    ProviderOutcomeClassificationNotApplicable,
    ProviderOutcomeRequiresOpenOrUnknown,
    ProviderOutcomeUnknownRequiresMissingFields,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    InvalidInitialIngestSequence,
    NonMonotonicIngestSequence { current: u64, received: u64 },
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInitialIngestSequence => {
                formatter.write_str("initial ingest sequence must be greater than zero")
            }
            Self::NonMonotonicIngestSequence { current, received } => write!(
                formatter,
                "ingest sequence must increase: current={current}, received={received}"
            ),
        }
    }
}

impl Error for RequestError {}

pub fn replay_request<I>(
    request_id: RequestId,
    kind: RequestKind,
    requested_ingest_seq: u64,
    events: I,
) -> Result<RequestProjection, RequestError>
where
    I: IntoIterator<Item = (u64, RequestEvent)>,
{
    let mut projection = RequestProjection::new(request_id, kind, requested_ingest_seq)?;
    for (ingest_seq, event) in events {
        projection.apply(ingest_seq, event)?;
    }
    Ok(projection)
}
