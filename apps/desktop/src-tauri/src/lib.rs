use flit_protocol::{
    CommandError, HealthStatus, PROTOCOL_VERSION, SystemHealthRequest, SystemHealthResponse,
};
use tauri::{WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
fn system_health(request: SystemHealthRequest) -> Result<SystemHealthResponse, CommandError> {
    if request.client_protocol_version != PROTOCOL_VERSION {
        return Err(CommandError::protocol_mismatch());
    }

    Ok(SystemHealthResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        core: HealthStatus::Ready,
        storage: HealthStatus::NotConfigured,
        providers: HealthStatus::NotConfigured,
    })
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![system_health])
        .setup(|app| {
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Flit")
                    .inner_size(1280.0, 720.0)
                    .min_inner_size(720.0, 560.0)
                    .build()?;
            window.show()?;
            window.set_focus()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Flit could not start the desktop runtime");
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fixtures/protocol/commands/v1.0")
            .join(name);
        let contents = fs::read_to_string(path).expect("fixture should be readable");
        serde_json::from_str(&contents).expect("fixture should contain valid JSON")
    }

    #[test]
    fn reports_only_the_implemented_foundation_state() {
        let response = system_health(SystemHealthRequest {
            client_protocol_version: PROTOCOL_VERSION.to_owned(),
        })
        .expect("matching protocol should return health");

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.core, HealthStatus::Ready);
        assert_eq!(response.storage, HealthStatus::NotConfigured);
        assert_eq!(response.providers, HealthStatus::NotConfigured);
        assert_eq!(
            serde_json::to_value(response).expect("health should serialize"),
            fixture("system_health.response.json")
        );
    }

    #[test]
    fn rejects_a_client_protocol_mismatch_before_returning_health() {
        let error = system_health(SystemHealthRequest {
            client_protocol_version: "2.0".to_owned(),
        })
        .expect_err("mismatched protocol must fail closed");

        assert_eq!(error, CommandError::protocol_mismatch());
        assert_eq!(
            serde_json::to_value(error).expect("error should serialize"),
            fixture("protocol_mismatch.error.json")
        );
    }
}
