use std::collections::BTreeMap;

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: &str = "1.27";
pub const EVENT_PROTOCOL_VERSION: &str = "1.4";
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationKindsRecord {
    pub permission: bool,
    pub question: bool,
    pub failure: bool,
    pub completion: bool,
    pub stuck: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHoursRecord {
    pub enabled: bool,
    pub start_minute: u16,
    pub end_minute: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalNotificationPolicyRecord {
    pub version: u64,
    pub kinds: NotificationKindsRecord,
    pub quiet_hours: QuietHoursRecord,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationOverrideRecord {
    Inherit,
    On,
    Off,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectNotificationMasterRecord {
    Inherit,
    Off,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationKindOverridesRecord {
    pub permission: NotificationOverrideRecord,
    pub question: NotificationOverrideRecord,
    pub failure: NotificationOverrideRecord,
    pub completion: NotificationOverrideRecord,
    pub stuck: NotificationOverrideRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectNotificationPolicyRecord {
    pub version: u64,
    pub master: ProjectNotificationMasterRecord,
    pub kinds: NotificationKindOverridesRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveNotificationPolicyRecord {
    pub global_version: u64,
    pub project_version: Option<u64>,
    pub kinds: NotificationKindsRecord,
    pub quiet_hours: QuietHoursRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPolicyResponse {
    pub protocol_version: String,
    pub global: GlobalNotificationPolicyRecord,
    pub project: Option<ProjectNotificationPolicyRecord>,
    pub effective: EffectiveNotificationPolicyRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPolicyReadRequest {
    pub project_id: Option<String>,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalNotificationPolicyUpdateRequest {
    pub expected_version: u64,
    pub kinds: NotificationKindsRecord,
    pub quiet_hours: QuietHoursRecord,
    pub updated_at: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectNotificationPolicyUpdateRequest {
    pub project_id: String,
    pub expected_version: u64,
    pub master: ProjectNotificationMasterRecord,
    pub kinds: NotificationKindOverridesRecord,
    pub updated_at: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitObservationRequest {
    pub project_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitNotWorktreeReason {
    NotRepository,
    BareRepository,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitObservationUnavailableReason {
    RunnerUnavailable,
    GitUnavailable,
    ProjectChanged,
    ProcessUnavailable,
    MalformedOutput,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitHead {
    Available { oid: String },
    Unborn,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitAvailableHeadTag {
    Available,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitUnbornHeadTag {
    Unborn,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitAvailableHeadWire {
    availability: GitAvailableHeadTag,
    oid: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitUnbornHeadWire {
    availability: GitUnbornHeadTag,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GitHeadWire {
    Available(GitAvailableHeadWire),
    Unborn(GitUnbornHeadWire),
}

impl<'de> Deserialize<'de> for GitHead {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match GitHeadWire::deserialize(deserializer)? {
            GitHeadWire::Available(wire) => {
                let _ = wire.availability;
                Self::Available { oid: wire.oid }
            }
            GitHeadWire::Unborn(wire) => {
                let _ = wire.availability;
                Self::Unborn
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitDirtySummary {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub entries: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitBaselineUnavailableReason {
    NotRepository,
    BareRepository,
    RunnerUnavailable,
    GitUnavailable,
    ProcessUnavailable,
    MalformedOutput,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitBaselinePayload {
    Available {
        project_id: String,
        head: GitHead,
        dirty: GitDirtySummary,
    },
    Unavailable {
        project_id: String,
        reason: GitBaselineUnavailableReason,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitBaselineAvailableTag {
    Available,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitBaselineUnavailableTag {
    Unavailable,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitBaselineAvailableWire {
    availability: GitBaselineAvailableTag,
    project_id: String,
    head: GitHead,
    dirty: GitDirtySummary,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitBaselineUnavailableWire {
    availability: GitBaselineUnavailableTag,
    project_id: String,
    reason: GitBaselineUnavailableReason,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GitBaselinePayloadWire {
    Available(GitBaselineAvailableWire),
    Unavailable(GitBaselineUnavailableWire),
}

impl<'de> Deserialize<'de> for GitBaselinePayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match GitBaselinePayloadWire::deserialize(deserializer)? {
            GitBaselinePayloadWire::Available(wire) => {
                let _ = wire.availability;
                Self::Available {
                    project_id: wire.project_id,
                    head: wire.head,
                    dirty: wire.dirty,
                }
            }
            GitBaselinePayloadWire::Unavailable(wire) => {
                let _ = wire.availability;
                Self::Unavailable {
                    project_id: wire.project_id,
                    reason: wire.reason,
                }
            }
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "observation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitObservationResponse {
    NotWorktree {
        protocol_version: String,
        project_id: String,
        reason: GitNotWorktreeReason,
    },
    Repository {
        protocol_version: String,
        project_id: String,
        canonical_root: String,
        head: GitHead,
        dirty: GitDirtySummary,
    },
    Unavailable {
        protocol_version: String,
        project_id: String,
        reason: GitObservationUnavailableReason,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitNotWorktreeTag {
    NotWorktree,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitRepositoryTag {
    Repository,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GitUnavailableTag {
    Unavailable,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitNotWorktreeResponseWire {
    observation: GitNotWorktreeTag,
    protocol_version: String,
    project_id: String,
    reason: GitNotWorktreeReason,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitRepositoryResponseWire {
    observation: GitRepositoryTag,
    protocol_version: String,
    project_id: String,
    canonical_root: String,
    head: GitHead,
    dirty: GitDirtySummary,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitUnavailableResponseWire {
    observation: GitUnavailableTag,
    protocol_version: String,
    project_id: String,
    reason: GitObservationUnavailableReason,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GitObservationResponseWire {
    NotWorktree(GitNotWorktreeResponseWire),
    Repository(GitRepositoryResponseWire),
    Unavailable(GitUnavailableResponseWire),
}

impl<'de> Deserialize<'de> for GitObservationResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            match GitObservationResponseWire::deserialize(deserializer)? {
                GitObservationResponseWire::NotWorktree(wire) => {
                    let _ = wire.observation;
                    Self::NotWorktree {
                        protocol_version: wire.protocol_version,
                        project_id: wire.project_id,
                        reason: wire.reason,
                    }
                }
                GitObservationResponseWire::Repository(wire) => {
                    let _ = wire.observation;
                    Self::Repository {
                        protocol_version: wire.protocol_version,
                        project_id: wire.project_id,
                        canonical_root: wire.canonical_root,
                        head: wire.head,
                        dirty: wire.dirty,
                    }
                }
                GitObservationResponseWire::Unavailable(wire) => {
                    let _ = wire.observation;
                    Self::Unavailable {
                        protocol_version: wire.protocol_version,
                        project_id: wire.project_id,
                        reason: wire.reason,
                    }
                }
            },
        )
    }
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
    #[serde(deserialize_with = "deserialize_active_stuck_occurrence_id")]
    pub active_stuck_occurrence_id: Option<String>,
    pub last_progress_at: Option<String>,
    pub last_liveness_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub changes: DashboardChangeSummary,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardChangeAttribution {
    Exact,
    ObservedDuringRun,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum DashboardChangeSummary {
    Available {
        attribution: DashboardChangeAttribution,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEvidenceCategory {
    Activity,
    Command,
    File,
    Test,
    Attention,
    Lifecycle,
    Unknown,
}

impl RunEvidenceCategory {
    #[must_use]
    pub fn for_event_type(event_type: &str) -> Self {
        match event_type {
            "activity.classified" => Self::Activity,
            "command.started" | "command.finished" => Self::Command,
            "file.changed" | "git.snapshot_recorded" => Self::File,
            "permission.requested"
            | "permission.mode_change_submitted"
            | "permission.mode_configured"
            | "permission.mode_configuration_failed"
            | "permission.mode_configuration_unknown"
            | "permission.response_submitted"
            | "permission.resolved"
            | "permission.response_failed"
            | "permission.delivery_unknown"
            | "permission.provider_outcome_resolved"
            | "permission.provider_outcome_unknown"
            | "question.requested"
            | "question.response_submitted"
            | "question.resolved"
            | "question.response_failed"
            | "question.delivery_unknown"
            | "risk.detected"
            | "attention.acknowledged" => Self::Attention,
            "run.created"
            | "run.start_requested"
            | "run.resume_requested"
            | "run.resume_failed"
            | "run.completed"
            | "run.failed"
            | "run.stop_requested"
            | "run.stopped"
            | "run.interrupted"
            | "session.connected"
            | "session.resumed" => Self::Lifecycle,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunEvidenceRecord {
    pub cursor: u64,
    pub event_id: String,
    pub session_id: Option<String>,
    pub event_type: String,
    pub category: RunEvidenceCategory,
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
#[serde(deny_unknown_fields)]
pub struct RunActiveAttentionReadRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActiveAttentionCategory {
    Permission,
    PermissionAudit,
    Question,
    Risk,
    Failure,
    Stuck,
    System,
    Completion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActiveAttentionSeverity {
    Informational,
    ActionRequired,
    Critical,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActiveAttentionStatus {
    Open,
    ResponsePending,
    DeliveryUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunActiveAttentionAction {
    Acknowledge,
    PermissionResponse {
        request_id: String,
        request_version: u64,
    },
    StillWorking {
        occurrence_id: String,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunActiveAttentionItem {
    pub attention_id: String,
    pub attention_version: u64,
    pub category: RunActiveAttentionCategory,
    pub severity: RunActiveAttentionSeverity,
    pub blocking: bool,
    pub status: RunActiveAttentionStatus,
    pub source_event_id: String,
    pub source_event_type: String,
    pub source_observed_at: String,
    pub content_unavailable_reason: String,
    pub action: RunActiveAttentionAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RunActiveAttentionSlot {
    Item(RunActiveAttentionItem),
    Null,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunActiveAttentionReadResponse {
    pub protocol_version: String,
    pub event_schema_version: String,
    pub run_id: String,
    pub run_version: u64,
    pub open_count: u64,
    pub item: RunActiveAttentionSlot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionAcknowledgeRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub attention_id: String,
    pub attention_version: u64,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionAcknowledgeRejectedReason {
    RunVersionStale,
    AttentionMismatch,
    NotAcknowledgeable,
    AlreadyApplied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttentionAcknowledgeResponse {
    Applied {
        protocol_version: String,
        run_id: String,
        previous_version: u64,
        event_id: String,
        event_version: u64,
        attention_id: String,
        attention_version: u64,
    },
    Rejected {
        protocol_version: String,
        run_id: String,
        expected_run_version: u64,
        attention_id: String,
        attention_version: u64,
        reason: AttentionAcknowledgeRejectedReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunChangesReadRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub after_cursor: Option<String>,
    pub requested_change_limit: u32,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunChangeHead {
    Available { oid: String },
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFileChangeStatus {
    Added,
    Modified,
    Deleted,
    TypeChanged,
    Untracked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFileProjectScope {
    InsideProject,
    OutsideProject,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunFileChangeRecord {
    pub change_id: String,
    pub display_path: String,
    pub status: RunFileChangeStatus,
    pub committed: bool,
    pub staged: bool,
    pub unstaged: bool,
    pub binary: bool,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
    pub project_scope: RunFileProjectScope,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunChangesUnavailableReason {
    ChangeSetNotAvailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunChangesReadResponse {
    Available {
        protocol_version: String,
        run_id: String,
        run_version: u64,
        attribution: DashboardChangeAttribution,
        baseline_head: RunChangeHead,
        terminal_head: RunChangeHead,
        next_cursor: Option<String>,
        has_more: bool,
        changes: Vec<RunFileChangeRecord>,
    },
    Unavailable {
        protocol_version: String,
        run_id: String,
        run_version: u64,
        reason: RunChangesUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunOpenInProviderRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunChangeExternalOpenRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub change_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunChangeExternalOpenDisabledReason {
    ChangeSetNotAvailable,
    ChangeNotFound,
    DeletedChange,
    OutsideProject,
    ProjectIdentityMismatch,
    RepositoryIdentityMismatch,
    TargetUnavailable,
    SymlinkEscape,
    TargetNotFile,
    TargetIdentityDrift,
    OpenFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunChangeExternalOpenResponse {
    Opened {
        protocol_version: String,
        run_id: String,
        run_version: u64,
        change_id: String,
    },
    Disabled {
        protocol_version: String,
        run_id: String,
        run_version: u64,
        change_id: String,
        reason: RunChangeExternalOpenDisabledReason,
    },
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
    pub git_baseline_observed_at: String,
    pub started_at: String,
    pub run_created_event_id: String,
    pub git_baseline_event_id: String,
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
#[serde(deny_unknown_fields)]
pub struct ManagedRunsAssessStuckRequest {
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRunsAssessStuckResponse {
    pub protocol_version: String,
    pub assessed_runs: u32,
    pub transitions_appended: u32,
    pub unchanged_runs: u32,
    pub unavailable_runs: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationsDueReadRequest {
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryKind {
    Permission,
    Question,
    Failure,
    Completion,
    Stuck,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveriesDueReadRequest {
    pub local_minute: u16,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveryRecord {
    pub notification_id: String,
    pub run_id: String,
    pub run_version: u64,
    pub project_id: String,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub delivery_claimed: bool,
    pub catch_up: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveriesDueReadResponse {
    pub protocol_version: String,
    pub notifications: Vec<NotificationDeliveryRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveryClaimRequest {
    pub notification_id: String,
    pub run_id: String,
    pub expected_run_version: u64,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub local_minute: u16,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveryClaimResponse {
    pub protocol_version: String,
    pub notification_id: String,
    pub run_id: String,
    pub run_version: u64,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub already_claimed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveryFailedRequest {
    pub notification_id: String,
    pub run_id: String,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveryFailedResponse {
    pub protocol_version: String,
    pub notification_id: String,
    pub run_id: String,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub released: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveredRequest {
    pub notification_id: String,
    pub run_id: String,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDeliveredResponse {
    pub protocol_version: String,
    pub notification_id: String,
    pub run_id: String,
    pub kind: NotificationDeliveryKind,
    pub item_id: String,
    pub item_version: u64,
    pub platform_id: String,
    pub already_delivered: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationDueRecord {
    pub run_id: String,
    pub run_version: u64,
    pub occurrence_id: String,
    pub platform_id: String,
    pub delivery_claimed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationsDueReadResponse {
    pub protocol_version: String,
    pub event_schema_version: String,
    pub notifications: Vec<ManagedStuckNotificationDueRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationDeliveryClaimRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub occurrence_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationDeliveryClaimResponse {
    pub protocol_version: String,
    pub run_id: String,
    pub run_version: u64,
    pub occurrence_id: String,
    pub platform_id: String,
    pub already_claimed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationDeliveryFailedRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub occurrence_id: String,
    pub platform_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationDeliveryFailedResponse {
    pub protocol_version: String,
    pub run_id: String,
    pub run_version: u64,
    pub occurrence_id: String,
    pub platform_id: String,
    pub released: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedStuckNotificationDeliveredRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub occurrence_id: String,
    pub platform_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedStuckNotificationDeliveredRejectedReason {
    RunVersionStale,
    OccurrenceMismatch,
    NotDue,
    AlreadyDelivered,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedStuckNotificationDeliveredResponse {
    Delivered {
        protocol_version: String,
        run_id: String,
        previous_version: u64,
        event_id: String,
        event_version: u64,
        occurrence_id: String,
        platform_id: String,
    },
    Rejected {
        protocol_version: String,
        run_id: String,
        expected_run_version: u64,
        occurrence_id: String,
        platform_id: String,
        reason: ManagedStuckNotificationDeliveredRejectedReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedRunStillWorkingRequest {
    pub run_id: String,
    pub expected_run_version: u64,
    pub occurrence_id: String,
    pub client_protocol_version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedRunStillWorkingRejectedReason {
    RunVersionStale,
    OccurrenceMismatch,
    NotCurrentlyStuck,
    ProcessUnavailable,
    AlreadyApplied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedRunStillWorkingResponse {
    Applied {
        protocol_version: String,
        run_id: String,
        previous_version: u64,
        event_id: String,
        event_version: u64,
        occurrence_id: String,
    },
    Rejected {
        protocol_version: String,
        run_id: String,
        expected_run_version: u64,
        occurrence_id: String,
        reason: ManagedRunStillWorkingRejectedReason,
    },
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
    InvalidNotificationPolicy,
    NotificationPolicyVersionStale,
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
    #[serde(rename = "1.1")]
    V1_1,
    #[serde(rename = "1.2")]
    V1_2,
    #[serde(rename = "1.3")]
    V1_3,
    #[serde(rename = "1.4")]
    V1_4,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuckCauseCode {
    Starting,
    Planning,
    Reading,
    Editing,
    Testing,
    Building,
    Reviewing,
    Waiting,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum StuckProcessReceipt {
    NotSpawned {
        #[serde(deserialize_with = "deserialize_json_safe_u64")]
        observed_monotonic_ms: u64,
    },
    Alive {
        generation: String,
        #[serde(deserialize_with = "deserialize_json_safe_u64")]
        observed_monotonic_ms: u64,
    },
    Unavailable {
        generation: Option<String>,
        reason: String,
        #[serde(deserialize_with = "deserialize_json_safe_u64")]
        observed_monotonic_ms: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PossiblyStuckPayload {
    pub occurrence_id: String,
    pub cause: StuckCauseCode,
    #[serde(deserialize_with = "deserialize_stuck_threshold")]
    pub threshold_seconds: u16,
    pub progress_event_id: String,
    pub progress_observed_at: String,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub progress_monotonic_ms: u64,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub baseline_monotonic_ms: u64,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub stuck_since_monotonic_ms: u64,
    pub process: StuckProcessReceipt,
    pub evidence_unavailable_reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StuckClearReasonCode {
    LifecycleInactive,
    BlockingRequestOpen,
    StructuredWait,
    ProgressObserved,
    ProcessUnavailable,
    WithinDeadline,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StuckClearedPayload {
    pub occurrence_id: String,
    pub reason: StuckClearReasonCode,
    pub process: StuckProcessReceipt,
    pub evidence_unavailable_reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StillWorkingPayload {
    pub occurrence_id: String,
    pub progress_event_id: String,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub reset_monotonic_ms: u64,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub notification_suppressed_until_monotonic_ms: u64,
    pub process: StuckProcessReceipt,
    pub evidence_unavailable_reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionAcknowledgedPayload {
    pub attention_id: String,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub attention_version: u64,
    pub source_event_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StuckNotificationDuePayload {
    pub occurrence_id: String,
    #[serde(deserialize_with = "deserialize_json_safe_u64")]
    pub due_at_monotonic_ms: u64,
    pub process: StuckProcessReceipt,
    pub evidence_unavailable_reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StuckNotificationDeliveredPayload {
    pub occurrence_id: String,
    pub platform_id: String,
}

fn deserialize_json_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value <= MAX_JSON_SAFE_INTEGER {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "monotonic milliseconds exceed the JSON safe integer bound",
        ))
    }
}

fn deserialize_active_stuck_occurrence_id<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if value
        .as_deref()
        .is_none_or(|value| !value.trim().is_empty() && value.len() <= 256)
    {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "active stuck occurrence ID must be a non-empty string of at most 256 bytes",
        ))
    }
}

fn deserialize_stuck_threshold<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u16::deserialize(deserializer)?;
    if (30..=1_800).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "stuck threshold must be between 30 and 1800 seconds",
        ))
    }
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
            CommandErrorCode::InvalidNotificationPolicy => "errors.invalidNotificationPolicy",
            CommandErrorCode::NotificationPolicyVersionStale => {
                "errors.notificationPolicyVersionStale"
            }
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

struct FlitNotificationKinds: Codable, Equatable, Sendable {
    let permission: Bool
    let question: Bool
    let failure: Bool
    let completion: Bool
    let stuck: Bool
}

struct FlitQuietHours: Codable, Equatable, Sendable {
    let enabled: Bool
    let startMinute: UInt16
    let endMinute: UInt16

    private enum CodingKeys: String, CodingKey {
        case enabled
        case startMinute = "start_minute"
        case endMinute = "end_minute"
    }
}

struct FlitGlobalNotificationPolicy: Codable, Equatable, Sendable {
    let version: UInt64
    let kinds: FlitNotificationKinds
    let quietHours: FlitQuietHours

    private enum CodingKeys: String, CodingKey {
        case version
        case kinds
        case quietHours = "quiet_hours"
    }
}

enum FlitNotificationOverride: String, Codable, Sendable {
    case inherit
    case on
    case off
}

enum FlitProjectNotificationMaster: String, Codable, Sendable {
    case inherit
    case off
}

struct FlitNotificationKindOverrides: Codable, Equatable, Sendable {
    let permission: FlitNotificationOverride
    let question: FlitNotificationOverride
    let failure: FlitNotificationOverride
    let completion: FlitNotificationOverride
    let stuck: FlitNotificationOverride
}

struct FlitProjectNotificationPolicy: Codable, Equatable, Sendable {
    let version: UInt64
    let master: FlitProjectNotificationMaster
    let kinds: FlitNotificationKindOverrides
}

struct FlitEffectiveNotificationPolicy: Codable, Equatable, Sendable {
    let globalVersion: UInt64
    let projectVersion: UInt64?
    let kinds: FlitNotificationKinds
    let quietHours: FlitQuietHours

    private enum CodingKeys: String, CodingKey {
        case globalVersion = "global_version"
        case projectVersion = "project_version"
        case kinds
        case quietHours = "quiet_hours"
    }
}

struct FlitNotificationPolicyResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let global: FlitGlobalNotificationPolicy
    let project: FlitProjectNotificationPolicy?
    let effective: FlitEffectiveNotificationPolicy

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case global
        case project
        case effective
    }
}

struct FlitNotificationPolicyReadRequest: Codable, Equatable, Sendable {
    let projectId: String?
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case projectId = "project_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitGlobalNotificationPolicyUpdateRequest: Codable, Equatable, Sendable {
    let expectedVersion: UInt64
    let kinds: FlitNotificationKinds
    let quietHours: FlitQuietHours
    let updatedAt: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case expectedVersion = "expected_version"
        case kinds
        case quietHours = "quiet_hours"
        case updatedAt = "updated_at"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitProjectNotificationPolicyUpdateRequest: Codable, Equatable, Sendable {
    let projectId: String
    let expectedVersion: UInt64
    let master: FlitProjectNotificationMaster
    let kinds: FlitNotificationKindOverrides
    let updatedAt: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case projectId = "project_id"
        case expectedVersion = "expected_version"
        case master
        case kinds
        case updatedAt = "updated_at"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitGitObservationRequest: Codable, Equatable, Sendable {
    let projectId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case projectId = "project_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitGitObservationKind: String, Codable, Sendable {
    case notWorktree = "not_worktree"
    case repository
    case unavailable
}

enum FlitGitNotWorktreeReason: String, Codable, Sendable {
    case notRepository = "not_repository"
    case bareRepository = "bare_repository"
}

enum FlitGitObservationUnavailableReason: String, Codable, Sendable {
    case runnerUnavailable = "runner_unavailable"
    case gitUnavailable = "git_unavailable"
    case projectChanged = "project_changed"
    case processUnavailable = "process_unavailable"
    case malformedOutput = "malformed_output"
}

enum FlitGitHeadAvailability: String, Codable, Sendable {
    case available
    case unborn
}

enum FlitGitHead: Codable, Equatable, Sendable {
    case available(oid: String)
    case unborn

    private enum CodingKeys: String, CodingKey {
        case availability
        case oid
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(FlitGitHeadAvailability.self, forKey: .availability) {
        case .available:
            self = .available(oid: try container.decode(String.self, forKey: .oid))
        case .unborn:
            guard !container.contains(.oid) else {
                throw DecodingError.dataCorruptedError(
                    forKey: .oid,
                    in: container,
                    debugDescription: "An unborn Git HEAD cannot include an object ID"
                )
            }
            self = .unborn
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .available(oid):
            try container.encode(FlitGitHeadAvailability.available, forKey: .availability)
            try container.encode(oid, forKey: .oid)
        case .unborn:
            try container.encode(FlitGitHeadAvailability.unborn, forKey: .availability)
        }
    }
}

struct FlitGitDirtySummary: Codable, Equatable, Sendable {
    let staged: UInt32
    let unstaged: UInt32
    let untracked: UInt32
    let entries: UInt32
}

struct FlitGitNotWorktreeResponse: Codable, Equatable, Sendable {
    let observation: FlitGitObservationKind
    let protocolVersion: String
    let projectId: String
    let reason: FlitGitNotWorktreeReason

    private enum CodingKeys: String, CodingKey {
        case observation
        case protocolVersion = "protocol_version"
        case projectId = "project_id"
        case reason
    }
}

struct FlitGitRepositoryResponse: Codable, Equatable, Sendable {
    let observation: FlitGitObservationKind
    let protocolVersion: String
    let projectId: String
    let canonicalRoot: String
    let head: FlitGitHead
    let dirty: FlitGitDirtySummary

    private enum CodingKeys: String, CodingKey {
        case observation
        case protocolVersion = "protocol_version"
        case projectId = "project_id"
        case canonicalRoot = "canonical_root"
        case head
        case dirty
    }
}

struct FlitGitUnavailableResponse: Codable, Equatable, Sendable {
    let observation: FlitGitObservationKind
    let protocolVersion: String
    let projectId: String
    let reason: FlitGitObservationUnavailableReason

    private enum CodingKeys: String, CodingKey {
        case observation
        case protocolVersion = "protocol_version"
        case projectId = "project_id"
        case reason
    }
}

enum FlitGitObservationResponse: Codable, Equatable, Sendable {
    case notWorktree(FlitGitNotWorktreeResponse)
    case repository(FlitGitRepositoryResponse)
    case unavailable(FlitGitUnavailableResponse)

    private enum CodingKeys: String, CodingKey {
        case observation
        case canonicalRoot = "canonical_root"
        case head
        case dirty
        case reason
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(FlitGitObservationKind.self, forKey: .observation) {
        case .notWorktree:
            guard
                !container.contains(.canonicalRoot),
                !container.contains(.head),
                !container.contains(.dirty)
            else {
                throw DecodingError.dataCorruptedError(
                    forKey: .observation,
                    in: container,
                    debugDescription: "A non-worktree Git observation cannot include repository fields"
                )
            }
            self = .notWorktree(try FlitGitNotWorktreeResponse(from: decoder))
        case .repository:
            guard !container.contains(.reason) else {
                throw DecodingError.dataCorruptedError(
                    forKey: .reason,
                    in: container,
                    debugDescription: "A repository Git observation cannot include an unavailable reason"
                )
            }
            self = .repository(try FlitGitRepositoryResponse(from: decoder))
        case .unavailable:
            guard
                !container.contains(.canonicalRoot),
                !container.contains(.head),
                !container.contains(.dirty)
            else {
                throw DecodingError.dataCorruptedError(
                    forKey: .observation,
                    in: container,
                    debugDescription: "An unavailable Git observation cannot include repository fields"
                )
            }
            self = .unavailable(try FlitGitUnavailableResponse(from: decoder))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .notWorktree(response):
            try response.encode(to: encoder)
        case let .repository(response):
            try response.encode(to: encoder)
        case let .unavailable(response):
            try response.encode(to: encoder)
        }
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
    let activeStuckOccurrenceId: String?
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
        case activeStuckOccurrenceId = "active_stuck_occurrence_id"
        case lastProgressAt = "last_progress_at"
        case lastLivenessAt = "last_liveness_at"
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case changes
        case updatedAt = "updated_at"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        guard values.contains(.activeStuckOccurrenceId) else {
            throw DecodingError.keyNotFound(
                CodingKeys.activeStuckOccurrenceId,
                DecodingError.Context(
                    codingPath: values.codingPath,
                    debugDescription: "Dashboard Run requires active_stuck_occurrence_id"
                )
            )
        }
        runId = try values.decode(String.self, forKey: .runId)
        projectId = try values.decode(String.self, forKey: .projectId)
        projectDisplayName = try values.decode(String.self, forKey: .projectDisplayName)
        title = try values.decode(String.self, forKey: .title)
        provider = try values.decode(FlitProviderKind.self, forKey: .provider)
        version = try values.decode(UInt64.self, forKey: .version)
        lifecycle = try values.decode(String.self, forKey: .lifecycle)
        activity = try values.decode(String.self, forKey: .activity)
        activityConfidence = try values.decode(Double.self, forKey: .activityConfidence)
        attentionLevel = try values.decode(String.self, forKey: .attentionLevel)
        attentionOpenCount = try values.decode(UInt64.self, forKey: .attentionOpenCount)
        dashboardBucket = try values.decode(String.self, forKey: .dashboardBucket)
        activeStuckOccurrenceId = try values.decodeIfPresent(
            String.self,
            forKey: .activeStuckOccurrenceId
        )
        if let activeStuckOccurrenceId,
           activeStuckOccurrenceId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || activeStuckOccurrenceId.utf8.count > 256
        {
            throw DecodingError.dataCorruptedError(
                forKey: .activeStuckOccurrenceId,
                in: values,
                debugDescription: "active stuck occurrence ID must be non-empty and bounded"
            )
        }
        lastProgressAt = try values.decodeIfPresent(String.self, forKey: .lastProgressAt)
        lastLivenessAt = try values.decodeIfPresent(String.self, forKey: .lastLivenessAt)
        startedAt = try values.decodeIfPresent(String.self, forKey: .startedAt)
        endedAt = try values.decodeIfPresent(String.self, forKey: .endedAt)
        changes = try values.decode(FlitDashboardChangeSummary.self, forKey: .changes)
        updatedAt = try values.decode(String.self, forKey: .updatedAt)
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(runId, forKey: .runId)
        try values.encode(projectId, forKey: .projectId)
        try values.encode(projectDisplayName, forKey: .projectDisplayName)
        try values.encode(title, forKey: .title)
        try values.encode(provider, forKey: .provider)
        try values.encode(version, forKey: .version)
        try values.encode(lifecycle, forKey: .lifecycle)
        try values.encode(activity, forKey: .activity)
        try values.encode(activityConfidence, forKey: .activityConfidence)
        try values.encode(attentionLevel, forKey: .attentionLevel)
        try values.encode(attentionOpenCount, forKey: .attentionOpenCount)
        try values.encode(dashboardBucket, forKey: .dashboardBucket)
        try values.encode(activeStuckOccurrenceId, forKey: .activeStuckOccurrenceId)
        try values.encode(lastProgressAt, forKey: .lastProgressAt)
        try values.encode(lastLivenessAt, forKey: .lastLivenessAt)
        try values.encode(startedAt, forKey: .startedAt)
        try values.encode(endedAt, forKey: .endedAt)
        try values.encode(changes, forKey: .changes)
        try values.encode(updatedAt, forKey: .updatedAt)
    }
}

enum FlitDashboardChangeAvailability: String, Codable, Sendable {
    case available
    case unavailable
}

enum FlitDashboardChangeAttribution: String, Codable, Sendable {
    case exact
    case observedDuringRun = "observed_during_run"
}

enum FlitDashboardChangeSummary: Codable, Equatable, Sendable {
    case available(
        attribution: FlitDashboardChangeAttribution,
        files: UInt64,
        insertions: UInt64,
        deletions: UInt64
    )
    case unavailable(reason: String)

    private enum CodingKeys: String, CodingKey {
        case availability
        case attribution
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
                attribution: try container.decode(
                    FlitDashboardChangeAttribution.self,
                    forKey: .attribution
                ),
                files: try container.decode(UInt64.self, forKey: .files),
                insertions: try container.decode(UInt64.self, forKey: .insertions),
                deletions: try container.decode(UInt64.self, forKey: .deletions)
            )
        case .unavailable:
            guard
                !container.contains(.files),
                !container.contains(.insertions),
                !container.contains(.deletions),
                !container.contains(.attribution)
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
        case let .available(attribution, files, insertions, deletions):
            try container.encode(
                FlitDashboardChangeAvailability.available,
                forKey: .availability
            )
            try container.encode(attribution, forKey: .attribution)
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

enum FlitRunEvidenceCategory: String, Codable, Sendable {
    case activity
    case command
    case file
    case test
    case attention
    case lifecycle
    case unknown
}

struct FlitRunEvidenceRecord: Codable, Equatable, Sendable {
    let cursor: UInt64
    let eventId: String
    let sessionId: String?
    let eventType: String
    let category: FlitRunEvidenceCategory
    let sourceKind: FlitEventSourceKind
    let confidence: Double
    let observedAt: String

    private enum CodingKeys: String, CodingKey {
        case cursor
        case eventId = "event_id"
        case sessionId = "session_id"
        case eventType = "event_type"
        case category
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

struct FlitRunActiveAttentionReadRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitRunActiveAttentionCategory: String, Codable, Sendable {
    case permission
    case permissionAudit = "permission_audit"
    case question
    case risk
    case failure
    case stuck
    case system
    case completion
}

enum FlitRunActiveAttentionSeverity: String, Codable, Sendable {
    case informational
    case actionRequired = "action_required"
    case critical
}

enum FlitRunActiveAttentionStatus: String, Codable, Sendable {
    case open
    case responsePending = "response_pending"
    case deliveryUnknown = "delivery_unknown"
}

enum FlitRunActiveAttentionAction: Codable, Equatable, Sendable {
    case acknowledge
    case permissionResponse(requestId: String, requestVersion: UInt64)
    case stillWorking(occurrenceId: String)
    case unavailable(reason: String)

    private enum CodingKeys: String, CodingKey {
        case kind
        case requestId = "request_id"
        case requestVersion = "request_version"
        case occurrenceId = "occurrence_id"
        case reason
    }

    private enum Kind: String, Codable {
        case permissionResponse = "permission_response"
        case acknowledge
        case stillWorking = "still_working"
        case unavailable
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(Kind.self, forKey: .kind) {
        case .acknowledge:
            self = .acknowledge
        case .permissionResponse:
            self = .permissionResponse(
                requestId: try container.decode(String.self, forKey: .requestId),
                requestVersion: try container.decode(UInt64.self, forKey: .requestVersion)
            )
        case .stillWorking:
            self = .stillWorking(
                occurrenceId: try container.decode(String.self, forKey: .occurrenceId)
            )
        case .unavailable:
            self = .unavailable(reason: try container.decode(String.self, forKey: .reason))
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .acknowledge:
            try container.encode(Kind.acknowledge, forKey: .kind)
        case let .permissionResponse(requestId, requestVersion):
            try container.encode(Kind.permissionResponse, forKey: .kind)
            try container.encode(requestId, forKey: .requestId)
            try container.encode(requestVersion, forKey: .requestVersion)
        case let .stillWorking(occurrenceId):
            try container.encode(Kind.stillWorking, forKey: .kind)
            try container.encode(occurrenceId, forKey: .occurrenceId)
        case let .unavailable(reason):
            try container.encode(Kind.unavailable, forKey: .kind)
            try container.encode(reason, forKey: .reason)
        }
    }
}

struct FlitRunActiveAttentionItem: Codable, Equatable, Sendable {
    let attentionId: String
    let attentionVersion: UInt64
    let category: FlitRunActiveAttentionCategory
    let severity: FlitRunActiveAttentionSeverity
    let blocking: Bool
    let status: FlitRunActiveAttentionStatus
    let sourceEventId: String
    let sourceEventType: String
    let sourceObservedAt: String
    let contentUnavailableReason: String
    let action: FlitRunActiveAttentionAction

    private enum CodingKeys: String, CodingKey {
        case attentionId = "attention_id"
        case attentionVersion = "attention_version"
        case category
        case severity
        case blocking
        case status
        case sourceEventId = "source_event_id"
        case sourceEventType = "source_event_type"
        case sourceObservedAt = "source_observed_at"
        case contentUnavailableReason = "content_unavailable_reason"
        case action
    }
}

enum FlitRunActiveAttentionSlot: Codable, Equatable, Sendable {
    case item(FlitRunActiveAttentionItem)
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else {
            self = .item(try container.decode(FlitRunActiveAttentionItem.self))
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case let .item(item):
            try container.encode(item)
        case .null:
            try container.encodeNil()
        }
    }
}

struct FlitRunActiveAttentionReadResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let eventSchemaVersion: String
    let runId: String
    let runVersion: UInt64
    let openCount: UInt64
    let item: FlitRunActiveAttentionSlot

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case eventSchemaVersion = "event_schema_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case openCount = "open_count"
        case item
    }
}

struct FlitAttentionAcknowledgeRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let attentionId: String
    let attentionVersion: UInt64
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case attentionId = "attention_id"
        case attentionVersion = "attention_version"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitAttentionAcknowledgeStatus: String, Codable, Sendable {
    case applied
    case rejected
}

enum FlitAttentionAcknowledgeRejectedReason: String, Codable, Sendable {
    case runVersionStale = "run_version_stale"
    case attentionMismatch = "attention_mismatch"
    case notAcknowledgeable = "not_acknowledgeable"
    case alreadyApplied = "already_applied"
}

struct FlitAttentionAcknowledgeResponse: Codable, Equatable, Sendable {
    let status: FlitAttentionAcknowledgeStatus
    let protocolVersion: String
    let runId: String
    let attentionId: String
    let attentionVersion: UInt64
    let previousVersion: UInt64?
    let eventId: String?
    let eventVersion: UInt64?
    let expectedRunVersion: UInt64?
    let reason: FlitAttentionAcknowledgeRejectedReason?

    private enum CodingKeys: String, CodingKey, CaseIterable {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case attentionId = "attention_id"
        case attentionVersion = "attention_version"
        case previousVersion = "previous_version"
        case eventId = "event_id"
        case eventVersion = "event_version"
        case expectedRunVersion = "expected_run_version"
        case reason
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        status = try values.decode(FlitAttentionAcknowledgeStatus.self, forKey: .status)
        protocolVersion = try values.decode(String.self, forKey: .protocolVersion)
        runId = try values.decode(String.self, forKey: .runId)
        attentionId = try values.decode(String.self, forKey: .attentionId)
        attentionVersion = try values.decode(UInt64.self, forKey: .attentionVersion)
        switch status {
        case .applied:
            previousVersion = try values.decode(UInt64.self, forKey: .previousVersion)
            eventId = try values.decode(String.self, forKey: .eventId)
            eventVersion = try values.decode(UInt64.self, forKey: .eventVersion)
            expectedRunVersion = nil
            reason = nil
            for key in [CodingKeys.expectedRunVersion, .reason] where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "applied acknowledgement must not contain rejection fields"
                )
            }
        case .rejected:
            previousVersion = nil
            eventId = nil
            eventVersion = nil
            expectedRunVersion = try values.decode(UInt64.self, forKey: .expectedRunVersion)
            reason = try values.decode(
                FlitAttentionAcknowledgeRejectedReason.self,
                forKey: .reason
            )
            for key in [CodingKeys.previousVersion, .eventId, .eventVersion]
            where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "rejected acknowledgement must not contain applied fields"
                )
            }
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(status, forKey: .status)
        try values.encode(protocolVersion, forKey: .protocolVersion)
        try values.encode(runId, forKey: .runId)
        try values.encode(attentionId, forKey: .attentionId)
        try values.encode(attentionVersion, forKey: .attentionVersion)
        switch status {
        case .applied:
            try values.encode(previousVersion, forKey: .previousVersion)
            try values.encode(eventId, forKey: .eventId)
            try values.encode(eventVersion, forKey: .eventVersion)
        case .rejected:
            try values.encode(expectedRunVersion, forKey: .expectedRunVersion)
            try values.encode(reason, forKey: .reason)
        }
    }
}

struct FlitRunChangesReadRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let afterCursor: String?
    let requestedChangeLimit: UInt32
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case afterCursor = "after_cursor"
        case requestedChangeLimit = "requested_change_limit"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitRunChangeHeadAvailability: String, Codable, Sendable {
    case available
    case unavailable
}

enum FlitRunChangeHead: Codable, Equatable, Sendable {
    case available(oid: String)
    case unavailable

    private enum CodingKeys: String, CodingKey {
        case availability
        case oid
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(FlitRunChangeHeadAvailability.self, forKey: .availability) {
        case .available:
            self = .available(oid: try container.decode(String.self, forKey: .oid))
        case .unavailable:
            guard !container.contains(.oid) else {
                throw DecodingError.dataCorruptedError(
                    forKey: .oid,
                    in: container,
                    debugDescription: "An unavailable change HEAD cannot include an object ID"
                )
            }
            self = .unavailable
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .available(oid):
            try container.encode(FlitRunChangeHeadAvailability.available, forKey: .availability)
            try container.encode(oid, forKey: .oid)
        case .unavailable:
            try container.encode(FlitRunChangeHeadAvailability.unavailable, forKey: .availability)
        }
    }
}

enum FlitRunFileChangeStatus: String, Codable, Sendable {
    case added
    case modified
    case deleted
    case typeChanged = "type_changed"
    case untracked
}

enum FlitRunFileProjectScope: String, Codable, Sendable {
    case insideProject = "inside_project"
    case outsideProject = "outside_project"
}

struct FlitRunFileChangeRecord: Codable, Equatable, Sendable {
    let changeId: String
    let displayPath: String
    let status: FlitRunFileChangeStatus
    let committed: Bool
    let staged: Bool
    let unstaged: Bool
    let binary: Bool
    let insertions: UInt64?
    let deletions: UInt64?
    let projectScope: FlitRunFileProjectScope

    private enum CodingKeys: String, CodingKey {
        case changeId = "change_id"
        case displayPath = "display_path"
        case status
        case committed
        case staged
        case unstaged
        case binary
        case insertions
        case deletions
        case projectScope = "project_scope"
    }
}

enum FlitRunChangesUnavailableReason: String, Codable, Sendable {
    case changeSetNotAvailable = "change_set_not_available"
}

enum FlitRunChangesAvailability: String, Codable, Sendable {
    case available
    case unavailable
}

struct FlitRunChangesAvailableResponse: Codable, Equatable, Sendable {
    let availability: FlitRunChangesAvailability
    let protocolVersion: String
    let runId: String
    let runVersion: UInt64
    let attribution: FlitDashboardChangeAttribution
    let baselineHead: FlitRunChangeHead
    let terminalHead: FlitRunChangeHead
    let nextCursor: String?
    let hasMore: Bool
    let changes: [FlitRunFileChangeRecord]

    private enum CodingKeys: String, CodingKey {
        case availability
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case attribution
        case baselineHead = "baseline_head"
        case terminalHead = "terminal_head"
        case nextCursor = "next_cursor"
        case hasMore = "has_more"
        case changes
    }
}

struct FlitRunChangesUnavailableResponse: Codable, Equatable, Sendable {
    let availability: FlitRunChangesAvailability
    let protocolVersion: String
    let runId: String
    let runVersion: UInt64
    let reason: FlitRunChangesUnavailableReason

    private enum CodingKeys: String, CodingKey {
        case availability
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case reason
    }
}

enum FlitRunChangesReadResponse: Codable, Equatable, Sendable {
    case available(FlitRunChangesAvailableResponse)
    case unavailable(FlitRunChangesUnavailableResponse)

    private enum CodingKeys: String, CodingKey {
        case availability
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(FlitRunChangesAvailability.self, forKey: .availability) {
        case .available:
            self = .available(try FlitRunChangesAvailableResponse(from: decoder))
        case .unavailable:
            self = .unavailable(try FlitRunChangesUnavailableResponse(from: decoder))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .available(response):
            try response.encode(to: encoder)
        case let .unavailable(response):
            try response.encode(to: encoder)
        }
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

struct FlitRunChangeExternalOpenRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let changeId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case changeId = "change_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitRunChangeExternalOpenDisabledReason: String, Codable, Sendable {
    case changeSetNotAvailable = "change_set_not_available"
    case changeNotFound = "change_not_found"
    case deletedChange = "deleted_change"
    case outsideProject = "outside_project"
    case projectIdentityMismatch = "project_identity_mismatch"
    case repositoryIdentityMismatch = "repository_identity_mismatch"
    case targetUnavailable = "target_unavailable"
    case symlinkEscape = "symlink_escape"
    case targetNotFile = "target_not_file"
    case targetIdentityDrift = "target_identity_drift"
    case openFailed = "open_failed"
}

enum FlitRunChangeExternalOpenStatus: String, Codable, Sendable {
    case opened
    case disabled
}

struct FlitRunChangeExternalOpenOpenedResponse: Codable, Equatable, Sendable {
    let status: FlitRunChangeExternalOpenStatus
    let protocolVersion: String
    let runId: String
    let runVersion: UInt64
    let changeId: String

    private enum CodingKeys: String, CodingKey {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case changeId = "change_id"
    }
}

struct FlitRunChangeExternalOpenDisabledResponse: Codable, Equatable, Sendable {
    let status: FlitRunChangeExternalOpenStatus
    let protocolVersion: String
    let runId: String
    let runVersion: UInt64
    let changeId: String
    let reason: FlitRunChangeExternalOpenDisabledReason

    private enum CodingKeys: String, CodingKey {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case changeId = "change_id"
        case reason
    }
}

enum FlitRunChangeExternalOpenResponse: Codable, Equatable, Sendable {
    case opened(FlitRunChangeExternalOpenOpenedResponse)
    case disabled(FlitRunChangeExternalOpenDisabledResponse)

    private enum CodingKeys: String, CodingKey {
        case status
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(FlitRunChangeExternalOpenStatus.self, forKey: .status) {
        case .opened:
            self = .opened(try FlitRunChangeExternalOpenOpenedResponse(from: decoder))
        case .disabled:
            self = .disabled(try FlitRunChangeExternalOpenDisabledResponse(from: decoder))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .opened(response):
            try response.encode(to: encoder)
        case let .disabled(response):
            try response.encode(to: encoder)
        }
    }
}

enum FlitCommandErrorCode: String, Codable, Sendable {
    case protocolMismatch = "PROTOCOL_MISMATCH"
    case invalidProjectRequest = "INVALID_PROJECT_REQUEST"
    case invalidNotificationPolicy = "INVALID_NOTIFICATION_POLICY"
    case notificationPolicyVersionStale = "NOTIFICATION_POLICY_VERSION_STALE"
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
    let gitBaselineObservedAt: String
    let startedAt: String
    let runCreatedEventId: String
    let gitBaselineEventId: String
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
        case gitBaselineObservedAt = "git_baseline_observed_at"
        case startedAt = "started_at"
        case runCreatedEventId = "run_created_event_id"
        case gitBaselineEventId = "git_baseline_event_id"
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

struct FlitManagedRunsAssessStuckRequest: Codable, Equatable, Sendable {
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitManagedRunsAssessStuckResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let assessedRuns: UInt32
    let transitionsAppended: UInt32
    let unchangedRuns: UInt32
    let unavailableRuns: UInt32

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case assessedRuns = "assessed_runs"
        case transitionsAppended = "transitions_appended"
        case unchangedRuns = "unchanged_runs"
        case unavailableRuns = "unavailable_runs"
    }
}

enum FlitNotificationDeliveryKind: String, Codable, Sendable {
    case permission
    case question
    case failure
    case completion
    case stuck
}

struct FlitNotificationDeliveriesDueReadRequest: Codable, Equatable, Sendable {
    let localMinute: UInt16
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case localMinute = "local_minute"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitNotificationDeliveryRecord: Codable, Equatable, Sendable {
    let notificationId: String
    let runId: String
    let runVersion: UInt64
    let projectId: String
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let deliveryClaimed: Bool
    let catchUp: Bool

    private enum CodingKeys: String, CodingKey {
        case notificationId = "notification_id"
        case runId = "run_id"
        case runVersion = "run_version"
        case projectId = "project_id"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case deliveryClaimed = "delivery_claimed"
        case catchUp = "catch_up"
    }
}

struct FlitNotificationDeliveriesDueReadResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let notifications: [FlitNotificationDeliveryRecord]

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case notifications
    }
}

struct FlitNotificationDeliveryClaimRequest: Codable, Equatable, Sendable {
    let notificationId: String
    let runId: String
    let expectedRunVersion: UInt64
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let localMinute: UInt16
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case notificationId = "notification_id"
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case localMinute = "local_minute"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitNotificationDeliveryClaimResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let notificationId: String
    let runId: String
    let runVersion: UInt64
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let alreadyClaimed: Bool

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case notificationId = "notification_id"
        case runId = "run_id"
        case runVersion = "run_version"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case alreadyClaimed = "already_claimed"
    }
}

struct FlitNotificationDeliveryFailedRequest: Codable, Equatable, Sendable {
    let notificationId: String
    let runId: String
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case notificationId = "notification_id"
        case runId = "run_id"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitNotificationDeliveryFailedResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let notificationId: String
    let runId: String
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let released: Bool

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case notificationId = "notification_id"
        case runId = "run_id"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case released
    }
}

struct FlitNotificationDeliveredRequest: Codable, Equatable, Sendable {
    let notificationId: String
    let runId: String
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case notificationId = "notification_id"
        case runId = "run_id"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitNotificationDeliveredResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let notificationId: String
    let runId: String
    let kind: FlitNotificationDeliveryKind
    let itemId: String
    let itemVersion: UInt64
    let platformId: String
    let alreadyDelivered: Bool

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case notificationId = "notification_id"
        case runId = "run_id"
        case kind
        case itemId = "item_id"
        case itemVersion = "item_version"
        case platformId = "platform_id"
        case alreadyDelivered = "already_delivered"
    }
}

struct FlitManagedStuckNotificationsDueReadRequest: Codable, Equatable, Sendable {
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitManagedStuckNotificationDueRecord: Codable, Equatable, Sendable {
    let runId: String
    let runVersion: UInt64
    let occurrenceId: String
    let platformId: String
    let deliveryClaimed: Bool

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case runVersion = "run_version"
        case occurrenceId = "occurrence_id"
        case platformId = "platform_id"
        case deliveryClaimed = "delivery_claimed"
    }
}

struct FlitManagedStuckNotificationDeliveryClaimRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let occurrenceId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case occurrenceId = "occurrence_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitManagedStuckNotificationDeliveryClaimResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let runId: String
    let runVersion: UInt64
    let occurrenceId: String
    let platformId: String
    let alreadyClaimed: Bool

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case occurrenceId = "occurrence_id"
        case platformId = "platform_id"
        case alreadyClaimed = "already_claimed"
    }
}

struct FlitManagedStuckNotificationDeliveryFailedRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let occurrenceId: String
    let platformId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case occurrenceId = "occurrence_id"
        case platformId = "platform_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

struct FlitManagedStuckNotificationDeliveryFailedResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let runId: String
    let runVersion: UInt64
    let occurrenceId: String
    let platformId: String
    let released: Bool

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case runVersion = "run_version"
        case occurrenceId = "occurrence_id"
        case platformId = "platform_id"
        case released
    }
}

struct FlitManagedStuckNotificationsDueReadResponse: Codable, Equatable, Sendable {
    let protocolVersion: String
    let eventSchemaVersion: String
    let notifications: [FlitManagedStuckNotificationDueRecord]

    private enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case eventSchemaVersion = "event_schema_version"
        case notifications
    }
}

struct FlitManagedStuckNotificationDeliveredRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let occurrenceId: String
    let platformId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case occurrenceId = "occurrence_id"
        case platformId = "platform_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitManagedStuckNotificationDeliveredStatus: String, Codable, Sendable {
    case delivered
    case rejected
}

enum FlitManagedStuckNotificationDeliveredRejectedReason: String, Codable, Sendable {
    case runVersionStale = "run_version_stale"
    case occurrenceMismatch = "occurrence_mismatch"
    case notDue = "not_due"
    case alreadyDelivered = "already_delivered"
}

struct FlitManagedStuckNotificationDeliveredResponse: Codable, Equatable, Sendable {
    let status: FlitManagedStuckNotificationDeliveredStatus
    let protocolVersion: String
    let runId: String
    let occurrenceId: String
    let platformId: String
    let previousVersion: UInt64?
    let eventId: String?
    let eventVersion: UInt64?
    let expectedRunVersion: UInt64?
    let reason: FlitManagedStuckNotificationDeliveredRejectedReason?

    private enum CodingKeys: String, CodingKey, CaseIterable {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case occurrenceId = "occurrence_id"
        case platformId = "platform_id"
        case previousVersion = "previous_version"
        case eventId = "event_id"
        case eventVersion = "event_version"
        case expectedRunVersion = "expected_run_version"
        case reason
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        status = try values.decode(
            FlitManagedStuckNotificationDeliveredStatus.self,
            forKey: .status
        )
        protocolVersion = try values.decode(String.self, forKey: .protocolVersion)
        runId = try values.decode(String.self, forKey: .runId)
        occurrenceId = try values.decode(String.self, forKey: .occurrenceId)
        platformId = try values.decode(String.self, forKey: .platformId)
        switch status {
        case .delivered:
            previousVersion = try values.decode(UInt64.self, forKey: .previousVersion)
            eventId = try values.decode(String.self, forKey: .eventId)
            eventVersion = try values.decode(UInt64.self, forKey: .eventVersion)
            expectedRunVersion = nil
            reason = nil
            for key in [CodingKeys.expectedRunVersion, .reason] where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "delivered notification response must not contain rejection fields"
                )
            }
        case .rejected:
            expectedRunVersion = try values.decode(UInt64.self, forKey: .expectedRunVersion)
            reason = try values.decode(
                FlitManagedStuckNotificationDeliveredRejectedReason.self,
                forKey: .reason
            )
            previousVersion = nil
            eventId = nil
            eventVersion = nil
            for key in [CodingKeys.previousVersion, .eventId, .eventVersion]
            where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "rejected notification response must not contain delivery fields"
                )
            }
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(status, forKey: .status)
        try values.encode(protocolVersion, forKey: .protocolVersion)
        try values.encode(runId, forKey: .runId)
        try values.encode(occurrenceId, forKey: .occurrenceId)
        try values.encode(platformId, forKey: .platformId)
        switch status {
        case .delivered:
            try values.encode(previousVersion, forKey: .previousVersion)
            try values.encode(eventId, forKey: .eventId)
            try values.encode(eventVersion, forKey: .eventVersion)
        case .rejected:
            try values.encode(expectedRunVersion, forKey: .expectedRunVersion)
            try values.encode(reason, forKey: .reason)
        }
    }
}

struct FlitManagedRunStillWorkingRequest: Codable, Equatable, Sendable {
    let runId: String
    let expectedRunVersion: UInt64
    let occurrenceId: String
    let clientProtocolVersion: String

    private enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case expectedRunVersion = "expected_run_version"
        case occurrenceId = "occurrence_id"
        case clientProtocolVersion = "client_protocol_version"
    }
}

enum FlitManagedRunStillWorkingStatus: String, Codable, Sendable {
    case applied
    case rejected
}

enum FlitManagedRunStillWorkingRejectedReason: String, Codable, Sendable {
    case runVersionStale = "run_version_stale"
    case occurrenceMismatch = "occurrence_mismatch"
    case notCurrentlyStuck = "not_currently_stuck"
    case processUnavailable = "process_unavailable"
    case alreadyApplied = "already_applied"
}

struct FlitManagedRunStillWorkingResponse: Codable, Equatable, Sendable {
    let status: FlitManagedRunStillWorkingStatus
    let protocolVersion: String
    let runId: String
    let occurrenceId: String
    let previousVersion: UInt64?
    let eventId: String?
    let eventVersion: UInt64?
    let expectedRunVersion: UInt64?
    let reason: FlitManagedRunStillWorkingRejectedReason?

    private enum CodingKeys: String, CodingKey, CaseIterable {
        case status
        case protocolVersion = "protocol_version"
        case runId = "run_id"
        case occurrenceId = "occurrence_id"
        case previousVersion = "previous_version"
        case eventId = "event_id"
        case eventVersion = "event_version"
        case expectedRunVersion = "expected_run_version"
        case reason
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        status = try values.decode(FlitManagedRunStillWorkingStatus.self, forKey: .status)
        protocolVersion = try values.decode(String.self, forKey: .protocolVersion)
        runId = try values.decode(String.self, forKey: .runId)
        occurrenceId = try values.decode(String.self, forKey: .occurrenceId)
        switch status {
        case .applied:
            previousVersion = try values.decode(UInt64.self, forKey: .previousVersion)
            eventId = try values.decode(String.self, forKey: .eventId)
            eventVersion = try values.decode(UInt64.self, forKey: .eventVersion)
            expectedRunVersion = nil
            reason = nil
            for key in [CodingKeys.expectedRunVersion, .reason] where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "applied Still working response must not contain rejection fields"
                )
            }
        case .rejected:
            expectedRunVersion = try values.decode(UInt64.self, forKey: .expectedRunVersion)
            reason = try values.decode(
                FlitManagedRunStillWorkingRejectedReason.self,
                forKey: .reason
            )
            previousVersion = nil
            eventId = nil
            eventVersion = nil
            for key in [
                CodingKeys.previousVersion,
                .eventId,
                .eventVersion,
            ] where values.contains(key) {
                throw DecodingError.dataCorruptedError(
                    forKey: key,
                    in: values,
                    debugDescription: "rejected Still working response must not contain applied fields"
                )
            }
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(status, forKey: .status)
        try values.encode(protocolVersion, forKey: .protocolVersion)
        try values.encode(runId, forKey: .runId)
        try values.encode(occurrenceId, forKey: .occurrenceId)
        switch status {
        case .applied:
            try values.encode(previousVersion, forKey: .previousVersion)
            try values.encode(eventId, forKey: .eventId)
            try values.encode(eventVersion, forKey: .eventVersion)
        case .rejected:
            try values.encode(expectedRunVersion, forKey: .expectedRunVersion)
            try values.encode(reason, forKey: .reason)
        }
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
