use std::{
    fs::{self, File, OpenOptions, TryLockError},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use flit_protocol::{CommandError, HealthStatus, PROTOCOL_VERSION, SystemHealthResponse};
use flit_store::Store;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub mod codex_recovery;

const DATABASE_FILE_NAME: &str = "flit.sqlite3";
const LOCK_FILE_NAME: &str = "core.lock";
const MAX_DATA_DIRECTORY_BYTES: usize = 4_096;

static CORE: LazyLock<CoreManager> = LazyLock::new(CoreManager::default);
static CORE_CONSTRUCTIONS: AtomicU64 = AtomicU64::new(0);

uniffi::setup_scaffolding!();

#[derive(Debug, Eq, PartialEq, thiserror::Error, uniffi::Error)]
pub enum BridgeError {
    #[error("the embedded Rust Core could not complete the request")]
    CoreFailure,
    #[error("the native client protocol does not match the embedded Rust Core")]
    ProtocolMismatch,
    #[error("the Core data directory is invalid or unavailable")]
    InvalidDataDirectory,
    #[error("the embedded Rust Core is already initialized for another data directory")]
    CoreAlreadyInitialized,
    #[error("another Flit Core already owns the data directory")]
    CoreAlreadyRunning,
    #[error("the Core data-directory lock could not be acquired")]
    CoreLockFailure,
    #[error("the Core Store could not be initialized")]
    StorageFailure,
    #[error("the embedded Rust Core could not serialize the response")]
    SerializationFailure,
}

struct FoundationCore {
    requested_data_directory: PathBuf,
    canonical_data_directory: PathBuf,
    // Rust drops fields in declaration order, so close SQLite before releasing the guard.
    _store: Store,
    _guard: File,
}

enum CoreState {
    Uninitialized { initialization_failed: bool },
    Ready(FoundationCore),
}

impl Default for CoreState {
    fn default() -> Self {
        Self::Uninitialized {
            initialization_failed: false,
        }
    }
}

#[derive(Default)]
struct CoreManager {
    state: Mutex<CoreState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitializationOutcome {
    Initialized,
    AlreadyInitialized,
}

impl CoreManager {
    fn initialize(&self, data_directory: &str) -> Result<InitializationOutcome, BridgeError> {
        let requested_data_directory = match validate_data_directory(data_directory) {
            Ok(path) => path,
            Err(error) => {
                let mut state = self.lock_state();
                if !matches!(*state, CoreState::Ready(_)) {
                    *state = CoreState::Uninitialized {
                        initialization_failed: true,
                    };
                }
                return Err(error);
            }
        };
        let mut state = self.lock_state();

        if let CoreState::Ready(core) = &*state {
            return if core.matches_data_directory(&requested_data_directory) {
                Ok(InitializationOutcome::AlreadyInitialized)
            } else {
                Err(BridgeError::CoreAlreadyInitialized)
            };
        }

        match FoundationCore::open(requested_data_directory) {
            Ok(core) => {
                *state = CoreState::Ready(core);
                Ok(InitializationOutcome::Initialized)
            }
            Err(error) => {
                *state = CoreState::Uninitialized {
                    initialization_failed: true,
                };
                Err(error)
            }
        }
    }

    fn storage_health(&self) -> HealthStatus {
        match &*self.lock_state() {
            CoreState::Ready(_) => HealthStatus::Ready,
            CoreState::Uninitialized {
                initialization_failed: true,
            } => HealthStatus::Unavailable,
            CoreState::Uninitialized {
                initialization_failed: false,
            } => HealthStatus::NotConfigured,
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, CoreState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl FoundationCore {
    fn open(requested_data_directory: PathBuf) -> Result<Self, BridgeError> {
        fs::create_dir_all(&requested_data_directory)
            .map_err(|_| BridgeError::InvalidDataDirectory)?;
        let canonical_data_directory = fs::canonicalize(&requested_data_directory)
            .map_err(|_| BridgeError::InvalidDataDirectory)?;
        if !canonical_data_directory.is_dir() {
            return Err(BridgeError::InvalidDataDirectory);
        }
        make_owner_only_directory(&canonical_data_directory)?;

        let runtime_directory = canonical_data_directory.join("runtime");
        fs::create_dir_all(&runtime_directory).map_err(|_| BridgeError::InvalidDataDirectory)?;
        let runtime_metadata = fs::symlink_metadata(&runtime_directory)
            .map_err(|_| BridgeError::InvalidDataDirectory)?;
        if runtime_metadata.file_type().is_symlink() || !runtime_metadata.is_dir() {
            return Err(BridgeError::InvalidDataDirectory);
        }
        let canonical_runtime_directory =
            fs::canonicalize(&runtime_directory).map_err(|_| BridgeError::InvalidDataDirectory)?;
        if canonical_runtime_directory.parent() != Some(canonical_data_directory.as_path()) {
            return Err(BridgeError::InvalidDataDirectory);
        }
        make_owner_only_directory(&canonical_runtime_directory)?;

        let guard = open_owner_only_file(&canonical_runtime_directory.join(LOCK_FILE_NAME))?;
        match guard.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(BridgeError::CoreAlreadyRunning),
            Err(TryLockError::Error(_)) => return Err(BridgeError::CoreLockFailure),
        }

        let database_path = canonical_data_directory.join(DATABASE_FILE_NAME);
        let wal_path = sidecar_path(&database_path, "-wal");
        let shared_memory_path = sidecar_path(&database_path, "-shm");
        make_owner_only_file_if_present(&wal_path)?;
        make_owner_only_file_if_present(&shared_memory_path)?;
        let _database_file = open_owner_only_file(&database_path)?;
        let store = Store::open_with_system_time(&database_path)
            .map_err(|_| BridgeError::StorageFailure)?;
        make_owner_only_file(&database_path)?;
        make_owner_only_file_if_present(&wal_path)?;
        make_owner_only_file_if_present(&shared_memory_path)?;

        Ok(Self {
            requested_data_directory,
            canonical_data_directory,
            _store: store,
            _guard: guard,
        })
    }

    fn matches_data_directory(&self, requested: &Path) -> bool {
        if requested == self.requested_data_directory {
            return true;
        }
        fs::canonicalize(requested)
            .map(|canonical| canonical == self.canonical_data_directory)
            .unwrap_or(false)
    }
}

fn validate_data_directory(data_directory: &str) -> Result<PathBuf, BridgeError> {
    if data_directory.is_empty()
        || data_directory.len() > MAX_DATA_DIRECTORY_BYTES
        || data_directory.contains('\0')
    {
        return Err(BridgeError::InvalidDataDirectory);
    }
    let path = PathBuf::from(data_directory);
    if !path.is_absolute() {
        return Err(BridgeError::InvalidDataDirectory);
    }
    Ok(path)
}

fn open_owner_only_file(path: &Path) -> Result<File, BridgeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(BridgeError::InvalidDataDirectory);
        }
        Ok(metadata) if !metadata.is_file() || has_multiple_links(&metadata) => {
            return Err(BridgeError::InvalidDataDirectory);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(BridgeError::InvalidDataDirectory),
    }

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(path)
        .map_err(|_| BridgeError::InvalidDataDirectory)?;
    let metadata = file
        .metadata()
        .map_err(|_| BridgeError::InvalidDataDirectory)?;
    if !metadata.is_file() || has_multiple_links(&metadata) {
        return Err(BridgeError::InvalidDataDirectory);
    }
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| BridgeError::InvalidDataDirectory)?;
    Ok(file)
}

fn make_owner_only_directory(path: &Path) -> Result<(), BridgeError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| BridgeError::InvalidDataDirectory)?;
    Ok(())
}

fn make_owner_only_file(path: &Path) -> Result<(), BridgeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| BridgeError::InvalidDataDirectory)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || has_multiple_links(&metadata) {
        return Err(BridgeError::InvalidDataDirectory);
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| BridgeError::InvalidDataDirectory)?;
    Ok(())
}

fn has_multiple_links(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        metadata.nlink() != 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn make_owner_only_file_if_present(path: &Path) -> Result<(), BridgeError> {
    match fs::symlink_metadata(path) {
        Ok(_) => make_owner_only_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(BridgeError::InvalidDataDirectory),
    }
}

fn sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn protect<T>(operation: impl FnOnce() -> Result<T, BridgeError>) -> Result<T, BridgeError> {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(Err(BridgeError::CoreFailure))
}

fn health_json(
    client_protocol_version: &str,
    storage: HealthStatus,
) -> Result<String, BridgeError> {
    let payload = if client_protocol_version == PROTOCOL_VERSION {
        serde_json::to_value(SystemHealthResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            core: HealthStatus::Ready,
            storage,
            providers: HealthStatus::NotConfigured,
        })
    } else {
        serde_json::to_value(CommandError::protocol_mismatch())
    }
    .map_err(|_| BridgeError::SerializationFailure)?;

    serde_json::to_string(&payload).map_err(|_| BridgeError::SerializationFailure)
}

#[uniffi::export]
pub fn initialize_core(
    data_directory: String,
    client_protocol_version: String,
) -> Result<(), BridgeError> {
    protect(|| {
        if client_protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::ProtocolMismatch);
        }
        if CORE.initialize(&data_directory)? == InitializationOutcome::Initialized {
            CORE_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    })
}

#[uniffi::export]
pub fn system_health_json(client_protocol_version: String) -> Result<String, BridgeError> {
    protect(|| health_json(&client_protocol_version, CORE.storage_health()))
}

#[uniffi::export]
pub fn core_construction_count() -> u64 {
    CORE_CONSTRUCTIONS.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use flit_protocol::SystemHealthRequest;

    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/protocol/commands/v1.0")
            .join(name);
        serde_json::from_str(&fs::read_to_string(path).expect("health fixture should be readable"))
            .expect("health fixture should be valid JSON")
    }

    #[test]
    fn normal_and_mismatch_payloads_match_the_protocol_fixtures() {
        let request_fixture = fixture("system_health.request.json");
        let request: SystemHealthRequest = serde_json::from_value(request_fixture.clone())
            .expect("health request fixture should match the Rust contract");
        assert_eq!(
            serde_json::to_value(&request).expect("health request should serialize"),
            request_fixture
        );
        let normal: serde_json::Value = serde_json::from_str(
            &health_json(&request.client_protocol_version, HealthStatus::Ready)
                .expect("matching protocol should return health"),
        )
        .expect("normal bridge payload should be valid JSON");
        let mismatch: serde_json::Value = serde_json::from_str(
            &health_json("2.0", HealthStatus::NotConfigured)
                .expect("protocol mismatch should return the typed command payload"),
        )
        .expect("mismatch bridge payload should be valid JSON");

        assert_eq!(normal, fixture("system_health.response.json"));
        assert_eq!(mismatch, fixture("protocol_mismatch.error.json"));
    }

    #[test]
    fn health_calls_do_not_construct_storage() {
        for _ in 0..100 {
            system_health_json(PROTOCOL_VERSION.to_owned())
                .expect("health should remain available");
        }

        assert_eq!(core_construction_count(), 0);
    }

    #[test]
    fn panic_is_contained_and_does_not_poison_the_next_request() {
        let failure = protect::<()>(|| panic!("bridge panic control"));
        assert_eq!(failure, Err(BridgeError::CoreFailure));
        assert!(system_health_json(PROTOCOL_VERSION.to_owned()).is_ok());
    }
}
