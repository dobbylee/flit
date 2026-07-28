use flit_core::projection::{
    CHANGES_UNAVAILABLE_REASON, ChangeSummary, ProjectionError, ProjectionEvent,
    replay_dashboard_projection,
};
use serde_json::{Map, Value, json};

const RUN_ID: &str = "run-projection";

fn event(
    ingest_seq: u64,
    event_type: &str,
    session_id: Option<&str>,
    payload: Value,
) -> ProjectionEvent {
    ProjectionEvent {
        event_id: format!("event-{ingest_seq}"),
        run_id: RUN_ID.to_owned(),
        session_id: session_id.map(ToOwned::to_owned),
        ingest_seq,
        observed_at: format!("2026-07-28T01:00:{ingest_seq:02}.000Z"),
        event_type: event_type.to_owned(),
        payload: payload.as_object().cloned().unwrap_or_else(Map::new),
    }
}

fn provider_auto_request(request_id: &str) -> Value {
    json!({
        "blocking": false,
        "permission_mode": "provider_auto",
        "permission_mode_version": 1,
        "provider_configuration": "readOnly+on-request+auto_review",
        "request_id": request_id,
        "response_supported": false,
        "evidence_unavailable_reason": "raw_provider_content_not_retained"
    })
}

fn provider_auto_outcome(request_id: &str, request_version: u64, decision_id: &str) -> Value {
    json!({
        "provider_decision_id": decision_id,
        "provider_decision": "allowed",
        "permission_mode": "provider_auto",
        "permission_mode_version": 1,
        "provider_configuration": "readOnly+on-request+auto_review",
        "request_id": request_id,
        "request_version": request_version,
        "terminal_outcome": "request_resolved",
        "evidence_unavailable_reason": "raw_provider_content_not_retained"
    })
}

#[test]
fn managed_history_replays_lifecycle_activity_attention_and_unavailable_changes() {
    let mut events = vec![
        event(1, "run.created", None, json!({})),
        event(2, "run.start_requested", None, json!({})),
        event(3, "session.connected", Some("session-1"), json!({})),
        event(4, "command.started", Some("session-1"), json!({})),
        event(
            5,
            "permission.requested",
            Some("session-1"),
            json!({
                "blocking": true,
                "request_id": "request-1",
                "evidence_unavailable_reason": "raw_provider_content_not_retained"
            }),
        ),
        event(
            6,
            "permission.response_submitted",
            Some("session-1"),
            json!({
                "request_id": "request-1",
                "request_version": 5,
                "response_attempt_id": "attempt-1",
                "evidence_unavailable_reason": "provider_delivery_not_attempted_yet"
            }),
        ),
        event(
            7,
            "permission.delivery_unknown",
            Some("session-1"),
            json!({
                "request_id": "request-1",
                "request_version": 5,
                "response_attempt_id": "attempt-1",
                "evidence_unavailable_reason": "provider_delivery_ack_unavailable"
            }),
        ),
    ];

    let unknown = replay_dashboard_projection(&events).expect("delivery-unknown projection");
    assert_eq!(unknown.lifecycle, "Running");
    assert_eq!(unknown.activity, "Waiting");
    assert_eq!(unknown.activity_confidence, 0.95);
    assert_eq!(unknown.attention_level, "ActionRequired");
    assert_eq!(unknown.attention_open_count, 1);
    assert_eq!(unknown.dashboard_bucket, "NeedsAttention");
    assert_eq!(
        unknown.changes,
        ChangeSummary::Unavailable {
            reason: CHANGES_UNAVAILABLE_REASON.to_owned(),
        }
    );

    events.push(event(
        8,
        "permission.resolved",
        Some("session-1"),
        json!({
            "request_id": "request-1",
            "request_version": 5,
            "response_attempt_id": "attempt-1",
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    ));
    let resolved = replay_dashboard_projection(&events).expect("resolved projection");
    assert_eq!(resolved.attention_level, "None");
    assert_eq!(resolved.attention_open_count, 0);
    assert_eq!(resolved.dashboard_bucket, "Working");

    events.push(event(
        9,
        "run.completed",
        Some("session-1"),
        json!({"evidence_unavailable_reason": "raw_provider_content_not_retained"}),
    ));
    let finished = replay_dashboard_projection(&events).expect("finished projection");
    assert_eq!(finished.version, 9);
    assert_eq!(finished.lifecycle, "Finished");
    assert_eq!(finished.activity, "Unknown");
    assert_eq!(finished.attention_level, "Informational");
    assert_eq!(finished.attention_open_count, 1);
    assert_eq!(finished.dashboard_bucket, "Finished");
}

#[test]
fn provider_auto_outcome_is_informational_without_a_flit_response_attempt() {
    let projection = replay_dashboard_projection(&[
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(
            3,
            "permission.requested",
            Some("session-1"),
            provider_auto_request("request-auto"),
        ),
        event(
            4,
            "permission.provider_outcome_resolved",
            Some("session-1"),
            provider_auto_outcome("request-auto", 3, "decision-1"),
        ),
    ])
    .expect("ProviderAuto projection");

    assert_eq!(projection.lifecycle, "Running");
    assert_eq!(projection.attention_level, "Informational");
    assert_eq!(projection.attention_open_count, 1);
    assert_eq!(projection.dashboard_bucket, "Working");
}

#[test]
fn provider_auto_outcome_requires_exact_facts_before_consuming_identity() {
    let base = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(
            3,
            "permission.requested",
            Some("session-1"),
            provider_auto_request("request-auto"),
        ),
    ];
    let mut unknown_decision = provider_auto_outcome("request-auto", 3, "decision-1");
    unknown_decision["provider_decision"] = json!("allow");
    let mut unknown_terminal = provider_auto_outcome("request-auto", 3, "decision-1");
    unknown_terminal["terminal_outcome"] = json!("applied");
    let mut wrong_mode_version = provider_auto_outcome("request-auto", 3, "decision-1");
    wrong_mode_version["permission_mode_version"] = json!(2);
    let mut missing_mode = provider_auto_outcome("request-auto", 3, "decision-1");
    missing_mode
        .as_object_mut()
        .expect("provider outcome object")
        .remove("permission_mode");
    let mut wrong_mode = provider_auto_outcome("request-auto", 3, "decision-1");
    wrong_mode["permission_mode"] = json!("manual");
    let mut wrong_configuration = provider_auto_outcome("request-auto", 3, "decision-1");
    wrong_configuration["provider_configuration"] = json!("different");
    let stale_request = provider_auto_outcome("request-auto", 2, "decision-1");

    for invalid in [
        unknown_decision,
        unknown_terminal,
        missing_mode,
        wrong_mode,
        wrong_mode_version,
        wrong_configuration,
        stale_request,
    ] {
        let mut history = base.clone();
        history.push(event(
            4,
            "permission.provider_outcome_resolved",
            Some("session-1"),
            invalid,
        ));
        let ignored = replay_dashboard_projection(&history).expect("ignored malformed outcome");
        assert_eq!(ignored.version, 4);
        assert_eq!(ignored.attention_level, "None");
        assert_eq!(ignored.attention_open_count, 0);

        history.push(event(
            5,
            "permission.provider_outcome_resolved",
            Some("session-1"),
            provider_auto_outcome("request-auto", 3, "decision-1"),
        ));
        let resolved = replay_dashboard_projection(&history).expect("later exact outcome");
        assert_eq!(resolved.version, 5);
        assert_eq!(resolved.attention_level, "Informational");
        assert_eq!(resolved.attention_open_count, 1);
    }
}

#[test]
fn reused_provider_decision_does_not_consume_a_later_exact_request() {
    let mut history = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(
            3,
            "permission.requested",
            Some("session-1"),
            provider_auto_request("request-1"),
        ),
        event(
            4,
            "permission.provider_outcome_resolved",
            Some("session-1"),
            provider_auto_outcome("request-1", 3, "decision-shared"),
        ),
        event(
            5,
            "permission.requested",
            Some("session-1"),
            provider_auto_request("request-2"),
        ),
        event(
            6,
            "permission.provider_outcome_resolved",
            Some("session-1"),
            provider_auto_outcome("request-2", 5, "decision-shared"),
        ),
    ];
    let reused = replay_dashboard_projection(&history).expect("reused decision ignored");
    assert_eq!(reused.version, 6);
    assert_eq!(reused.attention_open_count, 1);

    history.push(event(
        7,
        "permission.provider_outcome_resolved",
        Some("session-1"),
        provider_auto_outcome("request-2", 5, "decision-new"),
    ));
    let resolved = replay_dashboard_projection(&history).expect("fresh decision accepted");
    assert_eq!(resolved.version, 7);
    assert_eq!(resolved.attention_open_count, 2);
}

#[test]
fn stale_or_unowned_manual_resolution_keeps_permission_attention_active() {
    let base = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(
            3,
            "permission.requested",
            Some("session-1"),
            json!({
                "blocking": true,
                "request_id": "request-1",
                "evidence_unavailable_reason": "raw_provider_content_not_retained"
            }),
        ),
    ];
    let cases = [
        vec![event(
            4,
            "permission.resolved",
            Some("session-1"),
            json!({
                "request_id": "request-1",
                "request_version": 3,
                "response_attempt_id": "attempt-1"
            }),
        )],
        vec![
            event(
                4,
                "permission.response_submitted",
                Some("session-1"),
                json!({
                    "request_id": "request-1",
                    "request_version": 3,
                    "response_attempt_id": "attempt-1"
                }),
            ),
            event(
                5,
                "permission.resolved",
                Some("session-1"),
                json!({
                    "request_id": "request-1",
                    "request_version": 3,
                    "response_attempt_id": "attempt-2"
                }),
            ),
        ],
        vec![
            event(
                4,
                "permission.response_submitted",
                Some("session-1"),
                json!({
                    "request_id": "request-1",
                    "request_version": 3,
                    "response_attempt_id": "attempt-1"
                }),
            ),
            event(
                5,
                "permission.resolved",
                Some("session-1"),
                json!({
                    "request_id": "request-1",
                    "request_version": 2,
                    "response_attempt_id": "attempt-1"
                }),
            ),
        ],
    ];

    for suffix in cases {
        let mut history = base.clone();
        history.extend(suffix);
        let projection = replay_dashboard_projection(&history).expect("safe stale replay");
        assert_eq!(
            projection.version,
            history.last().expect("non-empty history").ingest_seq
        );
        assert_eq!(projection.attention_level, "ActionRequired");
        assert_eq!(projection.attention_open_count, 1);
        assert_eq!(projection.dashboard_bucket, "NeedsAttention");
    }
}

#[test]
fn replay_rejects_missing_creation_cross_run_and_invalid_connected_identity() {
    assert_eq!(
        replay_dashboard_projection(&[]),
        Err(ProjectionError::EmptyEventHistory)
    );
    assert_eq!(
        replay_dashboard_projection(&[event(1, "command.started", None, json!({}))]),
        Err(ProjectionError::MissingRunCreated)
    );

    let created = event(1, "run.created", None, json!({}));
    let mut other_run = event(2, "run.start_requested", None, json!({}));
    other_run.run_id = "other-run".to_owned();
    assert_eq!(
        replay_dashboard_projection(&[created.clone(), other_run]),
        Err(ProjectionError::RunIdentityMismatch)
    );
    assert_eq!(
        replay_dashboard_projection(&[created, event(2, "session.connected", None, json!({}))]),
        Err(ProjectionError::InvalidSessionIdentity)
    );
}
