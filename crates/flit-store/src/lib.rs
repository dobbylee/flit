use std::{error::Error, fmt, path::Path, time::Duration};

use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};

const INITIAL_MIGRATION_VERSION: i64 = 1;
const INITIAL_MIGRATION_NAME: &str = "initial";
const INITIAL_MIGRATION_SQL: &str = include_str!("../migrations/0001_initial.sql");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionPolicy {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub synchronous: i64,
    pub busy_timeout_ms: i64,
    pub temp_store: i64,
    pub wal_autocheckpoint_pages: i64,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>, migration_applied_at: &str) -> Result<Self, StoreError> {
        if migration_applied_at.trim().is_empty() {
            return Err(StoreError::InvalidMigrationAppliedAt);
        }

        let mut connection = Connection::open(path).map_err(StoreError::Sqlite)?;
        let needs_bootstrap = preflight_database(&connection)?;
        configure_connection(&connection)?;
        validate_connection_policy(&connection)?;
        if needs_bootstrap {
            apply_initial_migration(&mut connection, migration_applied_at)?;
        }
        validate_schema(&connection)?;
        validate_integrity(&connection)?;
        Ok(Self { connection })
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .map_err(StoreError::Sqlite)
    }

    pub fn connection_policy(&self) -> Result<ConnectionPolicy, StoreError> {
        Ok(ConnectionPolicy {
            foreign_keys: pragma_i64(&self.connection, "foreign_keys")? == 1,
            journal_mode: pragma_string(&self.connection, "journal_mode")?,
            synchronous: pragma_i64(&self.connection, "synchronous")?,
            busy_timeout_ms: pragma_i64(&self.connection, "busy_timeout")?,
            temp_store: pragma_i64(&self.connection, "temp_store")?,
            wal_autocheckpoint_pages: pragma_i64(&self.connection, "wal_autocheckpoint")?,
        })
    }

    pub fn quick_check(&self) -> Result<String, StoreError> {
        pragma_string(&self.connection, "quick_check")
    }
}

#[must_use]
pub fn initial_migration_checksum() -> String {
    let mut digest = Sha256::new();
    digest.update(INITIAL_MIGRATION_SQL.as_bytes());
    format!("{:x}", digest.finalize())
}

fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(StoreError::Sqlite)?;
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "temp_store", "MEMORY")
        .map_err(StoreError::Sqlite)?;
    connection
        .pragma_update(None, "wal_autocheckpoint", 1_000_i64)
        .map_err(StoreError::Sqlite)
}

fn preflight_database(connection: &Connection) -> Result<bool, StoreError> {
    let objects = schema_objects(connection)?;
    let has_registry = objects
        .iter()
        .any(|object| object.kind == "table" && object.name == "schema_migrations");

    if !has_registry {
        let unmanaged = objects
            .iter()
            .filter(|object| !object.name.starts_with("sqlite_"))
            .map(|object| object.name.clone())
            .collect::<Vec<_>>();
        if !unmanaged.is_empty() {
            return Err(StoreError::UnmanagedDatabase { objects: unmanaged });
        }
        return Ok(true);
    }

    validate_migration_registry(connection)?;
    validate_schema(connection)?;
    validate_integrity(connection)?;
    Ok(false)
}

fn apply_initial_migration(
    connection: &mut Connection,
    applied_at: &str,
) -> Result<(), StoreError> {
    apply_migration(
        connection,
        INITIAL_MIGRATION_VERSION,
        INITIAL_MIGRATION_NAME,
        &initial_migration_checksum(),
        applied_at,
        INITIAL_MIGRATION_SQL,
    )
}

fn apply_migration(
    connection: &mut Connection,
    version: i64,
    name: &str,
    checksum: &str,
    applied_at: &str,
    sql: &str,
) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StoreError::Sqlite)?;
    transaction.execute_batch(sql).map_err(StoreError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES(?1, ?2, ?3, ?4)",
            params![version, name, checksum, applied_at],
        )
        .map_err(StoreError::Sqlite)?;
    transaction.commit().map_err(StoreError::Sqlite)
}

fn validate_migration_registry(connection: &Connection) -> Result<(), StoreError> {
    let mut statement = connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .map_err(StoreError::Sqlite)?;
    let records = statement
        .query_map([], |row| {
            Ok(MigrationRecord {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })
        .map_err(StoreError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sqlite)?;

    let Some(initial) = records.first() else {
        return Err(StoreError::MissingMigration {
            version: INITIAL_MIGRATION_VERSION,
        });
    };
    if initial.version != INITIAL_MIGRATION_VERSION {
        return if initial.version > INITIAL_MIGRATION_VERSION {
            Err(StoreError::UnsupportedMigration {
                version: initial.version,
            })
        } else {
            Err(StoreError::MissingMigration {
                version: INITIAL_MIGRATION_VERSION,
            })
        };
    }
    if initial.name != INITIAL_MIGRATION_NAME {
        return Err(StoreError::MigrationNameMismatch {
            version: initial.version,
            expected: INITIAL_MIGRATION_NAME.to_owned(),
            actual: initial.name.clone(),
        });
    }
    let expected_checksum = initial_migration_checksum();
    if initial.checksum != expected_checksum {
        return Err(StoreError::MigrationChecksumMismatch {
            version: initial.version,
            expected: expected_checksum,
            actual: initial.checksum.clone(),
        });
    }
    if let Some(unsupported) = records.get(1) {
        return Err(StoreError::UnsupportedMigration {
            version: unsupported.version,
        });
    }
    Ok(())
}

fn validate_schema(connection: &Connection) -> Result<(), StoreError> {
    let expected_connection = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
    expected_connection
        .execute_batch(INITIAL_MIGRATION_SQL)
        .map_err(StoreError::Sqlite)?;
    let expected = schema_objects(&expected_connection)?;
    let actual = schema_objects(connection)?;
    if actual != expected {
        return Err(StoreError::SchemaDrift {
            expected: schema_signature(&expected),
            actual: schema_signature(&actual),
        });
    }
    Ok(())
}

fn validate_integrity(connection: &Connection) -> Result<(), StoreError> {
    let result = pragma_string(connection, "quick_check")?;
    if result != "ok" {
        return Err(StoreError::IntegrityCheckFailed(result));
    }
    Ok(())
}

fn validate_connection_policy(connection: &Connection) -> Result<(), StoreError> {
    let actual = read_connection_policy(connection)?;
    let expected = ConnectionPolicy {
        foreign_keys: true,
        journal_mode: "wal".to_owned(),
        synchronous: 1,
        busy_timeout_ms: 5_000,
        temp_store: 2,
        wal_autocheckpoint_pages: 1_000,
    };
    if actual != expected {
        return Err(StoreError::ConnectionPolicyMismatch {
            expected: Box::new(expected),
            actual: Box::new(actual),
        });
    }
    Ok(())
}

fn read_connection_policy(connection: &Connection) -> Result<ConnectionPolicy, StoreError> {
    Ok(ConnectionPolicy {
        foreign_keys: pragma_i64(connection, "foreign_keys")? == 1,
        journal_mode: pragma_string(connection, "journal_mode")?,
        synchronous: pragma_i64(connection, "synchronous")?,
        busy_timeout_ms: pragma_i64(connection, "busy_timeout")?,
        temp_store: pragma_i64(connection, "temp_store")?,
        wal_autocheckpoint_pages: pragma_i64(connection, "wal_autocheckpoint")?,
    })
}

fn pragma_i64(connection: &Connection, pragma: &str) -> Result<i64, StoreError> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .map_err(StoreError::Sqlite)
}

fn pragma_string(connection: &Connection, pragma: &str) -> Result<String, StoreError> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .map_err(StoreError::Sqlite)
}

fn schema_objects(connection: &Connection) -> Result<Vec<SchemaObject>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type, name",
        )
        .map_err(StoreError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok(SchemaObject {
                kind: row.get(0)?,
                name: row.get(1)?,
                table_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })
        .map_err(StoreError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sqlite)
}

fn schema_signature(objects: &[SchemaObject]) -> Vec<String> {
    objects
        .iter()
        .map(|object| {
            format!(
                "{}:{}:{}:{}",
                object.kind, object.name, object.table_name, object.sql
            )
        })
        .collect()
}

#[derive(Clone, Debug)]
struct MigrationRecord {
    version: i64,
    name: String,
    checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SchemaObject {
    kind: String,
    name: String,
    table_name: String,
    sql: String,
}

#[derive(Debug)]
pub enum StoreError {
    InvalidMigrationAppliedAt,
    UnmanagedDatabase {
        objects: Vec<String>,
    },
    MissingMigration {
        version: i64,
    },
    UnsupportedMigration {
        version: i64,
    },
    MigrationNameMismatch {
        version: i64,
        expected: String,
        actual: String,
    },
    MigrationChecksumMismatch {
        version: i64,
        expected: String,
        actual: String,
    },
    SchemaDrift {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    IntegrityCheckFailed(String),
    ConnectionPolicyMismatch {
        expected: Box<ConnectionPolicy>,
        actual: Box<ConnectionPolicy>,
    },
    Sqlite(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMigrationAppliedAt => {
                formatter.write_str("migration applied_at must not be empty")
            }
            Self::UnmanagedDatabase { objects } => {
                write!(formatter, "database has no migration registry: {objects:?}")
            }
            Self::MissingMigration { version } => {
                write!(formatter, "required migration {version} is missing")
            }
            Self::UnsupportedMigration { version } => {
                write!(formatter, "database migration {version} is not supported")
            }
            Self::MigrationNameMismatch {
                version,
                expected,
                actual,
            } => write!(
                formatter,
                "migration {version} name mismatch: expected {expected}, found {actual}"
            ),
            Self::MigrationChecksumMismatch {
                version,
                expected,
                actual,
            } => write!(
                formatter,
                "migration {version} checksum mismatch: expected {expected}, found {actual}"
            ),
            Self::SchemaDrift { expected, actual } => write!(
                formatter,
                "database schema drift: expected {expected:?}, found {actual:?}"
            ),
            Self::IntegrityCheckFailed(result) => {
                write!(formatter, "SQLite quick_check failed: {result}")
            }
            Self::ConnectionPolicyMismatch { expected, actual } => write!(
                formatter,
                "SQLite connection policy mismatch: expected {expected:?}, found {actual:?}"
            ),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TemporaryDirectory {
        path: PathBuf,
    }

    impl TemporaryDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("flit-store-{label}-{}-{nonce}", process::id()));
            fs::create_dir(&path).expect("unique temporary directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!(
                    "failed to remove test directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    #[test]
    fn failed_migration_rolls_back_all_ddl_and_allows_clean_bootstrap() {
        let directory = TemporaryDirectory::new("rollback");
        let path = directory.path().join("flit.sqlite3");
        let mut connection = Connection::open(&path).expect("rollback database");
        configure_connection(&connection).expect("connection policy");
        let failing_sql = "
            CREATE TABLE schema_migrations (
              version INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              checksum TEXT NOT NULL,
              applied_at TEXT NOT NULL
            ) STRICT;
            CREATE TABLE partial_table(id INTEGER PRIMARY KEY) STRICT;
            INSERT INTO table_that_does_not_exist(id) VALUES(1);
        ";
        assert!(matches!(
            apply_migration(&mut connection, 1, "failing", "failing", "now", failing_sql),
            Err(StoreError::Sqlite(_))
        ));
        assert!(
            schema_objects(&connection)
                .expect("rolled back schema")
                .is_empty()
        );
        assert_eq!(
            pragma_string(&connection, "quick_check").expect("quick check"),
            "ok"
        );

        apply_initial_migration(&mut connection, "now").expect("clean retry");
        validate_migration_registry(&connection).expect("migration registry");
        validate_schema(&connection).expect("initial schema");
    }

    #[test]
    fn temporary_directory_is_removed_during_panic_unwind() {
        let mut observed_path = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let directory = TemporaryDirectory::new("panic-cleanup");
            observed_path = Some(directory.path().to_owned());
            panic!("intentional cleanup control");
        }));
        assert!(result.is_err());
        assert!(
            !observed_path
                .expect("panic fixture path")
                .try_exists()
                .expect("inspect cleanup path")
        );
    }
}
