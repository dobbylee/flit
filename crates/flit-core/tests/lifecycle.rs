use flit_core::lifecycle::{
    IgnoredLifecycleReason, LifecycleDisposition, LifecycleError, LifecycleEvent,
    LifecycleProjection, ResumeIntentId, RunLifecycle, SessionId, replay_lifecycle,
};

fn session(value: &str) -> SessionId {
    SessionId::new(value).expect("test session identifier must be valid")
}

fn intent(value: &str) -> ResumeIntentId {
    ResumeIntentId::new(value).expect("test resume intent identifier must be valid")
}

fn running_projection() -> LifecycleProjection {
    let mut projection = LifecycleProjection::new(1).expect("valid initial sequence");
    assert_eq!(
        projection
            .apply(
                2,
                LifecycleEvent::SessionConnected {
                    session_id: session("session-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    projection
}

fn assert_invariants(projection: &LifecycleProjection) {
    match projection.lifecycle() {
        RunLifecycle::Starting => assert!(projection.active_session_id().is_none()),
        RunLifecycle::Running => assert!(projection.active_session_id().is_some()),
        lifecycle if lifecycle.is_terminal() => assert!(projection.active_session_id().is_none()),
        _ => panic!("all lifecycle variants must have an invariant"),
    }
    if projection.resume_intent().is_some() {
        assert!(projection.lifecycle().is_terminal());
    }
}

#[test]
fn identifiers_and_initial_sequence_reject_invalid_values() {
    assert!(SessionId::new("").is_err());
    assert!(SessionId::new("   ").is_err());
    assert!(ResumeIntentId::new("\n\t").is_err());
    assert_eq!(
        LifecycleProjection::new(0),
        Err(LifecycleError::InvalidInitialIngestSequence)
    );
}

#[test]
fn first_session_connection_starts_the_run_and_later_connections_are_ignored() {
    let mut projection = LifecycleProjection::new(1).expect("valid initial sequence");

    assert_eq!(
        projection
            .apply(
                2,
                LifecycleEvent::SessionConnected {
                    session_id: session("session-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    assert_eq!(projection.lifecycle(), RunLifecycle::Running);
    assert_eq!(
        projection.active_session_id().map(SessionId::as_str),
        Some("session-1")
    );

    assert_eq!(
        projection
            .apply(
                3,
                LifecycleEvent::SessionConnected {
                    session_id: session("session-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::DuplicateSessionConnection)
    );
    assert_eq!(
        projection
            .apply(
                4,
                LifecycleEvent::SessionConnected {
                    session_id: session("session-2"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::LiveSessionAlreadyExists)
    );
    assert_eq!(projection.version(), 4);
    assert_eq!(
        projection.active_session_id().map(SessionId::as_str),
        Some("session-1")
    );
}

#[test]
fn running_run_accepts_each_terminal_outcome_and_releases_its_session() {
    let cases = [
        (LifecycleEvent::RunCompleted, RunLifecycle::Finished),
        (LifecycleEvent::RunFailed, RunLifecycle::Failed),
        (LifecycleEvent::RunStopped, RunLifecycle::Stopped),
        (LifecycleEvent::RunInterrupted, RunLifecycle::Interrupted),
    ];

    for (event, expected) in cases {
        let mut projection = running_projection();
        assert_eq!(
            projection.apply(3, event).expect("ordered event"),
            LifecycleDisposition::Applied
        );
        assert_eq!(projection.lifecycle(), expected);
        assert!(projection.active_session_id().is_none());
        assert_eq!(
            projection.last_session_id().map(SessionId::as_str),
            Some("session-1")
        );
        assert_invariants(&projection);
    }
}

#[test]
fn starting_run_only_accepts_failure_or_stop_as_terminal_outcomes() {
    for event in [LifecycleEvent::RunFailed, LifecycleEvent::RunStopped] {
        let mut projection = LifecycleProjection::new(1).expect("valid initial sequence");
        assert_eq!(
            projection.apply(2, event).expect("ordered event"),
            LifecycleDisposition::Applied
        );
        assert!(projection.lifecycle().is_terminal());
    }

    for event in [LifecycleEvent::RunCompleted, LifecycleEvent::RunInterrupted] {
        let mut projection = LifecycleProjection::new(1).expect("valid initial sequence");
        assert_eq!(
            projection.apply(2, event).expect("ordered event"),
            LifecycleDisposition::Ignored(IgnoredLifecycleReason::TransitionRequiresRunning)
        );
        assert_eq!(projection.lifecycle(), RunLifecycle::Starting);
        assert_eq!(projection.version(), 2);
    }
}

#[test]
fn first_terminal_outcome_is_preserved() {
    let mut projection = running_projection();
    projection
        .apply(3, LifecycleEvent::RunCompleted)
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(4, LifecycleEvent::RunCompleted)
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::DuplicateTerminalEvent)
    );
    assert_eq!(
        projection
            .apply(5, LifecycleEvent::RunFailed)
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::FirstTerminalStatePreserved)
    );
    assert_eq!(
        projection
            .apply(
                6,
                LifecycleEvent::SessionConnected {
                    session_id: session("session-2"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::TerminalStateRequiresResume)
    );
    assert_eq!(projection.lifecycle(), RunLifecycle::Finished);
    assert_eq!(
        projection.last_session_id().map(SessionId::as_str),
        Some("session-1")
    );
}

#[test]
fn resume_requires_terminal_state_and_exact_projection_version() {
    let mut projection = running_projection();
    assert_eq!(
        projection
            .apply(
                3,
                LifecycleEvent::ResumeRequested {
                    intent_id: intent("intent-1"),
                    expected_version: 2,
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::ResumeRequiresTerminal)
    );
    projection
        .apply(4, LifecycleEvent::RunStopped)
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(
                5,
                LifecycleEvent::ResumeRequested {
                    intent_id: intent("intent-1"),
                    expected_version: 3,
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::StaleExpectedVersion)
    );
    assert_eq!(projection.version(), 5);
    assert!(projection.resume_intent().is_none());

    assert_eq!(
        projection
            .apply(
                6,
                LifecycleEvent::ResumeRequested {
                    intent_id: intent("intent-1"),
                    expected_version: 5,
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    let pending = projection.resume_intent().expect("resume must be pending");
    assert_eq!(pending.id().as_str(), "intent-1");
    assert_eq!(pending.expected_version(), 5);
    assert_eq!(pending.prior_lifecycle(), RunLifecycle::Stopped);
    assert_eq!(
        pending.previous_session_id().map(SessionId::as_str),
        Some("session-1")
    );
}

#[test]
fn non_lifecycle_run_event_advances_the_resume_cas_version() {
    let mut projection = running_projection();
    projection
        .apply(3, LifecycleEvent::RunCompleted)
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(4, LifecycleEvent::RunEventObserved)
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    assert_eq!(projection.version(), 4);
    assert_eq!(projection.lifecycle(), RunLifecycle::Finished);

    assert_eq!(
        projection
            .apply(
                5,
                LifecycleEvent::ResumeRequested {
                    intent_id: intent("intent-1"),
                    expected_version: 4,
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );

    let before = projection.clone();
    assert_eq!(
        projection.apply(5, LifecycleEvent::RunEventObserved),
        Err(LifecycleError::NonMonotonicIngestSequence {
            current: 5,
            received: 5,
        })
    );
    assert_eq!(projection, before);
}

#[test]
fn matching_resume_intent_creates_a_new_running_session() {
    let mut projection = running_projection();
    projection
        .apply(3, LifecycleEvent::RunCompleted)
        .expect("ordered event");
    projection
        .apply(
            4,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-1"),
                expected_version: 3,
            },
        )
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(
                5,
                LifecycleEvent::SessionResumed {
                    intent_id: intent("intent-1"),
                    session_id: session("session-2"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    assert_eq!(projection.lifecycle(), RunLifecycle::Running);
    assert_eq!(
        projection.active_session_id().map(SessionId::as_str),
        Some("session-2")
    );
    assert_eq!(
        projection.last_session_id().map(SessionId::as_str),
        Some("session-1")
    );
    assert!(projection.resume_intent().is_none());
    assert_invariants(&projection);
}

#[test]
fn invalid_resume_outcomes_do_not_consume_the_pending_intent() {
    let mut projection = running_projection();
    projection
        .apply(3, LifecycleEvent::RunInterrupted)
        .expect("ordered event");
    projection
        .apply(
            4,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-1"),
                expected_version: 3,
            },
        )
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(
                5,
                LifecycleEvent::SessionResumed {
                    intent_id: intent("intent-other"),
                    session_id: session("session-2"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::MismatchedResumeIntent)
    );
    assert_eq!(
        projection
            .apply(
                6,
                LifecycleEvent::SessionResumed {
                    intent_id: intent("intent-1"),
                    session_id: session("session-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::PreviousSessionReused)
    );
    assert_eq!(
        projection
            .apply(
                7,
                LifecycleEvent::ResumeRequested {
                    intent_id: intent("intent-2"),
                    expected_version: 6,
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::ResumeAlreadyPending)
    );
    assert_eq!(
        projection
            .resume_intent()
            .map(|pending| pending.id().as_str()),
        Some("intent-1")
    );
    assert_eq!(projection.lifecycle(), RunLifecycle::Interrupted);
}

#[test]
fn resume_rejects_every_previously_used_session_identity() {
    let mut projection = running_projection();
    projection
        .apply(3, LifecycleEvent::RunCompleted)
        .expect("ordered event");
    projection
        .apply(
            4,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-1"),
                expected_version: 3,
            },
        )
        .expect("ordered event");
    projection
        .apply(
            5,
            LifecycleEvent::SessionResumed {
                intent_id: intent("intent-1"),
                session_id: session("session-2"),
            },
        )
        .expect("ordered event");
    projection
        .apply(6, LifecycleEvent::RunStopped)
        .expect("ordered event");
    projection
        .apply(
            7,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-2"),
                expected_version: 6,
            },
        )
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(
                8,
                LifecycleEvent::SessionResumed {
                    intent_id: intent("intent-2"),
                    session_id: session("session-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::PreviousSessionReused)
    );
    assert_eq!(projection.lifecycle(), RunLifecycle::Stopped);
    assert!(projection.resume_intent().is_some());
    assert_eq!(
        projection
            .session_history()
            .iter()
            .map(SessionId::as_str)
            .collect::<Vec<_>>(),
        ["session-1", "session-2"]
    );

    assert_eq!(
        projection
            .apply(
                9,
                LifecycleEvent::SessionResumed {
                    intent_id: intent("intent-2"),
                    session_id: session("session-3"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    assert_eq!(projection.lifecycle(), RunLifecycle::Running);
    assert_eq!(
        projection.active_session_id().map(SessionId::as_str),
        Some("session-3")
    );
}

#[test]
fn matching_resume_failure_consumes_intent_without_changing_terminal_state() {
    let mut projection = running_projection();
    projection
        .apply(3, LifecycleEvent::RunFailed)
        .expect("ordered event");
    projection
        .apply(
            4,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-1"),
                expected_version: 3,
            },
        )
        .expect("ordered event");

    assert_eq!(
        projection
            .apply(
                5,
                LifecycleEvent::ResumeFailed {
                    intent_id: intent("intent-other"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::MismatchedResumeIntent)
    );
    assert!(projection.resume_intent().is_some());
    assert_eq!(
        projection
            .apply(
                6,
                LifecycleEvent::ResumeFailed {
                    intent_id: intent("intent-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Applied
    );
    assert!(projection.resume_intent().is_none());
    assert_eq!(projection.lifecycle(), RunLifecycle::Failed);
    assert_eq!(
        projection
            .apply(
                7,
                LifecycleEvent::ResumeFailed {
                    intent_id: intent("intent-1"),
                },
            )
            .expect("ordered event"),
        LifecycleDisposition::Ignored(IgnoredLifecycleReason::MissingResumeIntent)
    );
}

#[test]
fn non_monotonic_event_is_rejected_without_mutating_projection() {
    let mut projection = running_projection();
    let before = projection.clone();

    assert_eq!(
        projection.apply(2, LifecycleEvent::RunFailed),
        Err(LifecycleError::NonMonotonicIngestSequence {
            current: 2,
            received: 2,
        })
    );
    assert_eq!(projection, before);
}

#[test]
fn replay_matches_incremental_reduction_including_ignored_events() {
    let events = vec![
        (
            2,
            LifecycleEvent::SessionConnected {
                session_id: session("session-1"),
            },
        ),
        (3, LifecycleEvent::RunCompleted),
        (4, LifecycleEvent::RunFailed),
        (
            5,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-1"),
                expected_version: 4,
            },
        ),
        (
            6,
            LifecycleEvent::SessionResumed {
                intent_id: intent("intent-1"),
                session_id: session("session-2"),
            },
        ),
    ];
    let replayed = replay_lifecycle(1, events.clone()).expect("ordered replay");
    let mut incremental = LifecycleProjection::new(1).expect("valid initial sequence");
    for (ingest_seq, event) in events {
        incremental.apply(ingest_seq, event).expect("ordered event");
    }

    assert_eq!(replayed, incremental);
    assert_eq!(replayed.version(), 6);
    assert_eq!(replayed.lifecycle(), RunLifecycle::Running);
    assert_eq!(
        replayed.active_session_id().map(SessionId::as_str),
        Some("session-2")
    );
}

#[test]
fn short_event_sequences_preserve_projection_invariants() {
    fn events() -> Vec<LifecycleEvent> {
        vec![
            LifecycleEvent::RunEventObserved,
            LifecycleEvent::SessionConnected {
                session_id: session("session-1"),
            },
            LifecycleEvent::SessionConnected {
                session_id: session("session-2"),
            },
            LifecycleEvent::RunCompleted,
            LifecycleEvent::RunFailed,
            LifecycleEvent::RunStopped,
            LifecycleEvent::RunInterrupted,
            LifecycleEvent::ResumeRequested {
                intent_id: intent("intent-1"),
                expected_version: 1,
            },
            LifecycleEvent::SessionResumed {
                intent_id: intent("intent-1"),
                session_id: session("session-2"),
            },
            LifecycleEvent::ResumeFailed {
                intent_id: intent("intent-1"),
            },
        ]
    }

    for first in events() {
        for second in events() {
            for third in events() {
                let mut projection = LifecycleProjection::new(1).expect("valid initial sequence");
                projection.apply(2, first.clone()).expect("ordered event");
                assert_invariants(&projection);
                projection.apply(3, second.clone()).expect("ordered event");
                assert_invariants(&projection);
                projection.apply(4, third).expect("ordered event");
                assert_invariants(&projection);
            }
        }
    }
}
