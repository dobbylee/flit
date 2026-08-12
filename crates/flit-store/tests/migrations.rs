use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_store::{
    ConnectionPolicy, Store, StoreError, initial_migration_checksum,
    project_filesystem_identity_migration_checksum, run_git_changes_migration_checksum,
    stuck_notification_delivery_claims_migration_checksum,
};
use rusqlite::{Connection, params};

const APPLIED_AT: &str = "2026-07-23T00:00:00.000Z";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("flit-store-{label}-{}-{nonce}", process::id()));
        fs::create_dir(&directory).expect("unique test directory");
        let path = directory.join("flit.sqlite3");
        Self { directory, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).expect("remove exact test directory");
    }
}

#[test]
fn fresh_database_applies_full_initial_schema_and_reopens() {
    let database = TestDatabase::new("fresh");
    let store = Store::open(database.path(), APPLIED_AT).expect("fresh store opens");
    assert_eq!(store.schema_version().expect("schema version"), 4);
    assert_eq!(store.quick_check().expect("quick check"), "ok");
    assert_eq!(
        store.connection_policy().expect("connection policy"),
        ConnectionPolicy {
            foreign_keys: true,
            journal_mode: "wal".to_owned(),
            synchronous: 1,
            busy_timeout_ms: 5_000,
            temp_store: 2,
            wal_autocheckpoint_pages: 1_000,
        }
    );
    drop(store);

    let reopened = Store::open(database.path(), "different-time-is-not-reapplied")
        .expect("existing store reopens");
    assert_eq!(reopened.schema_version().expect("schema version"), 4);

    let connection = Connection::open(database.path()).expect("inspect database");
    let stored: (String, String, String) = connection
        .query_row(
            "SELECT name, checksum, applied_at FROM schema_migrations WHERE version = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("migration row");
    assert_eq!(stored.0, "initial");
    assert_eq!(stored.1, initial_migration_checksum());
    assert_eq!(stored.2, APPLIED_AT);
    let second: (String, String, String) = connection
        .query_row(
            "SELECT name, checksum, applied_at FROM schema_migrations WHERE version = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("second migration row");
    assert_eq!(second.0, "project_filesystem_identity");
    assert_eq!(second.1, project_filesystem_identity_migration_checksum());
    assert_eq!(second.2, APPLIED_AT);
    let third: (String, String, String) = connection
        .query_row(
            "SELECT name, checksum, applied_at FROM schema_migrations WHERE version = 3",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("third migration row");
    assert_eq!(third.0, "run_git_changes");
    assert_eq!(third.1, run_git_changes_migration_checksum());
    assert_eq!(third.2, APPLIED_AT);
    let fourth: (String, String, String) = connection
        .query_row(
            "SELECT name, checksum, applied_at FROM schema_migrations WHERE version = 4",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("fourth migration row");
    assert_eq!(fourth.0, "stuck_notification_delivery_claims");
    assert_eq!(
        fourth.1,
        stuck_notification_delivery_claims_migration_checksum()
    );
    assert_eq!(fourth.2, APPLIED_AT);

    let names = schema_names(&connection);
    for required in [
        "agent_sessions",
        "app_settings",
        "artifacts",
        "attention_items",
        "event_evidence",
        "events",
        "evidence",
        "permission_rules",
        "projects",
        "run_snapshots",
        "run_git_change_sets",
        "run_git_file_changes",
        "stuck_notification_delivery_claims",
        "runs",
        "schema_migrations",
        "one_live_session_per_run",
        "one_open_attention_per_key",
        "events_by_run_seq",
        "events_by_type_time",
        "snapshots_by_bucket_progress",
        "projects_by_filesystem_id",
        "run_git_file_changes_by_path",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
}

#[test]
fn system_time_open_records_a_utc_migration_timestamp() {
    let database = TestDatabase::new("system-time");
    Store::open_with_system_time(database.path()).expect("fresh store opens with system time");

    let connection = Connection::open(database.path()).expect("inspect database");
    let applied_at: String = connection
        .query_row(
            "SELECT applied_at FROM schema_migrations WHERE version = 1",
            [],
            |row| row.get(0),
        )
        .expect("migration timestamp");
    assert!(
        applied_at.contains('T') && applied_at.ends_with('Z'),
        "migration timestamp must be UTC RFC 3339-like text: {applied_at}"
    );
}

#[test]
fn store_exposes_a_bounded_utc_timestamp_for_core_owned_events() {
    let database = TestDatabase::new("current-utc-timestamp");
    let store = Store::open_with_system_time(database.path()).expect("open Store");
    let timestamp = store.current_utc_timestamp().expect("current timestamp");

    assert_eq!(timestamp.len(), 24);
    assert!(timestamp.ends_with('Z'));
    assert_eq!(timestamp.as_bytes()[4], b'-');
    assert_eq!(timestamp.as_bytes()[7], b'-');
    assert_eq!(timestamp.as_bytes()[10], b'T');
    assert_eq!(timestamp.as_bytes()[13], b':');
    assert_eq!(timestamp.as_bytes()[16], b':');
    assert_eq!(timestamp.as_bytes()[19], b'.');
}

#[test]
fn invalid_applied_at_is_rejected_before_a_database_file_is_created() {
    let database = TestDatabase::new("invalid-time");
    assert!(matches!(
        Store::open(database.path(), "  "),
        Err(StoreError::InvalidMigrationAppliedAt)
    ));
    assert!(!database.path().exists());
}

#[test]
fn unmanaged_nonempty_database_is_rejected_without_bootstrap() {
    let database = TestDatabase::new("unmanaged");
    let connection = Connection::open(database.path()).expect("unmanaged database");
    connection
        .execute(
            "CREATE TABLE foreign_table(id INTEGER PRIMARY KEY) STRICT",
            [],
        )
        .expect("foreign table");
    drop(connection);

    assert!(matches!(
        Store::open(database.path(), APPLIED_AT),
        Err(StoreError::UnmanagedDatabase { objects }) if objects == ["foreign_table"]
    ));
    let connection = Connection::open(database.path()).expect("inspect unmanaged database");
    assert_eq!(schema_names(&connection), vec!["foreign_table"]);
}

#[test]
fn migration_registry_mismatches_fail_closed() {
    for (label, mutation, assertion) in [
        (
            "name",
            "UPDATE schema_migrations SET name = 'changed' WHERE version = 1",
            ErrorKind::Name,
        ),
        (
            "checksum",
            "UPDATE schema_migrations SET checksum = 'changed' WHERE version = 1",
            ErrorKind::Checksum,
        ),
        (
            "missing",
            "DELETE FROM schema_migrations WHERE version = 1",
            ErrorKind::Missing,
        ),
    ] {
        let database = TestDatabase::new(label);
        Store::open(database.path(), APPLIED_AT).expect("bootstrap store");
        let connection = Connection::open(database.path()).expect("mutate fixture");
        let before = schema_names(&connection);
        connection.execute(mutation, []).expect("fixture mutation");
        drop(connection);

        let error = match Store::open(database.path(), APPLIED_AT) {
            Ok(_) => panic!("mismatch should be rejected"),
            Err(error) => error,
        };
        assertion.assert_matches(error);
        let connection = Connection::open(database.path()).expect("inspect rejected database");
        assert_eq!(schema_names(&connection), before);
    }
}

#[test]
fn version_one_schema_drift_is_rejected_before_pending_migrations_change_the_database() {
    let database = TestDatabase::new("version-one-drift");
    let connection = Connection::open(database.path()).expect("version one database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("version one schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(1, 'initial', ?1, ?2)",
            params![initial_migration_checksum(), APPLIED_AT],
        )
        .expect("version one registry");
    connection
        .execute(
            "CREATE TABLE drifted_project_data(id TEXT PRIMARY KEY) STRICT",
            [],
        )
        .expect("schema drift");
    let before = schema_names(&connection);
    drop(connection);

    assert!(matches!(
        Store::open(database.path(), APPLIED_AT),
        Err(StoreError::SchemaDrift { .. })
    ));
    let connection = Connection::open(database.path()).expect("inspect rejected database");
    assert_eq!(schema_names(&connection), before);
    assert_eq!(migration_versions(&connection), [1]);
}

#[test]
fn version_three_database_with_a_run_upgrades_and_preserves_delivery_claims() {
    let database = TestDatabase::new("version-three-run-upgrade");
    let connection = Connection::open(database.path()).expect("version three database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("version one schema");
    connection
        .execute_batch(include_str!(
            "../migrations/0002_project_filesystem_identity.sql"
        ))
        .expect("version two schema");
    connection
        .execute_batch(include_str!("../migrations/0003_run_git_changes.sql"))
        .expect("version three schema");
    for (version, name, checksum) in [
        (1_i64, "initial", initial_migration_checksum()),
        (
            2_i64,
            "project_filesystem_identity",
            project_filesystem_identity_migration_checksum(),
        ),
        (
            3_i64,
            "run_git_changes",
            run_git_changes_migration_checksum(),
        ),
    ] {
        connection
            .execute(
                "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(?1, ?2, ?3, ?4)",
                params![version, name, checksum, APPLIED_AT],
            )
            .expect("version three migration registry");
    }
    connection
        .execute(
            "INSERT INTO projects(
                id, display_name, canonical_path, filesystem_id, trusted,
                notification_policy_json, created_at, updated_at
             ) VALUES('project-upgrade', 'Upgrade', '/private/tmp/flit-upgrade', 'unix:1:2', 1, '{}', ?1, ?1)",
            [APPLIED_AT],
        )
        .expect("version three Project");
    connection
        .execute(
            "INSERT INTO runs(
                id, project_id, title, provider_kind, start_request_json, created_at
             ) VALUES('run-upgrade', 'project-upgrade', 'Upgrade Run', 'codex', '{}', ?1)",
            [APPLIED_AT],
        )
        .expect("version three Run");
    drop(connection);

    let store = Store::open(database.path(), APPLIED_AT).expect("upgrade version three Store");
    assert_eq!(store.schema_version().expect("upgraded schema version"), 4);
    drop(store);

    let connection = Connection::open(database.path()).expect("seed upgraded claim");
    connection
        .execute(
            "INSERT INTO stuck_notification_delivery_claims(
                run_id, run_version, occurrence_id, platform_id, claimed_at
             ) VALUES('run-upgrade', 7, 'occurrence-upgrade', 'occurrence-upgrade', ?1)",
            [APPLIED_AT],
        )
        .expect("outstanding delivery claim");
    drop(connection);

    let reopened = Store::open(database.path(), "2026-07-24T00:00:00.000Z")
        .expect("schema four Store reopens with outstanding claim");
    assert_eq!(
        reopened.schema_version().expect("reopened schema version"),
        4
    );
    drop(reopened);
    let connection = Connection::open(database.path()).expect("inspect preserved claim");
    let retained: (i64, String, String) = connection
        .query_row(
            "SELECT run_version, occurrence_id, platform_id
             FROM stuck_notification_delivery_claims
             WHERE run_id = 'run-upgrade'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("preserved outstanding claim");
    assert_eq!(
        retained,
        (
            7,
            "occurrence-upgrade".to_owned(),
            "occurrence-upgrade".to_owned(),
        )
    );
}

#[test]
fn malformed_legacy_filesystem_identity_blocks_the_identity_index_migration() {
    let database = TestDatabase::new("invalid-filesystem-identity");
    let connection = Connection::open(database.path()).expect("version one database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("version one schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(1, 'initial', ?1, ?2)",
            params![initial_migration_checksum(), APPLIED_AT],
        )
        .expect("version one registry");
    connection
        .execute(
            "INSERT INTO projects(id, display_name, canonical_path, filesystem_id, trusted, notification_policy_json, created_at, updated_at) VALUES('invalid-project', 'Invalid', '/private/tmp/flit-invalid-project', '', 0, '{}', ?1, ?1)",
            [APPLIED_AT],
        )
        .expect("invalid legacy Project");
    drop(connection);

    assert!(matches!(
        Store::open(database.path(), APPLIED_AT),
        Err(StoreError::InvalidStoredProjectFilesystemIdentity { project_id })
            if project_id == "invalid-project"
    ));
    let connection = Connection::open(database.path()).expect("inspect rejected database");
    assert_eq!(migration_versions(&connection), [1]);
    assert!(
        !schema_names(&connection)
            .iter()
            .any(|name| name == "projects_by_filesystem_id")
    );
}

#[test]
fn noncanonical_legacy_filesystem_identity_blocks_duplicate_protection_bypass() {
    let database = TestDatabase::new("noncanonical-filesystem-identity");
    let connection = Connection::open(database.path()).expect("version one database");
    connection
        .execute_batch(include_str!("../migrations/0001_initial.sql"))
        .expect("version one schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(1, 'initial', ?1, ?2)",
            params![initial_migration_checksum(), APPLIED_AT],
        )
        .expect("version one registry");
    for (id, path, filesystem_id) in [
        ("canonical", "/private/tmp/flit-canonical", "unix:1:2"),
        (
            "noncanonical",
            "/private/tmp/flit-noncanonical",
            "unix:01:02",
        ),
    ] {
        connection
            .execute(
                "INSERT INTO projects(id, display_name, canonical_path, filesystem_id, trusted, notification_policy_json, created_at, updated_at) VALUES(?1, ?1, ?2, ?3, 0, '{}', ?4, ?4)",
                params![id, path, filesystem_id, APPLIED_AT],
            )
            .expect("legacy Project");
    }
    drop(connection);

    assert!(matches!(
        Store::open(database.path(), APPLIED_AT),
        Err(StoreError::InvalidStoredProjectFilesystemIdentity { project_id })
            if project_id == "noncanonical"
    ));
    let connection = Connection::open(database.path()).expect("inspect rejected database");
    assert_eq!(migration_versions(&connection), [1]);
    assert!(
        !schema_names(&connection)
            .iter()
            .any(|name| name == "projects_by_filesystem_id")
    );
}

#[test]
fn unknown_newer_migration_and_schema_drift_are_rejected() {
    let newer = TestDatabase::new("newer");
    Store::open(newer.path(), APPLIED_AT).expect("bootstrap store");
    let connection = Connection::open(newer.path()).expect("newer fixture");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(?1, ?2, ?3, ?4)",
            params![5_i64, "future", "future", APPLIED_AT],
        )
        .expect("future migration row");
    drop(connection);
    assert!(matches!(
        Store::open(newer.path(), APPLIED_AT),
        Err(StoreError::UnsupportedMigration { version: 5 })
    ));

    let drift = TestDatabase::new("drift");
    Store::open(drift.path(), APPLIED_AT).expect("bootstrap store");
    let connection = Connection::open(drift.path()).expect("drift fixture");
    connection
        .execute("DROP INDEX events_by_type_time", [])
        .expect("remove index");
    drop(connection);
    assert!(matches!(
        Store::open(drift.path(), APPLIED_AT),
        Err(StoreError::SchemaDrift { .. })
    ));
}

#[test]
fn rejected_database_keeps_delete_journal_data_and_sidecar_state() {
    let database = TestDatabase::new("rejected-preservation");
    Store::open(database.path(), APPLIED_AT).expect("bootstrap store");
    let connection = Connection::open(database.path()).expect("rejected fixture");
    connection
        .pragma_update(None, "journal_mode", "DELETE")
        .expect("switch fixture to delete journal");
    connection
        .execute(
            "UPDATE schema_migrations SET checksum = 'rejected-checksum' WHERE version = 1",
            [],
        )
        .expect("tamper checksum");
    drop(connection);
    let wal = sidecar(database.path(), "-wal");
    let shared_memory = sidecar(database.path(), "-shm");
    assert!(!wal.exists());
    assert!(!shared_memory.exists());

    assert!(matches!(
        Store::open(database.path(), APPLIED_AT),
        Err(StoreError::MigrationChecksumMismatch { .. })
    ));
    assert!(!wal.exists());
    assert!(!shared_memory.exists());

    let connection = Connection::open(database.path()).expect("inspect rejected fixture");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode");
    let checksum: String = connection
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version = 1",
            [],
            |row| row.get(0),
        )
        .expect("preserved checksum");
    assert_eq!(journal_mode, "delete");
    assert_eq!(checksum, "rejected-checksum");
}

#[test]
fn store_rejects_a_connection_that_cannot_enter_wal_mode() {
    assert!(matches!(
        Store::open(Path::new(":memory:"), APPLIED_AT),
        Err(StoreError::ConnectionPolicyMismatch { actual, .. })
            if actual.journal_mode == "memory"
    ));
}

#[derive(Clone, Copy)]
enum ErrorKind {
    Name,
    Checksum,
    Missing,
}

impl ErrorKind {
    fn assert_matches(self, error: StoreError) {
        match self {
            Self::Name => assert!(matches!(error, StoreError::MigrationNameMismatch { .. })),
            Self::Checksum => {
                assert!(matches!(
                    error,
                    StoreError::MigrationChecksumMismatch { .. }
                ))
            }
            Self::Missing => assert!(matches!(error, StoreError::MissingMigration { version: 1 })),
        }
    }
}

fn schema_names(connection: &Connection) -> Vec<String> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .expect("schema statement");
    statement
        .query_map([], |row| row.get(0))
        .expect("schema rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("schema names")
}

fn migration_versions(connection: &Connection) -> Vec<i64> {
    connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("migration version statement")
        .query_map([], |row| row.get(0))
        .expect("migration version rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("migration versions")
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}
