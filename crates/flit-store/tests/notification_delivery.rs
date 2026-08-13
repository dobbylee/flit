use std::{
    fs,
    path::PathBuf,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_store::{
    NotificationDeliveryClaim, NotificationDeliveryClaimOutcome, NotificationDeliveryFailure,
    NotificationDeliveryFailureOutcome, NotificationDeliveryReceipt,
    NotificationDeliveryReceiptOutcome, NotificationKind, NotificationKinds, QuietHours, Store,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const APPLIED_AT: &str = "2026-08-13T00:00:00.000Z";
const PROJECT_ID: &str = "project-notifications";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "flit-notification-delivery-{label}-{}-{nonce}",
            process::id()
        ));
        fs::create_dir(&directory).expect("unique test directory");
        let path = directory.join("flit.sqlite3");
        let store = Store::open(&path, APPLIED_AT).expect("bootstrap Store");
        drop(store);
        let connection = Connection::open(&path).expect("seed connection");
        connection
            .execute(
                "INSERT INTO projects(
                    id, display_name, canonical_path, trusted,
                    notification_policy_json, created_at, updated_at
                 ) VALUES(?1, 'Notifications', ?2, 1, '{}', ?3, ?3)",
                params![
                    PROJECT_ID,
                    directory.join("project").to_string_lossy(),
                    APPLIED_AT
                ],
            )
            .expect("seed Project");
        drop(connection);
        Self { directory, path }
    }

    fn open(&self) -> Store {
        Store::open(&self.path, APPLIED_AT).expect("open Store")
    }

    fn seed_attention(&self, run_id: &str, category: &str, severity: &str, blocking: bool) {
        let connection = Connection::open(&self.path).expect("seed connection");
        connection
            .execute(
                "INSERT INTO runs(
                    id, project_id, title, provider_kind, start_request_json, created_at
                 ) VALUES(?1, ?2, ?1, 'codex', '{}', ?3)",
                params![run_id, PROJECT_ID, APPLIED_AT],
            )
            .expect("seed Run");
        connection
            .execute(
                "INSERT INTO events(
                    event_id, protocol_version, event_type, run_id, occurred_at, observed_at,
                    source_json, confidence, payload_json, extensions_json
                 ) VALUES(?1, '1.25', ?2, ?3, ?4, ?4, '{}', 1.0, '{}', '{}')",
                params![
                    format!("event-{run_id}"),
                    format!("run.{category}"),
                    run_id,
                    APPLIED_AT
                ],
            )
            .expect("seed event");
        let version = connection.last_insert_rowid();
        let attention_id = format!("attention-{run_id}");
        let action = if category == "stuck" {
            json!({ "kind": "still_working", "occurrence_id": attention_id })
        } else {
            json!({ "kind": "unavailable", "reason": "notification_has_no_response_action" })
        };
        let stuck = if category == "stuck" {
            json!({
                "occurrence_id": attention_id,
                "notification": {
                    "status": "due",
                    "occurrence_id": attention_id,
                    "due_at_monotonic_ms": 1000
                },
                "reset": null
            })
        } else {
            Value::Null
        };
        let dashboard_bucket = if category == "stuck" {
            "PossiblyStuck"
        } else {
            "NeedsAttention"
        };
        let snapshot = json!({
            "run_id": run_id,
            "version": version,
            "lifecycle": "Running",
            "activity": { "kind": "Unknown", "confidence": 0.0 },
            "attention": {
                "level": severity,
                "open_count": 1,
                "primary": {
                    "attention_id": attention_id,
                    "attention_version": version,
                    "category": category,
                    "severity": severity,
                    "blocking": blocking,
                    "status": "open",
                    "source_event_id": format!("event-{run_id}"),
                    "source_event_type": format!("run.{category}"),
                    "source_observed_at": APPLIED_AT,
                    "content_unavailable_reason": "raw_content_not_retained",
                    "action": action
                }
            },
            "dashboard_bucket": dashboard_bucket,
            "last_progress_at": APPLIED_AT,
            "last_liveness_at": APPLIED_AT,
            "changes": {
                "availability": "available",
                "attribution": "exact",
                "files": 0,
                "insertions": 0,
                "deletions": 0
            }
        });
        let mut snapshot = snapshot.as_object().expect("snapshot object").clone();
        if !stuck.is_null() {
            snapshot.insert("stuck".to_owned(), stuck);
        }
        connection
            .execute(
                "INSERT INTO run_snapshots(
                    run_id, version, lifecycle, activity, activity_confidence, attention_level,
                    dashboard_bucket, last_progress_at, last_liveness_at, snapshot_json, updated_at
                 ) VALUES(?1, ?2, 'Running', 'Unknown', 0.0, ?3, ?4, ?5, ?5, ?6, ?5)",
                params![
                    run_id,
                    version,
                    severity,
                    dashboard_bucket,
                    APPLIED_AT,
                    serde_json::to_string(&snapshot).expect("render snapshot")
                ],
            )
            .expect("seed snapshot");
    }

    fn set_attention_status(&self, run_id: &str, status: &str) {
        let connection = Connection::open(&self.path).expect("update connection");
        let rendered: String = connection
            .query_row(
                "SELECT snapshot_json FROM run_snapshots WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .expect("load snapshot");
        let mut snapshot: Value = serde_json::from_str(&rendered).expect("parse snapshot");
        snapshot["attention"]["primary"]["status"] = json!(status);
        connection
            .execute(
                "UPDATE run_snapshots SET snapshot_json = ?1 WHERE run_id = ?2",
                params![
                    serde_json::to_string(&snapshot).expect("render snapshot"),
                    run_id
                ],
            )
            .expect("update attention status");
    }

    fn advance_current_attention(&self, run_id: &str) -> i64 {
        let connection = Connection::open(&self.path).expect("advance connection");
        let event_id = format!("event-{run_id}-advance");
        connection
            .execute(
                "INSERT INTO events(
                    event_id, protocol_version, event_type, run_id, occurred_at, observed_at,
                    source_json, confidence, payload_json, extensions_json
                 ) VALUES(?1, '1.25', 'run.event_observed', ?2, ?3, ?3, '{}', 1.0, '{}', '{}')",
                params![event_id, run_id, APPLIED_AT],
            )
            .expect("append advancing event");
        let version = connection.last_insert_rowid();
        let rendered: String = connection
            .query_row(
                "SELECT snapshot_json FROM run_snapshots WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .expect("load snapshot");
        let mut snapshot: Value = serde_json::from_str(&rendered).expect("parse snapshot");
        snapshot["version"] = json!(version);
        snapshot["attention"]["primary"]["attention_version"] = json!(version);
        snapshot["attention"]["primary"]["source_event_id"] = json!(event_id);
        connection
            .execute(
                "UPDATE run_snapshots SET version = ?1, snapshot_json = ?2 WHERE run_id = ?3",
                params![
                    version,
                    serde_json::to_string(&snapshot).expect("render snapshot"),
                    run_id
                ],
            )
            .expect("advance snapshot");
        version
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "failed to remove test directory {}: {error}",
                self.directory.display()
            );
        }
    }
}

fn claim(candidate: &flit_store::NotificationDeliveryCandidate) -> NotificationDeliveryClaim {
    NotificationDeliveryClaim {
        notification_id: candidate.notification_id.clone(),
        run_id: candidate.run_id.clone(),
        expected_run_version: candidate.run_version,
        kind: candidate.kind,
        item_id: candidate.item_id.clone(),
        item_version: candidate.item_version,
        platform_id: candidate.platform_id.clone(),
        local_minute: 8 * 60,
        claimed_at: "2026-08-13T08:00:00.000Z".to_owned(),
    }
}

#[test]
fn quiet_hours_catch_up_is_one_current_highest_priority_item_per_project() {
    let database = TestDatabase::new("quiet-catch-up");
    database.seed_attention("run-permission", "permission", "ActionRequired", true);
    database.seed_attention("run-failure", "failure", "Critical", false);
    let mut store = database.open();
    store
        .update_global_notification_policy(
            0,
            NotificationKinds::default(),
            QuietHours {
                enabled: true,
                start_minute: 22 * 60,
                end_minute: 8 * 60,
            },
            "2026-08-13T00:01:00.000Z",
        )
        .expect("enable quiet hours");

    assert!(
        store
            .reconcile_notification_deliveries(23 * 60, "2026-08-13T23:00:00.000Z")
            .expect("quiet reconciliation")
            .is_empty()
    );
    let catch_up = store
        .reconcile_notification_deliveries(8 * 60, "2026-08-14T08:00:00.000Z")
        .expect("catch-up reconciliation");
    assert_eq!(catch_up.len(), 1);
    assert_eq!(catch_up[0].kind, NotificationKind::Failure);
    assert!(catch_up[0].catch_up);

    let failure_claim = claim(&catch_up[0]);
    assert_eq!(
        store
            .claim_notification_delivery(failure_claim.clone())
            .expect("claim catch-up"),
        NotificationDeliveryClaimOutcome::Claimed
    );
    assert_eq!(
        store
            .claim_notification_delivery(failure_claim.clone())
            .expect("duplicate claim"),
        NotificationDeliveryClaimOutcome::AlreadyClaimed
    );
    let while_claimed = store
        .reconcile_notification_deliveries(8 * 60 + 1, "2026-08-14T08:00:30.000Z")
        .expect("claimed catch-up reconciliation");
    assert_eq!(while_claimed.len(), 1);
    assert_eq!(
        while_claimed[0].notification_id,
        failure_claim.notification_id
    );
    assert!(while_claimed[0].delivery_claimed);
    assert!(!while_claimed[0].catch_up);
    assert_eq!(
        store
            .release_notification_delivery(NotificationDeliveryFailure {
                notification_id: failure_claim.notification_id.clone(),
                run_id: failure_claim.run_id.clone(),
                kind: failure_claim.kind,
                item_id: failure_claim.item_id.clone(),
                item_version: failure_claim.item_version,
                platform_id: failure_claim.platform_id.clone(),
            })
            .expect("release catch-up"),
        NotificationDeliveryFailureOutcome::Released
    );
    let retry = store
        .reconcile_notification_deliveries(8 * 60, "2026-08-14T08:01:00.000Z")
        .expect("retry catch-up");
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].notification_id, failure_claim.notification_id);
    assert!(retry[0].catch_up);

    let retry_claim = claim(&retry[0]);
    store
        .claim_notification_delivery(retry_claim.clone())
        .expect("claim retry");
    assert_eq!(
        store
            .record_notification_delivery(NotificationDeliveryReceipt {
                notification_id: retry_claim.notification_id.clone(),
                run_id: retry_claim.run_id.clone(),
                kind: retry_claim.kind,
                item_id: retry_claim.item_id.clone(),
                item_version: retry_claim.item_version,
                platform_id: retry_claim.platform_id.clone(),
                delivered_at: "2026-08-14T08:01:01.000Z".to_owned(),
            })
            .expect("record delivery"),
        NotificationDeliveryReceiptOutcome::Delivered
    );
    let next = store
        .reconcile_notification_deliveries(8 * 60 + 2, "2026-08-14T08:02:00.000Z")
        .expect("next catch-up");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].kind, NotificationKind::Permission);
    assert!(next[0].catch_up);
}

#[test]
fn completion_never_catches_up_and_policy_suppression_is_not_replayed() {
    let database = TestDatabase::new("completion-suppression");
    database.seed_attention("run-completion", "completion", "Informational", false);
    let mut store = database.open();

    assert!(
        store
            .reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z")
            .expect("default policy reconciliation")
            .is_empty()
    );
    let kinds = NotificationKinds {
        completion: true,
        ..NotificationKinds::default()
    };
    store
        .update_global_notification_policy(
            0,
            kinds,
            QuietHours::default(),
            "2026-08-13T12:01:00.000Z",
        )
        .expect("enable completion");
    assert!(
        store
            .reconcile_notification_deliveries(12 * 60 + 2, "2026-08-13T12:02:00.000Z")
            .expect("enabled policy reconciliation")
            .is_empty()
    );

    let second = TestDatabase::new("completion-quiet");
    second.seed_attention("run-completion-quiet", "completion", "Informational", false);
    let mut store = second.open();
    store
        .update_global_notification_policy(
            0,
            kinds,
            QuietHours {
                enabled: true,
                start_minute: 22 * 60,
                end_minute: 8 * 60,
            },
            "2026-08-13T21:59:00.000Z",
        )
        .expect("enable completion and quiet hours");
    assert!(
        store
            .reconcile_notification_deliveries(23 * 60, "2026-08-13T23:00:00.000Z")
            .expect("quiet completion")
            .is_empty()
    );
    assert!(
        store
            .reconcile_notification_deliveries(8 * 60, "2026-08-14T08:00:00.000Z")
            .expect("completion catch-up boundary")
            .is_empty()
    );
}

#[test]
fn stuck_requires_the_exact_due_occurrence_and_uses_a_stable_platform_id() {
    let database = TestDatabase::new("stuck-due");
    database.seed_attention("run-stuck", "stuck", "ActionRequired", false);
    let mut store = database.open();
    let first = store
        .reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z")
        .expect("first reconciliation");
    let second = store
        .reconcile_notification_deliveries(12 * 60 + 1, "2026-08-13T12:01:00.000Z")
        .expect("second reconciliation");
    assert_eq!(first.len(), 1);
    assert_eq!(first, second);
    assert_eq!(first[0].kind, NotificationKind::Stuck);
    assert_eq!(first[0].item_id, "attention-run-stuck");
    assert!(first[0].platform_id.starts_with("flit-"));
}

#[test]
fn non_open_questions_are_not_eligible_for_delivery() {
    let database = TestDatabase::new("question-status");
    database.seed_attention("run-question", "question", "ActionRequired", true);
    let mut store = database.open();
    assert_eq!(
        store
            .reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z")
            .expect("open question")
            .len(),
        1
    );
    database.set_attention_status("run-question", "response_pending");
    assert!(
        store
            .reconcile_notification_deliveries(12 * 60 + 1, "2026-08-13T12:01:00.000Z")
            .expect("pending question")
            .is_empty()
    );
    database.set_attention_status("run-question", "delivery_unknown");
    assert!(
        store
            .reconcile_notification_deliveries(12 * 60 + 2, "2026-08-13T12:02:00.000Z")
            .expect("delivery-unknown question")
            .is_empty()
    );
}

#[test]
fn claimed_identity_survives_current_attention_version_advance_and_reopen() {
    let database = TestDatabase::new("identity-advance");
    database.seed_attention("run-failure", "failure", "Critical", false);
    let mut store = database.open();
    let initial = store
        .reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z")
        .expect("initial candidate")
        .pop()
        .expect("failure candidate");
    let initial_claim = claim(&initial);
    store
        .claim_notification_delivery(initial_claim.clone())
        .expect("claim initial candidate");
    let advanced_version = database.advance_current_attention("run-failure");
    drop(store);

    let mut reopened = database.open();
    let restored = reopened
        .reconcile_notification_deliveries(12 * 60 + 1, "2026-08-13T12:01:00.000Z")
        .expect("restored claimed candidate");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].notification_id, initial.notification_id);
    assert_eq!(restored[0].platform_id, initial.platform_id);
    assert_eq!(restored[0].item_version, initial.item_version);
    assert_eq!(restored[0].run_version, advanced_version as u64);
    assert!(restored[0].delivery_claimed);
    assert_eq!(
        reopened
            .record_notification_delivery(NotificationDeliveryReceipt {
                notification_id: initial_claim.notification_id.clone(),
                run_id: initial_claim.run_id.clone(),
                kind: initial_claim.kind,
                item_id: initial_claim.item_id.clone(),
                item_version: initial_claim.item_version,
                platform_id: initial_claim.platform_id.clone(),
                delivered_at: "2026-08-13T12:01:01.000Z".to_owned(),
            })
            .expect("record restored delivery"),
        NotificationDeliveryReceiptOutcome::Delivered
    );
    assert!(
        reopened
            .reconcile_notification_deliveries(12 * 60 + 2, "2026-08-13T12:02:00.000Z")
            .expect("delivered reconciliation")
            .is_empty()
    );
}

#[test]
fn matching_legacy_stuck_claim_is_adopted_without_a_second_platform_identity() {
    let database = TestDatabase::new("legacy-stuck-claim");
    database.seed_attention("run-stuck-legacy", "stuck", "ActionRequired", false);
    let connection = Connection::open(&database.path).expect("legacy claim connection");
    let run_version: i64 = connection
        .query_row(
            "SELECT version FROM run_snapshots WHERE run_id = 'run-stuck-legacy'",
            [],
            |row| row.get(0),
        )
        .expect("Run version");
    connection
        .execute(
            "INSERT INTO stuck_notification_delivery_claims(
                run_id, run_version, occurrence_id, platform_id, claimed_at
             ) VALUES('run-stuck-legacy', ?1, 'attention-run-stuck-legacy',
                      'attention-run-stuck-legacy', ?2)",
            params![run_version, APPLIED_AT],
        )
        .expect("legacy stuck claim");
    drop(connection);

    let mut store = database.open();
    let adopted = store
        .reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z")
        .expect("adopt legacy claim");
    assert_eq!(adopted.len(), 1);
    assert!(adopted[0].delivery_claimed);
    assert_eq!(
        adopted[0].platform_id,
        "attention-run-stuck-legacy".to_owned()
    );
    assert_eq!(
        store
            .claim_notification_delivery(claim(&adopted[0]))
            .expect("duplicate adopted claim"),
        NotificationDeliveryClaimOutcome::AlreadyClaimed
    );
}

#[test]
fn future_stored_item_version_fails_closed() {
    let database = TestDatabase::new("future-item-version");
    database.seed_attention("run-failure-future", "failure", "Critical", false);
    let mut store = database.open();
    let candidate = store
        .reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z")
        .expect("candidate")
        .pop()
        .expect("failure candidate");
    let connection = Connection::open(&database.path).expect("ledger connection");
    connection
        .execute(
            "INSERT INTO notification_deliveries(
                notification_id, run_id, project_id, kind, item_id, item_version,
                platform_id, state, claimed_at
             ) VALUES(?1, ?2, ?3, 'failure', ?4, ?5, ?6, 'claimed', ?7)",
            params![
                candidate.notification_id,
                candidate.run_id,
                candidate.project_id,
                candidate.item_id,
                candidate.item_version as i64 + 1,
                candidate.platform_id,
                APPLIED_AT,
            ],
        )
        .expect("future ledger row");
    drop(connection);

    assert!(matches!(
        store.reconcile_notification_deliveries(12 * 60 + 1, "2026-08-13T12:01:00.000Z"),
        Err(flit_store::StoreError::StoredNotificationDeliveryInvalid {
            field: "item_version",
            ..
        })
    ));
}

#[test]
fn mismatched_legacy_stuck_platform_identity_fails_closed() {
    let database = TestDatabase::new("legacy-stuck-mismatch");
    database.seed_attention("run-stuck-mismatch", "stuck", "ActionRequired", false);
    let connection = Connection::open(&database.path).expect("legacy claim connection");
    let run_version: i64 = connection
        .query_row(
            "SELECT version FROM run_snapshots WHERE run_id = 'run-stuck-mismatch'",
            [],
            |row| row.get(0),
        )
        .expect("Run version");
    connection
        .execute(
            "INSERT INTO stuck_notification_delivery_claims(
                run_id, run_version, occurrence_id, platform_id, claimed_at
             ) VALUES('run-stuck-mismatch', ?1, 'attention-run-stuck-mismatch',
                      'different-platform-id', ?2)",
            params![run_version, APPLIED_AT],
        )
        .expect("malformed legacy stuck claim");
    drop(connection);

    let mut store = database.open();
    assert!(matches!(
        store.reconcile_notification_deliveries(12 * 60, "2026-08-13T12:00:00.000Z"),
        Err(flit_store::StoreError::StoredNotificationDeliveryInvalid {
            field: "legacy.platform_id",
            ..
        })
    ));
    let connection = Connection::open(&database.path).expect("inspect generic ledger");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM notification_deliveries", [], |row| {
            row.get(0)
        })
        .expect("generic ledger count");
    assert_eq!(count, 0);
}
