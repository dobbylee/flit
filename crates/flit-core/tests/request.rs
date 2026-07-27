use flit_core::{
    provider_outcome::{
        MissingProviderOutcomeField, ProviderDecision, ProviderOutcomeAuditReason,
        ProviderOutcomeClassification, ProviderOutcomeValue, ProviderTerminalOutcome,
        VerifiedProviderOutcome,
    },
    request::{
        IgnoredRequestReason, RequestDisposition, RequestError, RequestEvent, RequestId,
        RequestKind, RequestProjection, RequestResolution, RequestStatus, RequestValueError,
        ResponseAttempt, ResponseAttemptId, replay_request,
    },
};

fn request_id(value: &str) -> RequestId {
    RequestId::new(value).expect("test request ID must be valid")
}

fn attempt_id(value: &str) -> ResponseAttemptId {
    ResponseAttemptId::new(value).expect("test response attempt ID must be valid")
}

fn outcome_value(value: &str) -> ProviderOutcomeValue {
    ProviderOutcomeValue::new(value).expect("test provider outcome value must be valid")
}

fn provider_outcome(
    request: &str,
    request_version: u64,
    decision: &str,
) -> VerifiedProviderOutcome {
    VerifiedProviderOutcome {
        session_key: outcome_value("session-1"),
        request_id: request_id(request),
        request_version,
        decision_id: outcome_value(decision),
        decision: ProviderDecision::Allowed,
        terminal_outcome: ProviderTerminalOutcome::RequestResolved,
        captured_at_ms: 1_000,
        evidence_id: outcome_value("evidence-1"),
    }
}

fn provider_event(
    request: &str,
    expected_request_version: u64,
    classification: ProviderOutcomeClassification,
) -> RequestEvent {
    RequestEvent::ProviderOutcomeClassified {
        request_id: request_id(request),
        expected_request_version,
        classification: Box::new(classification),
    }
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
        RequestStatus::ResponsePending(attempt) | RequestStatus::DeliveryUnknown(attempt) => {
            attempt
        }
        RequestStatus::Resolved(RequestResolution::ManualResponse(attempt)) => attempt,
        RequestStatus::Open
        | RequestStatus::Resolved(RequestResolution::ProviderOutcome(_))
        | RequestStatus::ProviderOutcomeUnknown(_)
        | RequestStatus::Expired => panic!("expected a manual attempt-bound state"),
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

#[test]
fn exact_provider_outcome_resolves_permission_without_a_core_attempt() {
    let mut projection = projection(RequestKind::Permission);
    let outcome = provider_outcome("request-1", 10, "decision-1");
    assert_eq!(
        projection
            .apply(
                11,
                provider_event(
                    "request-1",
                    10,
                    ProviderOutcomeClassification::Resolved(outcome.clone()),
                ),
            )
            .expect("ordered provider outcome"),
        RequestDisposition::Applied
    );
    let RequestStatus::Resolved(RequestResolution::ProviderOutcome(resolution)) =
        projection.status()
    else {
        panic!("provider outcome must resolve the permission");
    };
    assert_eq!(resolution.outcome(), &outcome);
    assert!(projection.used_attempt_ids().is_empty());
    assert_eq!(
        projection.used_provider_decision_ids(),
        &[outcome_value("decision-1")]
    );
}

#[test]
fn incomplete_provider_outcome_locks_and_exact_complete_reconciliation_resolves() {
    let mut projection = projection(RequestKind::Permission);
    let missing = vec![
        MissingProviderOutcomeField::DecisionId,
        MissingProviderOutcomeField::EvidenceId,
    ];
    assert_eq!(
        projection
            .apply(
                11,
                provider_event(
                    "request-1",
                    10,
                    ProviderOutcomeClassification::OutcomeUnknown {
                        missing: missing.clone(),
                    },
                ),
            )
            .expect("ordered unknown outcome"),
        RequestDisposition::Applied
    );
    let RequestStatus::ProviderOutcomeUnknown(unknown) = projection.status() else {
        panic!("incomplete exact outcome must lock the request");
    };
    assert_eq!(unknown.original_request_version(), 10);
    assert_eq!(unknown.missing(), missing);
    assert_eq!(projection.version(), 11);
    assert_eq!(
        submit(&mut projection, 12, 11, "attempt-1"),
        RequestDisposition::Ignored(IgnoredRequestReason::ResponseRequiresOpen)
    );

    let outcome = provider_outcome("request-1", 10, "decision-1");
    assert_eq!(
        projection
            .apply(
                13,
                provider_event(
                    "request-1",
                    10,
                    ProviderOutcomeClassification::Resolved(outcome.clone()),
                ),
            )
            .expect("ordered reconciliation"),
        RequestDisposition::Applied
    );
    assert!(matches!(
        projection.status(),
        RequestStatus::Resolved(RequestResolution::ProviderOutcome(resolution))
            if resolution.outcome() == &outcome
    ));

    assert_eq!(
        projection
            .apply(
                14,
                provider_event(
                    "request-1",
                    10,
                    ProviderOutcomeClassification::Resolved(outcome),
                ),
            )
            .expect("duplicate decision"),
        RequestDisposition::Ignored(IgnoredRequestReason::ProviderDecisionAlreadyUsed)
    );
    assert_eq!(projection.version(), 13);
    assert_eq!(projection.last_ingest_seq(), 14);
}

#[test]
fn provider_outcome_rejects_question_stale_mismatch_manual_pending_and_audit_results() {
    let classification =
        ProviderOutcomeClassification::Resolved(provider_outcome("request-1", 10, "decision-1"));
    let mut question = projection(RequestKind::Question);
    assert_eq!(
        question
            .apply(11, provider_event("request-1", 10, classification.clone()))
            .expect("ordered question outcome"),
        RequestDisposition::Ignored(IgnoredRequestReason::ProviderOutcomeRequiresPermission)
    );

    let mut permission = projection(RequestKind::Permission);
    assert_eq!(
        permission
            .apply(
                11,
                provider_event("request-other", 10, classification.clone())
            )
            .expect("ordered mismatched request"),
        RequestDisposition::Ignored(IgnoredRequestReason::MismatchedProviderRequest)
    );
    assert_eq!(
        permission
            .apply(12, provider_event("request-1", 9, classification.clone()))
            .expect("ordered stale outcome"),
        RequestDisposition::Ignored(IgnoredRequestReason::MismatchedProviderRequest)
    );
    assert_eq!(
        submit(&mut permission, 13, 10, "attempt-1"),
        RequestDisposition::Applied
    );
    assert_eq!(
        permission
            .apply(14, provider_event("request-1", 10, classification))
            .expect("ordered pending outcome"),
        RequestDisposition::Ignored(IgnoredRequestReason::ProviderOutcomeRequiresOpenOrUnknown)
    );
    assert!(matches!(
        permission.status(),
        RequestStatus::ResponsePending(_)
    ));

    for (ingest_seq, classification) in [
        (
            15,
            ProviderOutcomeClassification::AuditOnly(ProviderOutcomeAuditReason::RequestMismatch),
        ),
        (
            16,
            ProviderOutcomeClassification::UnboundSessionCapabilityDegrade,
        ),
    ] {
        assert_eq!(
            permission
                .apply(ingest_seq, provider_event("request-1", 10, classification),)
                .expect("ordered non-applicable outcome"),
            RequestDisposition::Ignored(
                IgnoredRequestReason::ProviderOutcomeClassificationNotApplicable
            )
        );
    }
}

#[test]
fn provider_outcome_unknown_requires_missing_fields_and_exact_open_version() {
    let mut projection = projection(RequestKind::Permission);
    assert_eq!(
        projection
            .apply(
                11,
                provider_event(
                    "request-1",
                    10,
                    ProviderOutcomeClassification::OutcomeUnknown {
                        missing: Vec::new(),
                    },
                ),
            )
            .expect("ordered malformed unknown"),
        RequestDisposition::Ignored(
            IgnoredRequestReason::ProviderOutcomeUnknownRequiresMissingFields
        )
    );
    assert_eq!(
        projection
            .apply(
                12,
                provider_event(
                    "request-1",
                    9,
                    ProviderOutcomeClassification::OutcomeUnknown {
                        missing: vec![MissingProviderOutcomeField::EvidenceId],
                    },
                ),
            )
            .expect("ordered stale unknown"),
        RequestDisposition::Ignored(IgnoredRequestReason::StaleExpectedVersion {
            current: 10,
            received: 9,
        })
    );
    assert_eq!(projection.status(), &RequestStatus::Open);
    assert_eq!(projection.version(), 10);
    assert_eq!(projection.last_ingest_seq(), 12);
}

#[test]
fn provider_unknown_reconciliation_replay_matches_incremental_state() {
    let outcome = provider_outcome("request-1", 10, "decision-1");
    let events = vec![
        (
            11,
            provider_event(
                "request-1",
                10,
                ProviderOutcomeClassification::OutcomeUnknown {
                    missing: vec![MissingProviderOutcomeField::EvidenceId],
                },
            ),
        ),
        (
            12,
            provider_event(
                "request-1",
                10,
                ProviderOutcomeClassification::Resolved(outcome.clone()),
            ),
        ),
        (
            13,
            provider_event(
                "request-1",
                10,
                ProviderOutcomeClassification::Resolved(outcome),
            ),
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
    assert!(matches!(
        replayed.status(),
        RequestStatus::Resolved(RequestResolution::ProviderOutcome(_))
    ));
    assert_eq!(replayed.version(), 12);
    assert_eq!(replayed.last_ingest_seq(), 13);
    assert_eq!(replayed.used_provider_decision_ids().len(), 1);
}
