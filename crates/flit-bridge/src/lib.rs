use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use flit_protocol::{CommandError, HealthStatus, PROTOCOL_VERSION, SystemHealthResponse};

static CORE: OnceLock<FoundationCore> = OnceLock::new();
static CORE_CONSTRUCTIONS: AtomicU64 = AtomicU64::new(0);

uniffi::setup_scaffolding!();

#[derive(Debug, Eq, PartialEq, thiserror::Error, uniffi::Error)]
pub enum BridgeError {
    #[error("the embedded Rust Core could not complete the request")]
    CoreFailure,
    #[error("the embedded Rust Core could not serialize the response")]
    SerializationFailure,
}

struct FoundationCore;

fn core() -> &'static FoundationCore {
    CORE.get_or_init(|| {
        CORE_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
        FoundationCore
    })
}

fn protect<T>(operation: impl FnOnce() -> Result<T, BridgeError>) -> Result<T, BridgeError> {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(Err(BridgeError::CoreFailure))
}

fn health_json(client_protocol_version: &str) -> Result<String, BridgeError> {
    let payload = if client_protocol_version == PROTOCOL_VERSION {
        serde_json::to_value(SystemHealthResponse {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            core: HealthStatus::Ready,
            storage: HealthStatus::NotConfigured,
            providers: HealthStatus::NotConfigured,
        })
    } else {
        serde_json::to_value(CommandError::protocol_mismatch())
    }
    .map_err(|_| BridgeError::SerializationFailure)?;

    serde_json::to_string(&payload).map_err(|_| BridgeError::SerializationFailure)
}

#[uniffi::export]
pub fn system_health_json(client_protocol_version: String) -> Result<String, BridgeError> {
    protect(|| {
        let _ = core();
        health_json(&client_protocol_version)
    })
}

#[uniffi::export]
pub fn core_construction_count() -> u64 {
    CORE_CONSTRUCTIONS.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use flit_protocol::SystemHealthRequest;

    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures/protocol/commands/v1.0")
            .join(name);
        serde_json::from_str(&fs::read_to_string(path).expect("health fixture should be readable"))
            .expect("health fixture should be valid JSON")
    }

    #[test]
    fn normal_and_mismatch_payloads_match_the_protocol_fixtures() {
        let request_fixture = fixture("system_health.request.json");
        let request: SystemHealthRequest = serde_json::from_value(request_fixture.clone())
            .expect("health request fixture should match the Rust contract");
        assert_eq!(
            serde_json::to_value(&request).expect("health request should serialize"),
            request_fixture
        );
        let normal: serde_json::Value = serde_json::from_str(
            &system_health_json(request.client_protocol_version)
                .expect("matching protocol should return health"),
        )
        .expect("normal bridge payload should be valid JSON");
        let mismatch: serde_json::Value = serde_json::from_str(
            &system_health_json("2.0".to_owned())
                .expect("protocol mismatch should return the typed command payload"),
        )
        .expect("mismatch bridge payload should be valid JSON");

        assert_eq!(normal, fixture("system_health.response.json"));
        assert_eq!(mismatch, fixture("protocol_mismatch.error.json"));
    }

    #[test]
    fn repeated_calls_share_one_core_construction() {
        for _ in 0..100 {
            system_health_json(PROTOCOL_VERSION.to_owned())
                .expect("health should remain available");
        }

        assert_eq!(core_construction_count(), 1);
    }

    #[test]
    fn panic_is_contained_and_does_not_poison_the_next_request() {
        let failure = protect::<()>(|| panic!("bridge panic control"));
        assert_eq!(failure, Err(BridgeError::CoreFailure));
        assert!(system_health_json(PROTOCOL_VERSION.to_owned()).is_ok());
    }
}
