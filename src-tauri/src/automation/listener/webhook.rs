//! The inbound webhook server.
//!
//! Deliberately separate from the remote gateway. That server is opt-in,
//! loopback-bound, and gated on P-256 device pairing; a webhook sender can
//! perform none of that handshake, and a receiver that only worked when remote
//! access happened to be enabled would be a coupling nobody could reason
//! about. Different trust domain, different server.

use super::launch::{self, FireOutcome, ListenerFire};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde_json::{Map, Value};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tauri::AppHandle;
use wardian_core::listeners::{
    self, secrets, webhook as webhook_rules, AutomationListener, ListenerTrigger, WebhookTrigger,
    MAX_WEBHOOK_MAX_BODY_BYTES,
};

/// Substrings that mark a header as credential-bearing.
const CREDENTIAL_MARKERS: &[&str] = &[
    "auth",
    "signature",
    "token",
    "secret",
    "credential",
    "password",
    "cookie",
    "-key",
    "apikey",
    "bearer",
];

/// Whether a header must be kept out of a run payload.
///
/// The payload becomes `trigger.output` and therefore reaches an agent prompt
/// and a durable run log, so publishing a signature or bearer token there would
/// leak the listener's own secret.
///
/// `configured_signature` is the header this listener actually authenticates
/// with. A name-shape heuristic cannot be trusted to cover it, because a sender
/// may use `X-Webhook-Auth` or any other name, so the configured carrier is
/// removed by exact match regardless of what it is called.
fn is_credential_header(name: &str, configured_signature: Option<&str>) -> bool {
    let lower = name.to_ascii_lowercase();
    if configured_signature
        .map(str::trim)
        .filter(|configured| !configured.is_empty())
        .is_some_and(|configured| configured.eq_ignore_ascii_case(name))
    {
        return true;
    }
    if lower == webhook_rules::DEFAULT_SIGNATURE_HEADER || lower == webhook_rules::TOKEN_HEADER {
        return true;
    }
    CREDENTIAL_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn payload(
    listener: &AutomationListener,
    trigger: &WebhookTrigger,
    delivery_id: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("listener_id".into(), Value::String(listener.id.clone()));
    map.insert("listener_name".into(), Value::String(listener.name.clone()));
    map.insert("trigger_type".into(), Value::String("webhook".into()));
    map.insert("delivery_id".into(), Value::String(delivery_id.to_string()));

    let signature_header = trigger.signature_header.as_deref();
    let safe_headers: Map<String, Value> = headers
        .iter()
        .filter(|(name, _)| !is_credential_header(name, signature_header))
        .map(|(name, value)| (name.to_ascii_lowercase(), Value::String(value.clone())))
        .collect();
    map.insert("headers".into(), Value::Object(safe_headers));

    let text = String::from_utf8(body.to_vec()).ok();
    map.insert(
        "body".into(),
        text.as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .unwrap_or(Value::Null),
    );
    map.insert(
        "body_text".into(),
        text.map(Value::String).unwrap_or(Value::Null),
    );
    map.insert(
        "observed_at".into(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    map
}

fn find_listener(segment: &str) -> Option<(AutomationListener, WebhookTrigger)> {
    listeners::load_listeners()
        .into_iter()
        .find_map(|listener| {
            if !listener.should_arm() {
                return None;
            }
            match &listener.trigger {
                ListenerTrigger::Webhook(trigger)
                    if trigger.path_segment.eq_ignore_ascii_case(segment.trim()) =>
                {
                    let trigger = trigger.clone();
                    Some((listener, trigger))
                }
                _ => None,
            }
        })
}

async fn receive(
    State(app): State<AppHandle>,
    Path(segment): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // An unknown path answers exactly like a wrong secret would, so an
    // unauthenticated peer cannot enumerate which listeners exist.
    let Some((listener, trigger)) = find_listener(&segment) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found" })),
        );
    };

    let Some(secret) = secrets::load_secret(&listener.id).and_then(|stored| stored.webhook_secret)
    else {
        launch::record_rejection(
            &listener.id,
            "listener has no stored secret, so no delivery can be authenticated".to_string(),
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "listener is not configured" })),
        );
    };

    let pairs = header_pairs(&headers);
    if let Err(rejection) = webhook_rules::authenticate(&trigger, &secret, &pairs, &body) {
        launch::record_rejection(&listener.id, rejection.reason());
        let status = match rejection {
            webhook_rules::WebhookRejection::BodyTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::UNAUTHORIZED,
        };
        launch::emit_listeners_updated(&app);
        return (status, Json(serde_json::json!({ "error": "rejected" })));
    }

    let delivery_id = webhook_rules::delivery_id(&pairs, &body);
    let fire = ListenerFire {
        event_identity: delivery_id.clone(),
        payload: payload(&listener, &trigger, &delivery_id, &pairs, &body),
    };

    // Awaited rather than spawned: the response means "durably claimed", so it
    // cannot be sent before the run exists. Waiting for the run to *finish*
    // would instead hold the sender open for the length of an agent session
    // and guarantee a timeout-and-retry.
    let outcome = launch::fire(app.clone(), listener, fire).await;

    // `202 Accepted` is reserved for a delivery that produced a durable run, so
    // the code never promises processing that will not happen. A retry landing
    // on an already-claimed delivery is one of those: answering with an error
    // would drive an infinite retry for a request Wardian handled correctly.
    //
    // Overlap outcomes get `200 OK` instead. The delivery was received and then
    // deliberately dropped or superseded by the listener's own policy, so a
    // retry would be pointless, but claiming durable acceptance would be false.
    // A launch failure is Wardian's fault, so it is reported as retryable.
    let status = match outcome {
        FireOutcome::Started(_) | FireOutcome::AlreadyClaimed => StatusCode::ACCEPTED,
        FireOutcome::Skipped | FireOutcome::Coalesced => StatusCode::OK,
        FireOutcome::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        FireOutcome::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let described = match &outcome {
        FireOutcome::Started(run_id) => {
            serde_json::json!({ "outcome": "started", "durable": true, "run_id": run_id })
        }
        FireOutcome::AlreadyClaimed => {
            serde_json::json!({ "outcome": "already_accepted", "durable": true })
        }
        FireOutcome::Skipped => serde_json::json!({ "outcome": "skipped", "durable": false }),
        FireOutcome::Coalesced => serde_json::json!({ "outcome": "coalesced", "durable": false }),
        FireOutcome::RateLimited => {
            serde_json::json!({ "outcome": "rate_limited", "durable": false })
        }
        // The reason stays out of the response: an authenticated sender still
        // does not need Wardian's internal failure detail.
        FireOutcome::Failed(_) => serde_json::json!({ "outcome": "failed", "durable": false }),
    };
    (status, Json(described))
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

fn router(app: AppHandle) -> Router {
    Router::new()
        .route("/hooks/{segment}", post(receive))
        .route("/hooks/health", axum::routing::get(health))
        // A hard ceiling above every per-listener cap, so an oversized body is
        // refused by the server before a handler ever buffers it.
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_MAX_BODY_BYTES as usize))
        .with_state(app)
}

fn socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    match host.trim() {
        "127.0.0.1" | "localhost" => Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
        "::1" => Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)),
        other => Err(format!(
            "webhook gateway must bind to loopback; `{other}` is not a loopback host"
        )),
    }
}

/// Start the webhook server, returning its task handle and the bound address.
///
/// Binding is attempted synchronously so a port conflict surfaces as an arming
/// error on the webhook listeners rather than as silence.
pub async fn serve(
    app: AppHandle,
) -> Result<(tauri::async_runtime::JoinHandle<()>, SocketAddr), String> {
    let config = webhook_rules::load_gateway_config();
    webhook_rules::validate_gateway_config(&config)?;
    let addr = socket_addr(&config.host, config.port)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| format!("could not bind webhook gateway on {addr}: {error}"))?;
    let bound = listener
        .local_addr()
        .map_err(|error| format!("could not read webhook gateway address: {error}"))?;
    let router = router(app);
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            crate::utils::logging::log_debug(&format!(
                "[automation] webhook gateway stopped: {error}"
            ));
        }
    });
    Ok((handle, bound))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::listeners::{WebhookAuth, DEFAULT_WEBHOOK_MAX_BODY_BYTES};

    fn listener() -> AutomationListener {
        AutomationListener {
            id: "hook".into(),
            blueprint_id: "audit".into(),
            name: "CI".into(),
            enabled: true,
            trigger: ListenerTrigger::Webhook(WebhookTrigger {
                path_segment: "ci".into(),
                auth: WebhookAuth::HmacSha256,
                signature_header: None,
                max_body_bytes: DEFAULT_WEBHOOK_MAX_BODY_BYTES,
            }),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: Default::default(),
            assignments: Default::default(),
            overlap: None,
            runtime: Default::default(),
        }
    }

    fn webhook_trigger(signature_header: Option<&str>) -> WebhookTrigger {
        WebhookTrigger {
            path_segment: "ci".into(),
            auth: WebhookAuth::HmacSha256,
            signature_header: signature_header.map(str::to_string),
            max_body_bytes: DEFAULT_WEBHOOK_MAX_BODY_BYTES,
        }
    }

    #[test]
    fn credential_headers_never_reach_the_run_payload() {
        let headers = vec![
            ("X-GitHub-Event".to_string(), "release".to_string()),
            (
                "X-Hub-Signature-256".to_string(),
                "sha256=deadbeef".to_string(),
            ),
            ("Authorization".to_string(), "Bearer hunter2".to_string()),
            ("Proxy-Authorization".to_string(), "Basic abc".to_string()),
            (
                "X-Forwarded-Authorization".to_string(),
                "Basic abc".to_string(),
            ),
            ("X-Wardian-Token".to_string(), "s3cret".to_string()),
            ("X-Api-Key".to_string(), "abc".to_string()),
            ("X-ApiKey".to_string(), "abc".to_string()),
            ("X-Credential".to_string(), "abc".to_string()),
            ("Cookie".to_string(), "session=1".to_string()),
            ("Set-Cookie".to_string(), "session=1".to_string()),
        ];
        let map = payload(
            &listener(),
            &webhook_trigger(None),
            "delivery-1",
            &headers,
            b"{}",
        );
        let carried = map["headers"].as_object().expect("headers object");

        assert!(carried.contains_key("x-github-event"));
        for leaked in [
            "x-hub-signature-256",
            "authorization",
            "proxy-authorization",
            "x-forwarded-authorization",
            "x-wardian-token",
            "x-api-key",
            "x-apikey",
            "x-credential",
            "cookie",
            "set-cookie",
        ] {
            assert!(
                !carried.contains_key(leaked),
                "{leaked} must not reach an agent prompt or a run log"
            );
        }
    }

    #[test]
    fn a_custom_signature_header_is_removed_because_it_is_the_configured_carrier() {
        // `X-Delivery-Proof` matches no name-shape heuristic, so only knowing
        // which header this listener authenticates with keeps its value out of
        // the payload.
        let trigger = webhook_trigger(Some("X-Delivery-Proof"));
        let headers = vec![
            (
                "X-Delivery-Proof".to_string(),
                "sha256=deadbeef".to_string(),
            ),
            ("X-GitHub-Event".to_string(), "release".to_string()),
        ];
        let map = payload(&listener(), &trigger, "delivery-1", &headers, b"{}");
        let carried = map["headers"].as_object().expect("headers object");

        assert!(!carried.contains_key("x-delivery-proof"));
        assert!(carried.contains_key("x-github-event"));
    }

    #[test]
    fn the_default_signature_header_is_removed_even_when_none_is_configured() {
        let headers = vec![(
            "X-Hub-Signature-256".to_string(),
            "sha256=deadbeef".to_string(),
        )];
        let map = payload(
            &listener(),
            &webhook_trigger(None),
            "delivery-1",
            &headers,
            b"{}",
        );
        assert!(map["headers"]
            .as_object()
            .expect("headers object")
            .is_empty());
    }

    #[test]
    fn a_json_body_is_parsed_and_the_raw_text_is_kept_beside_it() {
        let map = payload(
            &listener(),
            &webhook_trigger(None),
            "delivery-1",
            &[],
            br#"{"action":"published","number":7}"#,
        );
        assert_eq!(map["body"]["action"], "published");
        assert_eq!(map["body"]["number"], serde_json::json!(7));
        assert!(map["body_text"]
            .as_str()
            .expect("text")
            .contains("published"));
    }

    #[test]
    fn a_non_json_body_leaves_the_parsed_field_null_without_losing_the_text() {
        let map = payload(
            &listener(),
            &webhook_trigger(None),
            "delivery-1",
            &[],
            b"plain text hook",
        );
        assert_eq!(map["body"], Value::Null);
        assert_eq!(map["body_text"], "plain text hook");
    }

    #[test]
    fn a_binary_body_reports_neither_parsed_nor_text() {
        let map = payload(
            &listener(),
            &webhook_trigger(None),
            "delivery-1",
            &[],
            &[0xff, 0xfe, 0x00],
        );
        assert_eq!(map["body"], Value::Null);
        assert_eq!(map["body_text"], Value::Null);
    }

    #[test]
    fn only_loopback_addresses_resolve() {
        assert!(socket_addr("127.0.0.1", 8787).is_ok());
        assert!(socket_addr("::1", 8787).is_ok());
        assert!(socket_addr("0.0.0.0", 8787).is_err());
        assert!(socket_addr("example.com", 8787).is_err());
    }

    #[test]
    fn credential_header_detection_covers_common_carriers() {
        for name in [
            "Authorization",
            "cookie",
            "X-Hub-Signature-256",
            "x-wardian-token",
            "X-Api-Key",
            "X-Secret-Thing",
            "X-Webhook-Auth",
            "X-Credential",
        ] {
            assert!(
                is_credential_header(name, None),
                "{name} should be filtered"
            );
        }
        for name in ["X-GitHub-Event", "Content-Type", "User-Agent"] {
            assert!(
                !is_credential_header(name, None),
                "{name} should be carried"
            );
        }
    }
}
