use crate::remote::models::{
    DeviceRecord, RemoteAccessStatus, RemoteDeviceStore, RemoteGatewayConfig,
};
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
pub fn list_remote_devices() -> Result<Vec<DeviceRecord>, String> {
    Ok(crate::remote::storage::load_device_store()?.devices)
}

fn revoke_device_in_store(
    mut store: RemoteDeviceStore,
    device_id: &str,
    revoked_at: &str,
) -> Result<RemoteDeviceStore, String> {
    let device = store
        .devices
        .iter_mut()
        .find(|device| device.device_id == device_id)
        .ok_or_else(|| "remote_device_not_found".to_string())?;
    device.revoked_at = Some(revoked_at.to_string());
    Ok(store)
}

#[tauri::command]
pub async fn revoke_remote_device(
    app: tauri::AppHandle,
    device_id: String,
) -> Result<Vec<DeviceRecord>, String> {
    let store = crate::remote::storage::load_device_store()?;
    let revoked_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let store = revoke_device_in_store(store, &device_id, &revoked_at)?;
    let saved = crate::remote::storage::save_device_store(&store)?;
    let state = app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    crate::remote::auth::revoke_sessions_for_device(&mut runtime, &device_id);
    Ok(saved.devices)
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

#[cfg(any(debug_assertions, test))]
fn millis_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(any(debug_assertions, test))]
fn debug_session_response_from_record(
    session: &crate::remote::models::RemoteSessionRecord,
) -> crate::remote::models::AuthSessionResponse {
    crate::remote::models::AuthSessionResponse {
        csrf_nonce: session.csrf_nonce.clone(),
        expires_at: millis_to_rfc3339(session.expires_at_ms),
        absolute_expires_at: millis_to_rfc3339(session.absolute_expires_at_ms),
    }
}

#[cfg(any(debug_assertions, test))]
fn create_debug_remote_session_record(
    device_id: &str,
    session_id: Option<&str>,
    now_ms: i64,
) -> Result<crate::remote::models::RemoteSessionRecord, String> {
    let device_id = device_id.trim();
    if device_id.is_empty() {
        return Err("remote_debug_device_required".to_string());
    }

    let mut session = crate::remote::auth::create_session_record(device_id, now_ms);
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        session.session_id = session_id.to_string();
    }
    Ok(session)
}

#[cfg(debug_assertions)]
#[tauri::command]
pub async fn debug_create_remote_session(
    app: tauri::AppHandle,
    device_id: String,
    session_id: Option<String>,
) -> Result<crate::remote::models::AuthSessionResponse, String> {
    if !cfg!(debug_assertions) {
        return Err("debug commands are disabled in production builds".to_string());
    }
    if std::env::var("WARDIAN_E2E").ok().as_deref() != Some("1") {
        return Err("debug remote sessions require WARDIAN_E2E=1".to_string());
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let session = create_debug_remote_session_record(&device_id, session_id.as_deref(), now_ms)?;
    let response = debug_session_response_from_record(&session);
    let state = app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    runtime.sessions.insert(session.session_id.clone(), session);
    Ok(response)
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

    #[test]
    fn revoke_device_in_store_marks_device_and_rejects_unknown_device() {
        let store = RemoteDeviceStore {
            schema_version: crate::remote::models::REMOTE_DEVICE_STORE_SCHEMA_VERSION,
            devices: vec![DeviceRecord {
                device_id: "dev-1".to_string(),
                label: "Phone".to_string(),
                public_key_spki_der_base64: "key".to_string(),
                public_key_fingerprint: "fp".to_string(),
                created_at: "2026-05-21T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            }],
        };

        let revoked = revoke_device_in_store(store.clone(), "dev-1", "2026-05-21T00:05:00.000Z")
            .expect("known device revoked");

        assert_eq!(
            revoked.devices[0].revoked_at.as_deref(),
            Some("2026-05-21T00:05:00.000Z")
        );
        assert_eq!(
            revoke_device_in_store(store, "missing", "2026-05-21T00:05:00.000Z"),
            Err("remote_device_not_found".to_string())
        );
    }

    #[test]
    fn debug_session_record_requires_device_and_uses_requested_session_id() {
        assert!(create_debug_remote_session_record(" ", Some("sess-1"), 1_000_000).is_err());

        let session = create_debug_remote_session_record("dev-1", Some("sess-1"), 1_000_000)
            .expect("session");

        assert_eq!(session.device_id, "dev-1");
        assert_eq!(session.session_id, "sess-1");
        assert!(!session.csrf_nonce.is_empty());
        assert!(session.expires_at_ms > 1_000_000);
    }
}
