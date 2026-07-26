#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{self, Command},
};

use flit_bridge::{initialize_core, provider_diagnostics_json, system_health_json};
use flit_protocol::{
    CapabilityStatus, CommandError, CommandErrorCode, HealthStatus, PROTOCOL_VERSION,
    ProviderCompatibility, ProviderDiagnosticsResponse, ProviderUnavailableReason,
    SystemHealthResponse,
};

const CHILD_MODE: &str = "FLIT_PROVIDER_DIAGNOSTICS_CHILD";
const CHILD_ROOT: &str = "FLIT_PROVIDER_DIAGNOSTICS_ROOT";

#[test]
fn generated_provider_diagnostics_are_path_only_bounded_and_truthful() {
    if let Some(mode) = env::var_os(CHILD_MODE) {
        run_child(&mode.to_string_lossy());
        return;
    }

    let root = fs::canonicalize(env::temp_dir())
        .expect("canonical temporary root")
        .join(format!("flit-provider-diagnostics-{}", process::id()));
    fs::create_dir(&root).expect("diagnostics test root");

    for mode in ["unknown", "missing"] {
        let mode_root = root.join(mode);
        let bin = mode_root.join("bin");
        fs::create_dir_all(&bin).expect("fake PATH directory");
        let marker = mode_root.join("probe-ran");
        if mode == "unknown" {
            write_fake_codex(&bin.join("codex"), &marker);
        }
        let output = Command::new(env::current_exe().expect("current test executable"))
            .arg("--exact")
            .arg("generated_provider_diagnostics_are_path_only_bounded_and_truthful")
            .arg("--nocapture")
            .env(CHILD_MODE, mode)
            .env(CHILD_ROOT, &mode_root)
            .env("PATH", &bin)
            .output()
            .expect("spawn isolated diagnostics child");
        assert!(
            output.status.success(),
            "diagnostics child {mode} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::remove_dir_all(root).expect("remove diagnostics test root");
}

fn run_child(mode: &str) {
    let root = PathBuf::from(env::var_os(CHILD_ROOT).expect("child root"));
    let marker = root.join("probe-ran");
    let data = root.join("data");

    let unavailable_before_initialization: CommandError = decode(
        provider_diagnostics_json(PROTOCOL_VERSION.to_owned())
            .expect("storage failure should use command error JSON"),
    );
    assert_eq!(
        unavailable_before_initialization,
        CommandError::for_code(CommandErrorCode::StorageUnavailable)
    );
    let stale_protocol: CommandError = decode(
        provider_diagnostics_json("1.1".to_owned())
            .expect("protocol mismatch should use command error JSON"),
    );
    assert_eq!(
        stale_protocol,
        CommandError::for_code(CommandErrorCode::ProtocolMismatch)
    );
    assert!(
        !marker.exists(),
        "rejected diagnostics must not execute Codex"
    );

    initialize_core(
        data.to_string_lossy().into_owned(),
        PROTOCOL_VERSION.to_owned(),
    )
    .expect("initialize Core");
    let before: SystemHealthResponse =
        decode(system_health_json(PROTOCOL_VERSION.to_owned()).expect("health before diagnostics"));
    assert_eq!(before.providers, HealthStatus::NotConfigured);

    let rendered =
        provider_diagnostics_json(PROTOCOL_VERSION.to_owned()).expect("run diagnostics command");
    assert!(rendered.len() < 65_536);
    for forbidden in [
        root.to_string_lossy().as_ref(),
        "stdout",
        "stderr",
        "sha256\":\"",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "diagnostics must not expose {forbidden}"
        );
    }
    let response: ProviderDiagnosticsResponse = decode(rendered);
    assert_eq!(response.capabilities.len(), 16);

    match mode {
        "unknown" => {
            assert_eq!(
                response,
                fixture("provider_diagnostics.unknown.response.json")
            );
            assert!(marker.is_file(), "the real fake executable must be probed");
            assert_eq!(response.compatibility, ProviderCompatibility::Unknown);
            assert_eq!(response.executable_version.as_deref(), Some("9.9.9"));
            assert_eq!(response.fingerprint_mismatches.len(), 8);
            assert_eq!(response.unavailable_reason, None);
            assert!(
                response
                    .capabilities
                    .iter()
                    .all(|entry| entry.status == CapabilityStatus::Unknown)
            );
        }
        "missing" => {
            assert_eq!(
                response,
                fixture("provider_diagnostics.unavailable.response.json")
            );
            assert!(!marker.exists());
            assert_eq!(response.compatibility, ProviderCompatibility::Unavailable);
            assert_eq!(response.executable_version, None);
            assert_eq!(
                response.unavailable_reason,
                Some(ProviderUnavailableReason::ExecutableNotFound)
            );
            assert!(
                response
                    .capabilities
                    .iter()
                    .all(|entry| entry.status == CapabilityStatus::Unavailable)
            );
        }
        other => panic!("unexpected child mode {other}"),
    }

    let after: SystemHealthResponse =
        decode(system_health_json(PROTOCOL_VERSION.to_owned()).expect("health after diagnostics"));
    assert_eq!(after.providers, HealthStatus::Unavailable);
}

fn write_fake_codex(path: &Path, marker: &Path) {
    assert!(
        !marker.to_string_lossy().contains('\''),
        "test marker must be safe for the shell fixture"
    );
    fs::write(
        path,
        format!(
            "#!/bin/sh\n\
             touch '{}'\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               printf 'codex-cli 9.9.9\\n'\n\
               exit 0\n\
             fi\n\
             printf '{{}}' > \"$5/codex_app_server_protocol.schemas.json\"\n\
             printf '{{}}' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
            marker.display()
        ),
    )
    .expect("write fake Codex");
    let mut permissions = fs::metadata(path)
        .expect("fake Codex metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("make fake Codex executable");
}

fn decode<T: serde::de::DeserializeOwned>(json: String) -> T {
    serde_json::from_str(&json).expect("typed diagnostics payload")
}

fn fixture(name: &str) -> ProviderDiagnosticsResponse {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(format!("fixtures/protocol/commands/v{PROTOCOL_VERSION}"))
        .join(name);
    serde_json::from_str(
        &fs::read_to_string(path).expect("provider diagnostics fixture should be readable"),
    )
    .expect("provider diagnostics fixture should match Rust type")
}
