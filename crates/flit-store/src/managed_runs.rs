use std::path::{Component, Path, PathBuf};

use flit_protocol::EventEnvelope;
use serde_json::{Map, Value};

pub const MANAGED_PROVIDER_KIND_CODEX: &str = "codex";
pub const MAX_MANAGED_METADATA_JSON_BYTES: usize = 256 * 1024;
pub const MAX_MANAGED_METADATA_JSON_DEPTH: usize = 32;
pub const MAX_MANAGED_METADATA_JSON_VALUES: usize = 4_096;
pub const MAX_LIVE_MANAGED_SESSIONS: usize = 100;
const MAX_MANAGED_ID_BYTES: usize = 256;
const MAX_MANAGED_TITLE_BYTES: usize = 4 * 1024;
const MAX_MANAGED_GOAL_BYTES: usize = 64 * 1024;
const MAX_MANAGED_PATH_BYTES: usize = 16 * 1024;
const MAX_MANAGED_FINGERPRINT_BYTES: usize = 4 * 1024;
const MAX_MANAGED_TIMESTAMP_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedRunIntent {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub start_request: Map<String, Value>,
    pub baseline_head: Option<String>,
    pub created_at: String,
    pub run_created_event_id: String,
    pub start_requested_event_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedRun {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub provider_kind: String,
    pub start_request: Map<String, Value>,
    pub baseline_head: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ManagedRunIntentOutcome {
    Created {
        run: ManagedRun,
        events: Vec<EventEnvelope>,
    },
    Duplicate {
        run: ManagedRun,
        events: Vec<EventEnvelope>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitialManagedSessionConnection {
    pub id: String,
    pub run_id: String,
    pub external_session_key: String,
    pub session_fingerprint: String,
    pub executable_path: Option<PathBuf>,
    pub executable_version: Option<String>,
    pub cwd: PathBuf,
    pub capabilities: Map<String, Value>,
    pub contract_version: String,
    pub started_at: String,
    pub connected_event_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedSession {
    pub id: String,
    pub run_id: String,
    pub ordinal: u64,
    pub provider_kind: String,
    pub external_session_key: String,
    pub session_fingerprint: String,
    pub executable_path: Option<PathBuf>,
    pub executable_version: Option<String>,
    pub cwd: PathBuf,
    pub capabilities: Map<String, Value>,
    pub provider_cursor: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub end_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InitialManagedSessionOutcome {
    Connected {
        session: ManagedSession,
        event: EventEnvelope,
    },
    Duplicate {
        session: ManagedSession,
        event: EventEnvelope,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedTurnTerminalOutcome {
    Completed,
    Interrupted,
}

impl ManagedTurnTerminalOutcome {
    pub(crate) const fn event_type(self) -> &'static str {
        match self {
            Self::Completed => "run.completed",
            Self::Interrupted => "run.interrupted",
        }
    }

    pub(crate) const fn end_reason(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedSessionTermination {
    pub run_id: String,
    pub session_id: String,
    pub external_session_key: String,
    pub provider_turn_id: String,
    pub contract_version: String,
    pub stream_seq: u64,
    pub ended_at: String,
    pub terminal_event_id: String,
    pub outcome: ManagedTurnTerminalOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ManagedSessionTerminationOutcome {
    Terminated {
        run: ManagedRun,
        session: ManagedSession,
        event: EventEnvelope,
    },
    Duplicate {
        run: ManagedRun,
        session: ManagedSession,
        event: EventEnvelope,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedReconciliationState {
    NoTurns,
    Completed,
    Failed,
    Interrupted,
    Unknown,
    Missing,
    ScopeConflict,
}

impl ManagedReconciliationState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NoTurns => "no_turns",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
            Self::Missing => "missing",
            Self::ScopeConflict => "scope_conflict",
        }
    }

    pub(crate) const fn terminal_event_type(self) -> Option<&'static str> {
        match self {
            Self::Completed => Some("run.completed"),
            Self::Failed => Some("run.failed"),
            Self::Interrupted => Some("run.interrupted"),
            Self::NoTurns | Self::Unknown | Self::Missing | Self::ScopeConflict => None,
        }
    }

    pub(crate) const fn end_reason(self) -> Option<&'static str> {
        match self {
            Self::Completed => Some("completed"),
            Self::Failed => Some("failed"),
            Self::Interrupted => Some("interrupted"),
            Self::NoTurns | Self::Unknown | Self::Missing | Self::ScopeConflict => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedSessionReconciliation {
    pub run_id: String,
    pub session_id: String,
    pub external_session_key: String,
    pub state: ManagedReconciliationState,
    pub latest_turn_id: Option<String>,
    pub contract_version: String,
    pub observed_at: String,
    pub gap_event_id: String,
    pub terminal_event_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ManagedSessionReconciliationOutcome {
    Recorded {
        run: ManagedRun,
        session: ManagedSession,
        events: Vec<EventEnvelope>,
    },
    Duplicate {
        run: ManagedRun,
        session: ManagedSession,
        events: Vec<EventEnvelope>,
    },
}

pub(crate) fn validate_run_intent(intent: &ManagedRunIntent) -> Result<(), &'static str> {
    validate_id(&intent.id).map_err(|()| "id")?;
    validate_id(&intent.project_id).map_err(|()| "project_id")?;
    validate_text(&intent.title, MAX_MANAGED_TITLE_BYTES).map_err(|()| "title")?;
    validate_optional_text(intent.goal.as_deref(), MAX_MANAGED_GOAL_BYTES).map_err(|()| "goal")?;
    validate_optional_token(intent.baseline_head.as_deref(), MAX_MANAGED_ID_BYTES)
        .map_err(|()| "baseline_head")?;
    validate_timestamp(&intent.created_at).map_err(|()| "created_at")?;
    validate_id(&intent.run_created_event_id).map_err(|()| "run_created_event_id")?;
    validate_id(&intent.start_requested_event_id).map_err(|()| "start_requested_event_id")?;
    if intent.run_created_event_id == intent.start_requested_event_id {
        return Err("event_ids");
    }
    validate_json_size(&intent.start_request).map_err(|()| "start_request")
}

pub(crate) fn validate_initial_session(
    connection: &InitialManagedSessionConnection,
) -> Result<(), &'static str> {
    validate_id(&connection.id).map_err(|()| "id")?;
    validate_id(&connection.run_id).map_err(|()| "run_id")?;
    validate_id(&connection.external_session_key).map_err(|()| "external_session_key")?;
    validate_token(
        &connection.session_fingerprint,
        MAX_MANAGED_FINGERPRINT_BYTES,
    )
    .map_err(|()| "session_fingerprint")?;
    validate_optional_path(connection.executable_path.as_deref())
        .map_err(|()| "executable_path")?;
    validate_optional_token(
        connection.executable_version.as_deref(),
        MAX_MANAGED_ID_BYTES,
    )
    .map_err(|()| "executable_version")?;
    validate_canonical_path(&connection.cwd).map_err(|()| "cwd")?;
    validate_json_size(&connection.capabilities).map_err(|()| "capabilities")?;
    validate_token(&connection.contract_version, MAX_MANAGED_ID_BYTES)
        .map_err(|()| "contract_version")?;
    validate_timestamp(&connection.started_at).map_err(|()| "started_at")?;
    validate_id(&connection.connected_event_id).map_err(|()| "connected_event_id")
}

pub(crate) fn validate_session_termination(
    termination: &ManagedSessionTermination,
) -> Result<(), &'static str> {
    validate_id(&termination.run_id).map_err(|()| "run_id")?;
    validate_id(&termination.session_id).map_err(|()| "session_id")?;
    validate_id(&termination.external_session_key).map_err(|()| "external_session_key")?;
    validate_id(&termination.provider_turn_id).map_err(|()| "provider_turn_id")?;
    validate_token(&termination.contract_version, MAX_MANAGED_ID_BYTES)
        .map_err(|()| "contract_version")?;
    if termination.stream_seq <= 1 || termination.stream_seq > flit_protocol::MAX_JSON_SAFE_INTEGER
    {
        return Err("stream_seq");
    }
    validate_timestamp(&termination.ended_at).map_err(|()| "ended_at")?;
    validate_id(&termination.terminal_event_id).map_err(|()| "terminal_event_id")
}

pub(crate) fn validate_session_reconciliation(
    reconciliation: &ManagedSessionReconciliation,
) -> Result<(), &'static str> {
    validate_id(&reconciliation.run_id).map_err(|()| "run_id")?;
    validate_id(&reconciliation.session_id).map_err(|()| "session_id")?;
    validate_id(&reconciliation.external_session_key).map_err(|()| "external_session_key")?;
    validate_optional_token(
        reconciliation.latest_turn_id.as_deref(),
        MAX_MANAGED_ID_BYTES,
    )
    .map_err(|()| "latest_turn_id")?;
    validate_token(&reconciliation.contract_version, MAX_MANAGED_ID_BYTES)
        .map_err(|()| "contract_version")?;
    validate_timestamp(&reconciliation.observed_at).map_err(|()| "observed_at")?;
    validate_id(&reconciliation.gap_event_id).map_err(|()| "gap_event_id")?;
    validate_optional_token(
        reconciliation.terminal_event_id.as_deref(),
        MAX_MANAGED_ID_BYTES,
    )
    .map_err(|()| "terminal_event_id")?;
    if reconciliation
        .terminal_event_id
        .as_ref()
        .is_some_and(|event_id| event_id == &reconciliation.gap_event_id)
    {
        return Err("event_ids");
    }

    let terminal = reconciliation.state.terminal_event_type().is_some();
    if terminal
        && (reconciliation.latest_turn_id.is_none() || reconciliation.terminal_event_id.is_none())
    {
        return Err("state");
    }
    if !terminal && reconciliation.terminal_event_id.is_some() {
        return Err("state");
    }
    if matches!(
        reconciliation.state,
        ManagedReconciliationState::NoTurns
            | ManagedReconciliationState::Missing
            | ManagedReconciliationState::ScopeConflict
    ) && reconciliation.latest_turn_id.is_some()
    {
        return Err("state");
    }
    Ok(())
}

pub(crate) fn validate_stored_run(run: &ManagedRun) -> Result<(), &'static str> {
    validate_id(&run.id).map_err(|()| "id")?;
    validate_id(&run.project_id).map_err(|()| "project_id")?;
    validate_text(&run.title, MAX_MANAGED_TITLE_BYTES).map_err(|()| "title")?;
    validate_optional_text(run.goal.as_deref(), MAX_MANAGED_GOAL_BYTES).map_err(|()| "goal")?;
    if run.provider_kind != MANAGED_PROVIDER_KIND_CODEX {
        return Err("provider_kind");
    }
    validate_json_size(&run.start_request).map_err(|()| "start_request")?;
    validate_optional_token(run.baseline_head.as_deref(), MAX_MANAGED_ID_BYTES)
        .map_err(|()| "baseline_head")?;
    validate_timestamp(&run.created_at).map_err(|()| "created_at")?;
    validate_optional_timestamp(run.started_at.as_deref()).map_err(|()| "started_at")?;
    validate_optional_timestamp(run.ended_at.as_deref()).map_err(|()| "ended_at")
}

pub(crate) fn validate_stored_session(session: &ManagedSession) -> Result<(), &'static str> {
    validate_id(&session.id).map_err(|()| "id")?;
    validate_id(&session.run_id).map_err(|()| "run_id")?;
    if session.ordinal == 0 || session.ordinal > flit_protocol::MAX_JSON_SAFE_INTEGER {
        return Err("ordinal");
    }
    if session.provider_kind != MANAGED_PROVIDER_KIND_CODEX {
        return Err("provider_kind");
    }
    validate_id(&session.external_session_key).map_err(|()| "external_session_key")?;
    validate_token(&session.session_fingerprint, MAX_MANAGED_FINGERPRINT_BYTES)
        .map_err(|()| "session_fingerprint")?;
    validate_optional_path(session.executable_path.as_deref()).map_err(|()| "executable_path")?;
    validate_optional_token(session.executable_version.as_deref(), MAX_MANAGED_ID_BYTES)
        .map_err(|()| "executable_version")?;
    validate_canonical_path(&session.cwd).map_err(|()| "cwd")?;
    validate_json_size(&session.capabilities).map_err(|()| "capabilities")?;
    validate_optional_token(session.provider_cursor.as_deref(), MAX_MANAGED_ID_BYTES)
        .map_err(|()| "provider_cursor")?;
    validate_timestamp(&session.started_at).map_err(|()| "started_at")?;
    validate_optional_timestamp(session.ended_at.as_deref()).map_err(|()| "ended_at")?;
    validate_optional_token(session.end_reason.as_deref(), MAX_MANAGED_ID_BYTES)
        .map_err(|()| "end_reason")?;
    if session.ended_at.is_some() != session.end_reason.is_some() {
        return Err("termination");
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), ()> {
    validate_token(value, MAX_MANAGED_ID_BYTES)
}

fn validate_timestamp(value: &str) -> Result<(), ()> {
    validate_token(value, MAX_MANAGED_TIMESTAMP_BYTES)
}

fn validate_optional_timestamp(value: Option<&str>) -> Result<(), ()> {
    validate_optional_text(value, MAX_MANAGED_TIMESTAMP_BYTES)
}

fn validate_optional_text(value: Option<&str>, max_bytes: usize) -> Result<(), ()> {
    match value {
        Some(value) => validate_text(value, max_bytes),
        None => Ok(()),
    }
}

fn validate_optional_token(value: Option<&str>, max_bytes: usize) -> Result<(), ()> {
    match value {
        Some(value) => validate_token(value, max_bytes),
        None => Ok(()),
    }
}

fn validate_token(value: &str, max_bytes: usize) -> Result<(), ()> {
    validate_text(value, max_bytes)?;
    if value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}

fn validate_text(value: &str, max_bytes: usize) -> Result<(), ()> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(());
    }
    Ok(())
}

fn validate_optional_path(path: Option<&Path>) -> Result<(), ()> {
    match path {
        Some(path) => validate_canonical_path(path),
        None => Ok(()),
    }
}

fn validate_canonical_path(path: &Path) -> Result<(), ()> {
    let Some(value) = path.to_str() else {
        return Err(());
    };
    if !path.is_absolute()
        || value.is_empty()
        || value.len() > MAX_MANAGED_PATH_BYTES
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(());
    }
    Ok(())
}

fn validate_json_size(value: &Map<String, Value>) -> Result<(), ()> {
    let mut value_count = 1_usize;
    let mut pending = value
        .values()
        .map(|value| (value, 1_usize))
        .collect::<Vec<_>>();
    while let Some((value, depth)) = pending.pop() {
        value_count = value_count.checked_add(1).ok_or(())?;
        if depth > MAX_MANAGED_METADATA_JSON_DEPTH || value_count > MAX_MANAGED_METADATA_JSON_VALUES
        {
            return Err(());
        }
        match value {
            Value::Array(values) => {
                pending.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    let bytes = serde_json::to_vec(value).map_err(|_| ())?;
    if bytes.len() > MAX_MANAGED_METADATA_JSON_BYTES {
        return Err(());
    }
    let decoded = serde_json::from_slice::<Map<String, Value>>(&bytes).map_err(|_| ())?;
    if &decoded != value {
        return Err(());
    }
    Ok(())
}
