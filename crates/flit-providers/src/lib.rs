use std::path::PathBuf;

mod codex_contract;
mod executable;
mod probe;
mod process;
mod profile;
mod schema;
mod version;

pub use codex_contract::{
    CodexContractError, CodexManagedListPage, CodexManagedScope, CodexManagedThreadConflict,
    CodexManagedThreadId, CodexStartedThread, CodexThreadRead, CodexThreadState,
    MAX_CODEX_APP_SERVER_FRAME_BYTES, MAX_CODEX_MANAGED_THREADS, codex_initialize_request,
    codex_initialized_notification, codex_read_only_start_request, codex_read_request,
    codex_thread_list_request, decode_codex_read_response, decode_codex_start_response,
    decode_codex_thread_list_response,
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

pub fn classify_codex(fingerprint: &ProviderFingerprint) -> ProviderCapabilitySnapshot {
    let expected = validated_codex_0_144_6_fingerprint();
    let fingerprint_mismatches = fingerprint_mismatches(fingerprint, &expected);
    if fingerprint_mismatches.is_empty() {
        ProviderCapabilitySnapshot {
            provider: ProviderKind::Codex,
            compatibility: ProviderCompatibility::Supported,
            capabilities: codex_0_144_6_capabilities(),
            fingerprint_mismatches,
        }
    } else {
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
