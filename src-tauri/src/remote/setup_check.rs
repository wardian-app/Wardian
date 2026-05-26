use crate::remote::models::{
    RemoteGatewayConfig, RemoteSetupCheck, RemoteSetupCheckResult, RemoteSetupCheckStatus,
    RemoteSetupCommandHint, RemoteSetupOverallStatus,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Deserialize)]
struct TailscaleStatusJson {
    #[serde(rename = "Self")]
    self_node: Option<TailscaleSelfNode>,
}

#[derive(Debug, Deserialize)]
struct TailscaleSelfNode {
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
    #[serde(rename = "Online")]
    online: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TailscaleServeStatusJson {
    #[serde(rename = "Web")]
    web: Option<HashMap<String, TailscaleServeWebConfig>>,
}

#[derive(Debug, Deserialize)]
struct TailscaleServeWebConfig {
    #[serde(rename = "Handlers")]
    handlers: Option<HashMap<String, TailscaleServeHandler>>,
}

#[derive(Debug, Deserialize)]
struct TailscaleServeHandler {
    #[serde(rename = "Proxy")]
    proxy: Option<String>,
}

pub async fn load_remote_setup_check() -> RemoteSetupCheckResult {
    let config = match crate::remote::storage::load_remote_config() {
        Ok(config) => config,
        Err(error) => {
            return result(
                RemoteSetupOverallStatus::NeedsAction,
                vec![check(
                    "wardian_config",
                    "Wardian remote config",
                    RemoteSetupCheckStatus::Error,
                    "Wardian could not load remote access settings.",
                    Some(error),
                )],
                None,
                None,
                None,
            );
        }
    };

    let Some(config) = config else {
        return result(
            RemoteSetupOverallStatus::Disabled,
            vec![check(
                "wardian_config",
                "Wardian remote config",
                RemoteSetupCheckStatus::Warning,
                "Remote access is disabled.",
                None,
            )],
            None,
            None,
            None,
        );
    };

    if !config.enabled {
        return result(
            RemoteSetupOverallStatus::Disabled,
            vec![check(
                "wardian_config",
                "Wardian remote config",
                RemoteSetupCheckStatus::Warning,
                "Remote access is disabled.",
                None,
            )],
            None,
            None,
            Some(setup_command(&config)),
        );
    }

    let mut checks = vec![check(
        "wardian_config",
        "Wardian remote config",
        RemoteSetupCheckStatus::Ok,
        "Remote access is enabled in Wardian.",
        Some(format!("{}:{}", config.loopback_host, config.loopback_port)),
    )];

    checks.push(local_gateway_check(&config).await);

    let status_output = run_tailscale_json(&["status", "--json"]).await;
    let inferred_origin = match status_output {
        Ok(json) => {
            checks.push(check(
                "tailscale_cli",
                "Tailscale CLI",
                RemoteSetupCheckStatus::Ok,
                "Wardian can run the Tailscale CLI.",
                None,
            ));
            tailscale_status_check(&json, &mut checks)
        }
        Err(error) => {
            checks.push(check(
                "tailscale_cli",
                "Tailscale CLI",
                RemoteSetupCheckStatus::Error,
                "Wardian could not run the Tailscale CLI.",
                Some(error),
            ));
            None
        }
    };

    let serve_output = run_tailscale_json(&["serve", "status", "--json"]).await;
    let serve_target = match serve_output {
        Ok(json) => tailscale_serve_check(&json, &config, &mut checks),
        Err(error) => {
            checks.push(check(
                "tailscale_serve",
                "Tailscale Serve",
                RemoteSetupCheckStatus::Error,
                "No matching Tailscale Serve forwarding rule was detected.",
                Some(error),
            ));
            None
        }
    };

    checks.push(https_gateway_check(&config).await);

    let overall_status = if checks
        .iter()
        .any(|entry| entry.status != RemoteSetupCheckStatus::Ok)
    {
        RemoteSetupOverallStatus::NeedsAction
    } else {
        RemoteSetupOverallStatus::Ready
    };

    result(
        overall_status,
        checks,
        inferred_origin,
        serve_target,
        Some(setup_command(&config)),
    )
}

async fn run_tailscale_json(args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("tailscale");
    command.args(args);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(TAILSCALE_TIMEOUT, command.output())
        .await
        .map_err(|_| "tailscale command timed out".to_string())?
        .map_err(|error| error.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("tailscale exited with {}", output.status)
        } else {
            stderr
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn tailscale_status_check(json: &str, checks: &mut Vec<RemoteSetupCheck>) -> Option<String> {
    let parsed = serde_json::from_str::<TailscaleStatusJson>(json);
    let Ok(parsed) = parsed else {
        checks.push(check(
            "tailscale_login",
            "Tailscale login",
            RemoteSetupCheckStatus::Warning,
            "Tailscale is installed, but Wardian could not read its status.",
            None,
        ));
        return None;
    };

    let Some(self_node) = parsed.self_node else {
        checks.push(check(
            "tailscale_login",
            "Tailscale login",
            RemoteSetupCheckStatus::Error,
            "This desktop is not signed into Tailscale.",
            None,
        ));
        return None;
    };

    if self_node.online == Some(false) {
        checks.push(check(
            "tailscale_login",
            "Tailscale login",
            RemoteSetupCheckStatus::Error,
            "This desktop is signed into Tailscale but currently offline.",
            None,
        ));
    } else {
        checks.push(check(
            "tailscale_login",
            "Tailscale login",
            RemoteSetupCheckStatus::Ok,
            "This desktop is signed into Tailscale.",
            None,
        ));
    }

    let origin = self_node
        .dns_name
        .map(|dns_name| format!("https://{}", dns_name.trim_end_matches('.')));

    if origin.is_some() {
        checks.push(check(
            "tailscale_https_origin",
            "Tailscale HTTPS origin",
            RemoteSetupCheckStatus::Ok,
            "Wardian detected this machine's Tailscale DNS name.",
            origin.clone(),
        ));
    } else {
        checks.push(check(
            "tailscale_https_origin",
            "Tailscale HTTPS origin",
            RemoteSetupCheckStatus::Warning,
            "Wardian could not detect this machine's Tailscale DNS name.",
            None,
        ));
    }

    origin
}

fn tailscale_serve_check(
    json: &str,
    config: &RemoteGatewayConfig,
    checks: &mut Vec<RemoteSetupCheck>,
) -> Option<String> {
    let parsed = serde_json::from_str::<TailscaleServeStatusJson>(json);
    let expected = format!("http://{}:{}", config.loopback_host, config.loopback_port);
    let Ok(parsed) = parsed else {
        checks.push(check(
            "tailscale_serve",
            "Tailscale Serve",
            RemoteSetupCheckStatus::Warning,
            "Wardian could not parse Tailscale Serve status.",
            None,
        ));
        return None;
    };

    let mut detected = None;
    if let Some(web) = parsed.web {
        for (_host, web_config) in web {
            if let Some(handlers) = web_config.handlers {
                for (_path, handler) in handlers {
                    if let Some(proxy) = handler.proxy {
                        if proxy.trim_end_matches('/') == expected {
                            detected = Some(proxy);
                        }
                    }
                }
            }
        }
    }

    if let Some(target) = detected.clone() {
        checks.push(check(
            "tailscale_serve",
            "Tailscale Serve",
            RemoteSetupCheckStatus::Ok,
            "Tailscale Serve forwards HTTPS traffic to Wardian's local gateway.",
            Some(target),
        ));
    } else {
        checks.push(check(
            "tailscale_serve",
            "Tailscale Serve",
            RemoteSetupCheckStatus::Error,
            "Tailscale Serve is not forwarding to Wardian's configured gateway port.",
            Some(expected),
        ));
    }

    detected
}

async fn local_gateway_check(config: &RemoteGatewayConfig) -> RemoteSetupCheck {
    let url = format!(
        "http://{}:{}/remote/api/health",
        config.loopback_host, config.loopback_port
    );
    probe_url(
        "local_gateway",
        "Wardian local gateway",
        &url,
        "Wardian's local gateway is responding.",
        "Wardian's local gateway is not responding on the configured port.",
    )
    .await
}

async fn https_gateway_check(config: &RemoteGatewayConfig) -> RemoteSetupCheck {
    let url = format!(
        "{}/remote/api/health",
        config.canonical_origin.trim_end_matches('/')
    );
    probe_url(
        "https_gateway",
        "HTTPS remote gateway",
        &url,
        "The HTTPS remote gateway is reachable.",
        "The HTTPS remote gateway is not reachable from this desktop.",
    )
    .await
}

async fn probe_url(
    id: &str,
    label: &str,
    url: &str,
    ok_message: &str,
    error_message: &str,
) -> RemoteSetupCheck {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            return check(
                id,
                label,
                RemoteSetupCheckStatus::Warning,
                error_message,
                Some(error.to_string()),
            );
        }
    };

    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => check(
            id,
            label,
            RemoteSetupCheckStatus::Ok,
            ok_message,
            Some(url.to_string()),
        ),
        Ok(response) => check(
            id,
            label,
            RemoteSetupCheckStatus::Error,
            error_message,
            Some(format!("{url} returned {}", response.status())),
        ),
        Err(error) => check(
            id,
            label,
            RemoteSetupCheckStatus::Error,
            error_message,
            Some(error.to_string()),
        ),
    }
}

fn setup_command(config: &RemoteGatewayConfig) -> RemoteSetupCommandHint {
    RemoteSetupCommandHint {
        label: "Configure Tailscale Serve".to_string(),
        command: format!(
            "tailscale serve --bg --https=443 http://{}:{}",
            config.loopback_host, config.loopback_port
        ),
    }
}

fn check(
    id: impl Into<String>,
    label: impl Into<String>,
    status: RemoteSetupCheckStatus,
    message: impl Into<String>,
    details: Option<String>,
) -> RemoteSetupCheck {
    RemoteSetupCheck {
        id: id.into(),
        label: label.into(),
        status,
        message: message.into(),
        details,
    }
}

fn result(
    overall_status: RemoteSetupOverallStatus,
    checks: Vec<RemoteSetupCheck>,
    inferred_origin: Option<String>,
    serve_target: Option<String>,
    setup_command: Option<RemoteSetupCommandHint>,
) -> RemoteSetupCheckResult {
    RemoteSetupCheckResult {
        overall_status,
        checks,
        inferred_origin,
        serve_target,
        setup_command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::REMOTE_SETTINGS_SCHEMA_VERSION;

    fn config() -> RemoteGatewayConfig {
        RemoteGatewayConfig {
            schema_version: REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://inanna.tailb6e29a.ts.net".to_string(),
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 41241,
            gateway_identity_public_key: "pub".to_string(),
            gateway_identity_fingerprint: "fp".to_string(),
        }
    }

    #[test]
    fn status_json_infers_https_origin() {
        let mut checks = Vec::new();
        let origin = tailscale_status_check(
            r#"{"Self":{"DNSName":"inanna.tailb6e29a.ts.net.","Online":true}}"#,
            &mut checks,
        );

        assert_eq!(origin.as_deref(), Some("https://inanna.tailb6e29a.ts.net"));
        assert!(checks.iter().any(
            |check| check.id == "tailscale_login" && check.status == RemoteSetupCheckStatus::Ok
        ));
        assert!(checks
            .iter()
            .any(|check| check.id == "tailscale_https_origin"
                && check.status == RemoteSetupCheckStatus::Ok));
    }

    #[test]
    fn serve_json_detects_matching_loopback_proxy() {
        let mut checks = Vec::new();
        let target = tailscale_serve_check(
            r#"{"Web":{"inanna.tailb6e29a.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:41241"}}}}}"#,
            &config(),
            &mut checks,
        );

        assert_eq!(target.as_deref(), Some("http://127.0.0.1:41241"));
        assert!(checks.iter().any(
            |check| check.id == "tailscale_serve" && check.status == RemoteSetupCheckStatus::Ok
        ));
    }

    #[test]
    fn serve_json_flags_wrong_port() {
        let mut checks = Vec::new();
        let target = tailscale_serve_check(
            r#"{"Web":{"inanna.tailb6e29a.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9999"}}}}}"#,
            &config(),
            &mut checks,
        );

        assert_eq!(target, None);
        assert!(checks
            .iter()
            .any(|check| check.id == "tailscale_serve"
                && check.status == RemoteSetupCheckStatus::Error));
    }
}
