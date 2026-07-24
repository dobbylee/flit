use sha2::{Digest, Sha256};

const CODEX_0_144_6_METHOD_ALLOWLIST: &[u8] =
    include_bytes!("../fixtures/codex-0.144.6-method-allowlist.txt");
const CODEX_0_144_6_CONTRACT_FIXTURES: &[u8] =
    include_bytes!("../fixtures/codex-0.144.6-contract.jsonl");
pub(crate) const CODEX_0_144_6_SMOKE_RUN_ID: &str = "2026-07-21-arm64-3ff2583";

pub(crate) struct BundledProfileEvidence {
    pub method_allowlist_sha256: String,
    pub fixture_sha256: String,
    pub smoke_run_id: String,
}

pub(crate) fn codex_0_144_6_bundled_evidence() -> BundledProfileEvidence {
    BundledProfileEvidence {
        method_allowlist_sha256: sha256(CODEX_0_144_6_METHOD_ALLOWLIST),
        fixture_sha256: sha256(CODEX_0_144_6_CONTRACT_FIXTURES),
        smoke_run_id: CODEX_0_144_6_SMOKE_RUN_ID.to_owned(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        CODEX_0_144_6_CONTRACT_FIXTURES, CODEX_0_144_6_METHOD_ALLOWLIST,
        CODEX_0_144_6_SMOKE_RUN_ID, codex_0_144_6_bundled_evidence,
    };

    #[test]
    fn bundled_profile_evidence_matches_the_frozen_s0_1_digests() {
        let evidence = codex_0_144_6_bundled_evidence();
        assert_eq!(
            evidence.method_allowlist_sha256,
            "eceb94d9e824065899efeebcbe191a772458b7330e26b15c9f91604103153ba2"
        );
        assert_eq!(
            evidence.fixture_sha256,
            "a3debd88e389320edf899c0a3399accca500bd6d5632c6862d5ac2c12ad73f8b"
        );
        assert_eq!(evidence.smoke_run_id, CODEX_0_144_6_SMOKE_RUN_ID);
    }

    #[test]
    fn bundled_allowlist_is_sorted_unique_and_every_fixture_line_is_valid_json() {
        let allowlist = std::str::from_utf8(CODEX_0_144_6_METHOD_ALLOWLIST)
            .expect("allowlist must be UTF-8")
            .lines()
            .collect::<Vec<_>>();
        assert_eq!(
            allowlist.iter().copied().collect::<BTreeSet<_>>().len(),
            allowlist.len()
        );
        assert!(allowlist.windows(2).all(|pair| pair[0] < pair[1]));

        let fixtures =
            std::str::from_utf8(CODEX_0_144_6_CONTRACT_FIXTURES).expect("fixtures must be UTF-8");
        let mut fixture_names = BTreeSet::new();
        for line in fixtures.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON fixture");
            let name = value["fixture"].as_str().expect("fixture name");
            assert!(
                fixture_names.insert(name.to_owned()),
                "duplicate fixture: {name}"
            );
            assert!(value["capability"].is_string());
            assert!(value["direction"].is_string());
            assert!(value["expected"].is_object());
        }
        assert_eq!(fixture_names.len(), 23);
    }
}
