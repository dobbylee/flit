use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

pub const MAX_CODEX_APP_SERVER_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_CODEX_MANAGED_THREADS: usize = 256;
const MAX_CODEX_THREAD_ID_BYTES: usize = 256;
const MAX_CODEX_CURSOR_BYTES: usize = 4 * 1024;
const MAX_CODEX_CWD_BYTES: usize = 16 * 1024;
const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CodexManagedThreadId(String);

impl CodexManagedThreadId {
    pub fn new(value: impl Into<String>) -> Result<Self, CodexContractError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.len() > MAX_CODEX_THREAD_ID_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(CodexContractError::InvalidThreadId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexManagedScope {
    canonical_cwd: PathBuf,
    exact_thread_ids: BTreeSet<CodexManagedThreadId>,
}

impl CodexManagedScope {
    pub fn new(
        canonical_cwd: impl Into<PathBuf>,
        exact_thread_ids: impl IntoIterator<Item = CodexManagedThreadId>,
    ) -> Result<Self, CodexContractError> {
        let canonical_cwd = canonical_cwd.into();
        validate_canonical_cwd(&canonical_cwd)?;
        let mut unique_thread_ids = BTreeSet::new();
        let mut input_count = 0_usize;
        for thread_id in exact_thread_ids {
            input_count += 1;
            if input_count > MAX_CODEX_MANAGED_THREADS || !unique_thread_ids.insert(thread_id) {
                return Err(CodexContractError::InvalidManagedScope);
            }
        }
        if unique_thread_ids.is_empty() {
            return Err(CodexContractError::InvalidManagedScope);
        }
        Ok(Self {
            canonical_cwd,
            exact_thread_ids: unique_thread_ids,
        })
    }

    pub fn canonical_cwd(&self) -> &Path {
        &self.canonical_cwd
    }

    pub fn exact_thread_ids(&self) -> &BTreeSet<CodexManagedThreadId> {
        &self.exact_thread_ids
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexStartedThread {
    pub thread_id: CodexManagedThreadId,
    pub canonical_cwd: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexManagedThreadConflict {
    pub thread_id: CodexManagedThreadId,
    pub observed_cwd: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexManagedListPage {
    pub matched_thread_ids: Vec<CodexManagedThreadId>,
    pub conflicting_threads: Vec<CodexManagedThreadConflict>,
    pub unseen_exact_thread_ids: Vec<CodexManagedThreadId>,
    pub unrelated_thread_count: usize,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexThreadState {
    NoTurns,
    Completed,
    Failed,
    Interrupted,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexThreadRead {
    pub thread_id: CodexManagedThreadId,
    pub latest_turn_id: Option<String>,
    pub state: CodexThreadState,
}

pub fn codex_initialize_request(request_id: u64) -> Result<Vec<u8>, CodexContractError> {
    encode_client_frame(
        request_id,
        json!({
            "id": request_id,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "flit",
                    "title": "Flit",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            },
        }),
    )
}

pub fn codex_initialized_notification() -> Vec<u8> {
    encode_value(json!({
        "method": "initialized",
        "params": {},
    }))
}

pub fn codex_read_only_start_request(
    request_id: u64,
    canonical_cwd: impl AsRef<Path>,
) -> Result<Vec<u8>, CodexContractError> {
    let canonical_cwd = canonical_cwd.as_ref();
    let cwd = canonical_cwd_string(canonical_cwd)?;
    encode_client_frame(
        request_id,
        json!({
            "id": request_id,
            "method": "thread/start",
            "params": {
                "cwd": cwd,
                "sandbox": "read-only",
                "approvalPolicy": "never",
                "ephemeral": false,
                "serviceName": "flit",
                "threadSource": "flit",
            },
        }),
    )
}

pub fn codex_thread_list_request(
    request_id: u64,
    canonical_cwd: impl AsRef<Path>,
    cursor: Option<&str>,
) -> Result<Vec<u8>, CodexContractError> {
    let cwd = canonical_cwd_string(canonical_cwd.as_ref())?;
    if cursor.is_some_and(|value| value.is_empty() || value.len() > MAX_CODEX_CURSOR_BYTES) {
        return Err(CodexContractError::InvalidCursor);
    }
    let mut params = serde_json::Map::from_iter([("cwd".to_owned(), Value::String(cwd))]);
    if let Some(cursor) = cursor {
        params.insert("cursor".to_owned(), Value::String(cursor.to_owned()));
    }
    encode_client_frame(
        request_id,
        json!({
            "id": request_id,
            "method": "thread/list",
            "params": params,
        }),
    )
}

pub fn codex_read_request(
    request_id: u64,
    thread_id: &CodexManagedThreadId,
) -> Result<Vec<u8>, CodexContractError> {
    encode_client_frame(
        request_id,
        json!({
            "id": request_id,
            "method": "thread/read",
            "params": {
                "threadId": thread_id.as_str(),
                "includeTurns": true,
            },
        }),
    )
}

pub fn decode_codex_start_response(
    frame: &[u8],
    expected_request_id: u64,
    canonical_cwd: impl Into<PathBuf>,
) -> Result<CodexStartedThread, CodexContractError> {
    let canonical_cwd = canonical_cwd.into();
    validate_canonical_cwd(&canonical_cwd)?;
    let result = response_result(frame, expected_request_id)?;
    let thread = required_object(&result, "thread")?;
    let thread_id = equal_thread_and_session_id(thread)?;
    let sandbox = required_object(&result, "sandbox")?;
    if required_string(sandbox, "type")? != "readOnly"
        || required_bool(sandbox, "networkAccess")?
        || required_string(&result, "approvalPolicy")? != "never"
    {
        return Err(CodexContractError::UnexpectedEffectivePolicy);
    }
    Ok(CodexStartedThread {
        thread_id,
        canonical_cwd,
    })
}

pub fn decode_codex_thread_list_response(
    frame: &[u8],
    expected_request_id: u64,
    scope: &CodexManagedScope,
) -> Result<CodexManagedListPage, CodexContractError> {
    let result = response_result(frame, expected_request_id)?;
    let data = required_array(&result, "data")?;
    if data.len() > MAX_CODEX_MANAGED_THREADS {
        return Err(CodexContractError::TooManyThreads);
    }

    let mut observed = BTreeMap::new();
    for value in data {
        let thread = value
            .as_object()
            .ok_or(CodexContractError::InvalidField { field: "data" })?;
        let thread_id = equal_thread_and_session_id(thread)?;
        let cwd = PathBuf::from(required_string(thread, "cwd")?);
        if observed.insert(thread_id, cwd).is_some() {
            return Err(CodexContractError::DuplicateThreadId);
        }
    }

    let mut matched_thread_ids = Vec::new();
    let mut conflicting_threads = Vec::new();
    let mut unrelated_thread_count = 0;
    for (thread_id, observed_cwd) in &observed {
        if !scope.exact_thread_ids.contains(thread_id) {
            unrelated_thread_count += 1;
        } else if observed_cwd == &scope.canonical_cwd {
            matched_thread_ids.push(thread_id.clone());
        } else {
            conflicting_threads.push(CodexManagedThreadConflict {
                thread_id: thread_id.clone(),
                observed_cwd: observed_cwd.clone(),
            });
        }
    }
    let unseen_exact_thread_ids = scope
        .exact_thread_ids
        .iter()
        .filter(|thread_id| !observed.contains_key(*thread_id))
        .cloned()
        .collect();
    let next_cursor = optional_cursor(&result)?;

    Ok(CodexManagedListPage {
        matched_thread_ids,
        conflicting_threads,
        unseen_exact_thread_ids,
        unrelated_thread_count,
        next_cursor,
    })
}

pub fn decode_codex_read_response(
    frame: &[u8],
    expected_request_id: u64,
    expected_thread_id: &CodexManagedThreadId,
) -> Result<CodexThreadRead, CodexContractError> {
    let result = response_result(frame, expected_request_id)?;
    let thread = required_object(&result, "thread")?;
    let observed_thread_id = equal_thread_and_session_id(thread)?;
    if &observed_thread_id != expected_thread_id {
        return Err(CodexContractError::UnexpectedThreadId);
    }
    let turns = required_array(thread, "turns")?;
    if turns.len() > MAX_CODEX_MANAGED_THREADS {
        return Err(CodexContractError::TooManyTurns);
    }
    let Some(latest_turn) = turns.last() else {
        return Ok(CodexThreadRead {
            thread_id: observed_thread_id,
            latest_turn_id: None,
            state: CodexThreadState::NoTurns,
        });
    };
    let latest_turn = latest_turn
        .as_object()
        .ok_or(CodexContractError::InvalidField { field: "turns" })?;
    let latest_turn_id = latest_turn
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_CODEX_THREAD_ID_BYTES)
        .map(str::to_owned);
    let state = match (
        latest_turn_id.as_ref(),
        latest_turn.get("status").and_then(Value::as_str),
    ) {
        (Some(_), Some("completed")) => CodexThreadState::Completed,
        (Some(_), Some("failed")) => CodexThreadState::Failed,
        (Some(_), Some("interrupted")) => CodexThreadState::Interrupted,
        _ => CodexThreadState::Unknown,
    };
    Ok(CodexThreadRead {
        thread_id: observed_thread_id,
        latest_turn_id,
        state,
    })
}

fn encode_client_frame(request_id: u64, value: Value) -> Result<Vec<u8>, CodexContractError> {
    validate_request_id(request_id)?;
    let frame = encode_value(value);
    if frame.len() > MAX_CODEX_APP_SERVER_FRAME_BYTES {
        return Err(CodexContractError::FrameTooLarge);
    }
    Ok(frame)
}

fn encode_value(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).expect("serializing a JSON value cannot fail");
    bytes.push(b'\n');
    bytes
}

fn response_result(
    frame: &[u8],
    expected_request_id: u64,
) -> Result<serde_json::Map<String, Value>, CodexContractError> {
    validate_request_id(expected_request_id)?;
    if frame.len() > MAX_CODEX_APP_SERVER_FRAME_BYTES {
        return Err(CodexContractError::FrameTooLarge);
    }
    let value: Value =
        serde_json::from_slice(frame).map_err(|_| CodexContractError::MalformedJson)?;
    let response = value
        .as_object()
        .ok_or(CodexContractError::InvalidResponse)?;
    if response.get("id").and_then(Value::as_u64) != Some(expected_request_id) {
        return Err(CodexContractError::UnexpectedRequestId);
    }
    if response.contains_key("error") {
        return Err(CodexContractError::ServerError);
    }
    response
        .get("result")
        .and_then(Value::as_object)
        .cloned()
        .ok_or(CodexContractError::MissingResult)
}

fn equal_thread_and_session_id(
    thread: &serde_json::Map<String, Value>,
) -> Result<CodexManagedThreadId, CodexContractError> {
    let thread_id = CodexManagedThreadId::new(required_string(thread, "id")?)?;
    if required_string(thread, "sessionId")? != thread_id.as_str() {
        return Err(CodexContractError::MismatchedSessionIdentity);
    }
    Ok(thread_id)
}

fn required_object<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a serde_json::Map<String, Value>, CodexContractError> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or(CodexContractError::InvalidField { field })
}

fn required_array<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a Vec<Value>, CodexContractError> {
    object
        .get(field)
        .and_then(Value::as_array)
        .ok_or(CodexContractError::InvalidField { field })
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, CodexContractError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or(CodexContractError::InvalidField { field })
}

fn required_bool(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<bool, CodexContractError> {
    object
        .get(field)
        .and_then(Value::as_bool)
        .ok_or(CodexContractError::InvalidField { field })
}

fn optional_cursor(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<String>, CodexContractError> {
    match object.get("nextCursor") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(cursor))
            if !cursor.is_empty() && cursor.len() <= MAX_CODEX_CURSOR_BYTES =>
        {
            Ok(Some(cursor.clone()))
        }
        _ => Err(CodexContractError::InvalidCursor),
    }
}

fn canonical_cwd_string(path: &Path) -> Result<String, CodexContractError> {
    validate_canonical_cwd(path)?;
    path.to_str()
        .map(str::to_owned)
        .ok_or(CodexContractError::InvalidCanonicalCwd)
}

fn validate_canonical_cwd(path: &Path) -> Result<(), CodexContractError> {
    let Some(path_string) = path.to_str() else {
        return Err(CodexContractError::InvalidCanonicalCwd);
    };
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(CodexContractError::InvalidCanonicalCwd);
    }
    if path_string.len() > MAX_CODEX_CWD_BYTES {
        return Err(CodexContractError::FrameTooLarge);
    }
    Ok(())
}

fn validate_request_id(request_id: u64) -> Result<(), CodexContractError> {
    if request_id > MAX_JSON_SAFE_INTEGER {
        return Err(CodexContractError::InvalidRequestId);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexContractError {
    InvalidRequestId,
    InvalidCanonicalCwd,
    InvalidThreadId,
    InvalidManagedScope,
    InvalidCursor,
    FrameTooLarge,
    MalformedJson,
    InvalidResponse,
    UnexpectedRequestId,
    ServerError,
    MissingResult,
    InvalidField { field: &'static str },
    MismatchedSessionIdentity,
    UnexpectedThreadId,
    UnexpectedEffectivePolicy,
    DuplicateThreadId,
    TooManyThreads,
    TooManyTurns,
}

impl fmt::Display for CodexContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequestId => formatter.write_str("invalid Codex request ID"),
            Self::InvalidCanonicalCwd => formatter.write_str("invalid canonical Project cwd"),
            Self::InvalidThreadId => formatter.write_str("invalid Codex managed thread ID"),
            Self::InvalidManagedScope => formatter.write_str("invalid Codex managed scope"),
            Self::InvalidCursor => formatter.write_str("invalid Codex list cursor"),
            Self::FrameTooLarge => formatter.write_str("Codex app-server frame exceeded its limit"),
            Self::MalformedJson => formatter.write_str("malformed Codex app-server JSON"),
            Self::InvalidResponse => formatter.write_str("invalid Codex app-server response"),
            Self::UnexpectedRequestId => {
                formatter.write_str("Codex response request ID did not match")
            }
            Self::ServerError => formatter.write_str("Codex app-server returned an error"),
            Self::MissingResult => formatter.write_str("Codex response did not contain a result"),
            Self::InvalidField { field } => {
                write!(formatter, "invalid Codex response field: {field}")
            }
            Self::MismatchedSessionIdentity => {
                formatter.write_str("Codex thread and session identities did not match")
            }
            Self::UnexpectedThreadId => {
                formatter.write_str("Codex response returned an unmanaged thread ID")
            }
            Self::UnexpectedEffectivePolicy => {
                formatter.write_str("Codex start response did not confirm the read-only policy")
            }
            Self::DuplicateThreadId => {
                formatter.write_str("Codex list response contained a duplicate thread ID")
            }
            Self::TooManyThreads => formatter.write_str("Codex list response had too many threads"),
            Self::TooManyTurns => formatter.write_str("Codex read response had too many turns"),
        }
    }
}

impl Error for CodexContractError {}
