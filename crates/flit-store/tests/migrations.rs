use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use flit_store::{ConnectionPolicy, Store, StoreError, initial_migration_checksum};
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
    assert_eq!(store.schema_version().expect("schema version"), 1);
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
    assert_eq!(reopened.schema_version().expect("schema version"), 1);

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
        "runs",
        "schema_migrations",
        "one_live_session_per_run",
        "one_open_attention_per_key",
        "events_by_run_seq",
        "events_by_type_time",
        "snapshots_by_bucket_progress",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
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
fn unknown_newer_migration_and_schema_drift_are_rejected() {
    let newer = TestDatabase::new("newer");
    Store::open(newer.path(), APPLIED_AT).expect("bootstrap store");
    let connection = Connection::open(newer.path()).expect("newer fixture");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(?1, ?2, ?3, ?4)",
            params![2_i64, "future", "future", APPLIED_AT],
        )
        .expect("future migration row");
    drop(connection);
    assert!(matches!(
        Store::open(newer.path(), APPLIED_AT),
        Err(StoreError::UnsupportedMigration { version: 2 })
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

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}
