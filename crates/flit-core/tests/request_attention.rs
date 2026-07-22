use flit_core::{
    activity::{EvidenceId, TimestampMs},
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionDisposition, AttentionError,
        AttentionEvent, AttentionEvidence, AttentionItemDraft, AttentionItemId,
        AttentionProjection, AttentionSeverity, AttentionStatus, SourceEventId,
    },
    permission_mode::{PermissionMode, PermissionModeSnapshot, PolicyFingerprint},
    provider_policy::{
        MissingProviderPolicyField, PolicyViolationReason, ProviderPolicyClassification,
        ProviderPolicyDecision, ProviderPolicyValue, ProviderTerminalOutcome,
        VerifiedProviderPolicyOutcome,
    },
    request::{
        RequestDisposition, RequestEvent, RequestId, RequestKind, RequestProjection, RequestStatus,
        ResponseAttemptId,
    },
    request_attention::{
        RequestAttentionError, RequestAttentionObservation, RequestAttentionSource,
        RequestAttentionState, sync_request_attention,
    },
};

fn request_id(value: &str) -> RequestId {
    RequestId::new(value).expect("test request identifier must be valid")
}

fn attempt_id(value: &str) -> ResponseAttemptId {
    ResponseAttemptId::new(value).expect("test attempt identifier must be valid")
}

fn evidence_id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("test evidence identifier must be valid")
}

fn source_event_id(value: &str) -> SourceEventId {
    SourceEventId::new(value).expect("test event identifier must be valid")
}

fn value(value: &str) -> ProviderPolicyValue {
    ProviderPolicyValue::new(value).expect("test provider value must be valid")
}

fn request(value: &str, kind: RequestKind, ingest_seq: u64) -> RequestProjection {
    RequestProjection::new(request_id(value), kind, ingest_seq)
        .expect("test request projection must be valid")
}

fn source(request: &RequestProjection, severity: AttentionSeverity) -> RequestAttentionSource {
    RequestAttentionSource::new(
        request,
        source_event_id(&format!("{}-requested", request.request_id().as_str())),
        severity,
        TimestampMs::new(request.last_ingest_seq() * 100),
        AttentionEvidence::new(
            vec![evidence_id(&format!(
                "{}-request-evidence",
                request.request_id().as_str()
            ))],
            None,
        )
        .expect("test request evidence must be valid"),
    )
    .expect("test request source must be valid")
}

fn observation(label: &str, observed_at: u64) -> RequestAttentionObservation {
    RequestAttentionObservation::new(
        source_event_id(&format!("event-{label}")),
        TimestampMs::new(observed_at),
        evidence_id(&format!("evidence-{label}")),
    )
}

fn approved_mode() -> PermissionModeSnapshot {
    PermissionModeSnapshot::new(
        PermissionMode::ApproveForMe,
        1,
        Some(PolicyFingerprint::new("policy-fp").expect("valid fingerprint")),
    )
    .expect("valid permission mode")
}

fn provider_outcome(
    request_id: &str,
    request_version: u64,
    decision_id: &str,
    evidence_id: &str,
    captured_at_ms: u64,
) -> VerifiedProviderPolicyOutcome {
    VerifiedProviderPolicyOutcome {
        session_key: value("session-1"),
        request_id: crate_request_id(request_id),
        request_version,
        action_fingerprint: value("action-fp"),
        scope_fingerprint: value("scope-fp"),
        bound_mode: approved_mode(),
        policy_source: value("provider-native"),
        policy_version: value("policy-v1"),
        policy_fingerprint: value("policy-fp"),
        decision_id: value(decision_id),
        decision: ProviderPolicyDecision::Allowed,
        terminal_outcome: ProviderTerminalOutcome::RequestResolved,
        captured_at_ms,
        evidence_id: value(evidence_id),
    }
}

fn provider_draft(
    decision_id: &str,
    category: AttentionCategory,
    severity: AttentionSeverity,
    evidence: &str,
    created_at: u64,
) -> AttentionItemDraft {
    AttentionItemDraft::new(
        AttentionItemId::new(format!("provider-policy:{decision_id}"))
            .expect("valid provider item"),
        source_event_id(&format!("provider-event-{decision_id}")),
        category,
        severity,
        false,
        AttentionDedupeKey::new(format!("provider-policy:{decision_id}"))
            .expect("valid provider key"),
        AttentionEvidence::new(vec![evidence_id(evidence)], None).expect("valid provider evidence"),
        TimestampMs::new(created_at),
    )
    .expect("valid provider item draft")
}

fn apply_provider_resolution(
    request: &mut RequestProjection,
    ingest_seq: u64,
    decision_id: &str,
    evidence: &str,
    captured_at: u64,
    violations: Vec<PolicyViolationReason>,
) {
    let request_id = request.request_id().clone();
    let request_version = request.version();
    let outcome = provider_outcome(
        request_id.as_str(),
        request_version,
        decision_id,
        evidence,
        captured_at,
    );
    let classification = if violations.is_empty() {
        ProviderPolicyClassification::InformationalResolve(outcome)
    } else {
        ProviderPolicyClassification::ResolvedWithPolicyViolation {
            outcome,
            reasons: violations,
        }
    };
    request
        .apply(
            ingest_seq,
            RequestEvent::ProviderPolicyClassified {
                request_id,
                expected_request_version: request_version,
                classification: Box::new(classification),
            },
        )
        .expect("test provider resolution must reduce");
}

fn crate_request_id(value: &str) -> RequestId {
    request_id(value)
}

fn sync(
    attention: &mut AttentionProjection,
    request: &RequestProjection,
    source: &RequestAttentionSource,
    label: &str,
) -> Result<Vec<AttentionDisposition>, RequestAttentionError> {
    sync_request_attention(
        attention,
        request.last_ingest_seq(),
        request,
        source,
        observation(label, request.last_ingest_seq() * 100),
    )
}

fn assert_ignored_history_mismatch(
    attention: &mut AttentionProjection,
    request: &RequestProjection,
    source: &RequestAttentionSource,
    label: &str,
) {
    let before = attention.clone();
    assert!(matches!(
        sync(attention, request, source, label),
        Err(RequestAttentionError::IgnoredEventHistoryMismatch { .. })
    ));
    assert_eq!(attention, &before);
}

#[test]
fn source_binds_deterministic_request_identity_category_and_initial_evidence() {
    let permission = request("permission-1", RequestKind::Permission, 2);
    let permission_source = source(&permission, AttentionSeverity::Critical);
    assert_eq!(permission_source.request_id(), permission.request_id());
    assert_eq!(permission_source.request_kind(), RequestKind::Permission);
    assert_eq!(permission_source.item_id().as_str(), "request:permission-1");
    assert_eq!(
        permission_source.dedupe_key().as_str(),
        "request:permission-1"
    );
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &permission, &permission_source, "requested")
        .expect("initial source must open the request item");
    let protected = attention.clone();
    let different_initial_source = RequestAttentionSource::new(
        &permission,
        source_event_id("different-request-event"),
        AttentionSeverity::Critical,
        TimestampMs::new(200),
        AttentionEvidence::new(vec![evidence_id("different-request-evidence")], None)
            .expect("valid evidence"),
    )
    .expect("alternate source is structurally valid");
    assert_eq!(
        sync_request_attention(
            &mut attention,
            2,
            &permission,
            &different_initial_source,
            observation("different-source", 200),
        ),
        Err(RequestAttentionError::IncompatibleRequestAttentionItem)
    );
    assert_eq!(attention, protected);

    let question = request("question-1", RequestKind::Question, 2);
    assert!(matches!(
        RequestAttentionSource::new(
            &question,
            source_event_id("question-requested"),
            AttentionSeverity::Critical,
            TimestampMs::new(200),
            AttentionEvidence::new(vec![evidence_id("question-evidence")], None)
                .expect("valid evidence"),
        ),
        Err(RequestAttentionError::Attention(
            AttentionError::InvalidCategoryPolicy {
                category: AttentionCategory::Question,
                severity: AttentionSeverity::Critical,
                blocking: true,
            }
        ))
    ));
}

#[test]
fn manual_request_statuses_mirror_pending_failure_unknown_and_resolution() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    assert_eq!(
        sync(&mut attention, &request, &source, "requested").expect("open sync"),
        vec![AttentionDisposition::Applied]
    );

    request
        .apply(
            3,
            RequestEvent::ResponseSubmitted {
                expected_version: 2,
                attempt_id: attempt_id("attempt-1"),
            },
        )
        .expect("submission must reduce");
    sync(&mut attention, &request, &source, "submitted").expect("pending sync");
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("request item")
            .status(),
        AttentionStatus::ResponsePending
    );

    request
        .apply(
            4,
            RequestEvent::ResponseFailed {
                attempt_id: attempt_id("attempt-1"),
            },
        )
        .expect("failure must reduce");
    sync(&mut attention, &request, &source, "failed").expect("reopen sync");
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("request item")
            .status(),
        AttentionStatus::Open
    );

    request
        .apply(
            5,
            RequestEvent::ResponseSubmitted {
                expected_version: 4,
                attempt_id: attempt_id("attempt-2"),
            },
        )
        .expect("second submission must reduce");
    sync(&mut attention, &request, &source, "submitted-2").expect("pending sync");
    request
        .apply(
            6,
            RequestEvent::DeliveryUnknown {
                attempt_id: attempt_id("attempt-2"),
            },
        )
        .expect("unknown delivery must reduce");
    sync(&mut attention, &request, &source, "unknown").expect("unknown sync");
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("request item")
            .status(),
        AttentionStatus::DeliveryUnknown
    );

    request
        .apply(
            7,
            RequestEvent::ResponseResolved {
                attempt_id: attempt_id("attempt-2"),
            },
        )
        .expect("reconciliation must reduce");
    sync(&mut attention, &request, &source, "resolved").expect("resolution sync");
    let item = attention.item(source.item_id()).expect("request item");
    assert_eq!(item.status(), AttentionStatus::Resolved);
    assert_eq!(item.version(), 7);
    assert_eq!(item.evidence().evidence_ids().len(), 6);
}

#[test]
fn question_expiration_closes_the_same_attention_item() {
    let mut request = request("question-1", RequestKind::Question, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    request
        .apply(3, RequestEvent::RequestExpired)
        .expect("expiration must reduce");
    sync(&mut attention, &request, &source, "expired").expect("expiration sync");
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("question item")
            .status(),
        AttentionStatus::Expired
    );
    assert!(attention.active_items_ordered().is_empty());
}

#[test]
fn informational_provider_resolution_is_one_batch_with_a_decision_bound_audit() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    let outcome = provider_outcome("permission-1", 2, "decision-1", "provider-evidence", 250);
    request
        .apply(
            3,
            RequestEvent::ProviderPolicyClassified {
                request_id: request_id("permission-1"),
                expected_request_version: 2,
                classification: Box::new(ProviderPolicyClassification::InformationalResolve(
                    outcome,
                )),
            },
        )
        .expect("provider outcome must reduce");

    assert_eq!(
        sync(&mut attention, &request, &source, "provider-resolved")
            .expect("provider resolution sync"),
        vec![AttentionDisposition::Applied, AttentionDisposition::Applied]
    );
    let request_item = attention.item(source.item_id()).expect("request item");
    let audit_item = attention
        .items()
        .iter()
        .find(|item| item.item_id().as_str() == "provider-policy:decision-1")
        .expect("provider audit item");
    assert_eq!(request_item.status(), AttentionStatus::Resolved);
    assert_eq!(request_item.version(), 3);
    assert_eq!(audit_item.version(), 3);
    assert_eq!(audit_item.category(), AttentionCategory::PermissionAudit);
    assert_eq!(audit_item.severity(), AttentionSeverity::Informational);
    assert!(!audit_item.blocking());
    assert_eq!(audit_item.created_at(), TimestampMs::new(250));
    assert_eq!(
        audit_item.evidence().evidence_ids()[0].as_str(),
        "provider-evidence"
    );
    let items_before_ignored = attention.items().to_vec();
    assert!(matches!(
        request
            .apply(4, RequestEvent::RequestExpired)
            .expect("late expiration must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    assert!(
        sync(&mut attention, &request, &source, "late-expiration")
            .expect("late ignored sync")
            .is_empty()
    );
    assert_eq!(attention.version(), 4);
    assert_eq!(attention.items(), items_before_ignored);
}

#[test]
fn provider_policy_violation_resolves_permission_and_opens_critical_non_blocking_risk() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    let outcome = provider_outcome("permission-1", 2, "decision-risk", "risk-evidence", 260);
    request
        .apply(
            3,
            RequestEvent::ProviderPolicyClassified {
                request_id: request_id("permission-1"),
                expected_request_version: 2,
                classification: Box::new(
                    ProviderPolicyClassification::ResolvedWithPolicyViolation {
                        outcome,
                        reasons: vec![PolicyViolationReason::OutOfProjectScope],
                    },
                ),
            },
        )
        .expect("provider violation must reduce");
    sync(&mut attention, &request, &source, "provider-violation").expect("provider violation sync");

    let risk = attention
        .items()
        .iter()
        .find(|item| item.item_id().as_str() == "provider-policy:decision-risk")
        .expect("policy violation risk item");
    assert_eq!(risk.category(), AttentionCategory::Risk);
    assert_eq!(risk.severity(), AttentionSeverity::Critical);
    assert!(!risk.blocking());
    assert_eq!(risk.status(), AttentionStatus::Open);
    assert_eq!(
        attention.highest_active_severity(),
        Some(AttentionSeverity::Critical)
    );
}

#[test]
fn provider_outcome_unknown_keeps_open_then_exact_reconciliation_resolves() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    request
        .apply(
            3,
            RequestEvent::ProviderPolicyClassified {
                request_id: request_id("permission-1"),
                expected_request_version: 2,
                classification: Box::new(ProviderPolicyClassification::ProviderOutcomeUnknown {
                    missing: vec![MissingProviderPolicyField::EvidenceId],
                }),
            },
        )
        .expect("unknown provider outcome must reduce");
    assert_eq!(
        sync(&mut attention, &request, &source, "provider-unknown").expect("unknown sync"),
        vec![AttentionDisposition::Applied]
    );
    assert!(matches!(
        request.status(),
        RequestStatus::ProviderOutcomeUnknown(_)
    ));
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("request item")
            .status(),
        AttentionStatus::Open
    );
    assert_eq!(attention.version(), 3);
    let unknown_item = attention.item(source.item_id()).expect("request item");
    assert_eq!(unknown_item.version(), 3);
    assert_eq!(unknown_item.evidence().evidence_ids().len(), 2);

    let outcome = provider_outcome("permission-1", 2, "decision-1", "provider-evidence", 400);
    request
        .apply(
            4,
            RequestEvent::ProviderPolicyClassified {
                request_id: request_id("permission-1"),
                expected_request_version: 2,
                classification: Box::new(ProviderPolicyClassification::InformationalResolve(
                    outcome,
                )),
            },
        )
        .expect("exact reconciliation must reduce");
    sync(&mut attention, &request, &source, "provider-reconciled").expect("reconciliation sync");
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("request item")
            .status(),
        AttentionStatus::Resolved
    );
}

#[test]
fn binding_and_missing_history_errors_preserve_attention_state() {
    let source_request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&source_request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    let before = attention.clone();

    let other_request = request("permission-2", RequestKind::Permission, 2);
    assert_eq!(
        sync_request_attention(
            &mut attention,
            2,
            &other_request,
            &source,
            observation("mismatch", 200),
        ),
        Err(RequestAttentionError::SourceRequestMismatch)
    );
    assert_eq!(attention, before);

    let other_kind = request("permission-1", RequestKind::Question, 2);
    assert_eq!(
        sync_request_attention(
            &mut attention,
            2,
            &other_kind,
            &source,
            observation("kind-mismatch", 200),
        ),
        Err(RequestAttentionError::SourceKindMismatch)
    );
    assert_eq!(attention, before);
    assert_eq!(
        sync_request_attention(
            &mut attention,
            3,
            &source_request,
            &source,
            observation("sequence-mismatch", 300),
        ),
        Err(RequestAttentionError::RequestIngestSequenceMismatch {
            request: 2,
            received: 3,
        })
    );
    assert_eq!(attention, before);

    let mut pending = source_request.clone();
    pending
        .apply(
            3,
            RequestEvent::ResponseSubmitted {
                expected_version: 2,
                attempt_id: attempt_id("attempt-1"),
            },
        )
        .expect("submission must reduce");
    assert_eq!(
        sync_request_attention(
            &mut attention,
            3,
            &pending,
            &source,
            observation("missing-open", 300),
        ),
        Err(RequestAttentionError::MissingRequestAttentionItem {
            request: RequestAttentionState::ResponsePending,
        })
    );
    assert_eq!(attention, before);

    assert_eq!(
        RequestAttentionSource::new(
            &pending,
            source_event_id("late-source"),
            AttentionSeverity::ActionRequired,
            TimestampMs::new(300),
            AttentionEvidence::new(vec![evidence_id("late-evidence")], None)
                .expect("valid evidence"),
        ),
        Err(RequestAttentionError::SourceRequiresInitialOpenRequest)
    );
}

#[test]
fn request_dedupe_collision_fails_before_advancing_projection() {
    let request = request("permission-1", RequestKind::Permission, 3);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    attention
        .apply(
            2,
            AttentionEvent::Opened(
                AttentionItemDraft::new(
                    AttentionItemId::new("unrelated-failure").expect("valid item"),
                    source_event_id("unrelated-failure-event"),
                    AttentionCategory::Failure,
                    AttentionSeverity::Critical,
                    false,
                    source.dedupe_key().clone(),
                    AttentionEvidence::new(vec![evidence_id("unrelated-evidence")], None)
                        .expect("valid evidence"),
                    TimestampMs::new(100),
                )
                .expect("valid unrelated item"),
            ),
        )
        .expect("collision setup must apply");
    let before = attention.clone();

    assert_eq!(
        sync(&mut attention, &request, &source, "requested"),
        Err(RequestAttentionError::IncompatibleRequestAttentionItem)
    );
    assert_eq!(attention, before);
}

#[test]
fn ignored_or_reopened_request_cannot_become_a_late_initial_source() {
    let mut ignored = request("permission-ignored", RequestKind::Permission, 2);
    let ignored_source = source(&ignored, AttentionSeverity::ActionRequired);
    assert!(matches!(
        ignored
            .apply(
                3,
                RequestEvent::ResponseSubmitted {
                    expected_version: 999,
                    attempt_id: attempt_id("ignored-attempt"),
                },
            )
            .expect("stale submission must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    assert_eq!(
        RequestAttentionSource::new(
            &ignored,
            source_event_id("late-ignored-source"),
            AttentionSeverity::ActionRequired,
            TimestampMs::new(300),
            AttentionEvidence::new(vec![evidence_id("late-ignored-evidence")], None)
                .expect("valid evidence"),
        ),
        Err(RequestAttentionError::SourceRequiresInitialOpenRequest)
    );
    let mut ignored_attention = AttentionProjection::new(1).expect("valid projection");
    let ignored_before = ignored_attention.clone();
    assert_eq!(
        sync(
            &mut ignored_attention,
            &ignored,
            &ignored_source,
            "late-ignored"
        ),
        Err(RequestAttentionError::MissingRequestAttentionItem {
            request: RequestAttentionState::Open,
        })
    );
    assert_eq!(ignored_attention, ignored_before);

    let mut reopened = request("permission-reopened", RequestKind::Permission, 2);
    let reopened_source = source(&reopened, AttentionSeverity::ActionRequired);
    reopened
        .apply(
            3,
            RequestEvent::ResponseSubmitted {
                expected_version: 2,
                attempt_id: attempt_id("reopened-attempt"),
            },
        )
        .expect("submission must reduce");
    reopened
        .apply(
            4,
            RequestEvent::ResponseFailed {
                attempt_id: attempt_id("reopened-attempt"),
            },
        )
        .expect("failure must reopen request");
    assert_eq!(
        RequestAttentionSource::new(
            &reopened,
            source_event_id("late-reopened-source"),
            AttentionSeverity::ActionRequired,
            TimestampMs::new(400),
            AttentionEvidence::new(vec![evidence_id("late-reopened-evidence")], None)
                .expect("valid evidence"),
        ),
        Err(RequestAttentionError::SourceRequiresInitialOpenRequest)
    );
    let mut reopened_attention = AttentionProjection::new(1).expect("valid projection");
    let reopened_before = reopened_attention.clone();
    assert_eq!(
        sync(
            &mut reopened_attention,
            &reopened,
            &reopened_source,
            "late-reopened"
        ),
        Err(RequestAttentionError::MissingRequestAttentionItem {
            request: RequestAttentionState::Open,
        })
    );
    assert_eq!(reopened_attention, reopened_before);
}

#[test]
fn provider_item_with_wrong_evidence_or_time_fails_without_mutation() {
    let cases = [
        (
            "info",
            AttentionCategory::PermissionAudit,
            AttentionSeverity::Informational,
            "wrong-evidence",
            400,
            Vec::new(),
        ),
        (
            "risk",
            AttentionCategory::Risk,
            AttentionSeverity::Critical,
            "provider-evidence-risk",
            399,
            vec![PolicyViolationReason::OutOfProjectScope],
        ),
    ];
    for (decision, category, severity, item_evidence, item_time, violations) in cases {
        let request_id = format!("permission-{decision}");
        let mut request = request(&request_id, RequestKind::Permission, 2);
        let source = source(&request, AttentionSeverity::ActionRequired);
        let mut attention = AttentionProjection::new(1).expect("valid attention projection");
        sync(&mut attention, &request, &source, "requested").expect("open sync");
        apply_provider_resolution(
            &mut request,
            3,
            decision,
            &format!("provider-evidence-{decision}"),
            400,
            violations,
        );
        attention
            .apply_batch(
                3,
                [
                    AttentionEvent::Resolved {
                        item_id: source.item_id().clone(),
                        observed_at: TimestampMs::new(400),
                        evidence_id: evidence_id("request-resolved"),
                    },
                    AttentionEvent::Opened(provider_draft(
                        decision,
                        category,
                        severity,
                        item_evidence,
                        item_time,
                    )),
                ],
            )
            .expect("mismatched provider item setup must apply");
        assert!(matches!(
            request
                .apply(4, RequestEvent::RequestExpired)
                .expect("late expiration must be an ordered no-op"),
            RequestDisposition::Ignored(_)
        ));
        let before = attention.clone();

        assert_eq!(
            sync(&mut attention, &request, &source, "late-ignored"),
            Err(RequestAttentionError::IncompatibleProviderPolicyAttentionItem)
        );
        assert_eq!(attention, before);
    }
}

#[test]
fn preexisting_provider_slot_fails_before_the_request_resolution_batch() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    attention
        .apply(
            3,
            AttentionEvent::Opened(provider_draft(
                "decision-1",
                AttentionCategory::PermissionAudit,
                AttentionSeverity::Informational,
                "provider-evidence",
                400,
            )),
        )
        .expect("provider collision setup must apply");
    apply_provider_resolution(
        &mut request,
        4,
        "decision-1",
        "provider-evidence",
        400,
        Vec::new(),
    );
    let before = attention.clone();

    assert_eq!(
        sync(&mut attention, &request, &source, "provider-resolved"),
        Err(RequestAttentionError::IncompatibleProviderPolicyAttentionItem)
    );
    assert_eq!(attention, before);
    assert_eq!(
        attention
            .item(source.item_id())
            .expect("request item")
            .status(),
        AttentionStatus::Open
    );
}

#[test]
fn missing_provider_item_cannot_be_recreated_by_a_later_ignored_event() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    apply_provider_resolution(
        &mut request,
        3,
        "decision-1",
        "provider-evidence",
        400,
        Vec::new(),
    );
    attention
        .apply(
            3,
            AttentionEvent::Resolved {
                item_id: source.item_id().clone(),
                observed_at: TimestampMs::new(400),
                evidence_id: evidence_id("request-resolved"),
            },
        )
        .expect("resolved request item setup must apply");
    assert!(matches!(
        request
            .apply(4, RequestEvent::RequestExpired)
            .expect("late expiration must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    let before = attention.clone();

    assert_eq!(
        sync(&mut attention, &request, &source, "late-ignored"),
        Err(RequestAttentionError::MissingProviderPolicyAttentionItem)
    );
    assert_eq!(attention, before);
}

#[test]
fn ignored_request_event_advances_attention_version_without_changing_item() {
    let mut request = request("permission-1", RequestKind::Permission, 2);
    let source = source(&request, AttentionSeverity::ActionRequired);
    let mut attention = AttentionProjection::new(1).expect("valid attention projection");
    sync(&mut attention, &request, &source, "requested").expect("open sync");
    let protected_item = attention
        .item(source.item_id())
        .expect("request item")
        .clone();

    assert!(matches!(
        request
            .apply(
                3,
                RequestEvent::ResponseSubmitted {
                    expected_version: 999,
                    attempt_id: attempt_id("stale-attempt"),
                },
            )
            .expect("stale event must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    assert!(
        sync(&mut attention, &request, &source, "stale")
            .expect("no-change sync")
            .is_empty()
    );
    assert_eq!(attention.version(), 3);
    assert_eq!(
        attention.item(source.item_id()).expect("request item"),
        &protected_item
    );
}

#[test]
fn ignored_event_cannot_catch_up_missed_pending_unknown_or_provider_resolution() {
    let mut pending = request("permission-pending", RequestKind::Permission, 2);
    let pending_source = source(&pending, AttentionSeverity::ActionRequired);
    let mut pending_attention = AttentionProjection::new(1).expect("valid projection");
    sync(
        &mut pending_attention,
        &pending,
        &pending_source,
        "pending-requested",
    )
    .expect("initial pending scenario sync");
    pending
        .apply(
            3,
            RequestEvent::ResponseSubmitted {
                expected_version: 2,
                attempt_id: attempt_id("pending-attempt"),
            },
        )
        .expect("pending transition must reduce");
    assert!(matches!(
        pending
            .apply(4, RequestEvent::RequestExpired)
            .expect("late expiration must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    assert_ignored_history_mismatch(
        &mut pending_attention,
        &pending,
        &pending_source,
        "missed-pending",
    );

    let mut unknown = request("permission-unknown", RequestKind::Permission, 2);
    let unknown_source = source(&unknown, AttentionSeverity::ActionRequired);
    let mut unknown_attention = AttentionProjection::new(1).expect("valid projection");
    sync(
        &mut unknown_attention,
        &unknown,
        &unknown_source,
        "unknown-requested",
    )
    .expect("initial unknown scenario sync");
    unknown
        .apply(
            3,
            RequestEvent::ProviderPolicyClassified {
                request_id: request_id("permission-unknown"),
                expected_request_version: 2,
                classification: Box::new(ProviderPolicyClassification::ProviderOutcomeUnknown {
                    missing: vec![MissingProviderPolicyField::EvidenceId],
                }),
            },
        )
        .expect("unknown transition must reduce");
    assert!(matches!(
        unknown
            .apply(4, RequestEvent::RequestExpired)
            .expect("late expiration must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    assert_ignored_history_mismatch(
        &mut unknown_attention,
        &unknown,
        &unknown_source,
        "missed-unknown",
    );

    let mut provider = request("permission-provider", RequestKind::Permission, 2);
    let provider_source = source(&provider, AttentionSeverity::ActionRequired);
    let mut provider_attention = AttentionProjection::new(1).expect("valid projection");
    sync(
        &mut provider_attention,
        &provider,
        &provider_source,
        "provider-requested",
    )
    .expect("initial provider scenario sync");
    apply_provider_resolution(
        &mut provider,
        3,
        "missed-decision",
        "missed-provider-evidence",
        300,
        Vec::new(),
    );
    assert!(matches!(
        provider
            .apply(4, RequestEvent::RequestExpired)
            .expect("late expiration must be an ordered no-op"),
        RequestDisposition::Ignored(_)
    ));
    assert_ignored_history_mismatch(
        &mut provider_attention,
        &provider,
        &provider_source,
        "missed-provider",
    );
}
