use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde_json::{Map, Value};

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
    stuck::{StuckAssessment, StuckClearReason},
};

pub const CHANGES_UNAVAILABLE_REASON: &str = "git_observation_not_configured";

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionEvent {
    pub event_id: String,
    pub run_id: String,
    pub session_id: Option<String>,
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

#[derive(Clone, Debug, PartialEq)]
pub struct DashboardProjection {
    pub run_id: String,
    pub version: u64,
    pub lifecycle: String,
    pub activity: String,
    pub activity_confidence: f64,
    pub attention_level: String,
    pub attention_open_count: u64,
    pub dashboard_bucket: String,
    pub last_progress_at: Option<String>,
    pub last_liveness_at: Option<String>,
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
        let attention_events =
            attention_events(event, &mut permission_requests, &mut provider_decision_ids)?;
        attention
            .apply_batch(event.ingest_seq, attention_events)
            .map_err(|_| ProjectionError::Attention)?;
    }

    let clear = StuckAssessment::Clear(StuckClearReason::ProcessUnavailable);
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
        attention_level,
        attention_open_count,
        dashboard_bucket: bucket_name(dashboard_bucket(&lifecycle, &attention, &clear)).to_owned(),
        last_progress_at,
        last_liveness_at,
        changes: ChangeSummary::Unavailable {
            reason: CHANGES_UNAVAILABLE_REASON.to_owned(),
        },
        updated_at: last.observed_at.clone(),
    })
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
        _ => ActivityEvent::LivenessObserved { evidence_id },
    })
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
        }
    }
}

impl Error for ProjectionError {}
