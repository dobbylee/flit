use std::path::PathBuf;

mod codex_contract;
mod codex_transport;
mod executable;
mod probe;
mod process;
mod profile;
mod schema;
mod version;

pub use codex_contract::{
    CodexContractError, CodexDeletedThread, CodexInterruptRequested, CodexManagedItemId,
    CodexManagedListPage, CodexManagedScope, CodexManagedThreadConflict, CodexManagedThreadId,
    CodexManagedTurnId, CodexManualStartedThread, CodexPermissionRequest, CodexStartedThread,
    CodexStartedTurn, CodexThreadRead, CodexThreadState, CodexTurnObservation,
    CodexTurnTerminalOutcome, MAX_CODEX_APP_SERVER_FRAME_BYTES, MAX_CODEX_MANAGED_THREADS,
    MAX_CODEX_TURN_PROMPT_BYTES, codex_initialize_request, codex_initialized_notification,
    codex_manual_start_request, codex_read_only_start_request, codex_read_request,
    codex_thread_delete_request, codex_thread_list_request, codex_turn_interrupt_request,
    codex_turn_start_request, decode_codex_initialize_response, decode_codex_manual_start_response,
    decode_codex_read_response, decode_codex_start_response, decode_codex_thread_delete_response,
    decode_codex_thread_deleted_notification, decode_codex_thread_list_response,
    decode_codex_turn_interrupt_response, decode_codex_turn_notification,
    decode_codex_turn_start_response,
};
pub use codex_transport::{
    CODEX_APP_SERVER_REQUEST_TIMEOUT, CodexAppServer, CodexAppServerError, CodexManagedThreads,
    MAX_CODEX_APP_SERVER_STDERR_BYTES, MAX_CODEX_COMMAND_STARTS_PER_TURN, MAX_CODEX_LIST_PAGES,
    MAX_CODEX_OBSERVATION_FRAMES, MAX_CODEX_PENDING_NOTIFICATION_BYTES,
    MAX_CODEX_PENDING_NOTIFICATIONS, MAX_CODEX_PERMISSION_REQUESTS_PER_TURN,
};
pub use executable::{
    ExecutableInspection, ExecutableInspectionError, ExecutableSelectionSource,
    MAX_EXECUTABLE_BYTES, inspect_codex_at, inspect_codex_on_path,
};
pub use probe::{
    CodexCompatibilityProbe, CodexCompatibilityProbeError, CodexRuntimeFingerprint,
    probe_codex_compatibility_at, probe_codex_compatibility_on_path,
};
pub use schema::{
    CodexSchemaProbe, CodexSchemaProbeError, MAX_SCHEMA_BYTES, MAX_SCHEMA_OUTPUT_BYTES,
    SCHEMA_PROBE_TIMEOUT, SchemaArtifact, probe_codex_schema,
};
pub use version::{
    CodexVersionProbe, CodexVersionProbeError, MAX_VERSION_OUTPUT_BYTES, VERSION_PROBE_TIMEOUT,
    probe_codex_version,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProviderKind {
    Codex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderCompatibility {
    Supported,
    Degraded,
    Unknown,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProviderCapability {
    Launch,
    ListManaged,
    Resume,
    Reconcile,
    StructuredActivity,
    PermissionDetect,
    PermissionRespond,
    PermissionPolicyConfigure,
    PermissionPolicyObserve,
    QuestionDetect,
    QuestionRespond,
    CompletionDetect,
    History,
    OpenInProvider,
    ContinueAfterQuit,
    Stop,
}

impl ProviderCapability {
    pub const ALL: [Self; 16] = [
        Self::Launch,
        Self::ListManaged,
        Self::Resume,
        Self::Reconcile,
        Self::StructuredActivity,
        Self::PermissionDetect,
        Self::PermissionRespond,
        Self::PermissionPolicyConfigure,
        Self::PermissionPolicyObserve,
        Self::QuestionDetect,
        Self::QuestionRespond,
        Self::CompletionDetect,
        Self::History,
        Self::OpenInProvider,
        Self::ContinueAfterQuit,
        Self::Stop,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityStatus {
    Supported,
    Degraded,
    Unsupported,
    Unknown,
    Unavailable,
}

impl CapabilityStatus {
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Supported | Self::Degraded)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityEntry {
    pub capability: ProviderCapability,
    pub status: CapabilityStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderFingerprint {
    pub canonical_executable: PathBuf,
    pub executable_version: String,
    pub executable_sha256: String,
    pub combined_schema_sha256: String,
    pub v2_schema_sha256: String,
    pub method_allowlist_sha256: String,
    pub fixture_sha256: String,
    pub smoke_run_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FingerprintAxis {
    CanonicalExecutable,
    ExecutableVersion,
    ExecutableSha256,
    CombinedSchemaSha256,
    V2SchemaSha256,
    MethodAllowlistSha256,
    FixtureSha256,
    SmokeRunId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCapabilitySnapshot {
    pub provider: ProviderKind,
    pub compatibility: ProviderCompatibility,
    pub capabilities: Vec<CapabilityEntry>,
    pub fingerprint_mismatches: Vec<FingerprintAxis>,
}

impl ProviderCapabilitySnapshot {
    pub fn status(&self, capability: ProviderCapability) -> CapabilityStatus {
        self.capabilities
            .iter()
            .find(|entry| entry.capability == capability)
            .map_or(CapabilityStatus::Unknown, |entry| entry.status)
    }

    pub fn has_available_capability(&self) -> bool {
        self.capabilities
            .iter()
            .any(|entry| entry.status.is_available())
    }
}

pub fn validated_codex_0_144_6_fingerprint() -> ProviderFingerprint {
    ProviderFingerprint {
        canonical_executable: PathBuf::from(
            "/opt/homebrew/Caskroom/codex/0.144.6/codex-aarch64-apple-darwin",
        ),
        executable_version: "0.144.6".to_owned(),
        executable_sha256: "80a3933d11a9d13ef806aa24f7bb8afc9169cfe4e9b09d6da6a92922cbde9cff"
            .to_owned(),
        combined_schema_sha256: "85ea836927d6cfdd3c68a9bda17dba48d2573bbc282ab2d5775a5005e40bc9c3"
            .to_owned(),
        v2_schema_sha256: "8928c45789c653017f967b59035b0bf802648d3259d328c1b7b37a8191b177ca"
            .to_owned(),
        method_allowlist_sha256: "eceb94d9e824065899efeebcbe191a772458b7330e26b15c9f91604103153ba2"
            .to_owned(),
        fixture_sha256: "a3debd88e389320edf899c0a3399accca500bd6d5632c6862d5ac2c12ad73f8b"
            .to_owned(),
        smoke_run_id: "2026-07-21-arm64-3ff2583".to_owned(),
    }
}

pub fn validated_codex_0_145_0_fingerprint() -> ProviderFingerprint {
    ProviderFingerprint {
        canonical_executable: PathBuf::from(
            "/opt/homebrew/Caskroom/codex/0.145.0/codex-aarch64-apple-darwin",
        ),
        executable_version: "0.145.0".to_owned(),
        executable_sha256: "1da3f4e0e96028b8a771814293c3033dafd1971f943f6c7e79b0897fe705f590"
            .to_owned(),
        combined_schema_sha256: "1f66700d1cc3de4a5004e5614a6098878b405c7e7c5f8c9be97fc900d0ad6c68"
            .to_owned(),
        v2_schema_sha256: "84bc00660a8c4e69073f4f0bafcf00ec5b7238dbe59eccf404ce2352daae64e0"
            .to_owned(),
        method_allowlist_sha256: "0de966cd124a25c926df49f4b697e588d51947c31c4e2febe2175338f6319d42"
            .to_owned(),
        fixture_sha256: "a41be68b42ee08b82b9ebdee78bae9fcaa01501671e824e79e988d0e8c93827b"
            .to_owned(),
        smoke_run_id: "2026-07-27-arm64-s0-9".to_owned(),
    }
}

pub fn classify_codex(fingerprint: &ProviderFingerprint) -> ProviderCapabilitySnapshot {
    let legacy_mismatches =
        fingerprint_mismatches(fingerprint, &validated_codex_0_144_6_fingerprint());
    let manual_mismatches =
        fingerprint_mismatches(fingerprint, &validated_codex_0_145_0_fingerprint());
    if legacy_mismatches.is_empty() {
        ProviderCapabilitySnapshot {
            provider: ProviderKind::Codex,
            compatibility: ProviderCompatibility::Supported,
            capabilities: codex_0_144_6_capabilities(),
            fingerprint_mismatches: Vec::new(),
        }
    } else if manual_mismatches.is_empty() {
        ProviderCapabilitySnapshot {
            provider: ProviderKind::Codex,
            compatibility: ProviderCompatibility::Supported,
            capabilities: codex_0_145_0_capabilities(),
            fingerprint_mismatches: Vec::new(),
        }
    } else {
        let fingerprint_mismatches = if legacy_mismatches.len() <= manual_mismatches.len() {
            legacy_mismatches
        } else {
            manual_mismatches
        };
        ProviderCapabilitySnapshot {
            provider: ProviderKind::Codex,
            compatibility: ProviderCompatibility::Unknown,
            capabilities: ProviderCapability::ALL
                .map(|capability| CapabilityEntry {
                    capability,
                    status: CapabilityStatus::Unknown,
                })
                .to_vec(),
            fingerprint_mismatches,
        }
    }
}

fn codex_0_145_0_capabilities() -> Vec<CapabilityEntry> {
    let mut capabilities = codex_0_144_6_capabilities();
    for entry in &mut capabilities {
        if entry.capability == ProviderCapability::PermissionPolicyConfigure {
            entry.status = CapabilityStatus::Supported;
        }
    }
    capabilities
}

fn fingerprint_mismatches(
    observed: &ProviderFingerprint,
    expected: &ProviderFingerprint,
) -> Vec<FingerprintAxis> {
    let mut mismatches = Vec::new();
    if observed.canonical_executable != expected.canonical_executable {
        mismatches.push(FingerprintAxis::CanonicalExecutable);
    }
    if observed.executable_version != expected.executable_version {
        mismatches.push(FingerprintAxis::ExecutableVersion);
    }
    if observed.executable_sha256 != expected.executable_sha256 {
        mismatches.push(FingerprintAxis::ExecutableSha256);
    }
    if observed.combined_schema_sha256 != expected.combined_schema_sha256 {
        mismatches.push(FingerprintAxis::CombinedSchemaSha256);
    }
    if observed.v2_schema_sha256 != expected.v2_schema_sha256 {
        mismatches.push(FingerprintAxis::V2SchemaSha256);
    }
    if observed.method_allowlist_sha256 != expected.method_allowlist_sha256 {
        mismatches.push(FingerprintAxis::MethodAllowlistSha256);
    }
    if observed.fixture_sha256 != expected.fixture_sha256 {
        mismatches.push(FingerprintAxis::FixtureSha256);
    }
    if observed.smoke_run_id != expected.smoke_run_id {
        mismatches.push(FingerprintAxis::SmokeRunId);
    }
    mismatches
}

fn codex_0_144_6_capabilities() -> Vec<CapabilityEntry> {
    use CapabilityStatus::{Degraded, Supported, Unsupported};
    use ProviderCapability::{
        CompletionDetect, ContinueAfterQuit, History, Launch, ListManaged, OpenInProvider,
        PermissionDetect, PermissionPolicyConfigure, PermissionPolicyObserve, PermissionRespond,
        QuestionDetect, QuestionRespond, Reconcile, Resume, Stop, StructuredActivity,
    };

    [
        (Launch, Supported),
        (ListManaged, Supported),
        (Resume, Supported),
        (Reconcile, Supported),
        (StructuredActivity, Degraded),
        (PermissionDetect, Degraded),
        (PermissionRespond, Unsupported),
        (PermissionPolicyConfigure, Unsupported),
        (PermissionPolicyObserve, Unsupported),
        (QuestionDetect, Supported),
        (QuestionRespond, Degraded),
        (CompletionDetect, Supported),
        (History, Unsupported),
        (OpenInProvider, Unsupported),
        (ContinueAfterQuit, Unsupported),
        (Stop, Supported),
    ]
    .map(|(capability, status)| CapabilityEntry { capability, status })
    .to_vec()
}
