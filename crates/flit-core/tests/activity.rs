use flit_core::activity::{
    Activity, ActivityDisposition, ActivityError, ActivityEvent, ActivityProjection,
    ActivitySignal, ActivityValueError, EvidenceId, ProgressKind, ScoreFactor, SignalSource,
    TimedEvidence, TimestampMs, WaitKind, replay_activity,
};

fn at(value: u64) -> TimestampMs {
    TimestampMs::new(value)
}

fn evidence(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("test evidence identifier must be valid")
}

fn factor(value: u16) -> ScoreFactor {
    ScoreFactor::new(value).expect("test score factor must be valid")
}

fn signal(
    activity: Activity,
    source: SignalSource,
    recency: u16,
    specificity: u16,
    evidence_id: &str,
    wait_kind: Option<WaitKind>,
) -> ActivityEvent {
    ActivityEvent::Signal(
        ActivitySignal::new(
            activity,
            source,
            factor(recency),
            factor(specificity),
            evidence(evidence_id),
            wait_kind,
        )
        .expect("test activity signal must be valid"),
    )
}

fn projection() -> ActivityProjection {
    ActivityProjection::new(1, at(0), evidence("run-created"))
        .expect("initial projection must be valid")
}

fn assert_timed_evidence(actual: &TimedEvidence, observed_at: u64, evidence_id: &str) {
    assert_eq!(actual.observed_at(), at(observed_at));
    assert_eq!(actual.evidence_id().as_str(), evidence_id);
}

fn activity_evidence(projection: &ActivityProjection) -> Vec<&str> {
    projection
        .activity_evidence_ids()
        .iter()
        .map(EvidenceId::as_str)
        .collect()
}

#[test]
fn value_types_and_fixed_scores_enforce_the_domain_boundaries() {
    assert_eq!(factor(0).as_milli(), 0);
    assert_eq!(factor(1_000).as_milli(), 1_000);
    assert_eq!(
        ScoreFactor::new(1_001),
        Err(ActivityValueError::ScoreFactorOutOfRange(1_001))
    );
    assert_eq!(
        EvidenceId::new(" \n\t"),
        Err(ActivityValueError::BlankEvidenceId)
    );
    assert_eq!(
        ActivityProjection::new(0, at(0), evidence("run-created")),
        Err(ActivityError::InvalidInitialIngestSequence)
    );

    for score in [800, 900] {
        assert_eq!(
            ActivitySignal::new(
                Activity::Unknown,
                SignalSource::StructuredActivity,
                factor(1_000),
                factor(score),
                evidence("unknown-signal"),
                None,
            ),
            Err(ActivityValueError::InvalidUnknownSignal)
        );
    }

    let source_scores = [
        (SignalSource::StructuredActivity, 1_000),
        (SignalSource::BlockingRequest, 950),
        (SignalSource::KnownCommand, 850),
        (SignalSource::TestBuildMarker, 750),
        (SignalSource::FileChangePattern, 550),
        (SignalSource::UnstructuredProviderText, 400),
    ];
    for (source, expected) in source_scores {
        assert_eq!(source.base_reliability().as_milli(), expected);
    }

    let scored = ActivitySignal::new(
        Activity::Testing,
        SignalSource::KnownCommand,
        factor(800),
        factor(900),
        evidence("score"),
        None,
    )
    .expect("valid signal");
    assert_eq!(scored.score().as_milli(), 612);

    assert_eq!(
        ActivitySignal::new(
            Activity::Waiting,
            SignalSource::StructuredActivity,
            factor(1_000),
            factor(1_000),
            evidence("missing-wait-kind"),
            None,
        ),
        Err(ActivityValueError::InvalidWaitKind)
    );
    assert_eq!(
        ActivitySignal::new(
            Activity::Planning,
            SignalSource::StructuredActivity,
            factor(1_000),
            factor(1_000),
            evidence("unexpected-wait-kind"),
            Some(WaitKind::Unstructured),
        ),
        Err(ActivityValueError::InvalidWaitKind)
    );
    assert_eq!(
        ActivitySignal::new(
            Activity::Waiting,
            SignalSource::BlockingRequest,
            factor(1_000),
            factor(1_000),
            evidence("wrong-blocking-kind"),
            Some(WaitKind::External),
        ),
        Err(ActivityValueError::InvalidBlockingRequestSignal)
    );
}

#[test]
fn exact_score_thresholds_are_deterministic() {
    let cases = [
        (
            900,
            ActivityDisposition::ActivityChanged,
            Activity::Planning,
            Some(900),
        ),
        (
            899,
            ActivityDisposition::PendingCorroboration,
            Activity::Unknown,
            None,
        ),
        (
            700,
            ActivityDisposition::PendingCorroboration,
            Activity::Unknown,
            None,
        ),
        (
            699,
            ActivityDisposition::ObservedOnly,
            Activity::Unknown,
            None,
        ),
    ];

    for (score, expected_disposition, expected_activity, expected_confidence) in cases {
        let mut projection = projection();
        assert_eq!(
            projection
                .apply(
                    2,
                    at(10),
                    signal(
                        Activity::Planning,
                        SignalSource::StructuredActivity,
                        1_000,
                        score,
                        "threshold",
                        None,
                    ),
                )
                .expect("ordered signal"),
            expected_disposition
        );
        assert_eq!(projection.activity(), expected_activity);
        assert_eq!(
            projection.confidence().map(|value| value.as_milli()),
            expected_confidence
        );
        assert_timed_evidence(projection.last_liveness(), 10, "threshold");
        if score >= 700 {
            assert_timed_evidence(projection.last_meaningful_signal(), 10, "threshold");
        } else {
            assert_timed_evidence(projection.last_meaningful_signal(), 0, "run-created");
        }
    }
}

#[test]
fn medium_confidence_requires_matching_independent_sources_within_500ms() {
    let mut corroborated = projection();
    assert_eq!(
        corroborated
            .apply(
                2,
                at(100),
                signal(
                    Activity::Reading,
                    SignalSource::KnownCommand,
                    1_000,
                    1_000,
                    "known-command",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::PendingCorroboration
    );
    assert_eq!(
        corroborated
            .apply(
                3,
                at(600),
                signal(
                    Activity::Reading,
                    SignalSource::StructuredActivity,
                    800,
                    1_000,
                    "structured",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::ActivityChanged
    );
    assert_eq!(corroborated.activity(), Activity::Reading);
    assert_eq!(
        corroborated.confidence().map(|score| score.as_milli()),
        Some(850)
    );
    assert_eq!(
        activity_evidence(&corroborated),
        vec!["known-command", "structured"]
    );

    let mut same_source = projection();
    for (sequence, time, evidence_id) in [(2, 100, "same-1"), (3, 200, "same-2")] {
        assert_eq!(
            same_source
                .apply(
                    sequence,
                    at(time),
                    signal(
                        Activity::Reading,
                        SignalSource::KnownCommand,
                        1_000,
                        1_000,
                        evidence_id,
                        None,
                    ),
                )
                .expect("ordered signal"),
            ActivityDisposition::PendingCorroboration
        );
    }
    assert_eq!(same_source.activity(), Activity::Unknown);

    let mut outside_window = projection();
    outside_window
        .apply(
            2,
            at(100),
            signal(
                Activity::Reading,
                SignalSource::KnownCommand,
                1_000,
                1_000,
                "outside-1",
                None,
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        outside_window
            .apply(
                3,
                at(601),
                signal(
                    Activity::Reading,
                    SignalSource::StructuredActivity,
                    800,
                    1_000,
                    "outside-2",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::PendingCorroboration
    );
    assert_eq!(outside_window.activity(), Activity::Unknown);

    let mut different_activity = projection();
    different_activity
        .apply(
            2,
            at(100),
            signal(
                Activity::Reading,
                SignalSource::KnownCommand,
                1_000,
                1_000,
                "different-1",
                None,
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        different_activity
            .apply(
                3,
                at(200),
                signal(
                    Activity::Editing,
                    SignalSource::StructuredActivity,
                    800,
                    1_000,
                    "different-2",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::PendingCorroboration
    );
    assert_eq!(different_activity.activity(), Activity::Unknown);

    let mut interleaved = projection();
    interleaved
        .apply(
            2,
            at(100),
            signal(
                Activity::Reading,
                SignalSource::KnownCommand,
                1_000,
                1_000,
                "reading-known",
                None,
            ),
        )
        .expect("ordered signal");
    interleaved
        .apply(
            3,
            at(200),
            signal(
                Activity::Editing,
                SignalSource::StructuredActivity,
                800,
                1_000,
                "editing-structured",
                None,
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        interleaved
            .apply(
                4,
                at(300),
                signal(
                    Activity::Reading,
                    SignalSource::TestBuildMarker,
                    1_000,
                    1_000,
                    "reading-marker",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::ActivityChanged
    );
    assert_eq!(interleaved.activity(), Activity::Reading);
    assert_eq!(
        activity_evidence(&interleaved),
        vec!["reading-known", "reading-marker"]
    );
}

#[test]
fn hysteresis_defers_normal_changes_and_blocking_requests_bypass_in_either_order() {
    let mut normal_change = projection();
    normal_change
        .apply(
            2,
            at(100),
            signal(
                Activity::Planning,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "planning",
                None,
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        normal_change
            .apply(
                3,
                at(2_099),
                signal(
                    Activity::Editing,
                    SignalSource::StructuredActivity,
                    1_000,
                    1_000,
                    "editing",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::TransitionDeferred
    );
    assert_eq!(normal_change.activity(), Activity::Planning);
    assert_eq!(
        normal_change
            .apply(
                4,
                at(2_099),
                ActivityEvent::Tick {
                    evidence_id: evidence("early-tick"),
                },
            )
            .expect("ordered tick"),
        ActivityDisposition::NoChange
    );
    assert_eq!(
        normal_change
            .apply(
                5,
                at(2_100),
                ActivityEvent::Tick {
                    evidence_id: evidence("boundary-tick"),
                },
            )
            .expect("ordered tick"),
        ActivityDisposition::ActivityChanged
    );
    assert_eq!(normal_change.activity(), Activity::Editing);
    assert_eq!(activity_evidence(&normal_change), vec!["editing"]);
    assert_timed_evidence(normal_change.last_progress(), 2_100, "editing");

    for blocking_first in [true, false] {
        let mut projection = projection();
        projection
            .apply(
                2,
                at(100),
                signal(
                    Activity::Planning,
                    SignalSource::StructuredActivity,
                    1_000,
                    1_000,
                    "planning",
                    None,
                ),
            )
            .expect("ordered signal");

        let blocking = signal(
            Activity::Waiting,
            SignalSource::BlockingRequest,
            900,
            1_000,
            "blocking",
            Some(WaitKind::BlockingRequest),
        );
        let corroborating = signal(
            Activity::Waiting,
            SignalSource::StructuredActivity,
            800,
            1_000,
            "corroborating",
            Some(WaitKind::Unstructured),
        );
        let (first, second) = if blocking_first {
            (blocking, corroborating)
        } else {
            (corroborating, blocking)
        };
        assert_eq!(
            projection.apply(3, at(200), first).expect("ordered signal"),
            ActivityDisposition::PendingCorroboration
        );
        assert_eq!(
            projection
                .apply(4, at(300), second)
                .expect("ordered signal"),
            ActivityDisposition::ActivityChanged
        );
        assert_eq!(projection.activity(), Activity::Waiting);
        assert_eq!(projection.wait_kind(), Some(WaitKind::BlockingRequest));
        assert_eq!(
            projection
                .apply(
                    5,
                    at(60_300),
                    ActivityEvent::Tick {
                        evidence_id: evidence("blocking-hold"),
                    },
                )
                .expect("ordered tick"),
            ActivityDisposition::NoChange
        );
        assert_eq!(projection.activity(), Activity::Waiting);
    }
}

#[test]
fn low_confidence_liveness_and_same_activity_do_not_create_false_progress() {
    let mut projection = projection();
    projection
        .apply(
            2,
            at(100),
            signal(
                Activity::Planning,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "planning",
                None,
            ),
        )
        .expect("ordered signal");
    assert_timed_evidence(projection.last_progress(), 100, "planning");

    assert_eq!(
        projection
            .apply(
                3,
                at(200),
                signal(
                    Activity::Editing,
                    SignalSource::UnstructuredProviderText,
                    1_000,
                    1_000,
                    "low-confidence",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::ObservedOnly
    );
    assert_timed_evidence(projection.last_meaningful_signal(), 100, "planning");
    assert_timed_evidence(projection.last_progress(), 100, "planning");
    assert_timed_evidence(projection.last_liveness(), 200, "low-confidence");

    assert_eq!(
        projection
            .apply(
                4,
                at(300),
                ActivityEvent::LivenessObserved {
                    evidence_id: evidence("heartbeat"),
                },
            )
            .expect("ordered liveness"),
        ActivityDisposition::LivenessRecorded
    );
    assert_timed_evidence(projection.last_meaningful_signal(), 100, "planning");
    assert_timed_evidence(projection.last_progress(), 100, "planning");
    assert_timed_evidence(projection.last_liveness(), 300, "heartbeat");

    assert_eq!(
        projection
            .apply(
                5,
                at(400),
                signal(
                    Activity::Planning,
                    SignalSource::StructuredActivity,
                    950,
                    1_000,
                    "planning-reinforced",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::ActivityReinforced
    );
    assert_timed_evidence(
        projection.last_meaningful_signal(),
        400,
        "planning-reinforced",
    );
    assert_timed_evidence(projection.last_liveness(), 400, "planning-reinforced");
    assert_timed_evidence(projection.last_progress(), 100, "planning");
    assert_eq!(activity_evidence(&projection), vec!["planning-reinforced"]);
}

#[test]
fn inactivity_downgrades_activity_but_structured_waits_hold_until_progress() {
    let mut active = projection();
    active
        .apply(
            2,
            at(1),
            signal(
                Activity::Editing,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "editing",
                None,
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        active
            .apply(
                3,
                at(60_000),
                ActivityEvent::Tick {
                    evidence_id: evidence("before-timeout"),
                },
            )
            .expect("ordered tick"),
        ActivityDisposition::NoChange
    );
    assert_eq!(
        active
            .apply(
                4,
                at(60_001),
                ActivityEvent::Tick {
                    evidence_id: evidence("timeout"),
                },
            )
            .expect("ordered tick"),
        ActivityDisposition::ActivityChanged
    );
    assert_eq!(active.activity(), Activity::Unknown);
    assert_eq!(active.confidence(), None);
    assert_eq!(activity_evidence(&active), vec!["timeout"]);

    for (wait_kind, source) in [
        (WaitKind::BlockingRequest, SignalSource::BlockingRequest),
        (WaitKind::External, SignalSource::StructuredActivity),
        (WaitKind::Service, SignalSource::StructuredActivity),
    ] {
        let mut waiting = projection();
        waiting
            .apply(
                2,
                at(1),
                signal(
                    Activity::Waiting,
                    source,
                    1_000,
                    1_000,
                    "structured-wait",
                    Some(wait_kind),
                ),
            )
            .expect("ordered signal");
        assert_eq!(
            waiting
                .apply(
                    3,
                    at(60_001),
                    ActivityEvent::Tick {
                        evidence_id: evidence("held-timeout"),
                    },
                )
                .expect("ordered tick"),
            ActivityDisposition::NoChange
        );
        assert_eq!(waiting.activity(), Activity::Waiting);
        assert_eq!(waiting.wait_kind(), Some(wait_kind));
    }

    let mut resumed = projection();
    resumed
        .apply(
            2,
            at(1),
            signal(
                Activity::Waiting,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "external-wait",
                Some(WaitKind::External),
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        resumed
            .apply(
                3,
                at(60_002),
                ActivityEvent::MeaningfulProgress {
                    kind: ProgressKind::AgentOutputResumed,
                    evidence_id: evidence("output-resumed"),
                },
            )
            .expect("ordered progress"),
        ActivityDisposition::ProgressRecorded
    );
    assert_eq!(resumed.wait_kind(), Some(WaitKind::Unstructured));
    assert_timed_evidence(resumed.last_progress(), 60_002, "output-resumed");
    assert_eq!(
        resumed
            .apply(
                4,
                at(120_002),
                ActivityEvent::Tick {
                    evidence_id: evidence("released-timeout"),
                },
            )
            .expect("ordered tick"),
        ActivityDisposition::ActivityChanged
    );
    assert_eq!(resumed.activity(), Activity::Unknown);
}

#[test]
fn explicit_progress_records_kind_and_updates_all_three_time_axes() {
    let progress_kinds = [
        ProgressKind::CommandStarted,
        ProgressKind::CommandFinished,
        ProgressKind::FileContentChanged,
        ProgressKind::TestBuildStageChanged,
        ProgressKind::AdapterStepChanged,
        ProgressKind::AgentOutputResumed,
    ];
    let mut projection = projection();

    for (index, kind) in progress_kinds.into_iter().enumerate() {
        let sequence = index as u64 + 2;
        let time = index as u64 + 10;
        let evidence_id = format!("progress-{index}");
        assert_eq!(
            projection
                .apply(
                    sequence,
                    at(time),
                    ActivityEvent::MeaningfulProgress {
                        kind,
                        evidence_id: evidence(&evidence_id),
                    },
                )
                .expect("ordered progress"),
            ActivityDisposition::ProgressRecorded
        );
        assert_eq!(projection.last_progress_kind(), Some(kind));
        assert_timed_evidence(projection.last_meaningful_signal(), time, &evidence_id);
        assert_timed_evidence(projection.last_progress(), time, &evidence_id);
        assert_timed_evidence(projection.last_liveness(), time, &evidence_id);
    }
}

#[test]
fn terminal_and_activation_events_bypass_hysteresis_without_accepting_terminal_activity() {
    let mut projection = projection();
    projection
        .apply(
            2,
            at(100),
            signal(
                Activity::Building,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "building",
                None,
            ),
        )
        .expect("ordered signal");
    assert_eq!(
        projection
            .apply(
                3,
                at(200),
                ActivityEvent::LifecycleTerminated {
                    evidence_id: evidence("terminated"),
                },
            )
            .expect("ordered terminal event"),
        ActivityDisposition::ActivityChanged
    );
    assert!(projection.is_terminal());
    assert_eq!(projection.activity(), Activity::Unknown);
    assert_eq!(activity_evidence(&projection), vec!["terminated"]);

    assert_eq!(
        projection
            .apply(
                4,
                at(300),
                signal(
                    Activity::Editing,
                    SignalSource::StructuredActivity,
                    1_000,
                    1_000,
                    "after-terminal",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::ObservedOnly
    );
    assert_eq!(projection.activity(), Activity::Unknown);
    assert_eq!(activity_evidence(&projection), vec!["terminated"]);

    assert_eq!(
        projection
            .apply(
                5,
                at(400),
                ActivityEvent::LifecycleActivated {
                    evidence_id: evidence("activated"),
                },
            )
            .expect("ordered activation"),
        ActivityDisposition::ActivityChanged
    );
    assert!(!projection.is_terminal());
    assert_eq!(projection.activity(), Activity::Unknown);
    assert_eq!(activity_evidence(&projection), vec!["activated"]);
    assert_eq!(
        projection
            .apply(
                6,
                at(401),
                signal(
                    Activity::Editing,
                    SignalSource::StructuredActivity,
                    1_000,
                    1_000,
                    "resumed-editing",
                    None,
                ),
            )
            .expect("ordered signal"),
        ActivityDisposition::ActivityChanged
    );
    assert_eq!(projection.activity(), Activity::Editing);
}

#[test]
fn ordered_replay_matches_incremental_reduction_and_errors_preserve_state() {
    let events = vec![
        (
            2,
            at(100),
            signal(
                Activity::Planning,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "planning",
                None,
            ),
        ),
        (
            3,
            at(200),
            ActivityEvent::LivenessObserved {
                evidence_id: evidence("heartbeat"),
            },
        ),
        (
            4,
            at(300),
            ActivityEvent::MeaningfulProgress {
                kind: ProgressKind::CommandStarted,
                evidence_id: evidence("command"),
            },
        ),
        (
            5,
            at(2_100),
            signal(
                Activity::Testing,
                SignalSource::StructuredActivity,
                1_000,
                1_000,
                "testing",
                None,
            ),
        ),
    ];

    let replayed =
        replay_activity(1, at(0), evidence("run-created"), events.clone()).expect("ordered replay");
    let mut incremental = projection();
    for (sequence, observed_at, event) in events {
        incremental
            .apply(sequence, observed_at, event)
            .expect("ordered event");
    }
    assert_eq!(replayed, incremental);
    assert_eq!(incremental.version(), 5);

    let before_sequence_error = incremental.clone();
    assert_eq!(
        incremental.apply(
            5,
            at(2_200),
            ActivityEvent::Tick {
                evidence_id: evidence("duplicate-sequence"),
            },
        ),
        Err(ActivityError::NonMonotonicIngestSequence {
            current: 5,
            received: 5,
        })
    );
    assert_eq!(incremental, before_sequence_error);

    let before_time_error = incremental.clone();
    assert_eq!(
        incremental.apply(
            6,
            at(2_099),
            ActivityEvent::Tick {
                evidence_id: evidence("reversed-time"),
            },
        ),
        Err(ActivityError::NonMonotonicTimestamp {
            current: at(2_100),
            received: at(2_099),
        })
    );
    assert_eq!(incremental, before_time_error);
}
