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
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use flit_git::{
    GitChangeBaseline, GitHead as NativeGitHead, GitObservation as NativeGitObservation,
    GitObservationError, NotWorktreeReason as NativeNotWorktreeReason, inspect_git_on_path,
    inspect_noexec_runner_at, observe_clean_change_baseline, observe_repository,
};
use flit_protocol::{
    AttentionAcknowledgeRejectedReason, AttentionAcknowledgeRequest, AttentionAcknowledgeResponse,
    CapabilityStatus as ProtocolCapabilityStatus, CommandError, CommandErrorCode,
    DashboardChangeAttribution as ProtocolDashboardChangeAttribution, DashboardEventRecord,
    DashboardReadRequest, DashboardReadResponse, DashboardRunRecord, DashboardSnapshotReason,
    EVENT_PROTOCOL_VERSION, EffectiveNotificationPolicyRecord, EventSourceKind,
    FingerprintAxis as ProtocolFingerprintAxis, GitBaselinePayload, GitBaselineUnavailableReason,
    GitDirtySummary, GitHead, GitNotWorktreeReason, GitObservationResponse,
    GitObservationUnavailableReason, GlobalNotificationPolicyRecord,
    GlobalNotificationPolicyUpdateRequest, HealthStatus, ManagedRunObserveRequest,
    ManagedRunObserveResponse, ManagedRunOpenInProviderRequest, ManagedRunPermissionRespondRequest,
    ManagedRunPermissionRespondResponse, ManagedRunStartRequest,
    ManagedRunStillWorkingRejectedReason, ManagedRunStillWorkingRequest,
    ManagedRunStillWorkingResponse, ManagedRunsAssessStuckRequest,
    ManagedStuckNotificationDeliveredRejectedReason, ManagedStuckNotificationDeliveredRequest,
    ManagedStuckNotificationDeliveredResponse, ManagedStuckNotificationDeliveryClaimRequest,
    ManagedStuckNotificationDeliveryClaimResponse, ManagedStuckNotificationDeliveryFailedRequest,
    ManagedStuckNotificationDeliveryFailedResponse, ManagedStuckNotificationDueRecord,
    ManagedStuckNotificationsDueReadRequest, ManagedStuckNotificationsDueReadResponse,
    NotificationDeliveredRequest, NotificationDeliveredResponse,
    NotificationDeliveriesDueReadRequest, NotificationDeliveriesDueReadResponse,
    NotificationDeliveryClaimRequest, NotificationDeliveryClaimResponse,
    NotificationDeliveryFailedRequest, NotificationDeliveryFailedResponse,
    NotificationDeliveryKind as ProtocolNotificationKind, NotificationDeliveryRecord,
    NotificationKindOverridesRecord, NotificationKindsRecord, NotificationOverrideRecord,
    NotificationPolicyReadRequest, NotificationPolicyResponse, PROTOCOL_VERSION,
    ProjectInspectionResponse, ProjectListCursor as ProjectListCursorResponse,
    ProjectNotificationMasterRecord, ProjectNotificationPolicyRecord,
    ProjectNotificationPolicyUpdateRequest, ProjectRecord, ProjectRegistrationResponse,
    ProjectRegistrationStatus, ProjectTrustResponse, ProjectTrustStatus, ProjectsListResponse,
    ProviderCapability as ProtocolProviderCapability, ProviderCapabilityEntry,
    ProviderCompatibility as ProtocolProviderCompatibility, ProviderDiagnosticsResponse,
    ProviderExecutionAfterQuit, ProviderKind as ProtocolProviderKind, ProviderUnavailableReason,
    QuietHoursRecord, QuitImpactReason, QuitImpactResponse, QuitImpactRun,
    RunActiveAttentionAction as ProtocolRunActiveAttentionAction,
    RunActiveAttentionCategory as ProtocolRunActiveAttentionCategory,
    RunActiveAttentionItem as ProtocolRunActiveAttentionItem, RunActiveAttentionReadRequest,
    RunActiveAttentionReadResponse,
    RunActiveAttentionSeverity as ProtocolRunActiveAttentionSeverity, RunActiveAttentionSlot,
    RunActiveAttentionStatus as ProtocolRunActiveAttentionStatus,
    RunChangeExternalOpenDisabledReason, RunChangeExternalOpenRequest,
    RunChangeExternalOpenResponse, RunChangeHead, RunChangesReadRequest, RunChangesReadResponse,
    RunChangesUnavailableReason, RunDetailReadRequest, RunDetailReadResponse, RunEvidenceRecord,
    RunFileChangeRecord, RunFileChangeStatus, RunFileProjectScope, SystemHealthResponse,
};
use flit_providers::{
    CapabilityStatus, CodexCompatibilityProbe, CodexCompatibilityProbeError, CodexProcessProbe,
    ExecutableInspectionError, FingerprintAxis, MAX_CODEX_COMMAND_STARTS_PER_TURN,
    ProviderCapability, ProviderCapabilitySnapshot, ProviderCompatibility,
    probe_codex_compatibility_on_path,
};
use flit_store::{
    AppendEventOutcome, DashboardChangeAttribution as StoreDashboardChangeAttribution,
    DashboardChangeSummary as StoreDashboardChangeSummary,
    DashboardRunSnapshot as StoreDashboardRunSnapshot,
    EffectiveNotificationPolicy as StoreEffectiveNotificationPolicy,
    GlobalNotificationPolicy as StoreGlobalNotificationPolicy, MAX_DASHBOARD_DELTA_EVENTS,
    MAX_LIVE_MANAGED_SESSIONS, MAX_MANAGED_GIT_CHANGE_PAGE_SIZE, MAX_PROJECT_PAGE_SIZE,
    MAX_RUN_DETAIL_EVENTS, ManagedAttentionAcknowledgeAction, ManagedAttentionAcknowledgeOutcome,
    ManagedAttentionAcknowledgeRejectedReason as StoreAttentionAcknowledgeRejectedReason,
    ManagedGitChangeAttribution as StoreManagedGitChangeAttribution,
    ManagedGitChangeSetMetadata as StoreManagedGitChangeSetMetadata,
    ManagedGitFileChange as StoreManagedGitFileChange,
    ManagedGitFileStatus as StoreManagedGitFileStatus,
    ManagedGitProjectScope as StoreManagedGitProjectScope, ManagedPermissionDecision,
    ManagedPermissionDeliveryUnknownReason, ManagedPermissionResponseAttemptOutcome,
    ManagedPermissionResponseResult, ManagedPermissionResponseResultKind, ManagedSession,
    ManagedStillWorkingAction, ManagedStillWorkingOutcome,
    ManagedStillWorkingRejectedReason as StoreStillWorkingRejectedReason,
    ManagedStuckNotificationDelivery, ManagedStuckNotificationDeliveryClaim,
    ManagedStuckNotificationDeliveryClaimOutcome, ManagedStuckNotificationDeliveryFailure,
    ManagedStuckNotificationDeliveryFailureOutcome, ManagedStuckNotificationState,
    NotificationDeliveryClaim as StoreNotificationDeliveryClaim,
    NotificationDeliveryClaimOutcome as StoreNotificationDeliveryClaimOutcome,
    NotificationDeliveryFailure as StoreNotificationDeliveryFailure,
    NotificationDeliveryFailureOutcome as StoreNotificationDeliveryFailureOutcome,
    NotificationDeliveryReceipt as StoreNotificationDeliveryReceipt,
    NotificationDeliveryReceiptOutcome as StoreNotificationDeliveryReceiptOutcome,
    NotificationKind as StoreNotificationKind,
    NotificationKindOverrides as StoreNotificationKindOverrides,
    NotificationKinds as StoreNotificationKinds, NotificationOverride as StoreNotificationOverride,
    NotificationPolicySnapshot as StoreNotificationPolicySnapshot, Project,
    ProjectDirectoryInspection, ProjectListCursor as StoreProjectListCursor,
    ProjectNotificationMaster as StoreProjectNotificationMaster,
    ProjectNotificationPolicy as StoreProjectNotificationPolicy, ProjectRegistration,
    ProjectRegistrationOutcome, ProjectTrustConfirmation, ProjectTrustOutcome,
    QuietHours as StoreQuietHours, RunActiveAttentionAction as StoreRunActiveAttentionAction,
    RunActiveAttentionItem as StoreRunActiveAttentionItem, Store, StoreError,
};
#[cfg(test)]
use flit_store::{
    ManagedGitChangeAttribution, ManagedGitChangeSet, ManagedGitChangeSummary,
    ManagedGitFileChange, ManagedGitFileStatus, ManagedGitProjectScope,
    ManagedGitRepositoryIdentity, ManagedProviderObservation, ManagedProviderObservationKind,
};
use sha2::{Digest, Sha256};

use crate::codex_recovery::{
    CodexRecoveryAttempt, ExactCodexRecoveryConnector, observe_codex_sessions,
    persist_codex_recovery_observations, unknown_codex_recovery_observations,
};
use crate::external_open::{
    ExternalOpenAuthority, ExternalOpenGuardError, ExternalOpenTarget,
    inspect_external_open_target, open_with_default_application,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub mod codex_recovery;
mod external_open;
mod managed_start;
#[cfg(test)]
mod phase2_journey;
mod stuck_assessment;

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
const MAX_NOTIFICATION_POLICY_REQUEST_BYTES: usize = 16 * 1_024;
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
    #[error("the notification policy request is invalid")]
    InvalidNotificationPolicy,
    #[error("the notification policy version is stale")]
    NotificationPolicyVersionStale,
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
    stuck_assessment: Box<StuckAssessmentState>,
    managed_observations_in_flight: BTreeSet<String>,
    startup_recovery_sessions: Option<Vec<ManagedSession>>,
    store: Store,
    provider_health: HealthStatus,
    _guard: File,
}

struct StuckAssessmentState {
    process_probes: BTreeMap<String, CodexProcessProbe>,
    progress_baselines: BTreeMap<String, stuck_assessment::ProgressBaseline>,
    clock_origin: Instant,
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
            stuck_assessment: Box::new(StuckAssessmentState {
                process_probes: BTreeMap::new(),
                progress_baselines: BTreeMap::new(),
                clock_origin: Instant::now(),
            }),
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
        BridgeError::InvalidNotificationPolicy => CommandErrorCode::InvalidNotificationPolicy,
        BridgeError::NotificationPolicyVersionStale => {
            CommandErrorCode::NotificationPolicyVersionStale
        }
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
        StoreError::InvalidNotificationPolicy { .. } => BridgeError::InvalidNotificationPolicy,
        StoreError::NotificationPolicyVersionStale { .. } => {
            BridgeError::NotificationPolicyVersionStale
        }
        StoreError::NotificationPolicyProjectUnavailable { .. } => BridgeError::ProjectNotFound,
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

fn protocol_notification_kinds(kinds: StoreNotificationKinds) -> NotificationKindsRecord {
    NotificationKindsRecord {
        permission: kinds.permission,
        question: kinds.question,
        failure: kinds.failure,
        completion: kinds.completion,
        stuck: kinds.stuck,
    }
}

fn store_notification_kinds(kinds: NotificationKindsRecord) -> StoreNotificationKinds {
    StoreNotificationKinds {
        permission: kinds.permission,
        question: kinds.question,
        failure: kinds.failure,
        completion: kinds.completion,
        stuck: kinds.stuck,
    }
}

fn protocol_quiet_hours(quiet_hours: StoreQuietHours) -> QuietHoursRecord {
    QuietHoursRecord {
        enabled: quiet_hours.enabled,
        start_minute: quiet_hours.start_minute,
        end_minute: quiet_hours.end_minute,
    }
}

fn store_quiet_hours(quiet_hours: QuietHoursRecord) -> StoreQuietHours {
    StoreQuietHours {
        enabled: quiet_hours.enabled,
        start_minute: quiet_hours.start_minute,
        end_minute: quiet_hours.end_minute,
    }
}

fn protocol_notification_override(value: StoreNotificationOverride) -> NotificationOverrideRecord {
    match value {
        StoreNotificationOverride::Inherit => NotificationOverrideRecord::Inherit,
        StoreNotificationOverride::On => NotificationOverrideRecord::On,
        StoreNotificationOverride::Off => NotificationOverrideRecord::Off,
    }
}

fn store_notification_override(value: NotificationOverrideRecord) -> StoreNotificationOverride {
    match value {
        NotificationOverrideRecord::Inherit => StoreNotificationOverride::Inherit,
        NotificationOverrideRecord::On => StoreNotificationOverride::On,
        NotificationOverrideRecord::Off => StoreNotificationOverride::Off,
    }
}

fn protocol_notification_overrides(
    kinds: StoreNotificationKindOverrides,
) -> NotificationKindOverridesRecord {
    NotificationKindOverridesRecord {
        permission: protocol_notification_override(kinds.permission),
        question: protocol_notification_override(kinds.question),
        failure: protocol_notification_override(kinds.failure),
        completion: protocol_notification_override(kinds.completion),
        stuck: protocol_notification_override(kinds.stuck),
    }
}

fn store_notification_overrides(
    kinds: NotificationKindOverridesRecord,
) -> StoreNotificationKindOverrides {
    StoreNotificationKindOverrides {
        permission: store_notification_override(kinds.permission),
        question: store_notification_override(kinds.question),
        failure: store_notification_override(kinds.failure),
        completion: store_notification_override(kinds.completion),
        stuck: store_notification_override(kinds.stuck),
    }
}

fn protocol_project_notification_master(
    master: StoreProjectNotificationMaster,
) -> ProjectNotificationMasterRecord {
    match master {
        StoreProjectNotificationMaster::Inherit => ProjectNotificationMasterRecord::Inherit,
        StoreProjectNotificationMaster::Off => ProjectNotificationMasterRecord::Off,
    }
}

fn store_project_notification_master(
    master: ProjectNotificationMasterRecord,
) -> StoreProjectNotificationMaster {
    match master {
        ProjectNotificationMasterRecord::Inherit => StoreProjectNotificationMaster::Inherit,
        ProjectNotificationMasterRecord::Off => StoreProjectNotificationMaster::Off,
    }
}

fn protocol_global_notification_policy(
    policy: StoreGlobalNotificationPolicy,
) -> GlobalNotificationPolicyRecord {
    GlobalNotificationPolicyRecord {
        version: policy.version,
        kinds: protocol_notification_kinds(policy.kinds),
        quiet_hours: protocol_quiet_hours(policy.quiet_hours),
    }
}

fn protocol_project_notification_policy(
    policy: StoreProjectNotificationPolicy,
) -> ProjectNotificationPolicyRecord {
    ProjectNotificationPolicyRecord {
        version: policy.version,
        master: protocol_project_notification_master(policy.master),
        kinds: protocol_notification_overrides(policy.kinds),
    }
}

fn protocol_effective_notification_policy(
    policy: StoreEffectiveNotificationPolicy,
) -> EffectiveNotificationPolicyRecord {
    EffectiveNotificationPolicyRecord {
        global_version: policy.global_version,
        project_version: policy.project_version,
        kinds: protocol_notification_kinds(policy.kinds),
        quiet_hours: protocol_quiet_hours(policy.quiet_hours),
    }
}

fn notification_policy_response(
    snapshot: StoreNotificationPolicySnapshot,
) -> NotificationPolicyResponse {
    NotificationPolicyResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        global: protocol_global_notification_policy(snapshot.global),
        project: snapshot.project.map(protocol_project_notification_policy),
        effective: protocol_effective_notification_policy(snapshot.effective),
    }
}

fn notification_policy_request<T: serde::de::DeserializeOwned>(
    request_json: &str,
) -> Result<T, BridgeError> {
    if request_json.len() > MAX_NOTIFICATION_POLICY_REQUEST_BYTES {
        return Err(BridgeError::InvalidNotificationPolicy);
    }
    serde_json::from_str(request_json).map_err(|_| BridgeError::InvalidNotificationPolicy)
}

#[uniffi::export]
pub fn notification_policy_read_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| notification_policy_read_with(&CORE, request_json))
}

fn notification_policy_read_with(
    manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    project_command_json(|| {
        let request: NotificationPolicyReadRequest = notification_policy_request(&request_json)?;
        validate_project_protocol(&request.client_protocol_version)?;
        if let Some(project_id) = request.project_id.as_deref() {
            validate_project_input(project_id, MAX_PROJECT_ID_BYTES)
                .map_err(|_| BridgeError::InvalidNotificationPolicy)?;
        }
        manager.with_ready_core(|core| {
            core.store
                .notification_policy(request.project_id.as_deref())
                .map(notification_policy_response)
                .map_err(map_project_store_error)
        })
    })
}

#[uniffi::export]
pub fn notification_policy_update_global_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| notification_policy_update_global_with(&CORE, request_json))
}

fn notification_policy_update_global_with(
    manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    project_command_json(|| {
        let request: GlobalNotificationPolicyUpdateRequest =
            notification_policy_request(&request_json)?;
        validate_project_protocol(&request.client_protocol_version)?;
        if request.expected_version > flit_protocol::MAX_JSON_SAFE_INTEGER {
            return Err(BridgeError::InvalidNotificationPolicy);
        }
        manager.with_ready_core(|core| {
            core.store
                .update_global_notification_policy(
                    request.expected_version,
                    store_notification_kinds(request.kinds),
                    store_quiet_hours(request.quiet_hours),
                    &request.updated_at,
                )
                .map(notification_policy_response)
                .map_err(map_project_store_error)
        })
    })
}

#[uniffi::export]
pub fn notification_policy_update_project_json(
    request_json: String,
) -> Result<String, BridgeError> {
    protect(|| notification_policy_update_project_with(&CORE, request_json))
}

fn notification_policy_update_project_with(
    manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    project_command_json(|| {
        let request: ProjectNotificationPolicyUpdateRequest =
            notification_policy_request(&request_json)?;
        validate_project_protocol(&request.client_protocol_version)?;
        validate_project_input(&request.project_id, MAX_PROJECT_ID_BYTES)
            .map_err(|_| BridgeError::InvalidNotificationPolicy)?;
        if request.expected_version > flit_protocol::MAX_JSON_SAFE_INTEGER {
            return Err(BridgeError::InvalidNotificationPolicy);
        }
        manager.with_ready_core(|core| {
            core.store
                .update_project_notification_policy(
                    &request.project_id,
                    request.expected_version,
                    store_project_notification_master(request.master),
                    store_notification_overrides(request.kinds),
                    &request.updated_at,
                )
                .map(notification_policy_response)
                .map_err(map_project_store_error)
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

fn observe_bundled_managed_baseline(
    canonical_project_directory: &Path,
) -> Result<(NativeGitObservation, Option<GitChangeBaseline>), GitObservationError> {
    let runner = inspect_noexec_runner_at(bundled_git_runner_path()?)?;
    let git = inspect_git_on_path(std::env::var_os("PATH").as_deref())?;
    let observation = observe_repository(&runner, &git, canonical_project_directory)?;
    let eligible = matches!(
        &observation,
        NativeGitObservation::Repository(receipt)
            if matches!(receipt.head, NativeGitHead::Available(_)) && receipt.dirty.is_clean()
    );
    if !eligible {
        return Ok((observation, None));
    }
    match observe_clean_change_baseline(&runner, &git, canonical_project_directory) {
        Ok(baseline) => Ok((
            NativeGitObservation::Repository(baseline.receipt().clone()),
            Some(baseline),
        )),
        Err(_) => Ok((observation, None)),
    }
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
            attribution,
            files,
            insertions,
            deletions,
        } => flit_protocol::DashboardChangeSummary::Available {
            attribution: match attribution {
                StoreDashboardChangeAttribution::Exact => ProtocolDashboardChangeAttribution::Exact,
                StoreDashboardChangeAttribution::ObservedDuringRun => {
                    ProtocolDashboardChangeAttribution::ObservedDuringRun
                }
            },
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
        active_stuck_occurrence_id: snapshot.active_stuck_occurrence_id,
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
                        category: event.category,
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

fn active_attention_item(
    item: StoreRunActiveAttentionItem,
) -> Result<ProtocolRunActiveAttentionItem, BridgeError> {
    let acknowledgeable = item.category == "failure"
        && item.status == "open"
        && !item.blocking
        && matches!(
            item.source_event_type.as_str(),
            "run.failed" | "run.interrupted" | "run.resume_failed"
        )
        && matches!(
            &item.action,
            StoreRunActiveAttentionAction::Unavailable { reason }
                if reason == "attention_action_not_implemented"
        );
    let category = match item.category.as_str() {
        "permission" => ProtocolRunActiveAttentionCategory::Permission,
        "permission_audit" => ProtocolRunActiveAttentionCategory::PermissionAudit,
        "question" => ProtocolRunActiveAttentionCategory::Question,
        "risk" => ProtocolRunActiveAttentionCategory::Risk,
        "failure" => ProtocolRunActiveAttentionCategory::Failure,
        "stuck" => ProtocolRunActiveAttentionCategory::Stuck,
        "system" => ProtocolRunActiveAttentionCategory::System,
        "completion" => ProtocolRunActiveAttentionCategory::Completion,
        _ => return Err(BridgeError::StorageFailure),
    };
    let severity = match item.severity.as_str() {
        "Informational" => ProtocolRunActiveAttentionSeverity::Informational,
        "ActionRequired" => ProtocolRunActiveAttentionSeverity::ActionRequired,
        "Critical" => ProtocolRunActiveAttentionSeverity::Critical,
        _ => return Err(BridgeError::StorageFailure),
    };
    let status = match item.status.as_str() {
        "open" => ProtocolRunActiveAttentionStatus::Open,
        "response_pending" => ProtocolRunActiveAttentionStatus::ResponsePending,
        "delivery_unknown" => ProtocolRunActiveAttentionStatus::DeliveryUnknown,
        _ => return Err(BridgeError::StorageFailure),
    };
    let action = if acknowledgeable {
        ProtocolRunActiveAttentionAction::Acknowledge
    } else {
        match item.action {
            StoreRunActiveAttentionAction::PermissionResponse {
                request_id,
                request_version,
            } => ProtocolRunActiveAttentionAction::PermissionResponse {
                request_id,
                request_version,
            },
            StoreRunActiveAttentionAction::StillWorking { occurrence_id } => {
                ProtocolRunActiveAttentionAction::StillWorking { occurrence_id }
            }
            StoreRunActiveAttentionAction::Unavailable { reason } => {
                ProtocolRunActiveAttentionAction::Unavailable { reason }
            }
        }
    };
    Ok(ProtocolRunActiveAttentionItem {
        attention_id: item.attention_id,
        attention_version: item.attention_version,
        category,
        severity,
        blocking: item.blocking,
        status,
        source_event_id: item.source_event_id,
        source_event_type: item.source_event_type,
        source_observed_at: item.source_observed_at,
        content_unavailable_reason: item.content_unavailable_reason,
        action,
    })
}

fn run_active_attention_read_with(
    core_manager: &CoreManager,
    request_json: &str,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return run_command_json(|| -> Result<RunActiveAttentionReadResponse, _> {
            Err(BridgeError::InvalidRunRequest)
        });
    }
    let request = serde_json::from_str::<RunActiveAttentionReadRequest>(request_json)
        .map_err(|_| BridgeError::InvalidRunRequest);
    run_command_json(|| {
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
                .managed_run_active_attention_context(&request.run_id)
                .map_err(|error| match error {
                    StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                    _ => BridgeError::StorageFailure,
                })?;
            if context.run_version != request.expected_run_version {
                return Err(BridgeError::RunVersionStale);
            }
            let item = match context.item {
                Some(item) => RunActiveAttentionSlot::Item(active_attention_item(item)?),
                None => RunActiveAttentionSlot::Null,
            };
            Ok(RunActiveAttentionReadResponse {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                event_schema_version: EVENT_PROTOCOL_VERSION.to_owned(),
                run_id: request.run_id,
                run_version: context.run_version,
                open_count: context.open_count,
                item,
            })
        })
    })
}

#[uniffi::export]
pub fn run_active_attention_read_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| run_active_attention_read_with(&CORE, &request_json))
}

fn attention_acknowledge_rejected_response(
    request: &AttentionAcknowledgeRequest,
    reason: AttentionAcknowledgeRejectedReason,
) -> AttentionAcknowledgeResponse {
    AttentionAcknowledgeResponse::Rejected {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: request.run_id.clone(),
        expected_run_version: request.expected_run_version,
        attention_id: request.attention_id.clone(),
        attention_version: request.attention_version,
        reason,
    }
}

fn attention_acknowledge_rejected_reason(
    reason: StoreAttentionAcknowledgeRejectedReason,
) -> AttentionAcknowledgeRejectedReason {
    match reason {
        StoreAttentionAcknowledgeRejectedReason::RunVersionStale => {
            AttentionAcknowledgeRejectedReason::RunVersionStale
        }
        StoreAttentionAcknowledgeRejectedReason::AttentionMismatch => {
            AttentionAcknowledgeRejectedReason::AttentionMismatch
        }
        StoreAttentionAcknowledgeRejectedReason::NotAcknowledgeable => {
            AttentionAcknowledgeRejectedReason::NotAcknowledgeable
        }
        StoreAttentionAcknowledgeRejectedReason::AlreadyApplied => {
            AttentionAcknowledgeRejectedReason::AlreadyApplied
        }
    }
}

fn attention_acknowledge_event_id(request: &AttentionAcknowledgeRequest) -> String {
    let mut digest = Sha256::new();
    for part in [
        request.run_id.as_str(),
        &request.expected_run_version.to_string(),
        request.attention_id.as_str(),
        &request.attention_version.to_string(),
    ] {
        digest.update(part.len().to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("attention-acknowledged-{:x}", digest.finalize())
}

fn attention_acknowledge_with(
    core_manager: &CoreManager,
    request_json: &str,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return run_command_json(|| -> Result<AttentionAcknowledgeResponse, _> {
            Err(BridgeError::InvalidRunRequest)
        });
    }
    let request = serde_json::from_str::<AttentionAcknowledgeRequest>(request_json)
        .map_err(|_| BridgeError::InvalidRunRequest);
    run_command_json(|| {
        let request = request?;
        if request.client_protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::ProtocolMismatch);
        }
        if request.run_id.trim().is_empty()
            || request.run_id.len() > MAX_PROJECT_ID_BYTES
            || request.run_id.chars().any(char::is_control)
            || request.attention_id.trim().is_empty()
            || request.attention_id.len() > 256
            || request.attention_id.chars().any(char::is_control)
            || request.expected_run_version == 0
            || request.expected_run_version > flit_protocol::MAX_JSON_SAFE_INTEGER
            || request.attention_version == 0
            || request.attention_version > flit_protocol::MAX_JSON_SAFE_INTEGER
        {
            return Err(BridgeError::InvalidRunRequest);
        }
        core_manager.with_ready_core(|core| {
            let observed_at = core
                .store
                .current_utc_timestamp()
                .map_err(|_| BridgeError::StorageFailure)?;
            let outcome = core
                .store
                .acknowledge_managed_attention(ManagedAttentionAcknowledgeAction {
                    run_id: request.run_id.clone(),
                    expected_run_version: request.expected_run_version,
                    attention_id: request.attention_id.clone(),
                    attention_version: request.attention_version,
                    event_id: attention_acknowledge_event_id(&request),
                    observed_at,
                })
                .map_err(|error| match error {
                    StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                    StoreError::InvalidManagedAttentionAcknowledge { .. } => {
                        BridgeError::InvalidRunRequest
                    }
                    _ => BridgeError::StorageFailure,
                })?;
            match outcome {
                ManagedAttentionAcknowledgeOutcome::Applied(event) => {
                    Ok(AttentionAcknowledgeResponse::Applied {
                        protocol_version: PROTOCOL_VERSION.to_owned(),
                        run_id: request.run_id,
                        previous_version: request.expected_run_version,
                        event_id: event.event_id.clone(),
                        event_version: event.ingest_seq,
                        attention_id: request.attention_id,
                        attention_version: request.attention_version,
                    })
                }
                ManagedAttentionAcknowledgeOutcome::Rejected { reason, .. } => {
                    Ok(attention_acknowledge_rejected_response(
                        &request,
                        attention_acknowledge_rejected_reason(reason),
                    ))
                }
            }
        })
    })
}

#[uniffi::export]
pub fn attention_acknowledge_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| attention_acknowledge_with(&CORE, &request_json))
}

fn run_change_head(oid: Option<String>) -> RunChangeHead {
    match oid {
        Some(oid) => RunChangeHead::Available { oid },
        None => RunChangeHead::Unavailable,
    }
}

fn run_changes_read_with(
    core_manager: &CoreManager,
    request_json: &str,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return run_command_json(|| -> Result<RunChangesReadResponse, _> {
            Err(BridgeError::InvalidRunRequest)
        });
    }
    let request = serde_json::from_str::<RunChangesReadRequest>(request_json)
        .map_err(|_| BridgeError::InvalidRunRequest);
    run_command_json(|| {
        let request = request?;
        if request.client_protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::ProtocolMismatch);
        }
        let limit = usize::try_from(request.requested_change_limit)
            .map_err(|_| BridgeError::InvalidRunRequest)?;
        if request.run_id.trim().is_empty()
            || request.run_id.len() > MAX_PROJECT_ID_BYTES
            || request.run_id.contains('\0')
            || request.expected_run_version == 0
            || request.expected_run_version > flit_protocol::MAX_JSON_SAFE_INTEGER
            || !(1..=MAX_MANAGED_GIT_CHANGE_PAGE_SIZE).contains(&limit)
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
            let Some(page) = core
                .store
                .managed_git_change_page(&request.run_id, request.after_cursor.as_deref(), limit)
                .map_err(|error| match error {
                    StoreError::InvalidManagedGitChangeRead { .. } => {
                        BridgeError::InvalidRunRequest
                    }
                    StoreError::ManagedGitChangeReadTooLarge { .. } => {
                        BridgeError::ManagedRunResponseTooLarge
                    }
                    _ => BridgeError::StorageFailure,
                })?
            else {
                return Ok(RunChangesReadResponse::Unavailable {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: request.run_id,
                    run_version: context.run_version,
                    reason: RunChangesUnavailableReason::ChangeSetNotAvailable,
                });
            };
            let attribution = match page.metadata.attribution {
                StoreManagedGitChangeAttribution::Exact => {
                    ProtocolDashboardChangeAttribution::Exact
                }
                StoreManagedGitChangeAttribution::ObservedDuringRun => {
                    ProtocolDashboardChangeAttribution::ObservedDuringRun
                }
            };
            let changes = page
                .changes
                .into_iter()
                .map(|change| RunFileChangeRecord {
                    change_id: change.change_id,
                    display_path: change.display_path,
                    status: match change.status {
                        StoreManagedGitFileStatus::Added => RunFileChangeStatus::Added,
                        StoreManagedGitFileStatus::Modified => RunFileChangeStatus::Modified,
                        StoreManagedGitFileStatus::Deleted => RunFileChangeStatus::Deleted,
                        StoreManagedGitFileStatus::TypeChanged => RunFileChangeStatus::TypeChanged,
                        StoreManagedGitFileStatus::Untracked => RunFileChangeStatus::Untracked,
                    },
                    committed: change.committed,
                    staged: change.staged,
                    unstaged: change.unstaged,
                    binary: change.binary,
                    insertions: change.insertions,
                    deletions: change.deletions,
                    project_scope: match change.project_scope {
                        StoreManagedGitProjectScope::InsideProject => {
                            RunFileProjectScope::InsideProject
                        }
                        StoreManagedGitProjectScope::OutsideProject => {
                            RunFileProjectScope::OutsideProject
                        }
                    },
                })
                .collect();
            Ok(RunChangesReadResponse::Available {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: request.run_id,
                run_version: context.run_version,
                attribution,
                baseline_head: run_change_head(page.metadata.baseline_head),
                terminal_head: run_change_head(page.metadata.terminal_head),
                next_cursor: page.next_cursor,
                has_more: page.has_more,
                changes,
            })
        })
    })
}

#[uniffi::export]
pub fn run_changes_read_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| run_changes_read_with(&CORE, &request_json))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ExternalOpenAuthorityState {
    Available(Box<ExternalOpenAuthority>),
    Disabled(RunChangeExternalOpenDisabledReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExternalOpenAuthorityRead {
    run_version: u64,
    state: ExternalOpenAuthorityState,
}

fn load_external_open_authority(
    core_manager: &CoreManager,
    run_id: &str,
    expected_run_version: u64,
    change_id: &str,
) -> Result<ExternalOpenAuthorityRead, BridgeError> {
    core_manager.with_ready_core(|core| {
        let context =
            core.store
                .managed_run_detail_context(run_id)
                .map_err(|error| match error {
                    StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                    _ => BridgeError::StorageFailure,
                })?;
        if context.run_version != expected_run_version {
            return Err(BridgeError::RunVersionStale);
        }
        let run = core
            .store
            .managed_run(run_id)
            .map_err(|_| BridgeError::StorageFailure)?
            .ok_or(BridgeError::RunNotFound)?;
        let Some(project) = core
            .store
            .project(&run.project_id)
            .map_err(|_| BridgeError::StorageFailure)?
        else {
            return Ok(ExternalOpenAuthorityRead {
                run_version: context.run_version,
                state: ExternalOpenAuthorityState::Disabled(
                    RunChangeExternalOpenDisabledReason::ProjectIdentityMismatch,
                ),
            });
        };
        let Some(project_filesystem_id) = project.filesystem_id.clone() else {
            return Ok(ExternalOpenAuthorityRead {
                run_version: context.run_version,
                state: ExternalOpenAuthorityState::Disabled(
                    RunChangeExternalOpenDisabledReason::ProjectIdentityMismatch,
                ),
            });
        };
        if !project.trusted {
            return Ok(ExternalOpenAuthorityRead {
                run_version: context.run_version,
                state: ExternalOpenAuthorityState::Disabled(
                    RunChangeExternalOpenDisabledReason::ProjectIdentityMismatch,
                ),
            });
        }
        let Some(metadata) = core
            .store
            .managed_git_change_set_metadata(run_id)
            .map_err(|_| BridgeError::StorageFailure)?
        else {
            return Ok(ExternalOpenAuthorityRead {
                run_version: context.run_version,
                state: ExternalOpenAuthorityState::Disabled(
                    RunChangeExternalOpenDisabledReason::ChangeSetNotAvailable,
                ),
            });
        };
        let Some(change) = core
            .store
            .managed_git_file_change(run_id, change_id)
            .map_err(|_| BridgeError::StorageFailure)?
        else {
            return Ok(ExternalOpenAuthorityRead {
                run_version: context.run_version,
                state: ExternalOpenAuthorityState::Disabled(
                    RunChangeExternalOpenDisabledReason::ChangeNotFound,
                ),
            });
        };
        Ok(ExternalOpenAuthorityRead {
            run_version: context.run_version,
            state: ExternalOpenAuthorityState::Available(Box::new(external_open_authority(
                project,
                project_filesystem_id,
                metadata,
                change,
            ))),
        })
    })
}

fn external_open_authority(
    project: Project,
    project_filesystem_id: String,
    metadata: StoreManagedGitChangeSetMetadata,
    change: StoreManagedGitFileChange,
) -> ExternalOpenAuthority {
    ExternalOpenAuthority {
        project_path: project.canonical_path,
        project_filesystem_id,
        repository_identity: metadata.repository_identity,
        raw_path: change.raw_path,
        status: change.status,
        project_scope: change.project_scope,
    }
}

fn external_open_disabled_reason(
    error: ExternalOpenGuardError,
) -> RunChangeExternalOpenDisabledReason {
    match error {
        ExternalOpenGuardError::DeletedChange => RunChangeExternalOpenDisabledReason::DeletedChange,
        ExternalOpenGuardError::OutsideProject => {
            RunChangeExternalOpenDisabledReason::OutsideProject
        }
        ExternalOpenGuardError::ProjectIdentityMismatch => {
            RunChangeExternalOpenDisabledReason::ProjectIdentityMismatch
        }
        ExternalOpenGuardError::RepositoryIdentityMismatch => {
            RunChangeExternalOpenDisabledReason::RepositoryIdentityMismatch
        }
        ExternalOpenGuardError::TargetUnavailable => {
            RunChangeExternalOpenDisabledReason::TargetUnavailable
        }
        ExternalOpenGuardError::SymlinkEscape => RunChangeExternalOpenDisabledReason::SymlinkEscape,
        ExternalOpenGuardError::TargetNotFile => RunChangeExternalOpenDisabledReason::TargetNotFile,
    }
}

fn external_open_response(
    request: &RunChangeExternalOpenRequest,
    run_version: u64,
    reason: Option<RunChangeExternalOpenDisabledReason>,
) -> RunChangeExternalOpenResponse {
    match reason {
        Some(reason) => RunChangeExternalOpenResponse::Disabled {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: request.run_id.clone(),
            run_version,
            change_id: request.change_id.clone(),
            reason,
        },
        None => RunChangeExternalOpenResponse::Opened {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: request.run_id.clone(),
            run_version,
            change_id: request.change_id.clone(),
        },
    }
}

fn valid_external_open_change_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn run_change_external_open_with<Inspect, Open>(
    core_manager: &CoreManager,
    request_json: &str,
    mut inspect: Inspect,
    open: Open,
) -> Result<String, BridgeError>
where
    Inspect: FnMut(&ExternalOpenAuthority) -> Result<ExternalOpenTarget, ExternalOpenGuardError>,
    Open: FnOnce(&Path) -> Result<(), ()>,
{
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return run_command_json(|| -> Result<RunChangeExternalOpenResponse, _> {
            Err(BridgeError::InvalidRunRequest)
        });
    }
    let request = serde_json::from_str::<RunChangeExternalOpenRequest>(request_json)
        .map_err(|_| BridgeError::InvalidRunRequest);
    run_command_json(|| -> Result<RunChangeExternalOpenResponse, BridgeError> {
        let request = request?;
        if request.client_protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::ProtocolMismatch);
        }
        if request.run_id.trim().is_empty()
            || request.run_id.len() > MAX_PROJECT_ID_BYTES
            || request.run_id.contains('\0')
            || request.expected_run_version == 0
            || request.expected_run_version > flit_protocol::MAX_JSON_SAFE_INTEGER
            || !valid_external_open_change_id(&request.change_id)
        {
            return Err(BridgeError::InvalidRunRequest);
        }

        let first = load_external_open_authority(
            core_manager,
            &request.run_id,
            request.expected_run_version,
            &request.change_id,
        )?;
        let first_authority = match first.state {
            ExternalOpenAuthorityState::Available(authority) => authority,
            ExternalOpenAuthorityState::Disabled(reason) => {
                return Ok(external_open_response(
                    &request,
                    first.run_version,
                    Some(reason),
                ));
            }
        };
        let first_target = match inspect(&first_authority) {
            Ok(target) => target,
            Err(error) => {
                return Ok(external_open_response(
                    &request,
                    first.run_version,
                    Some(external_open_disabled_reason(error)),
                ));
            }
        };

        let second = load_external_open_authority(
            core_manager,
            &request.run_id,
            request.expected_run_version,
            &request.change_id,
        )?;
        let second_authority = match second.state {
            ExternalOpenAuthorityState::Available(authority) => authority,
            ExternalOpenAuthorityState::Disabled(_) => {
                return Ok(external_open_response(
                    &request,
                    second.run_version,
                    Some(RunChangeExternalOpenDisabledReason::TargetIdentityDrift),
                ));
            }
        };
        if second_authority != first_authority {
            return Ok(external_open_response(
                &request,
                second.run_version,
                Some(RunChangeExternalOpenDisabledReason::TargetIdentityDrift),
            ));
        }
        let second_target = match inspect(&second_authority) {
            Ok(target) => target,
            Err(error) => {
                return Ok(external_open_response(
                    &request,
                    second.run_version,
                    Some(external_open_disabled_reason(error)),
                ));
            }
        };
        if second_target != first_target {
            return Ok(external_open_response(
                &request,
                second.run_version,
                Some(RunChangeExternalOpenDisabledReason::TargetIdentityDrift),
            ));
        }
        let reason = open(second_target.canonical_path())
            .err()
            .map(|()| RunChangeExternalOpenDisabledReason::OpenFailed);
        Ok(external_open_response(&request, second.run_version, reason))
    })
}

#[uniffi::export]
pub fn run_change_external_open_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| {
        run_change_external_open_with(
            &CORE,
            &request_json,
            inspect_external_open_target,
            open_with_default_application,
        )
    })
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
                observe_bundled_managed_baseline,
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
    observe: impl FnOnce(
        &Path,
    ) -> Result<
        (NativeGitObservation, Option<GitChangeBaseline>),
        GitObservationError,
    >,
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
    let (observation, exact_baseline) = match observe(&before.canonical_path) {
        Ok((observation, exact_baseline)) => (Ok(observation), exact_baseline),
        Err(error) => (Err(error), None),
    };
    let baseline = match managed_git_baseline(&request.project_id, observation) {
        Ok(baseline) => baseline,
        Err(error) => return managed_start_bridge_error_json(error),
    };
    let retained_baseline = retained_git_change_baseline(&baseline, exact_baseline);
    let after = match git_project_target(core_manager, &request.project_id) {
        Ok(target) => target,
        Err(error) => return managed_start_bridge_error_json(error),
    };
    if after != before {
        return managed_start_bridge_error_json(BridgeError::ProjectIdentityMismatch);
    }

    core_manager.with_ready_core(|core| {
        start_managed_run_in_core(
            core,
            connector,
            path_environment,
            baseline,
            retained_baseline,
            request,
        )
    })
}

fn retained_git_change_baseline(
    baseline: &GitBaselinePayload,
    exact: Option<GitChangeBaseline>,
) -> managed_start::RetainedGitChangeBaseline {
    if let Some(exact) = exact {
        return managed_start::RetainedGitChangeBaseline::Exact(Box::new(exact));
    }
    let reason = match baseline {
        GitBaselinePayload::Available {
            head: GitHead::Unborn,
            ..
        } => "git_baseline_head_unavailable",
        GitBaselinePayload::Available { dirty, .. }
            if dirty.staged != 0
                || dirty.unstaged != 0
                || dirty.untracked != 0
                || dirty.entries != 0 =>
        {
            "git_baseline_not_clean"
        }
        GitBaselinePayload::Available { .. } => "git_baseline_exact_capture_unavailable",
        GitBaselinePayload::Unavailable { .. } => "git_baseline_observation_unavailable",
    };
    managed_start::RetainedGitChangeBaseline::Unavailable(reason.to_owned())
}

fn managed_start_bridge_error_json(error: BridgeError) -> Result<String, BridgeError> {
    match project_command_error(&error) {
        Some(command_error) => project_json(&command_error),
        None => Err(error),
    }
}

#[uniffi::export]
pub fn managed_runs_assess_stuck_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| managed_runs_assess_stuck_with(&CORE, request_json))
}

fn managed_runs_assess_stuck_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request = match serde_json::from_str::<ManagedRunsAssessStuckRequest>(&request_json) {
        Ok(request) => request,
        Err(_) => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    let assessed = core_manager.with_ready_core(|core| {
        let elapsed = core.stuck_assessment.clock_origin.elapsed().as_millis();
        let now_monotonic_ms = u64::try_from(elapsed)
            .unwrap_or(flit_protocol::MAX_JSON_SAFE_INTEGER)
            .min(flit_protocol::MAX_JSON_SAFE_INTEGER);
        let observed_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        stuck_assessment::assess_managed_runs(
            &mut core.store,
            &mut core.stuck_assessment.progress_baselines,
            &mut core.stuck_assessment.process_probes,
            now_monotonic_ms,
            &observed_at,
        )
        .map_err(|_| BridgeError::StorageFailure)
    });
    match assessed {
        Ok(response) => bounded_json(
            &response,
            MAX_PROJECT_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
            CommandErrorCode::StorageUnavailable,
        )),
        Err(error) => Err(error),
    }
}

#[uniffi::export]
pub fn notification_deliveries_due_read_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| notification_deliveries_due_read_with(&CORE, request_json))
}

fn notification_deliveries_due_read_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    let request = match notification_delivery_request::<NotificationDeliveriesDueReadRequest>(
        &request_json,
    ) {
        Ok(request) => request,
        Err(_) => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    if request.local_minute >= 24 * 60 {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let response = core_manager.with_ready_core(|core| {
        let evaluated_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        let notifications = core
            .store
            .reconcile_notification_deliveries(request.local_minute, &evaluated_at)
            .map_err(map_notification_delivery_store_error)?
            .into_iter()
            .map(|candidate| NotificationDeliveryRecord {
                notification_id: candidate.notification_id,
                run_id: candidate.run_id,
                run_version: candidate.run_version,
                project_id: candidate.project_id,
                kind: protocol_notification_kind(candidate.kind),
                item_id: candidate.item_id,
                item_version: candidate.item_version,
                platform_id: candidate.platform_id,
                delivery_claimed: candidate.delivery_claimed,
                catch_up: candidate.catch_up,
            })
            .collect();
        Ok(NotificationDeliveriesDueReadResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            notifications,
        })
    });
    notification_delivery_command_response(response)
}

#[uniffi::export]
pub fn notification_delivery_claim_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| notification_delivery_claim_with(&CORE, request_json))
}

fn notification_delivery_claim_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    let request =
        match notification_delivery_request::<NotificationDeliveryClaimRequest>(&request_json) {
            Ok(request) => request,
            Err(_) => {
                return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
            }
        };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    if !valid_notification_delivery_identity(
        &request.notification_id,
        &request.run_id,
        &request.item_id,
        &request.platform_id,
        request.item_version,
    ) || request.expected_run_version == 0
        || request.expected_run_version > flit_protocol::MAX_JSON_SAFE_INTEGER
        || request.local_minute >= 24 * 60
    {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let response = core_manager.with_ready_core(|core| {
        let claimed_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        let outcome = core
            .store
            .claim_notification_delivery(StoreNotificationDeliveryClaim {
                notification_id: request.notification_id.clone(),
                run_id: request.run_id.clone(),
                expected_run_version: request.expected_run_version,
                kind: store_notification_kind(request.kind),
                item_id: request.item_id.clone(),
                item_version: request.item_version,
                platform_id: request.platform_id.clone(),
                local_minute: request.local_minute,
                claimed_at,
            })
            .map_err(map_notification_delivery_store_error)?;
        Ok(NotificationDeliveryClaimResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            notification_id: request.notification_id,
            run_id: request.run_id,
            run_version: request.expected_run_version,
            kind: request.kind,
            item_id: request.item_id,
            item_version: request.item_version,
            platform_id: request.platform_id,
            already_claimed: matches!(
                outcome,
                StoreNotificationDeliveryClaimOutcome::AlreadyClaimed
            ),
        })
    });
    notification_delivery_command_response(response)
}

#[uniffi::export]
pub fn notification_delivery_failed_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| notification_delivery_failed_with(&CORE, request_json))
}

fn notification_delivery_failed_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    let request =
        match notification_delivery_request::<NotificationDeliveryFailedRequest>(&request_json) {
            Ok(request) => request,
            Err(_) => {
                return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
            }
        };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    if !valid_notification_delivery_identity(
        &request.notification_id,
        &request.run_id,
        &request.item_id,
        &request.platform_id,
        request.item_version,
    ) {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let response = core_manager.with_ready_core(|core| {
        let outcome = core
            .store
            .release_notification_delivery(StoreNotificationDeliveryFailure {
                notification_id: request.notification_id.clone(),
                run_id: request.run_id.clone(),
                kind: store_notification_kind(request.kind),
                item_id: request.item_id.clone(),
                item_version: request.item_version,
                platform_id: request.platform_id.clone(),
            })
            .map_err(map_notification_delivery_store_error)?;
        Ok(NotificationDeliveryFailedResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            notification_id: request.notification_id,
            run_id: request.run_id,
            kind: request.kind,
            item_id: request.item_id,
            item_version: request.item_version,
            platform_id: request.platform_id,
            released: matches!(outcome, StoreNotificationDeliveryFailureOutcome::Released),
        })
    });
    notification_delivery_command_response(response)
}

#[uniffi::export]
pub fn notification_delivered_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| notification_delivered_with(&CORE, request_json))
}

fn notification_delivered_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    let request = match notification_delivery_request::<NotificationDeliveredRequest>(&request_json)
    {
        Ok(request) => request,
        Err(_) => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    if !valid_notification_delivery_identity(
        &request.notification_id,
        &request.run_id,
        &request.item_id,
        &request.platform_id,
        request.item_version,
    ) {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let response = core_manager.with_ready_core(|core| {
        let delivered_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        let outcome = core
            .store
            .record_notification_delivery(StoreNotificationDeliveryReceipt {
                notification_id: request.notification_id.clone(),
                run_id: request.run_id.clone(),
                kind: store_notification_kind(request.kind),
                item_id: request.item_id.clone(),
                item_version: request.item_version,
                platform_id: request.platform_id.clone(),
                delivered_at,
            })
            .map_err(map_notification_delivery_store_error)?;
        Ok(NotificationDeliveredResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            notification_id: request.notification_id,
            run_id: request.run_id,
            kind: request.kind,
            item_id: request.item_id,
            item_version: request.item_version,
            platform_id: request.platform_id,
            already_delivered: matches!(
                outcome,
                StoreNotificationDeliveryReceiptOutcome::AlreadyDelivered
            ),
        })
    });
    notification_delivery_command_response(response)
}

fn notification_delivery_request<T: serde::de::DeserializeOwned>(
    request_json: &str,
) -> Result<T, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return Err(BridgeError::InvalidRunRequest);
    }
    serde_json::from_str(request_json).map_err(|_| BridgeError::InvalidRunRequest)
}

fn notification_delivery_command_response<T: serde::Serialize>(
    response: Result<T, BridgeError>,
) -> Result<String, BridgeError> {
    match response {
        Ok(response) => bounded_json(
            &response,
            MAX_PROJECT_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(BridgeError::InvalidRunRequest) => {
            project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest))
        }
        Err(BridgeError::RunVersionStale) => {
            project_json(&CommandError::for_code(CommandErrorCode::RunVersionStale))
        }
        Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
            CommandErrorCode::StorageUnavailable,
        )),
        Err(error) => Err(error),
    }
}

fn map_notification_delivery_store_error(error: StoreError) -> BridgeError {
    match error {
        StoreError::InvalidNotificationDelivery { .. }
        | StoreError::NotificationDeliveryIdentityMismatch { .. }
        | StoreError::NotificationDeliveryUnclaimed { .. } => BridgeError::InvalidRunRequest,
        StoreError::NotificationDeliveryUnavailable { .. } => BridgeError::RunVersionStale,
        _ => BridgeError::StorageFailure,
    }
}

fn protocol_notification_kind(kind: StoreNotificationKind) -> ProtocolNotificationKind {
    match kind {
        StoreNotificationKind::Permission => ProtocolNotificationKind::Permission,
        StoreNotificationKind::Question => ProtocolNotificationKind::Question,
        StoreNotificationKind::Failure => ProtocolNotificationKind::Failure,
        StoreNotificationKind::Completion => ProtocolNotificationKind::Completion,
        StoreNotificationKind::Stuck => ProtocolNotificationKind::Stuck,
    }
}

fn store_notification_kind(kind: ProtocolNotificationKind) -> StoreNotificationKind {
    match kind {
        ProtocolNotificationKind::Permission => StoreNotificationKind::Permission,
        ProtocolNotificationKind::Question => StoreNotificationKind::Question,
        ProtocolNotificationKind::Failure => StoreNotificationKind::Failure,
        ProtocolNotificationKind::Completion => StoreNotificationKind::Completion,
        ProtocolNotificationKind::Stuck => StoreNotificationKind::Stuck,
    }
}

fn valid_notification_delivery_identity(
    notification_id: &str,
    run_id: &str,
    item_id: &str,
    platform_id: &str,
    item_version: u64,
) -> bool {
    valid_bounded_notification_token(notification_id, 96)
        && valid_bounded_notification_token(run_id, 256)
        && valid_bounded_notification_token(item_id, 256)
        && valid_bounded_notification_token(platform_id, 256)
        && item_version > 0
        && item_version <= flit_protocol::MAX_JSON_SAFE_INTEGER
}

fn valid_bounded_notification_token(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

#[uniffi::export]
pub fn managed_stuck_notifications_due_read_json(
    request_json: String,
) -> Result<String, BridgeError> {
    protect(|| managed_stuck_notifications_due_read_with(&CORE, request_json))
}

fn managed_stuck_notifications_due_read_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request =
        match serde_json::from_str::<ManagedStuckNotificationsDueReadRequest>(&request_json) {
            Ok(request) => request,
            Err(_) => {
                return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
            }
        };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    let response = core_manager.with_ready_core(|core| {
        let notifications = core
            .store
            .managed_stuck_notification_due_contexts()
            .map_err(|_| BridgeError::StorageFailure)?
            .into_iter()
            .map(|context| ManagedStuckNotificationDueRecord {
                run_id: context.run_id,
                run_version: context.run_version,
                occurrence_id: context.occurrence_id,
                platform_id: context.platform_id,
                delivery_claimed: context.delivery_claimed,
            })
            .collect();
        Ok(ManagedStuckNotificationsDueReadResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            event_schema_version: EVENT_PROTOCOL_VERSION.to_owned(),
            notifications,
        })
    });
    match response {
        Ok(response) => bounded_json(
            &response,
            MAX_PROJECT_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
            CommandErrorCode::StorageUnavailable,
        )),
        Err(error) => Err(error),
    }
}

#[uniffi::export]
pub fn managed_stuck_notification_delivery_claim_json(
    request_json: String,
) -> Result<String, BridgeError> {
    protect(|| managed_stuck_notification_delivery_claim_with(&CORE, request_json))
}

fn managed_stuck_notification_delivery_claim_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request =
        match serde_json::from_str::<ManagedStuckNotificationDeliveryClaimRequest>(&request_json) {
            Ok(request)
                if valid_stuck_notification_token(&request.run_id)
                    && valid_stuck_notification_token(&request.occurrence_id)
                    && request.expected_run_version > 0
                    && request.expected_run_version <= flit_protocol::MAX_JSON_SAFE_INTEGER =>
            {
                request
            }
            _ => {
                return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
            }
        };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    let response = core_manager.with_ready_core(|core| {
        let claimed_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        let outcome = core
            .store
            .claim_managed_stuck_notification_delivery(ManagedStuckNotificationDeliveryClaim {
                run_id: request.run_id.clone(),
                expected_run_version: request.expected_run_version,
                occurrence_id: request.occurrence_id.clone(),
                platform_id: request.occurrence_id.clone(),
                claimed_at,
            })
            .map_err(|error| match error {
                StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                _ => BridgeError::StorageFailure,
            })?;
        Ok(ManagedStuckNotificationDeliveryClaimResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: request.run_id,
            run_version: request.expected_run_version,
            occurrence_id: request.occurrence_id.clone(),
            platform_id: request.occurrence_id,
            already_claimed: matches!(
                outcome,
                ManagedStuckNotificationDeliveryClaimOutcome::AlreadyClaimed
            ),
        })
    });
    managed_stuck_notification_command_response(response)
}

#[uniffi::export]
pub fn managed_stuck_notification_delivery_failed_json(
    request_json: String,
) -> Result<String, BridgeError> {
    protect(|| managed_stuck_notification_delivery_failed_with(&CORE, request_json))
}

fn managed_stuck_notification_delivery_failed_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request = match serde_json::from_str::<ManagedStuckNotificationDeliveryFailedRequest>(
        &request_json,
    ) {
        Ok(request)
            if valid_stuck_notification_token(&request.run_id)
                && valid_stuck_notification_token(&request.occurrence_id)
                && valid_stuck_notification_token(&request.platform_id)
                && request.platform_id == request.occurrence_id
                && request.expected_run_version > 0
                && request.expected_run_version <= flit_protocol::MAX_JSON_SAFE_INTEGER =>
        {
            request
        }
        _ => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    let response = core_manager.with_ready_core(|core| {
        let outcome = core
            .store
            .release_managed_stuck_notification_delivery(ManagedStuckNotificationDeliveryFailure {
                run_id: request.run_id.clone(),
                expected_run_version: request.expected_run_version,
                occurrence_id: request.occurrence_id.clone(),
                platform_id: request.platform_id.clone(),
            })
            .map_err(|error| match error {
                StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                _ => BridgeError::StorageFailure,
            })?;
        Ok(ManagedStuckNotificationDeliveryFailedResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            run_id: request.run_id,
            run_version: request.expected_run_version,
            occurrence_id: request.occurrence_id,
            platform_id: request.platform_id,
            released: matches!(
                outcome,
                ManagedStuckNotificationDeliveryFailureOutcome::Released
            ),
        })
    });
    managed_stuck_notification_command_response(response)
}

fn managed_stuck_notification_command_response<T: serde::Serialize>(
    response: Result<T, BridgeError>,
) -> Result<String, BridgeError> {
    match response {
        Ok(response) => bounded_json(
            &response,
            MAX_PROJECT_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(BridgeError::RunNotFound) => {
            project_json(&CommandError::for_code(CommandErrorCode::RunNotFound))
        }
        Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
            CommandErrorCode::StorageUnavailable,
        )),
        Err(error) => Err(error),
    }
}

#[uniffi::export]
pub fn managed_stuck_notification_delivered_json(
    request_json: String,
) -> Result<String, BridgeError> {
    protect(|| managed_stuck_notification_delivered_with(&CORE, request_json))
}

fn managed_stuck_notification_delivered_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request =
        match serde_json::from_str::<ManagedStuckNotificationDeliveredRequest>(&request_json) {
            Ok(request)
                if valid_stuck_notification_token(&request.run_id)
                    && valid_stuck_notification_token(&request.occurrence_id)
                    && valid_stuck_notification_token(&request.platform_id)
                    && request.platform_id == request.occurrence_id
                    && request.expected_run_version > 0
                    && request.expected_run_version <= flit_protocol::MAX_JSON_SAFE_INTEGER =>
            {
                request
            }
            _ => {
                return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
            }
        };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    let response = core_manager.with_ready_core(|core| {
        let event_id = stuck_notification_delivered_event_id(&request);
        if let Some(event) = core
            .store
            .managed_stuck_notification_delivery_receipt(
                &request.run_id,
                &event_id,
                &request.occurrence_id,
                &request.platform_id,
            )
            .map_err(|_| BridgeError::StorageFailure)?
        {
            return Ok(ManagedStuckNotificationDeliveredResponse::Delivered {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                run_id: request.run_id,
                previous_version: request.expected_run_version,
                event_id: event.event_id,
                event_version: event.ingest_seq,
                occurrence_id: request.occurrence_id,
                platform_id: request.platform_id,
            });
        }
        let observed_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        match core.store.append_managed_stuck_notification_delivered(
            ManagedStuckNotificationDelivery {
                run_id: request.run_id.clone(),
                expected_run_version: request.expected_run_version,
                occurrence_id: request.occurrence_id.clone(),
                event_id,
                observed_at,
                platform_id: request.platform_id.clone(),
            },
        ) {
            Ok(AppendEventOutcome::Inserted(event) | AppendEventOutcome::Duplicate(event)) => {
                Ok(ManagedStuckNotificationDeliveredResponse::Delivered {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: request.run_id,
                    previous_version: request.expected_run_version,
                    event_id: event.event_id,
                    event_version: event.ingest_seq,
                    occurrence_id: request.occurrence_id,
                    platform_id: request.platform_id,
                })
            }
            Err(StoreError::MissingRun { .. }) => Err(BridgeError::RunNotFound),
            Err(StoreError::ManagedStuckRunVersionStale { .. }) => {
                Ok(stuck_notification_rejected_response(
                    &request,
                    ManagedStuckNotificationDeliveredRejectedReason::RunVersionStale,
                ))
            }
            Err(StoreError::ManagedStuckOccurrenceMismatch { .. }) => {
                let reason = stuck_notification_rejected_reason(&core.store, &request)?;
                Ok(stuck_notification_rejected_response(&request, reason))
            }
            Err(_) => Err(BridgeError::StorageFailure),
        }
    });
    match response {
        Ok(response) => bounded_json(
            &response,
            MAX_PROJECT_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(BridgeError::RunNotFound) => {
            project_json(&CommandError::for_code(CommandErrorCode::RunNotFound))
        }
        Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
            CommandErrorCode::StorageUnavailable,
        )),
        Err(error) => Err(error),
    }
}

fn valid_stuck_notification_token(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn stuck_notification_rejected_reason(
    store: &Store,
    request: &ManagedStuckNotificationDeliveredRequest,
) -> Result<ManagedStuckNotificationDeliveredRejectedReason, BridgeError> {
    let contexts = store
        .managed_stuck_assessment_contexts()
        .map_err(|_| BridgeError::StorageFailure)?;
    let Some(context) = contexts
        .into_iter()
        .find(|context| context.run_id == request.run_id)
    else {
        return Ok(ManagedStuckNotificationDeliveredRejectedReason::OccurrenceMismatch);
    };
    if context.active_occurrence_id.as_deref() != Some(request.occurrence_id.as_str()) {
        return Ok(ManagedStuckNotificationDeliveredRejectedReason::OccurrenceMismatch);
    }
    Ok(match context.notification {
        ManagedStuckNotificationState::Delivered { occurrence_id, .. }
            if occurrence_id == request.occurrence_id =>
        {
            ManagedStuckNotificationDeliveredRejectedReason::AlreadyDelivered
        }
        ManagedStuckNotificationState::Due { occurrence_id, .. }
            if occurrence_id != request.occurrence_id =>
        {
            ManagedStuckNotificationDeliveredRejectedReason::OccurrenceMismatch
        }
        ManagedStuckNotificationState::Inactive
        | ManagedStuckNotificationState::NotDue { .. }
        | ManagedStuckNotificationState::Suppressed { .. }
        | ManagedStuckNotificationState::Due { .. }
        | ManagedStuckNotificationState::Delivered { .. } => {
            ManagedStuckNotificationDeliveredRejectedReason::NotDue
        }
    })
}

fn stuck_notification_rejected_response(
    request: &ManagedStuckNotificationDeliveredRequest,
    reason: ManagedStuckNotificationDeliveredRejectedReason,
) -> ManagedStuckNotificationDeliveredResponse {
    ManagedStuckNotificationDeliveredResponse::Rejected {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: request.run_id.clone(),
        expected_run_version: request.expected_run_version,
        occurrence_id: request.occurrence_id.clone(),
        platform_id: request.platform_id.clone(),
        reason,
    }
}

fn stuck_notification_delivered_event_id(
    request: &ManagedStuckNotificationDeliveredRequest,
) -> String {
    let mut digest = Sha256::new();
    for part in [
        request.run_id.as_str(),
        &request.expected_run_version.to_string(),
        request.occurrence_id.as_str(),
        request.platform_id.as_str(),
    ] {
        digest.update(part.len().to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("stuck-notification-{:x}", digest.finalize())
}

#[uniffi::export]
pub fn managed_run_still_working_json(request_json: String) -> Result<String, BridgeError> {
    protect(|| managed_run_still_working_with(&CORE, request_json))
}

fn managed_run_still_working_with(
    core_manager: &CoreManager,
    request_json: String,
) -> Result<String, BridgeError> {
    managed_run_still_working_with_observation(core_manager, request_json, None)
}

fn managed_run_still_working_with_observation(
    core_manager: &CoreManager,
    request_json: String,
    observation: Option<(u64, flit_protocol::StuckProcessReceipt)>,
) -> Result<String, BridgeError> {
    if request_json.len() > MAX_MANAGED_RUN_REQUEST_BYTES {
        return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
    }
    let request = match serde_json::from_str::<ManagedRunStillWorkingRequest>(&request_json) {
        Ok(request)
            if !request.run_id.trim().is_empty()
                && request.run_id.len() <= 256
                && !request.occurrence_id.trim().is_empty()
                && request.occurrence_id.len() <= 256
                && request.expected_run_version > 0
                && request.expected_run_version <= flit_protocol::MAX_JSON_SAFE_INTEGER =>
        {
            request
        }
        _ => {
            return project_json(&CommandError::for_code(CommandErrorCode::InvalidRunRequest));
        }
    };
    if request.client_protocol_version != PROTOCOL_VERSION {
        return project_json(&CommandError::protocol_mismatch());
    }
    let response = core_manager.with_ready_core(|core| {
        let event_id = still_working_event_id(&request);
        if core
            .store
            .managed_still_working_was_applied(&request.run_id, &event_id, &request.occurrence_id)
            .map_err(|_| BridgeError::StorageFailure)?
        {
            return Ok(still_working_rejected_response(
                &request,
                ManagedRunStillWorkingRejectedReason::AlreadyApplied,
            ));
        }
        let now_monotonic_ms = observation.as_ref().map_or_else(
            || {
                let elapsed = core.stuck_assessment.clock_origin.elapsed().as_millis();
                u64::try_from(elapsed)
                    .unwrap_or(flit_protocol::MAX_JSON_SAFE_INTEGER)
                    .min(flit_protocol::MAX_JSON_SAFE_INTEGER)
            },
            |(now, _)| *now,
        );
        let contexts = core
            .store
            .managed_stuck_assessment_contexts()
            .map_err(|_| BridgeError::StorageFailure)?;
        let Some(context) = contexts
            .into_iter()
            .find(|context| context.run_id == request.run_id)
        else {
            let snapshot = core
                .store
                .run_snapshot(&request.run_id)
                .map_err(|_| BridgeError::StorageFailure)?;
            return if snapshot.is_some() {
                Ok(still_working_rejected_response(
                    &request,
                    ManagedRunStillWorkingRejectedReason::NotCurrentlyStuck,
                ))
            } else {
                Err(BridgeError::RunNotFound)
            };
        };
        if context.version != request.expected_run_version {
            return Ok(still_working_rejected_response(
                &request,
                ManagedRunStillWorkingRejectedReason::RunVersionStale,
            ));
        }
        if context.active_occurrence_id.as_deref() != Some(request.occurrence_id.as_str()) {
            return Ok(still_working_rejected_response(
                &request,
                if context.active_occurrence_id.is_some() {
                    ManagedRunStillWorkingRejectedReason::OccurrenceMismatch
                } else {
                    ManagedRunStillWorkingRejectedReason::NotCurrentlyStuck
                },
            ));
        }
        let process = observation.as_ref().map_or_else(
            || {
                stuck_assessment::process_receipt(
                    &context,
                    core.stuck_assessment.process_probes.get(&request.run_id),
                    now_monotonic_ms,
                )
            },
            |(_, process)| process.clone(),
        );
        if !matches!(process, flit_protocol::StuckProcessReceipt::Alive { .. }) {
            return Ok(still_working_rejected_response(
                &request,
                ManagedRunStillWorkingRejectedReason::ProcessUnavailable,
            ));
        }
        let observed_at = core
            .store
            .current_utc_timestamp()
            .map_err(|_| BridgeError::StorageFailure)?;
        let outcome = core
            .store
            .apply_managed_still_working(ManagedStillWorkingAction {
                run_id: request.run_id.clone(),
                expected_run_version: request.expected_run_version,
                occurrence_id: request.occurrence_id.clone(),
                event_id,
                observed_at,
                reset_monotonic_ms: now_monotonic_ms,
                process,
                evidence_unavailable_reason: "provider_content_not_retained".to_owned(),
            })
            .map_err(|error| match error {
                StoreError::MissingRun { .. } => BridgeError::RunNotFound,
                _ => BridgeError::StorageFailure,
            })?;
        match outcome {
            ManagedStillWorkingOutcome::Applied(event) => {
                core.stuck_assessment.progress_baselines.insert(
                    request.run_id.clone(),
                    stuck_assessment::ProgressBaseline {
                        progress_event_id: context.progress_event_id,
                        monotonic_ms: now_monotonic_ms,
                    },
                );
                Ok(ManagedRunStillWorkingResponse::Applied {
                    protocol_version: PROTOCOL_VERSION.to_owned(),
                    run_id: request.run_id,
                    previous_version: request.expected_run_version,
                    event_id: event.event_id.clone(),
                    event_version: event.ingest_seq,
                    occurrence_id: request.occurrence_id,
                })
            }
            ManagedStillWorkingOutcome::Rejected { reason, .. } => Ok(
                still_working_rejected_response(&request, still_working_rejected_reason(reason)),
            ),
        }
    });
    match response {
        Ok(response) => bounded_json(
            &response,
            MAX_PROJECT_RESPONSE_BYTES,
            BridgeError::ManagedRunResponseTooLarge,
        ),
        Err(BridgeError::RunNotFound) => {
            project_json(&CommandError::for_code(CommandErrorCode::RunNotFound))
        }
        Err(BridgeError::StorageFailure) => project_json(&CommandError::for_code(
            CommandErrorCode::StorageUnavailable,
        )),
        Err(error) => Err(error),
    }
}

fn still_working_rejected_response(
    request: &ManagedRunStillWorkingRequest,
    reason: ManagedRunStillWorkingRejectedReason,
) -> ManagedRunStillWorkingResponse {
    ManagedRunStillWorkingResponse::Rejected {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        run_id: request.run_id.clone(),
        expected_run_version: request.expected_run_version,
        occurrence_id: request.occurrence_id.clone(),
        reason,
    }
}

fn still_working_rejected_reason(
    reason: StoreStillWorkingRejectedReason,
) -> ManagedRunStillWorkingRejectedReason {
    match reason {
        StoreStillWorkingRejectedReason::RunVersionStale => {
            ManagedRunStillWorkingRejectedReason::RunVersionStale
        }
        StoreStillWorkingRejectedReason::OccurrenceMismatch => {
            ManagedRunStillWorkingRejectedReason::OccurrenceMismatch
        }
        StoreStillWorkingRejectedReason::NotCurrentlyStuck => {
            ManagedRunStillWorkingRejectedReason::NotCurrentlyStuck
        }
        StoreStillWorkingRejectedReason::ProcessUnavailable => {
            ManagedRunStillWorkingRejectedReason::ProcessUnavailable
        }
        StoreStillWorkingRejectedReason::AlreadyApplied => {
            ManagedRunStillWorkingRejectedReason::AlreadyApplied
        }
    }
}

fn still_working_event_id(request: &ManagedRunStillWorkingRequest) -> String {
    let mut digest = Sha256::new();
    for part in [
        request.run_id.as_str(),
        &request.expected_run_version.to_string(),
        request.occurrence_id.as_str(),
    ] {
        digest.update(part.len().to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("stuck-action-{:x}", digest.finalize())
}

fn start_managed_run_in_core(
    core: &mut FoundationCore,
    connector: &dyn managed_start::ManagedCodexConnector,
    path_environment: Option<&std::ffi::OsStr>,
    git_baseline: GitBaselinePayload,
    git_change_baseline: managed_start::RetainedGitChangeBaseline,
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
        git_change_baseline,
        request,
    ) {
        Ok(response) => {
            if let Some(probe) = core
                .managed_runtimes
                .get(&response.run_id)
                .and_then(managed_start::RetainedManagedRun::process_probe)
            {
                core.stuck_assessment
                    .process_probes
                    .insert(response.run_id.clone(), probe);
            }
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
            Option<managed_start::ManagedTerminalGitChanges>,
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
        let terminal_changes = match &observation {
            flit_providers::CodexTurnObservation::Terminal {
                thread_id, turn_id, ..
            } => {
                if let Err(error) = runtime.validate_observation_identity(thread_id, turn_id) {
                    return finish_managed_observation_unknown(
                        core_manager,
                        request,
                        runtime,
                        error,
                    );
                }
                Some(runtime.observe_terminal_changes(&request.run_id))
            }
            _ => None,
        };
        let committed = core_manager.with_ready_core(|core| {
            if !core
                .managed_observations_in_flight
                .contains(&request.run_id)
            {
                return Err(BridgeError::CoreFailure);
            }
            Ok(commit(
                &mut core.store,
                &mut runtime,
                &request,
                observation,
                terminal_changes,
            ))
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

    #[cfg(unix)]
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::MetadataExt},
    };

    use flit_protocol::{
        EventProtocolVersion, ManagedRunOpenInProviderRequest, ManagedRunPermissionDecision,
        ManagedRunPermissionMode, ManagedRunPermissionRespondRequest,
        ManagedRunPermissionRespondResponse, ManagedRunStartResponse,
        ManagedRunStillWorkingRequest, ManagedRunStillWorkingResponse, PossiblyStuckPayload,
        ProviderExecutionAfterQuit, QuitImpactReason, QuitImpactResponse, RunDetailReadRequest,
        RunDetailReadResponse, StuckCauseCode, StuckProcessReceipt, SystemHealthRequest,
    };
    use flit_providers::{
        CodexManagedItemId, CodexManagedThreadId, CodexManagedTurnId, CodexManualStartedThread,
        CodexPermissionDecision, CodexPermissionDelivery, CodexPermissionRequest,
        CodexProviderAutoStartedThread, CodexRuntimeFingerprint, CodexStartedTurn,
        CodexTurnObservation, CodexTurnTerminalOutcome, ProviderFingerprint, classify_codex,
        validated_codex_0_144_6_fingerprint, validated_codex_0_145_0_fingerprint,
    };
    use flit_store::{
        InitialManagedSessionConnection, ManagedRunIntent, ManagedRunStartFailure,
        ManagedStuckAssessment, ManagedStuckTransition, ProjectRegistration,
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

    fn notification_policy_command_state(
        manager: &CoreManager,
    ) -> (NotificationPolicyResponse, u64, Project) {
        let response = notification_policy_read_with(
            manager,
            serde_json::to_string(&NotificationPolicyReadRequest {
                project_id: Some("project-observe".to_owned()),
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("notification policy state request"),
        )
        .expect("notification policy state response");
        let policy = serde_json::from_str(&response).expect("notification policy state JSON");
        let (cursor, project) = manager
            .with_ready_core(|core| {
                Ok((
                    core.store.latest_ingest_seq().expect("policy state cursor"),
                    core.store
                        .project("project-observe")
                        .expect("policy state Project read")
                        .expect("policy state Project"),
                ))
            })
            .expect("notification policy Store state");
        (policy, cursor, project)
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

    #[test]
    fn managed_stuck_assessment_command_accepts_only_protocol_version_and_is_quiet_when_empty() {
        let (_directory, manager, _) = managed_start_core("stuck-assessment-command");
        let request = serde_json::to_string(&ManagedRunsAssessStuckRequest {
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("assessment request");
        let expected = flit_protocol::ManagedRunsAssessStuckResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            assessed_runs: 0,
            transitions_appended: 0,
            unchanged_runs: 0,
            unavailable_runs: 0,
        };
        for _ in 0..2 {
            let actual: flit_protocol::ManagedRunsAssessStuckResponse = serde_json::from_str(
                &managed_runs_assess_stuck_with(&manager, request.clone())
                    .expect("assessment response"),
            )
            .expect("typed assessment response");
            assert_eq!(actual, expected);
        }
        let rejected: CommandError = serde_json::from_str(
            &managed_runs_assess_stuck_with(
                &manager,
                format!(
                    r#"{{"client_protocol_version":"{PROTOCOL_VERSION}","observed_at":"native-fact"}}"#
                ),
            )
            .expect("invalid request response"),
        )
        .expect("command error");
        assert_eq!(
            rejected,
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );

        manager
            .with_ready_core(|core| {
                core.store
                    .create_managed_run_intent(ManagedRunIntent {
                        id: "run-assess-starting".to_owned(),
                        project_id: "project-observe".to_owned(),
                        title: "Assess starting Run".to_owned(),
                        goal: None,
                        start_request: serde_json::Map::new(),
                        git_baseline: GitBaselinePayload::Unavailable {
                            project_id: "project-observe".to_owned(),
                            reason: GitBaselineUnavailableReason::RunnerUnavailable,
                        },
                        git_baseline_observed_at: "2026-08-09T10:00:00Z".to_owned(),
                        created_at: "2026-08-09T10:00:00Z".to_owned(),
                        run_created_event_id: "event-assess-starting-created".to_owned(),
                        git_baseline_event_id: "event-assess-starting-baseline".to_owned(),
                        start_requested_event_id: "event-assess-starting-requested".to_owned(),
                    })
                    .expect("starting Run");
                Ok(())
            })
            .expect("seed starting Run");
        let baseline: flit_protocol::ManagedRunsAssessStuckResponse = serde_json::from_str(
            &managed_runs_assess_stuck_with(&manager, request.clone()).expect("baseline response"),
        )
        .expect("baseline assessment");
        assert_eq!(baseline.assessed_runs, 1);
        assert_eq!(baseline.transitions_appended, 0);
        assert_eq!(baseline.unchanged_runs, 1);
        assert_eq!(baseline.unavailable_runs, 1);
        let unchanged: flit_protocol::ManagedRunsAssessStuckResponse = serde_json::from_str(
            &managed_runs_assess_stuck_with(&manager, request).expect("unchanged response"),
        )
        .expect("unchanged assessment");
        assert_eq!(unchanged.transitions_appended, 0);
        assert_eq!(unchanged.unchanged_runs, 1);
        assert_eq!(unchanged.unavailable_runs, 1);
        manager
            .with_ready_core(|core| {
                let snapshot = core
                    .store
                    .run_snapshot("run-assess-starting")
                    .expect("starting snapshot")
                    .expect("starting projection");
                assert_ne!(snapshot.dashboard_bucket, "PossiblyStuck");
                Ok(())
            })
            .expect("read starting Run");
    }

    #[test]
    fn stuck_notification_commands_read_due_and_record_only_exact_delivery_once() {
        let (_directory, manager, _response) = observation_core("stuck-notification-command");
        let due_version = manager
            .with_ready_core(|core| {
                let running_version = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("running snapshot")
                    .expect("running projection")
                    .version;
                let opened = core
                    .store
                    .append_managed_stuck_transition(ManagedStuckTransition {
                        run_id: "run-observe".to_owned(),
                        expected_run_version: running_version,
                        event_id: "event-notification-command-open".to_owned(),
                        observed_at: "2026-08-09T10:02:10Z".to_owned(),
                        assessment: ManagedStuckAssessment::PossiblyStuck(PossiblyStuckPayload {
                            occurrence_id: "occurrence-notification-command".to_owned(),
                            cause: StuckCauseCode::Unknown,
                            threshold_seconds: 120,
                            progress_event_id: "event-observe-created".to_owned(),
                            progress_observed_at: "2026-07-27T12:00:00Z".to_owned(),
                            progress_monotonic_ms: 5_000,
                            baseline_monotonic_ms: 5_000,
                            stuck_since_monotonic_ms: 125_000,
                            process: StuckProcessReceipt::Alive {
                                generation: "process-notification-command".to_owned(),
                                observed_monotonic_ms: 130_000,
                            },
                            evidence_unavailable_reason: "raw_provider_content_not_retained"
                                .to_owned(),
                        }),
                    })
                    .expect("open stuck occurrence");
                let opened_version = match opened {
                    flit_store::ManagedStuckTransitionOutcome::Appended(event) => event.ingest_seq,
                    other => panic!("stuck occurrence must append: {other:?}"),
                };
                let due = core
                    .store
                    .append_managed_stuck_transition(ManagedStuckTransition {
                        run_id: "run-observe".to_owned(),
                        expected_run_version: opened_version,
                        event_id: "event-notification-command-due".to_owned(),
                        observed_at: "2026-08-09T10:07:10Z".to_owned(),
                        assessment: ManagedStuckAssessment::NotificationDue(
                            flit_protocol::StuckNotificationDuePayload {
                                occurrence_id: "occurrence-notification-command".to_owned(),
                                due_at_monotonic_ms: 425_000,
                                process: StuckProcessReceipt::Alive {
                                    generation: "process-notification-command".to_owned(),
                                    observed_monotonic_ms: 430_000,
                                },
                                evidence_unavailable_reason: "raw_provider_content_not_retained"
                                    .to_owned(),
                            },
                        ),
                    })
                    .expect("mark notification due");
                match due {
                    flit_store::ManagedStuckTransitionOutcome::Appended(event) => {
                        Ok(event.ingest_seq)
                    }
                    other => panic!("notification due must append: {other:?}"),
                }
            })
            .expect("seed due notification");

        let due_request = serde_json::to_string(&ManagedStuckNotificationsDueReadRequest {
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("due read request");
        let due: ManagedStuckNotificationsDueReadResponse = serde_json::from_str(
            &managed_stuck_notifications_due_read_with(&manager, due_request.clone())
                .expect("due read response"),
        )
        .expect("typed due read response");
        assert_eq!(due.protocol_version, PROTOCOL_VERSION);
        assert_eq!(due.event_schema_version, EVENT_PROTOCOL_VERSION);
        assert_eq!(
            due.notifications,
            vec![ManagedStuckNotificationDueRecord {
                run_id: "run-observe".to_owned(),
                run_version: due_version,
                occurrence_id: "occurrence-notification-command".to_owned(),
                platform_id: "occurrence-notification-command".to_owned(),
                delivery_claimed: false,
            }]
        );
        let smuggled_due = format!(
            r#"{{"client_protocol_version":"{PROTOCOL_VERSION}","run_id":"native-selected"}}"#
        );
        assert_eq!(
            command_error(
                &managed_stuck_notifications_due_read_with(&manager, smuggled_due)
                    .expect("invalid due read response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );

        let wrong_occurrence = ManagedStuckNotificationDeliveredRequest {
            run_id: "run-observe".to_owned(),
            expected_run_version: due_version,
            occurrence_id: "occurrence-notification-wrong".to_owned(),
            platform_id: "occurrence-notification-wrong".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let rejected: ManagedStuckNotificationDeliveredResponse = serde_json::from_str(
            &managed_stuck_notification_delivered_with(
                &manager,
                serde_json::to_string(&wrong_occurrence).expect("wrong receipt request"),
            )
            .expect("wrong receipt response"),
        )
        .expect("typed wrong receipt response");
        assert!(matches!(
            rejected,
            ManagedStuckNotificationDeliveredResponse::Rejected {
                reason: ManagedStuckNotificationDeliveredRejectedReason::OccurrenceMismatch,
                ..
            }
        ));

        let claim_request = ManagedStuckNotificationDeliveryClaimRequest {
            run_id: "run-observe".to_owned(),
            expected_run_version: due_version,
            occurrence_id: "occurrence-notification-command".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let claimed: ManagedStuckNotificationDeliveryClaimResponse = serde_json::from_str(
            &managed_stuck_notification_delivery_claim_with(
                &manager,
                serde_json::to_string(&claim_request).expect("claim request"),
            )
            .expect("claim response"),
        )
        .expect("typed claim response");
        assert!(!claimed.already_claimed);
        let claimed_retry: ManagedStuckNotificationDeliveryClaimResponse = serde_json::from_str(
            &managed_stuck_notification_delivery_claim_with(
                &manager,
                serde_json::to_string(&claim_request).expect("claim retry request"),
            )
            .expect("claim retry response"),
        )
        .expect("typed claim retry response");
        assert!(claimed_retry.already_claimed);

        let claimed_due: ManagedStuckNotificationsDueReadResponse = serde_json::from_str(
            &managed_stuck_notifications_due_read_with(&manager, due_request.clone())
                .expect("claimed due read response"),
        )
        .expect("typed claimed due read response");
        assert!(claimed_due.notifications[0].delivery_claimed);

        let request = ManagedStuckNotificationDeliveredRequest {
            run_id: "run-observe".to_owned(),
            expected_run_version: due_version,
            occurrence_id: "occurrence-notification-command".to_owned(),
            platform_id: "occurrence-notification-command".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let wrong_platform = ManagedStuckNotificationDeliveredRequest {
            platform_id: "different-platform-notification".to_owned(),
            ..request.clone()
        };
        let false_receipt_cursor = manager
            .with_ready_core(|core| {
                core.store
                    .latest_ingest_seq()
                    .map_err(|_| BridgeError::StorageFailure)
            })
            .expect("false receipt cursor");
        assert_eq!(
            command_error(
                &managed_stuck_notification_delivered_with(
                    &manager,
                    serde_json::to_string(&wrong_platform).expect("wrong platform request")
                )
                .expect("wrong platform response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store.latest_ingest_seq().expect("unchanged cursor"),
                    false_receipt_cursor
                );
                assert_eq!(
                    core.store
                        .managed_stuck_notification_due_contexts()
                        .expect("due remains")
                        .len(),
                    1
                );
                Ok(())
            })
            .expect("verify false receipt no-write");
        let request_json = serde_json::to_string(&request).expect("receipt request");
        let delivered: ManagedStuckNotificationDeliveredResponse = serde_json::from_str(
            &managed_stuck_notification_delivered_with(&manager, request_json.clone())
                .expect("delivered response"),
        )
        .expect("typed delivered response");
        let delivered_version = match &delivered {
            ManagedStuckNotificationDeliveredResponse::Delivered {
                previous_version,
                event_version,
                platform_id,
                ..
            } => {
                assert_eq!(*previous_version, due_version);
                assert_eq!(platform_id, "occurrence-notification-command");
                *event_version
            }
            other => panic!("exact receipt must be delivered: {other:?}"),
        };
        let retry: ManagedStuckNotificationDeliveredResponse = serde_json::from_str(
            &managed_stuck_notification_delivered_with(&manager, request_json)
                .expect("receipt retry response"),
        )
        .expect("typed receipt retry response");
        assert_eq!(retry, delivered);

        let empty: ManagedStuckNotificationsDueReadResponse = serde_json::from_str(
            &managed_stuck_notifications_due_read_with(&manager, due_request)
                .expect("empty due read response"),
        )
        .expect("typed empty due read response");
        assert!(empty.notifications.is_empty());

        let changed_platform = ManagedStuckNotificationDeliveredRequest {
            expected_run_version: delivered_version,
            platform_id: "different-platform-notification".to_owned(),
            ..request
        };
        assert_eq!(
            command_error(
                &managed_stuck_notification_delivered_with(
                    &manager,
                    serde_json::to_string(&changed_platform).expect("changed platform request"),
                )
                .expect("changed platform response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );
    }

    #[test]
    fn still_working_command_revalidates_exact_process_and_is_retry_safe() {
        let (_directory, manager, _response) = observation_core("still-working-command");
        let (version, cursor) = manager
            .with_ready_core(|core| {
                let version = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("running snapshot")
                    .expect("running projection")
                    .version;
                core.store
                    .append_managed_stuck_transition(ManagedStuckTransition {
                        run_id: "run-observe".to_owned(),
                        expected_run_version: version,
                        event_id: "event-still-working-command-open".to_owned(),
                        observed_at: "2026-08-09T10:02:10Z".to_owned(),
                        assessment: ManagedStuckAssessment::PossiblyStuck(PossiblyStuckPayload {
                            occurrence_id: "occurrence-still-working-command".to_owned(),
                            cause: StuckCauseCode::Unknown,
                            threshold_seconds: 120,
                            progress_event_id: "event-observe-created".to_owned(),
                            progress_observed_at: "2026-07-27T12:00:00Z".to_owned(),
                            progress_monotonic_ms: 5_000,
                            baseline_monotonic_ms: 5_000,
                            stuck_since_monotonic_ms: 125_000,
                            process: StuckProcessReceipt::Alive {
                                generation: "process-still-working-command".to_owned(),
                                observed_monotonic_ms: 130_000,
                            },
                            evidence_unavailable_reason: "raw_provider_content_not_retained"
                                .to_owned(),
                        }),
                    })
                    .expect("open stuck occurrence");
                let stuck_version = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("stuck snapshot")
                    .expect("stuck projection")
                    .version;
                Ok((
                    stuck_version,
                    core.store.latest_ingest_seq().expect("stuck cursor"),
                ))
            })
            .expect("seed stuck Run");
        let request = ManagedRunStillWorkingRequest {
            run_id: "run-observe".to_owned(),
            expected_run_version: version,
            occurrence_id: "occurrence-still-working-command".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let dashboard: DashboardReadResponse = serde_json::from_str(
            &dashboard_read_with(
                &manager,
                &serde_json::to_string(&DashboardReadRequest {
                    expected_core_instance_id: None,
                    after_cursor: None,
                    requested_event_limit: MAX_DASHBOARD_DELTA_EVENTS as u32,
                    client_protocol_version: PROTOCOL_VERSION.to_owned(),
                })
                .expect("Dashboard request"),
            )
            .expect("Dashboard response"),
        )
        .expect("typed Dashboard response");
        assert!(matches!(
            dashboard,
            DashboardReadResponse::Snapshot { runs, .. }
                if runs.len() == 1
                    && runs[0].version == version
                    && runs[0].active_stuck_occurrence_id.as_deref()
                        == Some("occurrence-still-working-command")
        ));
        let request_json = serde_json::to_string(&request).expect("Still working request");

        let unavailable: ManagedRunStillWorkingResponse = serde_json::from_str(
            &managed_run_still_working_with_observation(
                &manager,
                request_json.clone(),
                Some((
                    140_000,
                    StuckProcessReceipt::Unavailable {
                        generation: Some("process-still-working-command".to_owned()),
                        reason: "provider_process_observation_unavailable".to_owned(),
                        observed_monotonic_ms: 140_000,
                    },
                )),
            )
            .expect("unavailable response"),
        )
        .expect("typed unavailable response");
        assert!(matches!(
            unavailable,
            ManagedRunStillWorkingResponse::Rejected {
                reason: ManagedRunStillWorkingRejectedReason::ProcessUnavailable,
                ..
            }
        ));
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store.latest_ingest_seq().expect("no-write cursor"),
                    cursor
                );
                Ok(())
            })
            .expect("verify unavailable no-write");

        let applied: ManagedRunStillWorkingResponse = serde_json::from_str(
            &managed_run_still_working_with_observation(
                &manager,
                request_json.clone(),
                Some((
                    140_000,
                    StuckProcessReceipt::Alive {
                        generation: "process-still-working-command".to_owned(),
                        observed_monotonic_ms: 140_000,
                    },
                )),
            )
            .expect("applied response"),
        )
        .expect("typed applied response");
        assert!(matches!(
            applied,
            ManagedRunStillWorkingResponse::Applied { .. }
        ));

        let retry: ManagedRunStillWorkingResponse = serde_json::from_str(
            &managed_run_still_working_with_observation(
                &manager,
                request_json,
                Some((
                    140_000,
                    StuckProcessReceipt::Alive {
                        generation: "process-still-working-command".to_owned(),
                        observed_monotonic_ms: 140_000,
                    },
                )),
            )
            .expect("retry response"),
        )
        .expect("typed retry response");
        assert!(matches!(
            retry,
            ManagedRunStillWorkingResponse::Rejected {
                reason: ManagedRunStillWorkingRejectedReason::AlreadyApplied,
                ..
            }
        ));
        manager
            .with_ready_core(|core| {
                let snapshot = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("reset snapshot")
                    .expect("reset projection");
                assert_eq!(snapshot.dashboard_bucket, "Working");
                assert_eq!(
                    snapshot.snapshot["stuck"]["notification"]["status"],
                    "suppressed"
                );
                assert_eq!(
                    core.store.latest_ingest_seq().expect("one action cursor"),
                    cursor + 1
                );
                Ok(())
            })
            .expect("verify exact action");

        let smuggled = format!(
            r#"{{"run_id":"run-observe","expected_run_version":{version},"occurrence_id":"occurrence-still-working-command","client_protocol_version":"{PROTOCOL_VERSION}","process":"alive"}}"#
        );
        assert_eq!(
            command_error(
                &managed_run_still_working_with(&manager, smuggled)
                    .expect("fact-smuggling response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );
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

    fn exact_bridge_change_set(project: &Path) -> ManagedGitChangeSet {
        let project_identity = ProjectDirectoryInspection::inspect(project)
            .expect("Project identity")
            .identity;
        let git_directory_path = project.join(".git");
        fs::create_dir_all(&git_directory_path).expect("Git directory");
        let git_directory_metadata = fs::metadata(&git_directory_path).expect("Git directory");
        #[cfg(unix)]
        let git_directory_filesystem_id = format!(
            "unix:{}:{}",
            git_directory_metadata.dev(),
            git_directory_metadata.ino()
        );
        #[cfg(not(unix))]
        let git_directory_filesystem_id = "unsupported".to_owned();
        let root = project
            .to_str()
            .expect("UTF-8 test Project")
            .as_bytes()
            .to_vec();
        let mut git_directory = root.clone();
        git_directory.extend_from_slice(b"/.git");
        let raw_path = b"non-utf8-\xff.txt".to_vec();
        ManagedGitChangeSet {
            attribution: ManagedGitChangeAttribution::Exact,
            baseline_head: Some("1".repeat(40)),
            terminal_head: Some("2".repeat(40)),
            repository_identity: ManagedGitRepositoryIdentity {
                project_filesystem_id: project_identity.filesystem_id.clone(),
                repository_root: root,
                repository_root_filesystem_id: project_identity.filesystem_id,
                git_directory: git_directory.clone(),
                git_directory_filesystem_id: git_directory_filesystem_id.clone(),
                common_directory: git_directory,
                common_directory_filesystem_id: git_directory_filesystem_id,
            },
            files: 1,
            insertions: Some(2),
            deletions: Some(1),
            changes: vec![ManagedGitFileChange {
                change_id: "0123456789abcdef0123456789abcdef".to_owned(),
                display_path: String::from_utf8_lossy(&raw_path).into_owned(),
                raw_path,
                status: ManagedGitFileStatus::Modified,
                committed: true,
                staged: false,
                unstaged: false,
                binary: false,
                insertions: Some(2),
                deletions: Some(1),
                project_scope: ManagedGitProjectScope::InsideProject,
            }],
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
        let (directory, manager, response, _) = observation_core_with_baseline(label, false);
        (directory, manager, response)
    }

    fn observation_core_with_baseline(
        label: &str,
        exact_clean_baseline: bool,
    ) -> (
        ObservationDirectory,
        Arc<CoreManager>,
        ManagedRunStartResponse,
        PathBuf,
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
                        git_baseline: if exact_clean_baseline {
                            GitBaselinePayload::Available {
                                project_id: "project-observe".to_owned(),
                                head: GitHead::Available {
                                    oid: "1".repeat(40),
                                },
                                dirty: GitDirtySummary {
                                    staged: 0,
                                    unstaged: 0,
                                    untracked: 0,
                                    entries: 0,
                                },
                            }
                        } else {
                            GitBaselinePayload::Unavailable {
                                project_id: "project-observe".to_owned(),
                                reason: GitBaselineUnavailableReason::RunnerUnavailable,
                            }
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
                        cwd: project.clone(),
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
        (directory, manager, response, project)
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
    fn notification_policy_commands_are_exact_versioned_and_core_owned() {
        let (_directory, manager, _) = managed_start_core("notification-policy");
        let read_request = serde_json::to_string(&NotificationPolicyReadRequest {
            project_id: Some("project-observe".to_owned()),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("notification policy read request");
        let defaults: NotificationPolicyResponse = serde_json::from_str(
            &notification_policy_read_with(&manager, read_request.clone())
                .expect("read notification policy"),
        )
        .expect("notification policy response");
        assert_eq!(defaults.global.version, 0);
        assert_eq!(
            defaults.project.as_ref().expect("Project policy").version,
            0
        );
        assert!(defaults.effective.kinds.permission);
        assert!(!defaults.effective.kinds.completion);
        assert!(!defaults.effective.quiet_hours.enabled);

        let global_request = GlobalNotificationPolicyUpdateRequest {
            expected_version: 0,
            kinds: NotificationKindsRecord {
                permission: true,
                question: true,
                failure: true,
                completion: false,
                stuck: true,
            },
            quiet_hours: QuietHoursRecord {
                enabled: true,
                start_minute: 1_320,
                end_minute: 480,
            },
            updated_at: "2026-08-13T01:00:00Z".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let global_request_json =
            serde_json::to_string(&global_request).expect("global policy request");
        let global: NotificationPolicyResponse = serde_json::from_str(
            &notification_policy_update_global_with(&manager, global_request_json.clone())
                .expect("update global policy"),
        )
        .expect("global policy response");
        assert_eq!(global.global.version, 1);
        assert!(global.project.is_none());
        assert!(global.effective.quiet_hours.enabled);

        assert_eq!(
            command_error(
                &notification_policy_update_global_with(&manager, global_request_json)
                    .expect("stale global command error")
            )
            .code,
            CommandErrorCode::NotificationPolicyVersionStale
        );

        let project_request = ProjectNotificationPolicyUpdateRequest {
            project_id: "project-observe".to_owned(),
            expected_version: 0,
            master: ProjectNotificationMasterRecord::Inherit,
            kinds: NotificationKindOverridesRecord {
                permission: NotificationOverrideRecord::On,
                question: NotificationOverrideRecord::Off,
                failure: NotificationOverrideRecord::Inherit,
                completion: NotificationOverrideRecord::Off,
                stuck: NotificationOverrideRecord::Inherit,
            },
            updated_at: "2026-08-13T01:01:00Z".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let project: NotificationPolicyResponse = serde_json::from_str(
            &notification_policy_update_project_with(
                &manager,
                serde_json::to_string(&project_request).expect("Project policy request"),
            )
            .expect("update Project policy"),
        )
        .expect("Project policy response");
        assert_eq!(project.global.version, 1);
        assert_eq!(project.project.as_ref().expect("Project policy").version, 1);
        assert!(!project.effective.kinds.question);
        assert!(project.effective.kinds.failure);

        let accepted_state = notification_policy_command_state(&manager);
        assert_eq!(accepted_state.0, project);
        assert_eq!(
            command_error(
                &notification_policy_update_project_with(
                    &manager,
                    serde_json::to_string(&project_request).expect("stale Project policy request"),
                )
                .expect("stale Project policy command error")
            )
            .code,
            CommandErrorCode::NotificationPolicyVersionStale
        );
        assert_eq!(notification_policy_command_state(&manager), accepted_state);

        let invalid_quiet_hours = GlobalNotificationPolicyUpdateRequest {
            expected_version: 1,
            kinds: global.global.kinds,
            quiet_hours: QuietHoursRecord {
                enabled: true,
                start_minute: 480,
                end_minute: 480,
            },
            updated_at: "2026-08-13T01:02:00Z".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        assert_eq!(
            command_error(
                &notification_policy_update_global_with(
                    &manager,
                    serde_json::to_string(&invalid_quiet_hours)
                        .expect("invalid quiet hours request"),
                )
                .expect("invalid quiet hours command error")
            )
            .code,
            CommandErrorCode::InvalidNotificationPolicy
        );
        assert_eq!(notification_policy_command_state(&manager), accepted_state);

        let mut protocol_mismatch = invalid_quiet_hours;
        protocol_mismatch.quiet_hours = global.global.quiet_hours;
        protocol_mismatch.client_protocol_version = "1.24".to_owned();
        assert_eq!(
            command_error(
                &notification_policy_update_global_with(
                    &manager,
                    serde_json::to_string(&protocol_mismatch)
                        .expect("protocol mismatch policy request"),
                )
                .expect("protocol mismatch policy command error")
            )
            .code,
            CommandErrorCode::ProtocolMismatch
        );
        assert_eq!(notification_policy_command_state(&manager), accepted_state);

        let after: NotificationPolicyResponse = serde_json::from_str(
            &notification_policy_read_with(&manager, read_request)
                .expect("read updated notification policy"),
        )
        .expect("updated policy response");
        assert_eq!(after, project);

        let mut missing_project = project_request;
        missing_project.project_id = "missing-project".to_owned();
        assert_eq!(
            command_error(
                &notification_policy_update_project_with(
                    &manager,
                    serde_json::to_string(&missing_project).expect("missing Project request"),
                )
                .expect("missing Project command error")
            )
            .code,
            CommandErrorCode::ProjectNotFound
        );
        assert_eq!(notification_policy_command_state(&manager), accepted_state);
        assert_eq!(
            command_error(
                &notification_policy_read_with(
                    &manager,
                    format!(
                        r#"{{"project_id":null,"client_protocol_version":"{PROTOCOL_VERSION}","timezone":"UTC"}}"#
                    ),
                )
                .expect("unknown field command error")
            )
            .code,
            CommandErrorCode::InvalidNotificationPolicy
        );
        assert_eq!(notification_policy_command_state(&manager), accepted_state);
        assert_eq!(
            command_error(
                &notification_policy_read_with(
                    &manager,
                    " ".repeat(MAX_NOTIFICATION_POLICY_REQUEST_BYTES + 1),
                )
                .expect("oversized notification policy command error")
            )
            .code,
            CommandErrorCode::InvalidNotificationPolicy
        );
        assert_eq!(notification_policy_command_state(&manager), accepted_state);
    }

    #[test]
    fn notification_policy_archived_and_corrupt_store_fail_without_mutation() {
        let (archived_directory, archived_manager, _) =
            managed_start_core("notification-policy-archived");
        let raw = rusqlite::Connection::open(archived_directory.0.join(DATABASE_FILE_NAME))
            .expect("open archived notification policy Store");
        raw.execute(
            "UPDATE projects SET archived_at = ?1 WHERE id = 'project-observe'",
            ["2026-08-13T01:03:00Z"],
        )
        .expect("archive notification policy Project");
        let archived_row = raw
            .query_row(
                "SELECT archived_at, notification_policy_json, updated_at FROM projects WHERE id = 'project-observe'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .expect("archived notification policy row");
        drop(raw);
        let (archived_cursor, archived_project) = archived_manager
            .with_ready_core(|core| {
                Ok((
                    core.store
                        .latest_ingest_seq()
                        .expect("archived policy cursor"),
                    core.store
                        .project("project-observe")
                        .expect("archived Project read"),
                ))
            })
            .expect("archived notification policy state");
        let archived_read = NotificationPolicyReadRequest {
            project_id: Some("project-observe".to_owned()),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        assert_eq!(
            command_error(
                &notification_policy_read_with(
                    &archived_manager,
                    serde_json::to_string(&archived_read).expect("archived policy read request"),
                )
                .expect("archived policy read command error")
            )
            .code,
            CommandErrorCode::ProjectNotFound
        );
        let archived_update = ProjectNotificationPolicyUpdateRequest {
            project_id: "project-observe".to_owned(),
            expected_version: 0,
            master: ProjectNotificationMasterRecord::Inherit,
            kinds: NotificationKindOverridesRecord {
                permission: NotificationOverrideRecord::Inherit,
                question: NotificationOverrideRecord::Inherit,
                failure: NotificationOverrideRecord::Inherit,
                completion: NotificationOverrideRecord::Inherit,
                stuck: NotificationOverrideRecord::Inherit,
            },
            updated_at: "2026-08-13T01:04:00Z".to_owned(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        assert_eq!(
            command_error(
                &notification_policy_update_project_with(
                    &archived_manager,
                    serde_json::to_string(&archived_update)
                        .expect("archived policy update request"),
                )
                .expect("archived policy update command error")
            )
            .code,
            CommandErrorCode::ProjectNotFound
        );
        archived_manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store
                        .latest_ingest_seq()
                        .expect("unchanged archived cursor"),
                    archived_cursor
                );
                assert_eq!(
                    core.store
                        .project("project-observe")
                        .expect("unchanged archived Project"),
                    archived_project
                );
                Ok(())
            })
            .expect("inspect unchanged archived policy state");
        let raw = rusqlite::Connection::open(archived_directory.0.join(DATABASE_FILE_NAME))
            .expect("reopen archived notification policy Store");
        let unchanged_archived_row = raw
            .query_row(
                "SELECT archived_at, notification_policy_json, updated_at FROM projects WHERE id = 'project-observe'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .expect("unchanged archived notification policy row");
        assert_eq!(unchanged_archived_row, archived_row);
        drop(raw);

        let (corrupt_directory, corrupt_manager, _) =
            managed_start_core("notification-policy-corrupt");
        let raw = rusqlite::Connection::open(corrupt_directory.0.join(DATABASE_FILE_NAME))
            .expect("open corrupt notification policy Store");
        raw.execute(
            "INSERT INTO app_settings(key, value_json, updated_at) VALUES('notification_policy', '{', ?1)",
            ["2026-08-13T01:05:00Z"],
        )
        .expect("inject corrupt notification policy");
        drop(raw);
        let (corrupt_cursor, corrupt_project) = corrupt_manager
            .with_ready_core(|core| {
                Ok((
                    core.store
                        .latest_ingest_seq()
                        .expect("corrupt policy cursor"),
                    core.store
                        .project("project-observe")
                        .expect("corrupt Project read"),
                ))
            })
            .expect("corrupt notification policy state");
        let corrupt_read = NotificationPolicyReadRequest {
            project_id: None,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        assert_eq!(
            command_error(
                &notification_policy_read_with(
                    &corrupt_manager,
                    serde_json::to_string(&corrupt_read).expect("corrupt policy read request"),
                )
                .expect("corrupt policy command error")
            )
            .code,
            CommandErrorCode::StorageUnavailable
        );
        corrupt_manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store
                        .latest_ingest_seq()
                        .expect("unchanged corrupt cursor"),
                    corrupt_cursor
                );
                assert_eq!(
                    core.store
                        .project("project-observe")
                        .expect("unchanged corrupt Project"),
                    corrupt_project
                );
                Ok(())
            })
            .expect("inspect unchanged corrupt policy state");
        let raw = rusqlite::Connection::open(corrupt_directory.0.join(DATABASE_FILE_NAME))
            .expect("reopen corrupt notification policy Store");
        let unchanged_corrupt: String = raw
            .query_row(
                "SELECT value_json FROM app_settings WHERE key = 'notification_policy'",
                [],
                |row| row.get(0),
            )
            .expect("unchanged corrupt notification policy");
        assert_eq!(unchanged_corrupt, "{");
    }

    #[test]
    fn notification_delivery_commands_are_exact_bounded_and_retry_safe() {
        let (directory, manager, _) = observation_core("notification-delivery");
        let permission = open_permission(&manager);
        let ManagedRunObserveResponse::PermissionRequested {
            request_version, ..
        } = permission
        else {
            panic!("expected open permission");
        };
        let due_request = NotificationDeliveriesDueReadRequest {
            local_minute: 8 * 60,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let due: NotificationDeliveriesDueReadResponse = serde_json::from_str(
            &notification_deliveries_due_read_with(
                &manager,
                serde_json::to_string(&due_request).expect("due request"),
            )
            .expect("due response"),
        )
        .expect("due response JSON");
        assert_eq!(due.protocol_version, PROTOCOL_VERSION);
        assert_eq!(due.notifications.len(), 1);
        let candidate = &due.notifications[0];
        assert_eq!(candidate.run_id, "run-observe");
        assert_eq!(candidate.project_id, "project-observe");
        assert_eq!(candidate.kind, ProtocolNotificationKind::Permission);
        assert_eq!(candidate.item_version, request_version);
        assert!(!candidate.delivery_claimed);
        assert!(!candidate.catch_up);

        manager
            .with_ready_core(|core| {
                core.store
                    .update_global_notification_policy(
                        0,
                        StoreNotificationKinds::default(),
                        StoreQuietHours {
                            enabled: true,
                            start_minute: 0,
                            end_minute: 12 * 60,
                        },
                        "2026-08-13T00:00:00Z",
                    )
                    .expect("enable test quiet hours");
                Ok(())
            })
            .expect("quiet policy update");
        let wrong_claim = NotificationDeliveryClaimRequest {
            notification_id: "notification-wrong".to_owned(),
            run_id: candidate.run_id.clone(),
            expected_run_version: candidate.run_version,
            kind: candidate.kind,
            item_id: candidate.item_id.clone(),
            item_version: candidate.item_version,
            platform_id: candidate.platform_id.clone(),
            local_minute: 8 * 60,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        assert_eq!(
            command_error(
                &notification_delivery_claim_with(
                    &manager,
                    serde_json::to_string(&wrong_claim).expect("wrong claim request"),
                )
                .expect("wrong claim response")
            ),
            CommandError::for_code(CommandErrorCode::RunVersionStale)
        );
        let raw = rusqlite::Connection::open(directory.0.join(DATABASE_FILE_NAME))
            .expect("inspect notification ledger");
        let ledger_count: i64 = raw
            .query_row("SELECT COUNT(*) FROM notification_deliveries", [], |row| {
                row.get(0)
            })
            .expect("notification ledger count");
        assert_eq!(ledger_count, 0);
        drop(raw);
        let boundary_claim = NotificationDeliveryClaimRequest {
            notification_id: candidate.notification_id.clone(),
            ..wrong_claim
        };
        assert_eq!(
            command_error(
                &notification_delivery_claim_with(
                    &manager,
                    serde_json::to_string(&boundary_claim).expect("boundary claim request"),
                )
                .expect("boundary claim response")
            ),
            CommandError::for_code(CommandErrorCode::RunVersionStale)
        );
        let raw = rusqlite::Connection::open(directory.0.join(DATABASE_FILE_NAME))
            .expect("inspect quiet suppression");
        let suppression: (String, String) = raw
            .query_row(
                "SELECT state, suppression_reason FROM notification_deliveries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("quiet suppression row");
        assert_eq!(
            suppression,
            ("suppressed".to_owned(), "quiet_hours".to_owned())
        );
        drop(raw);
        manager
            .with_ready_core(|core| {
                core.store
                    .update_global_notification_policy(
                        1,
                        StoreNotificationKinds::default(),
                        StoreQuietHours::default(),
                        "2026-08-13T00:01:00Z",
                    )
                    .expect("disable test quiet hours");
                Ok(())
            })
            .expect("quiet policy reset");

        let malformed = format!(
            r#"{{"local_minute":480,"client_protocol_version":"{PROTOCOL_VERSION}","evaluated_at":"native-time"}}"#
        );
        assert_eq!(
            command_error(
                &notification_deliveries_due_read_with(&manager, malformed)
                    .expect("malformed due response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );

        let claim = NotificationDeliveryClaimRequest {
            notification_id: candidate.notification_id.clone(),
            run_id: candidate.run_id.clone(),
            expected_run_version: candidate.run_version,
            kind: candidate.kind,
            item_id: candidate.item_id.clone(),
            item_version: candidate.item_version,
            platform_id: candidate.platform_id.clone(),
            local_minute: 8 * 60,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let claimed: NotificationDeliveryClaimResponse = serde_json::from_str(
            &notification_delivery_claim_with(
                &manager,
                serde_json::to_string(&claim).expect("claim request"),
            )
            .expect("claim response"),
        )
        .expect("claim response JSON");
        assert!(!claimed.already_claimed);
        let duplicate_claimed: NotificationDeliveryClaimResponse = serde_json::from_str(
            &notification_delivery_claim_with(
                &manager,
                serde_json::to_string(&claim).expect("duplicate claim request"),
            )
            .expect("duplicate claim response"),
        )
        .expect("duplicate claim response JSON");
        assert!(duplicate_claimed.already_claimed);

        let mut failed = NotificationDeliveryFailedRequest {
            notification_id: claim.notification_id.clone(),
            run_id: claim.run_id.clone(),
            kind: claim.kind,
            item_id: claim.item_id.clone(),
            item_version: claim.item_version,
            platform_id: claim.platform_id.clone(),
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        failed.item_id = "wrong-item".to_owned();
        assert_eq!(
            command_error(
                &notification_delivery_failed_with(
                    &manager,
                    serde_json::to_string(&failed).expect("mismatch failure request"),
                )
                .expect("mismatch failure response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );
        failed.item_id = claim.item_id.clone();
        let released: NotificationDeliveryFailedResponse = serde_json::from_str(
            &notification_delivery_failed_with(
                &manager,
                serde_json::to_string(&failed).expect("failure request"),
            )
            .expect("failure response"),
        )
        .expect("failure response JSON");
        assert!(released.released);

        let claimed_again: NotificationDeliveryClaimResponse = serde_json::from_str(
            &notification_delivery_claim_with(
                &manager,
                serde_json::to_string(&claim).expect("retry claim request"),
            )
            .expect("retry claim response"),
        )
        .expect("retry claim response JSON");
        assert!(!claimed_again.already_claimed);
        let delivered_request = NotificationDeliveredRequest {
            notification_id: claim.notification_id,
            run_id: claim.run_id,
            kind: claim.kind,
            item_id: claim.item_id,
            item_version: claim.item_version,
            platform_id: claim.platform_id,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };
        let delivered: NotificationDeliveredResponse = serde_json::from_str(
            &notification_delivered_with(
                &manager,
                serde_json::to_string(&delivered_request).expect("delivered request"),
            )
            .expect("delivered response"),
        )
        .expect("delivered response JSON");
        assert!(!delivered.already_delivered);
        let duplicate: NotificationDeliveredResponse = serde_json::from_str(
            &notification_delivered_with(
                &manager,
                serde_json::to_string(&delivered_request).expect("duplicate delivered request"),
            )
            .expect("duplicate delivered response"),
        )
        .expect("duplicate delivered response JSON");
        assert!(duplicate.already_delivered);

        let empty: NotificationDeliveriesDueReadResponse = serde_json::from_str(
            &notification_deliveries_due_read_with(
                &manager,
                serde_json::to_string(&due_request).expect("final due request"),
            )
            .expect("final due response"),
        )
        .expect("final due response JSON");
        assert!(empty.notifications.is_empty());
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
                        BaselineCase::Clean => Ok((
                            NativeGitObservation::Repository(flit_git::RepositoryReceipt {
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
                            }),
                            None,
                        )),
                        BaselineCase::Dirty => Ok((
                            NativeGitObservation::Repository(flit_git::RepositoryReceipt {
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
                            }),
                            None,
                        )),
                        BaselineCase::Unborn => Ok((
                            NativeGitObservation::Repository(flit_git::RepositoryReceipt {
                                canonical_root: project.to_owned(),
                                head: NativeGitHead::Unborn,
                                dirty: flit_git::DirtySummary {
                                    staged: 0,
                                    unstaged: 0,
                                    untracked: 0,
                                    entries: 0,
                                },
                            }),
                            None,
                        )),
                        BaselineCase::NotRepository => Ok((
                            NativeGitObservation::NotWorktree(
                                NativeNotWorktreeReason::NotRepository,
                            ),
                            None,
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
                Ok((
                    NativeGitObservation::NotWorktree(NativeNotWorktreeReason::NotRepository),
                    None,
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
                Ok((
                    NativeGitObservation::NotWorktree(NativeNotWorktreeReason::NotRepository),
                    None,
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
    fn dashboard_bridge_preserves_every_available_change_attribution() {
        for (stored, expected) in [
            (
                StoreDashboardChangeAttribution::Exact,
                ProtocolDashboardChangeAttribution::Exact,
            ),
            (
                StoreDashboardChangeAttribution::ObservedDuringRun,
                ProtocolDashboardChangeAttribution::ObservedDuringRun,
            ),
        ] {
            let record = dashboard_run_record(StoreDashboardRunSnapshot {
                project_id: "project-attribution".to_owned(),
                project_display_name: "Attribution Project".to_owned(),
                title: "Attribution Run".to_owned(),
                provider_kind: "codex".to_owned(),
                started_at: Some("2026-08-04T00:00:00Z".to_owned()),
                ended_at: Some("2026-08-04T00:01:00Z".to_owned()),
                attention_open_count: 0,
                active_stuck_occurrence_id: None,
                changes: StoreDashboardChangeSummary::Available {
                    attribution: stored,
                    files: 3,
                    insertions: 42,
                    deletions: 7,
                },
                projection: flit_store::RunSnapshot {
                    run_id: "run-attribution".to_owned(),
                    version: 1,
                    lifecycle: "Completed".to_owned(),
                    activity: "Idle".to_owned(),
                    activity_confidence: 1.0,
                    attention_level: "None".to_owned(),
                    dashboard_bucket: "Finished".to_owned(),
                    last_progress_at: None,
                    last_liveness_at: None,
                    snapshot: serde_json::Map::new(),
                    updated_at: "2026-08-04T00:01:00Z".to_owned(),
                },
            })
            .expect("Dashboard attribution mapping");
            assert!(matches!(
                record.changes,
                flit_protocol::DashboardChangeSummary::Available { attribution, .. }
                    if attribution == expected
            ));
        }
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
        assert!(first.events.iter().all(|event| {
            event.category == flit_protocol::RunEvidenceCategory::for_event_type(&event.event_type)
        }));

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
        assert!(second.events.iter().all(|event| {
            event.category == flit_protocol::RunEvidenceCategory::for_event_type(&event.event_type)
        }));

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
    fn active_attention_read_is_exact_content_safe_and_read_only() {
        let (_directory, manager, _) = observation_core("active-attention-read");
        let initial = manager
            .with_ready_core(|core| {
                core.store
                    .managed_run_active_attention_context("run-observe")
                    .map_err(|_| BridgeError::StorageFailure)
            })
            .expect("initial active attention context");
        let empty_json = run_active_attention_read_with(
            &manager,
            &serde_json::to_string(&RunActiveAttentionReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: initial.run_version,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("empty active attention request"),
        )
        .expect("empty active attention response");
        let empty: RunActiveAttentionReadResponse =
            serde_json::from_str(&empty_json).expect("empty active attention JSON");
        assert_eq!(empty.open_count, 0);
        assert!(matches!(empty.item, RunActiveAttentionSlot::Null));

        let permission = open_permission(&manager);
        let ManagedRunObserveResponse::PermissionRequested {
            request_id,
            request_version,
            ..
        } = permission
        else {
            panic!("expected permission request");
        };
        let cursor = manager
            .with_ready_core(|core| {
                core.store
                    .latest_ingest_seq()
                    .map_err(|_| BridgeError::StorageFailure)
            })
            .expect("attention cursor");
        let response_json = run_active_attention_read_with(
            &manager,
            &serde_json::to_string(&RunActiveAttentionReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: request_version,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("active attention request"),
        )
        .expect("active attention response");
        for forbidden in [
            "/private/tmp",
            "provider_thread_id",
            "provider_request_id",
            "raw_payload",
            "secret command",
        ] {
            assert!(!response_json.contains(forbidden));
        }
        let response: RunActiveAttentionReadResponse =
            serde_json::from_str(&response_json).expect("active attention JSON");
        assert_eq!(response.run_version, request_version);
        assert_eq!(response.open_count, 1);
        assert!(matches!(
            response.item,
            RunActiveAttentionSlot::Item(item)
                if item.source_event_type == "permission.requested"
                    && item.content_unavailable_reason == "raw_provider_content_not_retained"
                    && matches!(
                        item.action,
                        ProtocolRunActiveAttentionAction::PermissionResponse {
                            request_id: ref actual_request_id,
                            request_version: actual_request_version,
                        } if actual_request_id == &request_id
                            && actual_request_version == request_version
                    )
        ));

        for invalid in [
            "{}".to_owned(),
            serde_json::to_string(&RunActiveAttentionReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: request_version - 1,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("stale attention request"),
            serde_json::to_string(&RunActiveAttentionReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: request_version,
                client_protocol_version: "0.0".to_owned(),
            })
            .expect("mismatched attention request"),
        ] {
            let response = run_active_attention_read_with(&manager, &invalid)
                .expect("typed active attention error");
            assert!(matches!(
                command_error(&response).code,
                CommandErrorCode::InvalidRunRequest
                    | CommandErrorCode::RunVersionStale
                    | CommandErrorCode::ProtocolMismatch
            ));
        }
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
            .expect("active attention read has no side effect");
    }

    #[test]
    fn attention_acknowledge_is_exact_failure_only_and_retry_safe() {
        let (_directory, manager, _) = managed_start_core("attention-acknowledge");
        let (context, cursor) = manager
            .with_ready_core(|core| {
                core.store
                    .create_managed_run_intent(ManagedRunIntent {
                        id: "run-acknowledge".to_owned(),
                        project_id: "project-observe".to_owned(),
                        title: "A failed Run".to_owned(),
                        goal: None,
                        start_request: serde_json::Map::new(),
                        git_baseline: GitBaselinePayload::Unavailable {
                            project_id: "project-observe".to_owned(),
                            reason: GitBaselineUnavailableReason::RunnerUnavailable,
                        },
                        git_baseline_observed_at: "2026-08-13T10:00:00Z".to_owned(),
                        created_at: "2026-08-13T10:00:00Z".to_owned(),
                        run_created_event_id: "event-acknowledge-created".to_owned(),
                        git_baseline_event_id: "event-acknowledge-baseline".to_owned(),
                        start_requested_event_id: "event-acknowledge-requested".to_owned(),
                    })
                    .expect("create failed Run");
                core.store
                    .fail_managed_run_start(ManagedRunStartFailure {
                        run_id: "run-acknowledge".to_owned(),
                        reason: "provider_start_failed".to_owned(),
                        contract_version: "codex-app-server/0.145.0".to_owned(),
                        failed_at: "2026-08-13T10:00:01Z".to_owned(),
                        failed_event_id: "event-acknowledge-failed".to_owned(),
                    })
                    .expect("fail Run");
                Ok((
                    core.store
                        .managed_run_active_attention_context("run-acknowledge")
                        .expect("failure attention context"),
                    core.store.latest_ingest_seq().expect("failure cursor"),
                ))
            })
            .expect("seed failed Run");
        let item = context.item.expect("failure attention item");
        assert_eq!(item.category, "failure");
        assert_eq!(item.source_event_type, "run.failed");
        let active: RunActiveAttentionReadResponse = serde_json::from_str(
            &run_active_attention_read_with(
                &manager,
                &serde_json::to_string(&RunActiveAttentionReadRequest {
                    run_id: "run-acknowledge".to_owned(),
                    expected_run_version: context.run_version,
                    client_protocol_version: PROTOCOL_VERSION.to_owned(),
                })
                .expect("failure attention read request"),
            )
            .expect("failure attention read response"),
        )
        .expect("typed failure attention response");
        assert!(matches!(
            active.item,
            RunActiveAttentionSlot::Item(item)
                if item.action == ProtocolRunActiveAttentionAction::Acknowledge
        ));
        let request = AttentionAcknowledgeRequest {
            run_id: "run-acknowledge".to_owned(),
            expected_run_version: context.run_version,
            attention_id: item.attention_id.clone(),
            attention_version: item.attention_version,
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        };

        let stale = AttentionAcknowledgeRequest {
            expected_run_version: request.expected_run_version - 1,
            ..request.clone()
        };
        let stale: AttentionAcknowledgeResponse = serde_json::from_str(
            &attention_acknowledge_with(
                &manager,
                &serde_json::to_string(&stale).expect("stale request"),
            )
            .expect("stale response"),
        )
        .expect("typed stale response");
        assert!(matches!(
            stale,
            AttentionAcknowledgeResponse::Rejected {
                reason: AttentionAcknowledgeRejectedReason::RunVersionStale,
                ..
            }
        ));

        let wrong = AttentionAcknowledgeRequest {
            attention_id: "lifecycle:event-not-current".to_owned(),
            ..request.clone()
        };
        let wrong: AttentionAcknowledgeResponse = serde_json::from_str(
            &attention_acknowledge_with(
                &manager,
                &serde_json::to_string(&wrong).expect("wrong item request"),
            )
            .expect("wrong item response"),
        )
        .expect("typed wrong item response");
        assert!(matches!(
            wrong,
            AttentionAcknowledgeResponse::Rejected {
                reason: AttentionAcknowledgeRejectedReason::AttentionMismatch,
                ..
            }
        ));
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store.latest_ingest_seq().expect("no-write cursor"),
                    cursor
                );
                Ok(())
            })
            .expect("rejections do not write");

        let request_json = serde_json::to_string(&request).expect("acknowledgement request");
        let applied: AttentionAcknowledgeResponse = serde_json::from_str(
            &attention_acknowledge_with(&manager, &request_json).expect("applied response"),
        )
        .expect("typed applied response");
        assert!(matches!(
            applied,
            AttentionAcknowledgeResponse::Applied {
                previous_version,
                event_version,
                ref attention_id,
                ..
            } if previous_version == context.run_version
                && event_version == cursor + 1
                && attention_id == &item.attention_id
        ));
        let retry: AttentionAcknowledgeResponse = serde_json::from_str(
            &attention_acknowledge_with(&manager, &request_json).expect("retry response"),
        )
        .expect("typed retry response");
        assert!(matches!(
            retry,
            AttentionAcknowledgeResponse::Rejected {
                reason: AttentionAcknowledgeRejectedReason::AlreadyApplied,
                ..
            }
        ));
        manager
            .with_ready_core(|core| {
                assert_eq!(
                    core.store.latest_ingest_seq().expect("single write cursor"),
                    cursor + 1
                );
                let acknowledged = core
                    .store
                    .managed_run_active_attention_context("run-acknowledge")
                    .expect("acknowledged context");
                assert_eq!(acknowledged.open_count, 0);
                assert!(acknowledged.item.is_none());
                Ok(())
            })
            .expect("verify exact acknowledgement");

        let smuggled = format!(
            r#"{{"run_id":"run-acknowledge","expected_run_version":{},"attention_id":"{}","attention_version":{},"client_protocol_version":"{PROTOCOL_VERSION}","resolution":"resolved"}}"#,
            request.expected_run_version, request.attention_id, request.attention_version
        );
        assert_eq!(
            command_error(
                &attention_acknowledge_with(&manager, &smuggled).expect("fact-smuggling response")
            ),
            CommandError::for_code(CommandErrorCode::InvalidRunRequest)
        );
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
                    managed_start::RetainedGitChangeBaseline::Unavailable(
                        "git_baseline_observation_unavailable".to_owned(),
                    ),
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
    fn restarted_core_exposes_an_open_permission_without_stale_action_authority() {
        let (directory, manager, _) = observation_core("open-permission-restart");
        let permission = open_permission(&manager);
        let request =
            permission_response_request(&permission, ManagedRunPermissionDecision::AllowOnce);
        drop(manager);

        let restarted = CoreManager::default();
        restarted
            .initialize(directory.0.to_str().expect("UTF-8 restart path"))
            .expect("restart Core");
        let context = restarted
            .with_ready_core(|core| {
                let cursor = core
                    .store
                    .latest_ingest_seq()
                    .map_err(|_| BridgeError::StorageFailure)?;
                let events = core
                    .store
                    .run_events_through("run-observe", 0, cursor, 10)
                    .map_err(|_| BridgeError::StorageFailure)?;
                assert_eq!(
                    events.events.last().map(|event| event.event_type.as_str()),
                    Some("diagnostic.sequence_gap"),
                    "unexpected restart events: {events:?}"
                );
                core.store
                    .managed_run_active_attention_context("run-observe")
                    .map_err(|_| BridgeError::StorageFailure)
            })
            .expect("restarted active attention context");
        let response_json = run_active_attention_read_with(
            &restarted,
            &serde_json::to_string(&RunActiveAttentionReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version: context.run_version,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("restarted active attention request"),
        )
        .expect("restarted active attention response");
        let response: RunActiveAttentionReadResponse =
            serde_json::from_str(&response_json).expect("restarted active attention JSON");
        assert!(
            matches!(
                response.item,
                RunActiveAttentionSlot::Item(ref item)
                    if item.status == flit_protocol::RunActiveAttentionStatus::Open
                        && item.action == ProtocolRunActiveAttentionAction::Unavailable {
                            reason: "provider_request_authority_lost".to_owned(),
                        }
            ),
            "unexpected restarted attention: {response:?}"
        );

        let calls = AtomicUsize::new(0);
        let command = managed_run_permission_respond_with(
            &restarted,
            serde_json::to_string(&request).expect("stale restart request"),
            |_runtime, _decision| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(())
            },
        )
        .expect("stale restart response");
        assert_eq!(
            command_error(&command),
            CommandError::for_code(CommandErrorCode::ManagedRunNotActive)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn terminal_change_summary_is_committed_with_lifecycle_and_projected_exactly() {
        let (_directory, manager, response) = observation_core("terminal-changes");
        let observer_manager = Arc::downgrade(&manager);
        let observe = Arc::new(move || {
            let manager = observer_manager.upgrade().expect("live Core manager");
            manager
                .with_ready_core(|core| {
                    assert!(
                        core.managed_observations_in_flight.contains("run-observe"),
                        "terminal Git observation must retain the in-flight guard"
                    );
                    assert!(
                        !core.managed_runtimes.contains_key("run-observe"),
                        "terminal Git observation must run after the runtime leaves the Core map"
                    );
                    Ok(())
                })
                .expect("terminal Git observation must not hold the Core mutex");
            ManagedGitChangeSummary::Exact {
                files: 3,
                insertions: 5,
                deletions: 2,
            }
        });
        manager
            .with_ready_core(|core| {
                core.managed_runtimes.insert(
                    "run-observe".to_owned(),
                    managed_start::RetainedManagedRun::for_test_with_change_observer(
                        response,
                        Box::new(DetachedTestRuntime),
                        observe,
                    ),
                );
                Ok(())
            })
            .expect("replace retained Git baseline");

        managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| Ok(terminal_observation()),
            managed_start::commit_managed_observation,
        )
        .expect("terminal response");
        assert_runtime_state(&manager, false);
        manager
            .with_ready_core(|core| {
                let snapshot = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("terminal snapshot")
                    .expect("terminal snapshot");
                assert_eq!(
                    snapshot.snapshot["changes"],
                    serde_json::json!({
                        "availability": "available",
                        "attribution": "exact",
                        "files": 3,
                        "insertions": 5,
                        "deletions": 2
                    })
                );
                let events = core
                    .store
                    .run_events_through("run-observe", 0, snapshot.version, 10)
                    .expect("terminal events");
                let terminal = events.events.last().expect("terminal event");
                assert_eq!(terminal.protocol_version, EventProtocolVersion::V1_2);
                assert_eq!(terminal.payload["changes"], snapshot.snapshot["changes"]);
                Ok(())
            })
            .expect("terminal projection");
    }

    #[test]
    fn terminal_file_receipt_is_committed_atomically_without_exposing_raw_paths() {
        let (_directory, manager, response, project) =
            observation_core_with_baseline("terminal-file-receipt", true);
        let change_set = exact_bridge_change_set(&project);
        let raw_path = change_set.changes[0].raw_path.clone();
        let display_path = change_set.changes[0].display_path.clone();
        let change_id = change_set.changes[0].change_id.clone();
        manager
            .with_ready_core(|core| {
                core.managed_runtimes.insert(
                    "run-observe".to_owned(),
                    managed_start::RetainedManagedRun::for_test_with_terminal_change_observer(
                        response,
                        Box::new(DetachedTestRuntime),
                        Arc::new(move || {
                            (
                                ManagedGitChangeSummary::Exact {
                                    files: 1,
                                    insertions: 2,
                                    deletions: 1,
                                },
                                Some(Box::new(change_set.clone())),
                            )
                        }),
                    ),
                );
                Ok(())
            })
            .expect("replace retained detailed Git baseline");

        managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| Ok(terminal_observation()),
            managed_start::commit_managed_observation,
        )
        .expect("terminal response");
        assert_runtime_state(&manager, false);
        manager
            .with_ready_core(|core| {
                let snapshot = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("terminal snapshot")
                    .expect("terminal snapshot");
                let event = core
                    .store
                    .run_events_through("run-observe", 0, snapshot.version, 10)
                    .expect("terminal events")
                    .events
                    .pop()
                    .expect("terminal event");
                let rendered =
                    serde_json::to_string(&(snapshot.snapshot, event)).expect("public JSON");
                assert!(!rendered.contains(&display_path));
                assert!(!rendered.contains("non-utf8"));
                assert_eq!(
                    core.store
                        .managed_git_file_change("run-observe", &change_id)
                        .expect("stored change")
                        .expect("stored change")
                        .raw_path,
                    raw_path
                );
                Ok(())
            })
            .expect("atomic terminal receipt");
    }

    #[test]
    fn run_changes_read_is_versioned_cursor_bounded_and_path_free() {
        let (_directory, manager, _response, project) =
            observation_core_with_baseline("run-changes-read", true);
        let change_set = exact_bridge_change_set(&project);
        let expected_display = change_set.changes[0].display_path.clone();
        let expected_cursor = change_set.changes[0].change_id.clone();
        let run_version = manager
            .with_ready_core(|core| {
                let outcome = core
                    .store
                    .append_managed_provider_observation(ManagedProviderObservation {
                        run_id: "run-observe".to_owned(),
                        session_id: "session-observe".to_owned(),
                        external_session_key: "thread-observe".to_owned(),
                        provider_turn_id: "turn-observe".to_owned(),
                        contract_version: "codex-app-server/0.145.0".to_owned(),
                        event_id: "event-run-changes-read-terminal".to_owned(),
                        observed_at: "2026-07-27T12:00:02Z".to_owned(),
                        kind: ManagedProviderObservationKind::TurnCompleted {
                            changes: ManagedGitChangeSummary::Exact {
                                files: 1,
                                insertions: 2,
                                deletions: 1,
                            },
                            change_set: Some(Box::new(change_set)),
                        },
                    })
                    .expect("terminal change set");
                Ok(appended_event(&outcome).ingest_seq)
            })
            .expect("seed terminal change set");
        let request = |after_cursor: Option<String>, expected_run_version: u64| {
            serde_json::to_string(&RunChangesReadRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version,
                after_cursor,
                requested_change_limit: 1,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("Run Changes request")
        };

        let first = run_changes_read_with(&manager, &request(None, run_version))
            .expect("first Run Changes response");
        assert!(!first.contains("raw_path"));
        assert!(!first.contains("filesystem_id"));
        assert!(!first.contains("repository_root"));
        let first: RunChangesReadResponse =
            serde_json::from_str(&first).expect("first Run Changes JSON");
        let RunChangesReadResponse::Available {
            attribution,
            baseline_head,
            terminal_head,
            next_cursor,
            has_more,
            changes,
            ..
        } = first
        else {
            panic!("expected available Run Changes");
        };
        assert_eq!(attribution, ProtocolDashboardChangeAttribution::Exact);
        assert!(matches!(baseline_head, RunChangeHead::Available { .. }));
        assert!(matches!(terminal_head, RunChangeHead::Available { .. }));
        assert_eq!(next_cursor.as_deref(), Some(expected_cursor.as_str()));
        assert!(!has_more);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].display_path, expected_display);

        let exhausted = run_changes_read_with(
            &manager,
            &request(Some(expected_cursor.clone()), run_version),
        )
        .expect("exhausted Run Changes response");
        let RunChangesReadResponse::Available {
            next_cursor,
            has_more,
            changes,
            ..
        } = serde_json::from_str(&exhausted).expect("exhausted Run Changes JSON")
        else {
            panic!("expected available exhausted Run Changes");
        };
        assert_eq!(next_cursor.as_deref(), Some(expected_cursor.as_str()));
        assert!(!has_more);
        assert!(changes.is_empty());

        let stale = run_changes_read_with(&manager, &request(None, run_version - 1))
            .expect("typed stale version");
        assert_eq!(
            command_error(&stale).code,
            CommandErrorCode::RunVersionStale
        );
        for invalid_cursor in ["not-an-opaque-cursor".to_owned(), "0".repeat(32)] {
            let response =
                run_changes_read_with(&manager, &request(Some(invalid_cursor), run_version))
                    .expect("typed invalid cursor");
            assert_eq!(
                command_error(&response).code,
                CommandErrorCode::InvalidRunRequest
            );
        }

        let (_missing_directory, missing_manager, _) = observation_core("run-changes-missing");
        let missing_version = missing_manager
            .with_ready_core(|core| {
                Ok(core
                    .store
                    .run_snapshot("run-observe")
                    .expect("missing-set snapshot")
                    .expect("missing-set snapshot")
                    .version)
            })
            .expect("missing-set version");
        let unavailable = run_changes_read_with(&missing_manager, &request(None, missing_version))
            .expect("unavailable Run Changes response");
        assert!(matches!(
            serde_json::from_str::<RunChangesReadResponse>(&unavailable)
                .expect("unavailable Run Changes JSON"),
            RunChangesReadResponse::Unavailable {
                reason: RunChangesUnavailableReason::ChangeSetNotAvailable,
                ..
            }
        ));
        let missing_cursor = run_changes_read_with(
            &missing_manager,
            &request(Some("0".repeat(32)), missing_version),
        )
        .expect("typed missing-set cursor");
        assert_eq!(
            command_error(&missing_cursor).code,
            CommandErrorCode::InvalidRunRequest
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_change_external_open_revalidates_exact_authority_and_calls_opener_once() {
        let (_directory, manager, _response, project) =
            observation_core_with_baseline("run-change-external-open", true);
        let mut change_set = exact_bridge_change_set(&project);
        change_set.changes[0].raw_path = b"inside.txt".to_vec();
        change_set.changes[0].display_path = "inside.txt".to_owned();
        let change_id = change_set.changes[0].change_id.clone();
        let target = project.join(OsString::from_vec(change_set.changes[0].raw_path.clone()));
        fs::write(&target, b"first identity").expect("external-open target");
        let run_version = manager
            .with_ready_core(|core| {
                let outcome = core
                    .store
                    .append_managed_provider_observation(ManagedProviderObservation {
                        run_id: "run-observe".to_owned(),
                        session_id: "session-observe".to_owned(),
                        external_session_key: "thread-observe".to_owned(),
                        provider_turn_id: "turn-observe".to_owned(),
                        contract_version: "codex-app-server/0.145.0".to_owned(),
                        event_id: "event-run-change-external-open-terminal".to_owned(),
                        observed_at: "2026-07-27T12:00:02Z".to_owned(),
                        kind: ManagedProviderObservationKind::TurnCompleted {
                            changes: ManagedGitChangeSummary::Exact {
                                files: 1,
                                insertions: 2,
                                deletions: 1,
                            },
                            change_set: Some(Box::new(change_set)),
                        },
                    })
                    .expect("terminal external-open change set");
                Ok(appended_event(&outcome).ingest_seq)
            })
            .expect("seed external-open change set");
        let request = |change_id: String, expected_run_version: u64| {
            serde_json::to_string(&RunChangeExternalOpenRequest {
                run_id: "run-observe".to_owned(),
                expected_run_version,
                change_id,
                client_protocol_version: PROTOCOL_VERSION.to_owned(),
            })
            .expect("external-open request")
        };

        let open_calls = AtomicUsize::new(0);
        let canonical_target = fs::canonicalize(&target).expect("canonical external-open target");
        let opened = run_change_external_open_with(
            &manager,
            &request(change_id.clone(), run_version),
            inspect_external_open_target,
            |path| {
                open_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(path, canonical_target);
                Ok(())
            },
        )
        .expect("external-open response");
        assert_eq!(open_calls.load(Ordering::SeqCst), 1);
        assert!(!opened.contains("raw_path"));
        assert!(!opened.contains("filesystem_id"));
        assert!(!opened.contains("inside.txt"));
        assert!(matches!(
            serde_json::from_str::<RunChangeExternalOpenResponse>(&opened)
                .expect("opened response JSON"),
            RunChangeExternalOpenResponse::Opened {
                run_version: opened_version,
                ref change_id,
                ..
            } if opened_version == run_version
                && change_id == "0123456789abcdef0123456789abcdef"
        ));

        let stale = run_change_external_open_with(
            &manager,
            &request(change_id.clone(), run_version - 1),
            |_| -> Result<ExternalOpenTarget, ExternalOpenGuardError> {
                panic!("stale request must not inspect the filesystem")
            },
            |_| -> Result<(), ()> { panic!("stale request must not call the opener") },
        )
        .expect("typed stale external-open response");
        assert_eq!(
            command_error(&stale).code,
            CommandErrorCode::RunVersionStale
        );

        let missing = run_change_external_open_with(
            &manager,
            &request("f".repeat(32), run_version),
            |_| -> Result<ExternalOpenTarget, ExternalOpenGuardError> {
                panic!("missing change must not inspect the filesystem")
            },
            |_| -> Result<(), ()> { panic!("missing change must not call the opener") },
        )
        .expect("typed missing external-open response");
        assert!(matches!(
            serde_json::from_str::<RunChangeExternalOpenResponse>(&missing)
                .expect("missing response JSON"),
            RunChangeExternalOpenResponse::Disabled {
                reason: RunChangeExternalOpenDisabledReason::ChangeNotFound,
                ..
            }
        ));

        let inspection_count = AtomicUsize::new(0);
        let replacement = project.join("replacement.txt");
        let drifted = run_change_external_open_with(
            &manager,
            &request(change_id, run_version),
            |authority| {
                if inspection_count.fetch_add(1, Ordering::SeqCst) == 1 {
                    fs::write(&replacement, b"second identity").expect("replacement target");
                    fs::rename(&replacement, &target).expect("replace target identity");
                }
                inspect_external_open_target(authority)
            },
            |_| -> Result<(), ()> { panic!("identity drift must not call the opener") },
        )
        .expect("typed drift external-open response");
        assert_eq!(inspection_count.load(Ordering::SeqCst), 2);
        assert!(matches!(
            serde_json::from_str::<RunChangeExternalOpenResponse>(&drifted)
                .expect("drift response JSON"),
            RunChangeExternalOpenResponse::Disabled {
                reason: RunChangeExternalOpenDisabledReason::TargetIdentityDrift,
                ..
            }
        ));
    }

    #[test]
    fn mismatched_terminal_identity_is_rejected_before_git_observation() {
        let (_directory, manager, response) = observation_core("terminal-identity-mismatch");
        let observations = Arc::new(AtomicU64::new(0));
        let observations_for_observer = Arc::clone(&observations);
        manager
            .with_ready_core(|core| {
                core.managed_runtimes.insert(
                    "run-observe".to_owned(),
                    managed_start::RetainedManagedRun::for_test_with_change_observer(
                        response,
                        Box::new(DetachedTestRuntime),
                        Arc::new(move || {
                            observations_for_observer.fetch_add(1, Ordering::SeqCst);
                            ManagedGitChangeSummary::Exact {
                                files: 1,
                                insertions: 1,
                                deletions: 0,
                            }
                        }),
                    ),
                );
                Ok(())
            })
            .expect("replace retained Git baseline");

        let response = managed_run_observe_with(
            &manager,
            observation_request_json(),
            |_runtime| {
                Ok(CodexTurnObservation::Terminal {
                    thread_id: CodexManagedThreadId::new("thread-mismatch").expect("thread ID"),
                    turn_id: CodexManagedTurnId::new("turn-observe").expect("turn ID"),
                    outcome: CodexTurnTerminalOutcome::Completed,
                })
            },
            managed_start::commit_managed_observation,
        )
        .expect("mismatched terminal response");
        assert_eq!(observations.load(Ordering::SeqCst), 0);
        assert_eq!(
            serde_json::from_str::<CommandError>(&response).expect("command error"),
            CommandError::for_code(CommandErrorCode::ProviderObservationUnknown)
        );
        assert_runtime_state(&manager, false);
        manager
            .with_ready_core(|core| {
                let run = core
                    .store
                    .managed_run("run-observe")
                    .expect("read mismatched terminal Run")
                    .expect("mismatched terminal Run");
                assert_eq!(run.ended_at, None);
                let snapshot = core
                    .store
                    .run_snapshot("run-observe")
                    .expect("mismatched terminal snapshot")
                    .expect("mismatched terminal snapshot");
                assert_eq!(
                    snapshot.snapshot["changes"],
                    serde_json::json!({
                        "availability": "unavailable",
                        "reason": "git_observation_not_configured"
                    })
                );
                Ok(())
            })
            .expect("mismatched terminal Store state");
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
            |_store, _runtime, _request, _observation, _changes| {
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
                |_store, _runtime, _request, _observation, _changes| panic!("commit panic control"),
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
