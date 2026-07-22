use std::{error::Error, fmt};

use crate::{
    activity::{EvidenceId, TimestampMs},
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionDisposition, AttentionError,
        AttentionEvent, AttentionEvidence, AttentionItem, AttentionItemDraft, AttentionItemId,
        AttentionProjection, AttentionSeverity, AttentionStatus, SourceEventId,
    },
    request::{RequestId, RequestKind, RequestProjection, RequestResolution, RequestStatus},
};

const REQUEST_ITEM_PREFIX: &str = "request:";
const PROVIDER_POLICY_ITEM_PREFIX: &str = "provider-policy:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAttentionSource {
    request_id: RequestId,
    request_kind: RequestKind,
    item_id: AttentionItemId,
    dedupe_key: AttentionDedupeKey,
    category: AttentionCategory,
    severity: AttentionSeverity,
    source_event_id: SourceEventId,
    created_at: TimestampMs,
    initial_evidence: AttentionEvidence,
    draft: AttentionItemDraft,
}

impl RequestAttentionSource {
    pub fn new(
        request: &RequestProjection,
        source_event_id: SourceEventId,
        severity: AttentionSeverity,
        created_at: TimestampMs,
        evidence: AttentionEvidence,
    ) -> Result<Self, RequestAttentionError> {
        if !is_initial_open_request(request) {
            return Err(RequestAttentionError::SourceRequiresInitialOpenRequest);
        }
        let category = request_category(request.kind());
        let item_id = request_item_id(request.request_id())?;
        let dedupe_key = request_dedupe_key(request.request_id())?;
        let draft = AttentionItemDraft::new(
            item_id.clone(),
            source_event_id.clone(),
            category,
            severity,
            true,
            dedupe_key.clone(),
            evidence.clone(),
            created_at,
        )?;
        Ok(Self {
            request_id: request.request_id().clone(),
            request_kind: request.kind(),
            item_id,
            dedupe_key,
            category,
            severity,
            source_event_id,
            created_at,
            initial_evidence: evidence,
            draft,
        })
    }

    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    #[must_use]
    pub const fn request_kind(&self) -> RequestKind {
        self.request_kind
    }

    #[must_use]
    pub fn item_id(&self) -> &AttentionItemId {
        &self.item_id
    }

    #[must_use]
    pub fn dedupe_key(&self) -> &AttentionDedupeKey {
        &self.dedupe_key
    }

    fn matches_item(&self, item: &AttentionItem) -> bool {
        item.item_id() == &self.item_id
            && item.dedupe_key() == &self.dedupe_key
            && item.category() == self.category
            && item.severity() == self.severity
            && item.blocking()
            && item.source_event_id() == &self.source_event_id
            && item.created_at() == self.created_at
            && self
                .initial_evidence
                .evidence_ids()
                .iter()
                .all(|evidence_id| item.evidence().evidence_ids().contains(evidence_id))
            && item.evidence().unavailable_reason() == self.initial_evidence.unavailable_reason()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAttentionObservation {
    source_event_id: SourceEventId,
    observed_at: TimestampMs,
    evidence_id: EvidenceId,
}

impl RequestAttentionObservation {
    #[must_use]
    pub const fn new(
        source_event_id: SourceEventId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> Self {
        Self {
            source_event_id,
            observed_at,
            evidence_id,
        }
    }
}

pub fn sync_request_attention(
    attention: &mut AttentionProjection,
    ingest_seq: u64,
    request: &RequestProjection,
    source: &RequestAttentionSource,
    observation: RequestAttentionObservation,
) -> Result<Vec<AttentionDisposition>, RequestAttentionError> {
    let plan = plan_request_attention(attention, ingest_seq, request, source, observation)?;
    attention
        .apply_batch(ingest_seq, plan.events)
        .map_err(RequestAttentionError::from)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAttentionPlan {
    ingest_seq: u64,
    base_attention_version: u64,
    events: Vec<AttentionEvent>,
}

impl RequestAttentionPlan {
    pub(crate) const fn ingest_seq(&self) -> u64 {
        self.ingest_seq
    }

    pub(crate) const fn base_attention_version(&self) -> u64 {
        self.base_attention_version
    }

    pub(crate) fn into_events(self) -> Vec<AttentionEvent> {
        self.events
    }
}

pub fn plan_request_attention(
    attention: &AttentionProjection,
    ingest_seq: u64,
    request: &RequestProjection,
    source: &RequestAttentionSource,
    observation: RequestAttentionObservation,
) -> Result<RequestAttentionPlan, RequestAttentionError> {
    validate_binding(ingest_seq, request, source)?;
    let mut events = request_events(attention, request, source, &observation)?;
    if let RequestStatus::Resolved(RequestResolution::ProviderPolicy(resolution)) = request.status()
    {
        let request_is_transitioning = events.iter().any(|event| {
            matches!(
                event,
                AttentionEvent::Resolved { item_id, .. } if item_id == source.item_id()
            )
        });
        if request_is_transitioning {
            validate_provider_policy_slot_empty(attention, resolution)?;
            events.push(provider_policy_event(resolution, &observation)?);
        } else {
            validate_existing_provider_policy_item(attention, request, resolution)?;
        }
    }
    Ok(RequestAttentionPlan {
        ingest_seq,
        base_attention_version: attention.version(),
        events,
    })
}

fn validate_binding(
    ingest_seq: u64,
    request: &RequestProjection,
    source: &RequestAttentionSource,
) -> Result<(), RequestAttentionError> {
    if request.request_id() != source.request_id() {
        return Err(RequestAttentionError::SourceRequestMismatch);
    }
    if request.kind() != source.request_kind() {
        return Err(RequestAttentionError::SourceKindMismatch);
    }
    if request.last_ingest_seq() != ingest_seq {
        return Err(RequestAttentionError::RequestIngestSequenceMismatch {
            request: request.last_ingest_seq(),
            received: ingest_seq,
        });
    }
    Ok(())
}

fn request_events(
    attention: &AttentionProjection,
    request: &RequestProjection,
    source: &RequestAttentionSource,
    observation: &RequestAttentionObservation,
) -> Result<Vec<AttentionEvent>, RequestAttentionError> {
    let Some(item) = attention.item(source.item_id()) else {
        if attention
            .items()
            .iter()
            .any(|item| item.dedupe_key() == source.dedupe_key())
        {
            return Err(RequestAttentionError::IncompatibleRequestAttentionItem);
        }
        return if is_initial_open_request(request) {
            Ok(vec![AttentionEvent::Opened(source.draft.clone())])
        } else {
            Err(RequestAttentionError::MissingRequestAttentionItem {
                request: request_state(request.status()),
            })
        };
    };
    if !source.matches_item(item) {
        return Err(RequestAttentionError::IncompatibleRequestAttentionItem);
    }

    let request_state = request_state(request.status());
    if request.version() != request.last_ingest_seq() {
        if item.version() == request.version()
            && attention_status_matches_request(item.status(), request.status())
        {
            return Ok(Vec::new());
        }
        return Err(RequestAttentionError::IgnoredEventHistoryMismatch {
            request: request_state,
            request_version: request.version(),
            attention: item.status(),
            attention_version: item.version(),
        });
    }
    let event = match (request.status(), item.status()) {
        (RequestStatus::Open, AttentionStatus::Open)
        | (RequestStatus::ResponsePending(_), AttentionStatus::ResponsePending)
        | (RequestStatus::DeliveryUnknown(_), AttentionStatus::DeliveryUnknown)
        | (RequestStatus::Resolved(_), AttentionStatus::Resolved)
        | (RequestStatus::Expired, AttentionStatus::Expired) => None,
        (RequestStatus::ProviderOutcomeUnknown(_), AttentionStatus::Open)
            if request.version() == request.last_ingest_seq() =>
        {
            Some(AttentionEvent::EvidenceObserved {
                item_id: source.item_id.clone(),
                observed_at: observation.observed_at,
                evidence_id: observation.evidence_id.clone(),
            })
        }
        (RequestStatus::ProviderOutcomeUnknown(_), AttentionStatus::Open) => None,
        (RequestStatus::Open, AttentionStatus::ResponsePending) => {
            Some(AttentionEvent::ResponseFailed {
                item_id: source.item_id.clone(),
                observed_at: observation.observed_at,
                evidence_id: observation.evidence_id.clone(),
            })
        }
        (RequestStatus::ResponsePending(_), AttentionStatus::Open) => {
            Some(AttentionEvent::ResponseSubmitted {
                item_id: source.item_id.clone(),
                observed_at: observation.observed_at,
                evidence_id: observation.evidence_id.clone(),
            })
        }
        (RequestStatus::DeliveryUnknown(_), AttentionStatus::ResponsePending) => {
            Some(AttentionEvent::DeliveryUnknown {
                item_id: source.item_id.clone(),
                observed_at: observation.observed_at,
                evidence_id: observation.evidence_id.clone(),
            })
        }
        (RequestStatus::Resolved(_), status) if status.is_active() => {
            Some(AttentionEvent::Resolved {
                item_id: source.item_id.clone(),
                observed_at: observation.observed_at,
                evidence_id: observation.evidence_id.clone(),
            })
        }
        (RequestStatus::Expired, AttentionStatus::Open) => Some(AttentionEvent::Expired {
            item_id: source.item_id.clone(),
            observed_at: observation.observed_at,
            evidence_id: observation.evidence_id.clone(),
        }),
        (_, attention_status) => {
            return Err(RequestAttentionError::IncompatibleStatuses {
                request: request_state,
                attention: attention_status,
            });
        }
    };
    Ok(event.into_iter().collect())
}

fn provider_policy_event(
    resolution: &crate::request::ProviderPolicyResolution,
    observation: &RequestAttentionObservation,
) -> Result<AttentionEvent, RequestAttentionError> {
    let outcome = resolution.outcome();
    let (item_id, dedupe_key, category, severity) = provider_policy_attributes(resolution)?;
    let evidence_id = EvidenceId::new(outcome.evidence_id.as_str())
        .map_err(|_| RequestAttentionError::InvalidProviderEvidenceId)?;
    let evidence = AttentionEvidence::new(vec![evidence_id], None)?;
    let draft = AttentionItemDraft::new(
        item_id,
        observation.source_event_id.clone(),
        category,
        severity,
        false,
        dedupe_key,
        evidence,
        TimestampMs::new(outcome.captured_at_ms),
    )?;
    Ok(AttentionEvent::Opened(draft))
}

fn validate_provider_policy_slot_empty(
    attention: &AttentionProjection,
    resolution: &crate::request::ProviderPolicyResolution,
) -> Result<(), RequestAttentionError> {
    let (item_id, dedupe_key, _, _) = provider_policy_attributes(resolution)?;
    if attention
        .items()
        .iter()
        .any(|item| item.item_id() == &item_id || item.dedupe_key() == &dedupe_key)
    {
        return Err(RequestAttentionError::IncompatibleProviderPolicyAttentionItem);
    }
    Ok(())
}

fn validate_existing_provider_policy_item(
    attention: &AttentionProjection,
    request: &RequestProjection,
    resolution: &crate::request::ProviderPolicyResolution,
) -> Result<(), RequestAttentionError> {
    let (item_id, dedupe_key, category, severity) = provider_policy_attributes(resolution)?;
    let candidates = attention
        .items()
        .iter()
        .filter(|item| item.item_id() == &item_id || item.dedupe_key() == &dedupe_key)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(RequestAttentionError::MissingProviderPolicyAttentionItem);
    }
    let outcome = resolution.outcome();
    let evidence_id = EvidenceId::new(outcome.evidence_id.as_str())
        .map_err(|_| RequestAttentionError::InvalidProviderEvidenceId)?;
    if candidates.len() != 1 {
        return Err(RequestAttentionError::IncompatibleProviderPolicyAttentionItem);
    }
    let item = candidates[0];
    if item.item_id() != &item_id
        || item.dedupe_key() != &dedupe_key
        || item.category() != category
        || item.severity() != severity
        || item.blocking()
        || item.created_at() != TimestampMs::new(outcome.captured_at_ms)
        || item.created_ingest_seq() != request.version()
        || !item.evidence().evidence_ids().contains(&evidence_id)
    {
        return Err(RequestAttentionError::IncompatibleProviderPolicyAttentionItem);
    }
    Ok(())
}

fn provider_policy_attributes(
    resolution: &crate::request::ProviderPolicyResolution,
) -> Result<
    (
        AttentionItemId,
        AttentionDedupeKey,
        AttentionCategory,
        AttentionSeverity,
    ),
    RequestAttentionError,
> {
    let identity = resolution.outcome().decision_id.as_str();
    let item_id = AttentionItemId::new(format!("{PROVIDER_POLICY_ITEM_PREFIX}{identity}"))?;
    let dedupe_key = AttentionDedupeKey::new(format!("{PROVIDER_POLICY_ITEM_PREFIX}{identity}"))?;
    let (category, severity) = if resolution.violations().is_empty() {
        (
            AttentionCategory::PermissionAudit,
            AttentionSeverity::Informational,
        )
    } else {
        (AttentionCategory::Risk, AttentionSeverity::Critical)
    };
    Ok((item_id, dedupe_key, category, severity))
}

fn request_category(kind: RequestKind) -> AttentionCategory {
    match kind {
        RequestKind::Permission => AttentionCategory::Permission,
        RequestKind::Question => AttentionCategory::Question,
    }
}

fn request_item_id(request_id: &RequestId) -> Result<AttentionItemId, AttentionError> {
    AttentionItemId::new(format!("{REQUEST_ITEM_PREFIX}{}", request_id.as_str()))
}

fn request_dedupe_key(request_id: &RequestId) -> Result<AttentionDedupeKey, AttentionError> {
    AttentionDedupeKey::new(format!("{REQUEST_ITEM_PREFIX}{}", request_id.as_str()))
}

fn is_initial_open_request(request: &RequestProjection) -> bool {
    matches!(request.status(), RequestStatus::Open)
        && request.version() == request.last_ingest_seq()
        && request.used_attempt_ids().is_empty()
        && request.used_provider_decision_ids().is_empty()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestAttentionState {
    Open,
    ResponsePending,
    DeliveryUnknown,
    Resolved,
    ProviderOutcomeUnknown,
    Expired,
}

fn request_state(status: &RequestStatus) -> RequestAttentionState {
    match status {
        RequestStatus::Open => RequestAttentionState::Open,
        RequestStatus::ResponsePending(_) => RequestAttentionState::ResponsePending,
        RequestStatus::DeliveryUnknown(_) => RequestAttentionState::DeliveryUnknown,
        RequestStatus::Resolved(_) => RequestAttentionState::Resolved,
        RequestStatus::ProviderOutcomeUnknown(_) => RequestAttentionState::ProviderOutcomeUnknown,
        RequestStatus::Expired => RequestAttentionState::Expired,
    }
}

fn attention_status_matches_request(attention: AttentionStatus, request: &RequestStatus) -> bool {
    matches!(
        (request, attention),
        (RequestStatus::Open, AttentionStatus::Open)
            | (
                RequestStatus::ResponsePending(_),
                AttentionStatus::ResponsePending
            )
            | (
                RequestStatus::DeliveryUnknown(_),
                AttentionStatus::DeliveryUnknown
            )
            | (RequestStatus::Resolved(_), AttentionStatus::Resolved)
            | (
                RequestStatus::ProviderOutcomeUnknown(_),
                AttentionStatus::Open
            )
            | (RequestStatus::Expired, AttentionStatus::Expired)
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestAttentionError {
    SourceRequiresInitialOpenRequest,
    SourceRequestMismatch,
    SourceKindMismatch,
    RequestIngestSequenceMismatch {
        request: u64,
        received: u64,
    },
    MissingRequestAttentionItem {
        request: RequestAttentionState,
    },
    IncompatibleRequestAttentionItem,
    IncompatibleProviderPolicyAttentionItem,
    MissingProviderPolicyAttentionItem,
    IncompatibleStatuses {
        request: RequestAttentionState,
        attention: AttentionStatus,
    },
    IgnoredEventHistoryMismatch {
        request: RequestAttentionState,
        request_version: u64,
        attention: AttentionStatus,
        attention_version: u64,
    },
    InvalidProviderEvidenceId,
    Attention(AttentionError),
}

impl From<AttentionError> for RequestAttentionError {
    fn from(error: AttentionError) -> Self {
        Self::Attention(error)
    }
}

impl fmt::Display for RequestAttentionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceRequiresInitialOpenRequest => {
                formatter.write_str("request attention source requires the initial open request")
            }
            Self::SourceRequestMismatch => {
                formatter.write_str("request attention source identity does not match request")
            }
            Self::SourceKindMismatch => {
                formatter.write_str("request attention source kind does not match request")
            }
            Self::RequestIngestSequenceMismatch { request, received } => write!(
                formatter,
                "request attention ingest sequence mismatch: request {request}, received {received}"
            ),
            Self::MissingRequestAttentionItem { request } => write!(
                formatter,
                "request attention item is missing for state {request:?}"
            ),
            Self::IncompatibleRequestAttentionItem => {
                formatter.write_str("request attention item does not match its source binding")
            }
            Self::IncompatibleProviderPolicyAttentionItem => formatter
                .write_str("provider policy attention item does not match its decision binding"),
            Self::MissingProviderPolicyAttentionItem => formatter
                .write_str("provider policy attention item is missing for a resolved decision"),
            Self::IncompatibleStatuses { request, attention } => write!(
                formatter,
                "request and attention statuses are incompatible: {request:?}, {attention:?}"
            ),
            Self::IgnoredEventHistoryMismatch {
                request,
                request_version,
                attention,
                attention_version,
            } => write!(
                formatter,
                "ignored request event cannot repair attention history: {request:?}@{request_version}, {attention:?}@{attention_version}"
            ),
            Self::InvalidProviderEvidenceId => {
                formatter.write_str("verified provider outcome has an invalid evidence identifier")
            }
            Self::Attention(error) => write!(formatter, "attention projection failed: {error}"),
        }
    }
}

impl Error for RequestAttentionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Attention(error) => Some(error),
            _ => None,
        }
    }
}
