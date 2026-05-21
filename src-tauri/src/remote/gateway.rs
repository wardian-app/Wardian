use crate::remote::models::RemoteGatewayConfig;
use axum::{routing::get, Router};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tauri::AppHandle;

pub fn validate_gateway_bind_config(config: &RemoteGatewayConfig) -> Result<(), String> {
    crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)?;
    if !crate::remote::policy::is_loopback_bind_host(&config.loopback_host) {
        return Err("Remote gateway must bind to loopback in v1".to_string());
    }
    Ok(())
}

pub fn spawn_remote_gateway(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Some(config) = crate::remote::storage::load_remote_config().ok().flatten() else {
            return;
        };
        if !config.enabled {
            return;
        }
        if let Err(error) = run_remote_gateway(app, config).await {
            crate::utils::logging::log_debug(&format!(
                "[Wardian] remote gateway unavailable: {error}"
            ));
        }
    });
}

async fn run_remote_gateway(app: AppHandle, config: RemoteGatewayConfig) -> Result<(), String> {
    validate_gateway_bind_config(&config)?;
    let addr = loopback_socket_addr(&config.loopback_host, config.loopback_port)?;
    let router = remote_router(app, config);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| error.to_string())?;
    axum::serve(listener, router)
        .await
        .map_err(|error| error.to_string())
}

fn loopback_socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    match host.trim() {
        "127.0.0.1" | "localhost" => Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
        "::1" => Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)),
        _ => Err("Remote gateway must bind to loopback in v1".to_string()),
    }
}

fn remote_router(app: AppHandle, config: RemoteGatewayConfig) -> Router {
    Router::new()
        .route("/remote/api/health", get(remote_health))
        .with_state(RemoteGatewayContext {
            _app: app,
            _config: config,
        })
}

#[derive(Clone)]
struct RemoteGatewayContext {
    _app: AppHandle,
    _config: RemoteGatewayConfig,
}

async fn remote_health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{RemoteGatewayConfig, REMOTE_SETTINGS_SCHEMA_VERSION};

    fn config() -> RemoteGatewayConfig {
        RemoteGatewayConfig {
            schema_version: REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://wardian.tailnet.ts.net".to_string(),
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 0,
            gateway_identity_public_key: "pub".to_string(),
            gateway_identity_fingerprint: "fp".to_string(),
        }
    }

    #[test]
    fn gateway_refuses_non_loopback_bind_host() {
        let mut cfg = config();
        cfg.loopback_host = "0.0.0.0".to_string();
        assert!(validate_gateway_bind_config(&cfg).is_err());
    }

    #[test]
    fn gateway_accepts_loopback_bind_host() {
        assert!(validate_gateway_bind_config(&config()).is_ok());
    }
}
