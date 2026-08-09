use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde_json::{Map, Value};

const MAX_PERSISTED_STUCK_ID_BYTES: usize = 256;
const MAX_PERSISTED_STUCK_TIMESTAMP_BYTES: usize = 128;
const MAX_PERSISTED_STUCK_REASON_BYTES: usize = 4 * 1024;
const STUCK_NOTIFICATION_DELAY_MS: u64 = 300_000;
const STUCK_NOTIFICATION_SUPPRESSION_MS: u64 = 600_000;

use crate::{
    activity::{
        Activity, ActivityEvent, ActivityProjection, EvidenceId, ProgressKind, ScoreFactor,
        SignalSource, TimestampMs, WaitKind,
    },
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionEvent, AttentionEvidence,
        AttentionItemDraft, AttentionItemId, AttentionProjection, AttentionSeverity, SourceEventId,
    },
    dashboard::{DashboardBucket, dashboard_bucket},
    lifecycle::{LifecycleEvent, LifecycleProjection, RunLifecycle, SessionId},
    run_attention::{
        RunAttentionObservation, StuckAttentionAssessment, StuckAttentionSource,
        plan_stuck_attention_events,
    },
    stuck::{
        StuckAssessment, StuckCause, StuckClearReason, StuckNotificationState, StuckOccurrence,
        StuckOccurrenceId, StuckThresholdSeconds,
    },
};

pub const CHANGES_UNAVAILABLE_REASON: &str = "git_observation_not_configured";
const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionEvent {
    pub protocol_version: String,
    pub event_id: String,
    pub run_id: String,
    pub session_id: Option<String>,
    pub source_kind: String,
    pub source_provider: Option<String>,
    pub source_contract_version: Option<String>,
    pub source_has_extensions: bool,
    pub ingest_seq: u64,
    pub observed_at: String,
    pub event_type: String,
    pub payload: Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeAttribution {
    Exact,
    ObservedDuringRun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeSummary {
    Available {
        attribution: ChangeAttribution,
        files: u64,
        insertions: u64,
        deletions: u64,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StuckNotificationProjection {
    Inactive,
    NotDue {
        occurrence_id: String,
        due_at_monotonic_ms: u64,
    },
    Suppressed {
        occurrence_id: String,
        until_monotonic_ms: u64,
    },
    Due {
        occurrence_id: String,
        due_at_monotonic_ms: u64,
    },
    Delivered {
        occurrence_id: String,
        platform_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StuckResetProjection {
    pub occurrence_id: String,
    pub progress_event_id: String,
    pub reset_monotonic_ms: u64,
    pub notification_suppressed_until_monotonic_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StuckProcessAuthority {
    generation: String,
    observed_monotonic_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DashboardProjection {
    pub run_id: String,
    pub version: u64,
    pub lifecycle: String,
    pub activity: String,
    pub activity_confidence: f64,
    pub activity_wait_kind: Option<WaitKind>,
    pub has_active_blocking_request: bool,
    pub attention_level: String,
    pub attention_open_count: u64,
    pub dashboard_bucket: String,
    pub last_progress_at: Option<String>,
    pub last_progress_event_id: String,
    pub last_liveness_at: Option<String>,
    pub current_stuck_occurrence_id: Option<String>,
    pub stuck_notification: StuckNotificationProjection,
    pub stuck_reset: Option<StuckResetProjection>,
    pub changes: ChangeSummary,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PermissionReplayState {
    ManualOpen {
        request_version: u64,
    },
    ManualPending {
        request_version: u64,
        response_attempt_id: String,
    },
    ManualDeliveryUnknown {
        request_version: u64,
        response_attempt_id: String,
    },
    ManualResolved,
    ProviderAutoOpen {
        request_version: u64,
        permission_mode_version: u64,
        provider_configuration: String,
    },
    ProviderAutoResolved,
}

pub fn replay_dashboard_projection(
    events: &[ProjectionEvent],
) -> Result<DashboardProjection, ProjectionError> {
    let first = events.first().ok_or(ProjectionError::EmptyEventHistory)?;
    if first.event_type != "run.created" {
        return Err(ProjectionError::MissingRunCreated);
    }
    let run_id = first.run_id.clone();
    if events.iter().any(|event| event.run_id != run_id) {
        return Err(ProjectionError::RunIdentityMismatch);
    }

    let mut lifecycle =
        LifecycleProjection::new(first.ingest_seq).map_err(|_| ProjectionError::Lifecycle)?;
    let mut activity = ActivityProjection::new(
        first.ingest_seq,
        TimestampMs::new(first.ingest_seq),
        evidence_id(first)?,
    )
    .map_err(|_| ProjectionError::Activity)?;
    let mut attention =
        AttentionProjection::new(first.ingest_seq).map_err(|_| ProjectionError::Attention)?;
    let mut permission_requests = BTreeMap::new();
    let mut provider_decision_ids = BTreeSet::new();
    let mut changes = ChangeSummary::Unavailable {
        reason: CHANGES_UNAVAILABLE_REASON.to_owned(),
    };
    let mut stuck_assessment = StuckAssessment::Clear(StuckClearReason::ProcessUnavailable);
    let mut stuck_source = None;
    let mut stuck_occurrence_id = None;
    let mut stuck_process_authority = None;
    let mut stuck_notification = StuckNotificationProjection::Inactive;
    let mut stuck_reset = None;
    let mut observed_at_by_evidence =
        BTreeMap::from([(first.event_id.clone(), first.observed_at.clone())]);

    for event in &events[1..] {
        observed_at_by_evidence.insert(event.event_id.clone(), event.observed_at.clone());
        let lifecycle_event = lifecycle_event(event)?;
        lifecycle
            .apply(event.ingest_seq, lifecycle_event.clone())
            .map_err(|_| ProjectionError::Lifecycle)?;
        activity
            .apply(
                event.ingest_seq,
                TimestampMs::new(event.ingest_seq),
                activity_event(event)?,
            )
            .map_err(|_| ProjectionError::Activity)?;
        let mut attention_events =
            attention_events(event, &mut permission_requests, &mut provider_decision_ids)?;
        update_stuck_replay(
            event,
            &mut stuck_assessment,
            &mut stuck_source,
            &mut stuck_occurrence_id,
            &mut stuck_process_authority,
            &mut stuck_notification,
            &mut stuck_reset,
        )?;
        if stuck_occurrence_id.is_none() {
            stuck_assessment = StuckAssessment::Clear(if lifecycle.lifecycle().is_terminal() {
                StuckClearReason::LifecycleInactive
            } else {
                StuckClearReason::ProcessUnavailable
            });
        }
        let stuck = StuckAttentionAssessment::new(
            event.ingest_seq,
            &lifecycle,
            &activity,
            stuck_assessment.clone(),
            stuck_source.clone(),
        )
        .map_err(|_| ProjectionError::Stuck)?;
        let observation = RunAttentionObservation::new(
            SourceEventId::new(event.event_id.clone()).map_err(|_| ProjectionError::Attention)?,
            TimestampMs::new(event.ingest_seq),
            evidence_id(event)?,
        );
        attention_events.extend(
            plan_stuck_attention_events(
                &attention,
                event.ingest_seq,
                &lifecycle,
                &activity,
                &stuck,
                &observation,
            )
            .map_err(|_| ProjectionError::Stuck)?,
        );
        attention
            .apply_batch(event.ingest_seq, attention_events)
            .map_err(|_| ProjectionError::Attention)?;
        match change_summary(event)? {
            Some(summary) => changes = summary,
            None if event.protocol_version == "1.2"
                && matches!(
                    event.event_type.as_str(),
                    "run.completed" | "run.interrupted"
                ) =>
            {
                return Err(ProjectionError::Changes);
            }
            None => {}
        }
    }

    let last_progress_at = observed_at_by_evidence
        .get(activity.last_progress().evidence_id().as_str())
        .cloned();
    let last_liveness_at = observed_at_by_evidence
        .get(activity.last_liveness().evidence_id().as_str())
        .cloned();
    let active_attention = attention.active_items_ordered();
    let attention_open_count =
        u64::try_from(active_attention.len()).map_err(|_| ProjectionError::Attention)?;
    let attention_level = attention
        .highest_active_severity()
        .map_or("None", attention_severity_name)
        .to_owned();
    let last = events.last().expect("non-empty event history");

    Ok(DashboardProjection {
        run_id,
        version: last.ingest_seq,
        lifecycle: lifecycle_name(lifecycle.lifecycle()).to_owned(),
        activity: activity_name(activity.activity()).to_owned(),
        activity_confidence: activity
            .confidence()
            .map_or(0.0, |confidence| f64::from(confidence.as_milli()) / 1_000.0),
        activity_wait_kind: activity.wait_kind(),
        has_active_blocking_request: attention.has_active_blocking_request(),
        attention_level,
        attention_open_count,
        dashboard_bucket: bucket_name(dashboard_bucket(&lifecycle, &attention, &stuck_assessment))
            .to_owned(),
        last_progress_at,
        last_progress_event_id: activity.last_progress().evidence_id().as_str().to_owned(),
        last_liveness_at,
        current_stuck_occurrence_id: stuck_occurrence_id,
        stuck_notification,
        stuck_reset,
        changes,
        updated_at: last.observed_at.clone(),
    })
}

fn change_summary(event: &ProjectionEvent) -> Result<Option<ChangeSummary>, ProjectionError> {
    if event.protocol_version != "1.2" {
        return Ok(None);
    }
    let Some(value) = event.payload.get("changes") else {
        return Ok(None);
    };
    if !matches!(
        event.event_type.as_str(),
        "run.completed" | "run.failed" | "run.interrupted"
    ) {
        return Err(ProjectionError::Changes);
    }
    let object = value.as_object().ok_or(ProjectionError::Changes)?;
    match object.get("availability").and_then(Value::as_str) {
        Some("available") if object.len() == 5 => {
            let attribution = match object.get("attribution").and_then(Value::as_str) {
                Some("exact") => ChangeAttribution::Exact,
                Some("observed_during_run") => ChangeAttribution::ObservedDuringRun,
                _ => return Err(ProjectionError::Changes),
            };
            let files = bounded_change_count(object, "files")?;
            let insertions = bounded_change_count(object, "insertions")?;
            let deletions = bounded_change_count(object, "deletions")?;
            Ok(Some(ChangeSummary::Available {
                attribution,
                files,
                insertions,
                deletions,
            }))
        }
        Some("unavailable") if object.len() == 2 => {
            let reason = object
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty())
                .ok_or(ProjectionError::Changes)?;
            Ok(Some(ChangeSummary::Unavailable {
                reason: reason.to_owned(),
            }))
        }
        _ => Err(ProjectionError::Changes),
    }
}

fn bounded_change_count(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<u64, ProjectionError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProjectionError::InvalidPayload { field })
}

fn lifecycle_event(event: &ProjectionEvent) -> Result<LifecycleEvent, ProjectionError> {
    Ok(match event.event_type.as_str() {
        "session.connected" => LifecycleEvent::SessionConnected {
            session_id: SessionId::new(session_id(event)?)
                .map_err(|_| ProjectionError::InvalidSessionIdentity)?,
        },
        "run.completed" => LifecycleEvent::RunCompleted,
        "run.failed" => LifecycleEvent::RunFailed,
        "run.stopped" => LifecycleEvent::RunStopped,
        "run.interrupted" => LifecycleEvent::RunInterrupted,
        _ => LifecycleEvent::RunEventObserved,
    })
}

fn activity_event(event: &ProjectionEvent) -> Result<ActivityEvent, ProjectionError> {
    let evidence_id = evidence_id(event)?;
    Ok(match event.event_type.as_str() {
        "session.connected" => ActivityEvent::LifecycleActivated { evidence_id },
        "command.started" => ActivityEvent::MeaningfulProgress {
            kind: ProgressKind::CommandStarted,
            evidence_id,
        },
        "permission.requested" if permission_request_is_blocking(event)? => ActivityEvent::Signal(
            crate::activity::ActivitySignal::new(
                Activity::Waiting,
                SignalSource::BlockingRequest,
                ScoreFactor::new(1_000).map_err(|_| ProjectionError::Activity)?,
                ScoreFactor::new(1_000).map_err(|_| ProjectionError::Activity)?,
                evidence_id,
                Some(WaitKind::BlockingRequest),
            )
            .map_err(|_| ProjectionError::Activity)?,
        ),
        "run.completed" | "run.failed" | "run.stopped" | "run.interrupted" => {
            ActivityEvent::LifecycleTerminated { evidence_id }
        }
        "run.possibly_stuck" | "run.stuck_cleared" if is_authoritative_stuck_event(event) => {
            ActivityEvent::ProjectionObserved
        }
        "run.still_working" if is_authoritative_still_working_event(event) => {
            ActivityEvent::ProjectionObserved
        }
        "notification.due" if is_authoritative_stuck_notification_due_event(event) => {
            ActivityEvent::ProjectionObserved
        }
        "notification.delivered" if is_authoritative_stuck_notification_delivered_event(event) => {
            ActivityEvent::ProjectionObserved
        }
        _ => ActivityEvent::LivenessObserved { evidence_id },
    })
}

fn update_stuck_replay(
    event: &ProjectionEvent,
    assessment: &mut StuckAssessment,
    source: &mut Option<StuckAttentionSource>,
    current_occurrence_id: &mut Option<String>,
    current_process_authority: &mut Option<StuckProcessAuthority>,
    notification: &mut StuckNotificationProjection,
    reset: &mut Option<StuckResetProjection>,
) -> Result<(), ProjectionError> {
    if matches!(
        event.event_type.as_str(),
        "run.possibly_stuck" | "run.stuck_cleared"
    ) {
        if event.protocol_version != "1.3" {
            return Ok(());
        }
        if !is_authoritative_stuck_event(event) {
            return Err(ProjectionError::Stuck);
        }
    }
    if matches!(
        event.event_type.as_str(),
        "run.still_working" | "notification.due" | "notification.delivered"
    ) {
        if event.protocol_version != "1.4" {
            return Ok(());
        }
        let authoritative = match event.event_type.as_str() {
            "run.still_working" => is_authoritative_still_working_event(event),
            "notification.due" => is_authoritative_stuck_notification_due_event(event),
            "notification.delivered" => is_authoritative_stuck_notification_delivered_event(event),
            _ => unreachable!("matched Event 1.4 stuck receipt name"),
        };
        if !authoritative {
            return Err(ProjectionError::Stuck);
        }
    }
    match event.event_type.as_str() {
        "run.possibly_stuck" => {
            let occurrence = persisted_stuck_occurrence(event)?;
            let occurrence_id =
                payload_bounded_token(event, "occurrence_id", MAX_PERSISTED_STUCK_ID_BYTES)?;
            if current_occurrence_id.as_deref() == Some(occurrence_id) {
                return Err(ProjectionError::Stuck);
            }
            *source = Some(
                StuckAttentionSource::new_at(
                    &occurrence,
                    SourceEventId::new(event.event_id.clone())
                        .map_err(|_| ProjectionError::Attention)?,
                    TimestampMs::new(event.ingest_seq),
                )
                .map_err(|_| ProjectionError::Stuck)?,
            );
            *assessment = StuckAssessment::PossiblyStuck(occurrence);
            *current_occurrence_id = Some(occurrence_id.to_owned());
            let process_generation = stuck_alive_generation(event)?;
            let due_at_monotonic_ms = stuck_notification_due_at(assessment)?;
            let progress_event_id =
                payload_bounded_token(event, "progress_event_id", MAX_PERSISTED_STUCK_ID_BYTES)?;
            let baseline_monotonic_ms = payload_safe_u64(event, "baseline_monotonic_ms")?;
            let process_observed_monotonic_ms = stuck_process_observed(event)?;
            *current_process_authority = Some(StuckProcessAuthority {
                generation: process_generation,
                observed_monotonic_ms: process_observed_monotonic_ms,
            });
            *notification = if reset.as_ref().is_some_and(|reset| {
                reset.progress_event_id == progress_event_id
                    && reset.reset_monotonic_ms == baseline_monotonic_ms
                    && process_observed_monotonic_ms
                        < reset.notification_suppressed_until_monotonic_ms
            }) {
                StuckNotificationProjection::Suppressed {
                    occurrence_id: occurrence_id.to_owned(),
                    until_monotonic_ms: reset
                        .as_ref()
                        .expect("matching reset exists")
                        .notification_suppressed_until_monotonic_ms,
                }
            } else {
                StuckNotificationProjection::NotDue {
                    occurrence_id: occurrence_id.to_owned(),
                    due_at_monotonic_ms,
                }
            };
        }
        "run.stuck_cleared" => {
            if event.payload.len() != 4
                || payload_bounded_token(
                    event,
                    "evidence_unavailable_reason",
                    MAX_PERSISTED_STUCK_REASON_BYTES,
                )
                .is_err()
                || !matches!(
                    payload_string(event, "reason")?,
                    "lifecycle_inactive"
                        | "blocking_request_open"
                        | "structured_wait"
                        | "progress_observed"
                        | "process_unavailable"
                        | "within_deadline"
                )
            {
                return Err(ProjectionError::Stuck);
            }
            validate_stuck_clear_process(
                event
                    .payload
                    .get("process")
                    .and_then(Value::as_object)
                    .ok_or(ProjectionError::Stuck)?,
            )?;
            let StuckAssessment::PossiblyStuck(current) = assessment else {
                return Err(ProjectionError::Stuck);
            };
            let _ = current;
            if current_occurrence_id.as_deref()
                != Some(payload_bounded_token(
                    event,
                    "occurrence_id",
                    MAX_PERSISTED_STUCK_ID_BYTES,
                )?)
            {
                return Err(ProjectionError::Stuck);
            }
            *assessment = StuckAssessment::Clear(StuckClearReason::ProcessUnavailable);
            *source = None;
            *current_occurrence_id = None;
            *current_process_authority = None;
            *notification = StuckNotificationProjection::Inactive;
        }
        "run.still_working" => {
            validate_still_working_event(
                event,
                assessment,
                current_occurrence_id,
                current_process_authority,
            )?;
            let occurrence_id =
                payload_bounded_token(event, "occurrence_id", MAX_PERSISTED_STUCK_ID_BYTES)?
                    .to_owned();
            let progress_event_id =
                payload_bounded_token(event, "progress_event_id", MAX_PERSISTED_STUCK_ID_BYTES)?
                    .to_owned();
            let reset_monotonic_ms = payload_safe_u64(event, "reset_monotonic_ms")?;
            let notification_suppressed_until_monotonic_ms =
                payload_safe_u64(event, "notification_suppressed_until_monotonic_ms")?;
            *reset = Some(StuckResetProjection {
                occurrence_id: occurrence_id.clone(),
                progress_event_id,
                reset_monotonic_ms,
                notification_suppressed_until_monotonic_ms,
            });
            *notification = StuckNotificationProjection::Suppressed {
                occurrence_id,
                until_monotonic_ms: notification_suppressed_until_monotonic_ms,
            };
            *assessment = StuckAssessment::Clear(StuckClearReason::ProcessUnavailable);
            *source = None;
            *current_occurrence_id = None;
            *current_process_authority = None;
        }
        "notification.due" => {
            validate_stuck_notification_due_event(
                event,
                assessment,
                current_occurrence_id,
                current_process_authority,
                notification,
            )?;
            *current_process_authority = Some(StuckProcessAuthority {
                generation: stuck_alive_generation(event)?,
                observed_monotonic_ms: stuck_process_observed(event)?,
            });
            *notification = StuckNotificationProjection::Due {
                occurrence_id: payload_bounded_token(
                    event,
                    "occurrence_id",
                    MAX_PERSISTED_STUCK_ID_BYTES,
                )?
                .to_owned(),
                due_at_monotonic_ms: payload_safe_u64(event, "due_at_monotonic_ms")?,
            };
        }
        "notification.delivered" => {
            validate_stuck_notification_delivered_event(
                event,
                current_occurrence_id,
                notification,
            )?;
            *notification = StuckNotificationProjection::Delivered {
                occurrence_id: payload_bounded_token(
                    event,
                    "occurrence_id",
                    MAX_PERSISTED_STUCK_ID_BYTES,
                )?
                .to_owned(),
                platform_id: payload_bounded_token(
                    event,
                    "platform_id",
                    MAX_PERSISTED_STUCK_ID_BYTES,
                )?
                .to_owned(),
            };
        }
        _ => {}
    }
    Ok(())
}

fn persisted_stuck_occurrence(event: &ProjectionEvent) -> Result<StuckOccurrence, ProjectionError> {
    if !is_authoritative_stuck_event(event) || event.payload.len() != 10 {
        return Err(ProjectionError::Stuck);
    }
    let cause = match payload_string(event, "cause")? {
        "starting" => StuckCause::Starting,
        "planning" => StuckCause::Activity(Activity::Planning),
        "reading" => StuckCause::Activity(Activity::Reading),
        "editing" => StuckCause::Activity(Activity::Editing),
        "testing" => StuckCause::Activity(Activity::Testing),
        "building" => StuckCause::Activity(Activity::Building),
        "reviewing" => StuckCause::Activity(Activity::Reviewing),
        "waiting" => StuckCause::Activity(Activity::Waiting),
        "unknown" => StuckCause::Activity(Activity::Unknown),
        _ => return Err(ProjectionError::Stuck),
    };
    let threshold = payload_u64(event, "threshold_seconds")
        .and_then(|value| u16::try_from(value).ok())
        .and_then(|value| StuckThresholdSeconds::new(value).ok())
        .ok_or(ProjectionError::Stuck)?;
    let progress_at = payload_safe_u64(event, "progress_monotonic_ms")?;
    let baseline_at = payload_safe_u64(event, "baseline_monotonic_ms")?;
    let stuck_since = payload_safe_u64(event, "stuck_since_monotonic_ms")?;
    let progress_evidence_id = EvidenceId::new(payload_bounded_token(
        event,
        "progress_event_id",
        MAX_PERSISTED_STUCK_ID_BYTES,
    )?)
    .map_err(|_| ProjectionError::Stuck)?;
    let process = event
        .payload
        .get("process")
        .and_then(Value::as_object)
        .ok_or(ProjectionError::Stuck)?;
    validate_stuck_process(cause, process, stuck_since)?;
    payload_bounded_token(
        event,
        "progress_observed_at",
        MAX_PERSISTED_STUCK_TIMESTAMP_BYTES,
    )?;
    payload_bounded_token(
        event,
        "evidence_unavailable_reason",
        MAX_PERSISTED_STUCK_REASON_BYTES,
    )?;
    let id = StuckOccurrenceId::from_persisted(
        cause,
        TimestampMs::new(progress_at),
        progress_evidence_id,
        TimestampMs::new(baseline_at),
        TimestampMs::new(stuck_since),
        threshold,
    )
    .map_err(|_| ProjectionError::Stuck)?;
    let due_at = stuck_since
        .checked_add(300_000)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProjectionError::Stuck)?;
    Ok(StuckOccurrence::from_persisted(
        id,
        StuckNotificationState::NotDue {
            due_at: TimestampMs::new(due_at),
        },
        payload_bounded_token(event, "occurrence_id", MAX_PERSISTED_STUCK_ID_BYTES)?.to_owned(),
    ))
}

fn stuck_notification_due_at(assessment: &StuckAssessment) -> Result<u64, ProjectionError> {
    let StuckAssessment::PossiblyStuck(occurrence) = assessment else {
        return Err(ProjectionError::Stuck);
    };
    occurrence
        .id()
        .stuck_since()
        .as_u64()
        .checked_add(STUCK_NOTIFICATION_DELAY_MS)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProjectionError::Stuck)
}

fn stuck_process_observed(event: &ProjectionEvent) -> Result<u64, ProjectionError> {
    event
        .payload
        .get("process")
        .and_then(Value::as_object)
        .and_then(|process| process.get("observed_monotonic_ms"))
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProjectionError::Stuck)
}

fn stuck_alive_generation(event: &ProjectionEvent) -> Result<String, ProjectionError> {
    let process = event
        .payload
        .get("process")
        .and_then(Value::as_object)
        .ok_or(ProjectionError::Stuck)?;
    if process.len() != 3 || process.get("status").and_then(Value::as_str) != Some("alive") {
        return Err(ProjectionError::Stuck);
    }
    let generation = process
        .get("generation")
        .and_then(|value| bounded_token(value, MAX_PERSISTED_STUCK_ID_BYTES))
        .ok_or(ProjectionError::Stuck)?;
    let _ = stuck_process_observed(event)?;
    Ok(generation.to_owned())
}

fn validate_still_working_event(
    event: &ProjectionEvent,
    assessment: &StuckAssessment,
    current_occurrence_id: &Option<String>,
    current_process_authority: &Option<StuckProcessAuthority>,
) -> Result<(), ProjectionError> {
    if event.payload.len() != 6
        || payload_bounded_token(
            event,
            "evidence_unavailable_reason",
            MAX_PERSISTED_STUCK_REASON_BYTES,
        )
        .is_err()
    {
        return Err(ProjectionError::Stuck);
    }
    let StuckAssessment::PossiblyStuck(occurrence) = assessment else {
        return Err(ProjectionError::Stuck);
    };
    let occurrence_id =
        payload_bounded_token(event, "occurrence_id", MAX_PERSISTED_STUCK_ID_BYTES)?;
    if current_occurrence_id.as_deref() != Some(occurrence_id)
        || payload_bounded_token(event, "progress_event_id", MAX_PERSISTED_STUCK_ID_BYTES)?
            != occurrence.id().progress_evidence_id().as_str()
    {
        return Err(ProjectionError::Stuck);
    }
    let reset_monotonic_ms = payload_safe_u64(event, "reset_monotonic_ms")?;
    let suppressed_until = payload_safe_u64(event, "notification_suppressed_until_monotonic_ms")?;
    let process_observed_monotonic_ms = stuck_process_observed(event)?;
    let process_generation = stuck_alive_generation(event)?;
    if reset_monotonic_ms < occurrence.id().stuck_since().as_u64()
        || reset_monotonic_ms.checked_add(STUCK_NOTIFICATION_SUPPRESSION_MS)
            != Some(suppressed_until)
        || process_observed_monotonic_ms != reset_monotonic_ms
        || current_process_authority.as_ref().is_none_or(|current| {
            process_observed_monotonic_ms < current.observed_monotonic_ms
                || current.generation.as_str() != process_generation.as_str()
        })
    {
        return Err(ProjectionError::Stuck);
    }
    Ok(())
}

fn validate_stuck_notification_due_event(
    event: &ProjectionEvent,
    assessment: &StuckAssessment,
    current_occurrence_id: &Option<String>,
    current_process_authority: &Option<StuckProcessAuthority>,
    notification: &StuckNotificationProjection,
) -> Result<(), ProjectionError> {
    if event.payload.len() != 4
        || payload_bounded_token(
            event,
            "evidence_unavailable_reason",
            MAX_PERSISTED_STUCK_REASON_BYTES,
        )
        .is_err()
    {
        return Err(ProjectionError::Stuck);
    }
    let occurrence_id =
        payload_bounded_token(event, "occurrence_id", MAX_PERSISTED_STUCK_ID_BYTES)?;
    let due_at = payload_safe_u64(event, "due_at_monotonic_ms")?;
    let process_observed_monotonic_ms = stuck_process_observed(event)?;
    let process_generation = stuck_alive_generation(event)?;
    if current_occurrence_id.as_deref() != Some(occurrence_id)
        || stuck_notification_due_at(assessment)? != due_at
        || process_observed_monotonic_ms < due_at
        || current_process_authority.as_ref().is_none_or(|current| {
            process_observed_monotonic_ms < current.observed_monotonic_ms
                || current.generation.as_str() != process_generation.as_str()
        })
    {
        return Err(ProjectionError::Stuck);
    }
    match notification {
        StuckNotificationProjection::NotDue {
            occurrence_id: current,
            due_at_monotonic_ms,
        } if current == occurrence_id && *due_at_monotonic_ms == due_at => Ok(()),
        StuckNotificationProjection::Suppressed {
            occurrence_id: current,
            until_monotonic_ms,
        } if current == occurrence_id && process_observed_monotonic_ms >= *until_monotonic_ms => {
            Ok(())
        }
        _ => Err(ProjectionError::Stuck),
    }
}

fn validate_stuck_notification_delivered_event(
    event: &ProjectionEvent,
    current_occurrence_id: &Option<String>,
    notification: &StuckNotificationProjection,
) -> Result<(), ProjectionError> {
    if event.payload.len() != 2 {
        return Err(ProjectionError::Stuck);
    }
    let occurrence_id =
        payload_bounded_token(event, "occurrence_id", MAX_PERSISTED_STUCK_ID_BYTES)?;
    payload_bounded_token(event, "platform_id", MAX_PERSISTED_STUCK_ID_BYTES)?;
    if current_occurrence_id.as_deref() != Some(occurrence_id)
        || !matches!(
            notification,
            StuckNotificationProjection::Due {
                occurrence_id: current,
                ..
            } if current == occurrence_id
        )
    {
        return Err(ProjectionError::Stuck);
    }
    Ok(())
}

fn is_authoritative_stuck_event(event: &ProjectionEvent) -> bool {
    event.protocol_version == "1.3"
        && event.session_id.is_none()
        && event.source_kind == "core"
        && event.source_provider.is_none()
        && event.source_contract_version.as_deref() == Some("stuck-transition/1.0")
        && !event.source_has_extensions
}

fn is_authoritative_still_working_event(event: &ProjectionEvent) -> bool {
    event.protocol_version == "1.4"
        && event.session_id.is_none()
        && event.source_kind == "core"
        && event.source_provider.is_none()
        && event.source_contract_version.as_deref() == Some("stuck-action/1.0")
        && !event.source_has_extensions
}

fn is_authoritative_stuck_notification_due_event(event: &ProjectionEvent) -> bool {
    event.protocol_version == "1.4"
        && event.session_id.is_none()
        && event.source_kind == "core"
        && event.source_provider.is_none()
        && event.source_contract_version.as_deref() == Some("stuck-notification/1.0")
        && !event.source_has_extensions
}

fn is_authoritative_stuck_notification_delivered_event(event: &ProjectionEvent) -> bool {
    event.protocol_version == "1.4"
        && event.session_id.is_none()
        && event.source_kind == "notifier"
        && event.source_provider.is_none()
        && event.source_contract_version.as_deref() == Some("stuck-notification/1.0")
        && !event.source_has_extensions
}

fn validate_stuck_process(
    cause: StuckCause,
    process: &Map<String, Value>,
    stuck_since: u64,
) -> Result<(), ProjectionError> {
    let status = process
        .get("status")
        .and_then(Value::as_str)
        .ok_or(ProjectionError::Stuck)?;
    let observed = process
        .get("observed_monotonic_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER && *value >= stuck_since)
        .ok_or(ProjectionError::Stuck)?;
    let _ = observed;
    match (cause, status) {
        (StuckCause::Starting, "not_spawned") if process.len() == 2 => Ok(()),
        (StuckCause::Activity(_), "alive")
            if process.len() == 3
                && process
                    .get("generation")
                    .and_then(|value| bounded_token(value, MAX_PERSISTED_STUCK_ID_BYTES))
                    .is_some() =>
        {
            Ok(())
        }
        _ => Err(ProjectionError::Stuck),
    }
}

fn validate_stuck_clear_process(process: &Map<String, Value>) -> Result<(), ProjectionError> {
    let status = process
        .get("status")
        .and_then(Value::as_str)
        .ok_or(ProjectionError::Stuck)?;
    process
        .get("observed_monotonic_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProjectionError::Stuck)?;
    match status {
        "not_spawned" if process.len() == 2 => Ok(()),
        "alive"
            if process.len() == 3
                && process
                    .get("generation")
                    .and_then(|value| bounded_token(value, MAX_PERSISTED_STUCK_ID_BYTES))
                    .is_some() =>
        {
            Ok(())
        }
        "unavailable"
            if process.len() == 4
                && process
                    .get("reason")
                    .and_then(|value| bounded_token(value, MAX_PERSISTED_STUCK_REASON_BYTES))
                    .is_some()
                && process.get("generation").is_some_and(|value| {
                    value.is_null() || bounded_token(value, MAX_PERSISTED_STUCK_ID_BYTES).is_some()
                }) =>
        {
            Ok(())
        }
        _ => Err(ProjectionError::Stuck),
    }
}

fn payload_bounded_token<'a>(
    event: &'a ProjectionEvent,
    field: &'static str,
    max_bytes: usize,
) -> Result<&'a str, ProjectionError> {
    let value = event
        .payload
        .get(field)
        .ok_or(ProjectionError::InvalidPayload { field })?;
    bounded_token(value, max_bytes).ok_or(ProjectionError::InvalidPayload { field })
}

fn bounded_token(value: &Value, max_bytes: usize) -> Option<&str> {
    value.as_str().filter(|value| {
        !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
    })
}

fn payload_safe_u64(event: &ProjectionEvent, field: &'static str) -> Result<u64, ProjectionError> {
    payload_u64(event, field)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProjectionError::InvalidPayload { field })
}

fn attention_events(
    event: &ProjectionEvent,
    permission_requests: &mut BTreeMap<String, PermissionReplayState>,
    provider_decision_ids: &mut BTreeSet<String>,
) -> Result<Vec<AttentionEvent>, ProjectionError> {
    let observed_at = TimestampMs::new(event.ingest_seq);
    let evidence_id = evidence_id(event)?;
    let source_event_id =
        SourceEventId::new(event.event_id.clone()).map_err(|_| ProjectionError::Attention)?;
    let evidence = attention_evidence(event)?;
    let mut events = Vec::new();

    match event.event_type.as_str() {
        "permission.requested" if permission_request_is_blocking(event)? => {
            let request_id = payload_string(event, "request_id")?;
            if permission_requests
                .insert(
                    request_id.to_owned(),
                    PermissionReplayState::ManualOpen {
                        request_version: event.ingest_seq,
                    },
                )
                .is_some()
            {
                return Err(ProjectionError::Permission);
            }
            events.push(AttentionEvent::Opened(
                AttentionItemDraft::new(
                    request_item_id(request_id)?,
                    source_event_id,
                    AttentionCategory::Permission,
                    AttentionSeverity::ActionRequired,
                    true,
                    request_dedupe_key(request_id)?,
                    evidence,
                    observed_at,
                )
                .map_err(|_| ProjectionError::Attention)?,
            ));
        }
        "permission.requested" => {
            let request_id = payload_string(event, "request_id")?;
            let permission_mode_version = payload_u64(event, "permission_mode_version")
                .filter(|version| *version > 0)
                .ok_or(ProjectionError::InvalidPayload {
                    field: "permission_mode_version",
                })?;
            let provider_configuration = payload_string(event, "provider_configuration")?;
            if payload_string(event, "permission_mode")? != "provider_auto"
                || payload_bool(event, "response_supported") != Some(false)
                || permission_requests
                    .insert(
                        request_id.to_owned(),
                        PermissionReplayState::ProviderAutoOpen {
                            request_version: event.ingest_seq,
                            permission_mode_version,
                            provider_configuration: provider_configuration.to_owned(),
                        },
                    )
                    .is_some()
            {
                return Err(ProjectionError::Permission);
            }
        }
        "permission.response_submitted" => {
            let Some((request_id, request_version, response_attempt_id)) =
                permission_response_identity(event)
            else {
                return Ok(events);
            };
            if matches!(
                permission_requests.get(request_id),
                Some(PermissionReplayState::ManualOpen {
                    request_version: current,
                }) if *current == request_version
            ) {
                permission_requests.insert(
                    request_id.to_owned(),
                    PermissionReplayState::ManualPending {
                        request_version,
                        response_attempt_id: response_attempt_id.to_owned(),
                    },
                );
                events.push(AttentionEvent::ResponseSubmitted {
                    item_id: request_item_id(request_id)?,
                    observed_at,
                    evidence_id,
                });
            }
        }
        "permission.resolved" => {
            let Some((request_id, request_version, response_attempt_id)) =
                permission_response_identity(event)
            else {
                return Ok(events);
            };
            if matches!(
                permission_requests.get(request_id),
                Some(
                    PermissionReplayState::ManualPending {
                        request_version: current_version,
                        response_attempt_id: current_attempt,
                    }
                    | PermissionReplayState::ManualDeliveryUnknown {
                        request_version: current_version,
                        response_attempt_id: current_attempt,
                    }
                ) if *current_version == request_version
                    && current_attempt == response_attempt_id
            ) {
                permission_requests
                    .insert(request_id.to_owned(), PermissionReplayState::ManualResolved);
                events.push(AttentionEvent::Resolved {
                    item_id: request_item_id(request_id)?,
                    observed_at,
                    evidence_id,
                });
            }
        }
        "permission.delivery_unknown" => {
            let Some((request_id, request_version, response_attempt_id)) =
                permission_response_identity(event)
            else {
                return Ok(events);
            };
            if matches!(
                permission_requests.get(request_id),
                Some(PermissionReplayState::ManualPending {
                    request_version: current_version,
                    response_attempt_id: current_attempt,
                }) if *current_version == request_version
                    && current_attempt == response_attempt_id
            ) {
                permission_requests.insert(
                    request_id.to_owned(),
                    PermissionReplayState::ManualDeliveryUnknown {
                        request_version,
                        response_attempt_id: response_attempt_id.to_owned(),
                    },
                );
                events.push(AttentionEvent::DeliveryUnknown {
                    item_id: request_item_id(request_id)?,
                    observed_at,
                    evidence_id,
                });
            }
        }
        "permission.provider_outcome_resolved" => {
            let Some(request_id) = payload_string_optional(event, "request_id") else {
                return Ok(events);
            };
            let Some(request_version) = payload_u64(event, "request_version") else {
                return Ok(events);
            };
            let Some(decision_id) = payload_string_optional(event, "provider_decision_id") else {
                return Ok(events);
            };
            let Some(permission_mode_version) =
                payload_u64(event, "permission_mode_version").filter(|version| *version > 0)
            else {
                return Ok(events);
            };
            let Some(provider_configuration) =
                payload_string_optional(event, "provider_configuration")
            else {
                return Ok(events);
            };
            let decision_is_exact = matches!(
                payload_string_optional(event, "provider_decision"),
                Some("allowed" | "denied")
            );
            let mode_is_exact =
                payload_string_optional(event, "permission_mode") == Some("provider_auto");
            let terminal_is_exact =
                payload_string_optional(event, "terminal_outcome") == Some("request_resolved");
            if matches!(
                permission_requests.get(request_id),
                Some(PermissionReplayState::ProviderAutoOpen {
                    request_version: current,
                    permission_mode_version: current_mode_version,
                    provider_configuration: current_configuration,
                }) if *current == request_version
                    && *current_mode_version == permission_mode_version
                    && current_configuration == provider_configuration
            ) && mode_is_exact
                && decision_is_exact
                && terminal_is_exact
                && provider_decision_ids.insert(decision_id.to_owned())
            {
                permission_requests.insert(
                    request_id.to_owned(),
                    PermissionReplayState::ProviderAutoResolved,
                );
                events.push(AttentionEvent::Opened(
                    AttentionItemDraft::new(
                        AttentionItemId::new(format!("provider-outcome:{decision_id}"))
                            .map_err(|_| ProjectionError::Attention)?,
                        source_event_id,
                        AttentionCategory::PermissionAudit,
                        AttentionSeverity::Informational,
                        false,
                        AttentionDedupeKey::new(format!("provider-outcome:{decision_id}"))
                            .map_err(|_| ProjectionError::Attention)?,
                        evidence,
                        observed_at,
                    )
                    .map_err(|_| ProjectionError::Attention)?,
                ));
            }
        }
        "run.completed" | "run.failed" | "run.interrupted" => {
            let (category, severity) = match event.event_type.as_str() {
                "run.completed" => (
                    AttentionCategory::Completion,
                    AttentionSeverity::Informational,
                ),
                "run.failed" => (AttentionCategory::Failure, AttentionSeverity::Critical),
                "run.interrupted" => (
                    AttentionCategory::Failure,
                    AttentionSeverity::ActionRequired,
                ),
                _ => unreachable!("matched lifecycle attention event"),
            };
            events.push(AttentionEvent::Opened(
                AttentionItemDraft::new(
                    AttentionItemId::new(format!("lifecycle:{}", event.event_id))
                        .map_err(|_| ProjectionError::Attention)?,
                    source_event_id,
                    category,
                    severity,
                    false,
                    AttentionDedupeKey::new(format!("lifecycle:{}", event.event_id))
                        .map_err(|_| ProjectionError::Attention)?,
                    evidence,
                    observed_at,
                )
                .map_err(|_| ProjectionError::Attention)?,
            ));
        }
        _ => {}
    }
    Ok(events)
}

fn attention_evidence(event: &ProjectionEvent) -> Result<AttentionEvidence, ProjectionError> {
    let reason = event
        .payload
        .get("evidence_unavailable_reason")
        .and_then(|value| value.as_str())
        .unwrap_or("event_evidence_not_hydrated");
    AttentionEvidence::new(
        Vec::new(),
        Some(
            crate::attention::EvidenceUnavailableReason::new(reason)
                .map_err(|_| ProjectionError::Attention)?,
        ),
    )
    .map_err(|_| ProjectionError::Attention)
}

fn session_id(event: &ProjectionEvent) -> Result<String, ProjectionError> {
    event
        .session_id
        .as_ref()
        .filter(|session_id| !session_id.trim().is_empty())
        .cloned()
        .ok_or(ProjectionError::InvalidSessionIdentity)
}

fn evidence_id(event: &ProjectionEvent) -> Result<EvidenceId, ProjectionError> {
    EvidenceId::new(event.event_id.clone()).map_err(|_| ProjectionError::Activity)
}

fn payload_string<'a>(
    event: &'a ProjectionEvent,
    field: &'static str,
) -> Result<&'a str, ProjectionError> {
    event
        .payload
        .get(field)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or(ProjectionError::InvalidPayload { field })
}

fn payload_bool(event: &ProjectionEvent, field: &str) -> Option<bool> {
    event.payload.get(field).and_then(|value| value.as_bool())
}

fn payload_string_optional<'a>(event: &'a ProjectionEvent, field: &str) -> Option<&'a str> {
    event
        .payload
        .get(field)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn payload_u64(event: &ProjectionEvent, field: &str) -> Option<u64> {
    event.payload.get(field).and_then(Value::as_u64)
}

fn permission_request_is_blocking(event: &ProjectionEvent) -> Result<bool, ProjectionError> {
    payload_bool(event, "blocking").ok_or(ProjectionError::InvalidPayload { field: "blocking" })
}

fn permission_response_identity(event: &ProjectionEvent) -> Option<(&str, u64, &str)> {
    Some((
        payload_string_optional(event, "request_id")?,
        payload_u64(event, "request_version")?,
        payload_string_optional(event, "response_attempt_id")?,
    ))
}

fn request_item_id(request_id: &str) -> Result<AttentionItemId, ProjectionError> {
    AttentionItemId::new(format!("request:{request_id}")).map_err(|_| ProjectionError::Attention)
}

fn request_dedupe_key(request_id: &str) -> Result<AttentionDedupeKey, ProjectionError> {
    AttentionDedupeKey::new(format!("request:{request_id}")).map_err(|_| ProjectionError::Attention)
}

const fn lifecycle_name(lifecycle: RunLifecycle) -> &'static str {
    match lifecycle {
        RunLifecycle::Starting => "Starting",
        RunLifecycle::Running => "Running",
        RunLifecycle::Finished => "Finished",
        RunLifecycle::Failed => "Failed",
        RunLifecycle::Stopped => "Stopped",
        RunLifecycle::Interrupted => "Interrupted",
    }
}

const fn activity_name(activity: Activity) -> &'static str {
    match activity {
        Activity::Planning => "Planning",
        Activity::Reading => "Reading",
        Activity::Editing => "Editing",
        Activity::Testing => "Testing",
        Activity::Building => "Building",
        Activity::Reviewing => "Reviewing",
        Activity::Waiting => "Waiting",
        Activity::Unknown => "Unknown",
    }
}

const fn attention_severity_name(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::Informational => "Informational",
        AttentionSeverity::ActionRequired => "ActionRequired",
        AttentionSeverity::Critical => "Critical",
    }
}

const fn bucket_name(bucket: DashboardBucket) -> &'static str {
    match bucket {
        DashboardBucket::NeedsAttention => "NeedsAttention",
        DashboardBucket::PossiblyStuck => "PossiblyStuck",
        DashboardBucket::Working => "Working",
        DashboardBucket::Finished => "Finished",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    EmptyEventHistory,
    MissingRunCreated,
    RunIdentityMismatch,
    InvalidSessionIdentity,
    InvalidPayload { field: &'static str },
    Lifecycle,
    Activity,
    Attention,
    Permission,
    Changes,
    Stuck,
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyEventHistory => formatter.write_str("Run projection requires event history"),
            Self::MissingRunCreated => {
                formatter.write_str("Run projection must start with run.created")
            }
            Self::RunIdentityMismatch => {
                formatter.write_str("Run projection history contains another Run")
            }
            Self::InvalidSessionIdentity => {
                formatter.write_str("Run projection event has an invalid session identity")
            }
            Self::InvalidPayload { field } => {
                write!(
                    formatter,
                    "Run projection event has an invalid {field} payload"
                )
            }
            Self::Lifecycle => formatter.write_str("Run lifecycle projection failed"),
            Self::Activity => formatter.write_str("Run activity projection failed"),
            Self::Attention => formatter.write_str("Run attention projection failed"),
            Self::Permission => formatter.write_str("Run permission projection failed"),
            Self::Changes => formatter.write_str("Run change projection failed"),
            Self::Stuck => formatter.write_str("Run stuck projection failed"),
        }
    }
}

impl Error for ProjectionError {}
