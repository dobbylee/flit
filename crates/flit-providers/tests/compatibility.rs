use std::{collections::BTreeSet, path::PathBuf};

use flit_providers::{
    CapabilityEntry, CapabilityStatus, FingerprintAxis, ProviderCapability, ProviderCompatibility,
    ProviderFingerprint, classify_codex, validated_codex_0_144_6_fingerprint,
};

#[test]
fn exact_validated_codex_fingerprint_exposes_only_the_recorded_capability_matrix() {
    use CapabilityStatus::{Degraded, Supported, Unsupported};
    use ProviderCapability::{
        CompletionDetect, ContinueAfterQuit, History, Launch, ListManaged, OpenInProvider,
        PermissionDetect, PermissionPolicyConfigure, PermissionPolicyObserve, PermissionRespond,
        QuestionDetect, QuestionRespond, Reconcile, Resume, Stop, StructuredActivity,
    };

    let frozen_s0_1_fingerprint = ProviderFingerprint {
        canonical_executable: PathBuf::from(
            "/opt/homebrew/Caskroom/codex/0.144.6/codex-aarch64-apple-darwin",
        ),
        executable_version: "0.144.6".to_owned(),
        executable_sha256: "80a3933d11a9d13ef806aa24f7bb8afc9169cfe4e9b09d6da6a92922cbde9cff"
            .to_owned(),
        combined_schema_sha256: "85ea836927d6cfdd3c68a9bda17dba48d2573bbc282ab2d5775a5005e40bc9c3"
            .to_owned(),
        v2_schema_sha256: "8928c45789c653017f967b59035b0bf802648d3259d328c1b7b37a8191b177ca"
            .to_owned(),
        method_allowlist_sha256: "eceb94d9e824065899efeebcbe191a772458b7330e26b15c9f91604103153ba2"
            .to_owned(),
        fixture_sha256: "a3debd88e389320edf899c0a3399accca500bd6d5632c6862d5ac2c12ad73f8b"
            .to_owned(),
        smoke_run_id: "2026-07-21-arm64-3ff2583".to_owned(),
    };
    assert_eq!(
        validated_codex_0_144_6_fingerprint(),
        frozen_s0_1_fingerprint
    );
    let snapshot = classify_codex(&frozen_s0_1_fingerprint);
    assert_eq!(snapshot.compatibility, ProviderCompatibility::Supported);
    assert!(snapshot.fingerprint_mismatches.is_empty());
    assert_eq!(
        snapshot.capabilities,
        [
            (Launch, Supported),
            (ListManaged, Supported),
            (Resume, Supported),
            (Reconcile, Supported),
            (StructuredActivity, Degraded),
            (PermissionDetect, Degraded),
            (PermissionRespond, Unsupported),
            (PermissionPolicyConfigure, Unsupported),
            (PermissionPolicyObserve, Unsupported),
            (QuestionDetect, Supported),
            (QuestionRespond, Degraded),
            (CompletionDetect, Supported),
            (History, Unsupported),
            (OpenInProvider, Unsupported),
            (ContinueAfterQuit, Unsupported),
            (Stop, Supported),
        ]
        .map(|(capability, status)| CapabilityEntry { capability, status })
    );
    assert!(snapshot.status(StructuredActivity).is_available());
    assert!(!snapshot.status(PermissionRespond).is_available());
}

#[test]
fn every_single_fingerprint_mismatch_disables_all_capabilities() {
    let cases = [
        (
            FingerprintAxis::CanonicalExecutable,
            mutate_fingerprint(|fingerprint| {
                fingerprint.canonical_executable =
                    "/opt/homebrew/Caskroom/codex/other/codex".into();
            }),
        ),
        (
            FingerprintAxis::ExecutableVersion,
            mutate_fingerprint(|fingerprint| {
                fingerprint.executable_version = "0.144.7".to_owned();
            }),
        ),
        (
            FingerprintAxis::ExecutableSha256,
            mutate_fingerprint(|fingerprint| {
                fingerprint.executable_sha256 = other_digest();
            }),
        ),
        (
            FingerprintAxis::CombinedSchemaSha256,
            mutate_fingerprint(|fingerprint| {
                fingerprint.combined_schema_sha256 = other_digest();
            }),
        ),
        (
            FingerprintAxis::V2SchemaSha256,
            mutate_fingerprint(|fingerprint| {
                fingerprint.v2_schema_sha256 = other_digest();
            }),
        ),
        (
            FingerprintAxis::MethodAllowlistSha256,
            mutate_fingerprint(|fingerprint| {
                fingerprint.method_allowlist_sha256 = other_digest();
            }),
        ),
        (
            FingerprintAxis::FixtureSha256,
            mutate_fingerprint(|fingerprint| {
                fingerprint.fixture_sha256 = other_digest();
            }),
        ),
        (
            FingerprintAxis::SmokeRunId,
            mutate_fingerprint(|fingerprint| {
                fingerprint.smoke_run_id = "different-smoke".to_owned();
            }),
        ),
    ];

    for (axis, fingerprint) in cases {
        let snapshot = classify_codex(&fingerprint);
        assert_eq!(snapshot.compatibility, ProviderCompatibility::Unknown);
        assert_eq!(snapshot.fingerprint_mismatches, [axis]);
        assert!(!snapshot.has_available_capability());
        assert!(
            snapshot
                .capabilities
                .iter()
                .all(|entry| entry.status == CapabilityStatus::Unknown)
        );
    }
}

#[test]
fn capability_snapshot_is_exhaustive_unique_and_deterministically_ordered() {
    let snapshot = classify_codex(&validated_codex_0_144_6_fingerprint());
    assert_eq!(
        snapshot
            .capabilities
            .iter()
            .map(|entry| entry.capability)
            .collect::<Vec<_>>(),
        ProviderCapability::ALL
    );
    assert_eq!(
        snapshot
            .capabilities
            .iter()
            .map(|entry| entry.capability)
            .collect::<BTreeSet<_>>()
            .len(),
        ProviderCapability::ALL.len()
    );
}

fn mutate_fingerprint(
    mutation: impl FnOnce(&mut flit_providers::ProviderFingerprint),
) -> flit_providers::ProviderFingerprint {
    let mut fingerprint = validated_codex_0_144_6_fingerprint();
    mutation(&mut fingerprint);
    fingerprint
}

fn other_digest() -> String {
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned()
}
