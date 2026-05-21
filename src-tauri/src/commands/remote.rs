use crate::remote::models::{RemoteAccessStatus, RemoteGatewayConfig};
use tauri::Manager;

fn canonicalized_remote_gateway_config(
    config: &RemoteGatewayConfig,
) -> Result<RemoteGatewayConfig, String> {
    let mut normalized = config.clone();
    if !config.enabled {
        return Ok(normalized);
    }
    let origin = crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)?;
    normalized.canonical_origin = origin.raw().to_string();
    if !crate::remote::policy::is_loopback_bind_host(&config.loopback_host) {
        return Err("Remote gateway must bind to loopback in v1".to_string());
    }
    Ok(normalized)
}

fn validate_remote_gateway_config(config: &RemoteGatewayConfig) -> Result<(), String> {
    canonicalized_remote_gateway_config(config).map(|_| ())
}

fn remote_access_status_for_config(config: Option<&RemoteGatewayConfig>) -> RemoteAccessStatus {
    match config {
        Some(config) if config.enabled && validate_remote_gateway_config(config).is_ok() => {
            RemoteAccessStatus::Enabled
        }
        Some(config) if config.enabled => RemoteAccessStatus::NeedsRepair,
        _ => RemoteAccessStatus::Disabled,
    }
}

#[tauri::command]
pub fn load_remote_access_status() -> Result<RemoteAccessStatus, String> {
    let config = crate::remote::storage::load_remote_config()?;
    Ok(remote_access_status_for_config(config.as_ref()))
}

#[tauri::command]
pub fn load_remote_gateway_config() -> Result<Option<RemoteGatewayConfig>, String> {
    crate::remote::storage::load_remote_config()
}

#[tauri::command]
pub fn save_remote_gateway_config(
    config: RemoteGatewayConfig,
) -> Result<RemoteGatewayConfig, String> {
    let config = canonicalized_remote_gateway_config(&config)?;
    crate::remote::storage::save_remote_config(&config)
}

#[tauri::command]
pub async fn create_remote_pairing_offer(
    app: tauri::AppHandle,
) -> Result<crate::remote::models::PairingQrPayload, String> {
    let config =
        crate::remote::storage::load_remote_config()?.ok_or("Remote access is not configured")?;
    if !config.enabled {
        return Err("Remote access is disabled".to_string());
    }
    validate_remote_gateway_config(&config)?;
    let origin = crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    crate::remote::auth::create_pairing_offer(
        &mut runtime,
        origin.raw(),
        &config.gateway_identity_fingerprint,
        now_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::REMOTE_SETTINGS_SCHEMA_VERSION;

    fn enabled_config(origin: &str, host: &str) -> RemoteGatewayConfig {
        RemoteGatewayConfig {
            schema_version: REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: origin.to_string(),
            loopback_host: host.to_string(),
            loopback_port: 41241,
            gateway_identity_public_key: "pub".to_string(),
            gateway_identity_fingerprint: "fp".to_string(),
        }
    }

    #[test]
    fn enabled_status_requires_valid_https_origin_and_loopback_host() {
        assert_eq!(
            remote_access_status_for_config(Some(&enabled_config(
                "https://wardian.tailnet.ts.net",
                "127.0.0.1",
            ))),
            RemoteAccessStatus::Enabled
        );
        assert_eq!(
            remote_access_status_for_config(Some(&enabled_config(
                "http://wardian.tailnet.ts.net",
                "127.0.0.1",
            ))),
            RemoteAccessStatus::NeedsRepair
        );
        assert_eq!(
            remote_access_status_for_config(Some(&enabled_config(
                "https://wardian.tailnet.ts.net",
                "0.0.0.0",
            ))),
            RemoteAccessStatus::NeedsRepair
        );
    }

    #[test]
    fn saving_enabled_config_rejects_invalid_remote_boundary() {
        assert!(validate_remote_gateway_config(&enabled_config(
            "http://wardian.tailnet.ts.net",
            "127.0.0.1",
        ))
        .is_err());
        assert!(validate_remote_gateway_config(&enabled_config(
            "https://wardian.tailnet.ts.net",
            "0.0.0.0",
        ))
        .is_err());
    }

    #[test]
    fn saving_enabled_config_persists_canonical_origin() {
        let temp = tempfile::tempdir().expect("temp dir");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };

        let saved = save_remote_gateway_config(enabled_config(
            " https://wardian.tailnet.ts.net/ ",
            "127.0.0.1",
        ))
        .expect("save config");
        let loaded = crate::remote::storage::load_remote_config_at(temp.path())
            .expect("load config")
            .expect("stored config");

        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }

        assert_eq!(saved.canonical_origin, "https://wardian.tailnet.ts.net");
        assert_eq!(loaded.canonical_origin, "https://wardian.tailnet.ts.net");
    }
}
