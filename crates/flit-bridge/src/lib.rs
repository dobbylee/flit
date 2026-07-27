use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use flit_protocol::{
    CapabilityStatus as ProtocolCapabilityStatus, CommandError, CommandErrorCode,
    FingerprintAxis as ProtocolFingerprintAxis, HealthStatus, ManagedRunStartRequest,
    PROTOCOL_VERSION, ProjectInspectionResponse, ProjectListCursor as ProjectListCursorResponse,
    ProjectRecord, ProjectRegistrationResponse, ProjectRegistrationStatus, ProjectTrustResponse,
    ProjectTrustStatus, ProjectsListResponse, ProviderCapability as ProtocolProviderCapability,
    ProviderCapabilityEntry, ProviderCompatibility as ProtocolProviderCompatibility,
    ProviderDiagnosticsResponse, ProviderKind as ProtocolProviderKind, ProviderUnavailableReason,
    SystemHealthResponse,
};
use flit_providers::{
    CapabilityStatus, CodexCompatibilityProbe, CodexCompatibilityProbeError,
    ExecutableInspectionError, FingerprintAxis, ProviderCapability, ProviderCapabilitySnapshot,
    ProviderCompatibility, probe_codex_compatibility_on_path,
};
use flit_store::{
    MAX_PROJECT_PAGE_SIZE, Project, ProjectDirectoryInspection,
    ProjectListCursor as StoreProjectListCursor, ProjectRegistration, ProjectRegistrationOutcome,
    ProjectTrustConfirmation, ProjectTrustOutcome, Store, StoreError,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub mod codex_recovery;
mod managed_start;

const DATABASE_FILE_NAME: &str = "flit.sqlite3";
const LOCK_FILE_NAME: &str = "core.lock";
const MAX_DATA_DIRECTORY_BYTES: usize = 4_096;
const MAX_PROJECT_ID_BYTES: usize = 128;
const MAX_PROJECT_DISPLAY_NAME_BYTES: usize = 256;
const MAX_PROJECT_PATH_BYTES: usize = 4_096;
const MAX_TIMESTAMP_BYTES: usize = 64;
const MAX_PROJECT_RESPONSE_BYTES: usize = 1_048_576;
const MAX_PROVIDER_DIAGNOSTICS_RESPONSE_BYTES: usize = 65_536;
const MAX_MANAGED_RUN_REQUEST_BYTES: usize = 128 * 1_024;

static CORE: LazyLock<CoreManager> = LazyLock::new(CoreManager::default);
static PROVIDER_DIAGNOSTIC_LOCK: LazyLock<Mutex<()>> = LazyLock::new(Mutex::default);
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
    #[error("the Project request is invalid")]
    InvalidProjectRequest,
    #[error("the Project directory could not be inspected")]
    ProjectInspectionFailure,
    #[error("the Project identity conflicts with stored state")]
    ProjectConflict,
    #[error("the Project was not found")]
    ProjectNotFound,
    #[error("the current Project directory identity does not match stored state")]
    ProjectIdentityMismatch,
    #[error("the Project response exceeds the native bridge limit")]
    ProjectResponseTooLarge,
    #[error("the provider diagnostics response exceeds the native bridge limit")]
    ProviderDiagnosticsResponseTooLarge,
    #[error("the managed Run request or response exceeds the native bridge limit")]
    ManagedRunResponseTooLarge,
    #[error("the embedded Rust Core could not serialize the response")]
    SerializationFailure,
}

struct FoundationCore {
    requested_data_directory: PathBuf,
    canonical_data_directory: PathBuf,
    // Rust drops fields in declaration order, so stop providers and close SQLite before the guard.
    managed_runtimes: BTreeMap<String, managed_start::RetainedManagedRun>,
    store: Store,
    provider_health: HealthStatus,
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

    fn provider_health(&self) -> HealthStatus {
        match &*self.lock_state() {
            CoreState::Ready(core) => core.provider_health.clone(),
            CoreState::Uninitialized { .. } => HealthStatus::NotConfigured,
        }
    }

    fn require_ready(&self) -> Result<(), BridgeError> {
        match &*self.lock_state() {
            CoreState::Ready(_) => Ok(()),
            CoreState::Uninitialized { .. } => Err(BridgeError::StorageFailure),
        }
    }

    fn set_provider_health(&self, provider_health: HealthStatus) -> Result<(), BridgeError> {
        match &mut *self.lock_state() {
            CoreState::Ready(core) => {
                core.provider_health = provider_health;
                Ok(())
            }
            CoreState::Uninitialized { .. } => Err(BridgeError::StorageFailure),
        }
    }

    fn with_ready_core<T>(
        &self,
        operation: impl FnOnce(&mut FoundationCore) -> Result<T, BridgeError>,
    ) -> Result<T, BridgeError> {
        let mut state = self.lock_state();
        match &mut *state {
            CoreState::Ready(core) => operation(core),
            CoreState::Uninitialized { .. } => Err(BridgeError::StorageFailure),
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
            managed_runtimes: BTreeMap::new(),
            store,
            provider_health: HealthStatus::NotConfigured,
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
    providers: HealthStatus,
) -> Result<String, BridgeError> {
    let payload = if client_protocol_version == PROTOCOL_VERSION {
        serde_json::to_value(SystemHealthResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            core: HealthStatus::Ready,
            storage,
            providers,
        })
    } else {
        serde_json::to_value(CommandError::protocol_mismatch())
    }
    .map_err(|_| BridgeError::SerializationFailure)?;

    serde_json::to_string(&payload).map_err(|_| BridgeError::SerializationFailure)
}

fn validate_project_input(value: &str, max_bytes: usize) -> Result<(), BridgeError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(BridgeError::InvalidProjectRequest);
    }
    Ok(())
}

fn validate_project_protocol(client_protocol_version: &str) -> Result<(), BridgeError> {
    if client_protocol_version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(BridgeError::ProtocolMismatch)
    }
}

fn project_record(project: Project) -> Result<ProjectRecord, BridgeError> {
    Ok(ProjectRecord {
        id: project.id,
        display_name: project.display_name,
        canonical_path: project
            .canonical_path
            .into_os_string()
            .into_string()
            .map_err(|_| BridgeError::ProjectResponseTooLarge)?,
        filesystem_id: project.filesystem_id,
        trusted: project.trusted,
        default_provider: project.default_provider,
        created_at: project.created_at,
        updated_at: project.updated_at,
    })
}

fn bounded_json<T: serde::Serialize>(
    response: &T,
    max_bytes: usize,
    response_too_large: BridgeError,
) -> Result<String, BridgeError> {
    let rendered =
        serde_json::to_string(response).map_err(|_| BridgeError::SerializationFailure)?;
    if rendered.len() > max_bytes {
        return Err(response_too_large);
    }
    Ok(rendered)
}

fn project_json<T: serde::Serialize>(response: &T) -> Result<String, BridgeError> {
    bounded_json(
        response,
        MAX_PROJECT_RESPONSE_BYTES,
        BridgeError::ProjectResponseTooLarge,
    )
}

fn project_command_error(error: &BridgeError) -> Option<CommandError> {
    let code = match error {
        BridgeError::ProtocolMismatch => CommandErrorCode::ProtocolMismatch,
        BridgeError::InvalidProjectRequest => CommandErrorCode::InvalidProjectRequest,
        BridgeError::ProjectInspectionFailure => CommandErrorCode::ProjectInspectionFailure,
        BridgeError::ProjectConflict => CommandErrorCode::ProjectConflict,
        BridgeError::ProjectNotFound => CommandErrorCode::ProjectNotFound,
        BridgeError::ProjectIdentityMismatch => CommandErrorCode::ProjectIdentityMismatch,
        BridgeError::StorageFailure => CommandErrorCode::StorageUnavailable,
        BridgeError::CoreFailure
        | BridgeError::InvalidDataDirectory
        | BridgeError::CoreAlreadyInitialized
        | BridgeError::CoreAlreadyRunning
        | BridgeError::CoreLockFailure
        | BridgeError::ProjectResponseTooLarge
        | BridgeError::ProviderDiagnosticsResponseTooLarge
        | BridgeError::ManagedRunResponseTooLarge
        | BridgeError::SerializationFailure => return None,
    };
    Some(CommandError::for_code(code))
}

fn project_command_json<T: serde::Serialize>(
    operation: impl FnOnce() -> Result<T, BridgeError>,
) -> Result<String, BridgeError> {
    match operation() {
        Ok(response) => project_json(&response),
        Err(error) => match project_command_error(&error) {
            Some(command_error) => project_json(&command_error),
            None => Err(error),
        },
    }
}

fn map_project_store_error(error: StoreError) -> BridgeError {
    match error {
        StoreError::InvalidProjectRegistration { .. }
        | StoreError::InvalidProjectTrustConfirmation { .. }
        | StoreError::InvalidProjectPageLimit { .. } => BridgeError::InvalidProjectRequest,
        StoreError::ProjectInspection(_) => BridgeError::ProjectInspectionFailure,
        StoreError::ProjectIdConflict { .. } => BridgeError::ProjectConflict,
        StoreError::MissingProject { .. } => BridgeError::ProjectNotFound,
        StoreError::ProjectFilesystemIdentityUnavailable { .. }
        | StoreError::ProjectIdentityMismatch { .. } => BridgeError::ProjectIdentityMismatch,
        _ => BridgeError::StorageFailure,
    }
}

fn protocol_capability(capability: ProviderCapability) -> ProtocolProviderCapability {
    match capability {
        ProviderCapability::Launch => ProtocolProviderCapability::Launch,
        ProviderCapability::ListManaged => ProtocolProviderCapability::ListManaged,
        ProviderCapability::Resume => ProtocolProviderCapability::Resume,
        ProviderCapability::Reconcile => ProtocolProviderCapability::Reconcile,
        ProviderCapability::StructuredActivity => ProtocolProviderCapability::StructuredActivity,
        ProviderCapability::PermissionDetect => ProtocolProviderCapability::PermissionDetect,
        ProviderCapability::PermissionRespond => ProtocolProviderCapability::PermissionRespond,
        ProviderCapability::PermissionPolicyConfigure => {
            ProtocolProviderCapability::PermissionPolicyConfigure
        }
        ProviderCapability::PermissionPolicyObserve => {
            ProtocolProviderCapability::PermissionPolicyObserve
        }
        ProviderCapability::QuestionDetect => ProtocolProviderCapability::QuestionDetect,
        ProviderCapability::QuestionRespond => ProtocolProviderCapability::QuestionRespond,
        ProviderCapability::CompletionDetect => ProtocolProviderCapability::CompletionDetect,
        ProviderCapability::History => ProtocolProviderCapability::History,
        ProviderCapability::OpenInProvider => ProtocolProviderCapability::OpenInProvider,
        ProviderCapability::ContinueAfterQuit => ProtocolProviderCapability::ContinueAfterQuit,
        ProviderCapability::Stop => ProtocolProviderCapability::Stop,
    }
}

fn protocol_capability_status(status: CapabilityStatus) -> ProtocolCapabilityStatus {
    match status {
        CapabilityStatus::Supported => ProtocolCapabilityStatus::Supported,
        CapabilityStatus::Degraded => ProtocolCapabilityStatus::Degraded,
        CapabilityStatus::Unsupported => ProtocolCapabilityStatus::Unsupported,
        CapabilityStatus::Unknown => ProtocolCapabilityStatus::Unknown,
        CapabilityStatus::Unavailable => ProtocolCapabilityStatus::Unavailable,
    }
}

fn protocol_compatibility(compatibility: ProviderCompatibility) -> ProtocolProviderCompatibility {
    match compatibility {
        ProviderCompatibility::Supported => ProtocolProviderCompatibility::Supported,
        ProviderCompatibility::Degraded => ProtocolProviderCompatibility::Degraded,
        ProviderCompatibility::Unknown => ProtocolProviderCompatibility::Unknown,
        ProviderCompatibility::Unavailable => ProtocolProviderCompatibility::Unavailable,
    }
}

fn protocol_fingerprint_axis(axis: FingerprintAxis) -> ProtocolFingerprintAxis {
    match axis {
        FingerprintAxis::CanonicalExecutable => ProtocolFingerprintAxis::CanonicalExecutable,
        FingerprintAxis::ExecutableVersion => ProtocolFingerprintAxis::ExecutableVersion,
        FingerprintAxis::ExecutableSha256 => ProtocolFingerprintAxis::ExecutableSha256,
        FingerprintAxis::CombinedSchemaSha256 => ProtocolFingerprintAxis::CombinedSchemaSha256,
        FingerprintAxis::V2SchemaSha256 => ProtocolFingerprintAxis::V2SchemaSha256,
        FingerprintAxis::MethodAllowlistSha256 => ProtocolFingerprintAxis::MethodAllowlistSha256,
        FingerprintAxis::FixtureSha256 => ProtocolFingerprintAxis::FixtureSha256,
        FingerprintAxis::SmokeRunId => ProtocolFingerprintAxis::SmokeRunId,
    }
}

fn protocol_capability_entries(
    snapshot: ProviderCapabilitySnapshot,
) -> Vec<ProviderCapabilityEntry> {
    snapshot
        .capabilities
        .into_iter()
        .map(|entry| ProviderCapabilityEntry {
            capability: protocol_capability(entry.capability),
            status: protocol_capability_status(entry.status),
        })
        .collect()
}

fn unavailable_capabilities() -> Vec<ProviderCapabilityEntry> {
    ProviderCapability::ALL
        .map(|capability| ProviderCapabilityEntry {
            capability: protocol_capability(capability),
            status: ProtocolCapabilityStatus::Unavailable,
        })
        .to_vec()
}

fn provider_unavailable_reason(error: &CodexCompatibilityProbeError) -> ProviderUnavailableReason {
    match error {
        CodexCompatibilityProbeError::Inspection(ExecutableInspectionError::NotFoundOnPath {
            ..
        }) => ProviderUnavailableReason::ExecutableNotFound,
        CodexCompatibilityProbeError::Inspection(_) => {
            ProviderUnavailableReason::ExecutableUnavailable
        }
        CodexCompatibilityProbeError::Version(_) => ProviderUnavailableReason::VersionProbeFailed,
        CodexCompatibilityProbeError::Schema(_) => ProviderUnavailableReason::SchemaProbeFailed,
        CodexCompatibilityProbeError::BundledEvidenceMismatch => {
            ProviderUnavailableReason::BundledEvidenceMismatch
        }
    }
}

fn provider_diagnostics_response(
    probe: Result<CodexCompatibilityProbe, CodexCompatibilityProbeError>,
) -> ProviderDiagnosticsResponse {
    match probe {
        Ok(probe) => {
            let compatibility = protocol_compatibility(probe.capability_snapshot.compatibility);
            ProviderDiagnosticsResponse {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                provider: ProtocolProviderKind::Codex,
                compatibility,
                executable_version: Some(probe.runtime_fingerprint.executable_version),
                capabilities: protocol_capability_entries(probe.capability_snapshot.clone()),
                fingerprint_mismatches: probe
                    .capability_snapshot
                    .fingerprint_mismatches
                    .into_iter()
                    .map(protocol_fingerprint_axis)
                    .collect(),
                unavailable_reason: None,
            }
        }
        Err(error) => ProviderDiagnosticsResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            provider: ProtocolProviderKind::Codex,
            compatibility: ProtocolProviderCompatibility::Unavailable,
            executable_version: None,
            capabilities: unavailable_capabilities(),
            fingerprint_mismatches: Vec::new(),
            unavailable_reason: Some(provider_unavailable_reason(&error)),
        },
    }
}

fn provider_health_for(response: &ProviderDiagnosticsResponse) -> HealthStatus {
    match response.compatibility {
        ProtocolProviderCompatibility::Supported | ProtocolProviderCompatibility::Degraded => {
            HealthStatus::Ready
        }
        ProtocolProviderCompatibility::Unknown | ProtocolProviderCompatibility::Unavailable => {
            HealthStatus::Unavailable
        }
    }
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
    protect(|| {
        health_json(
            &client_protocol_version,
            CORE.storage_health(),
            CORE.provider_health(),
        )
    })
}

#[uniffi::export]
pub fn provider_diagnostics_json(client_protocol_version: String) -> Result<String, BridgeError> {
    protect(|| {
        let validation = || {
            validate_project_protocol(&client_protocol_version)?;
            CORE.require_ready()?;
            Ok(())
        };
        match validation() {
            Ok(()) => with_provider_diagnostic_lock(|| {
                let path_environment = std::env::var_os("PATH");
                let response = provider_diagnostics_response(probe_codex_compatibility_on_path(
                    path_environment.as_deref(),
                ));
                let rendered = bounded_json(
                    &response,
                    MAX_PROVIDER_DIAGNOSTICS_RESPONSE_BYTES,
                    BridgeError::ProviderDiagnosticsResponseTooLarge,
                )?;
                CORE.set_provider_health(provider_health_for(&response))?;
                Ok(rendered)
            }),
            Err(error) => project_command_json::<ProviderDiagnosticsResponse>(|| Err(error)),
        }
    })
}

fn with_provider_diagnostic_lock<T>(operation: impl FnOnce() -> T) -> T {
    let _diagnostic_guard = PROVIDER_DIAGNOSTIC_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    operation()
}

#[uniffi::export]
pub fn project_inspect_json(
    selected_path: String,
    client_protocol_version: String,
) -> Result<String, BridgeError> {
    protect(|| {
        project_command_json(|| {
            validate_project_protocol(&client_protocol_version)?;
            validate_project_input(&selected_path, MAX_PROJECT_PATH_BYTES)?;
            CORE.with_ready_core(|_| {
                let inspection = ProjectDirectoryInspection::inspect(&selected_path)
                    .map_err(|_| BridgeError::ProjectInspectionFailure)?;
                Ok(ProjectInspectionResponse {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    canonical_path: inspection
                        .identity
                        .canonical_path
                        .into_os_string()
                        .into_string()
                        .map_err(|_| BridgeError::ProjectInspectionFailure)?,
                    filesystem_id: inspection.identity.filesystem_id,
                    selected_via_symlink: inspection.selected_via_symlink,
                })
            })
        })
    })
}

#[uniffi::export]
pub fn project_register_json(
    project_id: String,
    display_name: String,
    selected_path: String,
    created_at: String,
    client_protocol_version: String,
) -> Result<String, BridgeError> {
    protect(|| {
        project_command_json(|| {
            validate_project_protocol(&client_protocol_version)?;
            validate_project_input(&project_id, MAX_PROJECT_ID_BYTES)?;
            validate_project_input(&display_name, MAX_PROJECT_DISPLAY_NAME_BYTES)?;
            validate_project_input(&selected_path, MAX_PROJECT_PATH_BYTES)?;
            validate_project_input(&created_at, MAX_TIMESTAMP_BYTES)?;
            CORE.with_ready_core(|core| {
                match core
                    .store
                    .register_project(ProjectRegistration {
                        id: project_id,
                        display_name,
                        selected_path: PathBuf::from(selected_path),
                        created_at,
                    })
                    .map_err(map_project_store_error)?
                {
                    ProjectRegistrationOutcome::Registered(project) => {
                        Ok(ProjectRegistrationResponse {
                            protocol_version: PROTOCOL_VERSION.to_owned(),
                            status: ProjectRegistrationStatus::Registered,
                            project: Some(project_record(project)?),
                            existing_project_id: None,
                        })
                    }
                    ProjectRegistrationOutcome::DuplicateCanonicalPath {
                        existing_project_id,
                    } => Ok(ProjectRegistrationResponse {
                        protocol_version: PROTOCOL_VERSION.to_owned(),
                        status: ProjectRegistrationStatus::DuplicateCanonicalPath,
                        project: None,
                        existing_project_id: Some(existing_project_id),
                    }),
                    ProjectRegistrationOutcome::DuplicateFilesystemIdentity {
                        existing_project_id,
                    } => Ok(ProjectRegistrationResponse {
                        protocol_version: PROTOCOL_VERSION.to_owned(),
                        status: ProjectRegistrationStatus::DuplicateFilesystemIdentity,
                        project: None,
                        existing_project_id: Some(existing_project_id),
                    }),
                }
            })
        })
    })
}

#[uniffi::export]
pub fn project_trust_json(
    project_id: String,
    selected_path: String,
    confirmed_at: String,
    client_protocol_version: String,
) -> Result<String, BridgeError> {
    protect(|| {
        project_command_json(|| {
            validate_project_protocol(&client_protocol_version)?;
            validate_project_input(&project_id, MAX_PROJECT_ID_BYTES)?;
            validate_project_input(&selected_path, MAX_PROJECT_PATH_BYTES)?;
            validate_project_input(&confirmed_at, MAX_TIMESTAMP_BYTES)?;
            CORE.with_ready_core(|core| {
                let (status, project) = match core
                    .store
                    .confirm_project_trust(ProjectTrustConfirmation {
                        project_id,
                        selected_path: PathBuf::from(selected_path),
                        confirmed_at,
                    })
                    .map_err(map_project_store_error)?
                {
                    ProjectTrustOutcome::Trusted(project) => (ProjectTrustStatus::Trusted, project),
                    ProjectTrustOutcome::AlreadyTrusted(project) => {
                        (ProjectTrustStatus::AlreadyTrusted, project)
                    }
                };
                Ok(ProjectTrustResponse {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    status,
                    project: project_record(project)?,
                })
            })
        })
    })
}

#[uniffi::export]
pub fn projects_list_page_json(
    after_display_name: Option<String>,
    after_project_id: Option<String>,
    limit: u32,
    client_protocol_version: String,
) -> Result<String, BridgeError> {
    protect(|| {
        project_command_json(|| {
            validate_project_protocol(&client_protocol_version)?;
            let limit = usize::try_from(limit).map_err(|_| BridgeError::InvalidProjectRequest)?;
            if !(1..=MAX_PROJECT_PAGE_SIZE).contains(&limit) {
                return Err(BridgeError::InvalidProjectRequest);
            }
            let after = match (after_display_name, after_project_id) {
                (None, None) => None,
                (Some(display_name), Some(project_id)) => {
                    validate_project_input(&display_name, MAX_PROJECT_DISPLAY_NAME_BYTES)?;
                    validate_project_input(&project_id, MAX_PROJECT_ID_BYTES)?;
                    Some(StoreProjectListCursor {
                        display_name,
                        project_id,
                    })
                }
                _ => return Err(BridgeError::InvalidProjectRequest),
            };
            CORE.with_ready_core(|core| {
                let page = core
                    .store
                    .list_projects_page(after.as_ref(), limit)
                    .map_err(map_project_store_error)?;
                Ok(ProjectsListResponse {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    projects: page
                        .projects
                        .into_iter()
                        .map(project_record)
                        .collect::<Result<Vec<_>, _>>()?,
                    next_cursor: page.next_cursor.map(|cursor| ProjectListCursorResponse {
                        display_name: cursor.display_name,
                        project_id: cursor.project_id,
                    }),
                })
            })
        })
    })
}

#[uniffi::export]
pub fn managed_run_start_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| {
        if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
        let request = match serde_json::from_str::<ManagedRunStartRequest>(&request_json) {
            Ok(request) => request,
            Err(_) => {
                return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
            }
        };
        if request.client_protocol_version != PROTOCOL_VERSION {
            return project_json(&CommandError::protocol_mismatch());
        }
        let result = with_provider_diagnostic_lock(|| {
            CORE.with_ready_core(|core| {
                let path_environment = std::env::var_os("PATH");
                match managed_start::start_managed_run(
                    &mut core.store,
                    &mut core.managed_runtimes,
                    &managed_start::ProductionCodexConnector,
                    path_environment.as_deref(),
                    request,
                ) {
                    Ok(response) => {
                        core.provider_health = HealthStatus::Ready;
                        bounded_json(
                            &response,
                            MAX_PROJECT_RESPONSE_BYTES,
                            BridgeError::ManagedRunResponseTooLarge,
                        )
                    }
                    Err(error) => {
                        if error == managed_start::ManagedStartError::ProviderUnavailable {
                            core.provider_health = HealthStatus::Unavailable;
                        }
                        project_json(&CommandError::for_code(managed_start_error_code(error)))
                    }
                }
            })
        });
        match result {
            Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
                CommandErrorCode::StorageUnavailable,
            )),
            result => result,
        }
    })
}

fn managed_start_error_code(error: managed_start::ManagedStartError) -> CommandErrorCode {
    match error {
        managed_start::ManagedStartError::InvalidRequest => CommandErrorCode::InvalidRunRequest,
        managed_start::ManagedStartError::RunConflict => CommandErrorCode::RunConflict,
        managed_start::ManagedStartError::ProjectNotFound => CommandErrorCode::ProjectNotFound,
        managed_start::ManagedStartError::ProjectNotTrusted => CommandErrorCode::ProjectNotTrusted,
        managed_start::ManagedStartError::ProjectIdentityMismatch => {
            CommandErrorCode::ProjectIdentityMismatch
        }
        managed_start::ManagedStartError::ProviderUnavailable => {
            CommandErrorCode::ProviderUnavailable
        }
        managed_start::ManagedStartError::ProviderStartFailed => {
            CommandErrorCode::ProviderStartFailed
        }
        managed_start::ManagedStartError::ProviderStartUnknown => {
            CommandErrorCode::ProviderStartUnknown
        }
        managed_start::ManagedStartError::StorageUnavailable => {
            CommandErrorCode::StorageUnavailable
        }
    }
}

#[uniffi::export]
pub fn core_construction_count() -> u64 {
    CORE_CONSTRUCTIONS.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use flit_protocol::SystemHealthRequest;
    use flit_providers::{
        CodexRuntimeFingerprint, classify_codex, validated_codex_0_144_6_fingerprint,
    };

    use super::*;

    fn fixture_at(version: &str, name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(format!("fixtures/protocol/commands/v{version}"))
            .join(name);
        serde_json::from_str(&fs::read_to_string(path).expect("health fixture should be readable"))
            .expect("health fixture should be valid JSON")
    }

    fn fixture(name: &str) -> serde_json::Value {
        fixture_at(PROTOCOL_VERSION, name)
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
            &health_json(
                &request.client_protocol_version,
                HealthStatus::Ready,
                HealthStatus::NotConfigured,
            )
            .expect("matching protocol should return health"),
        )
        .expect("normal bridge payload should be valid JSON");
        let previous_request: SystemHealthRequest =
            serde_json::from_value(fixture_at("1.1", "system_health.request.json"))
                .expect("previous health request should remain readable");
        let mismatch: serde_json::Value = serde_json::from_str(
            &health_json(
                &previous_request.client_protocol_version,
                HealthStatus::NotConfigured,
                HealthStatus::NotConfigured,
            )
            .expect("protocol mismatch should return the typed command payload"),
        )
        .expect("mismatch bridge payload should be valid JSON");

        assert_eq!(normal, fixture("system_health.response.json"));
        assert_eq!(mismatch, fixture("protocol_mismatch.error.json"));
    }

    #[test]
    fn managed_run_start_rejects_malformed_and_mismatched_requests_as_command_errors() {
        let malformed: CommandError = serde_json::from_str(
            &managed_run_start_json("{}".to_owned()).expect("malformed command response"),
        )
        .expect("typed malformed command error");
        assert_eq!(
            malformed,
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );

        let mut mismatch = fixture("managed_run_start.request.json");
        mismatch["client_protocol_version"] = serde_json::Value::String("9.9".to_owned());
        let mismatch: CommandError = serde_json::from_str(
            &managed_run_start_json(mismatch.to_string()).expect("mismatch command response"),
        )
        .expect("typed mismatch command error");
        assert_eq!(mismatch, CommandError::protocol_mismatch());
    }

    #[test]
    fn supported_and_unavailable_diagnostics_match_protocol_fixtures_and_health() {
        let profile = validated_codex_0_144_6_fingerprint();
        let supported = provider_diagnostics_response(Ok(CodexCompatibilityProbe {
            runtime_fingerprint: CodexRuntimeFingerprint {
                canonical_executable: profile.canonical_executable.clone(),
                executable_version: profile.executable_version.clone(),
                executable_sha256: profile.executable_sha256.clone(),
                combined_schema_sha256: profile.combined_schema_sha256.clone(),
                v2_schema_sha256: profile.v2_schema_sha256.clone(),
            },
            validated_profile: Some(profile.clone()),
            capability_snapshot: classify_codex(&profile),
            version_stderr_bytes: 0,
            schema_stdout_bytes: 0,
            schema_stderr_bytes: 0,
        }));
        assert_eq!(
            serde_json::to_value(&supported).expect("supported diagnostics serialize"),
            fixture("provider_diagnostics.supported.response.json")
        );
        assert_eq!(provider_health_for(&supported), HealthStatus::Ready);
        let ready_health: serde_json::Value = serde_json::from_str(
            &health_json(
                PROTOCOL_VERSION,
                HealthStatus::Ready,
                provider_health_for(&supported),
            )
            .expect("ready provider health"),
        )
        .expect("ready provider health JSON");
        assert_eq!(
            ready_health,
            fixture("system_health.providers_ready.response.json")
        );

        let unavailable = provider_diagnostics_response(Err(
            CodexCompatibilityProbeError::Inspection(ExecutableInspectionError::NotFoundOnPath {
                searched_directories: Vec::new(),
            }),
        ));
        assert_eq!(
            serde_json::to_value(&unavailable).expect("unavailable diagnostics serialize"),
            fixture("provider_diagnostics.unavailable.response.json")
        );
        assert_eq!(provider_health_for(&unavailable), HealthStatus::Unavailable);
    }

    #[test]
    fn concurrent_diagnostics_serialize_probe_through_health_commit() {
        let active_operations = Arc::new(AtomicUsize::new(0));
        let maximum_active = Arc::new(AtomicUsize::new(0));
        let committed_health = Arc::new(AtomicUsize::new(0));
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (second_attempted_tx, second_attempted_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();

        let first_active = Arc::clone(&active_operations);
        let first_maximum = Arc::clone(&maximum_active);
        let first_health = Arc::clone(&committed_health);
        let first = thread::spawn(move || {
            with_provider_diagnostic_lock(|| {
                record_active(&first_active, &first_maximum);
                first_entered_tx.send(()).expect("signal first entry");
                release_first_rx.recv().expect("release first operation");
                first_health.store(1, Ordering::SeqCst);
                first_active.fetch_sub(1, Ordering::SeqCst);
            });
        });
        first_entered_rx.recv().expect("first operation enters");

        let second_active = Arc::clone(&active_operations);
        let second_maximum = Arc::clone(&maximum_active);
        let second_health = Arc::clone(&committed_health);
        let second = thread::spawn(move || {
            second_attempted_tx
                .send(())
                .expect("signal second lock attempt");
            with_provider_diagnostic_lock(|| {
                record_active(&second_active, &second_maximum);
                second_entered_tx.send(()).expect("signal second entry");
                second_health.store(2, Ordering::SeqCst);
                second_active.fetch_sub(1, Ordering::SeqCst);
            });
        });
        second_attempted_rx
            .recv()
            .expect("second operation attempts lock");
        assert!(
            second_entered_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "second probe must not enter before the first health commit"
        );

        release_first_tx.send(()).expect("release first operation");
        first.join().expect("join first diagnostics operation");
        second.join().expect("join second diagnostics operation");
        assert_eq!(maximum_active.load(Ordering::SeqCst), 1);
        assert_eq!(committed_health.load(Ordering::SeqCst), 2);
    }

    fn record_active(active: &AtomicUsize, maximum: &AtomicUsize) {
        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
        maximum.fetch_max(current, Ordering::SeqCst);
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
