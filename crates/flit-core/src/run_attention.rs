use std::{error::Error, fmt};

use crate::{
    activity::{Activity, ActivityProjection, EvidenceId, TimestampMs},
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionDisposition, AttentionError,
        AttentionEvent, AttentionEvidence, AttentionItem, AttentionItemDraft, AttentionItemId,
        AttentionProjection, AttentionSeverity, SourceEventId,
    },
    lifecycle::{LifecycleDisposition, LifecycleEvent, LifecycleProjection, RunLifecycle},
    request_attention::RequestAttentionPlan,
    stuck::{StuckAssessment, StuckCause, StuckClearReason, StuckOccurrence, StuckOccurrenceId},
};

const LIFECYCLE_ITEM_PREFIX: &str = "lifecycle:";
const STUCK_ITEM_PREFIX: &str = "stuck:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunAttentionObservation {
    source_event_id: SourceEventId,
    observed_at: TimestampMs,
    evidence_id: EvidenceId,
}

impl RunAttentionObservation {
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

#[derive(Clone, Copy, Debug)]
pub struct LifecycleAttentionUpdate<'a> {
    lifecycle: &'a LifecycleProjection,
    event: &'a LifecycleEvent,
    disposition: LifecycleDisposition,
}

impl<'a> LifecycleAttentionUpdate<'a> {
    #[must_use]
    pub const fn new(
        lifecycle: &'a LifecycleProjection,
        event: &'a LifecycleEvent,
        disposition: LifecycleDisposition,
    ) -> Self {
        Self {
            lifecycle,
            event,
            disposition,
        }
    }
}

pub fn sync_run_attention(
    attention: &mut AttentionProjection,
    ingest_seq: u64,
    activity: &ActivityProjection,
    lifecycle_update: LifecycleAttentionUpdate<'_>,
    request_plan: Option<RequestAttentionPlan>,
    stuck: &StuckAttentionAssessment,
    observation: RunAttentionObservation,
) -> Result<Vec<AttentionDisposition>, RunAttentionError> {
    let LifecycleAttentionUpdate {
        lifecycle,
        event,
        disposition,
    } = lifecycle_update;
    if lifecycle.version() != ingest_seq {
        return Err(RunAttentionError::LifecycleIngestSequenceMismatch {
            lifecycle: lifecycle.version(),
            received: ingest_seq,
        });
    }

    validate_stuck_binding(ingest_seq, lifecycle, activity, stuck)?;

    let mut events = if let Some(plan) = request_plan {
        if plan.ingest_seq() != ingest_seq {
            return Err(RunAttentionError::RequestPlanIngestSequenceMismatch {
                plan: plan.ingest_seq(),
                received: ingest_seq,
            });
        }
        if plan.base_attention_version() != attention.version() {
            return Err(RunAttentionError::RequestPlanBaseVersionMismatch {
                plan: plan.base_attention_version(),
                current: attention.version(),
            });
        }
        plan.into_events()
    } else {
        Vec::new()
    };
    events.extend(stuck_events(attention, stuck, &observation)?);
    if let Some((category, severity)) =
        lifecycle_attributes(event, disposition, lifecycle.lifecycle())?
    {
        validate_lifecycle_slot_empty(attention, &observation.source_event_id)?;
        events.push(AttentionEvent::Opened(lifecycle_draft(
            &observation,
            category,
            severity,
        )?));
    }

    attention
        .apply_batch(ingest_seq, events)
        .map_err(RunAttentionError::from)
}

fn lifecycle_attributes(
    event: &LifecycleEvent,
    disposition: LifecycleDisposition,
    lifecycle: RunLifecycle,
) -> Result<Option<(AttentionCategory, AttentionSeverity)>, RunAttentionError> {
    if !matches!(disposition, LifecycleDisposition::Applied) {
        return Ok(None);
    }

    let attributes = match event {
        LifecycleEvent::RunCompleted if lifecycle == RunLifecycle::Finished => Some((
            AttentionCategory::Completion,
            AttentionSeverity::Informational,
        )),
        LifecycleEvent::RunFailed if lifecycle == RunLifecycle::Failed => {
            Some((AttentionCategory::Failure, AttentionSeverity::Critical))
        }
        LifecycleEvent::RunInterrupted if lifecycle == RunLifecycle::Interrupted => Some((
            AttentionCategory::Failure,
            AttentionSeverity::ActionRequired,
        )),
        LifecycleEvent::ResumeFailed { .. } if lifecycle.is_terminal() => Some((
            AttentionCategory::Failure,
            AttentionSeverity::ActionRequired,
        )),
        LifecycleEvent::RunStopped if lifecycle == RunLifecycle::Stopped => None,
        LifecycleEvent::RunEventObserved
        | LifecycleEvent::SessionConnected { .. }
        | LifecycleEvent::ResumeRequested { .. }
        | LifecycleEvent::SessionResumed { .. } => None,
        _ => return Err(RunAttentionError::LifecycleDispositionMismatch),
    };
    Ok(attributes)
}

fn lifecycle_draft(
    observation: &RunAttentionObservation,
    category: AttentionCategory,
    severity: AttentionSeverity,
) -> Result<AttentionItemDraft, RunAttentionError> {
    let identity = encoded_source_identity(&observation.source_event_id);
    AttentionItemDraft::new(
        AttentionItemId::new(format!("{LIFECYCLE_ITEM_PREFIX}{identity}"))?,
        observation.source_event_id.clone(),
        category,
        severity,
        false,
        AttentionDedupeKey::new(format!("{LIFECYCLE_ITEM_PREFIX}{identity}"))?,
        AttentionEvidence::new(vec![observation.evidence_id.clone()], None)?,
        observation.observed_at,
    )
    .map_err(RunAttentionError::from)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StuckAttentionAssessment {
    source_ingest_seq: u64,
    lifecycle_version: u64,
    activity_version: u64,
    assessment: StuckAssessment,
    current_source: Option<StuckAttentionSource>,
}

impl StuckAttentionAssessment {
    pub fn new(
        source_ingest_seq: u64,
        lifecycle: &LifecycleProjection,
        activity: &ActivityProjection,
        assessment: StuckAssessment,
        current_source: Option<StuckAttentionSource>,
    ) -> Result<Self, RunAttentionError> {
        validate_stuck_source(&assessment, current_source.as_ref())?;
        let lifecycle_is_terminal = lifecycle.lifecycle().is_terminal();
        let assessment_is_lifecycle_inactive = matches!(
            assessment,
            StuckAssessment::Clear(StuckClearReason::LifecycleInactive)
        );
        if matches!(assessment, StuckAssessment::PossiblyStuck(_)) && lifecycle_is_terminal
            || assessment_is_lifecycle_inactive != lifecycle_is_terminal
        {
            return Err(RunAttentionError::StuckAssessmentLifecycleMismatch);
        }
        Ok(Self {
            source_ingest_seq,
            lifecycle_version: lifecycle.version(),
            activity_version: activity.version(),
            assessment,
            current_source,
        })
    }

    #[must_use]
    pub const fn assessment(&self) -> &StuckAssessment {
        &self.assessment
    }
}

fn validate_stuck_binding(
    ingest_seq: u64,
    lifecycle: &LifecycleProjection,
    activity: &ActivityProjection,
    stuck: &StuckAttentionAssessment,
) -> Result<(), RunAttentionError> {
    if stuck.source_ingest_seq != ingest_seq {
        return Err(RunAttentionError::StuckAssessmentIngestSequenceMismatch {
            assessment: stuck.source_ingest_seq,
            received: ingest_seq,
        });
    }
    if stuck.lifecycle_version != lifecycle.version() {
        return Err(RunAttentionError::StuckLifecycleVersionMismatch {
            assessment: stuck.lifecycle_version,
            current: lifecycle.version(),
        });
    }
    if stuck.activity_version != activity.version() {
        return Err(RunAttentionError::StuckActivityVersionMismatch {
            assessment: stuck.activity_version,
            current: activity.version(),
        });
    }
    Ok(())
}

fn validate_lifecycle_slot_empty(
    attention: &AttentionProjection,
    source_event_id: &SourceEventId,
) -> Result<(), RunAttentionError> {
    let identity = encoded_source_identity(source_event_id);
    let item_id = AttentionItemId::new(format!("{LIFECYCLE_ITEM_PREFIX}{identity}"))?;
    let dedupe_key = AttentionDedupeKey::new(format!("{LIFECYCLE_ITEM_PREFIX}{identity}"))?;
    if attention
        .items()
        .iter()
        .any(|item| item.item_id() == &item_id || item.dedupe_key() == &dedupe_key)
    {
        return Err(RunAttentionError::LifecycleAttentionCollision);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StuckAttentionSource {
    occurrence_id: StuckOccurrenceId,
    item_id: AttentionItemId,
    dedupe_key: AttentionDedupeKey,
    source_event_id: SourceEventId,
    initial_evidence: AttentionEvidence,
    created_at: TimestampMs,
    draft: AttentionItemDraft,
}

impl StuckAttentionSource {
    pub fn new(
        occurrence: &StuckOccurrence,
        source_event_id: SourceEventId,
    ) -> Result<Self, RunAttentionError> {
        let occurrence_id = occurrence.id().clone();
        let identity = stuck_identity(&occurrence_id);
        let item_id = AttentionItemId::new(format!("{STUCK_ITEM_PREFIX}{identity}"))?;
        let dedupe_key = AttentionDedupeKey::new(format!("{STUCK_ITEM_PREFIX}{identity}"))?;
        let evidence =
            AttentionEvidence::new(vec![occurrence_id.progress_evidence_id().clone()], None)?;
        let created_at = occurrence_id.stuck_since();
        let draft = AttentionItemDraft::new(
            item_id.clone(),
            source_event_id.clone(),
            AttentionCategory::Stuck,
            AttentionSeverity::Informational,
            false,
            dedupe_key.clone(),
            evidence.clone(),
            created_at,
        )?;
        Ok(Self {
            occurrence_id,
            item_id,
            dedupe_key,
            source_event_id,
            initial_evidence: evidence,
            created_at,
            draft,
        })
    }

    #[must_use]
    pub const fn occurrence_id(&self) -> &StuckOccurrenceId {
        &self.occurrence_id
    }

    #[must_use]
    pub fn item_id(&self) -> &AttentionItemId {
        &self.item_id
    }

    fn matches_item(&self, item: &AttentionItem) -> bool {
        item.item_id() == &self.item_id
            && item.dedupe_key() == &self.dedupe_key
            && item.source_event_id() == &self.source_event_id
            && item.category() == AttentionCategory::Stuck
            && item.severity() == AttentionSeverity::Informational
            && !item.blocking()
            && item.created_at() == self.created_at
            && self
                .initial_evidence
                .evidence_ids()
                .iter()
                .all(|evidence_id| item.evidence().evidence_ids().contains(evidence_id))
            && item.evidence().unavailable_reason() == self.initial_evidence.unavailable_reason()
    }
}

fn stuck_events(
    attention: &AttentionProjection,
    stuck: &StuckAttentionAssessment,
    observation: &RunAttentionObservation,
) -> Result<Vec<AttentionEvent>, RunAttentionError> {
    let current_source = stuck.current_source.as_ref();
    let active_stuck_items = attention
        .items()
        .iter()
        .filter(|item| item.category() == AttentionCategory::Stuck && item.status().is_active())
        .collect::<Vec<_>>();
    if active_stuck_items.len() > 1 {
        return Err(RunAttentionError::MultipleActiveStuckItems);
    }

    if let Some(source) = current_source
        && let Some(item) = attention.item(source.item_id())
        && !source.matches_item(item)
    {
        return Err(RunAttentionError::IncompatibleStuckAttentionItem);
    }

    let mut events = Vec::new();
    if let Some(active) = active_stuck_items.first()
        && current_source.is_none_or(|source| source.item_id() != active.item_id())
    {
        events.push(AttentionEvent::Resolved {
            item_id: active.item_id().clone(),
            observed_at: observation.observed_at,
            evidence_id: observation.evidence_id.clone(),
        });
    }

    if let Some(source) = current_source {
        match attention.item(source.item_id()) {
            None => {
                if attention
                    .items()
                    .iter()
                    .any(|item| item.dedupe_key() == &source.dedupe_key)
                {
                    return Err(RunAttentionError::IncompatibleStuckAttentionItem);
                }
                events.push(AttentionEvent::Opened(source.draft.clone()));
            }
            Some(item) if item.status().is_active() => {
                events.push(AttentionEvent::EvidenceObserved {
                    item_id: source.item_id.clone(),
                    observed_at: observation.observed_at,
                    evidence_id: observation.evidence_id.clone(),
                });
            }
            Some(_) => {}
        }
    }

    Ok(events)
}

fn validate_stuck_source<'a>(
    assessment: &StuckAssessment,
    source: Option<&'a StuckAttentionSource>,
) -> Result<Option<&'a StuckAttentionSource>, RunAttentionError> {
    match (assessment, source) {
        (StuckAssessment::PossiblyStuck(occurrence), Some(source))
            if occurrence.id() == source.occurrence_id() =>
        {
            Ok(Some(source))
        }
        (StuckAssessment::Clear(_), None) => Ok(None),
        _ => Err(RunAttentionError::StuckSourceMismatch),
    }
}

fn encoded_source_identity(source_event_id: &SourceEventId) -> String {
    let source = source_event_id.as_str();
    format!("{}:{source}", source.len())
}

fn stuck_identity(occurrence: &StuckOccurrenceId) -> String {
    let cause = match occurrence.cause() {
        StuckCause::Starting => "starting",
        StuckCause::Activity(activity) => activity_identity(activity),
    };
    let evidence = occurrence.progress_evidence_id().as_str();
    format!(
        "{cause}:{}:{}:{}:{}:{evidence}",
        occurrence.progress_at().as_u64(),
        occurrence.baseline_at().as_u64(),
        occurrence.stuck_since().as_u64(),
        evidence.len(),
    )
}

const fn activity_identity(activity: Activity) -> &'static str {
    match activity {
        Activity::Planning => "planning",
        Activity::Reading => "reading",
        Activity::Editing => "editing",
        Activity::Testing => "testing",
        Activity::Building => "building",
        Activity::Reviewing => "reviewing",
        Activity::Waiting => "waiting",
        Activity::Unknown => "unknown",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunAttentionError {
    Attention(AttentionError),
    LifecycleIngestSequenceMismatch { lifecycle: u64, received: u64 },
    LifecycleDispositionMismatch,
    LifecycleAttentionCollision,
    RequestPlanIngestSequenceMismatch { plan: u64, received: u64 },
    RequestPlanBaseVersionMismatch { plan: u64, current: u64 },
    StuckSourceMismatch,
    StuckAssessmentLifecycleMismatch,
    StuckAssessmentIngestSequenceMismatch { assessment: u64, received: u64 },
    StuckLifecycleVersionMismatch { assessment: u64, current: u64 },
    StuckActivityVersionMismatch { assessment: u64, current: u64 },
    MultipleActiveStuckItems,
    IncompatibleStuckAttentionItem,
}

impl From<AttentionError> for RunAttentionError {
    fn from(error: AttentionError) -> Self {
        Self::Attention(error)
    }
}

impl fmt::Display for RunAttentionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Attention(error) => write!(formatter, "attention projection failed: {error}"),
            Self::LifecycleIngestSequenceMismatch {
                lifecycle,
                received,
            } => write!(
                formatter,
                "lifecycle ingest sequence mismatch: lifecycle={lifecycle}, received={received}"
            ),
            Self::LifecycleDispositionMismatch => {
                formatter.write_str("lifecycle event and applied projection state do not match")
            }
            Self::LifecycleAttentionCollision => {
                formatter.write_str("lifecycle attention identity is already occupied")
            }
            Self::RequestPlanIngestSequenceMismatch { plan, received } => write!(
                formatter,
                "request attention plan ingest sequence mismatch: plan={plan}, received={received}"
            ),
            Self::RequestPlanBaseVersionMismatch { plan, current } => write!(
                formatter,
                "request attention plan base version mismatch: plan={plan}, current={current}"
            ),
            Self::StuckSourceMismatch => {
                formatter.write_str("stuck assessment and attention source do not match")
            }
            Self::StuckAssessmentLifecycleMismatch => formatter
                .write_str("stuck assessment does not match the bound lifecycle projection"),
            Self::StuckAssessmentIngestSequenceMismatch {
                assessment,
                received,
            } => write!(
                formatter,
                "stuck assessment ingest sequence mismatch: assessment={assessment}, received={received}"
            ),
            Self::StuckLifecycleVersionMismatch {
                assessment,
                current,
            } => write!(
                formatter,
                "stuck lifecycle version mismatch: assessment={assessment}, current={current}"
            ),
            Self::StuckActivityVersionMismatch {
                assessment,
                current,
            } => write!(
                formatter,
                "stuck activity version mismatch: assessment={assessment}, current={current}"
            ),
            Self::MultipleActiveStuckItems => {
                formatter.write_str("more than one active stuck attention item exists")
            }
            Self::IncompatibleStuckAttentionItem => {
                formatter.write_str("stuck attention identity is occupied by incompatible state")
            }
        }
    }
}

impl Error for RunAttentionError {}
