use flit_core::request::{
    IgnoredRequestReason, RequestDisposition, RequestError, RequestEvent, RequestId, RequestKind,
    RequestProjection, RequestStatus, RequestValueError, ResponseAttempt, ResponseAttemptId,
    replay_request,
};

fn request_id(value: &str) -> RequestId {
    RequestId::new(value).expect("test request ID must be valid")
}

fn attempt_id(value: &str) -> ResponseAttemptId {
    ResponseAttemptId::new(value).expect("test response attempt ID must be valid")
}

fn projection(kind: RequestKind) -> RequestProjection {
    RequestProjection::new(request_id("request-1"), kind, 10)
        .expect("initial ingest sequence must be valid")
}

fn submit(
    projection: &mut RequestProjection,
    ingest_seq: u64,
    expected_version: u64,
    attempt: &str,
) -> RequestDisposition {
    projection
        .apply(
            ingest_seq,
            RequestEvent::ResponseSubmitted {
                expected_version,
                attempt_id: attempt_id(attempt),
            },
        )
        .expect("event must be ordered")
}

fn active_attempt(status: &RequestStatus) -> &ResponseAttempt {
    match status {
        RequestStatus::ResponsePending(attempt)
        | RequestStatus::DeliveryUnknown(attempt)
        | RequestStatus::Resolved(attempt) => attempt,
        RequestStatus::Open | RequestStatus::Expired => panic!("expected an attempt-bound state"),
    }
}

#[test]
fn identifiers_and_initial_sequence_reject_invalid_values() {
    assert_eq!(
        RequestId::new("\n\t"),
        Err(RequestValueError::BlankRequestId)
    );
    assert_eq!(
        ResponseAttemptId::new("   "),
        Err(RequestValueError::BlankResponseAttemptId)
    );
    assert_eq!(
        RequestProjection::new(request_id("request-1"), RequestKind::Permission, 0),
        Err(RequestError::InvalidInitialIngestSequence)
    );
}

#[test]
fn permission_and_question_share_mechanics_while_preserving_kind() {
    for kind in [RequestKind::Permission, RequestKind::Question] {
        let mut projection = projection(kind);
        assert_eq!(projection.request_id().as_str(), "request-1");
        assert_eq!(projection.kind(), kind);
        assert_eq!(projection.version(), 10);
        assert_eq!(projection.last_ingest_seq(), 10);
        assert_eq!(projection.status(), &RequestStatus::Open);

        assert_eq!(
            submit(&mut projection, 11, 10, "attempt-1"),
            RequestDisposition::Applied
        );
        assert!(matches!(
            projection.status(),
            RequestStatus::ResponsePending(_)
        ));
        assert_eq!(projection.version(), 11);
        assert_eq!(projection.last_ingest_seq(), 11);
    }
}

#[test]
fn submission_requires_exact_request_version_without_advancing_protected_state() {
    let mut projection = projection(RequestKind::Permission);

    assert_eq!(
        submit(&mut projection, 11, 9, "attempt-stale"),
        RequestDisposition::Ignored(IgnoredRequestReason::StaleExpectedVersion {
            current: 10,
            received: 9,
        })
    );
    assert_eq!(projection.status(), &RequestStatus::Open);
    assert_eq!(projection.version(), 10);
    assert_eq!(projection.last_ingest_seq(), 11);
    assert!(projection.used_attempt_ids().is_empty());

    assert_eq!(
        submit(&mut projection, 12, 10, "attempt-1"),
        RequestDisposition::Applied
    );
    let attempt = active_attempt(projection.status());
    assert_eq!(attempt.id().as_str(), "attempt-1");
    assert_eq!(attempt.submitted_request_version(), 10);
    assert_eq!(projection.version(), 12);
}

#[test]
fn pending_request_blocks_duplicate_clicks_and_parallel_attempts() {
    let mut projection = projection(RequestKind::Permission);
    assert_eq!(
        submit(&mut projection, 11, 10, "attempt-1"),
        RequestDisposition::Applied
    );

    assert_eq!(
        submit(&mut projection, 12, 10, "attempt-1"),
        RequestDisposition::Ignored(IgnoredRequestReason::StaleExpectedVersion {
            current: 11,
            received: 10,
        })
    );
    assert_eq!(
        submit(&mut projection, 13, 11, "attempt-2"),
        RequestDisposition::Ignored(IgnoredRequestReason::ResponseRequiresOpen)
    );
    assert_eq!(
        active_attempt(projection.status()).id().as_str(),
        "attempt-1"
    );
    assert_eq!(projection.version(), 11);
    assert_eq!(projection.last_ingest_seq(), 13);
    assert_eq!(projection.used_attempt_ids().len(), 1);
}

#[test]
fn matching_ack_resolves_and_late_outcomes_are_audit_only() {
    let mut projection = projection(RequestKind::Question);
    assert_eq!(
        submit(&mut projection, 11, 10, "attempt-1"),
        RequestDisposition::Applied
    );
    assert_eq!(
        projection
            .apply(
                12,
                RequestEvent::ResponseResolved {
                    attempt_id: attempt_id("attempt-1"),
                },
            )
            .expect("ordered ack"),
        RequestDisposition::Applied
    );
    assert!(matches!(projection.status(), RequestStatus::Resolved(_)));
    assert_eq!(projection.version(), 12);

    for (ingest_seq, event, reason) in [
        (
            13,
            RequestEvent::ResponseResolved {
                attempt_id: attempt_id("attempt-1"),
            },
            IgnoredRequestReason::ResolutionRequiresActiveAttempt,
        ),
        (
            14,
            RequestEvent::ResponseFailed {
                attempt_id: attempt_id("attempt-1"),
            },
            IgnoredRequestReason::FailureRequiresPending,
        ),
        (
            15,
            RequestEvent::DeliveryUnknown {
                attempt_id: attempt_id("attempt-1"),
            },
            IgnoredRequestReason::DeliveryUnknownRequiresPending,
        ),
    ] {
        assert_eq!(
            projection
                .apply(ingest_seq, event)
                .expect("ordered outcome"),
            RequestDisposition::Ignored(reason)
        );
    }
    assert!(matches!(projection.status(), RequestStatus::Resolved(_)));
    assert_eq!(projection.version(), 12);
    assert_eq!(projection.last_ingest_seq(), 15);
}

#[test]
fn mismatched_outcomes_preserve_the_active_attempt() {
    let mut projection = projection(RequestKind::Permission);
    assert_eq!(
        submit(&mut projection, 11, 10, "attempt-1"),
        RequestDisposition::Applied
    );

    for (ingest_seq, event) in [
        (
            12,
            RequestEvent::ResponseResolved {
                attempt_id: attempt_id("attempt-other"),
            },
        ),
        (
            13,
            RequestEvent::ResponseFailed {
                attempt_id: attempt_id("attempt-other"),
            },
        ),
        (
            14,
            RequestEvent::DeliveryUnknown {
                attempt_id: attempt_id("attempt-other"),
            },
        ),
    ] {
        assert_eq!(
            projection
                .apply(ingest_seq, event)
                .expect("ordered outcome"),
            RequestDisposition::Ignored(IgnoredRequestReason::MismatchedResponseAttempt)
        );
    }
    assert!(matches!(
        projection.status(),
        RequestStatus::ResponsePending(_)
    ));
    assert_eq!(
        active_attempt(projection.status()).id().as_str(),
        "attempt-1"
    );
    assert_eq!(projection.version(), 11);
    assert_eq!(projection.last_ingest_seq(), 14);
}

#[test]
fn explicit_failure_reopens_with_a_new_version_and_attempt_identity() {
    let mut projection = projection(RequestKind::Permission);
    assert_eq!(
        submit(&mut projection, 11, 10, "attempt-1"),
        RequestDisposition::Applied
    );
    assert_eq!(
        projection
            .apply(
                12,
                RequestEvent::ResponseFailed {
                    attempt_id: attempt_id("attempt-1"),
                },
            )
            .expect("ordered failure"),
        RequestDisposition::Applied
    );
    assert_eq!(projection.status(), &RequestStatus::Open);
    assert_eq!(projection.version(), 12);

    assert_eq!(
        submit(&mut projection, 13, 12, "attempt-1"),
        RequestDisposition::Ignored(IgnoredRequestReason::ResponseAttemptAlreadyUsed)
    );
    assert_eq!(projection.status(), &RequestStatus::Open);
    assert_eq!(projection.version(), 12);

    assert_eq!(
        submit(&mut projection, 14, 12, "attempt-2"),
        RequestDisposition::Applied
    );
    assert_eq!(
        active_attempt(projection.status()).id().as_str(),
        "attempt-2"
    );
    assert_eq!(
        active_attempt(projection.status()).submitted_request_version(),
        12
    );
    assert_eq!(
        projection.used_attempt_ids(),
        &[attempt_id("attempt-1"), attempt_id("attempt-2")]
    );

    assert_eq!(
        projection
            .apply(
                15,
                RequestEvent::ResponseResolved {
                    attempt_id: attempt_id("attempt-1"),
                },
            )
            .expect("late old ack"),
        RequestDisposition::Ignored(IgnoredRequestReason::MismatchedResponseAttempt)
    );
    assert_eq!(
        active_attempt(projection.status()).id().as_str(),
        "attempt-2"
    );
    assert_eq!(projection.version(), 14);
}

#[test]
fn delivery_unknown_is_non_retry_and_only_matching_ack_can_resolve_it() {
    let mut projection = projection(RequestKind::Question);
    assert_eq!(
        submit(&mut projection, 11, 10, "attempt-1"),
        RequestDisposition::Applied
    );
    assert_eq!(
        projection
            .apply(
                12,
                RequestEvent::DeliveryUnknown {
                    attempt_id: attempt_id("attempt-1"),
                },
            )
            .expect("ordered unknown outcome"),
        RequestDisposition::Applied
    );
    let unknown = active_attempt(projection.status());
    assert_eq!(unknown.id().as_str(), "attempt-1");
    assert_eq!(unknown.submitted_request_version(), 10);

    assert_eq!(
        submit(&mut projection, 13, 12, "attempt-2"),
        RequestDisposition::Ignored(IgnoredRequestReason::ResponseRequiresOpen)
    );
    assert_eq!(
        projection
            .apply(
                14,
                RequestEvent::ResponseFailed {
                    attempt_id: attempt_id("attempt-1"),
                },
            )
            .expect("late failure"),
        RequestDisposition::Ignored(IgnoredRequestReason::FailureRequiresPending)
    );
    assert_eq!(
        projection
            .apply(15, RequestEvent::RequestExpired)
            .expect("late expiration"),
        RequestDisposition::Ignored(IgnoredRequestReason::ExpirationRequiresOpen)
    );
    assert_eq!(
        projection
            .apply(
                16,
                RequestEvent::ResponseResolved {
                    attempt_id: attempt_id("attempt-other"),
                },
            )
            .expect("mismatched reconciliation"),
        RequestDisposition::Ignored(IgnoredRequestReason::MismatchedResponseAttempt)
    );
    assert!(matches!(
        projection.status(),
        RequestStatus::DeliveryUnknown(_)
    ));
    assert_eq!(projection.version(), 12);

    assert_eq!(
        projection
            .apply(
                17,
                RequestEvent::ResponseResolved {
                    attempt_id: attempt_id("attempt-1"),
                },
            )
            .expect("matching reconciliation"),
        RequestDisposition::Applied
    );
    assert!(matches!(projection.status(), RequestStatus::Resolved(_)));
    assert_eq!(projection.version(), 17);
}

#[test]
fn open_request_can_expire_and_cannot_be_answered_afterward() {
    let mut projection = projection(RequestKind::Question);
    assert_eq!(
        projection
            .apply(11, RequestEvent::RequestExpired)
            .expect("ordered expiration"),
        RequestDisposition::Applied
    );
    assert_eq!(projection.status(), &RequestStatus::Expired);
    assert_eq!(projection.version(), 11);

    assert_eq!(
        submit(&mut projection, 12, 11, "attempt-1"),
        RequestDisposition::Ignored(IgnoredRequestReason::ResponseRequiresOpen)
    );
    assert_eq!(
        projection
            .apply(13, RequestEvent::RequestExpired)
            .expect("duplicate expiration"),
        RequestDisposition::Ignored(IgnoredRequestReason::ExpirationRequiresOpen)
    );
    assert_eq!(projection.status(), &RequestStatus::Expired);
    assert_eq!(projection.version(), 11);
    assert_eq!(projection.last_ingest_seq(), 13);
}

#[test]
fn non_monotonic_event_is_rejected_without_mutating_projection() {
    let mut projection = projection(RequestKind::Permission);
    assert_eq!(
        submit(&mut projection, 11, 10, "attempt-1"),
        RequestDisposition::Applied
    );
    let before = projection.clone();

    assert_eq!(
        projection.apply(
            11,
            RequestEvent::ResponseResolved {
                attempt_id: attempt_id("attempt-1"),
            },
        ),
        Err(RequestError::NonMonotonicIngestSequence {
            current: 11,
            received: 11,
        })
    );
    assert_eq!(projection, before);
}

#[test]
fn replay_matches_incremental_reduction_including_ignored_events() {
    let events = vec![
        (
            11,
            RequestEvent::ResponseSubmitted {
                expected_version: 10,
                attempt_id: attempt_id("attempt-1"),
            },
        ),
        (
            12,
            RequestEvent::ResponseResolved {
                attempt_id: attempt_id("attempt-old"),
            },
        ),
        (
            13,
            RequestEvent::ResponseFailed {
                attempt_id: attempt_id("attempt-1"),
            },
        ),
        (
            14,
            RequestEvent::ResponseSubmitted {
                expected_version: 13,
                attempt_id: attempt_id("attempt-2"),
            },
        ),
        (
            15,
            RequestEvent::DeliveryUnknown {
                attempt_id: attempt_id("attempt-2"),
            },
        ),
        (
            16,
            RequestEvent::ResponseResolved {
                attempt_id: attempt_id("attempt-2"),
            },
        ),
    ];
    let replayed = replay_request(
        request_id("request-1"),
        RequestKind::Permission,
        10,
        events.clone(),
    )
    .expect("ordered replay");
    let mut incremental = projection(RequestKind::Permission);
    for (ingest_seq, event) in events {
        incremental.apply(ingest_seq, event).expect("ordered event");
    }

    assert_eq!(replayed, incremental);
    assert!(matches!(replayed.status(), RequestStatus::Resolved(_)));
    assert_eq!(replayed.version(), 16);
    assert_eq!(replayed.last_ingest_seq(), 16);
    assert_eq!(replayed.used_attempt_ids().len(), 2);
}
