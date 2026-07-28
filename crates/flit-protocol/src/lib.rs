use std::collections::BTreeMap;

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: &str = "1.12";
pub const EVENT_PROTOCOL_VERSION: &str = "1.0";
pub const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[must_use]
pub fn event_schema_relative_path() -> String {
    format!("schemas/protocol/events/v{EVENT_PROTOCOL_VERSION}/event.schema.json")
}

#[must_use]
pub fn event_schema_id() -> String {
    format!("urn:flit:protocol:event:{EVENT_PROTOCOL_VERSION}")
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ready,
    NotConfigured,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemHealthRequest {
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemHealthResponse {
    pub protocol_version: String,
    pub core: HealthStatus,
    pub storage: HealthStatus,
    pub providers: HealthStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectInspectionRequest {
    pub selected_path: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectInspectionResponse {
    pub protocol_version: String,
    pub canonical_path: String,
    pub filesystem_id: String,
    pub selected_via_symlink: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectRegistrationRequest {
    pub project_id: String,
    pub display_name: String,
    pub selected_path: String,
    pub created_at: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectRecord {
    pub id: String,
    pub display_name: String,
    pub canonical_path: String,
    pub filesystem_id: Option<String>,
    pub trusted: bool,
    pub default_provider: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRegistrationStatus {
    Registered,
    DuplicateCanonicalPath,
    DuplicateFilesystemIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectRegistrationResponse {
    pub protocol_version: String,
    pub status: ProjectRegistrationStatus,
    pub project: Option<ProjectRecord>,
    pub existing_project_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectTrustRequest {
    pub project_id: String,
    pub selected_path: String,
    pub confirmed_at: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectTrustStatus {
    Trusted,
    AlreadyTrusted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectTrustResponse {
    pub protocol_version: String,
    pub status: ProjectTrustStatus,
    pub project: ProjectRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectListCursor {
    pub display_name: String,
    pub project_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectsListRequest {
    pub after: Option<ProjectListCursor>,
    pub limit: u32,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectsListResponse {
    pub protocol_version: String,
    pub projects: Vec<ProjectRecord>,
    pub next_cursor: Option<ProjectListCursor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DashboardReadRequest {
    pub expected_core_instance_id: Option<String>,
    pub after_cursor: Option<u64>,
    pub requested_event_limit: u32,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardSnapshotReason {
    Initial,
    CoreInstanceMismatch,
    CursorAhead,
    CursorExpired,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DashboardRunRecord {
    pub run_id: String,
    pub project_id: String,
    pub project_display_name: String,
    pub title: String,
    pub provider: ProviderKind,
    pub version: u64,
    pub lifecycle: String,
    pub activity: String,
    pub activity_confidence: f64,
    pub attention_level: String,
    pub attention_open_count: u64,
    pub dashboard_bucket: String,
    pub last_progress_at: Option<String>,
    pub last_liveness_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub changes: DashboardChangeSummary,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum DashboardChangeSummary {
    Available {
        files: u64,
        insertions: u64,
        deletions: u64,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DashboardEventRecord {
    pub cursor: u64,
    pub event_id: String,
    pub run_id: String,
    pub event_type: String,
    pub observed_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "delivery", rename_all = "snake_case")]
pub enum DashboardReadResponse {
    Snapshot {
        protocol_version: String,
        event_schema_version: String,
        core_instance_id: String,
        reason: DashboardSnapshotReason,
        requested_after_cursor: Option<u64>,
        retained_after_cursor: u64,
        next_cursor: u64,
        has_more: bool,
        runs: Vec<DashboardRunRecord>,
    },
    Delta {
        protocol_version: String,
        event_schema_version: String,
        core_instance_id: String,
        requested_after_cursor: u64,
        retained_after_cursor: u64,
        next_cursor: u64,
        has_more: bool,
        events: Vec<DashboardEventRecord>,
        runs: Vec<DashboardRunRecord>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunDetailReadRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub after_cursor: u64,
    pub requested_event_limit: u32,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunEvidenceRecord {
    pub cursor: u64,
    pub event_id: String,
    pub session_id: Option<String>,
    pub event_type: String,
    pub source_kind: EventSourceKind,
    pub confidence: f64,
    pub observed_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunDetailReadResponse {
    pub protocol_version: String,
    pub event_schema_version: String,
    pub run_id: String,
    pub run_version: u64,
    pub next_cursor: u64,
    pub has_more: bool,
    pub history_status: CapabilityStatus,
    pub open_in_provider_status: CapabilityStatus,
    pub events: Vec<RunEvidenceRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunOpenInProviderRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDiagnosticsRequest {
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuitImpactRequest {
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Codex,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCompatibility {
    Supported,
    Degraded,
    Unknown,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapability {
    Launch,
    ListManaged,
    Resume,
    Reconcile,
    StructuredActivity,
    PermissionDetect,
    PermissionRespond,
    PermissionModeConfigure,
    ProviderOutcomeObserve,
    QuestionDetect,
    QuestionRespond,
    CompletionDetect,
    History,
    OpenInProvider,
    ContinueAfterQuit,
    Stop,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Degraded,
    Unsupported,
    Unknown,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderCapabilityEntry {
    pub capability: ProviderCapability,
    pub status: CapabilityStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUnavailableReason {
    ExecutableNotFound,
    ExecutableUnavailable,
    VersionProbeFailed,
    SchemaProbeFailed,
    BundledEvidenceMismatch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDiagnosticsResponse {
    pub protocol_version: String,
    pub provider: ProviderKind,
    pub compatibility: ProviderCompatibility,
    pub executable_version: Option<String>,
    pub capabilities: Vec<ProviderCapabilityEntry>,
    pub fingerprint_mismatches: Vec<FingerprintAxis>,
    pub unavailable_reason: Option<ProviderUnavailableReason>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderExecutionAfterQuit {
    Continues,
    Stops,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuitImpactReason {
    CapabilitySupported,
    CapabilityUnsupported,
    CapabilityUncertain,
    CapabilityMissing,
    CapabilityInvalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuitImpactRun {
    pub run_id: String,
    pub title: String,
    pub provider: ProviderKind,
    pub execution_after_quit: ProviderExecutionAfterQuit,
    pub reason: QuitImpactReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuitImpactResponse {
    pub protocol_version: String,
    pub core_instance_id: String,
    pub cursor: u64,
    pub flit_monitoring_stops: bool,
    pub flit_notifications_stop: bool,
    pub runs: Vec<QuitImpactRun>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRunPermissionMode {
    Manual,
    ProviderAuto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunStartRequest {
    pub run_id: String,
    pub session_id: String,
    pub project_id: String,
    pub title: String,
    pub goal: String,
    pub provider: ProviderKind,
    pub permission_mode: ManagedRunPermissionMode,
    pub permission_mode_version: u64,
    pub created_at: String,
    pub started_at: String,
    pub run_created_event_id: String,
    pub start_requested_event_id: String,
    pub session_connected_event_id: String,
    pub start_failed_event_id: String,
    pub start_unknown_event_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunStartResponse {
    pub protocol_version: String,
    pub run_id: String,
    pub session_id: String,
    pub provider_thread_id: String,
    pub provider_turn_id: String,
    pub permission_mode: ManagedRunPermissionMode,
    pub permission_mode_version: u64,
    pub provider_configuration: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunObserveRequest {
    pub run_id: String,
    pub observed_at: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ManagedRunObserveResponse {
    PermissionRequested {
        protocol_version: String,
        run_id: String,
        session_id: String,
        provider_thread_id: String,
        provider_turn_id: String,
        provider_item_id: String,
        provider_request_id: u64,
        request_id: String,
        request_version: u64,
        event_id: String,
    },
    ProviderOutcomeResolved {
        protocol_version: String,
        run_id: String,
        session_id: String,
        provider_thread_id: String,
        provider_turn_id: String,
        provider_item_id: String,
        provider_decision_id: String,
        request_id: String,
        request_version: u64,
        request_event_id: String,
        provider_decision: ManagedRunProviderDecision,
        terminal_outcome: ManagedRunProviderTerminalOutcome,
        event_id: String,
        event_version: u64,
    },
    TurnCompleted {
        protocol_version: String,
        run_id: String,
        session_id: String,
        provider_thread_id: String,
        provider_turn_id: String,
        event_id: String,
        event_version: u64,
    },
    TurnInterrupted {
        protocol_version: String,
        run_id: String,
        session_id: String,
        provider_thread_id: String,
        provider_turn_id: String,
        event_id: String,
        event_version: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRunProviderDecision {
    Allowed,
    Denied,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRunProviderTerminalOutcome {
    RequestResolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRunPermissionDecision {
    AllowOnce,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunPermissionRespondRequest {
    pub run_id: String,
    pub request_id: String,
    pub request_version: u64,
    pub response_attempt_id: String,
    pub decision: ManagedRunPermissionDecision,
    pub submitted_at: String,
    pub finished_at: String,
    pub submitted_event_id: String,
    pub resolved_event_id: String,
    pub delivery_unknown_event_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ManagedRunPermissionRespondResponse {
    Delivered {
        protocol_version: String,
        run_id: String,
        request_id: String,
        request_version: u64,
        response_attempt_id: String,
        decision: ManagedRunPermissionDecision,
        submitted_event_id: String,
        submitted_version: u64,
        outcome_event_id: String,
        outcome_version: u64,
    },
    DeliveryUnknown {
        protocol_version: String,
        run_id: String,
        request_id: String,
        request_version: u64,
        response_attempt_id: String,
        decision: ManagedRunPermissionDecision,
        submitted_event_id: String,
        submitted_version: u64,
        outcome_event_id: String,
        outcome_version: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandErrorCode {
    ProtocolMismatch,
    InvalidProjectRequest,
    InvalidDashboardRequest,
    ProjectInspectionFailure,
    ProjectConflict,
    ProjectNotFound,
    ProjectIdentityMismatch,
    StorageUnavailable,
    InvalidRunRequest,
    RunNotFound,
    RunVersionStale,
    RunConflict,
    ProjectNotTrusted,
    ProviderUnavailable,
    CapabilityUnsupported,
    ProviderStartFailed,
    ProviderStartUnknown,
    ManagedRunNotActive,
    ProviderObservationUnknown,
    PermissionRequestStale,
    PermissionResponseConflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub enum EventProtocolVersion {
    #[serde(rename = "1.0")]
    V1_0,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSourceKind {
    Core,
    ProviderAdapter,
    GitWatcher,
    FileWatcher,
    Classifier,
    Policy,
    Ui,
    Notifier,
    Recovery,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct EventSource {
    pub kind: EventSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_version: Option<String>,
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NullableSessionId {
    Id(String),
    Null,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct EventEnvelope {
    pub protocol_version: EventProtocolVersion,
    #[schemars(length(min = 1))]
    pub event_id: String,
    #[schemars(length(min = 1))]
    pub run_id: String,
    pub session_id: NullableSessionId,
    #[schemars(range(min = 1, max = MAX_JSON_SAFE_INTEGER))]
    pub stream_seq: u64,
    #[schemars(range(min = 1, max = MAX_JSON_SAFE_INTEGER))]
    pub ingest_seq: u64,
    #[schemars(length(min = 1))]
    pub occurred_at: String,
    #[schemars(length(min = 1))]
    pub observed_at: String,
    #[serde(rename = "type")]
    #[schemars(length(min = 1))]
    pub event_type: String,
    pub source: EventSource,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub confidence: f64,
    #[schemars(inner(length(min = 1)))]
    pub evidence_ids: Vec<String>,
    pub payload: Map<String, Value>,
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UnsequencedEventEnvelope {
    pub protocol_version: EventProtocolVersion,
    pub event_id: String,
    pub run_id: String,
    pub session_id: NullableSessionId,
    pub stream_seq: u64,
    pub occurred_at: String,
    pub observed_at: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub source: EventSource,
    pub confidence: f64,
    pub evidence_ids: Vec<String>,
    pub payload: Map<String, Value>,
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl UnsequencedEventEnvelope {
    #[must_use]
    pub fn with_ingest_seq(self, ingest_seq: u64) -> EventEnvelope {
        EventEnvelope {
            protocol_version: self.protocol_version,
            event_id: self.event_id,
            run_id: self.run_id,
            session_id: self.session_id,
            stream_seq: self.stream_seq,
            ingest_seq,
            occurred_at: self.occurred_at,
            observed_at: self.observed_at,
            event_type: self.event_type,
            source: self.source,
            confidence: self.confidence,
            evidence_ids: self.evidence_ids,
            payload: self.payload,
            extensions: self.extensions,
        }
    }
}

impl From<EventEnvelope> for UnsequencedEventEnvelope {
    fn from(event: EventEnvelope) -> Self {
        Self {
            protocol_version: event.protocol_version,
            event_id: event.event_id,
            run_id: event.run_id,
            session_id: event.session_id,
            stream_seq: event.stream_seq,
            occurred_at: event.occurred_at,
            observed_at: event.observed_at,
            event_type: event.event_type,
            source: event.source,
            confidence: event.confidence,
            evidence_ids: event.evidence_ids,
            payload: event.payload,
            extensions: event.extensions,
        }
    }
}

impl CommandError {
    #[must_use]
    pub fn for_code(code: CommandErrorCode) -> Self {
        let message_key = match code {
            CommandErrorCode::ProtocolMismatch => "errors.protocolMismatch",
            CommandErrorCode::InvalidProjectRequest => "errors.invalidProjectRequest",
            CommandErrorCode::InvalidDashboardRequest => "errors.invalidDashboardRequest",
            CommandErrorCode::ProjectInspectionFailure => "errors.projectInspectionFailure",
            CommandErrorCode::ProjectConflict => "errors.projectConflict",
            CommandErrorCode::ProjectNotFound => "errors.projectNotFound",
            CommandErrorCode::ProjectIdentityMismatch => "errors.projectIdentityMismatch",
            CommandErrorCode::StorageUnavailable => "errors.storageUnavailable",
            CommandErrorCode::InvalidRunRequest => "errors.invalidRunRequest",
            CommandErrorCode::RunNotFound => "errors.runNotFound",
            CommandErrorCode::RunVersionStale => "errors.runVersionStale",
            CommandErrorCode::RunConflict => "errors.runConflict",
            CommandErrorCode::ProjectNotTrusted => "errors.projectNotTrusted",
            CommandErrorCode::ProviderUnavailable => "errors.providerUnavailable",
            CommandErrorCode::CapabilityUnsupported => "errors.capabilityUnsupported",
            CommandErrorCode::ProviderStartFailed => "errors.providerStartFailed",
            CommandErrorCode::ProviderStartUnknown => "errors.providerStartUnknown",
            CommandErrorCode::ManagedRunNotActive => "errors.managedRunNotActive",
            CommandErrorCode::ProviderObservationUnknown => "errors.providerObservationUnknown",
            CommandErrorCode::PermissionRequestStale => "errors.permissionRequestStale",
            CommandErrorCode::PermissionResponseConflict => "errors.permissionResponseConflict",
        };
        Self {
            code,
            message_key: message_key.to_owned(),
        }
    }

    #[must_use]
    pub fn protocol_mismatch() -> Self {
        Self::for_code(CommandErrorCode::ProtocolMismatch)
    }
}

#[must_use]
pub fn generated_swift_command_contract() -> String {
    SWIFT_COMMAND_CONTRACT_TEMPLATE
        .replace("__PROTOCOL_VERSION__", PROTOCOL_VERSION)
        .replace("__EVENT_SCHEMA_VERSION__", EVENT_PROTOCOL_VERSION)
}

const SWIFT_COMMAND_CONTRACT_TEMPLATE: &str = r#"// Generated from flit_protocol command contracts. Do not edit.
let flitClientProtocolVersion = "__PROTOCOL_VERSION__"
let flitEventSchemaVersion = "__EVENT_SCHEMA_VERSION__"

struct FlitProjectInspectionRequest: Codable, Equatable, Sendable {
    let selectedPath: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case selectedPath = "selected_path"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitProjectInspectionResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let canonicalPath: String
    let filesystemId: String
    let selectedViaSymlink: Bool

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case canonicalPath = "canonical_path"
        case filesystemId = "filesystem_id"
        case selectedViaSymlink = "selected_via_symlink"
    }
}

struct FlitProjectRegistrationRequest: Codable, Equatable, Sendable {
    let projectId: String
    let displayName: String
    let selectedPath: String
    let createdAt: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case projectId = "project_id"
        case displayName = "display_name"
        case selectedPath = "selected_path"
        case createdAt = "created_at"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitProjectRegistrationStatus: String, Codable, Sendable {
    case registered
    case duplicateCanonicalPath = "duplicate_canonical_path"
    case duplicateFilesystemIdentity = "duplicate_filesystem_identity"
}

struct FlitProjectRecord: Codable, Equatable, Sendable {
    let id: String
    let displayName: String
    let canonicalPath: String
    let filesystemId: String?
    let trusted: Bool
    let defaultProvider: String?
    let createdAt: String
    let updatedAt: String

    private enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case canonicalPath = "canonical_path"
        case filesystemId = "filesystem_id"
        case trusted
        case defaultProvider = "default_provider"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

struct FlitProjectRegistrationResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let status: FlitProjectRegistrationStatus
    let project: FlitProjectRecord?
    let existingProjectId: String?

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case status
        case project
        case existingProjectId = "existing_project_id"
    }
}

struct FlitProjectTrustRequest: Codable, Equatable, Sendable {
    let projectId: String
    let selectedPath: String
    let confirmedAt: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case projectId = "project_id"
        case selectedPath = "selected_path"
        case confirmedAt = "confirmed_at"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitProjectTrustStatus: String, Codable, Sendable {
    case trusted
    case alreadyTrusted = "already_trusted"
}

struct FlitProjectTrustResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let status: FlitProjectTrustStatus
    let project: FlitProjectRecord

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case status
        case project
    }
}

struct FlitProjectListCursor: Codable, Equatable, Sendable {
    let displayName: String
    let projectId: String

    private enum CodingKeys: String, CodingKey {
        case displayName = "display_name"
        case projectId = "project_id"
    }
}

struct FlitProjectsListRequest: Codable, Equatable, Sendable {
    let after: FlitProjectListCursor?
    let limit: UInt32
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case after
        case limit
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitProjectsListResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let projects: [FlitProjectRecord]
    let nextCursor: FlitProjectListCursor?

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case projects
        case nextCursor = "next_cursor"
    }
}

struct FlitDashboardReadRequest: Codable, Equatable, Sendable {
    let expectedCoreInstanceId: String?
    let afterCursor: UInt64?
    let requestedEventLimit: UInt32
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case expectedCoreInstanceId = "expected_core_instance_id"
        case afterCursor = "after_cursor"
        case requestedEventLimit = "requested_event_limit"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitDashboardDelivery: String, Codable, Sendable {
    case snapshot
    case delta
}

enum FlitDashboardSnapshotReason: String, Codable, Sendable {
    case initial
    case coreInstanceMismatch = "core_instance_mismatch"
    case cursorAhead = "cursor_ahead"
    case cursorExpired = "cursor_expired"
}

struct FlitDashboardRunRecord: Codable, Equatable, Sendable {
    let runId: String
    let projectId: String
    let projectDisplayName: String
    let title: String
    let provider: FlitProviderKind
    let version: UInt64
    let lifecycle: String
    let activity: String
    let activityConfidence: Double
    let attentionLevel: String
    let attentionOpenCount: UInt64
    let dashboardBucket: String
    let lastProgressAt: String?
    let lastLivenessAt: String?
    let startedAt: String?
    let endedAt: String?
    let changes: FlitDashboardChangeSummary
    let updatedAt: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case projectId = "project_id"
        case projectDisplayName = "project_display_name"
        case title
        case provider
        case version
        case lifecycle
        case activity
        case activityConfidence = "activity_confidence"
        case attentionLevel = "attention_level"
        case attentionOpenCount = "attention_open_count"
        case dashboardBucket = "dashboard_bucket"
        case lastProgressAt = "last_progress_at"
        case lastLivenessAt = "last_liveness_at"
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case changes
        case updatedAt = "updated_at"
    }
}

enum FlitDashboardChangeAvailability: String, Codable, Sendable {
    case available
    case unavailable
}

enum FlitDashboardChangeSummary: Codable, Equatable, Sendable {
    case available(files: UInt64, insertions: UInt64, deletions: UInt64)
    case unavailable(reason: String)

    private enum CodingKeys: String, CodingKey {
        case availability
        case files
        case insertions
        case deletions
        case reason
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(
            FlitDashboardChangeAvailability.self,
            forKey: .availability
        ) {
        case .available:
            guard !container.contains(.reason) else {
                throw DecodingError.dataCorruptedError(
                    forKey: .reason,
                    in: container,
                    debugDescription: "Available changes cannot include an unavailable reason"
                )
            }
            self = .available(
                files: try container.decode(UInt64.self, forKey: .files),
                insertions: try container.decode(UInt64.self, forKey: .insertions),
                deletions: try container.decode(UInt64.self, forKey: .deletions)
            )
        case .unavailable:
            guard
                !container.contains(.files),
                !container.contains(.insertions),
                !container.contains(.deletions)
            else {
                throw DecodingError.dataCorruptedError(
                    forKey: .availability,
                    in: container,
                    debugDescription: "Unavailable changes cannot include numeric counts"
                )
            }
            self = .unavailable(
                reason: try container.decode(String.self, forKey: .reason)
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .available(files, insertions, deletions):
            try container.encode(
                FlitDashboardChangeAvailability.available,
                forKey: .availability
            )
            try container.encode(files, forKey: .files)
            try container.encode(insertions, forKey: .insertions)
            try container.encode(deletions, forKey: .deletions)
        case let .unavailable(reason):
            try container.encode(
                FlitDashboardChangeAvailability.unavailable,
                forKey: .availability
            )
            try container.encode(reason, forKey: .reason)
        }
    }
}

struct FlitDashboardEventRecord: Codable, Equatable, Sendable {
    let cursor: UInt64
    let eventId: String
    let runId: String
    let eventType: String
    let observedAt: String

    private enum CodingKeys: String, CodingKey {
        case cursor
        case eventId = "event_id"
        case runId = "run_id"
        case eventType = "event_type"
        case observedAt = "observed_at"
    }
}

struct FlitDashboardSnapshotResponse: Codable, Equatable, Sendable {
    let delivery: FlitDashboardDelivery
    let protocolVersion: String
    let eventSchemaVersion: String
    let coreInstanceId: String
    let reason: FlitDashboardSnapshotReason
    let requestedAfterCursor: UInt64?
    let retainedAfterCursor: UInt64
    let nextCursor: UInt64
    let hasMore: Bool
    let runs: [FlitDashboardRunRecord]

    private enum CodingKeys: String, CodingKey {
        case delivery
        case protocolVersion = "protocol_version"
        case eventSchemaVersion = "event_schema_version"
        case coreInstanceId = "core_instance_id"
        case reason
        case requestedAfterCursor = "requested_after_cursor"
        case retainedAfterCursor = "retained_after_cursor"
        case nextCursor = "next_cursor"
        case hasMore = "has_more"
        case runs
    }
}

struct FlitDashboardDeltaResponse: Codable, Equatable, Sendable {
    let delivery: FlitDashboardDelivery
    let protocolVersion: String
    let eventSchemaVersion: String
    let coreInstanceId: String
    let requestedAfterCursor: UInt64
    let retainedAfterCursor: UInt64
    let nextCursor: UInt64
    let hasMore: Bool
    let events: [FlitDashboardEventRecord]
    let runs: [FlitDashboardRunRecord]

    private enum CodingKeys: String, CodingKey {
        case delivery
        case protocolVersion = "protocol_version"
        case eventSchemaVersion = "event_schema_version"
        case coreInstanceId = "core_instance_id"
        case requestedAfterCursor = "requested_after_cursor"
        case retainedAfterCursor = "retained_after_cursor"
        case nextCursor = "next_cursor"
        case hasMore = "has_more"
        case events
        case runs
    }
}

enum FlitDashboardReadResponse: Codable, Equatable, Sendable {
    case snapshot(FlitDashboardSnapshotResponse)
    case delta(FlitDashboardDeltaResponse)

    private enum CodingKeys: String, CodingKey {
        case delivery
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(FlitDashboardDelivery.self, forKey: .delivery) {
        case .snapshot:
            self = .snapshot(try FlitDashboardSnapshotResponse(from: decoder))
        case .delta:
            self = .delta(try FlitDashboardDeltaResponse(from: decoder))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .snapshot(response):
            try response.encode(to: encoder)
        case let .delta(response):
            try response.encode(to: encoder)
        }
    }
}

struct FlitRunDetailReadRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let afterCursor: UInt64
    let requestedEventLimit: UInt32
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case afterCursor = "after_cursor"
        case requestedEventLimit = "requested_event_limit"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitEventSourceKind: String, Codable, Sendable {
    case core
    case providerAdapter = "provider_adapter"
    case gitWatcher = "git_watcher"
    case fileWatcher = "file_watcher"
    case classifier
    case policy
    case ui
    case notifier
    case recovery
}

struct FlitRunEvidenceRecord: Codable, Equatable, Sendable {
    let cursor: UInt64
    let eventId: String
    let sessionId: String?
    let eventType: String
    let sourceKind: FlitEventSourceKind
    let confidence: Double
    let observedAt: String

    private enum CodingKeys: String, CodingKey {
        case cursor
        case eventId = "event_id"
        case sessionId = "session_id"
        case eventType = "event_type"
        case sourceKind = "source_kind"
        case confidence
        case observedAt = "observed_at"
    }
}

struct FlitRunDetailReadResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let eventSchemaVersion: String
    let runId: String
    let runVersion: UInt64
    let nextCursor: UInt64
    let hasMore: Bool
    let historyStatus: FlitCapabilityStatus
    let openInProviderStatus: FlitCapabilityStatus
    let events: [FlitRunEvidenceRecord]

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case eventSchemaVersion = "event_schema_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case nextCursor = "next_cursor"
        case hasMore = "has_more"
        case historyStatus = "history_status"
        case openInProviderStatus = "open_in_provider_status"
        case events
    }
}

struct FlitManagedRunOpenInProviderRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitCommandErrorCode: String, Codable, Sendable {
    case protocolMismatch = "PROTOCOL_MISMATCH"
    case invalidProjectRequest = "INVALID_PROJECT_REQUEST"
    case invalidDashboardRequest = "INVALID_DASHBOARD_REQUEST"
    case projectInspectionFailure = "PROJECT_INSPECTION_FAILURE"
    case projectConflict = "PROJECT_CONFLICT"
    case projectNotFound = "PROJECT_NOT_FOUND"
    case projectIdentityMismatch = "PROJECT_IDENTITY_MISMATCH"
    case storageUnavailable = "STORAGE_UNAVAILABLE"
    case invalidRunRequest = "INVALID_RUN_REQUEST"
    case runNotFound = "RUN_NOT_FOUND"
    case runVersionStale = "RUN_VERSION_STALE"
    case runConflict = "RUN_CONFLICT"
    case projectNotTrusted = "PROJECT_NOT_TRUSTED"
    case providerUnavailable = "PROVIDER_UNAVAILABLE"
    case capabilityUnsupported = "CAPABILITY_UNSUPPORTED"
    case providerStartFailed = "PROVIDER_START_FAILED"
    case providerStartUnknown = "PROVIDER_START_UNKNOWN"
    case managedRunNotActive = "MANAGED_RUN_NOT_ACTIVE"
    case providerObservationUnknown = "PROVIDER_OBSERVATION_UNKNOWN"
    case permissionRequestStale = "PERMISSION_REQUEST_STALE"
    case permissionResponseConflict = "PERMISSION_RESPONSE_CONFLICT"
}

struct FlitCommandError: Codable, Equatable, Sendable {
    let code: FlitCommandErrorCode
    let messageKey: String

    private enum CodingKeys: String, CodingKey {
        case code
        case messageKey = "message_key"
    }
}

struct FlitProviderDiagnosticsRequest: Codable, Equatable, Sendable {
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitProviderKind: String, Codable, Sendable {
    case codex
}

enum FlitProviderCompatibility: String, Codable, Sendable {
    case supported
    case degraded
    case unknown
    case unavailable
}

enum FlitProviderCapability: String, Codable, Sendable {
    case launch
    case listManaged = "list_managed"
    case resume
    case reconcile
    case structuredActivity = "structured_activity"
    case permissionDetect = "permission_detect"
    case permissionRespond = "permission_respond"
    case permissionModeConfigure = "permission_mode_configure"
    case providerOutcomeObserve = "provider_outcome_observe"
    case questionDetect = "question_detect"
    case questionRespond = "question_respond"
    case completionDetect = "completion_detect"
    case history
    case openInProvider = "open_in_provider"
    case continueAfterQuit = "continue_after_quit"
    case stop
}

enum FlitCapabilityStatus: String, Codable, Sendable {
    case supported
    case degraded
    case unsupported
    case unknown
    case unavailable
}

struct FlitProviderCapabilityEntry: Codable, Equatable, Sendable {
    let capability: FlitProviderCapability
    let status: FlitCapabilityStatus
}

enum FlitFingerprintAxis: String, Codable, Sendable {
    case canonicalExecutable = "canonical_executable"
    case executableVersion = "executable_version"
    case executableSha256 = "executable_sha256"
    case combinedSchemaSha256 = "combined_schema_sha256"
    case v2SchemaSha256 = "v2_schema_sha256"
    case methodAllowlistSha256 = "method_allowlist_sha256"
    case fixtureSha256 = "fixture_sha256"
    case smokeRunId = "smoke_run_id"
}

enum FlitProviderUnavailableReason: String, Codable, Sendable {
    case executableNotFound = "executable_not_found"
    case executableUnavailable = "executable_unavailable"
    case versionProbeFailed = "version_probe_failed"
    case schemaProbeFailed = "schema_probe_failed"
    case bundledEvidenceMismatch = "bundled_evidence_mismatch"
}

struct FlitProviderDiagnosticsResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let provider: FlitProviderKind
    let compatibility: FlitProviderCompatibility
    let executableVersion: String?
    let capabilities: [FlitProviderCapabilityEntry]
    let fingerprintMismatches: [FlitFingerprintAxis]
    let unavailableReason: FlitProviderUnavailableReason?

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case provider
        case compatibility
        case executableVersion = "executable_version"
        case capabilities
        case fingerprintMismatches = "fingerprint_mismatches"
        case unavailableReason = "unavailable_reason"
    }
}

struct FlitQuitImpactRequest: Codable, Equatable, Sendable {
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitProviderExecutionAfterQuit: String, Codable, Sendable {
    case continues
    case stops
    case unknown
}

enum FlitQuitImpactReason: String, Codable, Sendable {
    case capabilitySupported = "capability_supported"
    case capabilityUnsupported = "capability_unsupported"
    case capabilityUncertain = "capability_uncertain"
    case capabilityMissing = "capability_missing"
    case capabilityInvalid = "capability_invalid"
}

struct FlitQuitImpactRun: Codable, Equatable, Sendable {
    let runId: String
    let title: String
    let provider: FlitProviderKind
    let executionAfterQuit: FlitProviderExecutionAfterQuit
    let reason: FlitQuitImpactReason

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case title
        case provider
        case executionAfterQuit = "execution_after_quit"
        case reason
    }
}

struct FlitQuitImpactResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let coreInstanceId: String
    let cursor: UInt64
    let flitMonitoringStops: Bool
    let flitNotificationsStop: Bool
    let runs: [FlitQuitImpactRun]

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case coreInstanceId = "core_instance_id"
        case cursor
        case flitMonitoringStops = "flit_monitoring_stops"
        case flitNotificationsStop = "flit_notifications_stop"
        case runs
    }
}

enum FlitManagedRunPermissionMode: String, Codable, Sendable {
    case manual
    case providerAuto = "provider_auto"
}

struct FlitManagedRunStartRequest: Codable, Equatable, Sendable {
    let runId: String
    let sessionId: String
    let projectId: String
    let title: String
    let goal: String
    let provider: FlitProviderKind
    let permissionMode: FlitManagedRunPermissionMode
    let permissionModeVersion: UInt64
    let createdAt: String
    let startedAt: String
    let runCreatedEventId: String
    let startRequestedEventId: String
    let sessionConnectedEventId: String
    let startFailedEventId: String
    let startUnknownEventId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case sessionId = "session_id"
        case projectId = "project_id"
        case title
        case goal
        case provider
        case permissionMode = "permission_mode"
        case permissionModeVersion = "permission_mode_version"
        case createdAt = "created_at"
        case startedAt = "started_at"
        case runCreatedEventId = "run_created_event_id"
        case startRequestedEventId = "start_requested_event_id"
        case sessionConnectedEventId = "session_connected_event_id"
        case startFailedEventId = "start_failed_event_id"
        case startUnknownEventId = "start_unknown_event_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitManagedRunStartResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let runId: String
    let sessionId: String
    let providerThreadId: String
    let providerTurnId: String
    let permissionMode: FlitManagedRunPermissionMode
    let permissionModeVersion: UInt64
    let providerConfiguration: String

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case sessionId = "session_id"
        case providerThreadId = "provider_thread_id"
        case providerTurnId = "provider_turn_id"
        case permissionMode = "permission_mode"
        case permissionModeVersion = "permission_mode_version"
        case providerConfiguration = "provider_configuration"
    }
}

struct FlitManagedRunObserveRequest: Codable, Equatable, Sendable {
    let runId: String
    let observedAt: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case observedAt = "observed_at"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitManagedRunObservationStatus: String, Codable, Sendable {
    case permissionRequested = "permission_requested"
    case providerOutcomeResolved = "provider_outcome_resolved"
    case turnCompleted = "turn_completed"
    case turnInterrupted = "turn_interrupted"
}

enum FlitManagedRunProviderDecision: String, Codable, Sendable {
    case allowed
    case denied
}

enum FlitManagedRunProviderTerminalOutcome: String, Codable, Sendable {
    case requestResolved = "request_resolved"
}

enum FlitManagedRunObserveResponse: Codable, Equatable, Sendable {
    case permissionRequested(
        protocolVersion: String,
        runId: String,
        sessionId: String,
        providerThreadId: String,
        providerTurnId: String,
        providerItemId: String,
        providerRequestId: UInt64,
        requestId: String,
        requestVersion: UInt64,
        eventId: String
    )
    case providerOutcomeResolved(
        protocolVersion: String,
        runId: String,
        sessionId: String,
        providerThreadId: String,
        providerTurnId: String,
        providerItemId: String,
        providerDecisionId: String,
        requestId: String,
        requestVersion: UInt64,
        requestEventId: String,
        providerDecision: FlitManagedRunProviderDecision,
        terminalOutcome: FlitManagedRunProviderTerminalOutcome,
        eventId: String,
        eventVersion: UInt64
    )
    case turnCompleted(
        protocolVersion: String,
        runId: String,
        sessionId: String,
        providerThreadId: String,
        providerTurnId: String,
        eventId: String,
        eventVersion: UInt64
    )
    case turnInterrupted(
        protocolVersion: String,
        runId: String,
        sessionId: String,
        providerThreadId: String,
        providerTurnId: String,
        eventId: String,
        eventVersion: UInt64
    )

    var status: FlitManagedRunObservationStatus {
        switch self {
        case .permissionRequested:
            .permissionRequested
        case .providerOutcomeResolved:
            .providerOutcomeResolved
        case .turnCompleted:
            .turnCompleted
        case .turnInterrupted:
            .turnInterrupted
        }
    }

    private enum CodingKeys: String, CodingKey {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case sessionId = "session_id"
        case providerThreadId = "provider_thread_id"
        case providerTurnId = "provider_turn_id"
        case providerItemId = "provider_item_id"
        case providerRequestId = "provider_request_id"
        case providerDecisionId = "provider_decision_id"
        case requestId = "request_id"
        case requestVersion = "request_version"
        case requestEventId = "request_event_id"
        case providerDecision = "provider_decision"
        case terminalOutcome = "terminal_outcome"
        case eventId = "event_id"
        case eventVersion = "event_version"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let status = try values.decode(FlitManagedRunObservationStatus.self, forKey: .status)
        let protocolVersion = try values.decode(String.self, forKey: .protocolVersion)
        let runId = try values.decode(String.self, forKey: .runId)
        let sessionId = try values.decode(String.self, forKey: .sessionId)
        let providerThreadId = try values.decode(String.self, forKey: .providerThreadId)
        let providerTurnId = try values.decode(String.self, forKey: .providerTurnId)
        let eventId = try values.decode(String.self, forKey: .eventId)
        switch status {
        case .permissionRequested:
            for key in [
                CodingKeys.providerDecisionId,
                .requestEventId,
                .providerDecision,
                .terminalOutcome,
                .eventVersion,
            ] where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "permission observation must not contain outcome fields"
                )
            }
            self = .permissionRequested(
                protocolVersion: protocolVersion,
                runId: runId,
                sessionId: sessionId,
                providerThreadId: providerThreadId,
                providerTurnId: providerTurnId,
                providerItemId: try values.decode(String.self, forKey: .providerItemId),
                providerRequestId: try values.decode(UInt64.self, forKey: .providerRequestId),
                requestId: try values.decode(String.self, forKey: .requestId),
                requestVersion: try values.decode(UInt64.self, forKey: .requestVersion),
                eventId: eventId
            )
        case .providerOutcomeResolved:
            guard !values.contains(.providerRequestId) else {
                throw DecodingError.dataCorruptedError(
                    forKey: .providerRequestId,
                    in: values,
                    debugDescription: "provider outcome must not contain a client request identity"
                )
            }
            self = .providerOutcomeResolved(
                protocolVersion: protocolVersion,
                runId: runId,
                sessionId: sessionId,
                providerThreadId: providerThreadId,
                providerTurnId: providerTurnId,
                providerItemId: try values.decode(String.self, forKey: .providerItemId),
                providerDecisionId: try values.decode(String.self, forKey: .providerDecisionId),
                requestId: try values.decode(String.self, forKey: .requestId),
                requestVersion: try values.decode(UInt64.self, forKey: .requestVersion),
                requestEventId: try values.decode(String.self, forKey: .requestEventId),
                providerDecision: try values.decode(
                    FlitManagedRunProviderDecision.self,
                    forKey: .providerDecision
                ),
                terminalOutcome: try values.decode(
                    FlitManagedRunProviderTerminalOutcome.self,
                    forKey: .terminalOutcome
                ),
                eventId: eventId,
                eventVersion: try values.decode(UInt64.self, forKey: .eventVersion)
            )
        case .turnCompleted, .turnInterrupted:
            for key in [
                CodingKeys.providerItemId,
                .providerRequestId,
                .providerDecisionId,
                .requestId,
                .requestVersion,
                .requestEventId,
                .providerDecision,
                .terminalOutcome,
            ] where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "terminal observation must not contain permission fields"
                )
            }
            let eventVersion = try values.decode(UInt64.self, forKey: .eventVersion)
            if status == .turnCompleted {
                self = .turnCompleted(
                    protocolVersion: protocolVersion,
                    runId: runId,
                    sessionId: sessionId,
                    providerThreadId: providerThreadId,
                    providerTurnId: providerTurnId,
                    eventId: eventId,
                    eventVersion: eventVersion
                )
            } else {
                self = .turnInterrupted(
                    protocolVersion: protocolVersion,
                    runId: runId,
                    sessionId: sessionId,
                    providerThreadId: providerThreadId,
                    providerTurnId: providerTurnId,
                    eventId: eventId,
                    eventVersion: eventVersion
                )
            }
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(status, forKey: .status)
        switch self {
        case let .permissionRequested(
            protocolVersion,
            runId,
            sessionId,
            providerThreadId,
            providerTurnId,
            providerItemId,
            providerRequestId,
            requestId,
            requestVersion,
            eventId
        ):
            try values.encode(protocolVersion, forKey: .protocolVersion)
            try values.encode(runId, forKey: .runId)
            try values.encode(sessionId, forKey: .sessionId)
            try values.encode(providerThreadId, forKey: .providerThreadId)
            try values.encode(providerTurnId, forKey: .providerTurnId)
            try values.encode(providerItemId, forKey: .providerItemId)
            try values.encode(providerRequestId, forKey: .providerRequestId)
            try values.encode(requestId, forKey: .requestId)
            try values.encode(requestVersion, forKey: .requestVersion)
            try values.encode(eventId, forKey: .eventId)
        case let .providerOutcomeResolved(
            protocolVersion,
            runId,
            sessionId,
            providerThreadId,
            providerTurnId,
            providerItemId,
            providerDecisionId,
            requestId,
            requestVersion,
            requestEventId,
            providerDecision,
            terminalOutcome,
            eventId,
            eventVersion
        ):
            try values.encode(protocolVersion, forKey: .protocolVersion)
            try values.encode(runId, forKey: .runId)
            try values.encode(sessionId, forKey: .sessionId)
            try values.encode(providerThreadId, forKey: .providerThreadId)
            try values.encode(providerTurnId, forKey: .providerTurnId)
            try values.encode(providerItemId, forKey: .providerItemId)
            try values.encode(providerDecisionId, forKey: .providerDecisionId)
            try values.encode(requestId, forKey: .requestId)
            try values.encode(requestVersion, forKey: .requestVersion)
            try values.encode(requestEventId, forKey: .requestEventId)
            try values.encode(providerDecision, forKey: .providerDecision)
            try values.encode(terminalOutcome, forKey: .terminalOutcome)
            try values.encode(eventId, forKey: .eventId)
            try values.encode(eventVersion, forKey: .eventVersion)
        case let .turnCompleted(
            protocolVersion,
            runId,
            sessionId,
            providerThreadId,
            providerTurnId,
            eventId,
            eventVersion
        ),
        let .turnInterrupted(
            protocolVersion,
            runId,
            sessionId,
            providerThreadId,
            providerTurnId,
            eventId,
            eventVersion
        ):
            try values.encode(protocolVersion, forKey: .protocolVersion)
            try values.encode(runId, forKey: .runId)
            try values.encode(sessionId, forKey: .sessionId)
            try values.encode(providerThreadId, forKey: .providerThreadId)
            try values.encode(providerTurnId, forKey: .providerTurnId)
            try values.encode(eventId, forKey: .eventId)
            try values.encode(eventVersion, forKey: .eventVersion)
        }
    }
}

enum FlitManagedRunPermissionDecision: String, Codable, Sendable {
    case allowOnce = "allow_once"
    case deny
}

struct FlitManagedRunPermissionRespondRequest: Codable, Equatable, Sendable {
    let runId: String
    let requestId: String
    let requestVersion: UInt64
    let responseAttemptId: String
    let decision: FlitManagedRunPermissionDecision
    let submittedAt: String
    let finishedAt: String
    let submittedEventId: String
    let resolvedEventId: String
    let deliveryUnknownEventId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case requestId = "request_id"
        case requestVersion = "request_version"
        case responseAttemptId = "response_attempt_id"
        case decision
        case submittedAt = "submitted_at"
        case finishedAt = "finished_at"
        case submittedEventId = "submitted_event_id"
        case resolvedEventId = "resolved_event_id"
        case deliveryUnknownEventId = "delivery_unknown_event_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitManagedRunPermissionResponseStatus: String, Codable, Sendable {
    case delivered
    case deliveryUnknown = "delivery_unknown"
}

enum FlitManagedRunPermissionRespondResponse: Codable, Equatable, Sendable {
    case delivered(
        protocolVersion: String,
        runId: String,
        requestId: String,
        requestVersion: UInt64,
        responseAttemptId: String,
        decision: FlitManagedRunPermissionDecision,
        submittedEventId: String,
        submittedVersion: UInt64,
        outcomeEventId: String,
        outcomeVersion: UInt64
    )
    case deliveryUnknown(
        protocolVersion: String,
        runId: String,
        requestId: String,
        requestVersion: UInt64,
        responseAttemptId: String,
        decision: FlitManagedRunPermissionDecision,
        submittedEventId: String,
        submittedVersion: UInt64,
        outcomeEventId: String,
        outcomeVersion: UInt64
    )

    var status: FlitManagedRunPermissionResponseStatus {
        switch self {
        case .delivered:
            .delivered
        case .deliveryUnknown:
            .deliveryUnknown
        }
    }

    private enum CodingKeys: String, CodingKey {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case requestId = "request_id"
        case requestVersion = "request_version"
        case responseAttemptId = "response_attempt_id"
        case decision
        case submittedEventId = "submitted_event_id"
        case submittedVersion = "submitted_version"
        case outcomeEventId = "outcome_event_id"
        case outcomeVersion = "outcome_version"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        let status = try values.decode(
            FlitManagedRunPermissionResponseStatus.self,
            forKey: .status
        )
        let common = (
            try values.decode(String.self, forKey: .protocolVersion),
            try values.decode(String.self, forKey: .runId),
            try values.decode(String.self, forKey: .requestId),
            try values.decode(UInt64.self, forKey: .requestVersion),
            try values.decode(String.self, forKey: .responseAttemptId),
            try values.decode(FlitManagedRunPermissionDecision.self, forKey: .decision),
            try values.decode(String.self, forKey: .submittedEventId),
            try values.decode(UInt64.self, forKey: .submittedVersion),
            try values.decode(String.self, forKey: .outcomeEventId),
            try values.decode(UInt64.self, forKey: .outcomeVersion)
        )
        if status == .delivered {
            self = .delivered(
                protocolVersion: common.0,
                runId: common.1,
                requestId: common.2,
                requestVersion: common.3,
                responseAttemptId: common.4,
                decision: common.5,
                submittedEventId: common.6,
                submittedVersion: common.7,
                outcomeEventId: common.8,
                outcomeVersion: common.9
            )
        } else {
            self = .deliveryUnknown(
                protocolVersion: common.0,
                runId: common.1,
                requestId: common.2,
                requestVersion: common.3,
                responseAttemptId: common.4,
                decision: common.5,
                submittedEventId: common.6,
                submittedVersion: common.7,
                outcomeEventId: common.8,
                outcomeVersion: common.9
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(status, forKey: .status)
        switch self {
        case let .delivered(
            protocolVersion,
            runId,
            requestId,
            requestVersion,
            responseAttemptId,
            decision,
            submittedEventId,
            submittedVersion,
            outcomeEventId,
            outcomeVersion
        ),
        let .deliveryUnknown(
            protocolVersion,
            runId,
            requestId,
            requestVersion,
            responseAttemptId,
            decision,
            submittedEventId,
            submittedVersion,
            outcomeEventId,
            outcomeVersion
        ):
            try values.encode(protocolVersion, forKey: .protocolVersion)
            try values.encode(runId, forKey: .runId)
            try values.encode(requestId, forKey: .requestId)
            try values.encode(requestVersion, forKey: .requestVersion)
            try values.encode(responseAttemptId, forKey: .responseAttemptId)
            try values.encode(decision, forKey: .decision)
            try values.encode(submittedEventId, forKey: .submittedEventId)
            try values.encode(submittedVersion, forKey: .submittedVersion)
            try values.encode(outcomeEventId, forKey: .outcomeEventId)
            try values.encode(outcomeVersion, forKey: .outcomeVersion)
        }
    }
}
"#;

#[must_use]
pub fn generated_event_schema() -> String {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<EventEnvelope>();
    let mut value = serde_json::to_value(schema).expect("generated event schema should serialize");
    value
        .as_object_mut()
        .expect("generated event schema should be an object")
        .insert("$id".to_owned(), Value::String(event_schema_id()));

    let mut rendered =
        serde_json::to_string_pretty(&value).expect("generated event schema should render");
    rendered.push('\n');
    rendered
}
