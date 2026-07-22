use flit_core::{
    activity::{EvidenceId, TimestampMs},
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionDisposition, AttentionError,
        AttentionEvent, AttentionEvidence, AttentionItem, AttentionItemDraft, AttentionItemId,
        AttentionProjection, AttentionSeverity, AttentionStatus, EvidenceUnavailableReason,
        IgnoredAttentionReason, SourceEventId, replay_attention,
    },
};

fn at(value: u64) -> TimestampMs {
    TimestampMs::new(value)
}

fn item_id(value: &str) -> AttentionItemId {
    AttentionItemId::new(value).expect("test item identifier must be valid")
}

fn source_event_id(value: &str) -> SourceEventId {
    SourceEventId::new(value).expect("test source event identifier must be valid")
}

fn dedupe_key(value: &str) -> AttentionDedupeKey {
    AttentionDedupeKey::new(value).expect("test dedupe key must be valid")
}

fn evidence_id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("test evidence identifier must be valid")
}

fn evidence(value: &str) -> AttentionEvidence {
    AttentionEvidence::new(vec![evidence_id(value)], None).expect("test evidence must be complete")
}

#[allow(clippy::too_many_arguments)]
fn draft(
    id: &str,
    key: &str,
    category: AttentionCategory,
    severity: AttentionSeverity,
    blocking: bool,
    created_at: u64,
    evidence_value: &str,
) -> AttentionItemDraft {
    AttentionItemDraft::new(
        item_id(id),
        source_event_id(&format!("event-{id}")),
        category,
        severity,
        blocking,
        dedupe_key(key),
        evidence(evidence_value),
        at(created_at),
    )
    .expect("test attention item must satisfy its category policy")
}

fn item<'a>(projection: &'a AttentionProjection, id: &str) -> &'a AttentionItem {
    projection
        .item(&item_id(id))
        .expect("test attention item must exist")
}

fn resolve(id: &str, observed_at: u64, evidence_value: &str) -> AttentionEvent {
    AttentionEvent::Resolved {
        item_id: item_id(id),
        observed_at: at(observed_at),
        evidence_id: evidence_id(evidence_value),
    }
}

#[test]
fn value_types_evidence_and_category_policy_reject_invalid_inputs() {
    assert_eq!(
        AttentionProjection::new(0),
        Err(AttentionError::InvalidInitialIngestSequence)
    );
    assert_eq!(
        AttentionItemId::new("  "),
        Err(AttentionError::BlankAttentionItemId)
    );
    assert_eq!(
        AttentionDedupeKey::new(""),
        Err(AttentionError::BlankDedupeKey)
    );
    assert_eq!(
        SourceEventId::new("\t"),
        Err(AttentionError::BlankSourceEventId)
    );
    assert_eq!(
        EvidenceUnavailableReason::new(" "),
        Err(AttentionError::BlankEvidenceUnavailableReason)
    );
    assert_eq!(
        AttentionEvidence::new(Vec::new(), None),
        Err(AttentionError::MissingEvidence)
    );
    assert_eq!(
        AttentionEvidence::new(vec![evidence_id("same"), evidence_id("same")], None),
        Err(AttentionError::DuplicateEvidenceId("same".to_owned()))
    );

    let unavailable = AttentionEvidence::new(
        Vec::new(),
        Some(
            EvidenceUnavailableReason::new("provider omitted a raw locator")
                .expect("reason must be valid"),
        ),
    )
    .expect("an explicit unavailable reason satisfies the evidence contract");
    assert!(unavailable.evidence_ids().is_empty());
    assert_eq!(
        unavailable
            .unavailable_reason()
            .expect("reason must be retained")
            .as_str(),
        "provider omitted a raw locator"
    );

    let invalid_cases = [
        (
            AttentionCategory::Permission,
            AttentionSeverity::Informational,
            true,
        ),
        (
            AttentionCategory::PermissionAudit,
            AttentionSeverity::Critical,
            false,
        ),
        (
            AttentionCategory::Question,
            AttentionSeverity::Critical,
            true,
        ),
        (
            AttentionCategory::Risk,
            AttentionSeverity::ActionRequired,
            false,
        ),
        (
            AttentionCategory::Failure,
            AttentionSeverity::Critical,
            true,
        ),
        (
            AttentionCategory::Stuck,
            AttentionSeverity::Informational,
            true,
        ),
        (
            AttentionCategory::System,
            AttentionSeverity::Informational,
            false,
        ),
        (
            AttentionCategory::Completion,
            AttentionSeverity::ActionRequired,
            false,
        ),
    ];
    for (category, severity, blocking) in invalid_cases {
        assert_eq!(
            AttentionItemDraft::new(
                item_id("invalid"),
                source_event_id("invalid-event"),
                category,
                severity,
                blocking,
                dedupe_key("invalid-key"),
                evidence("invalid-evidence"),
                at(1),
            ),
            Err(AttentionError::InvalidCategoryPolicy {
                category,
                severity,
                blocking,
            })
        );
    }
}

#[test]
fn active_items_sort_by_severity_blocking_created_at_and_ingest_order() {
    let mut projection = AttentionProjection::new(1).expect("valid projection");
    let drafts = [
        draft(
            "info",
            "info",
            AttentionCategory::Stuck,
            AttentionSeverity::Informational,
            false,
            1,
            "info-evidence",
        ),
        draft(
            "action",
            "action",
            AttentionCategory::Failure,
            AttentionSeverity::ActionRequired,
            false,
            1,
            "action-evidence",
        ),
        draft(
            "critical-old",
            "critical-old",
            AttentionCategory::Failure,
            AttentionSeverity::Critical,
            false,
            5,
            "critical-old-evidence",
        ),
        draft(
            "critical-blocking-late",
            "critical-blocking-late",
            AttentionCategory::Risk,
            AttentionSeverity::Critical,
            true,
            30,
            "critical-blocking-late-evidence",
        ),
        draft(
            "critical-blocking-first-tie",
            "critical-blocking-first-tie",
            AttentionCategory::Risk,
            AttentionSeverity::Critical,
            true,
            10,
            "critical-blocking-first-tie-evidence",
        ),
        draft(
            "critical-blocking-second-tie",
            "critical-blocking-second-tie",
            AttentionCategory::Risk,
            AttentionSeverity::Critical,
            true,
            10,
            "critical-blocking-second-tie-evidence",
        ),
    ];
    for (offset, draft) in drafts.into_iter().enumerate() {
        assert_eq!(
            projection
                .apply(offset as u64 + 2, AttentionEvent::Opened(draft))
                .expect("ordered open must apply"),
            AttentionDisposition::Applied
        );
    }

    let ordered_ids = projection
        .active_items_ordered()
        .into_iter()
        .map(|item| item.item_id().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ordered_ids,
        vec![
            "critical-blocking-first-tie",
            "critical-blocking-second-tie",
            "critical-blocking-late",
            "critical-old",
            "action",
            "info",
        ]
    );
    assert_eq!(
        projection.highest_active_severity(),
        Some(AttentionSeverity::Critical)
    );
}

#[test]
fn item_and_dedupe_identity_are_lifetime_unique_after_resolution() {
    let mut projection = AttentionProjection::new(1).expect("valid projection");
    let original = draft(
        "failure",
        "failure:run-1",
        AttentionCategory::Failure,
        AttentionSeverity::Critical,
        false,
        10,
        "failure-open",
    );
    projection
        .apply(2, AttentionEvent::Opened(original.clone()))
        .expect("open must apply");
    assert_eq!(
        projection
            .apply(
                3,
                AttentionEvent::Opened(draft(
                    "failure",
                    "different-key",
                    AttentionCategory::Failure,
                    AttentionSeverity::Critical,
                    false,
                    11,
                    "duplicate-id",
                )),
            )
            .expect("duplicate must be an ordered no-op"),
        AttentionDisposition::Ignored(IgnoredAttentionReason::DuplicateItemId)
    );
    assert_eq!(
        projection
            .apply(
                4,
                AttentionEvent::Opened(draft(
                    "different-id",
                    "failure:run-1",
                    AttentionCategory::Failure,
                    AttentionSeverity::Critical,
                    false,
                    12,
                    "duplicate-key",
                )),
            )
            .expect("duplicate must be an ordered no-op"),
        AttentionDisposition::Ignored(IgnoredAttentionReason::DuplicateDedupeKey)
    );
    projection
        .apply(5, resolve("failure", 20, "failure-resolved"))
        .expect("resolution must apply");
    assert_eq!(
        projection
            .apply(6, AttentionEvent::Opened(original))
            .expect("late duplicate must be an ordered no-op"),
        AttentionDisposition::Ignored(IgnoredAttentionReason::DuplicateItemId)
    );

    assert_eq!(projection.items().len(), 1);
    assert_eq!(
        item(&projection, "failure").status(),
        AttentionStatus::Resolved
    );
    assert_eq!(item(&projection, "failure").version(), 5);
    assert_eq!(projection.version(), 6);
    assert!(projection.active_items_ordered().is_empty());
}

#[test]
fn request_response_statuses_follow_explicit_pending_failure_and_unknown_paths() {
    let mut projection = AttentionProjection::new(1).expect("valid projection");
    projection
        .apply(
            2,
            AttentionEvent::Opened(draft(
                "permission",
                "permission:req-1",
                AttentionCategory::Permission,
                AttentionSeverity::ActionRequired,
                true,
                100,
                "requested",
            )),
        )
        .expect("permission must open");
    assert_eq!(
        projection
            .apply(
                3,
                AttentionEvent::ResponseSubmitted {
                    item_id: item_id("permission"),
                    observed_at: at(90),
                    evidence_id: evidence_id("attempt-1"),
                },
            )
            .expect("submission must apply"),
        AttentionDisposition::Applied
    );
    assert_eq!(item(&projection, "permission").updated_at(), at(100));
    assert_eq!(
        projection
            .apply(
                4,
                AttentionEvent::ResponseSubmitted {
                    item_id: item_id("permission"),
                    observed_at: at(101),
                    evidence_id: evidence_id("duplicate-submit"),
                },
            )
            .expect("duplicate transition must be an ordered no-op"),
        AttentionDisposition::Ignored(
            IgnoredAttentionReason::ResponseSubmissionRequiresOpenRequest
        )
    );
    projection
        .apply(
            5,
            AttentionEvent::ResponseFailed {
                item_id: item_id("permission"),
                observed_at: at(110),
                evidence_id: evidence_id("attempt-1-failed"),
            },
        )
        .expect("explicit failure must reopen");
    assert_eq!(
        item(&projection, "permission").status(),
        AttentionStatus::Open
    );
    projection
        .apply(
            6,
            AttentionEvent::ResponseSubmitted {
                item_id: item_id("permission"),
                observed_at: at(120),
                evidence_id: evidence_id("attempt-2"),
            },
        )
        .expect("a new request attempt mirror must apply");
    projection
        .apply(
            7,
            AttentionEvent::DeliveryUnknown {
                item_id: item_id("permission"),
                observed_at: at(130),
                evidence_id: evidence_id("attempt-2-unknown"),
            },
        )
        .expect("unknown delivery must remain active");
    assert_eq!(
        item(&projection, "permission").status(),
        AttentionStatus::DeliveryUnknown
    );
    assert_eq!(projection.active_items_ordered().len(), 1);
    assert_eq!(
        projection
            .apply(
                8,
                AttentionEvent::ResponseFailed {
                    item_id: item_id("permission"),
                    observed_at: at(140),
                    evidence_id: evidence_id("late-failure"),
                },
            )
            .expect("late failure must be an ordered no-op"),
        AttentionDisposition::Ignored(
            IgnoredAttentionReason::ResponseFailureRequiresPendingRequest
        )
    );
    projection
        .apply(9, resolve("permission", 150, "attempt-2-ack"))
        .expect("matching request reconciliation mirror must resolve");
    assert_eq!(
        item(&projection, "permission").status(),
        AttentionStatus::Resolved
    );
    assert_eq!(
        item(&projection, "permission")
            .evidence()
            .evidence_ids()
            .iter()
            .map(EvidenceId::as_str)
            .collect::<Vec<_>>(),
        vec![
            "requested",
            "attempt-1",
            "attempt-1-failed",
            "attempt-2",
            "attempt-2-unknown",
            "attempt-2-ack",
        ]
    );
}

#[test]
fn request_expiration_requires_an_open_permission_or_question() {
    let mut projection = AttentionProjection::new(1).expect("valid projection");
    projection
        .apply(
            2,
            AttentionEvent::Opened(draft(
                "question",
                "question:req-2",
                AttentionCategory::Question,
                AttentionSeverity::ActionRequired,
                true,
                10,
                "question-open",
            )),
        )
        .expect("question must open");
    projection
        .apply(
            3,
            AttentionEvent::Expired {
                item_id: item_id("question"),
                observed_at: at(20),
                evidence_id: evidence_id("question-expired"),
            },
        )
        .expect("open question must expire");
    assert_eq!(
        item(&projection, "question").status(),
        AttentionStatus::Expired
    );
    assert_eq!(
        projection
            .apply(
                4,
                AttentionEvent::Expired {
                    item_id: item_id("question"),
                    observed_at: at(30),
                    evidence_id: evidence_id("late-expiry"),
                },
            )
            .expect("late expiry must be an ordered no-op"),
        AttentionDisposition::Ignored(IgnoredAttentionReason::ExpirationRequiresOpenRequest)
    );
    assert!(projection.active_items_ordered().is_empty());
}

#[test]
fn acknowledgement_preserves_evidence_and_only_closes_open_non_blocking_items() {
    let mut projection = AttentionProjection::new(1).expect("valid projection");
    projection
        .apply(
            2,
            AttentionEvent::Opened(draft(
                "failure",
                "failure:run",
                AttentionCategory::Failure,
                AttentionSeverity::Critical,
                false,
                10,
                "failure-evidence",
            )),
        )
        .expect("failure must open");
    projection
        .apply(
            3,
            AttentionEvent::Opened(draft(
                "risk",
                "risk:request",
                AttentionCategory::Risk,
                AttentionSeverity::Critical,
                true,
                11,
                "risk-evidence",
            )),
        )
        .expect("blocking risk must open");
    assert_eq!(
        projection
            .apply(
                4,
                AttentionEvent::Acknowledged {
                    item_id: item_id("risk"),
                    observed_at: at(20),
                    evidence_id: evidence_id("risk-ack"),
                },
            )
            .expect("blocking acknowledgement must be an ordered no-op"),
        AttentionDisposition::Ignored(
            IgnoredAttentionReason::AcknowledgementRequiresOpenNonBlockingItem
        )
    );
    projection
        .apply(
            5,
            AttentionEvent::Acknowledged {
                item_id: item_id("failure"),
                observed_at: at(21),
                evidence_id: evidence_id("failure-ack"),
            },
        )
        .expect("non-blocking failure acknowledgement must apply");

    let failure = item(&projection, "failure");
    assert_eq!(failure.status(), AttentionStatus::Acknowledged);
    assert_eq!(failure.source_event_id().as_str(), "event-failure");
    assert_eq!(failure.dedupe_key().as_str(), "failure:run");
    assert_eq!(failure.evidence().evidence_ids().len(), 2);
    assert_eq!(
        projection
            .active_items_ordered()
            .into_iter()
            .map(|item| item.item_id().as_str())
            .collect::<Vec<_>>(),
        vec!["risk"]
    );
}

#[test]
fn invalid_transitions_and_missing_items_preserve_protected_state() {
    let mut projection = AttentionProjection::new(1).expect("valid projection");
    projection
        .apply(
            2,
            AttentionEvent::Opened(draft(
                "stuck",
                "stuck:occurrence",
                AttentionCategory::Stuck,
                AttentionSeverity::Informational,
                false,
                10,
                "stuck-evidence",
            )),
        )
        .expect("stuck item must open");
    let protected_item = item(&projection, "stuck").clone();

    let invalid = [
        AttentionEvent::ResponseSubmitted {
            item_id: item_id("stuck"),
            observed_at: at(20),
            evidence_id: evidence_id("invalid-submit"),
        },
        AttentionEvent::ResponseFailed {
            item_id: item_id("stuck"),
            observed_at: at(21),
            evidence_id: evidence_id("invalid-failure"),
        },
        AttentionEvent::DeliveryUnknown {
            item_id: item_id("stuck"),
            observed_at: at(22),
            evidence_id: evidence_id("invalid-unknown"),
        },
        AttentionEvent::Expired {
            item_id: item_id("stuck"),
            observed_at: at(23),
            evidence_id: evidence_id("invalid-expiry"),
        },
    ];
    for (offset, event) in invalid.into_iter().enumerate() {
        assert!(matches!(
            projection
                .apply(offset as u64 + 3, event)
                .expect("invalid transition must be an ordered no-op"),
            AttentionDisposition::Ignored(_)
        ));
        assert_eq!(item(&projection, "stuck"), &protected_item);
    }
    assert_eq!(
        projection
            .apply(7, resolve("missing", 30, "missing-resolve"))
            .expect("missing target must be an ordered no-op"),
        AttentionDisposition::Ignored(IgnoredAttentionReason::ItemNotFound)
    );
    assert_eq!(item(&projection, "stuck"), &protected_item);
}

#[test]
fn non_monotonic_ingest_fails_without_mutating_projection() {
    let mut projection = AttentionProjection::new(10).expect("valid projection");
    let before = projection.clone();
    assert_eq!(
        projection.apply(
            10,
            AttentionEvent::Opened(draft(
                "failure",
                "failure",
                AttentionCategory::Failure,
                AttentionSeverity::Critical,
                false,
                10,
                "failure",
            )),
        ),
        Err(AttentionError::NonMonotonicIngestSequence {
            current: 10,
            received: 10,
        })
    );
    assert_eq!(projection, before);
}

#[test]
fn ordered_replay_matches_incremental_reduction_including_ignored_events() {
    let events = vec![
        (
            2,
            AttentionEvent::Opened(draft(
                "permission",
                "permission:req",
                AttentionCategory::Permission,
                AttentionSeverity::ActionRequired,
                true,
                100,
                "request",
            )),
        ),
        (
            3,
            AttentionEvent::Opened(draft(
                "duplicate-key",
                "permission:req",
                AttentionCategory::Permission,
                AttentionSeverity::ActionRequired,
                true,
                101,
                "duplicate",
            )),
        ),
        (
            4,
            AttentionEvent::ResponseSubmitted {
                item_id: item_id("permission"),
                observed_at: at(90),
                evidence_id: evidence_id("submitted"),
            },
        ),
        (
            5,
            AttentionEvent::DeliveryUnknown {
                item_id: item_id("permission"),
                observed_at: at(110),
                evidence_id: evidence_id("unknown"),
            },
        ),
        (6, resolve("permission", 120, "reconciled")),
        (
            7,
            AttentionEvent::Opened(draft(
                "completion",
                "completion:run",
                AttentionCategory::Completion,
                AttentionSeverity::Informational,
                false,
                130,
                "completed",
            )),
        ),
        (
            8,
            AttentionEvent::Acknowledged {
                item_id: item_id("completion"),
                observed_at: at(140),
                evidence_id: evidence_id("completion-ack"),
            },
        ),
    ];

    let replayed = replay_attention(1, events.clone()).expect("replay must succeed");
    let mut incremental = AttentionProjection::new(1).expect("valid projection");
    for (ingest_seq, event) in events {
        incremental
            .apply(ingest_seq, event)
            .expect("incremental reduction must succeed");
    }
    assert_eq!(replayed, incremental);
    assert!(replayed.active_items_ordered().is_empty());
    assert_eq!(replayed.items().len(), 2);
}
