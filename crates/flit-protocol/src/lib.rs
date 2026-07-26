use std::collections::BTreeMap;

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: &str = "1.2";
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
pub struct ProviderDiagnosticsRequest {
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
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandErrorCode {
    ProtocolMismatch,
    InvalidProjectRequest,
    ProjectInspectionFailure,
    ProjectConflict,
    ProjectNotFound,
    ProjectIdentityMismatch,
    StorageUnavailable,
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
            CommandErrorCode::ProjectInspectionFailure => "errors.projectInspectionFailure",
            CommandErrorCode::ProjectConflict => "errors.projectConflict",
            CommandErrorCode::ProjectNotFound => "errors.projectNotFound",
            CommandErrorCode::ProjectIdentityMismatch => "errors.projectIdentityMismatch",
            CommandErrorCode::StorageUnavailable => "errors.storageUnavailable",
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
    SWIFT_COMMAND_CONTRACT_TEMPLATE.replace("__PROTOCOL_VERSION__", PROTOCOL_VERSION)
}

const SWIFT_COMMAND_CONTRACT_TEMPLATE: &str = r#"// Generated from flit_protocol command contracts. Do not edit.
let flitClientProtocolVersion = "__PROTOCOL_VERSION__"

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

enum FlitCommandErrorCode: String, Codable, Sendable {
    case protocolMismatch = "PROTOCOL_MISMATCH"
    case invalidProjectRequest = "INVALID_PROJECT_REQUEST"
    case projectInspectionFailure = "PROJECT_INSPECTION_FAILURE"
    case projectConflict = "PROJECT_CONFLICT"
    case projectNotFound = "PROJECT_NOT_FOUND"
    case projectIdentityMismatch = "PROJECT_IDENTITY_MISMATCH"
    case storageUnavailable = "STORAGE_UNAVAILABLE"
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
    case permissionPolicyConfigure = "permission_policy_configure"
    case permissionPolicyObserve = "permission_policy_observe"
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
