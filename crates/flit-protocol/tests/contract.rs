use std::{fs, path::PathBuf};

use flit_protocol::{CommandError, PROTOCOL_VERSION, SystemHealthRequest, SystemHealthResponse};
use serde::{Serialize, de::DeserializeOwned};

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
