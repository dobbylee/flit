use std::collections::{BTreeMap, BTreeSet};

use flit_core::{
    activity::{Activity, EvidenceId, TimestampMs, WaitKind},
    lifecycle::RunLifecycle,
    stuck::{
        ProcessState, StuckAssessment, StuckCause, StuckClearReason, StuckContext, StuckPolicy,
        StuckProjection,
    },
};
use flit_protocol::{
    ManagedRunsAssessStuckResponse, PossiblyStuckPayload, StuckCauseCode, StuckClearReasonCode,
    StuckClearedPayload, StuckNotificationDuePayload, StuckProcessReceipt,
};
use flit_providers::{CodexProcessHealth, CodexProcessProbe};
use flit_store::{
    ManagedStuckActivity, ManagedStuckAssessment, ManagedStuckAssessmentContext,
    ManagedStuckLifecycle, ManagedStuckNotificationState, ManagedStuckTransition,
    ManagedStuckTransitionOutcome, ManagedStuckWaitKind, Store, StoreError,
};
use sha2::{Digest, Sha256};

const EVIDENCE_UNAVAILABLE_REASON: &str = "provider_content_not_retained";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProgressBaseline {
    pub(crate) progress_event_id: String,
    pub(crate) monotonic_ms: u64,
}

struct PlannedAssessment {
    transitions: Vec<ManagedStuckTransition>,
    next_baselines: BTreeMap<String, ProgressBaseline>,
    unchanged_without_write: u32,
    unavailable_runs: u32,
    active_run_ids: BTreeSet<String>,
}

pub(crate) fn assess_managed_runs(
    store: &mut Store,
    baselines: &mut BTreeMap<String, ProgressBaseline>,
    probes: &mut BTreeMap<String, CodexProcessProbe>,
    now_monotonic_ms: u64,
    observed_at: &str,
) -> Result<ManagedRunsAssessStuckResponse, StoreError> {
    let contexts = store.managed_stuck_assessment_contexts()?;
    let plan = plan_assessment(
        &contexts,
        baselines,
        now_monotonic_ms,
        observed_at,
        |context| process_receipt(context, probes.get(&context.run_id), now_monotonic_ms),
    )?;
    let assessed_runs = u32::try_from(contexts.len()).expect("assessment bound fits u32");
    let outcomes = store.append_managed_stuck_transitions(plan.transitions)?;
    let transitions_appended = u32::try_from(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ManagedStuckTransitionOutcome::Appended(_)))
            .count(),
    )
    .expect("assessment bound fits u32");
    let unchanged_writes = u32::try_from(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ManagedStuckTransitionOutcome::Unchanged { .. }))
            .count(),
    )
    .expect("assessment bound fits u32");
    *baselines = plan.next_baselines;
    probes.retain(|run_id, _| plan.active_run_ids.contains(run_id));
    Ok(ManagedRunsAssessStuckResponse {
        protocol_version: flit_protocol::PROTOCOL_VERSION.to_owned(),
        assessed_runs,
        transitions_appended,
        unchanged_runs: plan.unchanged_without_write + unchanged_writes,
        unavailable_runs: plan.unavailable_runs,
    })
}

fn plan_assessment(
    contexts: &[ManagedStuckAssessmentContext],
    baselines: &BTreeMap<String, ProgressBaseline>,
    now_monotonic_ms: u64,
    observed_at: &str,
    mut observe_process: impl FnMut(&ManagedStuckAssessmentContext) -> StuckProcessReceipt,
) -> Result<PlannedAssessment, StoreError> {
    let mut transitions = Vec::with_capacity(contexts.len());
    let mut next_baselines = BTreeMap::new();
    let mut unchanged_without_write = 0_u32;
    let mut unavailable_runs = 0_u32;
    let mut active_run_ids = BTreeSet::new();
    for context in contexts {
        active_run_ids.insert(context.run_id.clone());
        let previous = baselines.get(&context.run_id);
        let progress_changed = previous
            .is_some_and(|baseline| baseline.progress_event_id != context.progress_event_id);
        let mut baseline = match previous {
            Some(baseline) if baseline.progress_event_id == context.progress_event_id => {
                baseline.clone()
            }
            _ => ProgressBaseline {
                progress_event_id: context.progress_event_id.clone(),
                monotonic_ms: now_monotonic_ms,
            },
        };
        if let Some(reset) = context.reset.as_ref().filter(|reset| {
            reset.progress_event_id == context.progress_event_id
                && reset.reset_monotonic_ms <= now_monotonic_ms
        }) && reset.reset_monotonic_ms > baseline.monotonic_ms
        {
            baseline.monotonic_ms = reset.reset_monotonic_ms;
        }
        next_baselines.insert(context.run_id.clone(), baseline.clone());

        let process = observe_process(context);
        if matches!(process, StuckProcessReceipt::Unavailable { .. }) {
            unavailable_runs += 1;
        }
        let domain_context = domain_context(context, &baseline, &process)?;
        let assessment = StuckProjection::new()
            .assess(
                TimestampMs::new(now_monotonic_ms),
                &domain_context,
                StuckPolicy::default(),
            )
            .map_err(|_| StoreError::ManagedStuckAssessmentContextInvalid {
                run_id: context.run_id.clone(),
                field: "monotonic_time",
            })?;
        let managed = match assessment {
            StuckAssessment::PossiblyStuck(occurrence) => {
                let id = occurrence.id();
                let threshold_seconds =
                    u16::try_from((id.stuck_since().as_u64() - id.baseline_at().as_u64()) / 1_000)
                        .expect("StuckPolicy thresholds fit u16");
                let payload = PossiblyStuckPayload {
                    occurrence_id: occurrence_id(&context.run_id, &id.persistent_identity()),
                    cause: cause_code(id.cause()),
                    threshold_seconds,
                    progress_event_id: context.progress_event_id.clone(),
                    progress_observed_at: context.progress_observed_at.clone(),
                    progress_monotonic_ms: id.progress_at().as_u64(),
                    baseline_monotonic_ms: id.baseline_at().as_u64(),
                    stuck_since_monotonic_ms: id.stuck_since().as_u64(),
                    process: process.clone(),
                    evidence_unavailable_reason: EVIDENCE_UNAVAILABLE_REASON.to_owned(),
                };
                let due_at_monotonic_ms = payload
                    .stuck_since_monotonic_ms
                    .checked_add(300_000)
                    .filter(|value| *value <= flit_protocol::MAX_JSON_SAFE_INTEGER)
                    .ok_or(StoreError::ManagedStuckAssessmentContextInvalid {
                        run_id: context.run_id.clone(),
                        field: "notification_due_at",
                    })?;
                let notification_due = match &context.notification {
                    ManagedStuckNotificationState::NotDue {
                        occurrence_id,
                        due_at_monotonic_ms: persisted_due_at,
                    } => {
                        occurrence_id == &payload.occurrence_id
                            && *persisted_due_at == due_at_monotonic_ms
                            && now_monotonic_ms >= due_at_monotonic_ms
                    }
                    ManagedStuckNotificationState::Suppressed {
                        occurrence_id,
                        until_monotonic_ms,
                    } => {
                        occurrence_id == &payload.occurrence_id
                            && now_monotonic_ms >= due_at_monotonic_ms
                            && now_monotonic_ms >= *until_monotonic_ms
                    }
                    ManagedStuckNotificationState::Inactive
                    | ManagedStuckNotificationState::Due { .. }
                    | ManagedStuckNotificationState::Delivered { .. } => false,
                };
                if notification_due {
                    ManagedStuckAssessment::NotificationDue(StuckNotificationDuePayload {
                        occurrence_id: payload.occurrence_id,
                        due_at_monotonic_ms,
                        process: process.clone(),
                        evidence_unavailable_reason: EVIDENCE_UNAVAILABLE_REASON.to_owned(),
                    })
                } else {
                    ManagedStuckAssessment::PossiblyStuck(payload)
                }
            }
            StuckAssessment::Clear(reason) => {
                let Some(occurrence_id) = context.active_occurrence_id.clone() else {
                    unchanged_without_write += 1;
                    continue;
                };
                ManagedStuckAssessment::Clear(StuckClearedPayload {
                    occurrence_id,
                    reason: if progress_changed {
                        StuckClearReasonCode::ProgressObserved
                    } else {
                        clear_reason_code(reason)
                    },
                    process: process.clone(),
                    evidence_unavailable_reason: EVIDENCE_UNAVAILABLE_REASON.to_owned(),
                })
            }
        };
        transitions.push(ManagedStuckTransition {
            run_id: context.run_id.clone(),
            expected_run_version: context.version,
            event_id: transition_event_id(&context.run_id, context.version, &managed),
            observed_at: observed_at.to_owned(),
            assessment: managed,
        });
    }
    Ok(PlannedAssessment {
        transitions,
        next_baselines,
        unchanged_without_write,
        unavailable_runs,
        active_run_ids,
    })
}

pub(crate) fn process_receipt(
    context: &ManagedStuckAssessmentContext,
    probe: Option<&CodexProcessProbe>,
    observed_monotonic_ms: u64,
) -> StuckProcessReceipt {
    if context.lifecycle == ManagedStuckLifecycle::Starting {
        return StuckProcessReceipt::Unavailable {
            generation: probe.map(|probe| probe.generation().to_owned()),
            reason: "provider_process_generation_unavailable".to_owned(),
            observed_monotonic_ms,
        };
    }
    match probe {
        Some(probe) => match probe.observe() {
            CodexProcessHealth::Alive => StuckProcessReceipt::Alive {
                generation: probe.generation().to_owned(),
                observed_monotonic_ms,
            },
            CodexProcessHealth::Exited => StuckProcessReceipt::Unavailable {
                generation: Some(probe.generation().to_owned()),
                reason: "provider_process_exited".to_owned(),
                observed_monotonic_ms,
            },
            CodexProcessHealth::Unavailable => StuckProcessReceipt::Unavailable {
                generation: Some(probe.generation().to_owned()),
                reason: "provider_process_observation_unavailable".to_owned(),
                observed_monotonic_ms,
            },
        },
        None => StuckProcessReceipt::Unavailable {
            generation: None,
            reason: "provider_process_generation_unavailable".to_owned(),
            observed_monotonic_ms,
        },
    }
}

fn domain_context(
    context: &ManagedStuckAssessmentContext,
    baseline: &ProgressBaseline,
    process: &StuckProcessReceipt,
) -> Result<StuckContext, StoreError> {
    StuckContext::new(
        match context.lifecycle {
            ManagedStuckLifecycle::Starting => RunLifecycle::Starting,
            ManagedStuckLifecycle::Running => RunLifecycle::Running,
        },
        match context.activity {
            ManagedStuckActivity::Planning => Activity::Planning,
            ManagedStuckActivity::Reading => Activity::Reading,
            ManagedStuckActivity::Editing => Activity::Editing,
            ManagedStuckActivity::Testing => Activity::Testing,
            ManagedStuckActivity::Building => Activity::Building,
            ManagedStuckActivity::Reviewing => Activity::Reviewing,
            ManagedStuckActivity::Waiting => Activity::Waiting,
            ManagedStuckActivity::Unknown => Activity::Unknown,
        },
        context.wait_kind.map(|kind| match kind {
            ManagedStuckWaitKind::BlockingRequest => WaitKind::BlockingRequest,
            ManagedStuckWaitKind::External => WaitKind::External,
            ManagedStuckWaitKind::Service => WaitKind::Service,
            ManagedStuckWaitKind::Unstructured => WaitKind::Unstructured,
        }),
        match process {
            StuckProcessReceipt::NotSpawned { .. } => ProcessState::NotSpawned,
            StuckProcessReceipt::Alive { .. } => ProcessState::Alive,
            StuckProcessReceipt::Unavailable { .. } => ProcessState::Unavailable,
        },
        context.has_open_blocking_request,
        TimestampMs::new(baseline.monotonic_ms),
        EvidenceId::new(context.progress_event_id.clone()).map_err(|_| {
            StoreError::ManagedStuckAssessmentContextInvalid {
                run_id: context.run_id.clone(),
                field: "progress_event_id",
            }
        })?,
    )
    .map_err(|_| StoreError::ManagedStuckAssessmentContextInvalid {
        run_id: context.run_id.clone(),
        field: "activity_wait_kind",
    })
}

fn cause_code(cause: StuckCause) -> StuckCauseCode {
    match cause {
        StuckCause::Starting => StuckCauseCode::Starting,
        StuckCause::Activity(activity) => match activity {
            Activity::Planning => StuckCauseCode::Planning,
            Activity::Reading => StuckCauseCode::Reading,
            Activity::Editing => StuckCauseCode::Editing,
            Activity::Testing => StuckCauseCode::Testing,
            Activity::Building => StuckCauseCode::Building,
            Activity::Reviewing => StuckCauseCode::Reviewing,
            Activity::Waiting => StuckCauseCode::Waiting,
            Activity::Unknown => StuckCauseCode::Unknown,
        },
    }
}

fn clear_reason_code(reason: StuckClearReason) -> StuckClearReasonCode {
    match reason {
        StuckClearReason::LifecycleInactive => StuckClearReasonCode::LifecycleInactive,
        StuckClearReason::BlockingRequestOpen => StuckClearReasonCode::BlockingRequestOpen,
        StuckClearReason::StructuredWait(_) => StuckClearReasonCode::StructuredWait,
        StuckClearReason::ProcessUnavailable => StuckClearReasonCode::ProcessUnavailable,
        StuckClearReason::WithinDeadline { .. } => StuckClearReasonCode::WithinDeadline,
    }
}

fn occurrence_id(run_id: &str, persistent_identity: &str) -> String {
    opaque_id("stuck-occurrence", &[run_id, persistent_identity])
}

fn transition_event_id(
    run_id: &str,
    expected_version: u64,
    assessment: &ManagedStuckAssessment,
) -> String {
    let identity = match assessment {
        ManagedStuckAssessment::PossiblyStuck(payload) => {
            format!("possibly-stuck:{}", payload.occurrence_id)
        }
        ManagedStuckAssessment::Clear(payload) => {
            format!(
                "stuck-cleared:{}:{}",
                payload.occurrence_id,
                clear_reason_identity(payload.reason)
            )
        }
        ManagedStuckAssessment::NotificationDue(payload) => {
            format!("notification-due:{}", payload.occurrence_id)
        }
    };
    opaque_id(
        "stuck-transition",
        &[run_id, &expected_version.to_string(), &identity],
    )
}

fn clear_reason_identity(reason: StuckClearReasonCode) -> &'static str {
    match reason {
        StuckClearReasonCode::LifecycleInactive => "lifecycle-inactive",
        StuckClearReasonCode::BlockingRequestOpen => "blocking-request-open",
        StuckClearReasonCode::StructuredWait => "structured-wait",
        StuckClearReasonCode::ProgressObserved => "progress-observed",
        StuckClearReasonCode::ProcessUnavailable => "process-unavailable",
        StuckClearReasonCode::WithinDeadline => "within-deadline",
    }
}

fn opaque_id(prefix: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.len().to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("{prefix}-{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBSERVED_AT: &str = "2026-08-09T10:00:00Z";

    fn context(
        run_id: &str,
        lifecycle: ManagedStuckLifecycle,
        activity: ManagedStuckActivity,
        wait_kind: Option<ManagedStuckWaitKind>,
    ) -> ManagedStuckAssessmentContext {
        ManagedStuckAssessmentContext {
            run_id: run_id.to_owned(),
            version: 4,
            lifecycle,
            activity,
            wait_kind,
            has_open_blocking_request: false,
            progress_event_id: format!("event-{run_id}-progress"),
            progress_observed_at: OBSERVED_AT.to_owned(),
            active_occurrence_id: None,
            reset: None,
            notification: ManagedStuckNotificationState::Inactive,
        }
    }

    fn alive(now: u64) -> StuckProcessReceipt {
        StuckProcessReceipt::Alive {
            generation: "codex-test-generation".to_owned(),
            observed_monotonic_ms: now,
        }
    }

    fn not_spawned(now: u64) -> StuckProcessReceipt {
        StuckProcessReceipt::NotSpawned {
            observed_monotonic_ms: now,
        }
    }

    #[test]
    fn exact_inclusive_thresholds_are_30_120_and_300_seconds() {
        for (context, threshold, process) in [
            (
                context(
                    "run-starting",
                    ManagedStuckLifecycle::Starting,
                    ManagedStuckActivity::Unknown,
                    None,
                ),
                30_000,
                not_spawned as fn(u64) -> StuckProcessReceipt,
            ),
            (
                context(
                    "run-reading",
                    ManagedStuckLifecycle::Running,
                    ManagedStuckActivity::Reading,
                    None,
                ),
                120_000,
                alive as fn(u64) -> StuckProcessReceipt,
            ),
            (
                context(
                    "run-testing",
                    ManagedStuckLifecycle::Running,
                    ManagedStuckActivity::Testing,
                    None,
                ),
                300_000,
                alive as fn(u64) -> StuckProcessReceipt,
            ),
            (
                context(
                    "run-waiting",
                    ManagedStuckLifecycle::Running,
                    ManagedStuckActivity::Waiting,
                    Some(ManagedStuckWaitKind::Unstructured),
                ),
                300_000,
                alive as fn(u64) -> StuckProcessReceipt,
            ),
        ] {
            let initial = plan_assessment(
                std::slice::from_ref(&context),
                &BTreeMap::new(),
                10_000,
                OBSERVED_AT,
                |_| process(10_000),
            )
            .expect("initial baseline");
            assert!(initial.transitions.is_empty());
            let before = plan_assessment(
                std::slice::from_ref(&context),
                &initial.next_baselines,
                10_000 + threshold - 1,
                OBSERVED_AT,
                |_| process(10_000 + threshold - 1),
            )
            .expect("before threshold");
            assert!(before.transitions.is_empty());
            let exact = plan_assessment(
                std::slice::from_ref(&context),
                &initial.next_baselines,
                10_000 + threshold,
                OBSERVED_AT,
                |_| process(10_000 + threshold),
            )
            .expect("exact threshold");
            let ManagedStuckAssessment::PossiblyStuck(payload) = &exact.transitions[0].assessment
            else {
                panic!("exact threshold should be possibly stuck")
            };
            assert_eq!(u64::from(payload.threshold_seconds) * 1_000, threshold);
        }
    }

    #[test]
    fn same_assessment_has_stable_occurrence_and_event_identity() {
        let context = context(
            "run-stable",
            ManagedStuckLifecycle::Running,
            ManagedStuckActivity::Reading,
            None,
        );
        let baselines = BTreeMap::from([(
            context.run_id.clone(),
            ProgressBaseline {
                progress_event_id: context.progress_event_id.clone(),
                monotonic_ms: 1_000,
            },
        )]);
        let first = plan_assessment(
            std::slice::from_ref(&context),
            &baselines,
            121_000,
            OBSERVED_AT,
            |_| alive(121_000),
        )
        .expect("first assessment");
        let second = plan_assessment(&[context], &baselines, 150_000, OBSERVED_AT, |_| {
            alive(150_000)
        })
        .expect("same assessment");
        let first_transition = &first.transitions[0];
        let second_transition = &second.transitions[0];
        assert_eq!(first_transition.event_id, second_transition.event_id);
        let ManagedStuckAssessment::PossiblyStuck(first_payload) = &first_transition.assessment
        else {
            panic!("first assessment")
        };
        let ManagedStuckAssessment::PossiblyStuck(second_payload) = &second_transition.assessment
        else {
            panic!("second assessment")
        };
        assert_eq!(first_payload.occurrence_id, second_payload.occurrence_id);
    }

    #[test]
    fn persisted_notification_becomes_due_once_at_the_exact_boundary() {
        let mut context = context(
            "run-notification-due",
            ManagedStuckLifecycle::Running,
            ManagedStuckActivity::Reading,
            None,
        );
        let baselines = BTreeMap::from([(
            context.run_id.clone(),
            ProgressBaseline {
                progress_event_id: context.progress_event_id.clone(),
                monotonic_ms: 1_000,
            },
        )]);
        let opened = plan_assessment(
            std::slice::from_ref(&context),
            &baselines,
            121_000,
            OBSERVED_AT,
            |_| alive(121_000),
        )
        .expect("open occurrence");
        let ManagedStuckAssessment::PossiblyStuck(opened) = &opened.transitions[0].assessment
        else {
            panic!("threshold must open occurrence")
        };
        context.active_occurrence_id = Some(opened.occurrence_id.clone());
        context.notification = ManagedStuckNotificationState::NotDue {
            occurrence_id: opened.occurrence_id.clone(),
            due_at_monotonic_ms: 421_000,
        };
        let before = plan_assessment(
            std::slice::from_ref(&context),
            &baselines,
            420_999,
            OBSERVED_AT,
            |_| alive(420_999),
        )
        .expect("before due boundary");
        assert!(matches!(
            before.transitions[0].assessment,
            ManagedStuckAssessment::PossiblyStuck(_)
        ));
        let exact = plan_assessment(&[context], &baselines, 421_000, OBSERVED_AT, |_| {
            alive(421_000)
        })
        .expect("exact due boundary");
        let ManagedStuckAssessment::NotificationDue(due) = &exact.transitions[0].assessment else {
            panic!("exact due boundary must persist notification state")
        };
        assert_eq!(due.occurrence_id, opened.occurrence_id);
        assert_eq!(due.due_at_monotonic_ms, 421_000);
    }

    #[test]
    fn new_progress_clears_only_the_persisted_occurrence() {
        let mut context = context(
            "run-progress",
            ManagedStuckLifecycle::Running,
            ManagedStuckActivity::Editing,
            None,
        );
        context.progress_event_id = "event-new-progress".to_owned();
        context.active_occurrence_id = Some("occurrence-exact".to_owned());
        let baselines = BTreeMap::from([(
            context.run_id.clone(),
            ProgressBaseline {
                progress_event_id: "event-old-progress".to_owned(),
                monotonic_ms: 1_000,
            },
        )]);
        let plan = plan_assessment(&[context], &baselines, 200_000, OBSERVED_AT, |_| {
            alive(200_000)
        })
        .expect("progress assessment");
        let ManagedStuckAssessment::Clear(payload) = &plan.transitions[0].assessment else {
            panic!("progress should clear")
        };
        assert_eq!(payload.occurrence_id, "occurrence-exact");
        assert_eq!(payload.reason, StuckClearReasonCode::ProgressObserved);
    }

    #[test]
    fn missing_running_generation_is_unavailable_and_clears_exact_occurrence() {
        let mut context = context(
            "run-restarted",
            ManagedStuckLifecycle::Running,
            ManagedStuckActivity::Unknown,
            None,
        );
        context.active_occurrence_id = Some("occurrence-before-restart".to_owned());
        let receipt = process_receipt(&context, None, 500_000);
        let plan = plan_assessment(&[context], &BTreeMap::new(), 500_000, OBSERVED_AT, |_| {
            receipt.clone()
        })
        .expect("restart assessment");
        assert_eq!(plan.unavailable_runs, 1);
        let ManagedStuckAssessment::Clear(payload) = &plan.transitions[0].assessment else {
            panic!("missing generation should clear")
        };
        assert_eq!(payload.occurrence_id, "occurrence-before-restart");
        assert_eq!(payload.reason, StuckClearReasonCode::ProcessUnavailable);
        assert!(matches!(
            payload.process,
            StuckProcessReceipt::Unavailable {
                generation: None,
                ..
            }
        ));
    }

    #[test]
    fn persisted_starting_row_without_current_generation_is_unavailable() {
        let context = context(
            "run-starting-after-restart",
            ManagedStuckLifecycle::Starting,
            ManagedStuckActivity::Unknown,
            None,
        );
        assert!(matches!(
            process_receipt(&context, None, 10_000),
            StuckProcessReceipt::Unavailable {
                generation: None,
                ..
            }
        ));
    }
}
