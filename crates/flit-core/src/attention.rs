use std::{error::Error, fmt};

use crate::activity::{EvidenceId, TimestampMs};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttentionItemId(String);

impl AttentionItemId {
    pub fn new(value: impl Into<String>) -> Result<Self, AttentionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AttentionError::BlankAttentionItemId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttentionDedupeKey(String);

impl AttentionDedupeKey {
    pub fn new(value: impl Into<String>) -> Result<Self, AttentionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AttentionError::BlankDedupeKey);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEventId(String);

impl SourceEventId {
    pub fn new(value: impl Into<String>) -> Result<Self, AttentionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AttentionError::BlankSourceEventId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceUnavailableReason(String);

impl EvidenceUnavailableReason {
    pub fn new(value: impl Into<String>) -> Result<Self, AttentionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AttentionError::BlankEvidenceUnavailableReason);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttentionEvidence {
    evidence_ids: Vec<EvidenceId>,
    unavailable_reason: Option<EvidenceUnavailableReason>,
}

impl AttentionEvidence {
    pub fn new(
        evidence_ids: Vec<EvidenceId>,
        unavailable_reason: Option<EvidenceUnavailableReason>,
    ) -> Result<Self, AttentionError> {
        if evidence_ids.is_empty() && unavailable_reason.is_none() {
            return Err(AttentionError::MissingEvidence);
        }
        if let Some(duplicate) = evidence_ids
            .iter()
            .enumerate()
            .find_map(|(index, evidence_id)| {
                evidence_ids[..index]
                    .contains(evidence_id)
                    .then_some(evidence_id)
            })
        {
            return Err(AttentionError::DuplicateEvidenceId(
                duplicate.as_str().to_owned(),
            ));
        }
        Ok(Self {
            evidence_ids,
            unavailable_reason,
        })
    }

    #[must_use]
    pub fn evidence_ids(&self) -> &[EvidenceId] {
        &self.evidence_ids
    }

    #[must_use]
    pub fn unavailable_reason(&self) -> Option<&EvidenceUnavailableReason> {
        self.unavailable_reason.as_ref()
    }

    fn append(&mut self, evidence_id: EvidenceId) {
        if !self.evidence_ids.contains(&evidence_id) {
            self.evidence_ids.push(evidence_id);
        }
    }

    fn is_complete(&self) -> bool {
        (!self.evidence_ids.is_empty() || self.unavailable_reason.is_some())
            && self
                .evidence_ids
                .iter()
                .enumerate()
                .all(|(index, evidence_id)| !self.evidence_ids[..index].contains(evidence_id))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttentionCategory {
    Permission,
    PermissionAudit,
    Question,
    Risk,
    Failure,
    Stuck,
    System,
    Completion,
}

impl AttentionCategory {
    const fn is_request(self) -> bool {
        matches!(self, Self::Permission | Self::Question)
    }

    const fn accepts(self, severity: AttentionSeverity, blocking: bool) -> bool {
        match self {
            Self::Permission => {
                blocking
                    && matches!(
                        severity,
                        AttentionSeverity::ActionRequired | AttentionSeverity::Critical
                    )
            }
            Self::PermissionAudit | Self::Stuck | Self::Completion => {
                !blocking && matches!(severity, AttentionSeverity::Informational)
            }
            Self::Question => blocking && matches!(severity, AttentionSeverity::ActionRequired),
            Self::Risk => matches!(severity, AttentionSeverity::Critical),
            Self::Failure => {
                !blocking
                    && matches!(
                        severity,
                        AttentionSeverity::ActionRequired | AttentionSeverity::Critical
                    )
            }
            Self::System => matches!(severity, AttentionSeverity::Critical),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttentionSeverity {
    Informational,
    ActionRequired,
    Critical,
}

impl AttentionSeverity {
    const fn rank(self) -> u8 {
        match self {
            Self::Informational => 0,
            Self::ActionRequired => 1,
            Self::Critical => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttentionStatus {
    Open,
    ResponsePending,
    DeliveryUnknown,
    Resolved,
    Acknowledged,
    Expired,
}

impl AttentionStatus {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Open | Self::ResponsePending | Self::DeliveryUnknown
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttentionItemDraft {
    item_id: AttentionItemId,
    source_event_id: SourceEventId,
    category: AttentionCategory,
    severity: AttentionSeverity,
    blocking: bool,
    dedupe_key: AttentionDedupeKey,
    evidence: AttentionEvidence,
    created_at: TimestampMs,
}

impl AttentionItemDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        item_id: AttentionItemId,
        source_event_id: SourceEventId,
        category: AttentionCategory,
        severity: AttentionSeverity,
        blocking: bool,
        dedupe_key: AttentionDedupeKey,
        evidence: AttentionEvidence,
        created_at: TimestampMs,
    ) -> Result<Self, AttentionError> {
        if !category.accepts(severity, blocking) {
            return Err(AttentionError::InvalidCategoryPolicy {
                category,
                severity,
                blocking,
            });
        }
        Ok(Self {
            item_id,
            source_event_id,
            category,
            severity,
            blocking,
            dedupe_key,
            evidence,
            created_at,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttentionItem {
    item_id: AttentionItemId,
    source_event_id: SourceEventId,
    category: AttentionCategory,
    severity: AttentionSeverity,
    blocking: bool,
    status: AttentionStatus,
    dedupe_key: AttentionDedupeKey,
    version: u64,
    evidence: AttentionEvidence,
    created_at: TimestampMs,
    updated_at: TimestampMs,
    created_ingest_seq: u64,
}

impl AttentionItem {
    fn from_draft(draft: AttentionItemDraft, ingest_seq: u64) -> Self {
        Self {
            item_id: draft.item_id,
            source_event_id: draft.source_event_id,
            category: draft.category,
            severity: draft.severity,
            blocking: draft.blocking,
            status: AttentionStatus::Open,
            dedupe_key: draft.dedupe_key,
            version: ingest_seq,
            evidence: draft.evidence,
            created_at: draft.created_at,
            updated_at: draft.created_at,
            created_ingest_seq: ingest_seq,
        }
    }

    #[must_use]
    pub fn item_id(&self) -> &AttentionItemId {
        &self.item_id
    }

    #[must_use]
    pub fn source_event_id(&self) -> &SourceEventId {
        &self.source_event_id
    }

    #[must_use]
    pub const fn category(&self) -> AttentionCategory {
        self.category
    }

    #[must_use]
    pub const fn severity(&self) -> AttentionSeverity {
        self.severity
    }

    #[must_use]
    pub const fn blocking(&self) -> bool {
        self.blocking
    }

    #[must_use]
    pub const fn status(&self) -> AttentionStatus {
        self.status
    }

    #[must_use]
    pub fn dedupe_key(&self) -> &AttentionDedupeKey {
        &self.dedupe_key
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn evidence(&self) -> &AttentionEvidence {
        &self.evidence
    }

    #[must_use]
    pub const fn created_at(&self) -> TimestampMs {
        self.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> TimestampMs {
        self.updated_at
    }

    #[must_use]
    pub const fn created_ingest_seq(&self) -> u64 {
        self.created_ingest_seq
    }

    fn transition(
        &mut self,
        status: AttentionStatus,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) {
        self.status = status;
        self.version = ingest_seq;
        self.updated_at = self.updated_at.max(observed_at);
        self.evidence.append(evidence_id);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttentionProjection {
    version: u64,
    items: Vec<AttentionItem>,
}

impl AttentionProjection {
    pub fn new(run_created_ingest_seq: u64) -> Result<Self, AttentionError> {
        if run_created_ingest_seq == 0 {
            return Err(AttentionError::InvalidInitialIngestSequence);
        }
        Ok(Self {
            version: run_created_ingest_seq,
            items: Vec::new(),
        })
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub fn items(&self) -> &[AttentionItem] {
        &self.items
    }

    #[must_use]
    pub fn item(&self, item_id: &AttentionItemId) -> Option<&AttentionItem> {
        self.items.iter().find(|item| item.item_id == *item_id)
    }

    #[must_use]
    pub fn active_items_ordered(&self) -> Vec<&AttentionItem> {
        let mut items = self
            .items
            .iter()
            .filter(|item| item.status.is_active())
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .severity
                .rank()
                .cmp(&left.severity.rank())
                .then_with(|| right.blocking.cmp(&left.blocking))
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.created_ingest_seq.cmp(&right.created_ingest_seq))
        });
        items
    }

    #[must_use]
    pub fn highest_active_severity(&self) -> Option<AttentionSeverity> {
        self.active_items_ordered()
            .first()
            .map(|item| item.severity)
    }

    pub fn apply(
        &mut self,
        ingest_seq: u64,
        event: AttentionEvent,
    ) -> Result<AttentionDisposition, AttentionError> {
        let mut dispositions = self.apply_batch(ingest_seq, [event])?;
        Ok(dispositions
            .pop()
            .expect("single-event batch must return one disposition"))
    }

    pub fn apply_batch<I>(
        &mut self,
        ingest_seq: u64,
        events: I,
    ) -> Result<Vec<AttentionDisposition>, AttentionError>
    where
        I: IntoIterator<Item = AttentionEvent>,
    {
        if ingest_seq <= self.version {
            return Err(AttentionError::NonMonotonicIngestSequence {
                current: self.version,
                received: ingest_seq,
            });
        }

        let dispositions = events
            .into_iter()
            .map(|event| self.apply_ordered(ingest_seq, event))
            .collect();
        self.version = ingest_seq;
        debug_assert!(self.invariants_hold());
        Ok(dispositions)
    }

    fn apply_ordered(&mut self, ingest_seq: u64, event: AttentionEvent) -> AttentionDisposition {
        match event {
            AttentionEvent::Opened(draft) => self.open(draft, ingest_seq),
            AttentionEvent::ResponseSubmitted {
                item_id,
                observed_at,
                evidence_id,
            } => self.submit_response(&item_id, ingest_seq, observed_at, evidence_id),
            AttentionEvent::ResponseFailed {
                item_id,
                observed_at,
                evidence_id,
            } => self.fail_response(&item_id, ingest_seq, observed_at, evidence_id),
            AttentionEvent::DeliveryUnknown {
                item_id,
                observed_at,
                evidence_id,
            } => self.mark_delivery_unknown(&item_id, ingest_seq, observed_at, evidence_id),
            AttentionEvent::Resolved {
                item_id,
                observed_at,
                evidence_id,
            } => self.resolve(&item_id, ingest_seq, observed_at, evidence_id),
            AttentionEvent::Expired {
                item_id,
                observed_at,
                evidence_id,
            } => self.expire(&item_id, ingest_seq, observed_at, evidence_id),
            AttentionEvent::Acknowledged {
                item_id,
                observed_at,
                evidence_id,
            } => self.acknowledge(&item_id, ingest_seq, observed_at, evidence_id),
            AttentionEvent::EvidenceObserved {
                item_id,
                observed_at,
                evidence_id,
            } => self.observe_evidence(&item_id, ingest_seq, observed_at, evidence_id),
        }
    }

    fn open(&mut self, draft: AttentionItemDraft, ingest_seq: u64) -> AttentionDisposition {
        if self.items.iter().any(|item| item.item_id == draft.item_id) {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::DuplicateItemId);
        }
        if self
            .items
            .iter()
            .any(|item| item.dedupe_key == draft.dedupe_key)
        {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::DuplicateDedupeKey);
        }
        self.items
            .push(AttentionItem::from_draft(draft, ingest_seq));
        AttentionDisposition::Applied
    }

    fn submit_response(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        if !item.category.is_request() || item.status != AttentionStatus::Open {
            return AttentionDisposition::Ignored(
                IgnoredAttentionReason::ResponseSubmissionRequiresOpenRequest,
            );
        }
        item.transition(
            AttentionStatus::ResponsePending,
            ingest_seq,
            observed_at,
            evidence_id,
        );
        AttentionDisposition::Applied
    }

    fn fail_response(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        if !item.category.is_request() || item.status != AttentionStatus::ResponsePending {
            return AttentionDisposition::Ignored(
                IgnoredAttentionReason::ResponseFailureRequiresPendingRequest,
            );
        }
        item.transition(AttentionStatus::Open, ingest_seq, observed_at, evidence_id);
        AttentionDisposition::Applied
    }

    fn mark_delivery_unknown(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        if !item.category.is_request() || item.status != AttentionStatus::ResponsePending {
            return AttentionDisposition::Ignored(
                IgnoredAttentionReason::DeliveryUnknownRequiresPendingRequest,
            );
        }
        item.transition(
            AttentionStatus::DeliveryUnknown,
            ingest_seq,
            observed_at,
            evidence_id,
        );
        AttentionDisposition::Applied
    }

    fn resolve(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        if !item.status.is_active() {
            return AttentionDisposition::Ignored(
                IgnoredAttentionReason::ResolutionRequiresActiveItem,
            );
        }
        item.transition(
            AttentionStatus::Resolved,
            ingest_seq,
            observed_at,
            evidence_id,
        );
        AttentionDisposition::Applied
    }

    fn expire(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        if !item.category.is_request() || item.status != AttentionStatus::Open {
            return AttentionDisposition::Ignored(
                IgnoredAttentionReason::ExpirationRequiresOpenRequest,
            );
        }
        item.transition(
            AttentionStatus::Expired,
            ingest_seq,
            observed_at,
            evidence_id,
        );
        AttentionDisposition::Applied
    }

    fn acknowledge(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        if item.blocking || item.status != AttentionStatus::Open {
            return AttentionDisposition::Ignored(
                IgnoredAttentionReason::AcknowledgementRequiresOpenNonBlockingItem,
            );
        }
        item.transition(
            AttentionStatus::Acknowledged,
            ingest_seq,
            observed_at,
            evidence_id,
        );
        AttentionDisposition::Applied
    }

    fn observe_evidence(
        &mut self,
        item_id: &AttentionItemId,
        ingest_seq: u64,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    ) -> AttentionDisposition {
        let Some(item) = self.item_mut(item_id) else {
            return AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound);
        };
        item.transition(item.status, ingest_seq, observed_at, evidence_id);
        AttentionDisposition::Applied
    }

    fn item_mut(&mut self, item_id: &AttentionItemId) -> Option<&mut AttentionItem> {
        self.items.iter_mut().find(|item| item.item_id == *item_id)
    }

    fn invariants_hold(&self) -> bool {
        self.version > 0
            && self.items.iter().enumerate().all(|(index, item)| {
                !self.items[..index]
                    .iter()
                    .any(|prior| prior.item_id == item.item_id)
                    && !self.items[..index]
                        .iter()
                        .any(|prior| prior.dedupe_key == item.dedupe_key)
                    && item.version <= self.version
                    && item.created_ingest_seq <= item.version
                    && item.updated_at >= item.created_at
                    && item.evidence.is_complete()
                    && item.category.accepts(item.severity, item.blocking)
                    && (!matches!(
                        item.status,
                        AttentionStatus::ResponsePending
                            | AttentionStatus::DeliveryUnknown
                            | AttentionStatus::Expired
                    ) || item.category.is_request())
                    && (item.status != AttentionStatus::Acknowledged || !item.blocking)
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttentionEvent {
    Opened(AttentionItemDraft),
    ResponseSubmitted {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
    ResponseFailed {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
    DeliveryUnknown {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
    Resolved {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
    Expired {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
    Acknowledged {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
    EvidenceObserved {
        item_id: AttentionItemId,
        observed_at: TimestampMs,
        evidence_id: EvidenceId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttentionDisposition {
    Applied,
    Ignored(IgnoredAttentionReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoredAttentionReason {
    DuplicateItemId,
    DuplicateDedupeKey,
    ItemNotFound,
    ResponseSubmissionRequiresOpenRequest,
    ResponseFailureRequiresPendingRequest,
    DeliveryUnknownRequiresPendingRequest,
    ResolutionRequiresActiveItem,
    ExpirationRequiresOpenRequest,
    AcknowledgementRequiresOpenNonBlockingItem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttentionError {
    InvalidInitialIngestSequence,
    BlankAttentionItemId,
    BlankDedupeKey,
    BlankSourceEventId,
    BlankEvidenceUnavailableReason,
    MissingEvidence,
    DuplicateEvidenceId(String),
    InvalidCategoryPolicy {
        category: AttentionCategory,
        severity: AttentionSeverity,
        blocking: bool,
    },
    NonMonotonicIngestSequence {
        current: u64,
        received: u64,
    },
}

impl fmt::Display for AttentionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInitialIngestSequence => {
                formatter.write_str("initial ingest sequence must be non-zero")
            }
            Self::BlankAttentionItemId => {
                formatter.write_str("attention item identifier must not be blank")
            }
            Self::BlankDedupeKey => formatter.write_str("dedupe key must not be blank"),
            Self::BlankSourceEventId => {
                formatter.write_str("source event identifier must not be blank")
            }
            Self::BlankEvidenceUnavailableReason => {
                formatter.write_str("evidence unavailable reason must not be blank")
            }
            Self::MissingEvidence => {
                formatter.write_str("attention item requires evidence or an unavailable reason")
            }
            Self::DuplicateEvidenceId(evidence_id) => {
                write!(formatter, "duplicate evidence identifier: {evidence_id}")
            }
            Self::InvalidCategoryPolicy {
                category,
                severity,
                blocking,
            } => write!(
                formatter,
                "invalid attention category policy: {category:?}, {severity:?}, blocking={blocking}"
            ),
            Self::NonMonotonicIngestSequence { current, received } => write!(
                formatter,
                "ingest sequence must increase: current {current}, received {received}"
            ),
        }
    }
}

impl Error for AttentionError {}

pub fn replay_attention<I>(
    run_created_ingest_seq: u64,
    events: I,
) -> Result<AttentionProjection, AttentionError>
where
    I: IntoIterator<Item = (u64, AttentionEvent)>,
{
    let mut projection = AttentionProjection::new(run_created_ingest_seq)?;
    for (ingest_seq, event) in events {
        projection.apply(ingest_seq, event)?;
    }
    Ok(projection)
}
