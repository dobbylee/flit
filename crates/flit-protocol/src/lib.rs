use std::collections::BTreeMap;

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: &str = "1.0";
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
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandErrorCode {
    ProtocolMismatch,
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
    pub fn protocol_mismatch() -> Self {
        Self {
            code: CommandErrorCode::ProtocolMismatch,
            message_key: "errors.protocolMismatch".to_owned(),
        }
    }
}

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
