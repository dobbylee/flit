use flit_core::{
    activity::WaitKind,
    projection::{
        ActiveAttentionAction, CHANGES_UNAVAILABLE_REASON, ChangeAttribution, ChangeSummary,
        ProjectionError, ProjectionEvent, StuckNotificationProjection, replay_dashboard_projection,
    },
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
        protocol_version: "1.1".to_owned(),
        event_id: format!("event-{ingest_seq}"),
        run_id: RUN_ID.to_owned(),
        session_id: session_id.map(ToOwned::to_owned),
        source_kind: if session_id.is_some() {
            "provider_adapter"
        } else {
            "core"
        }
        .to_owned(),
        source_provider: session_id.map(|_| "codex".to_owned()),
        source_contract_version: None,
        source_has_extensions: false,
        ingest_seq,
        observed_at: format!("2026-07-28T01:00:{ingest_seq:02}.000Z"),
        event_type: event_type.to_owned(),
        payload: payload.as_object().cloned().unwrap_or_else(Map::new),
    }
}

fn stuck_event(
    ingest_seq: u64,
    occurrence_id: &str,
    progress_event_id: &str,
    progress_ms: u64,
) -> ProjectionEvent {
    let mut event = event(
        ingest_seq,
        "run.possibly_stuck",
        None,
        json!({
            "occurrence_id": occurrence_id,
            "cause": "unknown",
            "threshold_seconds": 120,
            "progress_event_id": progress_event_id,
            "progress_observed_at": "2026-08-09T10:00:00Z",
            "progress_monotonic_ms": progress_ms,
            "baseline_monotonic_ms": progress_ms,
            "stuck_since_monotonic_ms": progress_ms + 120_000,
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": progress_ms + 130_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    event.protocol_version = "1.3".to_owned();
    event.source_contract_version = Some("stuck-transition/1.0".to_owned());
    event
}

fn stuck_clear_event(ingest_seq: u64, occurrence_id: &str) -> ProjectionEvent {
    let mut event = event(
        ingest_seq,
        "run.stuck_cleared",
        None,
        json!({
            "occurrence_id": occurrence_id,
            "reason": "lifecycle_inactive",
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 135_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    event.protocol_version = "1.3".to_owned();
    event.source_contract_version = Some("stuck-transition/1.0".to_owned());
    event
}

#[test]
fn explicit_stuck_transitions_own_dashboard_and_attention_replay() {
    let base = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(3, "command.started", Some("session-1"), json!({})),
    ];
    let first = stuck_event(4, "occurrence-1", "event-3", 5_000);
    let stuck = replay_dashboard_projection(&[base.clone(), vec![first.clone()]].concat())
        .expect("first explicit stuck transition");
    assert_eq!(stuck.dashboard_bucket, "PossiblyStuck");
    assert_eq!(stuck.attention_level, "Informational");
    assert_eq!(stuck.attention_open_count, 1);
    let primary = stuck
        .primary_attention
        .as_ref()
        .expect("stuck primary attention");
    assert_eq!(primary.category, "stuck");
    assert_eq!(primary.source_event_id, "event-4");
    assert_eq!(
        primary.action,
        ActiveAttentionAction::StillWorking {
            occurrence_id: "occurrence-1".to_owned(),
        }
    );
    assert_eq!(
        stuck.current_stuck_occurrence_id.as_deref(),
        Some("occurrence-1")
    );
    assert_eq!(
        stuck.last_liveness_at.as_deref(),
        Some(base[2].observed_at.as_str())
    );

    let second = stuck_event(5, "occurrence-2", "event-progress-2", 10_000);
    let replaced =
        replay_dashboard_projection(&[base.clone(), vec![first.clone(), second.clone()]].concat())
            .expect("changed occurrence transition");
    assert_eq!(replaced.dashboard_bucket, "PossiblyStuck");
    assert_eq!(replaced.attention_open_count, 1);

    let mut clear = event(
        6,
        "run.stuck_cleared",
        None,
        json!({
            "occurrence_id": "occurrence-2",
            "reason": "progress_observed",
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 140_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    clear.protocol_version = "1.3".to_owned();
    clear.source_contract_version = Some("stuck-transition/1.0".to_owned());
    let cleared = replay_dashboard_projection(&[base, vec![first, second, clear.clone()]].concat())
        .expect("explicit clear transition");
    assert_eq!(cleared.dashboard_bucket, "Working");
    assert_eq!(cleared.attention_open_count, 0);

    clear.payload["occurrence_id"] = json!("stale-occurrence");
    assert_eq!(
        replay_dashboard_projection(&[
            event(1, "run.created", None, json!({})),
            event(2, "session.connected", Some("session-1"), json!({})),
            stuck_event(3, "occurrence-1", "event-progress-1", 5_000),
            clear,
        ]),
        Err(ProjectionError::Stuck)
    );
}

#[test]
fn still_working_and_notification_receipts_replay_exact_persisted_state() {
    let base = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(3, "command.started", Some("session-1"), json!({})),
        stuck_event(4, "occurrence-1", "event-3", 5_000),
    ];
    let mut still_working = event(
        5,
        "run.still_working",
        None,
        json!({
            "occurrence_id": "occurrence-1",
            "progress_event_id": "event-3",
            "reset_monotonic_ms": 140_000,
            "notification_suppressed_until_monotonic_ms": 740_000,
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 140_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    still_working.protocol_version = "1.4".to_owned();
    still_working.source_contract_version = Some("stuck-action/1.0".to_owned());

    let reset = replay_dashboard_projection(&[base.clone(), vec![still_working.clone()]].concat())
        .expect("exact Still working replay");
    assert_eq!(reset.dashboard_bucket, "Working");
    assert_eq!(reset.attention_open_count, 0);
    assert_eq!(reset.current_stuck_occurrence_id, None);
    assert_eq!(
        reset.stuck_notification,
        StuckNotificationProjection::Suppressed {
            occurrence_id: "occurrence-1".to_owned(),
            until_monotonic_ms: 740_000,
        }
    );
    let reset_receipt = reset.stuck_reset.expect("persisted reset receipt");
    assert_eq!(reset_receipt.progress_event_id, "event-3");
    assert_eq!(reset_receipt.reset_monotonic_ms, 140_000);

    let mut next_occurrence = stuck_event(6, "occurrence-2", "event-3", 5_000);
    next_occurrence.payload["baseline_monotonic_ms"] = json!(140_000);
    next_occurrence.payload["stuck_since_monotonic_ms"] = json!(260_000);
    next_occurrence.payload["process"]["observed_monotonic_ms"] = json!(270_000);
    let suppressed = replay_dashboard_projection(
        &[
            base.clone(),
            vec![still_working.clone(), next_occurrence.clone()],
        ]
        .concat(),
    )
    .expect("same-progress occurrence remains suppressed");
    assert_eq!(
        suppressed.stuck_notification,
        StuckNotificationProjection::Suppressed {
            occurrence_id: "occurrence-2".to_owned(),
            until_monotonic_ms: 740_000,
        }
    );

    let mut due = event(
        7,
        "notification.due",
        None,
        json!({
            "occurrence_id": "occurrence-2",
            "due_at_monotonic_ms": 560_000,
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 740_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    due.protocol_version = "1.4".to_owned();
    due.source_contract_version = Some("stuck-notification/1.0".to_owned());
    let due_projection = replay_dashboard_projection(
        &[
            base.clone(),
            vec![still_working.clone(), next_occurrence.clone(), due.clone()],
        ]
        .concat(),
    )
    .expect("due receipt after suppression boundary");
    assert_eq!(
        due_projection.stuck_notification,
        StuckNotificationProjection::Due {
            occurrence_id: "occurrence-2".to_owned(),
            due_at_monotonic_ms: 560_000,
        }
    );

    let mut delivered = event(
        8,
        "notification.delivered",
        None,
        json!({
            "occurrence_id": "occurrence-2",
            "platform_id": "notification-occurrence-2"
        }),
    );
    delivered.protocol_version = "1.4".to_owned();
    delivered.source_kind = "notifier".to_owned();
    delivered.source_contract_version = Some("stuck-notification/1.0".to_owned());
    let delivered_projection = replay_dashboard_projection(
        &[
            base.clone(),
            vec![
                still_working.clone(),
                next_occurrence.clone(),
                due.clone(),
                delivered,
            ],
        ]
        .concat(),
    )
    .expect("delivered receipt replay");
    assert_eq!(
        delivered_projection.stuck_notification,
        StuckNotificationProjection::Delivered {
            occurrence_id: "occurrence-2".to_owned(),
            platform_id: "notification-occurrence-2".to_owned(),
        }
    );

    let mut stale_due = due;
    stale_due.payload["occurrence_id"] = json!("occurrence-1");
    assert_eq!(
        replay_dashboard_projection(
            &[base, vec![still_working, next_occurrence, stale_due]].concat()
        ),
        Err(ProjectionError::Stuck)
    );
}

#[test]
fn every_terminal_lifecycle_atomically_clears_the_active_stuck_occurrence() {
    for terminal in [
        "run.completed",
        "run.interrupted",
        "run.failed",
        "run.stopped",
    ] {
        let terminal_session =
            matches!(terminal, "run.completed" | "run.interrupted").then_some("session-1");
        let projection = replay_dashboard_projection(&[
            event(1, "run.created", None, json!({})),
            event(2, "session.connected", Some("session-1"), json!({})),
            stuck_event(3, "occurrence-terminal", "event-2", 5_000),
            stuck_clear_event(4, "occurrence-terminal"),
            event(5, terminal, terminal_session, json!({})),
        ])
        .unwrap_or_else(|error| panic!("{terminal} must clear stuck replay: {error}"));
        assert_ne!(projection.dashboard_bucket, "PossiblyStuck", "{terminal}");
        assert_eq!(projection.current_stuck_occurrence_id, None, "{terminal}");
    }
}

#[test]
fn late_stuck_occurrence_requires_an_explicit_due_receipt_before_delivery() {
    let mut late = stuck_event(3, "occurrence-late", "event-2", 5_000);
    late.payload["process"]["observed_monotonic_ms"] = json!(500_000);
    let base = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        late,
    ];
    let projected = replay_dashboard_projection(&base).expect("late occurrence replay");
    assert_eq!(
        projected.stuck_notification,
        StuckNotificationProjection::NotDue {
            occurrence_id: "occurrence-late".to_owned(),
            due_at_monotonic_ms: 425_000,
        }
    );

    let mut backdated_action = event(
        4,
        "run.still_working",
        None,
        json!({
            "occurrence_id": "occurrence-late",
            "progress_event_id": "event-2",
            "reset_monotonic_ms": 130_000,
            "notification_suppressed_until_monotonic_ms": 730_000,
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 130_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    backdated_action.protocol_version = "1.4".to_owned();
    backdated_action.source_contract_version = Some("stuck-action/1.0".to_owned());
    assert_eq!(
        replay_dashboard_projection(&[base.clone(), vec![backdated_action]].concat()),
        Err(ProjectionError::Stuck)
    );

    let mut delivered = event(
        4,
        "notification.delivered",
        None,
        json!({
            "occurrence_id": "occurrence-late",
            "platform_id": "notification-occurrence-late"
        }),
    );
    delivered.protocol_version = "1.4".to_owned();
    delivered.source_kind = "notifier".to_owned();
    delivered.source_contract_version = Some("stuck-notification/1.0".to_owned());
    assert_eq!(
        replay_dashboard_projection(&[base.clone(), vec![delivered.clone()]].concat()),
        Err(ProjectionError::Stuck)
    );

    let mut due = event(
        4,
        "notification.due",
        None,
        json!({
            "occurrence_id": "occurrence-late",
            "due_at_monotonic_ms": 425_000,
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 425_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    due.protocol_version = "1.4".to_owned();
    due.source_contract_version = Some("stuck-notification/1.0".to_owned());
    assert_eq!(
        replay_dashboard_projection(&[base.clone(), vec![due.clone()]].concat()),
        Err(ProjectionError::Stuck)
    );

    due.payload["process"]["observed_monotonic_ms"] = json!(700_000);
    let due_projection = replay_dashboard_projection(&[base.clone(), vec![due.clone()]].concat())
        .expect("later current process receipt establishes due");
    assert!(matches!(
        due_projection.stuck_notification,
        StuckNotificationProjection::Due { .. }
    ));
    let mut after_due_backdated_action = event(
        5,
        "run.still_working",
        None,
        json!({
            "occurrence_id": "occurrence-late",
            "progress_event_id": "event-2",
            "reset_monotonic_ms": 600_000,
            "notification_suppressed_until_monotonic_ms": 1_200_000,
            "process": {
                "status": "alive",
                "generation": "process-generation-1",
                "observed_monotonic_ms": 600_000
            },
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    after_due_backdated_action.protocol_version = "1.4".to_owned();
    after_due_backdated_action.source_contract_version = Some("stuck-action/1.0".to_owned());
    assert_eq!(
        replay_dashboard_projection(
            &[base.clone(), vec![due.clone(), after_due_backdated_action]].concat()
        ),
        Err(ProjectionError::Stuck)
    );
    delivered.ingest_seq = 5;
    replay_dashboard_projection(&[base, vec![due, delivered]].concat())
        .expect("delivery follows an explicit current due receipt");
}

#[test]
fn legacy_stuck_named_events_remain_unknown_and_non_core_v13_is_rejected() {
    let legacy = replay_dashboard_projection(&[
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
        event(3, "run.possibly_stuck", None, json!({"legacy": true})),
        event(4, "run.stuck_cleared", None, json!({"legacy": true})),
    ])
    .expect("legacy event names remain unknown-compatible");
    assert_eq!(legacy.dashboard_bucket, "Working");
    assert_eq!(legacy.attention_open_count, 0);

    for version in ["1.0", "1.1", "1.2", "1.3"] {
        for name in [
            "run.still_working",
            "notification.due",
            "notification.delivered",
        ] {
            let mut lookalike = event(3, name, None, json!({"legacy": true}));
            lookalike.protocol_version = version.to_owned();
            let projection = replay_dashboard_projection(&[
                event(1, "run.created", None, json!({})),
                event(2, "session.connected", Some("session-1"), json!({})),
                lookalike,
            ])
            .unwrap_or_else(|error| panic!("{version} {name} must remain unknown: {error}"));
            assert_eq!(projection.dashboard_bucket, "Working");
            assert_eq!(projection.attention_open_count, 0);
        }
    }

    let mut provider_owned = stuck_event(3, "occurrence-provider", "event-2", 5_000);
    provider_owned.session_id = Some("session-1".to_owned());
    provider_owned.source_kind = "provider_adapter".to_owned();
    provider_owned.source_provider = Some("codex".to_owned());
    assert_eq!(
        replay_dashboard_projection(&[
            event(1, "run.created", None, json!({})),
            event(2, "session.connected", Some("session-1"), json!({})),
            provider_owned,
        ]),
        Err(ProjectionError::Stuck)
    );

    let mut extended_source = stuck_event(3, "occurrence-extended", "event-2", 5_000);
    extended_source.source_has_extensions = true;
    assert_eq!(
        replay_dashboard_projection(&[
            event(1, "run.created", None, json!({})),
            event(2, "session.connected", Some("session-1"), json!({})),
            extended_source,
        ]),
        Err(ProjectionError::Stuck)
    );
}

#[test]
fn current_terminal_changes_require_one_exact_bounded_variant() {
    let base = vec![
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
    ];
    let mut exact = event(
        3,
        "run.completed",
        Some("session-1"),
        json!({
            "changes": {
                "availability": "available",
                "attribution": "exact",
                "files": 3,
                "insertions": 5,
                "deletions": 2
            }
        }),
    );
    exact.protocol_version = "1.2".to_owned();
    let projection = replay_dashboard_projection(&[base.clone(), vec![exact.clone()]].concat())
        .expect("exact terminal changes");
    assert_eq!(
        projection.changes,
        ChangeSummary::Available {
            attribution: ChangeAttribution::Exact,
            files: 3,
            insertions: 5,
            deletions: 2,
        }
    );

    let mut unavailable = exact.clone();
    unavailable.payload["changes"] = json!({
        "availability": "unavailable",
        "reason": "git_terminal_observation_unavailable"
    });
    assert_eq!(
        replay_dashboard_projection(&[base.clone(), vec![unavailable]].concat())
            .expect("unavailable terminal changes")
            .changes,
        ChangeSummary::Unavailable {
            reason: "git_terminal_observation_unavailable".to_owned(),
        }
    );

    let mut legacy = exact.clone();
    legacy.protocol_version = "1.1".to_owned();
    assert_eq!(
        replay_dashboard_projection(&[base.clone(), vec![legacy]].concat())
            .expect("legacy terminal ignores additive changes")
            .changes,
        ChangeSummary::Unavailable {
            reason: CHANGES_UNAVAILABLE_REASON.to_owned(),
        }
    );

    let mut malformed = exact.clone();
    malformed.payload["changes"]["reason"] = json!("mixed");
    assert_eq!(
        replay_dashboard_projection(&[base.clone(), vec![malformed]].concat()),
        Err(ProjectionError::Changes)
    );
    let mut missing = exact;
    missing.payload.remove("changes");
    assert_eq!(
        replay_dashboard_projection(&[base, vec![missing]].concat()),
        Err(ProjectionError::Changes)
    );

    let mut forged_nonterminal = event(
        3,
        "command.started",
        Some("session-1"),
        json!({
            "changes": {
                "availability": "available",
                "attribution": "exact",
                "files": 3,
                "insertions": 5,
                "deletions": 2
            }
        }),
    );
    forged_nonterminal.protocol_version = "1.2".to_owned();
    assert_eq!(
        replay_dashboard_projection(&[
            event(1, "run.created", None, json!({})),
            event(2, "session.connected", Some("session-1"), json!({})),
            forged_nonterminal,
        ]),
        Err(ProjectionError::Changes)
    );
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

    let open = replay_dashboard_projection(&events[..5]).expect("open permission projection");
    let primary = open
        .primary_attention
        .as_ref()
        .expect("open permission primary attention");
    assert_eq!(primary.attention_version, 5);
    assert_eq!(
        primary.content_unavailable_reason,
        "raw_provider_content_not_retained"
    );
    assert_eq!(
        primary.action,
        ActiveAttentionAction::PermissionResponse {
            request_id: "request-1".to_owned(),
            request_version: 5,
        }
    );

    let unknown = replay_dashboard_projection(&events).expect("delivery-unknown projection");
    assert_eq!(unknown.lifecycle, "Running");
    assert_eq!(unknown.activity, "Waiting");
    assert_eq!(unknown.activity_confidence, 0.95);
    assert_eq!(unknown.activity_wait_kind, Some(WaitKind::BlockingRequest));
    assert!(unknown.has_active_blocking_request);
    assert_eq!(unknown.attention_level, "ActionRequired");
    assert_eq!(unknown.attention_open_count, 1);
    let primary = unknown
        .primary_attention
        .as_ref()
        .expect("delivery-unknown primary attention");
    assert_eq!(primary.category, "permission");
    assert_eq!(primary.severity, "ActionRequired");
    assert_eq!(primary.status, "delivery_unknown");
    assert_eq!(primary.source_event_id, "event-5");
    assert_eq!(primary.source_event_type, "permission.requested");
    assert_eq!(
        primary.action,
        ActiveAttentionAction::Unavailable {
            reason: "delivery_unknown_non_retry".to_owned(),
        }
    );
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
    assert!(resolved.primary_attention.is_none());
    assert!(!resolved.has_active_blocking_request);
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
    assert_eq!(
        finished
            .primary_attention
            .as_ref()
            .expect("completion attention")
            .action,
        ActiveAttentionAction::Unavailable {
            reason: "attention_action_not_implemented".to_owned(),
        }
    );
    assert_eq!(finished.dashboard_bucket, "Finished");
}

#[test]
fn recovery_gap_revokes_the_open_permission_action_authority() {
    let mut gap = event(
        6,
        "diagnostic.sequence_gap",
        Some("session-1"),
        json!({
            "gap_reason": "provider_notifications_unavailable_after_restart",
            "evidence_unavailable_reason": "raw_provider_content_not_retained"
        }),
    );
    gap.source_kind = "recovery".to_owned();
    let projection = replay_dashboard_projection(&[
        event(1, "run.created", None, json!({})),
        event(2, "session.connected", Some("session-1"), json!({})),
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
        gap,
    ])
    .expect("recovery-gap projection");

    let primary = projection
        .primary_attention
        .expect("open permission remains visible");
    assert_eq!(primary.status, "open");
    assert_eq!(
        primary.action,
        ActiveAttentionAction::Unavailable {
            reason: "provider_request_authority_lost".to_owned(),
        }
    );
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
    let primary = projection
        .primary_attention
        .as_ref()
        .expect("provider audit attention");
    assert_eq!(primary.category, "permission_audit");
    assert!(matches!(
        primary.action,
        ActiveAttentionAction::Unavailable { .. }
    ));
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
