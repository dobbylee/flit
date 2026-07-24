use std::{
    error::Error,
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
};

use crate::{
    CodexSchemaProbeError, CodexVersionProbeError, ExecutableInspection, ExecutableInspectionError,
    ProviderCapabilitySnapshot, ProviderFingerprint, classify_codex, inspect_codex_at,
    inspect_codex_on_path, probe_codex_schema, probe_codex_version,
    profile::codex_0_144_6_bundled_evidence, validated_codex_0_144_6_fingerprint,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRuntimeFingerprint {
    pub canonical_executable: PathBuf,
    pub executable_version: String,
    pub executable_sha256: String,
    pub combined_schema_sha256: String,
    pub v2_schema_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexCompatibilityProbe {
    pub runtime_fingerprint: CodexRuntimeFingerprint,
    pub validated_profile: Option<ProviderFingerprint>,
    pub capability_snapshot: ProviderCapabilitySnapshot,
    pub version_stderr_bytes: usize,
    pub schema_stdout_bytes: usize,
    pub schema_stderr_bytes: usize,
}

pub fn probe_codex_compatibility_at(
    path: impl AsRef<Path>,
) -> Result<CodexCompatibilityProbe, CodexCompatibilityProbeError> {
    let inspection = inspect_codex_at(path).map_err(CodexCompatibilityProbeError::Inspection)?;
    probe_codex_compatibility(&inspection)
}

pub fn probe_codex_compatibility_on_path(
    path_environment: Option<&OsStr>,
) -> Result<CodexCompatibilityProbe, CodexCompatibilityProbeError> {
    let inspection = inspect_codex_on_path(path_environment)
        .map_err(CodexCompatibilityProbeError::Inspection)?;
    probe_codex_compatibility(&inspection)
}

fn probe_codex_compatibility(
    inspection: &ExecutableInspection,
) -> Result<CodexCompatibilityProbe, CodexCompatibilityProbeError> {
    let version = probe_codex_version(inspection).map_err(CodexCompatibilityProbeError::Version)?;
    let schema = probe_codex_schema(inspection).map_err(CodexCompatibilityProbeError::Schema)?;
    let evidence = codex_0_144_6_bundled_evidence();
    let runtime_fingerprint = CodexRuntimeFingerprint {
        canonical_executable: inspection.canonical_path.clone(),
        executable_version: version.executable_version,
        executable_sha256: inspection.sha256.clone(),
        combined_schema_sha256: schema.combined_schema_sha256,
        v2_schema_sha256: schema.v2_schema_sha256,
    };
    let (validated_profile, capability_snapshot) =
        classify_runtime_fingerprint(&runtime_fingerprint, evidence)?;

    Ok(CodexCompatibilityProbe {
        runtime_fingerprint,
        validated_profile,
        capability_snapshot,
        version_stderr_bytes: version.stderr_bytes,
        schema_stdout_bytes: schema.stdout_bytes,
        schema_stderr_bytes: schema.stderr_bytes,
    })
}

fn classify_runtime_fingerprint(
    runtime_fingerprint: &CodexRuntimeFingerprint,
    evidence: crate::profile::BundledProfileEvidence,
) -> Result<(Option<ProviderFingerprint>, ProviderCapabilitySnapshot), CodexCompatibilityProbeError>
{
    let expected = validated_codex_0_144_6_fingerprint();
    if evidence.method_allowlist_sha256 != expected.method_allowlist_sha256
        || evidence.fixture_sha256 != expected.fixture_sha256
        || evidence.smoke_run_id != expected.smoke_run_id
    {
        return Err(CodexCompatibilityProbeError::BundledEvidenceMismatch);
    }
    let runtime_matches = runtime_fingerprint.canonical_executable == expected.canonical_executable
        && runtime_fingerprint.executable_version == expected.executable_version
        && runtime_fingerprint.executable_sha256 == expected.executable_sha256
        && runtime_fingerprint.combined_schema_sha256 == expected.combined_schema_sha256
        && runtime_fingerprint.v2_schema_sha256 == expected.v2_schema_sha256;
    let validated_profile = runtime_matches.then_some(expected);
    let classification_input = validated_profile
        .clone()
        .unwrap_or_else(|| ProviderFingerprint {
            canonical_executable: runtime_fingerprint.canonical_executable.clone(),
            executable_version: runtime_fingerprint.executable_version.clone(),
            executable_sha256: runtime_fingerprint.executable_sha256.clone(),
            combined_schema_sha256: runtime_fingerprint.combined_schema_sha256.clone(),
            v2_schema_sha256: runtime_fingerprint.v2_schema_sha256.clone(),
            method_allowlist_sha256: String::new(),
            fixture_sha256: String::new(),
            smoke_run_id: String::new(),
        });
    let capability_snapshot = classify_codex(&classification_input);
    Ok((validated_profile, capability_snapshot))
}

#[derive(Debug)]
pub enum CodexCompatibilityProbeError {
    Inspection(ExecutableInspectionError),
    Version(CodexVersionProbeError),
    Schema(CodexSchemaProbeError),
    BundledEvidenceMismatch,
}

impl fmt::Display for CodexCompatibilityProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inspection(error) => write!(formatter, "Codex inspection failed: {error}"),
            Self::Version(error) => write!(formatter, "Codex version probe failed: {error}"),
            Self::Schema(error) => write!(formatter, "Codex schema probe failed: {error}"),
            Self::BundledEvidenceMismatch => formatter
                .write_str("bundled Codex profile evidence did not match its frozen digest"),
        }
    }
}

impl Error for CodexCompatibilityProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspection(error) => Some(error),
            Self::Version(error) => Some(error),
            Self::Schema(error) => Some(error),
            Self::BundledEvidenceMismatch => None,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use crate::{
        CapabilityStatus, CodexCompatibilityProbeError, FingerprintAxis, ProviderCapability,
        ProviderCompatibility, inspect_codex_at,
    };

    use super::{
        CodexRuntimeFingerprint, classify_runtime_fingerprint, probe_codex_compatibility_at,
        probe_codex_compatibility_on_path,
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "flit-provider-complete-probe-{label}-{}-{nonce}",
                process::id()
            ));
            fs::create_dir(&path).expect("test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn live_axis_mismatches_return_one_exhaustive_unknown_snapshot() {
        let directory = TestDirectory::new("unknown");
        let executable = directory.0.join("codex");
        write_script(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then\n  printf 'codex-cli 9.9.9\\n'\n  exit 0\nfi\nprintf '{}' > \"$5/codex_app_server_protocol.schemas.json\"\nprintf '{}' > \"$5/codex_app_server_protocol.v2.schemas.json\"\n",
        );
        let inspection = inspect_codex_at(&executable).expect("inspection");

        let report = probe_codex_compatibility_at(&executable).expect("complete probe");
        assert_eq!(
            report.capability_snapshot.compatibility,
            ProviderCompatibility::Unknown
        );
        assert_eq!(
            report.capability_snapshot.fingerprint_mismatches,
            [
                FingerprintAxis::CanonicalExecutable,
                FingerprintAxis::ExecutableVersion,
                FingerprintAxis::ExecutableSha256,
                FingerprintAxis::CombinedSchemaSha256,
                FingerprintAxis::V2SchemaSha256,
                FingerprintAxis::MethodAllowlistSha256,
                FingerprintAxis::FixtureSha256,
                FingerprintAxis::SmokeRunId,
            ]
        );
        assert!(report.validated_profile.is_none());
        assert_eq!(
            report.runtime_fingerprint.canonical_executable,
            inspection.canonical_path
        );
        assert_eq!(report.runtime_fingerprint.executable_version, "9.9.9");
        assert_eq!(
            report.runtime_fingerprint.executable_sha256,
            inspection.sha256
        );
        assert_eq!(report.version_stderr_bytes, 0);
        assert_eq!(report.schema_stdout_bytes, 0);
        assert_eq!(report.schema_stderr_bytes, 0);
        assert_eq!(
            report.capability_snapshot.capabilities.len(),
            ProviderCapability::ALL.len()
        );
        assert!(report.capability_snapshot.capabilities.iter().all(|entry| {
            entry.status == CapabilityStatus::Unknown && !entry.status.is_available()
        }));

        let path_environment =
            std::env::join_paths([directory.0.as_path()]).expect("PATH environment");
        let path_report = probe_codex_compatibility_on_path(Some(&path_environment))
            .expect("PATH complete probe");
        assert_eq!(path_report.runtime_fingerprint, report.runtime_fingerprint);
        assert_eq!(path_report.capability_snapshot, report.capability_snapshot);
    }

    #[test]
    fn version_or_schema_failure_never_returns_a_partial_snapshot() {
        let directory = TestDirectory::new("failures");
        let bad_version = directory.0.join("bad-version");
        write_script(
            &bad_version,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'invalid\\n'; exit 0; fi\nexit 9\n",
        );
        assert!(matches!(
            probe_codex_compatibility_at(&bad_version),
            Err(CodexCompatibilityProbeError::Version(_))
        ));

        let bad_schema = directory.0.join("bad-schema");
        write_script(
            &bad_schema,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'codex-cli 9.9.9\\n'; exit 0; fi\nexit 9\n",
        );
        let error = probe_codex_compatibility_at(&bad_schema)
            .expect_err("schema failure must not return a report");
        assert!(
            matches!(error, CodexCompatibilityProbeError::Schema(_)),
            "{error:?}"
        );
    }

    #[test]
    fn public_entrypoints_wrap_inspection_failures_without_a_report() {
        assert!(matches!(
            probe_codex_compatibility_at("relative/codex"),
            Err(CodexCompatibilityProbeError::Inspection(_))
        ));
        assert!(matches!(
            probe_codex_compatibility_on_path(None),
            Err(CodexCompatibilityProbeError::Inspection(_))
        ));
    }

    #[test]
    fn exact_runtime_axes_activate_only_the_matching_bundled_profile() {
        let expected = crate::validated_codex_0_144_6_fingerprint();
        let runtime = CodexRuntimeFingerprint {
            canonical_executable: expected.canonical_executable.clone(),
            executable_version: expected.executable_version.clone(),
            executable_sha256: expected.executable_sha256.clone(),
            combined_schema_sha256: expected.combined_schema_sha256.clone(),
            v2_schema_sha256: expected.v2_schema_sha256.clone(),
        };
        let (validated_profile, snapshot) = classify_runtime_fingerprint(
            &runtime,
            crate::profile::codex_0_144_6_bundled_evidence(),
        )
        .expect("matching profile");
        assert_eq!(validated_profile, Some(expected));
        assert_eq!(snapshot.compatibility, ProviderCompatibility::Supported);
        assert!(snapshot.fingerprint_mismatches.is_empty());
    }

    #[test]
    fn bundled_evidence_drift_fails_before_classification() {
        let expected = crate::validated_codex_0_144_6_fingerprint();
        let runtime = CodexRuntimeFingerprint {
            canonical_executable: expected.canonical_executable,
            executable_version: expected.executable_version,
            executable_sha256: expected.executable_sha256,
            combined_schema_sha256: expected.combined_schema_sha256,
            v2_schema_sha256: expected.v2_schema_sha256,
        };
        let mut evidence = crate::profile::codex_0_144_6_bundled_evidence();
        evidence.fixture_sha256 = "0".repeat(64);
        assert!(matches!(
            classify_runtime_fingerprint(&runtime, evidence),
            Err(CodexCompatibilityProbeError::BundledEvidenceMismatch)
        ));
    }

    fn write_script(path: &std::path::Path, script: &str) {
        fs::write(path, script).expect("write script");
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("permissions");
    }
}
