use std::{fs, path::PathBuf};

use flit_protocol::{
    CommandError, EVENT_PROTOCOL_VERSION, EventEnvelope, EventProtocolVersion,
    MAX_JSON_SAFE_INTEGER, PROTOCOL_VERSION, SystemHealthRequest, SystemHealthResponse,
    event_schema_id, event_schema_relative_path,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

fn repository_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn assert_fixture_round_trip<T>(relative: &str)
where
    T: DeserializeOwned + Serialize,
{
    let fixture =
        fs::read_to_string(repository_path(relative)).expect("fixture should be readable");
    let expected: serde_json::Value =
        serde_json::from_str(&fixture).expect("fixture should contain valid JSON");
    let decoded: T =
        serde_json::from_value(expected.clone()).expect("fixture should match Rust type");
    let actual = serde_json::to_value(decoded).expect("Rust type should serialize");
    assert_eq!(actual, expected);
}

#[test]
fn checked_in_typescript_matches_the_rust_source() {
    let generated = fs::read_to_string(repository_path("apps/desktop/src/generated/protocol.ts"))
        .expect("generated TypeScript should be checked in");

    assert_eq!(generated, flit_protocol::generated_typescript());
    assert!(generated.contains("export type NullableSessionId = string | null;"));
    assert!(generated.contains("session_id: NullableSessionId,"));
    assert!(!generated.contains("session_id?:"));
}

#[test]
fn checked_in_event_schema_matches_the_rust_source() {
    let manifest = read_compatibility_manifest();
    assert_eq!(manifest.current.schema, event_schema_relative_path());
    let generated = fs::read_to_string(repository_path(&manifest.current.schema))
        .expect("generated event schema should be checked in");

    assert_eq!(generated, flit_protocol::generated_event_schema());
}

#[test]
fn current_system_health_fixtures_round_trip() {
    assert_fixture_round_trip::<SystemHealthRequest>(
        "fixtures/protocol/commands/v1.0/system_health.request.json",
    );
    assert_fixture_round_trip::<SystemHealthResponse>(
        "fixtures/protocol/commands/v1.0/system_health.response.json",
    );
    assert_fixture_round_trip::<CommandError>(
        "fixtures/protocol/commands/v1.0/protocol_mismatch.error.json",
    );
}

#[test]
fn fixtures_are_bound_to_the_current_protocol_version() {
    let request: SystemHealthRequest = serde_json::from_str(
        &fs::read_to_string(repository_path(
            "fixtures/protocol/commands/v1.0/system_health.request.json",
        ))
        .expect("request fixture should be readable"),
    )
    .expect("request fixture should match Rust type");
    let response: SystemHealthResponse = serde_json::from_str(
        &fs::read_to_string(repository_path(
            "fixtures/protocol/commands/v1.0/system_health.response.json",
        ))
        .expect("response fixture should be readable"),
    )
    .expect("response fixture should match Rust type");

    assert_eq!(request.client_protocol_version, PROTOCOL_VERSION);
    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
}

#[test]
fn current_event_fixture_round_trips_without_losing_unknown_fields() {
    let manifest = read_compatibility_manifest();
    let fixture = fs::read_to_string(repository_path(&manifest.current.fixture))
        .expect("fixture should be readable");
    let expected: serde_json::Value =
        serde_json::from_str(&fixture).expect("fixture should contain valid JSON");
    let decoded: EventEnvelope =
        serde_json::from_value(expected.clone()).expect("fixture should match event envelope");

    assert_eq!(expected["protocol_version"], EVENT_PROTOCOL_VERSION);
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn event_schema_accepts_current_fixture_and_rejects_invalid_boundaries() {
    let manifest = read_compatibility_manifest();
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.schema))
            .expect("event schema should be readable"),
    )
    .expect("event schema should contain valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("event schema should compile");
    let fixture: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.fixture))
            .expect("event fixture should be readable"),
    )
    .expect("event fixture should contain valid JSON");

    assert!(validator.is_valid(&fixture));

    let mut invalid_confidence = fixture.clone();
    invalid_confidence["confidence"] = serde_json::json!(1.01);
    assert!(!validator.is_valid(&invalid_confidence));

    let mut invalid_stream_sequence = fixture.clone();
    invalid_stream_sequence["stream_seq"] = serde_json::json!(0);
    assert!(!validator.is_valid(&invalid_stream_sequence));

    let mut invalid_payload = fixture;
    invalid_payload["payload"] = serde_json::json!("not an object");
    assert!(!validator.is_valid(&invalid_payload));

    let mut unsafe_json_sequence = invalid_payload;
    unsafe_json_sequence["payload"] = serde_json::json!({});
    unsafe_json_sequence["ingest_seq"] = serde_json::json!(MAX_JSON_SAFE_INTEGER + 1);
    assert!(!validator.is_valid(&unsafe_json_sequence));

    let mut missing_session_id = unsafe_json_sequence;
    missing_session_id["ingest_seq"] = serde_json::json!(1);
    missing_session_id
        .as_object_mut()
        .expect("fixture should be an object")
        .remove("session_id");
    assert!(!validator.is_valid(&missing_session_id));
    assert!(serde_json::from_value::<EventEnvelope>(missing_session_id).is_err());
}

#[derive(Clone, Debug, Deserialize)]
struct CompatibilityManifest {
    current: FixtureReference,
    previous_minor: Option<FixtureReference>,
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureReference {
    version: String,
    schema: String,
    fixture: String,
}

fn read_compatibility_manifest() -> CompatibilityManifest {
    serde_json::from_str(
        &fs::read_to_string(repository_path(
            "fixtures/protocol/events/compatibility.json",
        ))
        .expect("compatibility manifest should be readable"),
    )
    .expect("compatibility manifest should contain valid JSON")
}

#[derive(Clone)]
struct CurrentContractSnapshot {
    expected_version: String,
    serialized_version: String,
    generated_schema_path: String,
    schema_id: String,
    fixture_version: String,
    manifest_version: String,
    manifest_schema_path: String,
}

fn validate_current_contract(snapshot: &CurrentContractSnapshot) -> Result<(), String> {
    let expected_path = format!(
        "schemas/protocol/events/v{}/event.schema.json",
        snapshot.expected_version
    );
    let expected_id = format!("urn:flit:protocol:event:{}", snapshot.expected_version);
    if snapshot.serialized_version != snapshot.expected_version {
        return Err("serialized event version must match current".to_owned());
    }
    if snapshot.generated_schema_path != expected_path {
        return Err("generated schema path must match current".to_owned());
    }
    if snapshot.schema_id != expected_id {
        return Err("schema ID must match current".to_owned());
    }
    if snapshot.fixture_version != snapshot.expected_version {
        return Err("fixture event version must match current".to_owned());
    }
    if snapshot.manifest_version != snapshot.expected_version {
        return Err("manifest event version must match current".to_owned());
    }
    if snapshot.manifest_schema_path != expected_path {
        return Err("manifest schema path must match generated path".to_owned());
    }
    Ok(())
}

fn current_contract_snapshot() -> CurrentContractSnapshot {
    let manifest = read_compatibility_manifest();
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.schema))
            .expect("current schema should be readable"),
    )
    .expect("current schema should contain valid JSON");
    let fixture: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repository_path(&manifest.current.fixture))
            .expect("current fixture should be readable"),
    )
    .expect("current fixture should contain valid JSON");
    let serialized_version = serde_json::to_value(EventProtocolVersion::V1_0)
        .expect("event version should serialize")
        .as_str()
        .expect("event version should serialize as a string")
        .to_owned();

    CurrentContractSnapshot {
        expected_version: EVENT_PROTOCOL_VERSION.to_owned(),
        serialized_version,
        generated_schema_path: event_schema_relative_path(),
        schema_id: schema["$id"]
            .as_str()
            .expect("event schema should declare an ID")
            .to_owned(),
        fixture_version: fixture["protocol_version"]
            .as_str()
            .expect("event fixture should declare a protocol version")
            .to_owned(),
        manifest_version: manifest.current.version,
        manifest_schema_path: manifest.current.schema,
    }
}

#[test]
fn current_event_version_sources_cannot_drift_independently() {
    let current = current_contract_snapshot();
    validate_current_contract(&current).expect("current event version sources should agree");
    assert_eq!(current.schema_id, event_schema_id());

    for mutate in [
        |snapshot: &mut CurrentContractSnapshot| snapshot.serialized_version = "0.9".to_owned(),
        |snapshot: &mut CurrentContractSnapshot| {
            snapshot.schema_id = "urn:flit:protocol:event:0.9".to_owned()
        },
        |snapshot: &mut CurrentContractSnapshot| {
            snapshot.generated_schema_path =
                "schemas/protocol/events/v0.9/event.schema.json".to_owned()
        },
        |snapshot: &mut CurrentContractSnapshot| snapshot.fixture_version = "0.9".to_owned(),
        |snapshot: &mut CurrentContractSnapshot| snapshot.manifest_version = "0.9".to_owned(),
        |snapshot: &mut CurrentContractSnapshot| {
            snapshot.manifest_schema_path =
                "schemas/protocol/events/v0.9/event.schema.json".to_owned()
        },
    ] {
        let mut stale = current.clone();
        mutate(&mut stale);
        assert!(validate_current_contract(&stale).is_err());
    }
}

fn validate_compatibility_manifest(
    manifest: &CompatibilityManifest,
    expected_current: &str,
    require_files: bool,
) -> Result<(), String> {
    if manifest.current.version != expected_current {
        return Err("current event version must match the Rust protocol version".to_owned());
    }

    let (major, minor) = manifest
        .current
        .version
        .split_once('.')
        .ok_or_else(|| "event version must be major.minor".to_owned())?;
    let minor = minor
        .parse::<u64>()
        .map_err(|_| "event minor version must be numeric".to_owned())?;

    match (minor.checked_sub(1), &manifest.previous_minor) {
        (None, None) => {}
        (None, Some(_)) => {
            return Err("initial minor must not invent a previous fixture".to_owned());
        }
        (Some(_), None) => return Err("non-initial minor requires a previous fixture".to_owned()),
        (Some(previous_minor), Some(previous)) => {
            let expected = format!("{major}.{previous_minor}");
            if previous.version != expected {
                return Err("previous fixture must be the exact preceding minor".to_owned());
            }
        }
    }

    for reference in std::iter::once(&manifest.current).chain(manifest.previous_minor.as_ref()) {
        let version_segment = format!("/v{}/", reference.version);
        if !reference.schema.contains(&version_segment)
            || !reference.fixture.contains(&version_segment)
        {
            return Err("manifest paths must be bound to their declared version".to_owned());
        }
    }

    if require_files {
        for reference in std::iter::once(&manifest.current).chain(manifest.previous_minor.as_ref())
        {
            let schema_path = repository_path(&reference.schema);
            let fixture_path = repository_path(&reference.fixture);
            if !schema_path.is_file() || !fixture_path.is_file() {
                return Err("manifest schema and fixture paths must exist".to_owned());
            }

            let schema: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(schema_path)
                    .map_err(|_| "manifest schema must be readable".to_owned())?,
            )
            .map_err(|_| "manifest schema must contain valid JSON".to_owned())?;
            let fixture: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(fixture_path)
                    .map_err(|_| "manifest fixture must be readable".to_owned())?,
            )
            .map_err(|_| "manifest fixture must contain valid JSON".to_owned())?;
            let validator = jsonschema::validator_for(&schema)
                .map_err(|_| "manifest schema must compile".to_owned())?;
            if !validator.is_valid(&fixture) {
                return Err("manifest fixture must validate against its schema".to_owned());
            }
        }
    }

    Ok(())
}

#[test]
fn compatibility_manifest_enforces_initial_and_future_minor_rules() {
    let manifest = read_compatibility_manifest();
    validate_compatibility_manifest(&manifest, EVENT_PROTOCOL_VERSION, true)
        .expect("current manifest should be valid");

    let future_without_previous: CompatibilityManifest =
        serde_json::from_value(serde_json::json!({
            "current": {
                "version": "1.1",
                "schema": "schemas/protocol/events/v1.1/event.schema.json",
                "fixture": "fixtures/protocol/events/v1.1/permission.requested.json"
            },
            "previous_minor": null
        }))
        .unwrap();
    assert!(validate_compatibility_manifest(&future_without_previous, "1.1", false).is_err());

    let future_with_wrong_previous: CompatibilityManifest =
        serde_json::from_value(serde_json::json!({
            "current": {
                "version": "1.1",
                "schema": "schemas/protocol/events/v1.1/event.schema.json",
                "fixture": "fixtures/protocol/events/v1.1/permission.requested.json"
            },
            "previous_minor": {
                "version": "0.9",
                "schema": "schemas/protocol/events/v0.9/event.schema.json",
                "fixture": "fixtures/protocol/events/v0.9/permission.requested.json"
            }
        }))
        .unwrap();
    assert!(validate_compatibility_manifest(&future_with_wrong_previous, "1.1", false).is_err());

    let future_with_previous: CompatibilityManifest = serde_json::from_value(serde_json::json!({
        "current": {
            "version": "1.1",
            "schema": "schemas/protocol/events/v1.1/event.schema.json",
            "fixture": "fixtures/protocol/events/v1.1/permission.requested.json"
        },
        "previous_minor": {
            "version": "1.0",
            "schema": "schemas/protocol/events/v1.0/event.schema.json",
            "fixture": "fixtures/protocol/events/v1.0/permission.requested.json"
        }
    }))
    .unwrap();
    validate_compatibility_manifest(&future_with_previous, "1.1", false)
        .expect("future manifest should accept the exact preceding minor");
}
