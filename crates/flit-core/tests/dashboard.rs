use flit_core::{
    activity::{Activity, EvidenceId, TimestampMs},
    attention::{
        AttentionCategory, AttentionDedupeKey, AttentionEvent, AttentionEvidence,
        AttentionItemDraft, AttentionItemId, AttentionProjection, AttentionSeverity, SourceEventId,
    },
    dashboard::{DashboardBucket, dashboard_bucket},
    lifecycle::{LifecycleEvent, LifecycleProjection, SessionId},
    stuck::{ProcessState, StuckContext, StuckPolicy, StuckProjection},
};

fn evidence(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("valid evidence")
}

fn running() -> LifecycleProjection {
    let mut lifecycle = LifecycleProjection::new(1).expect("valid lifecycle");
    lifecycle
        .apply(
            2,
            LifecycleEvent::SessionConnected {
                session_id: SessionId::new("session-1").expect("valid session"),
            },
        )
        .expect("connection applies");
    lifecycle
}

fn assessment(lifecycle: &LifecycleProjection, now: u64) -> flit_core::stuck::StuckAssessment {
    let context = StuckContext::new(
        lifecycle.lifecycle(),
        Activity::Editing,
        None,
        ProcessState::Alive,
        false,
        TimestampMs::new(0),
        evidence("progress"),
    )
    .expect("valid context");
    StuckProjection::new()
        .assess(TimestampMs::new(now), &context, StuckPolicy::default())
        .expect("assessment succeeds")
}

fn open_item(
    attention: &mut AttentionProjection,
    ingest_seq: u64,
    id: &str,
    category: AttentionCategory,
    severity: AttentionSeverity,
    blocking: bool,
) {
    attention
        .apply(
            ingest_seq,
            AttentionEvent::Opened(
                AttentionItemDraft::new(
                    AttentionItemId::new(id).expect("valid item"),
                    SourceEventId::new(format!("source-{id}")).expect("valid source"),
                    category,
                    severity,
                    blocking,
                    AttentionDedupeKey::new(format!("key-{id}")).expect("valid key"),
                    AttentionEvidence::new(vec![evidence(&format!("evidence-{id}"))], None)
                        .expect("valid evidence"),
                    TimestampMs::new(ingest_seq),
                )
                .expect("valid draft"),
            ),
        )
        .expect("open applies");
}

#[test]
fn dashboard_uses_documented_exclusive_bucket_priority() {
    let mut lifecycle = running();
    let mut attention = AttentionProjection::new(1).expect("valid attention");
    let clear = assessment(&lifecycle, 119_999);
    let stuck = assessment(&lifecycle, 120_000);

    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &clear),
        DashboardBucket::Working
    );
    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &stuck),
        DashboardBucket::PossiblyStuck
    );

    open_item(
        &mut attention,
        2,
        "permission",
        AttentionCategory::Permission,
        AttentionSeverity::ActionRequired,
        true,
    );
    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &stuck),
        DashboardBucket::NeedsAttention
    );

    lifecycle
        .apply(3, LifecycleEvent::RunCompleted)
        .expect("completion applies");
    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &stuck),
        DashboardBucket::NeedsAttention
    );

    attention
        .apply(
            3,
            AttentionEvent::Resolved {
                item_id: AttentionItemId::new("permission").expect("valid item"),
                observed_at: TimestampMs::new(3),
                evidence_id: evidence("permission-resolved"),
            },
        )
        .expect("resolution applies");
    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &stuck),
        DashboardBucket::Finished
    );
}

#[test]
fn informational_attention_does_not_promote_a_run_to_needs_attention() {
    let mut lifecycle = running();
    lifecycle
        .apply(3, LifecycleEvent::RunCompleted)
        .expect("completion applies");
    let mut attention = AttentionProjection::new(1).expect("valid attention");
    open_item(
        &mut attention,
        2,
        "completion",
        AttentionCategory::Completion,
        AttentionSeverity::Informational,
        false,
    );

    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &assessment(&lifecycle, 1)),
        DashboardBucket::Finished
    );
}

#[test]
fn acknowledged_failure_moves_a_failed_run_from_needs_attention_to_finished() {
    let mut lifecycle = running();
    lifecycle
        .apply(3, LifecycleEvent::RunFailed)
        .expect("failure applies");
    let mut attention = AttentionProjection::new(1).expect("valid attention");
    open_item(
        &mut attention,
        2,
        "failure",
        AttentionCategory::Failure,
        AttentionSeverity::Critical,
        false,
    );
    let stale_clear = assessment(&lifecycle, 1);
    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &stale_clear),
        DashboardBucket::NeedsAttention
    );

    attention
        .apply(
            3,
            AttentionEvent::Acknowledged {
                item_id: AttentionItemId::new("failure").expect("valid item"),
                observed_at: TimestampMs::new(3),
                evidence_id: evidence("failure-acknowledged"),
            },
        )
        .expect("acknowledgement applies");
    assert_eq!(
        dashboard_bucket(&lifecycle, &attention, &stale_clear),
        DashboardBucket::Finished
    );
}

#[test]
fn active_blocking_query_tracks_only_active_permission_or_question_items() {
    let mut attention = AttentionProjection::new(1).expect("valid attention");
    open_item(
        &mut attention,
        2,
        "informational",
        AttentionCategory::Stuck,
        AttentionSeverity::Informational,
        false,
    );
    assert!(!attention.has_active_blocking_request());

    open_item(
        &mut attention,
        3,
        "risk",
        AttentionCategory::Risk,
        AttentionSeverity::Critical,
        true,
    );
    assert!(!attention.has_active_blocking_request());

    open_item(
        &mut attention,
        4,
        "permission",
        AttentionCategory::Permission,
        AttentionSeverity::ActionRequired,
        true,
    );
    assert!(attention.has_active_blocking_request());

    attention
        .apply(
            5,
            AttentionEvent::Resolved {
                item_id: AttentionItemId::new("permission").expect("valid item"),
                observed_at: TimestampMs::new(5),
                evidence_id: evidence("resolved"),
            },
        )
        .expect("resolution applies");
    assert!(!attention.has_active_blocking_request());
}
