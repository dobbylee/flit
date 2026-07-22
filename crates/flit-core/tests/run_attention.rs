use flit_core::{
    activity::{Activity, ActivityEvent, ActivityProjection, EvidenceId, TimestampMs},
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionDisposition, AttentionError,
        AttentionEvent, AttentionEvidence, AttentionItemDraft, AttentionItemId,
        AttentionProjection, AttentionSeverity, AttentionStatus, SourceEventId,
    },
    lifecycle::{
        LifecycleDisposition, LifecycleEvent, LifecycleProjection, ResumeIntentId, RunLifecycle,
        SessionId,
    },
    request::{RequestId, RequestKind, RequestProjection},
    request_attention::{
        RequestAttentionObservation, RequestAttentionSource, plan_request_attention,
    },
    run_attention::{
        LifecycleAttentionUpdate, RunAttentionError, RunAttentionObservation,
        StuckAttentionAssessment, StuckAttentionSource, sync_run_attention,
    },
    stuck::{
        ProcessState, StuckAssessment, StuckCause, StuckClearReason, StuckContext, StuckPolicy,
        StuckProjection,
    },
};

fn at(value: u64) -> TimestampMs {
    TimestampMs::new(value)
}

fn evidence(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("valid evidence")
}

fn source_event(value: &str) -> SourceEventId {
    SourceEventId::new(value).expect("valid source event")
}

fn observation(label: &str, observed_at: u64) -> RunAttentionObservation {
    RunAttentionObservation::new(
        source_event(&format!("event-{label}")),
        at(observed_at),
        evidence(&format!("evidence-{label}")),
    )
}

fn activity_projection() -> ActivityProjection {
    ActivityProjection::new(1, at(0), evidence("run-created")).expect("valid activity")
}

fn clear_assessment(lifecycle: RunLifecycle) -> StuckAssessment {
    if lifecycle.is_terminal() {
        StuckAssessment::Clear(StuckClearReason::LifecycleInactive)
    } else {
        StuckAssessment::Clear(StuckClearReason::WithinDeadline {
            cause: if lifecycle == RunLifecycle::Starting {
                StuckCause::Starting
            } else {
                StuckCause::Activity(Activity::Unknown)
            },
            deadline_at: at(1),
        })
    }
}

fn sync_lifecycle_attention(
    attention: &mut AttentionProjection,
    ingest_seq: u64,
    lifecycle: &LifecycleProjection,
    event: &LifecycleEvent,
    disposition: LifecycleDisposition,
    observation: RunAttentionObservation,
) -> Result<Vec<AttentionDisposition>, RunAttentionError> {
    let activity = activity_projection();
    let stuck = StuckAttentionAssessment::new(
        ingest_seq,
        lifecycle,
        &activity,
        clear_assessment(lifecycle.lifecycle()),
        None,
    )?;
    sync_run_attention(
        attention,
        ingest_seq,
        &activity,
        LifecycleAttentionUpdate::new(lifecycle, event, disposition),
        None,
        &stuck,
        observation,
    )
}

fn sync_stuck_attention(
    attention: &mut AttentionProjection,
    ingest_seq: u64,
    assessment: &StuckAssessment,
    current_source: Option<&StuckAttentionSource>,
    observed_at: TimestampMs,
    evidence_id: EvidenceId,
) -> Result<Vec<AttentionDisposition>, RunAttentionError> {
    let mut lifecycle = LifecycleProjection::new(1).expect("valid lifecycle");
    let connected = LifecycleEvent::SessionConnected {
        session_id: SessionId::new("stuck-session").expect("valid session"),
    };
    let mut event = connected.clone();
    let mut disposition = lifecycle.apply(2, connected).expect("connection applies");
    if ingest_seq != 2 {
        event = LifecycleEvent::RunEventObserved;
        disposition = lifecycle
            .apply(ingest_seq, event.clone())
            .expect("run observation applies");
    }
    let activity = activity_projection();
    let stuck = StuckAttentionAssessment::new(
        ingest_seq,
        &lifecycle,
        &activity,
        assessment.clone(),
        current_source.cloned(),
    )?;
    sync_run_attention(
        attention,
        ingest_seq,
        &activity,
        LifecycleAttentionUpdate::new(&lifecycle, &event, disposition),
        None,
        &stuck,
        RunAttentionObservation::new(source_event("stuck-evaluation"), observed_at, evidence_id),
    )
}

fn running_projections() -> (LifecycleProjection, AttentionProjection) {
    let mut lifecycle = LifecycleProjection::new(1).expect("valid lifecycle");
    let mut attention = AttentionProjection::new(1).expect("valid attention");
    let event = LifecycleEvent::SessionConnected {
        session_id: SessionId::new("session-1").expect("valid session"),
    };
    let disposition = lifecycle.apply(2, event.clone()).expect("connect applies");
    sync_lifecycle_attention(
        &mut attention,
        2,
        &lifecycle,
        &event,
        disposition,
        observation("connected", 10),
    )
    .expect("non-attention lifecycle event syncs");
    (lifecycle, attention)
}

fn terminal_case(
    event: LifecycleEvent,
) -> (
    LifecycleProjection,
    AttentionProjection,
    LifecycleDisposition,
) {
    let (mut lifecycle, mut attention) = running_projections();
    let disposition = lifecycle.apply(3, event.clone()).expect("terminal applies");
    sync_lifecycle_attention(
        &mut attention,
        3,
        &lifecycle,
        &event,
        disposition,
        observation("terminal", 20),
    )
    .expect("terminal attention syncs");
    (lifecycle, attention, disposition)
}

fn stuck_assessment(progress_at: u64, now: u64, blocking: bool) -> (StuckAssessment, StuckContext) {
    let context = StuckContext::new(
        RunLifecycle::Running,
        Activity::Editing,
        None,
        ProcessState::Alive,
        blocking,
        at(progress_at),
        evidence(&format!("progress-{progress_at}")),
    )
    .expect("valid stuck context");
    let assessment = StuckProjection::new()
        .assess(at(now), &context, StuckPolicy::default())
        .expect("assessment succeeds");
    (assessment, context)
}

fn item(
    attention: &AttentionProjection,
    category: AttentionCategory,
) -> &flit_core::attention::AttentionItem {
    attention
        .items()
        .iter()
        .find(|item| item.category() == category)
        .expect("expected attention item")
}

fn active_stuck_state() -> (
    LifecycleProjection,
    ActivityProjection,
    AttentionProjection,
    StuckProjection,
    StuckAttentionSource,
) {
    let mut lifecycle = LifecycleProjection::new(1).expect("valid lifecycle");
    let mut activity = activity_projection();
    let mut attention = AttentionProjection::new(1).expect("valid attention");
    let stuck_projection = StuckProjection::new();

    let connected = LifecycleEvent::SessionConnected {
        session_id: SessionId::new("atomic-session").expect("valid session"),
    };
    let connected_disposition = lifecycle
        .apply(2, connected.clone())
        .expect("connection applies");
    activity
        .apply(
            2,
            at(10),
            ActivityEvent::LifecycleActivated {
                evidence_id: evidence("connected"),
            },
        )
        .expect("activity activates");
    let connected_context = StuckContext::from_projections(
        &lifecycle,
        &activity,
        ProcessState::Alive,
        attention.has_active_blocking_request(),
    );
    let connected_assessment = stuck_projection
        .assess(at(10), &connected_context, StuckPolicy::default())
        .expect("connection assessment succeeds");
    let connected_stuck =
        StuckAttentionAssessment::new(2, &lifecycle, &activity, connected_assessment, None)
            .expect("connection binding");
    sync_run_attention(
        &mut attention,
        2,
        &activity,
        LifecycleAttentionUpdate::new(&lifecycle, &connected, connected_disposition),
        None,
        &connected_stuck,
        observation("connected-atomic", 10),
    )
    .expect("connection syncs");

    let observed = LifecycleEvent::RunEventObserved;
    let observed_disposition = lifecycle
        .apply(3, observed.clone())
        .expect("observation applies");
    activity
        .apply(
            3,
            at(120_000),
            ActivityEvent::LivenessObserved {
                evidence_id: evidence("stuck-tick"),
            },
        )
        .expect("liveness applies");
    let context = StuckContext::from_projections(
        &lifecycle,
        &activity,
        ProcessState::Alive,
        attention.has_active_blocking_request(),
    );
    let assessment = stuck_projection
        .assess(at(120_000), &context, StuckPolicy::default())
        .expect("stuck assessment succeeds");
    let occurrence = match &assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(reason) => panic!("expected stuck, got {reason:?}"),
    };
    let source = StuckAttentionSource::new(occurrence, source_event("progress-event"))
        .expect("stuck source");
    let stuck =
        StuckAttentionAssessment::new(3, &lifecycle, &activity, assessment, Some(source.clone()))
            .expect("stuck binding");
    sync_run_attention(
        &mut attention,
        3,
        &activity,
        LifecycleAttentionUpdate::new(&lifecycle, &observed, observed_disposition),
        None,
        &stuck,
        observation("stuck-atomic", 120_000),
    )
    .expect("stuck opens");

    (lifecycle, activity, attention, stuck_projection, source)
}

#[test]
fn applied_terminal_events_open_exact_attention_while_normal_stop_stays_quiet() {
    let cases = [
        (
            LifecycleEvent::RunCompleted,
            RunLifecycle::Finished,
            Some((
                AttentionCategory::Completion,
                AttentionSeverity::Informational,
            )),
        ),
        (
            LifecycleEvent::RunFailed,
            RunLifecycle::Failed,
            Some((AttentionCategory::Failure, AttentionSeverity::Critical)),
        ),
        (
            LifecycleEvent::RunInterrupted,
            RunLifecycle::Interrupted,
            Some((
                AttentionCategory::Failure,
                AttentionSeverity::ActionRequired,
            )),
        ),
        (LifecycleEvent::RunStopped, RunLifecycle::Stopped, None),
    ];

    for (event, expected_lifecycle, expected_attention) in cases {
        let (lifecycle, attention, disposition) = terminal_case(event);
        assert_eq!(disposition, LifecycleDisposition::Applied);
        assert_eq!(lifecycle.lifecycle(), expected_lifecycle);
        assert_eq!(attention.version(), 3);
        match expected_attention {
            Some((category, severity)) => {
                assert_eq!(attention.items().len(), 1);
                let item = &attention.items()[0];
                assert_eq!(item.category(), category);
                assert_eq!(item.severity(), severity);
                assert!(!item.blocking());
                assert_eq!(item.status(), AttentionStatus::Open);
                assert_eq!(item.source_event_id().as_str(), "event-terminal");
                assert_eq!(
                    item.evidence().evidence_ids(),
                    &[evidence("evidence-terminal")]
                );
            }
            None => assert!(attention.items().is_empty()),
        }
    }
}

#[test]
fn matching_resume_failure_opens_action_required_failure_without_changing_terminal_state() {
    let (mut lifecycle, mut attention, _) = terminal_case(LifecycleEvent::RunCompleted);
    let intent_id = ResumeIntentId::new("resume-1").expect("valid intent");
    let request = LifecycleEvent::ResumeRequested {
        intent_id: intent_id.clone(),
        expected_version: lifecycle.version(),
    };
    let request_disposition = lifecycle
        .apply(4, request.clone())
        .expect("request applies");
    sync_lifecycle_attention(
        &mut attention,
        4,
        &lifecycle,
        &request,
        request_disposition,
        observation("resume-request", 30),
    )
    .expect("request advances attention");

    let failed = LifecycleEvent::ResumeFailed { intent_id };
    let failed_disposition = lifecycle.apply(5, failed.clone()).expect("failure applies");
    sync_lifecycle_attention(
        &mut attention,
        5,
        &lifecycle,
        &failed,
        failed_disposition,
        observation("resume-failed", 40),
    )
    .expect("resume failure opens attention");

    assert_eq!(lifecycle.lifecycle(), RunLifecycle::Finished);
    assert!(lifecycle.resume_intent().is_none());
    assert_eq!(attention.items().len(), 2);
    let failures = attention
        .items()
        .iter()
        .filter(|item| item.category() == AttentionCategory::Failure)
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].severity(), AttentionSeverity::ActionRequired);
}

#[test]
fn terminal_event_resolves_active_stuck_and_opens_lifecycle_attention_in_one_batch() {
    let cases = [
        (
            LifecycleEvent::RunCompleted,
            Some((
                AttentionCategory::Completion,
                AttentionSeverity::Informational,
            )),
        ),
        (
            LifecycleEvent::RunFailed,
            Some((AttentionCategory::Failure, AttentionSeverity::Critical)),
        ),
        (
            LifecycleEvent::RunInterrupted,
            Some((
                AttentionCategory::Failure,
                AttentionSeverity::ActionRequired,
            )),
        ),
        (LifecycleEvent::RunStopped, None),
    ];

    for (event, expected_lifecycle_item) in cases {
        let (mut lifecycle, mut activity, mut attention, stuck_projection, stuck_source) =
            active_stuck_state();
        let disposition = lifecycle.apply(4, event.clone()).expect("terminal applies");
        activity
            .apply(
                4,
                at(121_000),
                ActivityEvent::LifecycleTerminated {
                    evidence_id: evidence("terminal"),
                },
            )
            .expect("activity terminates");
        let context = StuckContext::from_projections(
            &lifecycle,
            &activity,
            ProcessState::Alive,
            attention.has_active_blocking_request(),
        );
        let assessment = stuck_projection
            .assess(at(121_000), &context, StuckPolicy::default())
            .expect("terminal assessment succeeds");
        assert_eq!(
            assessment,
            StuckAssessment::Clear(StuckClearReason::LifecycleInactive)
        );
        let stuck = StuckAttentionAssessment::new(4, &lifecycle, &activity, assessment, None)
            .expect("terminal stuck binding");
        let dispositions = sync_run_attention(
            &mut attention,
            4,
            &activity,
            LifecycleAttentionUpdate::new(&lifecycle, &event, disposition),
            None,
            &stuck,
            observation("terminal-atomic", 121_000),
        )
        .expect("terminal batch applies");

        let expected_count = if expected_lifecycle_item.is_some() {
            2
        } else {
            1
        };
        assert_eq!(
            dispositions,
            vec![AttentionDisposition::Applied; expected_count]
        );
        assert_eq!(attention.version(), 4);
        assert_eq!(
            attention
                .item(stuck_source.item_id())
                .expect("stuck item")
                .status(),
            AttentionStatus::Resolved
        );
        match expected_lifecycle_item {
            Some((category, severity)) => {
                let lifecycle_item = attention
                    .items()
                    .iter()
                    .find(|item| {
                        item.category() == category && item.item_id() != stuck_source.item_id()
                    })
                    .expect("lifecycle item");
                assert_eq!(lifecycle_item.severity(), severity);
                assert_eq!(lifecycle_item.version(), 4);
            }
            None => assert_eq!(attention.items().len(), 1),
        }
    }
}

#[test]
fn blocking_request_resolves_active_stuck_and_opens_permission_or_question_atomically() {
    for kind in [RequestKind::Permission, RequestKind::Question] {
        let (mut lifecycle, mut activity, mut attention, _stuck_projection, stuck_source) =
            active_stuck_state();
        let event = LifecycleEvent::RunEventObserved;
        let disposition = lifecycle
            .apply(4, event.clone())
            .expect("run event applies");
        activity
            .apply(
                4,
                at(121_000),
                ActivityEvent::LivenessObserved {
                    evidence_id: evidence("blocking-request"),
                },
            )
            .expect("activity advances");

        let request = RequestProjection::new(
            RequestId::new(match kind {
                RequestKind::Permission => "permission-1",
                RequestKind::Question => "question-1",
            })
            .expect("valid request ID"),
            kind,
            4,
        )
        .expect("valid request");
        let request_source = RequestAttentionSource::new(
            &request,
            source_event("request-event"),
            match kind {
                RequestKind::Permission => AttentionSeverity::Critical,
                RequestKind::Question => AttentionSeverity::ActionRequired,
            },
            at(121_000),
            AttentionEvidence::new(vec![evidence("request-evidence")], None)
                .expect("valid request evidence"),
        )
        .expect("valid request source");
        let request_plan = plan_request_attention(
            &attention,
            4,
            &request,
            &request_source,
            RequestAttentionObservation::new(
                source_event("request-observed"),
                at(121_000),
                evidence("request-observed"),
            ),
        )
        .expect("request plan");
        let stuck = StuckAttentionAssessment::new(
            4,
            &lifecycle,
            &activity,
            StuckAssessment::Clear(StuckClearReason::BlockingRequestOpen),
            None,
        )
        .expect("blocking request stuck binding");

        assert_eq!(
            sync_run_attention(
                &mut attention,
                4,
                &activity,
                LifecycleAttentionUpdate::new(&lifecycle, &event, disposition),
                Some(request_plan),
                &stuck,
                observation("blocking-request-batch", 121_000),
            )
            .expect("request and stuck apply in one batch"),
            vec![AttentionDisposition::Applied, AttentionDisposition::Applied]
        );
        assert_eq!(attention.version(), 4);
        assert_eq!(
            attention
                .item(stuck_source.item_id())
                .expect("stuck item")
                .status(),
            AttentionStatus::Resolved
        );
        let request_item = attention
            .item(request_source.item_id())
            .expect("request item");
        assert_eq!(request_item.status(), AttentionStatus::Open);
        assert!(request_item.blocking());
        assert_eq!(
            request_item.category(),
            match kind {
                RequestKind::Permission => AttentionCategory::Permission,
                RequestKind::Question => AttentionCategory::Question,
            }
        );
    }
}

#[test]
fn combined_request_plan_failure_preserves_the_pre_batch_attention_projection() {
    let (mut lifecycle, mut activity, mut attention, _stuck_projection, _stuck_source) =
        active_stuck_state();
    let actual_event = LifecycleEvent::RunEventObserved;
    let disposition = lifecycle.apply(4, actual_event).expect("run event applies");
    activity
        .apply(
            4,
            at(121_000),
            ActivityEvent::LivenessObserved {
                evidence_id: evidence("blocking-request"),
            },
        )
        .expect("activity advances");
    let request = RequestProjection::new(
        RequestId::new("permission-failure").expect("valid request ID"),
        RequestKind::Permission,
        4,
    )
    .expect("valid request");
    let request_source = RequestAttentionSource::new(
        &request,
        source_event("request-failure-event"),
        AttentionSeverity::ActionRequired,
        at(121_000),
        AttentionEvidence::new(vec![evidence("request-failure-evidence")], None)
            .expect("valid request evidence"),
    )
    .expect("valid request source");
    let request_plan = plan_request_attention(
        &attention,
        4,
        &request,
        &request_source,
        RequestAttentionObservation::new(
            source_event("request-failure-observed"),
            at(121_000),
            evidence("request-failure-observed"),
        ),
    )
    .expect("request plan");
    let stuck = StuckAttentionAssessment::new(
        4,
        &lifecycle,
        &activity,
        StuckAssessment::Clear(StuckClearReason::BlockingRequestOpen),
        None,
    )
    .expect("stuck binding");
    let before = attention.clone();
    assert_eq!(
        sync_run_attention(
            &mut attention,
            4,
            &activity,
            LifecycleAttentionUpdate::new(&lifecycle, &LifecycleEvent::RunCompleted, disposition,),
            Some(request_plan),
            &stuck,
            observation("failed-combined-batch", 121_000),
        ),
        Err(RunAttentionError::LifecycleDispositionMismatch)
    );
    assert_eq!(attention, before);
    assert!(attention.item(request_source.item_id()).is_none());
}

#[test]
fn combined_request_and_stuck_sequence_replays_to_the_incremental_projection() {
    fn reduce() -> AttentionProjection {
        let (mut lifecycle, mut activity, mut attention, _stuck_projection, _stuck_source) =
            active_stuck_state();
        let event = LifecycleEvent::RunEventObserved;
        let disposition = lifecycle
            .apply(4, event.clone())
            .expect("run event applies");
        activity
            .apply(
                4,
                at(121_000),
                ActivityEvent::LivenessObserved {
                    evidence_id: evidence("replay-request"),
                },
            )
            .expect("activity advances");
        let request = RequestProjection::new(
            RequestId::new("permission-replay").expect("valid request ID"),
            RequestKind::Permission,
            4,
        )
        .expect("valid request");
        let request_source = RequestAttentionSource::new(
            &request,
            source_event("request-replay-event"),
            AttentionSeverity::ActionRequired,
            at(121_000),
            AttentionEvidence::new(vec![evidence("request-replay-evidence")], None)
                .expect("valid request evidence"),
        )
        .expect("valid request source");
        let request_plan = plan_request_attention(
            &attention,
            4,
            &request,
            &request_source,
            RequestAttentionObservation::new(
                source_event("request-replay-observed"),
                at(121_000),
                evidence("request-replay-observed"),
            ),
        )
        .expect("request plan");
        let stuck = StuckAttentionAssessment::new(
            4,
            &lifecycle,
            &activity,
            StuckAssessment::Clear(StuckClearReason::BlockingRequestOpen),
            None,
        )
        .expect("stuck binding");
        sync_run_attention(
            &mut attention,
            4,
            &activity,
            LifecycleAttentionUpdate::new(&lifecycle, &event, disposition),
            Some(request_plan),
            &stuck,
            observation("replay-batch", 121_000),
        )
        .expect("combined batch applies");
        attention
    }

    let incremental = reduce();
    let replayed = reduce();
    assert_eq!(replayed, incremental);
    assert_eq!(replayed.version(), 4);
    assert_eq!(replayed.items().len(), 2);
}

#[test]
fn ignored_lifecycle_event_advances_version_without_duplicate_attention() {
    let (mut lifecycle, mut attention, _) = terminal_case(LifecycleEvent::RunFailed);
    let duplicate = LifecycleEvent::RunFailed;
    let disposition = lifecycle
        .apply(4, duplicate.clone())
        .expect("duplicate is ordered");
    assert!(matches!(disposition, LifecycleDisposition::Ignored(_)));
    assert_eq!(
        sync_lifecycle_attention(
            &mut attention,
            4,
            &lifecycle,
            &duplicate,
            disposition,
            observation("duplicate", 30),
        )
        .expect("ignored event advances"),
        Vec::<AttentionDisposition>::new()
    );
    assert_eq!(attention.version(), 4);
    assert_eq!(attention.items().len(), 1);
}

#[test]
fn lifecycle_mismatch_and_identity_collision_fail_before_attention_mutation() {
    let (mut lifecycle, mut attention) = running_projections();
    let before = attention.clone();
    assert_eq!(
        sync_lifecycle_attention(
            &mut attention,
            3,
            &lifecycle,
            &LifecycleEvent::RunCompleted,
            LifecycleDisposition::Applied,
            observation("bad-sequence", 20),
        ),
        Err(RunAttentionError::LifecycleIngestSequenceMismatch {
            lifecycle: 2,
            received: 3,
        })
    );
    assert_eq!(attention, before);

    lifecycle
        .apply(3, LifecycleEvent::RunEventObserved)
        .expect("version advances");
    assert_eq!(
        sync_lifecycle_attention(
            &mut attention,
            3,
            &lifecycle,
            &LifecycleEvent::RunCompleted,
            LifecycleDisposition::Applied,
            observation("bad-state", 20),
        ),
        Err(RunAttentionError::LifecycleDispositionMismatch)
    );
    assert_eq!(attention, before);

    let collision_source = source_event("event-terminal");
    attention
        .apply(
            3,
            AttentionEvent::Opened(
                AttentionItemDraft::new(
                    AttentionItemId::new("lifecycle:14:event-terminal").expect("valid item"),
                    collision_source,
                    AttentionCategory::Failure,
                    AttentionSeverity::Critical,
                    false,
                    AttentionDedupeKey::new("unrelated-key").expect("valid key"),
                    AttentionEvidence::new(vec![evidence("collision")], None)
                        .expect("valid evidence"),
                    at(20),
                )
                .expect("valid draft"),
            ),
        )
        .expect("collision fixture opens");
    let collision_before = attention.clone();
    let failed = LifecycleEvent::RunFailed;
    let disposition = lifecycle.apply(4, failed.clone()).expect("failure applies");
    assert_eq!(
        sync_lifecycle_attention(
            &mut attention,
            4,
            &lifecycle,
            &failed,
            disposition,
            observation("terminal", 30),
        ),
        Err(RunAttentionError::LifecycleAttentionCollision)
    );
    assert_eq!(attention, collision_before);
}

#[test]
fn stuck_occurrence_opens_once_and_clear_or_new_occurrence_resolves_the_previous_item() {
    let (first_assessment, _) = stuck_assessment(0, 120_000, false);
    let first_occurrence = match &first_assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(reason) => panic!("expected stuck, got {reason:?}"),
    };
    let first_source =
        StuckAttentionSource::new(first_occurrence, source_event("progress-event-1"))
            .expect("valid source");
    let mut attention = AttentionProjection::new(1).expect("valid attention");

    assert_eq!(
        sync_stuck_attention(
            &mut attention,
            2,
            &first_assessment,
            Some(&first_source),
            at(120_000),
            evidence("stuck-observed"),
        )
        .expect("first stuck opens"),
        vec![AttentionDisposition::Applied]
    );
    assert_eq!(
        item(&attention, AttentionCategory::Stuck).status(),
        AttentionStatus::Open
    );

    sync_stuck_attention(
        &mut attention,
        3,
        &first_assessment,
        Some(&first_source),
        at(121_000),
        evidence("stuck-still-current"),
    )
    .expect("same occurrence observes evidence");
    assert_eq!(attention.items().len(), 1);

    let (clear, _) = stuck_assessment(0, 121_000, true);
    sync_stuck_attention(
        &mut attention,
        4,
        &clear,
        None,
        at(121_000),
        evidence("blocking-request-opened"),
    )
    .expect("clear resolves prior stuck item");
    assert_eq!(
        item(&attention, AttentionCategory::Stuck).status(),
        AttentionStatus::Resolved
    );

    let (second_assessment, _) = stuck_assessment(200_000, 320_000, false);
    let second_occurrence = match &second_assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(reason) => panic!("expected stuck, got {reason:?}"),
    };
    let second_source =
        StuckAttentionSource::new(second_occurrence, source_event("progress-event-2"))
            .expect("valid source");
    sync_stuck_attention(
        &mut attention,
        5,
        &second_assessment,
        Some(&second_source),
        at(320_000),
        evidence("second-stuck"),
    )
    .expect("new occurrence opens separately");

    assert_eq!(attention.items().len(), 2);
    assert_ne!(first_source.item_id(), second_source.item_id());
    assert_eq!(
        attention
            .item(second_source.item_id())
            .expect("second item")
            .status(),
        AttentionStatus::Open
    );
}

#[test]
fn changing_directly_to_a_new_stuck_occurrence_resolves_and_opens_atomically() {
    let (first_assessment, _) = stuck_assessment(0, 120_000, false);
    let first = match &first_assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(_) => panic!("expected stuck"),
    };
    let first_source =
        StuckAttentionSource::new(first, source_event("progress-event-1")).expect("source");
    let mut attention = AttentionProjection::new(1).expect("attention");
    sync_stuck_attention(
        &mut attention,
        2,
        &first_assessment,
        Some(&first_source),
        at(120_000),
        evidence("first"),
    )
    .expect("first opens");

    let (second_assessment, _) = stuck_assessment(200_000, 320_000, false);
    let second = match &second_assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(_) => panic!("expected stuck"),
    };
    let second_source =
        StuckAttentionSource::new(second, source_event("progress-event-2")).expect("source");
    assert_eq!(
        sync_stuck_attention(
            &mut attention,
            3,
            &second_assessment,
            Some(&second_source),
            at(320_000),
            evidence("changed"),
        )
        .expect("change applies as one batch"),
        vec![AttentionDisposition::Applied, AttentionDisposition::Applied]
    );
    assert_eq!(attention.version(), 3);
    assert_eq!(
        attention
            .item(first_source.item_id())
            .expect("first")
            .status(),
        AttentionStatus::Resolved
    );
    assert_eq!(
        attention
            .item(second_source.item_id())
            .expect("second")
            .status(),
        AttentionStatus::Open
    );
}

#[test]
fn stuck_source_mismatch_and_non_monotonic_sequence_preserve_attention() {
    let (assessment, _) = stuck_assessment(0, 120_000, false);
    let occurrence = match &assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(_) => panic!("expected stuck"),
    };
    let source = StuckAttentionSource::new(occurrence, source_event("progress-event"))
        .expect("valid source");
    let mut attention = AttentionProjection::new(1).expect("attention");
    let before = attention.clone();
    assert_eq!(
        sync_stuck_attention(
            &mut attention,
            2,
            &assessment,
            None,
            at(120_000),
            evidence("missing-source"),
        ),
        Err(RunAttentionError::StuckSourceMismatch)
    );
    assert_eq!(attention, before);

    sync_stuck_attention(
        &mut attention,
        2,
        &assessment,
        Some(&source),
        at(120_000),
        evidence("opened"),
    )
    .expect("opens");
    let opened = attention.clone();
    assert_eq!(
        sync_stuck_attention(
            &mut attention,
            2,
            &assessment,
            Some(&source),
            at(121_000),
            evidence("duplicate-sequence"),
        ),
        Err(RunAttentionError::Attention(
            AttentionError::NonMonotonicIngestSequence {
                current: 2,
                received: 2,
            }
        ))
    );
    assert_eq!(attention, opened);
}

#[test]
fn repeated_mapping_sequence_replays_to_the_same_attention_projection() {
    fn reduce() -> AttentionProjection {
        let (assessment, _) = stuck_assessment(0, 120_000, false);
        let occurrence = match &assessment {
            StuckAssessment::PossiblyStuck(occurrence) => occurrence,
            StuckAssessment::Clear(_) => panic!("expected stuck"),
        };
        let source = StuckAttentionSource::new(occurrence, source_event("progress-event"))
            .expect("valid source");
        let mut attention = AttentionProjection::new(1).expect("attention");
        sync_stuck_attention(
            &mut attention,
            2,
            &assessment,
            Some(&source),
            at(120_000),
            evidence("opened"),
        )
        .expect("opens");
        let (clear, _) = stuck_assessment(0, 121_000, true);
        sync_stuck_attention(
            &mut attention,
            3,
            &clear,
            None,
            at(121_000),
            evidence("cleared"),
        )
        .expect("resolves");
        attention
    }

    assert_eq!(reduce(), reduce());
}

#[test]
fn stale_clear_and_old_occurrence_cannot_resolve_a_newer_stuck_item() {
    let (mut lifecycle, mut activity, mut attention, stuck_projection, first_source) =
        active_stuck_state();
    let old_lifecycle = lifecycle.clone();
    let old_activity = activity.clone();
    let old_clear = StuckAttentionAssessment::new(
        3,
        &old_lifecycle,
        &old_activity,
        StuckAssessment::Clear(StuckClearReason::BlockingRequestOpen),
        None,
    )
    .expect("old clear binding");
    let old_occurrence = match stuck_projection
        .assess(
            at(120_000),
            &StuckContext::from_projections(
                &old_lifecycle,
                &old_activity,
                ProcessState::Alive,
                false,
            ),
            StuckPolicy::default(),
        )
        .expect("old assessment")
    {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(reason) => panic!("expected old stuck, got {reason:?}"),
    };
    let old_source = StuckAttentionSource::new(&old_occurrence, source_event("progress-event"))
        .expect("old source");
    let old_stuck = StuckAttentionAssessment::new(
        3,
        &old_lifecycle,
        &old_activity,
        StuckAssessment::PossiblyStuck(old_occurrence),
        Some(old_source),
    )
    .expect("old stuck binding");

    let progress = LifecycleEvent::RunEventObserved;
    let progress_disposition = lifecycle
        .apply(4, progress.clone())
        .expect("progress run event applies");
    activity
        .apply(
            4,
            at(200_000),
            ActivityEvent::MeaningfulProgress {
                kind: flit_core::activity::ProgressKind::FileContentChanged,
                evidence_id: evidence("new-progress"),
            },
        )
        .expect("new progress applies");
    let second_context = StuckContext::from_projections(
        &lifecycle,
        &activity,
        ProcessState::Alive,
        attention.has_active_blocking_request(),
    );
    let second_assessment = stuck_projection
        .assess(at(320_000), &second_context, StuckPolicy::default())
        .expect("second assessment");
    let second_occurrence = match &second_assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(reason) => panic!("expected second stuck, got {reason:?}"),
    };
    let second_source =
        StuckAttentionSource::new(second_occurrence, source_event("new-progress-event"))
            .expect("second source");
    let second_stuck = StuckAttentionAssessment::new(
        4,
        &lifecycle,
        &activity,
        second_assessment,
        Some(second_source.clone()),
    )
    .expect("second binding");
    sync_run_attention(
        &mut attention,
        4,
        &activity,
        LifecycleAttentionUpdate::new(&lifecycle, &progress, progress_disposition),
        None,
        &second_stuck,
        observation("second-stuck", 320_000),
    )
    .expect("second occurrence replaces first");
    assert_eq!(
        attention
            .item(first_source.item_id())
            .expect("first")
            .status(),
        AttentionStatus::Resolved
    );
    assert_eq!(
        attention
            .item(second_source.item_id())
            .expect("second")
            .status(),
        AttentionStatus::Open
    );

    let current_event = LifecycleEvent::RunEventObserved;
    let current_disposition = lifecycle
        .apply(5, current_event.clone())
        .expect("current run event applies");
    activity
        .apply(
            5,
            at(321_000),
            ActivityEvent::LivenessObserved {
                evidence_id: evidence("current-tick"),
            },
        )
        .expect("current liveness applies");
    let protected = attention.clone();
    for stale in [&old_clear, &old_stuck] {
        assert_eq!(
            sync_run_attention(
                &mut attention,
                5,
                &activity,
                LifecycleAttentionUpdate::new(&lifecycle, &current_event, current_disposition,),
                None,
                stale,
                observation("delayed", 321_000),
            ),
            Err(RunAttentionError::StuckAssessmentIngestSequenceMismatch {
                assessment: 3,
                received: 5,
            })
        );
        assert_eq!(attention, protected);
    }

    let exact_clear = StuckAttentionAssessment::new(
        5,
        &lifecycle,
        &activity,
        StuckAssessment::Clear(StuckClearReason::BlockingRequestOpen),
        None,
    )
    .expect("exact clear binding");
    sync_run_attention(
        &mut attention,
        5,
        &activity,
        LifecycleAttentionUpdate::new(&lifecycle, &current_event, current_disposition),
        None,
        &exact_clear,
        observation("exact-clear", 321_000),
    )
    .expect("exact current clear resolves second");
    assert_eq!(
        attention
            .item(second_source.item_id())
            .expect("second")
            .status(),
        AttentionStatus::Resolved
    );
}
