use flit_core::{
    activity::{
        Activity, ActivityEvent, ActivityProjection, ActivitySignal, EvidenceId, ScoreFactor,
        SignalSource, TimestampMs, WaitKind,
    },
    lifecycle::{LifecycleEvent, LifecycleProjection, RunLifecycle, SessionId},
    stuck::{
        ProcessState, StuckActionDisposition, StuckActionIgnoredReason, StuckAssessment,
        StuckCause, StuckClearReason, StuckContext, StuckError, StuckNotificationState,
        StuckPolicy, StuckProjection, StuckThresholdSeconds,
    },
};

fn at(value: u64) -> TimestampMs {
    TimestampMs::new(value)
}

fn evidence(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("test evidence identifier must be valid")
}

fn context(
    lifecycle: RunLifecycle,
    activity: Activity,
    wait_kind: Option<WaitKind>,
    process_state: ProcessState,
    blocking: bool,
    progress_at: u64,
    progress_evidence_id: &str,
) -> StuckContext {
    StuckContext::new(
        lifecycle,
        activity,
        wait_kind,
        process_state,
        blocking,
        at(progress_at),
        evidence(progress_evidence_id),
    )
    .expect("test stuck context must be valid")
}

fn running(activity: Activity, wait_kind: Option<WaitKind>) -> StuckContext {
    context(
        RunLifecycle::Running,
        activity,
        wait_kind,
        ProcessState::Alive,
        false,
        0,
        "progress",
    )
}

fn occurrence(assessment: StuckAssessment) -> flit_core::stuck::StuckOccurrence {
    match assessment {
        StuckAssessment::PossiblyStuck(occurrence) => occurrence,
        StuckAssessment::Clear(reason) => panic!("expected Possibly Stuck, got {reason:?}"),
    }
}

#[test]
fn threshold_and_context_value_types_reject_invalid_boundaries() {
    assert_eq!(
        StuckThresholdSeconds::new(29),
        Err(StuckError::ThresholdOutOfRange(29))
    );
    assert_eq!(
        StuckThresholdSeconds::new(1_801),
        Err(StuckError::ThresholdOutOfRange(1_801))
    );
    assert_eq!(
        StuckThresholdSeconds::new(30)
            .expect("lower boundary")
            .as_u16(),
        30
    );
    assert_eq!(
        StuckThresholdSeconds::new(1_800)
            .expect("upper boundary")
            .as_u16(),
        1_800
    );

    let defaults = StuckPolicy::default();
    assert_eq!(defaults.starting().as_u16(), 30);
    assert_eq!(defaults.regular_activity().as_u16(), 120);
    assert_eq!(defaults.long_running_activity().as_u16(), 300);
    assert_eq!(defaults.unstructured_wait().as_u16(), 300);

    assert_eq!(
        StuckContext::new(
            RunLifecycle::Running,
            Activity::Waiting,
            None,
            ProcessState::Alive,
            false,
            at(0),
            evidence("missing-wait-kind"),
        ),
        Err(StuckError::InvalidWaitKind)
    );
    assert_eq!(
        StuckContext::new(
            RunLifecycle::Running,
            Activity::Planning,
            Some(WaitKind::Unstructured),
            ProcessState::Alive,
            false,
            at(0),
            evidence("unexpected-wait-kind"),
        ),
        Err(StuckError::InvalidWaitKind)
    );
}

#[test]
fn default_deadlines_use_exact_inclusive_boundaries() {
    let policy = StuckPolicy::default();
    let projection = StuckProjection::new();
    let cases = [
        (
            context(
                RunLifecycle::Starting,
                Activity::Unknown,
                None,
                ProcessState::NotSpawned,
                false,
                1_000,
                "created",
            ),
            StuckCause::Starting,
            30_u64,
        ),
        (
            running(Activity::Planning, None),
            StuckCause::Activity(Activity::Planning),
            120,
        ),
        (
            running(Activity::Reading, None),
            StuckCause::Activity(Activity::Reading),
            120,
        ),
        (
            running(Activity::Editing, None),
            StuckCause::Activity(Activity::Editing),
            120,
        ),
        (
            running(Activity::Reviewing, None),
            StuckCause::Activity(Activity::Reviewing),
            120,
        ),
        (
            running(Activity::Unknown, None),
            StuckCause::Activity(Activity::Unknown),
            120,
        ),
        (
            running(Activity::Testing, None),
            StuckCause::Activity(Activity::Testing),
            300,
        ),
        (
            running(Activity::Building, None),
            StuckCause::Activity(Activity::Building),
            300,
        ),
        (
            running(Activity::Waiting, Some(WaitKind::Unstructured)),
            StuckCause::Activity(Activity::Waiting),
            300,
        ),
    ];

    for (context, expected_cause, deadline_seconds) in cases {
        let deadline_at = context.last_progress_at().as_u64() + deadline_seconds * 1_000;
        assert_eq!(
            projection
                .assess(at(deadline_at - 1), &context, policy)
                .expect("valid assessment"),
            StuckAssessment::Clear(StuckClearReason::WithinDeadline {
                cause: expected_cause,
                deadline_at: at(deadline_at),
            })
        );
        let occurrence = occurrence(
            projection
                .assess(at(deadline_at), &context, policy)
                .expect("valid assessment"),
        );
        assert_eq!(occurrence.id().cause(), expected_cause);
        assert_eq!(occurrence.id().stuck_since(), at(deadline_at));
        assert_eq!(occurrence.id().baseline_at(), context.last_progress_at());
        assert_eq!(
            occurrence.notification(),
            StuckNotificationState::NotDue {
                due_at: at(deadline_at + 300_000),
            }
        );
    }
}

#[test]
fn custom_policy_is_applied_per_deadline_class() {
    let policy = StuckPolicy::new(
        StuckThresholdSeconds::new(31).expect("valid threshold"),
        StuckThresholdSeconds::new(121).expect("valid threshold"),
        StuckThresholdSeconds::new(301).expect("valid threshold"),
        StuckThresholdSeconds::new(302).expect("valid threshold"),
    );
    let projection = StuckProjection::new();
    let cases = [
        (
            context(
                RunLifecycle::Starting,
                Activity::Unknown,
                None,
                ProcessState::Alive,
                false,
                0,
                "created",
            ),
            31_000,
        ),
        (running(Activity::Planning, None), 121_000),
        (running(Activity::Testing, None), 301_000),
        (
            running(Activity::Waiting, Some(WaitKind::Unstructured)),
            302_000,
        ),
    ];

    for (context, deadline_at) in cases {
        assert!(matches!(
            projection
                .assess(at(deadline_at - 1), &context, policy)
                .expect("valid assessment"),
            StuckAssessment::Clear(StuckClearReason::WithinDeadline { .. })
        ));
        assert!(matches!(
            projection
                .assess(at(deadline_at), &context, policy)
                .expect("valid assessment"),
            StuckAssessment::PossiblyStuck(_)
        ));
    }
}

#[test]
fn ineligible_lifecycle_process_request_and_structured_wait_fail_closed() {
    let policy = StuckPolicy::default();
    let projection = StuckProjection::new();

    for lifecycle in [
        RunLifecycle::Finished,
        RunLifecycle::Failed,
        RunLifecycle::Stopped,
        RunLifecycle::Interrupted,
    ] {
        assert_eq!(
            projection
                .assess(
                    at(1_000_000),
                    &context(
                        lifecycle,
                        Activity::Unknown,
                        None,
                        ProcessState::Unavailable,
                        false,
                        0,
                        "terminal",
                    ),
                    policy,
                )
                .expect("valid assessment"),
            StuckAssessment::Clear(StuckClearReason::LifecycleInactive)
        );
    }

    assert_eq!(
        projection
            .assess(
                at(1_000_000),
                &context(
                    RunLifecycle::Running,
                    Activity::Planning,
                    None,
                    ProcessState::Alive,
                    true,
                    0,
                    "blocking",
                ),
                policy,
            )
            .expect("valid assessment"),
        StuckAssessment::Clear(StuckClearReason::BlockingRequestOpen)
    );

    for (lifecycle, process_state) in [
        (RunLifecycle::Starting, ProcessState::Unavailable),
        (RunLifecycle::Running, ProcessState::Unavailable),
        (RunLifecycle::Running, ProcessState::NotSpawned),
    ] {
        assert_eq!(
            projection
                .assess(
                    at(1_000_000),
                    &context(
                        lifecycle,
                        Activity::Planning,
                        None,
                        process_state,
                        false,
                        0,
                        "process",
                    ),
                    policy,
                )
                .expect("valid assessment"),
            StuckAssessment::Clear(StuckClearReason::ProcessUnavailable)
        );
    }

    for wait_kind in [
        WaitKind::BlockingRequest,
        WaitKind::External,
        WaitKind::Service,
    ] {
        assert_eq!(
            projection
                .assess(
                    at(1_000_000),
                    &running(Activity::Waiting, Some(wait_kind)),
                    policy,
                )
                .expect("valid assessment"),
            StuckAssessment::Clear(StuckClearReason::StructuredWait(wait_kind))
        );
    }
}

#[test]
fn still_working_resets_deadline_and_suppresses_same_cause_for_ten_minutes() {
    let policy = StuckPolicy::default();
    let context = running(Activity::Planning, None);
    let mut projection = StuckProjection::new();

    assert_eq!(
        projection
            .still_working(at(119_999), &context, policy)
            .expect("valid action"),
        StuckActionDisposition::Ignored(StuckActionIgnoredReason::NotCurrentlyStuck)
    );
    assert_eq!(
        projection
            .still_working(at(120_000), &context, policy)
            .expect("valid action"),
        StuckActionDisposition::Applied
    );
    assert_eq!(
        projection
            .assess(at(239_999), &context, policy)
            .expect("valid assessment"),
        StuckAssessment::Clear(StuckClearReason::WithinDeadline {
            cause: StuckCause::Activity(Activity::Planning),
            deadline_at: at(240_000),
        })
    );

    let at_new_deadline = occurrence(
        projection
            .assess(at(240_000), &context, policy)
            .expect("valid assessment"),
    );
    assert_eq!(at_new_deadline.id().baseline_at(), at(120_000));
    assert_eq!(
        at_new_deadline.notification(),
        StuckNotificationState::NotDue {
            due_at: at(540_000),
        }
    );
    for now in [540_000, 719_999] {
        assert_eq!(
            occurrence(
                projection
                    .assess(at(now), &context, policy)
                    .expect("valid assessment"),
            )
            .notification(),
            StuckNotificationState::Suppressed { until: at(720_000) }
        );
    }
    let suppressed_id = occurrence(
        projection
            .assess(at(540_000), &context, policy)
            .expect("valid assessment"),
    )
    .id()
    .clone();
    assert_eq!(
        projection
            .notification_delivered(at(540_000), &context, policy, &suppressed_id)
            .expect("valid suppressed delivery attempt"),
        StuckActionDisposition::Ignored(StuckActionIgnoredReason::NotificationNotDue)
    );
    assert_eq!(
        occurrence(
            projection
                .assess(at(720_000), &context, policy)
                .expect("valid assessment"),
        )
        .notification(),
        StuckNotificationState::Due
    );
}

#[test]
fn notification_is_due_once_and_new_progress_creates_a_new_occurrence() {
    let policy = StuckPolicy::default();
    let initial = running(Activity::Planning, None);
    let mut projection = StuckProjection::new();

    let initial_occurrence = occurrence(
        projection
            .assess(at(120_000), &initial, policy)
            .expect("valid assessment"),
    );
    let initial_id = initial_occurrence.id().clone();
    assert_eq!(
        projection
            .notification_delivered(at(419_999), &initial, policy, &initial_id)
            .expect("valid action"),
        StuckActionDisposition::Ignored(StuckActionIgnoredReason::NotificationNotDue)
    );
    assert_eq!(
        occurrence(
            projection
                .assess(at(420_000), &initial, policy)
                .expect("valid assessment"),
        )
        .notification(),
        StuckNotificationState::Due
    );
    assert_eq!(
        projection
            .notification_delivered(at(420_000), &initial, policy, &initial_id)
            .expect("valid action"),
        StuckActionDisposition::Applied
    );
    assert_eq!(
        occurrence(
            projection
                .assess(at(420_000), &initial, policy)
                .expect("valid assessment"),
        )
        .notification(),
        StuckNotificationState::Delivered
    );
    assert_eq!(
        projection
            .notification_delivered(at(420_001), &initial, policy, &initial_id)
            .expect("valid action"),
        StuckActionDisposition::Ignored(StuckActionIgnoredReason::NotificationAlreadyDelivered)
    );

    let progressed = context(
        RunLifecycle::Running,
        Activity::Planning,
        None,
        ProcessState::Alive,
        false,
        500_000,
        "new-progress",
    );
    assert_eq!(
        projection
            .assess(at(619_999), &progressed, policy)
            .expect("valid assessment"),
        StuckAssessment::Clear(StuckClearReason::WithinDeadline {
            cause: StuckCause::Activity(Activity::Planning),
            deadline_at: at(620_000),
        })
    );
    let new_occurrence = occurrence(
        projection
            .assess(at(920_000), &progressed, policy)
            .expect("valid assessment"),
    );
    assert_eq!(new_occurrence.id().progress_at(), at(500_000));
    assert_eq!(
        new_occurrence.id().progress_evidence_id().as_str(),
        "new-progress"
    );
    assert_eq!(new_occurrence.notification(), StuckNotificationState::Due);

    assert_eq!(
        projection
            .notification_delivered(at(920_000), &progressed, policy, &initial_id)
            .expect("valid stale receipt"),
        StuckActionDisposition::Ignored(StuckActionIgnoredReason::NotificationOccurrenceMismatch)
    );
    assert_eq!(
        occurrence(
            projection
                .assess(at(920_000), &progressed, policy)
                .expect("valid assessment"),
        )
        .notification(),
        StuckNotificationState::Due
    );
    assert_eq!(
        projection
            .notification_delivered(at(920_000), &progressed, policy, new_occurrence.id())
            .expect("valid matching receipt"),
        StuckActionDisposition::Applied
    );
    assert_eq!(
        occurrence(
            projection
                .assess(at(920_000), &progressed, policy)
                .expect("valid assessment"),
        )
        .notification(),
        StuckNotificationState::Delivered
    );
}

#[test]
fn new_progress_invalidates_a_still_working_reset_immediately() {
    let policy = StuckPolicy::default();
    let initial = running(Activity::Planning, None);
    let mut projection = StuckProjection::new();
    projection
        .still_working(at(120_000), &initial, policy)
        .expect("valid reset");

    let progressed = context(
        RunLifecycle::Running,
        Activity::Planning,
        None,
        ProcessState::Alive,
        false,
        130_000,
        "new-progress",
    );
    assert_eq!(
        projection
            .assess(at(249_999), &progressed, policy)
            .expect("valid assessment"),
        StuckAssessment::Clear(StuckClearReason::WithinDeadline {
            cause: StuckCause::Activity(Activity::Planning),
            deadline_at: at(250_000),
        })
    );
    let occurrence = occurrence(
        projection
            .assess(at(250_000), &progressed, policy)
            .expect("valid assessment"),
    );
    assert_eq!(occurrence.id().baseline_at(), at(130_000));
    assert_eq!(
        occurrence.notification(),
        StuckNotificationState::NotDue {
            due_at: at(550_000),
        }
    );
}

#[test]
fn projection_context_uses_lifecycle_and_activity_progress_without_io() {
    let mut lifecycle = LifecycleProjection::new(1).expect("valid lifecycle");
    lifecycle
        .apply(
            2,
            LifecycleEvent::SessionConnected {
                session_id: SessionId::new("session-1").expect("valid session"),
            },
        )
        .expect("ordered lifecycle event");
    let mut activity =
        ActivityProjection::new(1, at(0), evidence("created")).expect("valid activity projection");
    activity
        .apply(
            2,
            at(1_000),
            ActivityEvent::Signal(
                ActivitySignal::new(
                    Activity::Testing,
                    SignalSource::StructuredActivity,
                    ScoreFactor::new(1_000).expect("valid factor"),
                    ScoreFactor::new(1_000).expect("valid factor"),
                    evidence("testing"),
                    None,
                )
                .expect("valid signal"),
            ),
        )
        .expect("ordered activity event");

    let context = StuckContext::from_projections(&lifecycle, &activity, ProcessState::Alive, false);
    assert_eq!(context.lifecycle(), RunLifecycle::Running);
    assert_eq!(context.activity(), Activity::Testing);
    assert_eq!(context.wait_kind(), None);
    assert_eq!(context.process_state(), ProcessState::Alive);
    assert!(!context.has_open_blocking_request());
    assert_eq!(context.last_progress_at(), at(1_000));
    assert_eq!(context.last_progress_evidence_id().as_str(), "testing");
    assert!(matches!(
        StuckProjection::new()
            .assess(at(301_000), &context, StuckPolicy::default())
            .expect("valid assessment"),
        StuckAssessment::PossiblyStuck(_)
    ));
}

#[test]
fn non_monotonic_and_overflow_errors_preserve_projection_state() {
    let policy = StuckPolicy::default();
    let initial = running(Activity::Planning, None);
    let mut projection = StuckProjection::new();
    projection
        .still_working(at(120_000), &initial, policy)
        .expect("valid reset");
    let after_reset = projection.clone();

    assert_eq!(
        projection.assess(at(119_999), &initial, policy),
        Err(StuckError::NonMonotonicTime {
            current: at(120_000),
            received: at(119_999),
        })
    );
    assert_eq!(projection, after_reset);
    assert_eq!(
        projection.still_working(at(119_999), &initial, policy),
        Err(StuckError::NonMonotonicTime {
            current: at(120_000),
            received: at(119_999),
        })
    );
    assert_eq!(projection, after_reset);

    let future_progress = context(
        RunLifecycle::Running,
        Activity::Planning,
        None,
        ProcessState::Alive,
        false,
        130_000,
        "future-progress",
    );
    assert_eq!(
        projection.assess(at(129_999), &future_progress, policy),
        Err(StuckError::NonMonotonicTime {
            current: at(130_000),
            received: at(129_999),
        })
    );
    assert_eq!(projection, after_reset);

    let overflow_base = u64::MAX - 10_000;
    let overflow_context = context(
        RunLifecycle::Running,
        Activity::Planning,
        None,
        ProcessState::Alive,
        false,
        overflow_base,
        "overflow",
    );
    let fresh = StuckProjection::new();
    assert_eq!(
        fresh.assess(at(overflow_base), &overflow_context, policy),
        Err(StuckError::TimestampOverflow {
            base: at(overflow_base),
            seconds: 120,
        })
    );
    assert_eq!(fresh, StuckProjection::new());
}

#[test]
fn replaying_the_same_ordered_actions_produces_the_same_projection() {
    fn replay() -> StuckProjection {
        let policy = StuckPolicy::default();
        let context = running(Activity::Planning, None);
        let mut projection = StuckProjection::new();
        assert_eq!(
            projection
                .still_working(at(120_000), &context, policy)
                .expect("valid reset"),
            StuckActionDisposition::Applied
        );
        let occurrence_id = occurrence(
            projection
                .assess(at(720_000), &context, policy)
                .expect("valid due assessment"),
        )
        .id()
        .clone();
        assert_eq!(
            projection
                .notification_delivered(at(720_000), &context, policy, &occurrence_id)
                .expect("valid notification receipt"),
            StuckActionDisposition::Applied
        );
        projection
    }

    let first = replay();
    let second = replay();
    assert_eq!(first, second);
}
