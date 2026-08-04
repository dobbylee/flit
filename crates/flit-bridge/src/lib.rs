use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions, TryLockError},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use flit_git::{
    GitHead as NativeGitHead, GitObservation as NativeGitObservation, GitObservationError,
    NotWorktreeReason as NativeNotWorktreeReason, inspect_git_on_path, inspect_noexec_runner_at,
    observe_repository,
};
use flit_protocol::{
    CapabilityStatus as ProtocolCapabilityStatus, CommandError, CommandErrorCode,
    DashboardEventRecord, DashboardReadRequest, DashboardReadResponse, DashboardRunRecord,
    DashboardSnapshotReason, EVENT_PROTOCOL_VERSION, EventSourceKind,
    FingerprintAxis as ProtocolFingerprintAxis, GitBaselinePayload, GitBaselineUnavailableReason,
    GitDirtySummary, GitHead, GitNotWorktreeReason, GitObservationResponse,
    GitObservationUnavailableReason, HealthStatus, ManagedRunObserveRequest,
    ManagedRunObserveResponse, ManagedRunOpenInProviderRequest, ManagedRunPermissionRespondRequest,
    ManagedRunPermissionRespondResponse, ManagedRunStartRequest, PROTOCOL_VERSION,
    ProjectInspectionResponse, ProjectListCursor as ProjectListCursorResponse, ProjectRecord,
    ProjectRegistrationResponse, ProjectRegistrationStatus, ProjectTrustResponse,
    ProjectTrustStatus, ProjectsListResponse, ProviderCapability as ProtocolProviderCapability,
    ProviderCapabilityEntry, ProviderCompatibility as ProtocolProviderCompatibility,
    ProviderDiagnosticsResponse, ProviderExecutionAfterQuit, ProviderKind as ProtocolProviderKind,
    ProviderUnavailableReason, QuitImpactReason, QuitImpactResponse, QuitImpactRun,
    RunDetailReadRequest, RunDetailReadResponse, RunEvidenceRecord, SystemHealthResponse,
};
use flit_providers::{
    CapabilityStatus, CodexCompatibilityProbe, CodexCompatibilityProbeError,
    ExecutableInspectionError, FingerprintAxis, MAX_CODEX_COMMAND_STARTS_PER_TURN,
    ProviderCapability, ProviderCapabilitySnapshot, ProviderCompatibility,
    probe_codex_compatibility_on_path,
};
use flit_store::{
    AppendEventOutcome, DashboardChangeSummary as StoreDashboardChangeSummary,
    DashboardRunSnapshot as StoreDashboardRunSnapshot, MAX_DASHBOARD_DELTA_EVENTS,
    MAX_LIVE_MANAGED_SESSIONS, MAX_PROJECT_PAGE_SIZE, MAX_RUN_DETAIL_EVENTS,
    ManagedPermissionDecision, ManagedPermissionDeliveryUnknownReason,
    ManagedPermissionResponseAttemptOutcome, ManagedPermissionResponseResult,
    ManagedPermissionResponseResultKind, ManagedSession, Project, ProjectDirectoryInspection,
    ProjectListCursor as StoreProjectListCursor, ProjectRegistration, ProjectRegistrationOutcome,
    ProjectTrustConfirmation, ProjectTrustOutcome, Store, StoreError,
};
use sha2::{Digest, Sha256};

use crate::codex_recovery::{
    CodexRecoveryAttempt, ExactCodexRecoveryConnector, observe_codex_sessions,
    persist_codex_recovery_observations, unknown_codex_recovery_observations,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub mod codex_recovery;
mod managed_start;
#[cfg(test)]
mod phase2_journey;

const DATABASE_FILE_NAME: &str = "flit.sqlite3";
const LOCK_FILE_NAME: &str = "core.lock";
const MAX_DATA_DIRECTORY_BYTES: usize = 4_096;
const MAX_PROJECT_ID_BYTES: usize = 128;
const MAX_PROJECT_DISPLAY_NAME_BYTES: usize = 256;
const MAX_PROJECT_PATH_BYTES: usize = 4_096;
const MAX_TIMESTAMP_BYTES: usize = 64;
const MAX_PROJECT_RESPONSE_BYTES: usize = 1_048_576;
const MAX_PROVIDER_DIAGNOSTICS_RESPONSE_BYTES: usize = 65_536;
const MAX_QUIT_IMPACT_RESPONSE_BYTES: usize = 1_048_576;
const MAX_MANAGED_RUN_REQUEST_BYTES: usize = 128 * 1_024;
const MAX_DASHBOARD_REQUEST_BYTES: usize = 64 * 1_024;
const MAX_DASHBOARD_RESPONSE_BYTES: usize = 8 * 1_024 * 1_024;
const MAX_RUN_DETAIL_RESPONSE_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_CORE_INSTANCE_ID_BYTES: usize = 256;
const DASHBOARD_DELTA_RETENTION_EVENTS: u64 = 2_000;

static CORE: LazyLock<CoreManager> = LazyLock::new(CoreManager::default);
static PROVIDER_DIAGNOSTIC_LOCK: LazyLock<Mutex<()>> = LazyLock::new(Mutex::default);
static CORE_CONSTRUCTIONS: AtomicU64 = AtomicU64::new(0);
static CORE_INSTANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    #[error("the Project is not trusted")]
    ProjectNotTrusted,
    #[error("the current Project directory identity does not match stored state")]
    ProjectIdentityMismatch,
    #[error("the Project response exceeds the native bridge limit")]
    ProjectResponseTooLarge,
    #[error("the provider diagnostics response exceeds the native bridge limit")]
    ProviderDiagnosticsResponseTooLarge,
    #[error("the explicit Quit impact response exceeds the native bridge limit")]
    QuitImpactResponseTooLarge,
    #[error("the Dashboard request is invalid")]
    InvalidDashboardRequest,
    #[error("the Dashboard response exceeds the native bridge limit")]
    DashboardResponseTooLarge,
    #[error("the managed Run request or response exceeds the native bridge limit")]
    ManagedRunResponseTooLarge,
    #[error("the managed Run request is invalid")]
    InvalidRunRequest,
    #[error("the managed Run was not found")]
    RunNotFound,
    #[error("the managed Run version is stale")]
    RunVersionStale,
    #[error("the provider capability is unsupported")]
    CapabilityUnsupported,
    #[error("the provider capability is unavailable")]
    ProviderUnavailable,
    #[error("the embedded Rust Core could not serialize the response")]
    SerializationFailure,
}

struct FoundationCore {
    requested_data_directory: PathBuf,
    canonical_data_directory: PathBuf,
    core_instance_id: String,
    // Rust drops fields in declaration order, so stop providers and close SQLite before the guard.
    managed_runtimes: BTreeMap<String, managed_start::RetainedManagedRun>,
    managed_observations_in_flight: BTreeSet<String>,
    startup_recovery_sessions: Option<Vec<ManagedSession>>,
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

    fn take_startup_recovery(&self) -> Option<(String, String, Vec<ManagedSession>)> {
        let mut state = self.lock_state();
        let CoreState::Ready(core) = &mut *state else {
            return None;
        };
        let sessions = core.startup_recovery_sessions.take()?;
        if sessions.is_empty() {
            return None;
        }
        let observed_at = core.store.current_utc_timestamp().ok()?;
        Some((core.core_instance_id.clone(), observed_at, sessions))
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
        let mut store = Store::open_with_system_time(&database_path)
            .map_err(|_| BridgeError::StorageFailure)?;
        make_owner_only_file(&database_path)?;
        make_owner_only_file_if_present(&wal_path)?;
        make_owner_only_file_if_present(&shared_memory_path)?;
        let core_instance_id = core_instance_id(&canonical_data_directory);
        let startup_recovery_sessions = store
            .live_managed_sessions(MAX_LIVE_MANAGED_SESSIONS)
            .map_err(|_| BridgeError::StorageFailure)?;
        if !startup_recovery_sessions.is_empty() {
            let unknown_observations =
                unknown_codex_recovery_observations(startup_recovery_sessions.clone())
                    .map_err(|_| BridgeError::StorageFailure)?;
            let attempt = CodexRecoveryAttempt {
                id: format!("startup-gap-{core_instance_id}"),
                observed_at: store
                    .current_utc_timestamp()
                    .map_err(|_| BridgeError::StorageFailure)?,
            };
            persist_codex_recovery_observations(&mut store, &attempt, unknown_observations)
                .map_err(|_| BridgeError::StorageFailure)?;
        }

        Ok(Self {
            requested_data_directory,
            core_instance_id,
            canonical_data_directory,
            managed_runtimes: BTreeMap::new(),
            managed_observations_in_flight: BTreeSet::new(),
            startup_recovery_sessions: Some(startup_recovery_sessions),
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

fn core_instance_id(canonical_data_directory: &Path) -> String {
    let sequence = CORE_INSTANCE_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut digest = Sha256::new();
    digest.update(canonical_data_directory.to_string_lossy().as_bytes());
    digest.update(std::process::id().to_le_bytes());
    digest.update(sequence.to_le_bytes());
    digest.update(timestamp.to_le_bytes());
    format!("core-{:x}", digest.finalize())
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
        BridgeError::ProjectNotTrusted => CommandErrorCode::ProjectNotTrusted,
        BridgeError::ProjectIdentityMismatch => CommandErrorCode::ProjectIdentityMismatch,
        BridgeError::StorageFailure => CommandErrorCode::StorageUnavailable,
        BridgeError::CoreFailure
        | BridgeError::InvalidDataDirectory
        | BridgeError::CoreAlreadyInitialized
        | BridgeError::CoreAlreadyRunning
        | BridgeError::CoreLockFailure
        | BridgeError::ProjectResponseTooLarge
        | BridgeError::ProviderDiagnosticsResponseTooLarge
        | BridgeError::QuitImpactResponseTooLarge
        | BridgeError::InvalidDashboardRequest
        | BridgeError::DashboardResponseTooLarge
        | BridgeError::ManagedRunResponseTooLarge
        | BridgeError::InvalidRunRequest
        | BridgeError::RunNotFound
        | BridgeError::RunVersionStale
        | BridgeError::CapabilityUnsupported
        | BridgeError::ProviderUnavailable
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
        ProviderCapability::PermissionModeConfigure => {
            ProtocolProviderCapability::PermissionModeConfigure
        }
        ProviderCapability::ProviderOutcomeObserve => {
            ProtocolProviderCapability::ProviderOutcomeObserve
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
        let outcome = CORE.initialize(&data_directory)?;
        if outcome == InitializationOutcome::Initialized {
            CORE_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
            if let Some((core_instance_id, observed_at, sessions)) = CORE.take_startup_recovery() {
                spawn_startup_recovery(core_instance_id, observed_at, sessions);
            }
        }
        Ok(())
    })
}

fn spawn_startup_recovery(
    core_instance_id: String,
    observed_at: String,
    sessions: Vec<ManagedSession>,
) {
    let _ = thread::Builder::new()
        .name("flit-startup-recovery".to_owned())
        .spawn(move || {
            let mut connector = ExactCodexRecoveryConnector;
            let Ok(observations) = observe_codex_sessions(sessions, &mut connector) else {
                return;
            };
            let _ = CORE.with_ready_core(|core| {
                if core.core_instance_id != core_instance_id {
                    return Ok(());
                }
                let attempt = CodexRecoveryAttempt {
                    id: format!("startup-exact-{core_instance_id}"),
                    observed_at,
                };
                persist_codex_recovery_observations(&mut core.store, &attempt, observations)
                    .map_err(|_| BridgeError::StorageFailure)?;
                Ok(())
            });
        });
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

fn quit_impact_command_json(
    operation: impl FnOnce() -> Result<QuitImpactResponse, BridgeError>,
) -> Result<String, BridgeError> {
    match operation() {
        Ok(response) => bounded_json(
            &response,
            MAX_QUIT_IMPACT_RESPONSE_BYTES,
            BridgeError::QuitImpactResponseTooLarge,
        ),
        Err(error) => match project_command_error(&error) {
            Some(command_error) => bounded_json(
                &command_error,
                MAX_QUIT_IMPACT_RESPONSE_BYTES,
                BridgeError::QuitImpactResponseTooLarge,
            ),
            None => Err(error),
        },
    }
}

fn provider_execution_after_quit(
    session: &ManagedSession,
) -> (ProviderExecutionAfterQuit, QuitImpactReason) {
    match session.capabilities.get("continue_after_quit") {
        Some(serde_json::Value::String(status)) if status == "supported" => (
            ProviderExecutionAfterQuit::Continues,
            QuitImpactReason::CapabilitySupported,
        ),
        Some(serde_json::Value::String(status)) if status == "unsupported" => (
            ProviderExecutionAfterQuit::Stops,
            QuitImpactReason::CapabilityUnsupported,
        ),
        Some(serde_json::Value::String(status))
            if matches!(status.as_str(), "degraded" | "unknown" | "unavailable") =>
        {
            (
                ProviderExecutionAfterQuit::Unknown,
                QuitImpactReason::CapabilityUncertain,
            )
        }
        None => (
            ProviderExecutionAfterQuit::Unknown,
            QuitImpactReason::CapabilityMissing,
        ),
        Some(_) => (
            ProviderExecutionAfterQuit::Unknown,
            QuitImpactReason::CapabilityInvalid,
        ),
    }
}

#[uniffi::export]
pub fn quit_impact_json(client_protocol_version: String) -> Result<String, BridgeError> {
    protect(|| quit_impact_with(&CORE, &client_protocol_version))
}

fn quit_impact_with(
    core_manager: &CoreManager,
    client_protocol_version: &str,
) -> Result<String, BridgeError> {
    quit_impact_command_json(|| {
        validate_project_protocol(client_protocol_version)?;
        core_manager.with_ready_core(|core| {
            let cursor = core
                .store
                .latest_ingest_seq()
                .map_err(|_| BridgeError::StorageFailure)?;
            let sessions = core
                .store
                .complete_live_managed_sessions(MAX_LIVE_MANAGED_SESSIONS)
                .map_err(|_| BridgeError::StorageFailure)?;
            let runs = sessions
                .into_iter()
                .map(|session| {
                    let run = core
                        .store
                        .managed_run(&session.run_id)
                        .map_err(|_| BridgeError::StorageFailure)?
                        .ok_or(BridgeError::StorageFailure)?;
                    if run.ended_at.is_some()
                        || run.provider_kind != session.provider_kind
                        || session.provider_kind != "codex"
                    {
                        return Err(BridgeError::StorageFailure);
                    }
                    let (execution_after_quit, reason) = provider_execution_after_quit(&session);
                    Ok(QuitImpactRun {
                        run_id: run.id,
                        title: run.title,
                        provider: ProtocolProviderKind::Codex,
                        execution_after_quit,
                        reason,
                    })
                })
                .collect::<Result<Vec<_>, BridgeError>>()?;
            Ok(QuitImpactResponse {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                core_instance_id: core.core_instance_id.clone(),
                cursor,
                flit_monitoring_stops: true,
                flit_notifications_stop: true,
                runs,
            })
        })
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitProjectTarget {
    canonical_path: PathBuf,
    filesystem_id: String,
}

fn git_project_target(
    core_manager: &CoreManager,
    project_id: &str,
) -> Result<GitProjectTarget, BridgeError> {
    core_manager.with_ready_core(|core| {
        let project = core
            .store
            .project(project_id)
            .map_err(|_| BridgeError::StorageFailure)?
            .ok_or(BridgeError::ProjectNotFound)?;
        if !project.trusted {
            return Err(BridgeError::ProjectNotTrusted);
        }
        let filesystem_id = project
            .filesystem_id
            .ok_or(BridgeError::ProjectIdentityMismatch)?;
        let inspection = ProjectDirectoryInspection::inspect(&project.canonical_path)
            .map_err(|_| BridgeError::ProjectIdentityMismatch)?;
        if inspection.selected_via_symlink
            || inspection.identity.canonical_path != project.canonical_path
            || inspection.identity.filesystem_id != filesystem_id
        {
            return Err(BridgeError::ProjectIdentityMismatch);
        }
        Ok(GitProjectTarget {
            canonical_path: project.canonical_path,
            filesystem_id,
        })
    })
}

fn git_observation_with(
    core_manager: &CoreManager,
    project_id: &str,
    observe: impl FnOnce(&Path) -> Result<NativeGitObservation, GitObservationError>,
) -> Result<GitObservationResponse, BridgeError> {
    let before = git_project_target(core_manager, project_id)?;
    let observation = observe(&before.canonical_path);
    let after = git_project_target(core_manager, project_id)?;
    if after != before {
        return Err(BridgeError::ProjectIdentityMismatch);
    }
    Ok(protocol_git_observation(project_id, observation))
}

fn protocol_git_observation(
    project_id: &str,
    observation: Result<NativeGitObservation, GitObservationError>,
) -> GitObservationResponse {
    match observation {
        Ok(NativeGitObservation::NotWorktree(reason)) => GitObservationResponse::NotWorktree {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            project_id: project_id.to_owned(),
            reason: match reason {
                NativeNotWorktreeReason::NotRepository => GitNotWorktreeReason::NotRepository,
                NativeNotWorktreeReason::BareRepository => GitNotWorktreeReason::BareRepository,
            },
        },
        Ok(NativeGitObservation::Repository(receipt)) => {
            let Ok(canonical_root) = receipt.canonical_root.into_os_string().into_string() else {
                return git_unavailable(
                    project_id,
                    GitObservationUnavailableReason::MalformedOutput,
                );
            };
            GitObservationResponse::Repository {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                project_id: project_id.to_owned(),
                canonical_root,
                head: match receipt.head {
                    NativeGitHead::Available(oid) => GitHead::Available { oid },
                    NativeGitHead::Unborn => GitHead::Unborn,
                },
                dirty: GitDirtySummary {
                    staged: receipt.dirty.staged,
                    unstaged: receipt.dirty.unstaged,
                    untracked: receipt.dirty.untracked,
                    entries: receipt.dirty.entries,
                },
            }
        }
        Err(error) => git_unavailable(project_id, git_unavailable_reason(&error)),
    }
}

fn managed_git_baseline(
    project_id: &str,
    observation: Result<NativeGitObservation, GitObservationError>,
) -> Result<GitBaselinePayload, BridgeError> {
    match observation {
        Ok(NativeGitObservation::NotWorktree(reason)) => Ok(GitBaselinePayload::Unavailable {
            project_id: project_id.to_owned(),
            reason: match reason {
                NativeNotWorktreeReason::NotRepository => {
                    GitBaselineUnavailableReason::NotRepository
                }
                NativeNotWorktreeReason::BareRepository => {
                    GitBaselineUnavailableReason::BareRepository
                }
            },
        }),
        Ok(NativeGitObservation::Repository(receipt)) => Ok(GitBaselinePayload::Available {
            project_id: project_id.to_owned(),
            head: match receipt.head {
                NativeGitHead::Available(oid) => GitHead::Available { oid },
                NativeGitHead::Unborn => GitHead::Unborn,
            },
            dirty: GitDirtySummary {
                staged: receipt.dirty.staged,
                unstaged: receipt.dirty.unstaged,
                untracked: receipt.dirty.untracked,
                entries: receipt.dirty.entries,
            },
        }),
        Err(error) => {
            let reason = git_unavailable_reason(&error);
            if reason == GitObservationUnavailableReason::ProjectChanged {
                return Err(BridgeError::ProjectIdentityMismatch);
            }
            Ok(GitBaselinePayload::Unavailable {
                project_id: project_id.to_owned(),
                reason: match reason {
                    GitObservationUnavailableReason::RunnerUnavailable => {
                        GitBaselineUnavailableReason::RunnerUnavailable
                    }
                    GitObservationUnavailableReason::GitUnavailable => {
                        GitBaselineUnavailableReason::GitUnavailable
                    }
                    GitObservationUnavailableReason::ProjectChanged => {
                        unreachable!("Project drift returns before baseline construction")
                    }
                    GitObservationUnavailableReason::ProcessUnavailable => {
                        GitBaselineUnavailableReason::ProcessUnavailable
                    }
                    GitObservationUnavailableReason::MalformedOutput => {
                        GitBaselineUnavailableReason::MalformedOutput
                    }
                },
            })
        }
    }
}

fn git_unavailable(
    project_id: &str,
    reason: GitObservationUnavailableReason,
) -> GitObservationResponse {
    GitObservationResponse::Unavailable {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        project_id: project_id.to_owned(),
        reason,
    }
}

fn git_unavailable_reason(error: &GitObservationError) -> GitObservationUnavailableReason {
    match error {
        GitObservationError::RunnerPathNotAbsolute
        | GitObservationError::RunnerUnavailable
        | GitObservationError::RunnerChanged
        | GitObservationError::RunnerBoundaryFailed { .. } => {
            GitObservationUnavailableReason::RunnerUnavailable
        }
        GitObservationError::GitNotFound | GitObservationError::GitExecutableChanged => {
            GitObservationUnavailableReason::GitUnavailable
        }
        GitObservationError::ProjectDirectoryUnavailable
        | GitObservationError::ProjectDirectoryNotCanonical
        | GitObservationError::ProjectDirectoryChanged
        | GitObservationError::RepositoryRootChanged
        | GitObservationError::RepositoryRootMismatch => {
            GitObservationUnavailableReason::ProjectChanged
        }
        GitObservationError::CommandSpawnFailed { .. }
        | GitObservationError::CommandIoFailed { .. }
        | GitObservationError::CommandTimedOut { .. }
        | GitObservationError::CommandOutputDrainTimedOut { .. }
        | GitObservationError::CommandOutputTooLarge { .. }
        | GitObservationError::CommandFailed { .. }
        | GitObservationError::UnexpectedCommandStderr { .. } => {
            GitObservationUnavailableReason::ProcessUnavailable
        }
        GitObservationError::MalformedRepositoryRoot
        | GitObservationError::MalformedPorcelain
        | GitObservationError::DuplicatePorcelainRecord
        | GitObservationError::TooManyPorcelainEntries
        | GitObservationError::GitPathTooLong => GitObservationUnavailableReason::MalformedOutput,
    }
}

fn bundled_git_runner_path() -> Result<PathBuf, GitObservationError> {
    let app_executable =
        std::env::current_exe().map_err(|_| GitObservationError::RunnerUnavailable)?;
    bundled_git_runner_path_for(&app_executable)
}

fn bundled_git_runner_path_for(app_executable: &Path) -> Result<PathBuf, GitObservationError> {
    if !app_executable.is_absolute()
        || app_executable.file_name().and_then(|name| name.to_str()) != Some("Flit")
    {
        return Err(GitObservationError::RunnerUnavailable);
    }
    let canonical_app =
        fs::canonicalize(app_executable).map_err(|_| GitObservationError::RunnerUnavailable)?;
    if canonical_app != app_executable {
        return Err(GitObservationError::RunnerUnavailable);
    }
    let macos = canonical_app
        .parent()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("MacOS"))
        .ok_or(GitObservationError::RunnerUnavailable)?;
    let contents = macos
        .parent()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("Contents"))
        .ok_or(GitObservationError::RunnerUnavailable)?;
    if contents
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        != Some("Flit.app")
    {
        return Err(GitObservationError::RunnerUnavailable);
    }
    let runner = contents.join("Helpers/flit-git-noexec");
    let metadata =
        fs::symlink_metadata(&runner).map_err(|_| GitObservationError::RunnerUnavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(GitObservationError::RunnerUnavailable);
    }
    let canonical_runner =
        fs::canonicalize(&runner).map_err(|_| GitObservationError::RunnerUnavailable)?;
    if canonical_runner != runner {
        return Err(GitObservationError::RunnerUnavailable);
    }
    Ok(runner)
}

fn observe_bundled_project(
    canonical_project_directory: &Path,
) -> Result<NativeGitObservation, GitObservationError> {
    let runner = inspect_noexec_runner_at(bundled_git_runner_path()?)?;
    let git = inspect_git_on_path(std::env::var_os("PATH").as_deref())?;
    observe_repository(&runner, &git, canonical_project_directory)
}

#[uniffi::export]
pub fn git_observe_project_json(
    project_id: String,
    client_protocol_version: String,
) -> Result<String, BridgeError> {
    protect(|| {
        project_command_json(|| {
            validate_project_protocol(&client_protocol_version)?;
            validate_project_input(&project_id, MAX_PROJECT_ID_BYTES)?;
            git_observation_with(&CORE, &project_id, observe_bundled_project)
        })
    })
}

fn dashboard_command_error(error: &BridgeError) -> Option<CommandError> {
    let code = match error {
        BridgeError::ProtocolMismatch => CommandErrorCode::ProtocolMismatch,
        BridgeError::InvalidDashboardRequest => CommandErrorCode::InvalidDashboardRequest,
        BridgeError::StorageFailure => CommandErrorCode::StorageUnavailable,
        _ => return None,
    };
    Some(CommandError::for_code(code))
}

fn dashboard_command_json(
    operation: impl FnOnce() -> Result<DashboardReadResponse, BridgeError>,
) -> Result<String, BridgeError> {
    match operation() {
        Ok(response) => bounded_json(
            &response,
            MAX_DASHBOARD_RESPONSE_BYTES,
            BridgeError::DashboardResponseTooLarge,
        ),
        Err(error) => match dashboard_command_error(&error) {
            Some(command_error) => bounded_json(
                &command_error,
                MAX_DASHBOARD_RESPONSE_BYTES,
                BridgeError::DashboardResponseTooLarge,
            ),
            None => Err(error),
        },
    }
}

fn dashboard_run_record(
    snapshot: StoreDashboardRunSnapshot,
) -> Result<DashboardRunRecord, BridgeError> {
    let provider = match snapshot.provider_kind.as_str() {
        "codex" => ProtocolProviderKind::Codex,
        _ => return Err(BridgeError::StorageFailure),
    };
    let changes = match snapshot.changes {
        StoreDashboardChangeSummary::Available {
            files,
            insertions,
            deletions,
        } => flit_protocol::DashboardChangeSummary::Available {
            files,
            insertions,
            deletions,
        },
        StoreDashboardChangeSummary::Unavailable { reason } => {
            flit_protocol::DashboardChangeSummary::Unavailable { reason }
        }
    };
    Ok(DashboardRunRecord {
        run_id: snapshot.projection.run_id,
        project_id: snapshot.project_id,
        project_display_name: snapshot.project_display_name,
        title: snapshot.title,
        provider,
        version: snapshot.projection.version,
        lifecycle: snapshot.projection.lifecycle,
        activity: snapshot.projection.activity,
        activity_confidence: snapshot.projection.activity_confidence,
        attention_level: snapshot.projection.attention_level,
        attention_open_count: snapshot.attention_open_count,
        dashboard_bucket: snapshot.projection.dashboard_bucket,
        last_progress_at: snapshot.projection.last_progress_at,
        last_liveness_at: snapshot.projection.last_liveness_at,
        started_at: snapshot.started_at,
        ended_at: snapshot.ended_at,
        changes,
        updated_at: snapshot.projection.updated_at,
    })
}

fn dashboard_snapshot_response(
    core: &FoundationCore,
    reason: DashboardSnapshotReason,
    requested_after_cursor: Option<u64>,
    latest_cursor: u64,
    retained_after_cursor: u64,
) -> Result<DashboardReadResponse, BridgeError> {
    let runs = core
        .store
        .dashboard_run_snapshots_through(latest_cursor)
        .map_err(|error| match error {
            StoreError::DashboardSnapshotReadTooLarge { .. } => {
                BridgeError::DashboardResponseTooLarge
            }
            _ => BridgeError::StorageFailure,
        })?
        .into_iter()
        .map(dashboard_run_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashboardReadResponse::Snapshot {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        event_schema_version: EVENT_PROTOCOL_VERSION.to_owned(),
        core_instance_id: core.core_instance_id.clone(),
        reason,
        requested_after_cursor,
        retained_after_cursor,
        next_cursor: latest_cursor,
        has_more: false,
        runs,
    })
}

fn dashboard_resync_reason(
    expected_core_instance_id: &str,
    actual_core_instance_id: &str,
    after_cursor: u64,
    latest_cursor: u64,
    retained_after_cursor: u64,
) -> Option<DashboardSnapshotReason> {
    if expected_core_instance_id != actual_core_instance_id {
        Some(DashboardSnapshotReason::CoreInstanceMismatch)
    } else if after_cursor > latest_cursor {
        Some(DashboardSnapshotReason::CursorAhead)
    } else if after_cursor < retained_after_cursor {
        Some(DashboardSnapshotReason::CursorExpired)
    } else {
        None
    }
}

fn dashboard_read_response(
    core: &FoundationCore,
    request: DashboardReadRequest,
) -> Result<DashboardReadResponse, BridgeError> {
    if request.client_protocol_version != PROTOCOL_VERSION {
        return Err(BridgeError::ProtocolMismatch);
    }
    let requested_event_limit = usize::try_from(request.requested_event_limit)
        .map_err(|_| BridgeError::InvalidDashboardRequest)?;
    if !(1..=MAX_DASHBOARD_DELTA_EVENTS).contains(&requested_event_limit) {
        return Err(BridgeError::InvalidDashboardRequest);
    }
    let latest_cursor = core
        .store
        .latest_ingest_seq()
        .map_err(|_| BridgeError::StorageFailure)?;
    let retained_after_cursor = latest_cursor.saturating_sub(DASHBOARD_DELTA_RETENTION_EVENTS);

    let (expected_core_instance_id, after_cursor) =
        match (request.expected_core_instance_id, request.after_cursor) {
            (None, None) => {
                return dashboard_snapshot_response(
                    core,
                    DashboardSnapshotReason::Initial,
                    None,
                    latest_cursor,
                    retained_after_cursor,
                );
            }
            (Some(expected_core_instance_id), Some(after_cursor))
                if !expected_core_instance_id.trim().is_empty()
                    && expected_core_instance_id.len() <= MAX_CORE_INSTANCE_ID_BYTES
                    && !expected_core_instance_id.contains('\0')
                    && after_cursor <= flit_protocol::MAX_JSON_SAFE_INTEGER =>
            {
                (expected_core_instance_id, after_cursor)
            }
            _ => return Err(BridgeError::InvalidDashboardRequest),
        };

    let resync_reason = dashboard_resync_reason(
        &expected_core_instance_id,
        &core.core_instance_id,
        after_cursor,
        latest_cursor,
        retained_after_cursor,
    );
    if let Some(reason) = resync_reason {
        return dashboard_snapshot_response(
            core,
            reason,
            Some(after_cursor),
            latest_cursor,
            retained_after_cursor,
        );
    }

    let page = core
        .store
        .dashboard_event_locators_through(after_cursor, latest_cursor, requested_event_limit)
        .map_err(|_| BridgeError::StorageFailure)?;
    let changed_run_ids = page
        .events
        .iter()
        .map(|event| event.run_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let events = page
        .events
        .into_iter()
        .map(|event| DashboardEventRecord {
            cursor: event.cursor,
            event_id: event.event_id,
            run_id: event.run_id,
            event_type: event.event_type,
            observed_at: event.observed_at,
        })
        .collect::<Vec<_>>();
    let next_cursor = events.last().map_or(after_cursor, |event| event.cursor);
    let runs = core
        .store
        .dashboard_run_snapshots_for_delta(&changed_run_ids, after_cursor, next_cursor)
        .map_err(|error| match error {
            StoreError::DashboardSnapshotReadTooLarge { .. } => {
                BridgeError::DashboardResponseTooLarge
            }
            _ => BridgeError::StorageFailure,
        })?
        .into_iter()
        .map(dashboard_run_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DashboardReadResponse::Delta {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        event_schema_version: EVENT_PROTOCOL_VERSION.to_owned(),
        core_instance_id: core.core_instance_id.clone(),
        requested_after_cursor: after_cursor,
        retained_after_cursor,
        next_cursor,
        has_more: next_cursor < page.upper_bound,
        events,
        runs,
    })
}

fn dashboard_read_with(
    core_manager: &CoreManager,
    request_json: &str,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_DASHBOARD_REQUEST_BYTES {
        return dashboard_command_json(|| Err(BridgeError::InvalidDashboardRequest));
    }
    let request = serde_json::from_str::<DashboardReadRequest>(request_json)
        .map_err(|_| BridgeError::InvalidDashboardRequest);
    dashboard_command_json(|| {
        let request = request?;
        core_manager.with_ready_core(|core| dashboard_read_response(core, request))
    })
}

#[uniffi::export]
pub fn dashboard_read_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| dashboard_read_with(&CORE, &request_json))
}

fn run_command_error(error: &BridgeError) -> Option<CommandError> {
    let code = match error {
        BridgeError::ProtocolMismatch => CommandErrorCode::ProtocolMismatch,
        BridgeError::InvalidRunRequest => CommandErrorCode::InvalidRunRequest,
        BridgeError::RunNotFound => CommandErrorCode::RunNotFound,
        BridgeError::RunVersionStale => CommandErrorCode::RunVersionStale,
        BridgeError::CapabilityUnsupported => CommandErrorCode::CapabilityUnsupported,
        BridgeError::ProviderUnavailable => CommandErrorCode::ProviderUnavailable,
        BridgeError::StorageFailure => CommandErrorCode::StorageUnavailable,
        _ => return None,
    };
    Some(CommandError::for_code(code))
}

fn run_command_json<T: serde::Serialize>(
    operation: impl FnOnce() -> Result<T, BridgeError>,
) -> Result<String, BridgeError> {
    match operation() {
        Ok(response) => bounded_json(
            &response,
            MAX_RUN_DETAIL_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(error) => match run_command_error(&error) {
            Some(command_error) => bounded_json(
                &command_error,
                MAX_RUN_DETAIL_RESPONSE_BYTES,
                BridgeError::ManagedRunResponseTooLarge,
            ),
            None => Err(error),
        },
    }
}

fn stored_capability_status(status: &str) -> Result<ProtocolCapabilityStatus, BridgeError> {
    match status {
        "supported" => Ok(ProtocolCapabilityStatus::Supported),
        "degraded" => Ok(ProtocolCapabilityStatus::Degraded),
        "unsupported" => Ok(ProtocolCapabilityStatus::Unsupported),
        "unknown" => Ok(ProtocolCapabilityStatus::Unknown),
        "unavailable" => Ok(ProtocolCapabilityStatus::Unavailable),
        _ => Err(BridgeError::StorageFailure),
    }
}

fn evidence_source_kind(kind: &str) -> Result<EventSourceKind, BridgeError> {
    match kind {
        "core" => Ok(EventSourceKind::Core),
        "provider_adapter" => Ok(EventSourceKind::ProviderAdapter),
        "git_watcher" => Ok(EventSourceKind::GitWatcher),
        "file_watcher" => Ok(EventSourceKind::FileWatcher),
        "classifier" => Ok(EventSourceKind::Classifier),
        "policy" => Ok(EventSourceKind::Policy),
        "ui" => Ok(EventSourceKind::Ui),
        "notifier" => Ok(EventSourceKind::Notifier),
        "recovery" => Ok(EventSourceKind::Recovery),
        _ => Err(BridgeError::StorageFailure),
    }
}

fn provider_open_error(status: ProtocolCapabilityStatus) -> BridgeError {
    match status {
        ProtocolCapabilityStatus::Unsupported => BridgeError::CapabilityUnsupported,
        ProtocolCapabilityStatus::Supported
        | ProtocolCapabilityStatus::Degraded
        | ProtocolCapabilityStatus::Unknown
        | ProtocolCapabilityStatus::Unavailable => BridgeError::ProviderUnavailable,
    }
}

fn validate_run_detail_request(request: &RunDetailReadRequest) -> Result<usize, BridgeError> {
    let limit = usize::try_from(request.requested_event_limit)
        .map_err(|_| BridgeError::InvalidRunRequest)?;
    if request.run_id.trim().is_empty()
        || request.run_id.len() > MAX_PROJECT_ID_BYTES
        || request.run_id.contains('\0')
        || request.expected_run_version == 0
        || request.expected_run_version > flit_protocol::MAX_JSON_SAFE_INTEGER
        || request.after_cursor > request.expected_run_version
        || !(1..=MAX_RUN_DETAIL_EVENTS).contains(&limit)
    {
        return Err(BridgeError::InvalidRunRequest);
    }
    Ok(limit)
}

fn run_detail_read_with(
    core_manager: &CoreManager,
    request_json: &str,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return run_command_json(|| -> Result<RunDetailReadResponse, _> {
            Err(BridgeError::InvalidRunRequest)
        });
    }
    let request = serde_json::from_str::<RunDetailReadRequest>(request_json)
        .map_err(|_| BridgeError::InvalidRunRequest);
    run_command_json(|| {
        let request = request?;
        if request.client_protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::ProtocolMismatch);
        }
        let limit = validate_run_detail_request(&request)?;
        core_manager.with_ready_core(|core| {
            let context = core
                .store
                .managed_run_detail_context(&request.run_id)
                .map_err(|error| match error {
                    StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                    _ => BridgeError::StorageFailure,
                })?;
            if context.run_version != request.expected_run_version {
                return Err(BridgeError::RunVersionStale);
            }
            let page = core
                .store
                .run_evidence_through(
                    &request.run_id,
                    request.after_cursor,
                    request.expected_run_version,
                    limit,
                )
                .map_err(|error| match error {
                    StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                    StoreError::RunDetailReadTooLarge { .. } => {
                        BridgeError::ManagedRunResponseTooLarge
                    }
                    _ => BridgeError::StorageFailure,
                })?;
            let events = page
                .events
                .into_iter()
                .map(|event| {
                    Ok(RunEvidenceRecord {
                        cursor: event.cursor,
                        event_id: event.event_id,
                        session_id: event.session_id,
                        event_type: event.event_type,
                        source_kind: evidence_source_kind(&event.source_kind)?,
                        confidence: event.confidence,
                        observed_at: event.observed_at,
                    })
                })
                .collect::<Result<Vec<_>, BridgeError>>()?;
            let next_cursor = events
                .last()
                .map_or(request.after_cursor, |event| event.cursor);
            Ok(RunDetailReadResponse {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                event_schema_version: EVENT_PROTOCOL_VERSION.to_owned(),
                run_id: request.run_id,
                run_version: context.run_version,
                next_cursor,
                has_more: page.has_more,
                history_status: stored_capability_status(&context.history_status)?,
                open_in_provider_status: stored_capability_status(
                    &context.open_in_provider_status,
                )?,
                events,
            })
        })
    })
}

#[uniffi::export]
pub fn run_detail_read_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| run_detail_read_with(&CORE, &request_json))
}

fn managed_run_open_in_provider_with(
    core_manager: &CoreManager,
    request_json: &str,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return run_command_json(|| -> Result<serde_json::Value, _> {
            Err(BridgeError::InvalidRunRequest)
        });
    }
    let request = serde_json::from_str::<ManagedRunOpenInProviderRequest>(request_json)
        .map_err(|_| BridgeError::InvalidRunRequest);
    run_command_json(|| -> Result<serde_json::Value, BridgeError> {
        let request = request?;
        if request.client_protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::ProtocolMismatch);
        }
        if request.run_id.trim().is_empty()
            || request.run_id.len() > MAX_PROJECT_ID_BYTES
            || request.run_id.contains('\0')
            || request.expected_run_version == 0
            || request.expected_run_version > flit_protocol::MAX_JSON_SAFE_INTEGER
        {
            return Err(BridgeError::InvalidRunRequest);
        }
        core_manager.with_ready_core(|core| {
            let context = core
                .store
                .managed_run_detail_context(&request.run_id)
                .map_err(|error| match error {
                    StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                    _ => BridgeError::StorageFailure,
                })?;
            if context.run_version != request.expected_run_version {
                return Err(BridgeError::RunVersionStale);
            }
            Err(provider_open_error(stored_capability_status(
                &context.open_in_provider_status,
            )?))
        })
    })
}

#[uniffi::export]
pub fn managed_run_open_in_provider_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| managed_run_open_in_provider_with(&CORE, &request_json))
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
            let path_environment = std::env::var_os("PATH");
            managed_run_start_with(
                &CORE,
                &managed_start::ProductionCodexConnector,
                path_environment.as_deref(),
                request,
                observe_bundled_project,
            )
        });
        match result {
            Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
                CommandErrorCode::StorageUnavailable,
            )),
            result => result,
        }
    })
}

fn managed_run_start_with(
    core_manager: &CoreManager,
    connector: &dyn managed_start::ManagedCodexConnector,
    path_environment: Option<&std::ffi::OsStr>,
    request: ManagedRunStartRequest,
    observe: impl FnOnce(&Path) -> Result<NativeGitObservation, GitObservationError>,
) -> Result<String, BridgeError> {
    if let Err(error) = managed_start::validate_request(&request) {
        return project_json(&CommandError::for_code(managed_start_error_code(error)));
    }
    let cached = core_manager.with_ready_core(|core| {
        if core
            .managed_observations_in_flight
            .contains(&request.run_id)
        {
            return Ok(Some(project_json(&CommandError::for_code(
                CommandErrorCode::ProviderObservationUnknown,
            ))?));
        }
        match managed_start::cached_start_response(&core.managed_runtimes, &request) {
            Ok(Some(response)) => Ok(Some(bounded_json(
                &response,
                MAX_PROJECT_RESPONSE_BYTES,
                BridgeError::ManagedRunResponseTooLarge,
            )?)),
            Ok(None) => Ok(None),
            Err(error) => Ok(Some(project_json(&CommandError::for_code(
                managed_start_error_code(error),
            ))?)),
        }
    })?;
    if let Some(cached) = cached {
        return Ok(cached);
    }

    let before = match git_project_target(core_manager, &request.project_id) {
        Ok(target) => target,
        Err(error) => return managed_start_bridge_error_json(error),
    };
    let baseline = match managed_git_baseline(&request.project_id, observe(&before.canonical_path))
    {
        Ok(baseline) => baseline,
        Err(error) => return managed_start_bridge_error_json(error),
    };
    let after = match git_project_target(core_manager, &request.project_id) {
        Ok(target) => target,
        Err(error) => return managed_start_bridge_error_json(error),
    };
    if after != before {
        return managed_start_bridge_error_json(BridgeError::ProjectIdentityMismatch);
    }

    core_manager.with_ready_core(|core| {
        start_managed_run_in_core(core, connector, path_environment, baseline, request)
    })
}

fn managed_start_bridge_error_json(error: BridgeError) -> Result<String, BridgeError> {
    match project_command_error(&error) {
        Some(command_error) => project_json(&command_error),
        None => Err(error),
    }
}

fn start_managed_run_in_core(
    core: &mut FoundationCore,
    connector: &dyn managed_start::ManagedCodexConnector,
    path_environment: Option<&std::ffi::OsStr>,
    git_baseline: GitBaselinePayload,
    request: ManagedRunStartRequest,
) -> Result<String, BridgeError> {
    if core
        .managed_observations_in_flight
        .contains(&request.run_id)
    {
        return project_json(&CommandError::for_code(
            CommandErrorCode::ProviderObservationUnknown,
        ));
    }
    match managed_start::start_managed_run_with_baseline(
        &mut core.store,
        &mut core.managed_runtimes,
        connector,
        path_environment,
        git_baseline,
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
}

enum TakenManagedRuntime {
    Cached(Box<ManagedRunObserveResponse>),
    Runtime(Box<managed_start::RetainedManagedRun>),
    Error(managed_start::ManagedStartError),
}

struct ManagedObservationFlight<'a> {
    core_manager: &'a CoreManager,
    run_id: String,
}

impl Drop for ManagedObservationFlight<'_> {
    fn drop(&mut self) {
        let _ = self.core_manager.with_ready_core(|core| {
            core.managed_observations_in_flight.remove(&self.run_id);
            Ok(())
        });
    }
}

#[uniffi::export]
pub fn managed_run_observe_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| {
        managed_run_observe_with(
            &CORE,
            request_json,
            managed_start::wait_managed_observation,
            managed_start::commit_managed_observation,
        )
    })
}

fn managed_run_observe_with<Wait, Commit>(
    core_manager: &CoreManager,
    request_json: String,
    mut wait: Wait,
    mut commit: Commit,
) -> Result<String, BridgeError>
where
    Wait: FnMut(
        &mut managed_start::RetainedManagedRun,
    )
        -> Result<flit_providers::CodexTurnObservation, managed_start::ManagedStartError>,
    Commit:
        FnMut(
            &mut Store,
            &mut managed_start::RetainedManagedRun,
            &ManagedRunObserveRequest,
            flit_providers::CodexTurnObservation,
        )
            -> Result<managed_start::ManagedObservationCommit, managed_start::ManagedStartError>,
{
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request = match serde_json::from_str::<ManagedRunObserveRequest>(&request_json) {
        Ok(request) => request,
        Err(_) => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    if let Err(error) = managed_start::validate_observe_request(&request) {
        return project_json(&CommandError::for_code(managed_start_error_code(error)));
    }

    let taken = match core_manager.with_ready_core(|core| {
        if core
            .managed_observations_in_flight
            .contains(&request.run_id)
        {
            return Ok(TakenManagedRuntime::Error(
                managed_start::ManagedStartError::ProviderObservationUnknown,
            ));
        }
        let Some(runtime) = core.managed_runtimes.remove(&request.run_id) else {
            return Ok(TakenManagedRuntime::Error(
                managed_start::ManagedStartError::RunNotActive,
            ));
        };
        if let Some(permission) = runtime.cached_permission() {
            core.managed_runtimes
                .insert(request.run_id.clone(), runtime);
            return Ok(TakenManagedRuntime::Cached(Box::new(permission)));
        }
        core.managed_observations_in_flight
            .insert(request.run_id.clone());
        Ok(TakenManagedRuntime::Runtime(Box::new(runtime)))
    }) {
        Ok(taken) => taken,
        Err(BridgeError::StorageFailure) => {
            return project_json(&CommandError::for_code(
                CommandErrorCode::StorageUnavailable,
            ));
        }
        Err(error) => return Err(error),
    };

    let mut runtime = match taken {
        TakenManagedRuntime::Cached(response) => {
            return bounded_json(
                response.as_ref(),
                MAX_PROJECT_RESPONSE_BYTES,
                BridgeError::ManagedRunResponseTooLarge,
            );
        }
        TakenManagedRuntime::Runtime(runtime) => *runtime,
        TakenManagedRuntime::Error(error) => {
            return project_json(&CommandError::for_code(managed_start_error_code(error)));
        }
    };
    let _flight_guard = ManagedObservationFlight {
        core_manager,
        run_id: request.run_id.clone(),
    };

    for _ in 0..=MAX_CODEX_COMMAND_STARTS_PER_TURN {
        let observation = match wait(&mut runtime) {
            Ok(observation) => observation,
            Err(error) => {
                return finish_managed_observation_unknown(core_manager, request, runtime, error);
            }
        };
        let committed = core_manager.with_ready_core(|core| {
            if !core
                .managed_observations_in_flight
                .contains(&request.run_id)
            {
                return Err(BridgeError::CoreFailure);
            }
            Ok(commit(&mut core.store, &mut runtime, &request, observation))
        });
        match committed {
            Ok(Ok(managed_start::ManagedObservationCommit::Continue)) => {}
            Ok(Ok(managed_start::ManagedObservationCommit::Complete(response))) => {
                let retain_runtime = matches!(
                    response.as_ref(),
                    ManagedRunObserveResponse::PermissionRequested { .. }
                        | ManagedRunObserveResponse::ProviderOutcomeResolved { .. }
                );
                core_manager.with_ready_core(|core| {
                    core.managed_observations_in_flight.remove(&request.run_id);
                    if retain_runtime {
                        core.managed_runtimes
                            .insert(request.run_id.clone(), runtime);
                    }
                    Ok(())
                })?;
                return bounded_json(
                    response.as_ref(),
                    MAX_PROJECT_RESPONSE_BYTES,
                    BridgeError::ManagedRunResponseTooLarge,
                );
            }
            Ok(Err(error)) => {
                if error == managed_start::ManagedStartError::ProviderObservationUnknown {
                    return finish_managed_observation_unknown(
                        core_manager,
                        request,
                        runtime,
                        error,
                    );
                }
                core_manager.with_ready_core(|core| {
                    core.managed_observations_in_flight.remove(&request.run_id);
                    Ok(())
                })?;
                return project_json(&CommandError::for_code(managed_start_error_code(error)));
            }
            Err(error) => return Err(error),
        }
    }
    finish_managed_observation_unknown(
        core_manager,
        request,
        runtime,
        managed_start::ManagedStartError::ProviderObservationUnknown,
    )
}

fn finish_managed_observation_unknown(
    core_manager: &CoreManager,
    request: ManagedRunObserveRequest,
    runtime: managed_start::RetainedManagedRun,
    error: managed_start::ManagedStartError,
) -> Result<String, BridgeError> {
    let start_response = runtime.start_response();
    let contract_version = runtime.contract_version().to_owned();
    let recorded = core_manager.with_ready_core(|core| {
        core.managed_observations_in_flight.remove(&request.run_id);
        Ok(managed_start::record_observation_unknown(
            &mut core.store,
            &request,
            &start_response,
            &contract_version,
        ))
    })?;
    drop(runtime);
    match recorded {
        Ok(()) => project_json(&CommandError::for_code(managed_start_error_code(error))),
        Err(storage) => project_json(&CommandError::for_code(managed_start_error_code(storage))),
    }
}

enum TakenPermissionResponse {
    Provider(Box<PermissionProviderFlight>),
    Complete(ManagedRunPermissionRespondResponse),
    Error(managed_start::ManagedStartError),
}

struct PermissionProviderFlight {
    runtime: managed_start::RetainedManagedRun,
    prepared: managed_start::PreparedPermissionResponse,
    submitted: flit_protocol::EventEnvelope,
}

#[uniffi::export]
pub fn managed_run_permission_respond_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| {
        managed_run_permission_respond_with(&CORE, request_json, |runtime, decision| {
            runtime.respond_to_active_permission(decision)
        })
    })
}

fn managed_run_permission_respond_with<Respond>(
    core_manager: &CoreManager,
    request_json: String,
    mut respond: Respond,
) -> Result<String, BridgeError>
where
    Respond: FnMut(
        &mut managed_start::RetainedManagedRun,
        flit_providers::CodexPermissionDecision,
    ) -> Result<flit_providers::CodexPermissionDelivery, ()>,
{
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request = match serde_json::from_str::<ManagedRunPermissionRespondRequest>(&request_json) {
        Ok(request) => request,
        Err(_) => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    if let Err(error) = managed_start::validate_permission_response_request(&request) {
        return project_json(&CommandError::for_code(managed_start_error_code(error)));
    }

    let taken = core_manager.with_ready_core(|core| {
        if core
            .managed_observations_in_flight
            .contains(&request.run_id)
        {
            return Ok(TakenPermissionResponse::Error(
                managed_start::ManagedStartError::ProviderObservationUnknown,
            ));
        }
        let Some(runtime) = core.managed_runtimes.remove(&request.run_id) else {
            return Ok(match recover_pending_permission_response(core, &request) {
                Ok(Some(response)) => TakenPermissionResponse::Complete(response),
                Ok(None) => {
                    TakenPermissionResponse::Error(managed_start::ManagedStartError::RunNotActive)
                }
                Err(error) => TakenPermissionResponse::Error(error),
            });
        };
        let prepared = match managed_start::prepare_permission_response(&runtime, &request) {
            Ok(prepared) => prepared,
            Err(error) => {
                core.managed_runtimes
                    .insert(request.run_id.clone(), runtime);
                return Ok(TakenPermissionResponse::Error(error));
            }
        };
        let attempt = core
            .store
            .submit_managed_permission_response(prepared.attempt.clone())
            .map_err(managed_start::map_permission_store_error);
        match attempt {
            Ok(ManagedPermissionResponseAttemptOutcome::Submitted { event }) => {
                core.managed_observations_in_flight
                    .insert(request.run_id.clone());
                Ok(TakenPermissionResponse::Provider(Box::new(
                    PermissionProviderFlight {
                        runtime,
                        prepared,
                        submitted: event,
                    },
                )))
            }
            Ok(ManagedPermissionResponseAttemptOutcome::Duplicate {
                event,
                terminal_event: Some(terminal),
            }) => {
                let response = permission_response_from_events(&request, &event, &terminal)?;
                if terminal.event_type == "permission.resolved" {
                    let mut runtime = runtime;
                    runtime.clear_active_permission();
                    core.managed_runtimes
                        .insert(request.run_id.clone(), runtime);
                }
                Ok(TakenPermissionResponse::Complete(response))
            }
            Ok(ManagedPermissionResponseAttemptOutcome::Duplicate {
                event,
                terminal_event: None,
            }) => {
                let result = managed_start::permission_response_result(
                    &prepared,
                    &request,
                    ManagedPermissionResponseResultKind::DeliveryUnknown(
                        ManagedPermissionDeliveryUnknownReason::CoreRestartedAfterSubmit,
                    ),
                );
                let outcome = match core.store.finish_managed_permission_response(result) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Ok(TakenPermissionResponse::Error(
                            managed_start::map_permission_store_error(error),
                        ));
                    }
                };
                let outcome = appended_event(&outcome);
                Ok(TakenPermissionResponse::Complete(
                    permission_response_from_events(&request, &event, outcome)?,
                ))
            }
            Err(error) => {
                core.managed_runtimes
                    .insert(request.run_id.clone(), runtime);
                Ok(TakenPermissionResponse::Error(error))
            }
        }
    })?;

    let (mut runtime, prepared, submitted) = match taken {
        TakenPermissionResponse::Provider(flight) => {
            let PermissionProviderFlight {
                runtime,
                prepared,
                submitted,
            } = *flight;
            (runtime, prepared, submitted)
        }
        TakenPermissionResponse::Complete(response) => {
            return bounded_json(
                &response,
                MAX_PROJECT_RESPONSE_BYTES,
                BridgeError::ManagedRunResponseTooLarge,
            );
        }
        TakenPermissionResponse::Error(error) => {
            return project_json(&CommandError::for_code(managed_start_error_code(error)));
        }
    };
    let _flight_guard = ManagedObservationFlight {
        core_manager,
        run_id: request.run_id.clone(),
    };
    let delivered = respond(&mut runtime, prepared.provider_decision)
        .ok()
        .is_some_and(|delivery| permission_delivery_matches(&prepared, &delivery));
    let result_kind = if delivered {
        ManagedPermissionResponseResultKind::Resolved(prepared.resolution)
    } else {
        ManagedPermissionResponseResultKind::DeliveryUnknown(
            ManagedPermissionDeliveryUnknownReason::ProviderOutcomeAmbiguous,
        )
    };
    let result = managed_start::permission_response_result(&prepared, &request, result_kind);
    let finished = core_manager.with_ready_core(|core| {
        let outcome = core
            .store
            .finish_managed_permission_response(result)
            .map_err(|_| BridgeError::StorageFailure)?;
        core.managed_observations_in_flight.remove(&request.run_id);
        if delivered {
            runtime.clear_active_permission();
            core.managed_runtimes
                .insert(request.run_id.clone(), runtime);
        }
        Ok((outcome, delivered))
    });
    let (outcome, _) = match finished {
        Ok(result) => result,
        Err(BridgeError::StorageFailure) => {
            return project_json(&CommandError::for_code(
                CommandErrorCode::StorageUnavailable,
            ));
        }
        Err(error) => return Err(error),
    };
    bounded_json(
        &permission_response_from_events(&request, &submitted, appended_event(&outcome))?,
        MAX_PROJECT_RESPONSE_BYTES,
        BridgeError::ManagedRunResponseTooLarge,
    )
}

fn recover_pending_permission_response(
    core: &mut FoundationCore,
    request: &ManagedRunPermissionRespondRequest,
) -> Result<Option<ManagedRunPermissionRespondResponse>, managed_start::ManagedStartError> {
    let latest = core
        .store
        .latest_ingest_seq()
        .map_err(|_| managed_start::ManagedStartError::StorageUnavailable)?;
    if latest <= request.request_version {
        return Ok(None);
    }
    let page =
        match core
            .store
            .run_events_through(&request.run_id, request.request_version, latest, 3)
        {
            Ok(page) => page,
            Err(StoreError::MissingRun { .. }) => return Ok(None),
            Err(error) => return Err(managed_start::map_permission_store_error(error)),
        };
    let Some(submitted) = page.events.first() else {
        return Ok(None);
    };
    if !stored_permission_submit_matches(submitted, request) {
        return Ok(None);
    }
    if let Some(outcome) = page.events.get(1)
        && matches!(
            outcome.event_type.as_str(),
            "permission.resolved" | "permission.delivery_unknown"
        )
    {
        if !stored_permission_terminal_matches(outcome, request) {
            return Err(managed_start::ManagedStartError::PermissionResponseConflict);
        }
        return permission_response_from_events(request, submitted, outcome)
            .map(Some)
            .map_err(|_| managed_start::ManagedStartError::StorageUnavailable);
    }

    let flit_protocol::NullableSessionId::Id(session_id) = &submitted.session_id else {
        return Err(managed_start::ManagedStartError::PermissionResponseConflict);
    };
    let session = core
        .store
        .managed_session(session_id)
        .map_err(|_| managed_start::ManagedStartError::StorageUnavailable)?
        .ok_or(managed_start::ManagedStartError::PermissionResponseConflict)?;
    let provider_turn_id = submitted
        .payload
        .get("provider_turn_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(managed_start::ManagedStartError::PermissionResponseConflict)?;
    let provider_item_id = submitted
        .payload
        .get("provider_item_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(managed_start::ManagedStartError::PermissionResponseConflict)?;
    let provider_request_id = submitted
        .payload
        .get("provider_request_id")
        .and_then(serde_json::Value::as_u64)
        .ok_or(managed_start::ManagedStartError::PermissionResponseConflict)?;
    let delivery_plan_fingerprint = submitted
        .payload
        .get("delivery_plan_fingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or(managed_start::ManagedStartError::PermissionResponseConflict)?;
    let decision = store_permission_decision(request.decision);
    let contract_version = submitted
        .source
        .contract_version
        .as_deref()
        .ok_or(managed_start::ManagedStartError::PermissionResponseConflict)?;
    let result = ManagedPermissionResponseResult {
        run_id: request.run_id.clone(),
        session_id: session_id.clone(),
        external_session_key: session.external_session_key,
        provider_turn_id: provider_turn_id.to_owned(),
        provider_item_id: provider_item_id.to_owned(),
        provider_request_id,
        request_id: request.request_id.clone(),
        request_version: request.request_version,
        response_attempt_id: request.response_attempt_id.clone(),
        decision,
        delivery_plan_fingerprint: delivery_plan_fingerprint.to_owned(),
        contract_version: contract_version.to_owned(),
        finished_at: request.finished_at.clone(),
        outcome_event_id: request.delivery_unknown_event_id.clone(),
        kind: ManagedPermissionResponseResultKind::DeliveryUnknown(
            ManagedPermissionDeliveryUnknownReason::CoreRestartedAfterSubmit,
        ),
    };
    let outcome = core
        .store
        .finish_managed_permission_response(result)
        .map_err(managed_start::map_permission_store_error)?;
    permission_response_from_events(request, submitted, appended_event(&outcome))
        .map(Some)
        .map_err(|_| managed_start::ManagedStartError::StorageUnavailable)
}

fn stored_permission_submit_matches(
    event: &flit_protocol::EventEnvelope,
    request: &ManagedRunPermissionRespondRequest,
) -> bool {
    event.event_type == "permission.response_submitted"
        && event.event_id == request.submitted_event_id
        && event.run_id == request.run_id
        && event.occurred_at == request.submitted_at
        && event.source.kind == flit_protocol::EventSourceKind::Core
        && event.source.provider.as_deref() == Some("codex")
        && matches!(
            event.source.contract_version.as_deref(),
            Some("codex-app-server/0.145.0" | "codex-app-server/0.146.0")
        )
        && stored_permission_payload_matches(event, request)
}

fn stored_permission_terminal_matches(
    event: &flit_protocol::EventEnvelope,
    request: &ManagedRunPermissionRespondRequest,
) -> bool {
    let expected_event_id = match event.event_type.as_str() {
        "permission.resolved" => request.resolved_event_id.as_str(),
        "permission.delivery_unknown" => request.delivery_unknown_event_id.as_str(),
        _ => return false,
    };
    event.event_id == expected_event_id
        && event.run_id == request.run_id
        && stored_permission_payload_matches(event, request)
}

fn stored_permission_payload_matches(
    event: &flit_protocol::EventEnvelope,
    request: &ManagedRunPermissionRespondRequest,
) -> bool {
    event
        .payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        == Some(request.request_id.as_str())
        && event
            .payload
            .get("request_version")
            .and_then(serde_json::Value::as_u64)
            == Some(request.request_version)
        && event
            .payload
            .get("response_attempt_id")
            .and_then(serde_json::Value::as_str)
            == Some(request.response_attempt_id.as_str())
        && event
            .payload
            .get("decision")
            .and_then(serde_json::Value::as_str)
            == Some(match request.decision {
                flit_protocol::ManagedRunPermissionDecision::AllowOnce => "allow_once",
                flit_protocol::ManagedRunPermissionDecision::Deny => "deny",
            })
}

fn store_permission_decision(
    decision: flit_protocol::ManagedRunPermissionDecision,
) -> ManagedPermissionDecision {
    match decision {
        flit_protocol::ManagedRunPermissionDecision::AllowOnce => {
            ManagedPermissionDecision::AllowOnce
        }
        flit_protocol::ManagedRunPermissionDecision::Deny => ManagedPermissionDecision::Deny,
    }
}

fn permission_delivery_matches(
    prepared: &managed_start::PreparedPermissionResponse,
    delivery: &flit_providers::CodexPermissionDelivery,
) -> bool {
    delivery.provider_request_id == prepared.attempt.provider_request_id
        && delivery.thread_id.as_str() == prepared.attempt.external_session_key
        && delivery.turn_id.as_str() == prepared.attempt.provider_turn_id
        && delivery.item_id.as_str() == prepared.attempt.provider_item_id
        && delivery.decision == prepared.provider_decision
}

fn appended_event(outcome: &AppendEventOutcome) -> &flit_protocol::EventEnvelope {
    match outcome {
        AppendEventOutcome::Inserted(event) | AppendEventOutcome::Duplicate(event) => event,
    }
}

fn permission_response_from_events(
    request: &ManagedRunPermissionRespondRequest,
    submitted: &flit_protocol::EventEnvelope,
    outcome: &flit_protocol::EventEnvelope,
) -> Result<ManagedRunPermissionRespondResponse, BridgeError> {
    let common = (
        PROTOCOL_VERSION.to_owned(),
        request.run_id.clone(),
        request.request_id.clone(),
        request.request_version,
        request.response_attempt_id.clone(),
        request.decision,
        submitted.event_id.clone(),
        submitted.ingest_seq,
        outcome.event_id.clone(),
        outcome.ingest_seq,
    );
    match outcome.event_type.as_str() {
        "permission.resolved" => Ok(ManagedRunPermissionRespondResponse::Delivered {
            protocol_version: common.0,
            run_id: common.1,
            request_id: common.2,
            request_version: common.3,
            response_attempt_id: common.4,
            decision: common.5,
            submitted_event_id: common.6,
            submitted_version: common.7,
            outcome_event_id: common.8,
            outcome_version: common.9,
        }),
        "permission.delivery_unknown" => Ok(ManagedRunPermissionRespondResponse::DeliveryUnknown {
            protocol_version: common.0,
            run_id: common.1,
            request_id: common.2,
            request_version: common.3,
            response_attempt_id: common.4,
            decision: common.5,
            submitted_event_id: common.6,
            submitted_version: common.7,
            outcome_event_id: common.8,
            outcome_version: common.9,
        }),
        _ => Err(BridgeError::CoreFailure),
    }
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
        managed_start::ManagedStartError::RunNotActive => CommandErrorCode::ManagedRunNotActive,
        managed_start::ManagedStartError::ProviderObservationUnknown => {
            CommandErrorCode::ProviderObservationUnknown
        }
        managed_start::ManagedStartError::PermissionRequestStale => {
            CommandErrorCode::PermissionRequestStale
        }
        managed_start::ManagedStartError::PermissionResponseConflict => {
            CommandErrorCode::PermissionResponseConflict
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
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        process,
        sync::{
            Arc,
            atomic::{AtomicU64, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use flit_protocol::{
        ManagedRunOpenInProviderRequest, ManagedRunPermissionDecision, ManagedRunPermissionMode,
        ManagedRunPermissionRespondRequest, ManagedRunPermissionRespondResponse,
        ManagedRunStartResponse, ProviderExecutionAfterQuit, QuitImpactReason, QuitImpactResponse,
        RunDetailReadRequest, RunDetailReadResponse, SystemHealthRequest,
    };
    use flit_providers::{
        CodexManagedItemId, CodexManagedThreadId, CodexManagedTurnId, CodexManualStartedThread,
        CodexPermissionDecision, CodexPermissionDelivery, CodexPermissionRequest,
        CodexProviderAutoStartedThread, CodexRuntimeFingerprint, CodexStartedTurn,
        CodexTurnObservation, CodexTurnTerminalOutcome, ProviderFingerprint, classify_codex,
        validated_codex_0_144_6_fingerprint, validated_codex_0_145_0_fingerprint,
    };
    use flit_store::{
        InitialManagedSessionConnection, ManagedRunIntent, ProjectRegistration,
        ProjectTrustConfirmation,
    };

    use super::*;

    static NEXT_OBSERVATION_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct ObservationDirectory(PathBuf);

    impl ObservationDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_OBSERVATION_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-bridge-observation-{label}-{}-{nonce}",
                process::id()
            ));
            fs::create_dir(&path).expect("observation directory");
            Self(path)
        }
    }

    impl Drop for ObservationDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct DetachedTestRuntime;

    impl managed_start::ManagedCodexRuntime for DetachedTestRuntime {
        fn validated_profile(&self) -> Option<&ProviderFingerprint> {
            None
        }

        fn start_manual(
            &mut self,
            _cwd: &Path,
        ) -> Result<CodexManualStartedThread, managed_start::ProviderStartAttemptError> {
            panic!("detached test runtime must not start a thread")
        }

        fn start_provider_auto(
            &mut self,
            _cwd: &Path,
        ) -> Result<CodexProviderAutoStartedThread, managed_start::ProviderStartAttemptError>
        {
            panic!("detached test runtime must not start a thread")
        }

        fn start_turn(
            &mut self,
            _thread_id: &CodexManagedThreadId,
            _prompt: &str,
        ) -> Result<CodexStartedTurn, ()> {
            panic!("detached test runtime must not start a turn")
        }

        fn wait_for_turn_observation(&mut self) -> Result<CodexTurnObservation, ()> {
            panic!("the injected observation boundary must be used")
        }

        fn respond_to_file_change_permission(
            &mut self,
            _request: &CodexPermissionRequest,
            _decision: flit_providers::CodexPermissionDecision,
        ) -> Result<flit_providers::CodexPermissionDelivery, ()> {
            panic!("the injected permission response boundary must be used")
        }

        fn delete_started_thread(
            self: Box<Self>,
            _thread_id: &CodexManagedThreadId,
        ) -> Result<(), ()> {
            Ok(())
        }
    }

    struct PanicConnector;

    impl managed_start::ManagedCodexConnector for PanicConnector {
        fn connect(
            &self,
            _path_environment: Option<&OsStr>,
        ) -> Result<Box<dyn managed_start::ManagedCodexRuntime>, ()> {
            panic!("an in-flight Run must reject start before provider connection")
        }
    }

    struct CountingFailureConnector {
        calls: Arc<AtomicUsize>,
    }

    impl managed_start::ManagedCodexConnector for CountingFailureConnector {
        fn connect(
            &self,
            _path_environment: Option<&OsStr>,
        ) -> Result<Box<dyn managed_start::ManagedCodexRuntime>, ()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(())
        }
    }

    fn managed_start_core(label: &str) -> (ObservationDirectory, Arc<CoreManager>, PathBuf) {
        let directory = ObservationDirectory::new(label);
        let project = directory.0.join("project");
        fs::create_dir(&project).expect("Project directory");
        let project = fs::canonicalize(project).expect("canonical Project");
        let manager = Arc::new(CoreManager::default());
        manager
            .initialize(directory.0.to_str().expect("UTF-8 test path"))
            .expect("initialize managed start Core");
        manager
            .with_ready_core(|core| {
                core.store
                    .register_project(ProjectRegistration {
                        id: "project-observe".to_owned(),
                        display_name: "Observation Project".to_owned(),
                        selected_path: project.clone(),
                        created_at: "2026-07-27T12:00:00Z".to_owned(),
                    })
                    .expect("register Project");
                core.store
                    .confirm_project_trust(ProjectTrustConfirmation {
                        project_id: "project-observe".to_owned(),
                        selected_path: project.clone(),
                        confirmed_at: "2026-07-27T12:00:00Z".to_owned(),
                    })
                    .expect("trust Project");
                Ok(())
            })
            .expect("seed managed start Core");
        (directory, manager, project)
    }

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

    fn observation_request_json() -> String {
        serde_json::to_string(&ManagedRunObserveRequest {
            run_id: "run-observe".to_owned(),
            observed_at: "2026-07-27T12:00:02Z".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("observation request")
    }

    fn permission_response_request(
        permission: &ManagedRunObserveResponse,
        decision: ManagedRunPermissionDecision,
    ) -> ManagedRunPermissionRespondRequest {
        let ManagedRunObserveResponse::PermissionRequested {
            run_id,
            request_id,
            request_version,
            ..
        } = permission
        else {
            panic!("permission response requires an open permission");
        };
        ManagedRunPermissionRespondRequest {
            run_id: run_id.clone(),
            request_id: request_id.clone(),
            request_version: *request_version,
            response_attempt_id: "attempt-observe".to_owned(),
            decision,
            submitted_at: "2026-07-27T12:00:03Z".to_owned(),
            finished_at: "2026-07-27T12:00:04Z".to_owned(),
            submitted_event_id: "event-permission-submitted".to_owned(),
            resolved_event_id: "event-permission-resolved".to_owned(),
            delivery_unknown_event_id: "event-permission-unknown".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        }
    }

    fn open_permission(manager: &CoreManager) -> ManagedRunObserveResponse {
        let response = managed_run_observe_with(
            manager,
            observation_request_json(),
            |_runtime| Ok(permission_observation()),
            managed_start::commit_managed_observation,
        )
        .expect("open permission");
        serde_json::from_str(&response).expect("permission JSON")
    }

    fn observation_start_request() -> ManagedRunStartRequest {
        ManagedRunStartRequest {
            run_id: "run-observe".to_owned(),
            session_id: "session-observe".to_owned(),
            project_id: "project-observe".to_owned(),
            title: "Observe one exact permission".to_owned(),
            goal: "Request one exact file change.".to_owned(),
            provider: ProtocolProviderKind::Codex,
            permission_mode: ManagedRunPermissionMode::Manual,
            permission_mode_version: 1,
            created_at: "2026-07-27T12:00:00Z".to_owned(),
            git_baseline_observed_at: "2026-07-27T12:00:00Z".to_owned(),
            started_at: "2026-07-27T12:00:01Z".to_owned(),
            run_created_event_id: "event-observe-created".to_owned(),
            git_baseline_event_id: "event-observe-git-baseline".to_owned(),
            start_requested_event_id: "event-observe-start-requested".to_owned(),
            session_connected_event_id: "event-observe-session-connected".to_owned(),
            start_failed_event_id: "event-observe-start-failed".to_owned(),
            start_unknown_event_id: "event-observe-start-unknown".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        }
    }

    fn permission_observation() -> CodexTurnObservation {
        CodexTurnObservation::PermissionRequested(CodexPermissionRequest {
            provider_request_id: 7,
            thread_id: CodexManagedThreadId::new("thread-observe").expect("thread ID"),
            turn_id: CodexManagedTurnId::new("turn-observe").expect("turn ID"),
            item_id: CodexManagedItemId::new("item-observe").expect("item ID"),
            started_at_ms: 17,
        })
    }

    fn terminal_observation() -> CodexTurnObservation {
        CodexTurnObservation::Terminal {
            thread_id: CodexManagedThreadId::new("thread-observe").expect("thread ID"),
            turn_id: CodexManagedTurnId::new("turn-observe").expect("turn ID"),
            outcome: CodexTurnTerminalOutcome::Completed,
        }
    }

    fn provider_auto_observation() -> CodexTurnObservation {
        CodexTurnObservation::ProviderAutoOutcome {
            thread_id: CodexManagedThreadId::new("thread-observe").expect("thread ID"),
            turn_id: CodexManagedTurnId::new("turn-observe").expect("turn ID"),
            review_id: CodexManagedItemId::new("review-observe").expect("review ID"),
            target_item_id: CodexManagedItemId::new("item-observe").expect("item ID"),
        }
    }

    fn observation_core(
        label: &str,
    ) -> (
        ObservationDirectory,
        Arc<CoreManager>,
        ManagedRunStartResponse,
    ) {
        let directory = ObservationDirectory::new(label);
        let project = directory.0.join("project");
        fs::create_dir(&project).expect("Project directory");
        let project = fs::canonicalize(project).expect("canonical Project");
        let manager = Arc::new(CoreManager::default());
        manager
            .initialize(directory.0.to_str().expect("UTF-8 test path"))
            .expect("initialize observation Core");
        let response = ManagedRunStartResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: "run-observe".to_owned(),
            session_id: "session-observe".to_owned(),
            provider_thread_id: "thread-observe".to_owned(),
            provider_turn_id: "turn-observe".to_owned(),
            permission_mode: ManagedRunPermissionMode::Manual,
            permission_mode_version: 1,
            provider_configuration: "readOnly+on-request+user".to_owned(),
        };
        manager
            .with_ready_core(|core| {
                core.store
                    .register_project(ProjectRegistration {
                        id: "project-observe".to_owned(),
                        display_name: "Observation Project".to_owned(),
                        selected_path: project.clone(),
                        created_at: "2026-07-27T12:00:00Z".to_owned(),
                    })
                    .expect("register Project");
                core.store
                    .confirm_project_trust(ProjectTrustConfirmation {
                        project_id: "project-observe".to_owned(),
                        selected_path: project.clone(),
                        confirmed_at: "2026-07-27T12:00:00Z".to_owned(),
                    })
                    .expect("trust Project");
                core.store
                    .create_managed_run_intent(ManagedRunIntent {
                        id: "run-observe".to_owned(),
                        project_id: "project-observe".to_owned(),
                        title: "Observe one exact permission".to_owned(),
                        goal: Some("Request one exact file change.".to_owned()),
                        start_request: serde_json::Map::new(),
                        git_baseline: GitBaselinePayload::Unavailable {
                            project_id: "project-observe".to_owned(),
                            reason: GitBaselineUnavailableReason::RunnerUnavailable,
                        },
                        git_baseline_observed_at: "2026-07-27T12:00:00Z".to_owned(),
                        created_at: "2026-07-27T12:00:00Z".to_owned(),
                        run_created_event_id: "event-observe-created".to_owned(),
                        git_baseline_event_id: "event-observe-git-baseline".to_owned(),
                        start_requested_event_id: "event-observe-start-requested".to_owned(),
                    })
                    .expect("create managed Run");
                core.store
                    .connect_initial_managed_session(InitialManagedSessionConnection {
                        id: "session-observe".to_owned(),
                        run_id: "run-observe".to_owned(),
                        external_session_key: "thread-observe".to_owned(),
                        session_fingerprint: "test-profile".to_owned(),
                        executable_path: Some(PathBuf::from("/private/tmp/codex")),
                        executable_version: Some("0.145.0".to_owned()),
                        cwd: project,
                        capabilities: serde_json::Map::from_iter([
                            ("history".to_owned(), serde_json::json!("unsupported")),
                            (
                                "open_in_provider".to_owned(),
                                serde_json::json!("unsupported"),
                            ),
                        ]),
                        contract_version: "codex-app-server/0.145.0".to_owned(),
                        started_at: "2026-07-27T12:00:01Z".to_owned(),
                        connected_event_id: "event-observe-session-connected".to_owned(),
                    })
                    .expect("connect managed session");
                core.managed_runtimes.insert(
                    "run-observe".to_owned(),
                    managed_start::RetainedManagedRun::for_test(
                        response.clone(),
                        Box::new(DetachedTestRuntime),
                    ),
                );
                Ok(())
            })
            .expect("seed observation Core");
        (directory, manager, response)
    }

    fn command_error(response: &str) -> CommandError {
        serde_json::from_str(response).expect("command error")
    }

    fn assert_runtime_state(manager: &CoreManager, retained: bool) {
        manager
            .with_ready_core(|core| {
                assert_eq!(core.managed_runtimes.contains_key("run-observe"), retained);
                assert!(!core.managed_observations_in_flight.contains("run-observe"));
                Ok(())
            })
            .expect("runtime state");
    }

    #[test]
    fn quit_impact_is_core_owned_complete_content_safe_and_read_only() {
        let (_directory, manager, _) = observation_core("quit-impact");
        let cursor = manager
            .with_ready_core(|core| {
                let cwd = core
                    .store
                    .managed_session("session-observe")
                    .expect("managed session read")
                    .expect("managed session")
                    .cwd;
                for (index, (suffix, capability)) in [
                    ("supported", serde_json::json!("supported")),
                    ("unsupported", serde_json::json!("unsupported")),
                    ("uncertain", serde_json::json!("degraded")),
                    ("invalid", serde_json::json!(7)),
                ]
                .into_iter()
                .enumerate()
                {
                    let run_id = format!("run-quit-{suffix}");
                    core.store
                        .create_managed_run_intent(ManagedRunIntent {
                            id: run_id.clone(),
                            project_id: "project-observe".to_owned(),
                            title: format!("Quit {suffix}"),
                            goal: None,
                            start_request: serde_json::Map::new(),
                            git_baseline: GitBaselinePayload::Unavailable {
                                project_id: "project-observe".to_owned(),
                                reason: GitBaselineUnavailableReason::RunnerUnavailable,
                            },
                            git_baseline_observed_at: format!("2026-07-27T12:01:{index:02}Z"),
                            created_at: format!("2026-07-27T12:01:{index:02}Z"),
                            run_created_event_id: format!("event-{suffix}-created"),
                            git_baseline_event_id: format!("event-{suffix}-git-baseline"),
                            start_requested_event_id: format!("event-{suffix}-requested"),
                        })
                        .expect("create Quit impact Run");
                    core.store
                        .connect_initial_managed_session(InitialManagedSessionConnection {
                            id: format!("session-quit-{suffix}"),
                            run_id,
                            external_session_key: format!("thread-quit-{suffix}"),
                            session_fingerprint: "test-profile".to_owned(),
                            executable_path: Some(PathBuf::from("/private/tmp/codex")),
                            executable_version: Some("0.145.0".to_owned()),
                            cwd: cwd.clone(),
                            capabilities: serde_json::Map::from_iter([(
                                "continue_after_quit".to_owned(),
                                capability,
                            )]),
                            contract_version: "codex-app-server/0.145.0".to_owned(),
                            started_at: format!("2026-07-27T12:02:{index:02}Z"),
                            connected_event_id: format!("event-{suffix}-connected"),
                        })
                        .expect("connect Quit impact session");
                }
                Ok(core.store.latest_ingest_seq().expect("Quit impact cursor"))
            })
            .expect("seed Quit impacts");

        let rendered = quit_impact_with(&manager, PROTOCOL_VERSION).expect("Quit impact response");
        let response =
            serde_json::from_str::<QuitImpactResponse>(&rendered).expect("Quit impact JSON");
        assert_eq!(response.cursor, cursor);
        assert!(response.flit_monitoring_stops);
        assert!(response.flit_notifications_stop);
        assert_eq!(response.runs.len(), 5);
        for (run_id, execution, reason) in [
            (
                "run-observe",
                ProviderExecutionAfterQuit::Unknown,
                QuitImpactReason::CapabilityMissing,
            ),
            (
                "run-quit-supported",
                ProviderExecutionAfterQuit::Continues,
                QuitImpactReason::CapabilitySupported,
            ),
            (
                "run-quit-unsupported",
                ProviderExecutionAfterQuit::Stops,
                QuitImpactReason::CapabilityUnsupported,
            ),
            (
                "run-quit-uncertain",
                ProviderExecutionAfterQuit::Unknown,
                QuitImpactReason::CapabilityUncertain,
            ),
            (
                "run-quit-invalid",
                ProviderExecutionAfterQuit::Unknown,
                QuitImpactReason::CapabilityInvalid,
            ),
        ] {
            let impact = response
                .runs
                .iter()
                .find(|impact| impact.run_id == run_id)
                .expect("Run impact");
            assert_eq!(impact.execution_after_quit, execution);
            assert_eq!(impact.reason, reason);
        }
        for forbidden in [
            "/private/tmp",
            "session-quit",
            "thread-quit",
            "test-profile",
            "continue_after_quit",
            "0.145.0",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "Quit impact must not expose {forbidden}"
            );
        }
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store.latest_ingest_seq().expect("post-read cursor"),
                    cursor
                );
                Ok(())
            })
            .expect("Quit impact must be read-only");
        assert_eq!(
            command_error(
                &quit_impact_with(&manager, "2.0").expect("protocol mismatch command error")
            )
            .code,
            CommandErrorCode::ProtocolMismatch
        );
    }

    #[test]
    fn quit_impact_empty_store_is_an_exact_empty_snapshot() {
        let directory = ObservationDirectory::new("quit-impact-empty");
        let manager = CoreManager::default();
        manager
            .initialize(directory.0.to_str().expect("UTF-8 path"))
            .expect("initialize empty Core");
        let response = serde_json::from_str::<QuitImpactResponse>(
            &quit_impact_with(&manager, PROTOCOL_VERSION).expect("empty Quit impact"),
        )
        .expect("empty Quit impact JSON");
        assert_eq!(response.cursor, 0);
        assert!(response.runs.is_empty());
        assert!(!response.core_instance_id.is_empty());
    }

    #[test]
    fn git_observation_releases_core_and_revalidates_the_exact_project() {
        let (_directory, manager, _) = observation_core("git-observation");
        let expected_project =
            git_project_target(&manager, "project-observe").expect("trusted Git Project target");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_manager = Arc::clone(&manager);
        let worker = thread::spawn(move || {
            git_observation_with(&worker_manager, "project-observe", |project| {
                assert_eq!(project, expected_project.canonical_path);
                entered_tx.send(()).expect("enter Git observation");
                release_rx.recv().expect("release Git observation");
                Ok(NativeGitObservation::NotWorktree(
                    NativeNotWorktreeReason::NotRepository,
                ))
            })
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("Git observation should start");

        let (health_tx, health_rx) = mpsc::channel();
        let health_manager = Arc::clone(&manager);
        let health = thread::spawn(move || {
            health_tx
                .send(health_manager.require_ready())
                .expect("send concurrent Core health");
        });
        assert_eq!(
            health_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("Git observation must not hold the Core mutex"),
            Ok(())
        );
        health.join().expect("concurrent health thread");
        release_tx.send(()).expect("release Git worker");
        assert_eq!(
            worker.join().expect("Git observation thread"),
            Ok(GitObservationResponse::NotWorktree {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                project_id: "project-observe".to_owned(),
                reason: GitNotWorktreeReason::NotRepository,
            })
        );

        let original = git_project_target(&manager, "project-observe")
            .expect("original Project target")
            .canonical_path;
        let moved = original.with_extension("moved");
        assert_eq!(
            git_observation_with(&manager, "project-observe", |_project| {
                fs::rename(&original, &moved).expect("move observed Project");
                fs::create_dir(&original).expect("replace observed Project");
                Ok(NativeGitObservation::NotWorktree(
                    NativeNotWorktreeReason::NotRepository,
                ))
            }),
            Err(BridgeError::ProjectIdentityMismatch)
        );
    }

    #[test]
    fn git_observation_mapping_is_content_free_and_exhaustive() {
        assert_eq!(
            protocol_git_observation(
                "project-observe",
                Ok(NativeGitObservation::Repository(
                    flit_git::RepositoryReceipt {
                        canonical_root: PathBuf::from("/private/tmp/project-observe"),
                        head: NativeGitHead::Available(
                            "0123456789abcdef0123456789abcdef01234567".to_owned(),
                        ),
                        dirty: flit_git::DirtySummary {
                            staged: 1,
                            unstaged: 2,
                            untracked: 3,
                            entries: 4,
                        },
                    },
                )),
            ),
            GitObservationResponse::Repository {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                project_id: "project-observe".to_owned(),
                canonical_root: "/private/tmp/project-observe".to_owned(),
                head: GitHead::Available {
                    oid: "0123456789abcdef0123456789abcdef01234567".to_owned(),
                },
                dirty: GitDirtySummary {
                    staged: 1,
                    unstaged: 2,
                    untracked: 3,
                    entries: 4,
                },
            }
        );
        for (error, reason) in [
            (
                GitObservationError::RunnerUnavailable,
                GitObservationUnavailableReason::RunnerUnavailable,
            ),
            (
                GitObservationError::GitNotFound,
                GitObservationUnavailableReason::GitUnavailable,
            ),
            (
                GitObservationError::ProjectDirectoryChanged,
                GitObservationUnavailableReason::ProjectChanged,
            ),
            (
                GitObservationError::UnexpectedCommandStderr {
                    phase: flit_git::GitCommandPhase::Status,
                },
                GitObservationUnavailableReason::ProcessUnavailable,
            ),
            (
                GitObservationError::MalformedPorcelain,
                GitObservationUnavailableReason::MalformedOutput,
            ),
        ] {
            assert_eq!(
                protocol_git_observation("project-observe", Err(error)),
                GitObservationResponse::Unavailable {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    project_id: "project-observe".to_owned(),
                    reason,
                }
            );
        }
        let rendered = serde_json::to_string(&protocol_git_observation(
            "project-observe",
            Err(GitObservationError::CommandFailed {
                phase: flit_git::GitCommandPhase::Status,
                exit_code: Some(77),
            }),
        ))
        .expect("content-free Git unavailable response");
        assert!(!rendered.contains("77"));
        assert!(!rendered.contains("stderr"));
    }

    #[test]
    fn managed_start_persists_every_baseline_shape_before_provider_connection() {
        #[derive(Clone, Copy)]
        enum BaselineCase {
            Clean,
            Dirty,
            Unborn,
            NotRepository,
            Unavailable,
        }

        for (label, case, expected_availability, expected_reason) in [
            ("clean", BaselineCase::Clean, "available", None),
            ("dirty", BaselineCase::Dirty, "available", None),
            ("unborn", BaselineCase::Unborn, "available", None),
            (
                "not-repository",
                BaselineCase::NotRepository,
                "unavailable",
                Some("not_repository"),
            ),
            (
                "unavailable",
                BaselineCase::Unavailable,
                "unavailable",
                Some("git_unavailable"),
            ),
        ] {
            let (_directory, manager, _project) = managed_start_core(label);
            let calls = Arc::new(AtomicUsize::new(0));
            let connector = CountingFailureConnector {
                calls: Arc::clone(&calls),
            };
            let manager_during_observation = Arc::clone(&manager);
            let response = managed_run_start_with(
                &manager,
                &connector,
                None,
                observation_start_request(),
                move |project| {
                    manager_during_observation
                        .with_ready_core(|core| {
                            assert_eq!(
                                core.store.latest_ingest_seq().expect("pre-start cursor"),
                                0
                            );
                            Ok(())
                        })
                        .expect("Git observation must not hold the Core mutex");
                    match case {
                        BaselineCase::Clean => Ok(NativeGitObservation::Repository(
                            flit_git::RepositoryReceipt {
                                canonical_root: project.to_owned(),
                                head: NativeGitHead::Available(
                                    "0123456789abcdef0123456789abcdef01234567".to_owned(),
                                ),
                                dirty: flit_git::DirtySummary {
                                    staged: 0,
                                    unstaged: 0,
                                    untracked: 0,
                                    entries: 0,
                                },
                            },
                        )),
                        BaselineCase::Dirty => Ok(NativeGitObservation::Repository(
                            flit_git::RepositoryReceipt {
                                canonical_root: project.to_owned(),
                                head: NativeGitHead::Available(
                                    "0123456789abcdef0123456789abcdef01234567".to_owned(),
                                ),
                                dirty: flit_git::DirtySummary {
                                    staged: 1,
                                    unstaged: 2,
                                    untracked: 1,
                                    entries: 3,
                                },
                            },
                        )),
                        BaselineCase::Unborn => Ok(NativeGitObservation::Repository(
                            flit_git::RepositoryReceipt {
                                canonical_root: project.to_owned(),
                                head: NativeGitHead::Unborn,
                                dirty: flit_git::DirtySummary {
                                    staged: 0,
                                    unstaged: 0,
                                    untracked: 0,
                                    entries: 0,
                                },
                            },
                        )),
                        BaselineCase::NotRepository => Ok(NativeGitObservation::NotWorktree(
                            NativeNotWorktreeReason::NotRepository,
                        )),
                        BaselineCase::Unavailable => Err(GitObservationError::GitNotFound),
                    }
                },
            )
            .expect("managed start command response");
            assert_eq!(
                command_error(&response),
                CommandError::for_code(CommandErrorCode::ProviderUnavailable)
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            manager
                .with_ready_core(|core| {
                    let events = core
                        .store
                        .run_events_through("run-observe", 0, 4, 10)
                        .expect("baseline events");
                    assert_eq!(
                        events
                            .events
                            .iter()
                            .map(|event| event.event_type.as_str())
                            .collect::<Vec<_>>(),
                        [
                            "run.created",
                            "git.snapshot_recorded",
                            "run.start_requested",
                            "run.failed",
                        ]
                    );
                    let baseline = &events.events[1];
                    assert_eq!(baseline.payload["availability"], expected_availability);
                    assert_eq!(
                        baseline
                            .payload
                            .get("reason")
                            .and_then(serde_json::Value::as_str),
                        expected_reason
                    );
                    assert_eq!(baseline.payload["project_id"], "project-observe");
                    let snapshot = core
                        .store
                        .run_snapshot("run-observe")
                        .expect("Run snapshot")
                        .expect("persisted Run snapshot");
                    assert_eq!(
                        snapshot.snapshot["changes"],
                        serde_json::json!({
                            "availability": "unavailable",
                            "reason": "git_observation_not_configured"
                        })
                    );
                    Ok(())
                })
                .expect("inspect baseline Store");
        }
    }

    #[test]
    fn managed_start_blocks_project_drift_and_baseline_store_failure_before_provider() {
        let (_drift_directory, drift_manager, project) = managed_start_core("project-drift");
        let drift_calls = Arc::new(AtomicUsize::new(0));
        let drift_response = managed_run_start_with(
            &drift_manager,
            &CountingFailureConnector {
                calls: Arc::clone(&drift_calls),
            },
            None,
            observation_start_request(),
            move |_project| {
                fs::rename(&project, project.with_extension("replaced"))
                    .expect("move observed Project");
                fs::create_dir(&project).expect("replace observed Project");
                Ok(NativeGitObservation::NotWorktree(
                    NativeNotWorktreeReason::NotRepository,
                ))
            },
        )
        .expect("Project drift response");
        assert_eq!(
            command_error(&drift_response),
            CommandError::for_code(CommandErrorCode::ProjectIdentityMismatch)
        );
        assert_eq!(drift_calls.load(Ordering::SeqCst), 0);
        drift_manager
            .with_ready_core(|core| {
                assert!(
                    core.store
                        .managed_run("run-observe")
                        .expect("Run read")
                        .is_none()
                );
                assert_eq!(core.store.latest_ingest_seq().expect("drift cursor"), 0);
                Ok(())
            })
            .expect("inspect drift Store");

        let (failure_directory, failure_manager, _project) = managed_start_core("store-failure");
        let raw = rusqlite::Connection::open(failure_directory.0.join(DATABASE_FILE_NAME))
            .expect("open raw Store connection");
        raw.execute_batch(
            "CREATE TRIGGER reject_git_baseline BEFORE INSERT ON events
             WHEN NEW.event_type = 'git.snapshot_recorded'
             BEGIN
               SELECT RAISE(ABORT, 'injected baseline failure');
             END;",
        )
        .expect("install baseline failure trigger");
        drop(raw);
        let failure_calls = Arc::new(AtomicUsize::new(0));
        let failure_response = managed_run_start_with(
            &failure_manager,
            &CountingFailureConnector {
                calls: Arc::clone(&failure_calls),
            },
            None,
            observation_start_request(),
            |_project| {
                Ok(NativeGitObservation::NotWorktree(
                    NativeNotWorktreeReason::NotRepository,
                ))
            },
        )
        .expect("baseline Store failure response");
        assert_eq!(
            command_error(&failure_response),
            CommandError::for_code(CommandErrorCode::StorageUnavailable)
        );
        assert_eq!(failure_calls.load(Ordering::SeqCst), 0);
        failure_manager
            .with_ready_core(|core| {
                assert!(
                    core.store
                        .managed_run("run-observe")
                        .expect("Run read")
                        .is_none()
                );
                assert_eq!(core.store.latest_ingest_seq().expect("failure cursor"), 0);
                Ok(())
            })
            .expect("inspect failed baseline Store");
    }

    #[test]
    fn bundled_git_runner_discovery_rejects_noncanonical_or_escaped_layouts() {
        let directory = ObservationDirectory::new("git-bundle-layout");
        let root = fs::canonicalize(&directory.0).expect("canonical bundle root");
        let contents = root.join("Flit.app/Contents");
        let macos = contents.join("MacOS");
        let helpers = contents.join("Helpers");
        fs::create_dir_all(&macos).expect("app executable directory");
        fs::create_dir_all(&helpers).expect("app helper directory");
        let app = macos.join("Flit");
        let runner = helpers.join("flit-git-noexec");
        fs::write(&app, b"app").expect("app executable fixture");
        fs::write(&runner, b"runner").expect("runner fixture");
        assert_eq!(
            bundled_git_runner_path_for(&app).expect("exact bundled runner"),
            runner
        );

        let alias = root.join("Flit-alias");
        std::os::unix::fs::symlink(&app, &alias).expect("app alias");
        assert_eq!(
            bundled_git_runner_path_for(&alias),
            Err(GitObservationError::RunnerUnavailable)
        );
        fs::remove_file(&runner).expect("remove exact runner");
        let escaped = root.join("escaped-runner");
        fs::write(&escaped, b"escaped").expect("escaped runner");
        std::os::unix::fs::symlink(&escaped, &runner).expect("escaped runner link");
        assert_eq!(
            bundled_git_runner_path_for(&app),
            Err(GitObservationError::RunnerUnavailable)
        );
    }

    #[test]
    fn dashboard_initial_delta_and_resync_are_exact_and_bounded() {
        let (directory, manager, _) = observation_core("dashboard-read");
        let initial = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: None,
                after_cursor: None,
                requested_event_limit: MAX_DASHBOARD_DELTA_EVENTS as u32,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("initial request"),
        )
        .expect("initial Dashboard read");
        let (core_instance_id, initial_cursor) =
            match serde_json::from_str::<DashboardReadResponse>(&initial)
                .expect("initial Dashboard response")
            {
                DashboardReadResponse::Snapshot {
                    reason,
                    core_instance_id,
                    requested_after_cursor,
                    retained_after_cursor,
                    next_cursor,
                    has_more,
                    runs,
                    ..
                } => {
                    assert_eq!(reason, DashboardSnapshotReason::Initial);
                    assert_eq!(requested_after_cursor, None);
                    assert_eq!(retained_after_cursor, 0);
                    assert!(!has_more);
                    assert_eq!(runs.len(), 1);
                    assert_eq!(runs[0].run_id, "run-observe");
                    assert_eq!(runs[0].project_display_name, "Observation Project");
                    assert_eq!(runs[0].version, next_cursor);
                    assert_eq!(runs[0].activity, "Unknown");
                    assert_eq!(runs[0].attention_open_count, 0);
                    assert_eq!(
                        runs[0].changes,
                        flit_protocol::DashboardChangeSummary::Unavailable {
                            reason: "git_observation_not_configured".to_owned(),
                        }
                    );
                    assert!(core_instance_id.starts_with("core-"));
                    assert!(
                        !core_instance_id
                            .contains(directory.0.to_str().expect("UTF-8 Dashboard path"))
                    );
                    (core_instance_id, next_cursor)
                }
                DashboardReadResponse::Delta { .. } => panic!("initial read must be a snapshot"),
            };

        managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| Ok(terminal_observation()),
            managed_start::commit_managed_observation,
        )
        .expect("append Dashboard delta");
        let delta = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some(core_instance_id.clone()),
                after_cursor: Some(initial_cursor),
                requested_event_limit: 1,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("delta request"),
        )
        .expect("Dashboard delta");
        let delta_cursor = match serde_json::from_str::<DashboardReadResponse>(&delta)
            .expect("delta response")
        {
            DashboardReadResponse::Delta {
                requested_after_cursor,
                retained_after_cursor,
                next_cursor,
                has_more,
                events,
                runs,
                ..
            } => {
                assert_eq!(requested_after_cursor, initial_cursor);
                assert_eq!(retained_after_cursor, 0);
                assert_eq!(next_cursor, initial_cursor + 1);
                assert!(!has_more);
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].cursor, next_cursor);
                assert_eq!(events[0].event_type, "run.completed");
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].run_id, "run-observe");
                assert_eq!(runs[0].version, next_cursor);
                assert_eq!(runs[0].lifecycle, "Finished");
                next_cursor
            }
            DashboardReadResponse::Snapshot { .. } => panic!("current cursor must return a delta"),
        };
        assert!(delta.len() <= MAX_DASHBOARD_RESPONSE_BYTES);

        let empty_delta = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some(core_instance_id.clone()),
                after_cursor: Some(delta_cursor),
                requested_event_limit: 50,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("empty delta request"),
        )
        .expect("empty Dashboard delta");
        assert!(matches!(
            serde_json::from_str::<DashboardReadResponse>(&empty_delta)
                .expect("empty delta response"),
            DashboardReadResponse::Delta {
                next_cursor,
                has_more: false,
                events,
                runs,
                ..
            } if next_cursor == delta_cursor && events.is_empty() && runs.is_empty()
        ));

        let mismatched = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some("stale-core".to_owned()),
                after_cursor: Some(initial_cursor),
                requested_event_limit: 50,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("mismatched request"),
        )
        .expect("mismatched resync");
        assert!(matches!(
            serde_json::from_str::<DashboardReadResponse>(&mismatched).expect("resync response"),
            DashboardReadResponse::Snapshot {
                reason: DashboardSnapshotReason::CoreInstanceMismatch,
                requested_after_cursor: Some(cursor),
                next_cursor,
                ..
            } if cursor == initial_cursor && next_cursor == initial_cursor + 1
        ));

        let ahead = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some(core_instance_id),
                after_cursor: Some(initial_cursor + 2),
                requested_event_limit: 50,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("ahead request"),
        )
        .expect("ahead resync");
        assert!(matches!(
            serde_json::from_str::<DashboardReadResponse>(&ahead).expect("ahead response"),
            DashboardReadResponse::Snapshot {
                reason: DashboardSnapshotReason::CursorAhead,
                ..
            }
        ));
    }

    #[test]
    fn dashboard_delta_defers_a_projection_until_its_exact_tail_page() {
        let (_directory, manager, _) = observation_core("dashboard-delta-tail");
        let permission = open_permission(&manager);
        let permission_cursor = manager
            .with_ready_core(|core| {
                core.store
                    .latest_ingest_seq()
                    .map_err(|_| BridgeError::StorageFailure)
            })
            .expect("permission cursor");
        let request =
            permission_response_request(&permission, ManagedRunPermissionDecision::AllowOnce);
        managed_run_permission_respond_with(
            &manager,
            serde_json::to_string(&request).expect("permission response request"),
            |_runtime, decision| {
                Ok(CodexPermissionDelivery {
                    provider_request_id: 7,
                    thread_id: CodexManagedThreadId::new("thread-observe").expect("thread ID"),
                    turn_id: CodexManagedTurnId::new("turn-observe").expect("turn ID"),
                    item_id: CodexManagedItemId::new("item-observe").expect("item ID"),
                    decision,
                })
            },
        )
        .expect("resolve permission");
        let core_instance_id = manager
            .with_ready_core(|core| Ok(core.core_instance_id.clone()))
            .expect("Core instance ID");

        let first = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some(core_instance_id.clone()),
                after_cursor: Some(permission_cursor),
                requested_event_limit: 1,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("first delta request"),
        )
        .expect("first delta page");
        let first_cursor =
            match serde_json::from_str::<DashboardReadResponse>(&first).expect("first delta") {
                DashboardReadResponse::Delta {
                    next_cursor,
                    has_more,
                    events,
                    runs,
                    ..
                } => {
                    assert!(has_more);
                    assert_eq!(events.len(), 1);
                    assert_eq!(events[0].event_type, "permission.response_submitted");
                    assert!(runs.is_empty());
                    next_cursor
                }
                DashboardReadResponse::Snapshot { .. } => {
                    panic!("current cursor must return a first delta page")
                }
            };

        let second = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some(core_instance_id.clone()),
                after_cursor: Some(first_cursor),
                requested_event_limit: 1,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("second delta request"),
        )
        .expect("second delta page");
        match serde_json::from_str::<DashboardReadResponse>(&second).expect("second delta") {
            DashboardReadResponse::Delta {
                next_cursor,
                has_more,
                events,
                runs,
                ..
            } => {
                assert!(!has_more);
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].event_type, "permission.resolved");
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].run_id, "run-observe");
                assert_eq!(runs[0].version, next_cursor);
                assert_eq!(runs[0].attention_open_count, 0);
            }
            DashboardReadResponse::Snapshot { .. } => {
                panic!("current cursor must return a second delta page")
            }
        }

        let combined = dashboard_read_with(
            &manager,
            &serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: Some(core_instance_id),
                after_cursor: Some(permission_cursor),
                requested_event_limit: 2,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("combined delta request"),
        )
        .expect("combined delta page");
        assert!(matches!(
            serde_json::from_str::<DashboardReadResponse>(&combined)
                .expect("combined delta response"),
            DashboardReadResponse::Delta {
                has_more: false,
                events,
                runs,
                ..
            } if events.len() == 2 && runs.len() == 1 && runs[0].run_id == "run-observe"
        ));
    }

    #[test]
    fn dashboard_request_boundaries_fail_closed_without_store_mutation() {
        let (_directory, manager, _) = observation_core("dashboard-invalid");
        let cursor = manager
            .with_ready_core(|core| {
                core.store
                    .latest_ingest_seq()
                    .map_err(|_| BridgeError::StorageFailure)
            })
            .expect("initial cursor");
        for request in [
            "{}".to_owned(),
            serde_json::json!({
                "expected_core_instance_id": "partial",
                "after_cursor": null,
                "requested_event_limit": 50,
                "client_protocol_version": PROTOCOL_VERSION
            })
            .to_string(),
            serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: None,
                after_cursor: None,
                requested_event_limit: 51,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("limit request"),
            serde_json::to_string(&DashboardReadRequest {
                expected_core_instance_id: None,
                after_cursor: None,
                requested_event_limit: 50,
                client_protocol_version: "0.0".to_owned(),
            })
            .expect("protocol request"),
        ] {
            let response = dashboard_read_with(&manager, &request).expect("command error response");
            let error = command_error(&response);
            assert!(matches!(
                error.code,
                CommandErrorCode::InvalidDashboardRequest | CommandErrorCode::ProtocolMismatch
            ));
        }
        let oversized = " ".repeat(MAX_DASHBOARD_REQUEST_BYTES + 1);
        assert_eq!(
            command_error(
                &dashboard_read_with(&manager, &oversized).expect("oversized command response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidDashboardRequest)
        );
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store
                        .latest_ingest_seq()
                        .map_err(|_| BridgeError::StorageFailure)?,
                    cursor
                );
                Ok(())
            })
            .expect("unchanged Dashboard Store");
    }

    #[test]
    fn run_detail_pages_structured_evidence_and_provider_open_is_exactly_unsupported() {
        assert_eq!(
            provider_open_error(ProtocolCapabilityStatus::Unsupported),
            BridgeError::CapabilityUnsupported
        );
        for status in [
            ProtocolCapabilityStatus::Supported,
            ProtocolCapabilityStatus::Degraded,
            ProtocolCapabilityStatus::Unknown,
            ProtocolCapabilityStatus::Unavailable,
        ] {
            assert_eq!(
                provider_open_error(status),
                BridgeError::ProviderUnavailable
            );
        }
        let (_directory, manager, _) = observation_core("run-detail");
        let (run_version, ingest_cursor) = manager
            .with_ready_core(|core| {
                let context = core
                    .store
                    .managed_run_detail_context("run-observe")
                    .map_err(|_| BridgeError::StorageFailure)?;
                let cursor = core
                    .store
                    .latest_ingest_seq()
                    .map_err(|_| BridgeError::StorageFailure)?;
                Ok((context.run_version, cursor))
            })
            .expect("Run detail context");

        let first = run_detail_read_with(
            &manager,
            &serde_json::to_string(&RunDetailReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: run_version,
                after_cursor: 0,
                requested_event_limit: 2,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("first detail request"),
        )
        .expect("first detail page");
        assert!(!first.contains("\"payload\""));
        assert!(!first.contains("\"source\""));
        assert!(!first.contains("/private/tmp"));
        let first: RunDetailReadResponse = serde_json::from_str(&first).expect("first detail JSON");
        assert_eq!(first.run_id, "run-observe");
        assert_eq!(first.run_version, run_version);
        assert_eq!(first.history_status, ProtocolCapabilityStatus::Unsupported);
        assert_eq!(
            first.open_in_provider_status,
            ProtocolCapabilityStatus::Unsupported
        );
        assert_eq!(first.events.len(), 2);
        assert!(first.has_more);
        assert!(first.events[0].cursor < first.events[1].cursor);

        let second = run_detail_read_with(
            &manager,
            &serde_json::to_string(&RunDetailReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: run_version,
                after_cursor: first.next_cursor,
                requested_event_limit: 2,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("second detail request"),
        )
        .expect("second detail page");
        let second: RunDetailReadResponse =
            serde_json::from_str(&second).expect("second detail JSON");
        assert!(!second.has_more);
        assert_eq!(second.next_cursor, run_version);
        assert_eq!(second.events.len(), 2);

        let open = managed_run_open_in_provider_with(
            &manager,
            &serde_json::to_string(&ManagedRunOpenInProviderRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: run_version,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("provider-open request"),
        )
        .expect("provider-open command error");
        assert_eq!(
            command_error(&open),
            CommandError::for_code(CommandErrorCode::CapabilityUnsupported)
        );
        let stale = managed_run_open_in_provider_with(
            &manager,
            &serde_json::to_string(&ManagedRunOpenInProviderRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: run_version - 1,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("stale provider-open request"),
        )
        .expect("stale provider-open command error");
        assert_eq!(
            command_error(&stale),
            CommandError::for_code(CommandErrorCode::RunVersionStale)
        );
        let missing = managed_run_open_in_provider_with(
            &manager,
            &serde_json::to_string(&ManagedRunOpenInProviderRequest {
                run_id: "run-missing".to_owned(),
                expected_run_version: run_version,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("missing provider-open request"),
        )
        .expect("missing provider-open command error");
        assert_eq!(
            command_error(&missing),
            CommandError::for_code(CommandErrorCode::RunNotFound)
        );
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store
                        .latest_ingest_seq()
                        .map_err(|_| BridgeError::StorageFailure)?,
                    ingest_cursor
                );
                assert!(core.managed_runtimes.contains_key("run-observe"));
                Ok(())
            })
            .expect("provider-open has no side effect");

        for response in [
            run_detail_read_with(&manager, "{}").expect("malformed detail command error"),
            managed_run_open_in_provider_with(&manager, "{}")
                .expect("malformed provider-open command error"),
            run_detail_read_with(&manager, &" ".repeat(MAX_MANAGED_RUN_REQUEST_BYTES + 1))
                .expect("oversized detail command error"),
        ] {
            assert_eq!(
                command_error(&response),
                CommandError::for_code(CommandErrorCode::InvalidRunRequest)
            );
        }
    }

    #[test]
    fn dashboard_resync_priority_and_response_byte_cap_are_explicit() {
        assert_eq!(
            dashboard_resync_reason("stale", "current", 9, 8, 7),
            Some(DashboardSnapshotReason::CoreInstanceMismatch)
        );
        assert_eq!(
            dashboard_resync_reason("current", "current", 9, 8, 7),
            Some(DashboardSnapshotReason::CursorAhead)
        );
        assert_eq!(
            dashboard_resync_reason("current", "current", 6, 8, 7),
            Some(DashboardSnapshotReason::CursorExpired)
        );
        assert_eq!(dashboard_resync_reason("current", "current", 7, 8, 7), None);
        assert_eq!(
            bounded_json(
                &"x".repeat(MAX_DASHBOARD_RESPONSE_BYTES),
                MAX_DASHBOARD_RESPONSE_BYTES,
                BridgeError::DashboardResponseTooLarge,
            ),
            Err(BridgeError::DashboardResponseTooLarge)
        );
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

        let malformed_observe: CommandError = serde_json::from_str(
            &managed_run_observe_json("{}".to_owned()).expect("malformed observe response"),
        )
        .expect("typed malformed observe error");
        assert_eq!(
            malformed_observe,
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );
        let mut observe_mismatch = fixture("managed_run_observe.request.json");
        observe_mismatch["client_protocol_version"] = serde_json::Value::String("9.9".to_owned());
        let observe_mismatch: CommandError = serde_json::from_str(
            &managed_run_observe_json(observe_mismatch.to_string())
                .expect("mismatched observe response"),
        )
        .expect("typed mismatched observe error");
        assert_eq!(observe_mismatch, CommandError::protocol_mismatch());
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

        let manual_profile = validated_codex_0_145_0_fingerprint();
        let manual = provider_diagnostics_response(Ok(CodexCompatibilityProbe {
            runtime_fingerprint: CodexRuntimeFingerprint {
                canonical_executable: manual_profile.canonical_executable.clone(),
                executable_version: manual_profile.executable_version.clone(),
                executable_sha256: manual_profile.executable_sha256.clone(),
                combined_schema_sha256: manual_profile.combined_schema_sha256.clone(),
                v2_schema_sha256: manual_profile.v2_schema_sha256.clone(),
            },
            validated_profile: Some(manual_profile.clone()),
            capability_snapshot: classify_codex(&manual_profile),
            version_stderr_bytes: 0,
            schema_stdout_bytes: 0,
            schema_stderr_bytes: 0,
        }));
        assert_eq!(
            manual
                .capabilities
                .iter()
                .find(|entry| entry.capability == ProtocolProviderCapability::PermissionRespond)
                .map(|entry| entry.status),
            Some(ProtocolCapabilityStatus::Supported)
        );
        assert_eq!(
            manual.capabilities.iter().find(|entry| {
                entry.capability == ProtocolProviderCapability::ProviderOutcomeObserve
            }),
            Some(&ProviderCapabilityEntry {
                capability: ProtocolProviderCapability::ProviderOutcomeObserve,
                status: ProtocolCapabilityStatus::Supported,
            })
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
    fn detached_observation_releases_core_and_guards_same_run_until_runtime_restoration() {
        let (_directory, manager, expected_start) = observation_core("detached-success");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let observer_manager = Arc::clone(&manager);
        let observer = thread::spawn(move || {
            managed_run_observe_with(
                &observer_manager,
                observation_request_json(),
                move |_runtime| {
                    entered_tx.send(()).expect("signal detached wait");
                    release_rx.recv().expect("release detached wait");
                    Ok(permission_observation())
                },
                managed_start::commit_managed_observation,
            )
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("detached wait begins");

        let (ready_tx, ready_rx) = mpsc::channel();
        let ready_manager = Arc::clone(&manager);
        let ready = thread::spawn(move || {
            ready_tx
                .send(ready_manager.require_ready())
                .expect("send ready result");
        });
        let core_was_responsive = ready_rx
            .recv_timeout(Duration::from_millis(250))
            .map(|result| result.is_ok())
            .unwrap_or(false);
        if !core_was_responsive {
            release_tx.send(()).expect("release stalled observation");
            observer.join().expect("join stalled observer").ok();
            ready.join().expect("join ready command");
            panic!("detached provider wait held the Core lock");
        }
        ready.join().expect("join ready command");

        let second_observe = managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| -> Result<CodexTurnObservation, managed_start::ManagedStartError> {
                panic!("parallel observe must fail before provider wait")
            },
            managed_start::commit_managed_observation,
        )
        .expect("parallel observe response");
        assert_eq!(
            command_error(&second_observe),
            CommandError::for_code(CommandErrorCode::ProviderObservationUnknown)
        );
        let start_while_observing = manager
            .with_ready_core(|core| {
                start_managed_run_in_core(
                    core,
                    &PanicConnector,
                    None,
                    GitBaselinePayload::Unavailable {
                        project_id: "project-observe".to_owned(),
                        reason: GitBaselineUnavailableReason::RunnerUnavailable,
                    },
                    observation_start_request(),
                )
            })
            .expect("parallel start response");
        assert_eq!(
            command_error(&start_while_observing),
            CommandError::for_code(CommandErrorCode::ProviderObservationUnknown)
        );

        release_tx.send(()).expect("release observation");
        let permission_json = observer
            .join()
            .expect("join observer")
            .expect("permission response");
        let permission: ManagedRunObserveResponse =
            serde_json::from_str(&permission_json).expect("permission response JSON");
        assert!(matches!(
            permission,
            ManagedRunObserveResponse::PermissionRequested { .. }
        ));
        assert_runtime_state(&manager, true);

        let cached_json = managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| -> Result<CodexTurnObservation, managed_start::ManagedStartError> {
                panic!("cached permission must not wait again")
            },
            managed_start::commit_managed_observation,
        )
        .expect("cached permission");
        assert_eq!(
            serde_json::from_str::<ManagedRunObserveResponse>(&cached_json)
                .expect("cached response"),
            permission
        );
        assert_runtime_state(&manager, true);
        assert_eq!(expected_start.run_id, "run-observe");
    }

    #[test]
    fn detached_provider_outcome_returns_durable_fact_and_restores_runtime() {
        let (_directory, manager, mut response) = observation_core("detached-provider-outcome");
        response.permission_mode = ManagedRunPermissionMode::ProviderAuto;
        response.provider_configuration = "readOnly+on-request+auto_review".to_owned();
        manager
            .with_ready_core(|core| {
                core.managed_runtimes.insert(
                    "run-observe".to_owned(),
                    managed_start::RetainedManagedRun::for_test(
                        response,
                        Box::new(DetachedTestRuntime),
                    ),
                );
                Ok(())
            })
            .expect("replace retained ProviderAuto runtime");

        let response = managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| Ok(provider_auto_observation()),
            managed_start::commit_managed_observation,
        )
        .expect("provider outcome response");
        let observed: ManagedRunObserveResponse =
            serde_json::from_str(&response).expect("provider outcome JSON");
        assert!(matches!(
            observed,
            ManagedRunObserveResponse::ProviderOutcomeResolved {
                request_version: 5,
                event_version: 6,
                provider_decision: flit_protocol::ManagedRunProviderDecision::Allowed,
                terminal_outcome: flit_protocol::ManagedRunProviderTerminalOutcome::RequestResolved,
                ..
            }
        ));
        assert_runtime_state(&manager, true);
        manager
            .with_ready_core(|core| {
                let events = core
                    .store
                    .run_events_through("run-observe", 0, 6, 10)
                    .expect("provider outcome events");
                assert_eq!(
                    events
                        .events
                        .iter()
                        .map(|event| event.event_type.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "run.created",
                        "git.snapshot_recorded",
                        "run.start_requested",
                        "session.connected",
                        "permission.requested",
                        "permission.provider_outcome_resolved",
                    ]
                );
                assert!(
                    events
                        .events
                        .iter()
                        .all(|event| { event.payload.get("response_attempt_id").is_none() })
                );
                Ok(())
            })
            .expect("inspect provider outcome Store");
    }

    #[test]
    fn exact_permission_response_submits_before_one_detached_causal_delivery() {
        let (_directory, manager, _) = observation_core("permission-delivered");
        let permission = open_permission(&manager);
        let request =
            permission_response_request(&permission, ManagedRunPermissionDecision::AllowOnce);
        let response_manager = Arc::clone(&manager);
        let calls = Arc::new(AtomicUsize::new(0));
        let response_calls = Arc::clone(&calls);
        let response = managed_run_permission_respond_with(
            &manager,
            serde_json::to_string(&request).expect("response request"),
            move |_runtime, decision| {
                response_calls.fetch_add(1, Ordering::SeqCst);
                response_manager
                    .with_ready_core(|core| {
                        assert!(core.managed_observations_in_flight.contains("run-observe"));
                        let cursor = core.store.latest_ingest_seq().expect("submitted cursor");
                        let events = core
                            .store
                            .run_events_through("run-observe", 0, cursor, 10)
                            .expect("submitted events");
                        assert_eq!(
                            events.events.last().expect("submitted event").event_type,
                            "permission.response_submitted"
                        );
                        Ok(())
                    })
                    .expect("Core remains responsive during provider response");
                assert_eq!(decision, CodexPermissionDecision::Accept);
                Ok(CodexPermissionDelivery {
                    provider_request_id: 7,
                    thread_id: CodexManagedThreadId::new("thread-observe").expect("thread ID"),
                    turn_id: CodexManagedTurnId::new("turn-observe").expect("turn ID"),
                    item_id: CodexManagedItemId::new("item-observe").expect("item ID"),
                    decision,
                })
            },
        )
        .expect("delivered response");
        let response: ManagedRunPermissionRespondResponse =
            serde_json::from_str(&response).expect("delivered JSON");
        assert!(matches!(
            response,
            ManagedRunPermissionRespondResponse::Delivered {
                submitted_version: 6,
                outcome_version: 7,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_runtime_state(&manager, true);
        manager
            .with_ready_core(|core| {
                let runtime = core
                    .managed_runtimes
                    .get("run-observe")
                    .expect("retained delivered runtime");
                assert!(runtime.cached_permission().is_none());
                Ok(())
            })
            .expect("delivered runtime state");
    }

    #[test]
    fn ambiguous_and_duplicate_pending_permission_responses_never_retry() {
        let (_unknown_directory, unknown_manager, _) = observation_core("permission-unknown");
        let unknown_permission = open_permission(&unknown_manager);
        let unknown_request =
            permission_response_request(&unknown_permission, ManagedRunPermissionDecision::Deny);
        let calls = Arc::new(AtomicUsize::new(0));
        let response_calls = Arc::clone(&calls);
        let unknown = managed_run_permission_respond_with(
            &unknown_manager,
            serde_json::to_string(&unknown_request).expect("unknown request"),
            move |_runtime, decision| {
                response_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(decision, CodexPermissionDecision::Decline);
                Err(())
            },
        )
        .expect("unknown response");
        assert!(matches!(
            serde_json::from_str::<ManagedRunPermissionRespondResponse>(&unknown)
                .expect("unknown JSON"),
            ManagedRunPermissionRespondResponse::DeliveryUnknown { .. }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_runtime_state(&unknown_manager, false);
        let retry = managed_run_permission_respond_with(
            &unknown_manager,
            serde_json::to_string(&unknown_request).expect("retry request"),
            |_runtime, _decision| -> Result<CodexPermissionDelivery, ()> {
                panic!("ambiguous delivery must never retry")
            },
        )
        .expect("retry delivery-unknown response");
        assert!(matches!(
            serde_json::from_str::<ManagedRunPermissionRespondResponse>(&retry)
                .expect("retry delivery-unknown JSON"),
            ManagedRunPermissionRespondResponse::DeliveryUnknown { .. }
        ));

        let (_pending_directory, pending_manager, _) =
            observation_core("permission-pending-duplicate");
        let pending_permission = open_permission(&pending_manager);
        let pending_request = permission_response_request(
            &pending_permission,
            ManagedRunPermissionDecision::AllowOnce,
        );
        pending_manager
            .with_ready_core(|core| {
                let prepared = {
                    let runtime = core
                        .managed_runtimes
                        .get("run-observe")
                        .expect("pending runtime");
                    managed_start::prepare_permission_response(runtime, &pending_request)
                        .expect("prepare pending response")
                };
                core.store
                    .submit_managed_permission_response(prepared.attempt)
                    .expect("seed pending submit");
                Ok(())
            })
            .expect("pending seed");
        let pending = managed_run_permission_respond_with(
            &pending_manager,
            serde_json::to_string(&pending_request).expect("pending request"),
            |_runtime, _decision| -> Result<CodexPermissionDelivery, ()> {
                panic!("duplicate pending response must not reach provider")
            },
        )
        .expect("pending duplicate response");
        assert!(matches!(
            serde_json::from_str::<ManagedRunPermissionRespondResponse>(&pending)
                .expect("pending JSON"),
            ManagedRunPermissionRespondResponse::DeliveryUnknown { .. }
        ));
        assert_runtime_state(&pending_manager, false);
    }

    #[test]
    fn stale_permission_response_is_rejected_before_submit_or_provider_call() {
        let (_directory, manager, _) = observation_core("permission-stale");
        let permission = open_permission(&manager);
        let mut request =
            permission_response_request(&permission, ManagedRunPermissionDecision::AllowOnce);
        request.request_version += 1;
        let calls = AtomicUsize::new(0);
        let response = managed_run_permission_respond_with(
            &manager,
            serde_json::to_string(&request).expect("stale request"),
            |_runtime, _decision| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(())
            },
        )
        .expect("stale response");
        assert_eq!(
            command_error(&response),
            CommandError::for_code(CommandErrorCode::PermissionRequestStale)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_runtime_state(&manager, true);
        manager
            .with_ready_core(|core| {
                assert_eq!(core.store.latest_ingest_seq().expect("stale cursor"), 5);
                Ok(())
            })
            .expect("stale Store state");

        let mut missing =
            permission_response_request(&permission, ManagedRunPermissionDecision::AllowOnce);
        missing.run_id = "run-missing".to_owned();
        missing.request_version = 1;
        let missing_response = managed_run_permission_respond_with(
            &manager,
            serde_json::to_string(&missing).expect("missing Run request"),
            |_runtime, _decision| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(())
            },
        )
        .expect("missing Run response");
        assert_eq!(
            command_error(&missing_response),
            CommandError::for_code(CommandErrorCode::ManagedRunNotActive)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store.latest_ingest_seq().expect("missing Run cursor"),
                    5
                );
                Ok(())
            })
            .expect("missing Run Store state");
    }

    #[test]
    fn restarted_core_closes_exact_persisted_pending_response_without_provider_call() {
        let (directory, manager, _) = observation_core("permission-restart");
        let permission = open_permission(&manager);
        let request =
            permission_response_request(&permission, ManagedRunPermissionDecision::AllowOnce);
        manager
            .with_ready_core(|core| {
                let prepared = {
                    let runtime = core
                        .managed_runtimes
                        .get("run-observe")
                        .expect("pre-restart runtime");
                    managed_start::prepare_permission_response(runtime, &request)
                        .expect("prepare pre-restart response")
                };
                core.store
                    .submit_managed_permission_response(prepared.attempt)
                    .expect("persist response before restart");
                Ok(())
            })
            .expect("persist pending response");
        drop(manager);

        let restarted = CoreManager::default();
        restarted
            .initialize(directory.0.to_str().expect("UTF-8 restart path"))
            .expect("restart Core");
        let response = managed_run_permission_respond_with(
            &restarted,
            serde_json::to_string(&request).expect("restart request"),
            |_runtime, _decision| -> Result<CodexPermissionDelivery, ()> {
                panic!("restart recovery must not call the provider")
            },
        )
        .expect("restart recovery response");
        assert!(matches!(
            serde_json::from_str::<ManagedRunPermissionRespondResponse>(&response)
                .expect("restart response JSON"),
            ManagedRunPermissionRespondResponse::DeliveryUnknown {
                submitted_version: 6,
                outcome_version: 8,
                ..
            }
        ));
        assert_runtime_state(&restarted, false);
        restarted
            .with_ready_core(|core| {
                let events = core
                    .store
                    .run_events_through("run-observe", 0, 8, 10)
                    .expect("restart events");
                assert_eq!(
                    events.events[events.events.len() - 2].event_type,
                    "diagnostic.sequence_gap"
                );
                assert_eq!(
                    events.events.last().expect("restart outcome").event_type,
                    "permission.delivery_unknown"
                );
                Ok(())
            })
            .expect("restart Store state");
    }

    #[test]
    fn detached_observation_closes_each_terminal_unknown_storage_and_unwind_path() {
        let (_terminal_directory, terminal_manager, _) = observation_core("detached-terminal");
        let terminal_json = managed_run_observe_with(
            &terminal_manager,
            observation_request_json(),
            |_runtime| Ok(terminal_observation()),
            managed_start::commit_managed_observation,
        )
        .expect("terminal response");
        assert!(matches!(
            serde_json::from_str::<ManagedRunObserveResponse>(&terminal_json)
                .expect("terminal JSON"),
            ManagedRunObserveResponse::TurnCompleted { .. }
        ));
        assert_runtime_state(&terminal_manager, false);
        terminal_manager
            .with_ready_core(|core| {
                assert!(
                    core.store
                        .managed_run("run-observe")
                        .expect("read terminal Run")
                        .expect("terminal Run")
                        .ended_at
                        .is_some()
                );
                Ok(())
            })
            .expect("terminal state");

        let (_unknown_directory, unknown_manager, _) = observation_core("detached-unknown");
        let unknown_json = managed_run_observe_with(
            &unknown_manager,
            observation_request_json(),
            |_runtime| Err(managed_start::ManagedStartError::ProviderObservationUnknown),
            managed_start::commit_managed_observation,
        )
        .expect("Unknown response");
        assert_eq!(
            command_error(&unknown_json),
            CommandError::for_code(CommandErrorCode::ProviderObservationUnknown)
        );
        assert_runtime_state(&unknown_manager, false);
        unknown_manager
            .with_ready_core(|core| {
                let cursor = core.store.latest_ingest_seq().expect("Unknown cursor");
                let events = core
                    .store
                    .run_events_through("run-observe", 0, cursor, 10)
                    .expect("Unknown events");
                assert_eq!(
                    events.events.last().expect("Unknown event").event_type,
                    "diagnostic.sequence_gap"
                );
                Ok(())
            })
            .expect("Unknown event state");

        let (_storage_directory, storage_manager, _) = observation_core("detached-storage");
        let storage_json = managed_run_observe_with(
            &storage_manager,
            observation_request_json(),
            |_runtime| Ok(permission_observation()),
            |_store, _runtime, _request, _observation| {
                Err(managed_start::ManagedStartError::StorageUnavailable)
            },
        )
        .expect("storage error response");
        assert_eq!(
            command_error(&storage_json),
            CommandError::for_code(CommandErrorCode::StorageUnavailable)
        );
        assert_runtime_state(&storage_manager, false);

        let (_panic_directory, panic_manager, _) = observation_core("detached-panic");
        let panic_result = protect(|| {
            managed_run_observe_with(
                &panic_manager,
                observation_request_json(),
                |_runtime| Ok(permission_observation()),
                |_store, _runtime, _request, _observation| panic!("commit panic control"),
            )
        });
        assert_eq!(panic_result, Err(BridgeError::CoreFailure));
        assert_runtime_state(&panic_manager, false);
        assert!(panic_manager.require_ready().is_ok());
    }

    #[test]
    fn panic_is_contained_and_does_not_poison_the_next_request() {
        let failure = protect::<()>(|| panic!("bridge panic control"));
        assert_eq!(failure, Err(BridgeError::CoreFailure));
        assert!(system_health_json(PROTOCOL_VERSION.to_owned()).is_ok());
    }
}
