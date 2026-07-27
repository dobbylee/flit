use flit_core::permission_mode::{
    CompletedPermissionModeOutcome, IgnoredPermissionModeReason, ModeConfigurationState,
    ModeObservationBinding, ModeObservationUnknownReason, OrderedProviderCursor,
    PendingModeObservation, PermissionMode, PermissionModeDisposition, PermissionModeError,
    PermissionModeEvent, PermissionModeOperationId, PermissionModeProjection,
    PermissionModeSnapshot, PermissionModeValueError, ProviderConfigurationIdentity,
    ProviderStreamId, replay_permission_mode,
};

fn operation(value: &str) -> PermissionModeOperationId {
    PermissionModeOperationId::new(value).expect("test operation ID must be valid")
}

fn configuration_identity(value: &str) -> ProviderConfigurationIdentity {
    ProviderConfigurationIdentity::new(value).expect("test configuration_identity must be valid")
}

fn stream(value: &str) -> ProviderStreamId {
    ProviderStreamId::new(value).expect("test provider stream ID must be valid")
}

fn cursor(stream_value: &str, position: u64) -> OrderedProviderCursor {
    OrderedProviderCursor::new(stream(stream_value), position)
}

fn observation(
    operation_value: &str,
    cursor: Option<OrderedProviderCursor>,
) -> PendingModeObservation {
    PendingModeObservation::new(operation(operation_value), cursor)
}

fn snapshot(
    mode: PermissionMode,
    version: u64,
    configuration_identity_value: &str,
) -> PermissionModeSnapshot {
    PermissionModeSnapshot::new(
        mode,
        version,
        Some(configuration_identity(configuration_identity_value)),
    )
    .expect("test verified snapshot must be valid")
}

fn unknown_snapshot(version: u64) -> PermissionModeSnapshot {
    PermissionModeSnapshot::new(PermissionMode::Unknown, version, None)
        .expect("test unknown snapshot must be valid")
}

fn projection(initial: PermissionModeSnapshot) -> PermissionModeProjection {
    PermissionModeProjection::new(initial, 10).expect("initial ingest sequence must be valid")
}

fn submit(
    projection: &mut PermissionModeProjection,
    ingest_seq: u64,
    operation_value: &str,
    expected_mode_version: u64,
    requested: PermissionModeSnapshot,
) -> PermissionModeDisposition {
    projection
        .apply(
            ingest_seq,
            PermissionModeEvent::ChangeSubmitted {
                operation_id: operation(operation_value),
                expected_mode_version,
                requested,
            },
        )
        .expect("mode event must be ordered")
}

#[test]
fn value_types_enforce_identity_version_and_configuration_identity_invariants() {
    assert_eq!(
        PermissionModeOperationId::new("\n\t"),
        Err(PermissionModeValueError::BlankOperationId)
    );
    assert_eq!(
        ProviderConfigurationIdentity::new("   "),
        Err(PermissionModeValueError::BlankProviderConfigurationIdentity)
    );
    assert_eq!(
        ProviderStreamId::new("\t\n"),
        Err(PermissionModeValueError::BlankProviderStreamId)
    );
    let zero_cursor = cursor("stream-1", 0);
    assert_eq!(zero_cursor.stream_id().as_str(), "stream-1");
    assert_eq!(zero_cursor.position(), 0);
    assert_eq!(
        PermissionModeSnapshot::new(
            PermissionMode::Manual,
            0,
            Some(configuration_identity("fp"))
        ),
        Err(PermissionModeValueError::InvalidModeVersion)
    );
    assert_eq!(
        PermissionModeSnapshot::new(PermissionMode::Manual, 1, None),
        Err(PermissionModeValueError::VerifiedModeRequiresProviderConfiguration)
    );
    assert_eq!(
        PermissionModeSnapshot::new(
            PermissionMode::Unknown,
            1,
            Some(configuration_identity("unexpected")),
        ),
        Err(PermissionModeValueError::UnknownModeHasProviderConfiguration)
    );
    assert_eq!(
        PermissionModeProjection::new(snapshot(PermissionMode::Manual, 1, "fp"), 0),
        Err(PermissionModeError::InvalidInitialIngestSequence)
    );
}

#[test]
fn stable_authority_path_uses_the_request_time_mode_while_unknown_fails_closed() {
    let manual_request = snapshot(PermissionMode::Manual, 1, "manual-fp");
    let provider_auto_request = snapshot(PermissionMode::ProviderAuto, 2, "provider-auto-config");
    let unknown_request = unknown_snapshot(3);

    for current_mode in [PermissionMode::Manual, PermissionMode::ProviderAuto] {
        let projection = projection(snapshot(current_mode, 4, "current-config"));
        assert!(projection.permission_response_enabled(&manual_request));
        assert!(!projection.provider_outcome_observation_enabled(&manual_request));
        assert!(!projection.permission_response_enabled(&provider_auto_request));
        assert!(projection.provider_outcome_observation_enabled(&provider_auto_request));
        assert!(!projection.permission_response_enabled(&unknown_request));
        assert!(!projection.provider_outcome_observation_enabled(&unknown_request));
        assert_eq!(
            projection.configuration_state(),
            &ModeConfigurationState::Stable
        );
    }

    let unknown_projection = projection(unknown_snapshot(7));
    assert_eq!(unknown_projection.current().mode(), PermissionMode::Unknown);
    assert!(
        unknown_projection
            .current()
            .provider_configuration_identity()
            .is_none()
    );
    assert!(!unknown_projection.permission_response_enabled(&manual_request));
    assert!(!unknown_projection.provider_outcome_observation_enabled(&provider_auto_request));
}

#[test]
fn exact_current_and_next_versions_submit_one_fresh_operation_and_lock_controls() {
    let mut projection = projection(snapshot(PermissionMode::Manual, 4, "manual-fp"));
    let requested = snapshot(PermissionMode::ProviderAuto, 5, "provider-auto-config");

    assert_eq!(
        submit(&mut projection, 11, "operation-1", 4, requested.clone()),
        PermissionModeDisposition::Applied
    );
    let ModeConfigurationState::Pending(change) = projection.configuration_state() else {
        panic!("configuration must be pending");
    };
    assert_eq!(change.operation_id().as_str(), "operation-1");
    assert_eq!(change.expected_mode_version(), 4);
    assert_eq!(change.prior(), projection.current());
    assert_eq!(change.requested(), &requested);
    assert_eq!(projection.current().mode(), PermissionMode::Manual);
    assert_eq!(projection.current().version(), 4);
    assert!(!projection.permission_response_enabled(projection.current()));
    assert!(!projection.provider_outcome_observation_enabled(&requested));
    assert_eq!(projection.last_ingest_seq(), 11);
    assert_eq!(projection.used_operation_ids(), &[operation("operation-1")]);

    assert_eq!(
        submit(
            &mut projection,
            12,
            "operation-2",
            4,
            snapshot(PermissionMode::Manual, 5, "manual-fp-2"),
        ),
        PermissionModeDisposition::Ignored(
            IgnoredPermissionModeReason::ConfigurationAlreadyPending
        )
    );
    assert_eq!(projection.last_ingest_seq(), 12);
    assert_eq!(projection.used_operation_ids().len(), 1);
}

#[test]
fn stale_and_invalid_requested_versions_do_not_start_an_operation() {
    let mut projection = projection(snapshot(PermissionMode::Manual, 4, "manual-fp"));

    assert_eq!(
        submit(
            &mut projection,
            11,
            "operation-stale",
            3,
            snapshot(PermissionMode::ProviderAuto, 5, "provider-auto-config"),
        ),
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::StaleExpectedModeVersion {
            current: 4,
            received: 3,
        })
    );
    assert_eq!(
        submit(
            &mut projection,
            12,
            "operation-skip",
            4,
            snapshot(PermissionMode::ProviderAuto, 6, "provider-auto-config"),
        ),
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::InvalidNextModeVersion {
            expected: 5,
            received: 6,
        })
    );
    assert_eq!(
        submit(
            &mut projection,
            13,
            "operation-unknown",
            4,
            unknown_snapshot(5),
        ),
        PermissionModeDisposition::Ignored(
            IgnoredPermissionModeReason::RequestedModeMustBeVerified
        )
    );
    assert_eq!(projection.current().version(), 4);
    assert_eq!(
        projection.configuration_state(),
        &ModeConfigurationState::Stable
    );
    assert!(projection.used_operation_ids().is_empty());
    assert_eq!(projection.last_ingest_seq(), 13);
}

#[test]
fn exhausted_mode_version_rejects_new_operations() {
    let mut projection = projection(snapshot(PermissionMode::Manual, u64::MAX, "manual-fp"));
    let requested = snapshot(PermissionMode::ProviderAuto, 1, "provider-auto-config");

    assert_eq!(
        submit(
            &mut projection,
            11,
            "operation-overflow",
            u64::MAX,
            requested,
        ),
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::ModeVersionExhausted)
    );
    assert!(projection.used_operation_ids().is_empty());
}

#[test]
fn exact_success_commits_requested_mode_version_and_configuration_identity() {
    let mut projection = projection(snapshot(PermissionMode::Manual, 4, "manual-fp"));
    let requested = snapshot(PermissionMode::ProviderAuto, 5, "provider-auto-config");
    assert_eq!(
        submit(&mut projection, 11, "operation-1", 4, requested.clone()),
        PermissionModeDisposition::Applied
    );

    assert_eq!(
        projection
            .apply(
                12,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-1"),
                    applied: requested.clone(),
                    effective_cursor: None,
                },
            )
            .expect("success receipt must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(projection.current(), &requested);
    assert_eq!(
        projection.configuration_state(),
        &ModeConfigurationState::Stable
    );
    assert!(!projection.permission_response_enabled(&requested));
    assert!(projection.provider_outcome_observation_enabled(&requested));
    assert!(projection.permission_response_enabled(&snapshot(
        PermissionMode::Manual,
        4,
        "manual-fp",
    )));

    assert_eq!(
        projection
            .apply(
                13,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-1"),
                    applied: requested,
                    effective_cursor: None,
                },
            )
            .expect("duplicate receipt must be ordered"),
        PermissionModeDisposition::Ignored(
            IgnoredPermissionModeReason::StaleOrDuplicateOperationReceipt
        )
    );
}

#[test]
fn malformed_matching_receipt_locks_unknown_until_exact_reconciliation() {
    let initial = snapshot(PermissionMode::Manual, 4, "manual-fp");
    let requested = snapshot(PermissionMode::ProviderAuto, 5, "provider-auto-config");
    let mut projection = projection(initial.clone());
    assert_eq!(
        submit(&mut projection, 11, "operation-1", 4, requested.clone()),
        PermissionModeDisposition::Applied
    );

    assert_eq!(
        projection
            .apply(
                12,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-1"),
                    applied: snapshot(PermissionMode::ProviderAuto, 5, "wrong-fp"),
                    effective_cursor: None,
                },
            )
            .expect("mismatched receipt must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(projection.current(), &initial);
    assert!(matches!(
        projection.configuration_state(),
        ModeConfigurationState::Unknown(_)
    ));
    assert!(!projection.permission_response_enabled(&initial));
    assert!(!projection.provider_outcome_observation_enabled(&requested));

    assert_eq!(
        projection
            .apply(
                13,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-1"),
                    applied: snapshot(PermissionMode::ProviderAuto, 5, "still-wrong"),
                    effective_cursor: None,
                },
            )
            .expect("mismatched reconciliation must be ordered"),
        PermissionModeDisposition::Ignored(
            IgnoredPermissionModeReason::ConfigurationAlreadyUnknown
        )
    );
    assert_eq!(
        projection
            .apply(
                14,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-1"),
                    applied: requested.clone(),
                    effective_cursor: None,
                },
            )
            .expect("exact reconciliation must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(projection.current(), &requested);
    assert_eq!(
        projection.configuration_state(),
        &ModeConfigurationState::Stable
    );
}

#[test]
fn authenticated_not_applied_rejection_unlocks_prior_mode_without_version_change() {
    let initial = snapshot(PermissionMode::ProviderAuto, 8, "provider-auto-config");
    let mut projection = projection(initial.clone());
    assert_eq!(
        submit(
            &mut projection,
            11,
            "operation-1",
            8,
            snapshot(PermissionMode::Manual, 9, "manual-fp"),
        ),
        PermissionModeDisposition::Applied
    );
    assert_eq!(
        projection
            .apply(
                12,
                PermissionModeEvent::ConfigurationRejectedNotApplied {
                    operation_id: operation("operation-1"),
                },
            )
            .expect("rejection must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(projection.current(), &initial);
    assert_eq!(
        projection.configuration_state(),
        &ModeConfigurationState::Stable
    );
    assert!(!projection.permission_response_enabled(&initial));
    assert!(projection.provider_outcome_observation_enabled(&initial));

    assert_eq!(
        submit(
            &mut projection,
            13,
            "operation-1",
            8,
            snapshot(PermissionMode::Manual, 9, "manual-fp"),
        ),
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::OperationAlreadyUsed)
    );
    assert_eq!(
        submit(
            &mut projection,
            14,
            "operation-2",
            8,
            snapshot(PermissionMode::Manual, 9, "manual-fp"),
        ),
        PermissionModeDisposition::Applied
    );
}

#[test]
fn application_unknown_is_durable_and_same_operation_rejection_can_reconcile() {
    let initial = snapshot(PermissionMode::Manual, 2, "manual-fp");
    let requested = snapshot(PermissionMode::ProviderAuto, 3, "provider-auto-config");
    let mut projection = projection(initial.clone());
    assert_eq!(
        submit(&mut projection, 11, "operation-1", 2, requested),
        PermissionModeDisposition::Applied
    );
    assert_eq!(
        projection
            .apply(
                12,
                PermissionModeEvent::ConfigurationApplicationUnknown {
                    operation_id: operation("operation-1"),
                },
            )
            .expect("unknown outcome must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert!(matches!(
        projection.configuration_state(),
        ModeConfigurationState::Unknown(_)
    ));
    assert_eq!(projection.current(), &initial);

    assert_eq!(
        submit(
            &mut projection,
            13,
            "operation-2",
            2,
            snapshot(PermissionMode::ProviderAuto, 3, "provider-auto-config"),
        ),
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::ConfigurationUnknownLocked)
    );
    assert_eq!(
        projection
            .apply(
                14,
                PermissionModeEvent::ConfigurationApplicationUnknown {
                    operation_id: operation("operation-1"),
                },
            )
            .expect("duplicate unknown must be ordered"),
        PermissionModeDisposition::Ignored(
            IgnoredPermissionModeReason::ConfigurationAlreadyUnknown
        )
    );
    assert_eq!(
        projection
            .apply(
                15,
                PermissionModeEvent::ConfigurationRejectedNotApplied {
                    operation_id: operation("operation-1"),
                },
            )
            .expect("reconciliation rejection must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(projection.current(), &initial);
    assert_eq!(
        projection.configuration_state(),
        &ModeConfigurationState::Stable
    );
}

#[test]
fn stale_consumed_receipt_is_audit_only_but_unrelated_active_receipt_locks_unknown() {
    let mut projection = projection(snapshot(PermissionMode::Manual, 1, "manual-fp"));
    let first = snapshot(PermissionMode::ProviderAuto, 2, "provider-auto-config");
    assert_eq!(
        submit(&mut projection, 11, "operation-1", 1, first.clone()),
        PermissionModeDisposition::Applied
    );
    projection
        .apply(
            12,
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id: operation("operation-1"),
                applied: first,
                effective_cursor: None,
            },
        )
        .expect("first success must be ordered");
    assert_eq!(
        submit(
            &mut projection,
            13,
            "operation-2",
            2,
            snapshot(PermissionMode::Manual, 3, "manual-fp-2"),
        ),
        PermissionModeDisposition::Applied
    );

    assert_eq!(
        projection
            .apply(
                14,
                PermissionModeEvent::ConfigurationRejectedNotApplied {
                    operation_id: operation("operation-1"),
                },
            )
            .expect("stale receipt must be ordered"),
        PermissionModeDisposition::Ignored(
            IgnoredPermissionModeReason::StaleOrDuplicateOperationReceipt
        )
    );
    assert!(matches!(
        projection.configuration_state(),
        ModeConfigurationState::Pending(change)
            if change.operation_id().as_str() == "operation-2"
    ));

    assert_eq!(
        projection
            .apply(
                15,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-unrelated"),
                    applied: snapshot(PermissionMode::Manual, 3, "manual-fp-2"),
                    effective_cursor: None,
                },
            )
            .expect("unrelated receipt must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert!(matches!(
        projection.configuration_state(),
        ModeConfigurationState::Unknown(change)
            if change.operation_id().as_str() == "operation-2"
    ));
}

#[test]
fn pending_and_unknown_observations_fail_closed_until_exact_reconciliation() {
    let initial = snapshot(PermissionMode::Manual, 1, "manual-fp");
    let requested = snapshot(PermissionMode::ProviderAuto, 2, "provider-auto-config");
    let mut projection = projection(initial);
    assert_eq!(
        submit(&mut projection, 11, "operation-1", 1, requested.clone()),
        PermissionModeDisposition::Applied
    );
    let pending = observation("operation-1", Some(cursor("stream-1", 50)));
    assert_eq!(
        projection.bind_pending_observation(&pending),
        ModeObservationBinding::AwaitingConfiguration
    );
    assert_eq!(
        projection.bind_pending_observation(&observation(
            "operation-unknown",
            Some(cursor("stream-1", 50)),
        )),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::UnknownOperation
        )
    );

    assert_eq!(
        projection
            .apply(
                12,
                PermissionModeEvent::ConfigurationApplicationUnknown {
                    operation_id: operation("operation-1"),
                },
            )
            .expect("unknown outcome must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(
        projection.bind_pending_observation(&pending),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::ConfigurationApplicationUnknown
        )
    );

    assert_eq!(
        projection
            .apply(
                13,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-1"),
                    applied: requested.clone(),
                    effective_cursor: Some(cursor("stream-1", 50)),
                },
            )
            .expect("reconciliation must be ordered"),
        PermissionModeDisposition::Applied
    );
    assert_eq!(
        projection.bind_pending_observation(&pending),
        ModeObservationBinding::Bound(requested)
    );
    assert_eq!(projection.completed_changes().len(), 1);
}

#[test]
fn configured_cursor_boundary_binds_prior_before_and_requested_at_or_after_effective() {
    let initial = snapshot(PermissionMode::Manual, 4, "manual-fp");
    let requested = snapshot(PermissionMode::ProviderAuto, 5, "provider-auto-config");
    let mut projection = projection(initial.clone());
    assert_eq!(
        submit(&mut projection, 11, "operation-1", 4, requested.clone()),
        PermissionModeDisposition::Applied
    );
    projection
        .apply(
            12,
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id: operation("operation-1"),
                applied: requested.clone(),
                effective_cursor: Some(cursor("stream-1", 100)),
            },
        )
        .expect("success receipt must be ordered");

    assert_eq!(
        projection
            .bind_pending_observation(&observation("operation-1", Some(cursor("stream-1", 99)),)),
        ModeObservationBinding::Bound(initial.clone())
    );
    for position in [100, 101] {
        assert_eq!(
            projection.bind_pending_observation(&observation(
                "operation-1",
                Some(cursor("stream-1", position)),
            )),
            ModeObservationBinding::Bound(requested.clone())
        );
    }

    let completed = projection
        .completed_changes()
        .first()
        .expect("completed change must be retained");
    assert_eq!(completed.change().prior(), &initial);
    assert_eq!(completed.change().requested(), &requested);
    assert!(matches!(
        completed.outcome(),
        CompletedPermissionModeOutcome::Configured {
            effective_cursor: Some(boundary)
        } if boundary.position() == 100
    ));
}

#[test]
fn missing_or_incomparable_cursor_is_unknown_and_old_completion_survives_new_change() {
    let first_initial = snapshot(PermissionMode::Manual, 1, "manual-fp");
    let first_requested = snapshot(PermissionMode::ProviderAuto, 2, "provider-auto-config");
    let mut projection = projection(first_initial);
    assert_eq!(
        submit(
            &mut projection,
            11,
            "operation-1",
            1,
            first_requested.clone(),
        ),
        PermissionModeDisposition::Applied
    );
    projection
        .apply(
            12,
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id: operation("operation-1"),
                applied: first_requested,
                effective_cursor: None,
            },
        )
        .expect("first success must be ordered");
    assert_eq!(
        projection
            .bind_pending_observation(&observation("operation-1", Some(cursor("stream-1", 10)),)),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::MissingEffectiveCursor
        )
    );

    let second_requested = snapshot(PermissionMode::Manual, 3, "manual-fp-2");
    assert_eq!(
        submit(
            &mut projection,
            13,
            "operation-2",
            2,
            second_requested.clone(),
        ),
        PermissionModeDisposition::Applied
    );
    projection
        .apply(
            14,
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id: operation("operation-2"),
                applied: second_requested.clone(),
                effective_cursor: Some(cursor("stream-2", 20)),
            },
        )
        .expect("second success must be ordered");

    assert_eq!(projection.completed_changes().len(), 2);
    assert_eq!(
        projection.bind_pending_observation(&observation("operation-2", None)),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::MissingObservationCursor
        )
    );
    assert_eq!(
        projection.bind_pending_observation(&observation(
            "operation-2",
            Some(cursor("other-stream", 20)),
        )),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::IncomparableProviderStream
        )
    );
    assert_eq!(
        projection
            .bind_pending_observation(&observation("operation-2", Some(cursor("stream-2", 20)),)),
        ModeObservationBinding::Bound(second_requested)
    );
    assert_eq!(
        projection
            .bind_pending_observation(&observation("operation-1", Some(cursor("stream-1", 10)),)),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::MissingEffectiveCursor
        )
    );
}

#[test]
fn authenticated_rejection_binds_pending_observation_to_prior_without_a_cursor() {
    let initial = snapshot(PermissionMode::ProviderAuto, 7, "provider-auto-config");
    let mut projection = projection(initial.clone());
    assert_eq!(
        submit(
            &mut projection,
            11,
            "operation-1",
            7,
            snapshot(PermissionMode::Manual, 8, "manual-fp"),
        ),
        PermissionModeDisposition::Applied
    );
    projection
        .apply(
            12,
            PermissionModeEvent::ConfigurationRejectedNotApplied {
                operation_id: operation("operation-1"),
            },
        )
        .expect("rejection must be ordered");

    assert_eq!(
        projection.bind_pending_observation(&observation("operation-1", None)),
        ModeObservationBinding::Bound(initial)
    );
    assert!(matches!(
        projection
            .completed_changes()
            .first()
            .expect("completed rejection must be retained")
            .outcome(),
        CompletedPermissionModeOutcome::RejectedNotApplied
    ));
}

#[test]
fn receipt_without_active_configuration_is_ignored() {
    let mut projection = projection(snapshot(PermissionMode::Manual, 1, "manual-fp"));
    assert_eq!(
        projection
            .apply(
                11,
                PermissionModeEvent::ConfigurationSucceeded {
                    operation_id: operation("operation-never-submitted"),
                    applied: snapshot(PermissionMode::ProviderAuto, 2, "provider-auto-config"),
                    effective_cursor: None,
                },
            )
            .expect("orphan receipt must be ordered"),
        PermissionModeDisposition::Ignored(IgnoredPermissionModeReason::NoActiveConfiguration)
    );
    assert_eq!(projection.current().mode(), PermissionMode::Manual);
    assert_eq!(projection.last_ingest_seq(), 11);
}

#[test]
fn non_monotonic_event_is_rejected_without_mutating_projection() {
    let mut projection = projection(snapshot(PermissionMode::Manual, 1, "manual-fp"));
    assert_eq!(
        submit(
            &mut projection,
            11,
            "operation-1",
            1,
            snapshot(PermissionMode::ProviderAuto, 2, "provider-auto-config"),
        ),
        PermissionModeDisposition::Applied
    );
    let before = projection.clone();

    assert_eq!(
        projection.apply(
            11,
            PermissionModeEvent::ConfigurationApplicationUnknown {
                operation_id: operation("operation-1"),
            },
        ),
        Err(PermissionModeError::NonMonotonicIngestSequence {
            current: 11,
            received: 11,
        })
    );
    assert_eq!(projection, before);
}

#[test]
fn replay_matches_incremental_reduction_with_unknown_reconciliation() {
    let initial = unknown_snapshot(4);
    let requested = snapshot(PermissionMode::Manual, 5, "manual-fp");
    let events = vec![
        (
            11,
            PermissionModeEvent::ChangeSubmitted {
                operation_id: operation("operation-1"),
                expected_mode_version: 4,
                requested: requested.clone(),
            },
        ),
        (
            12,
            PermissionModeEvent::ConfigurationApplicationUnknown {
                operation_id: operation("operation-1"),
            },
        ),
        (
            13,
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id: operation("operation-1"),
                applied: requested.clone(),
                effective_cursor: None,
            },
        ),
        (
            14,
            PermissionModeEvent::ConfigurationSucceeded {
                operation_id: operation("operation-1"),
                applied: requested.clone(),
                effective_cursor: None,
            },
        ),
    ];
    let replayed = replay_permission_mode(initial.clone(), 10, events.clone())
        .expect("ordered replay must succeed");
    let mut incremental = projection(initial);
    for (ingest_seq, event) in events {
        incremental.apply(ingest_seq, event).expect("ordered event");
    }

    assert_eq!(replayed, incremental);
    assert_eq!(replayed.current(), &requested);
    assert_eq!(
        replayed.configuration_state(),
        &ModeConfigurationState::Stable
    );
    assert_eq!(replayed.last_ingest_seq(), 14);
    assert_eq!(replayed.used_operation_ids().len(), 1);
    assert_eq!(replayed.completed_changes().len(), 1);
    assert_eq!(
        replayed
            .bind_pending_observation(&observation("operation-1", Some(cursor("stream-1", 1)),)),
        ModeObservationBinding::ProviderOutcomeUnknown(
            ModeObservationUnknownReason::MissingEffectiveCursor
        )
    );
}
