use crate::remote::models::{
    AuthChallengeRequest, AuthChallengeResponse, AuthSessionRequest, AuthSessionResponse,
    DeviceCredentialKind, DeviceRecord, PairingSubmitRequest, PairingSubmitResponse,
    PendingPairingDecision, PendingPairingRequestRecord, RemoteAgentActionRequest,
    RemoteAuditRecord, RemoteClientAuthChallengeResponse, RemoteClientAuthSessionResponse,
    RemoteClientMessageOperation, RemoteClientMessageSubmitRequest,
    RemoteClientPairingSubmitRequest, RemoteClientPairingSubmitResponse, RemoteClientScope,
    RemoteClientSessionRecord, RemoteClientSharingStatus, RemoteCredentialAudience,
    RemoteGatewayConfig, RemoteInboxActionRequest, RemoteReadGrant, RemoteSendGrant,
    RemoteSessionRecord, RemoteWebSocketTicketRequest, RemoteWebSocketTicketResponse,
    REMOTE_AUDIT_SCHEMA_VERSION,
};
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Json, Path as AxumPath, Query, RawQuery, State,
    },
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Router,
};
use base64::Engine;
use p256::ecdsa::VerifyingKey;
use p256::pkcs8::DecodePublicKey;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tower::ServiceExt;

const REMOTE_SESSION_COOKIE_NAME: &str = "__Host-wardian_remote_session";
const REMOTE_CSRF_HEADER_NAME: &str = "x-wardian-csrf";
const REMOTE_STATUS_STREAM_NAME: &str = "agent_status";
const WEBSOCKET_FIRST_TICKET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const REMOTE_TERMINAL_DEFAULT_TAIL_BYTES: usize = 64 * 1024;
const REMOTE_TERMINAL_MAX_TAIL_BYTES: usize = 128 * 1024;

#[derive(serde::Deserialize)]
struct ClientPairingStatusPath {
    request_id: String,
}

#[derive(serde::Deserialize)]
struct ClientAgentSessionPath {
    session_id: String,
}

pub fn validate_gateway_bind_config(config: &RemoteGatewayConfig) -> Result<(), String> {
    crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)?;
    if !crate::remote::policy::is_loopback_bind_host(&config.loopback_host) {
        return Err("Remote gateway must bind to loopback in v1".to_string());
    }
    Ok(())
}

pub fn spawn_remote_gateway(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::state::AppState>();
        let _config_guard = state.remote_gateway_config_lock.lock().await;
        if crate::remote::client_messages::recover_interrupted_operations().is_err() {
            crate::utils::logging::log_debug(
                "[Wardian] remote client message recovery unavailable; uncertain operations remain durable",
            );
            return;
        }
        let operations = match crate::remote::client_messages::admitted_operations_for_recovery() {
            Ok(operations) => operations,
            Err(_) => {
                crate::utils::logging::log_debug(
                    "[Wardian] remote client message outbox unavailable; gateway startup deferred",
                );
                return;
            }
        };
        for operation in operations {
            schedule_client_message_dispatch(&app, operation);
        }
        let Some(config) = crate::remote::storage::load_remote_config().ok().flatten() else {
            return;
        };
        if let Err(error) = configure_remote_gateway(app.clone(), config).await {
            crate::utils::logging::log_debug(&format!(
                "[Wardian] remote gateway unavailable: {error}"
            ));
        }
    });
}

/// Caller holds `remote_gateway_config_lock` through persistence and binding.
pub async fn configure_remote_gateway(
    app: AppHandle,
    config: RemoteGatewayConfig,
) -> Result<(), String> {
    if config.enabled {
        validate_gateway_bind_config(&config)?;
    }
    let state = app.state::<crate::state::AppState>();
    // Disabled settings still close the running listener, even if their saved
    // bind host is invalid. It is never used for a bind in that case.
    let addr = if config.enabled {
        loopback_socket_addr(&config.loopback_host, config.loopback_port)?
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
    };
    let router = remote_router(app.clone(), config.clone());
    state.remote_listener.configure(config, addr, router).await
}

fn loopback_socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    match host.trim() {
        "127.0.0.1" | "localhost" => Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
        "::1" => Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)),
        _ => Err("Remote gateway must bind to loopback in v1".to_string()),
    }
}

fn remote_router(app: AppHandle, config: RemoteGatewayConfig) -> Router {
    let context = RemoteGatewayContext { app, config };
    let pwa_router = Router::new()
        .route("/remote", get(serve_remote_shell))
        .route("/remote/", get(serve_remote_shell))
        .route("/manifest.webmanifest", get(serve_manifest))
        .route("/remote-sw.js", get(serve_remote_sw))
        .route("/icon.png", get(serve_icon))
        .route("/icon-maskable.png", get(serve_maskable_icon))
        .route("/assets/{*asset_path}", get(serve_asset_path))
        .route("/remote/api/health", get(remote_health))
        .route("/remote/api/session", get(current_remote_session))
        .route("/remote/api/pairing/submit", post(submit_pairing))
        .route("/remote/api/pairing/{request_id}", get(pairing_status))
        .route("/remote/api/auth/challenge", post(create_auth_challenge))
        .route("/remote/api/auth/session", post(create_auth_session))
        .route("/remote/api/agents", get(list_remote_agents))
        .route("/remote/api/queue", get(load_remote_queue))
        .route("/remote/api/queue/action", post(run_inbox_action))
        .route("/remote/api/watchlists", get(load_remote_watchlists))
        .route("/remote/api/automations", get(list_remote_automations))
        .route(
            "/remote/api/automations/monitor",
            get(load_remote_automation_monitor),
        )
        .route(
            "/remote/api/agents/{session_id}/chat",
            get(load_remote_agent_chat),
        )
        .route(
            "/remote/api/agents/{session_id}/terminal",
            get(load_remote_agent_terminal),
        )
        .route(
            "/remote/api/agents/{session_id}/terminal-stream",
            get(terminal_stream_upgrade),
        )
        .route("/remote/api/agents/action", post(run_agent_action))
        .route("/remote/api/ws-ticket", post(create_ws_ticket))
        .route("/remote/api/status-stream", get(status_stream_upgrade))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(context.clone());
    pwa_router.merge(client_namespace_router(context))
}

struct ClientNamespaceState<R: Runtime> {
    context: RemoteGatewayContext<R>,
    routes: Router,
}

impl<R: Runtime> Clone for ClientNamespaceState<R> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            routes: self.routes.clone(),
        }
    }
}

fn client_namespace_router<R: Runtime>(context: RemoteGatewayContext<R>) -> Router {
    let routes = Router::new()
        .route(
            "/remote/client/v1/pairing/submit",
            post(client_submit_pairing),
        )
        .route(
            "/remote/client/v1/pairing/{request_id}",
            get(client_pairing_status),
        )
        .route(
            "/remote/client/v1/auth/challenge",
            post(create_client_auth_challenge),
        )
        .route(
            "/remote/client/v1/auth/token",
            post(create_client_auth_token),
        )
        .route("/remote/client/v1/sharing", get(client_sharing_status))
        .route("/remote/client/v1/agents", get(list_client_agents))
        .route(
            "/remote/client/v1/agents/{session_id}/chat",
            get(load_client_agent_chat),
        )
        .route(
            "/remote/client/v1/agents/{session_id}/messages",
            post(submit_client_agent_message).layer(DefaultBodyLimit::max(512 * 1024)),
        )
        .route(
            "/remote/client/v1/message-operations/by-key",
            get(load_client_message_operation),
        )
        .fallback(client_namespace_not_found)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(context.clone());
    let state = ClientNamespaceState { context, routes };
    Router::new()
        .route("/remote/client/v1", any(dispatch_client_namespace))
        .route("/remote/client/v1/", any(dispatch_client_namespace))
        .route("/remote/client/v1/{*path}", any(dispatch_client_namespace))
        .with_state(state)
}

async fn dispatch_client_namespace<R: Runtime>(
    State(namespace): State<ClientNamespaceState<R>>,
    request: Request<Body>,
) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let current_config = tokio::task::spawn_blocking(crate::remote::storage::load_remote_config)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let allowed_origin = current_config.as_ref().and_then(|config| {
        origin
            .as_deref()
            .filter(|origin| {
                crate::remote::policy::is_approved_client_origin(
                    &config.client_origin_allowlist,
                    origin,
                )
            })
            .map(str::to_string)
    });
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let host_matches = current_config.as_ref().is_some_and(|config| {
        let Ok(canonical) = crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)
        else {
            return false;
        };
        config.enabled
            && config.canonical_origin == namespace.context.config.canonical_origin
            && config.gateway_identity_fingerprint
                == namespace.context.config.gateway_identity_fingerprint
            && config.gateway_identity_public_key
                == namespace.context.config.gateway_identity_public_key
            && host.is_some_and(|host| host.eq_ignore_ascii_case(canonical.host()))
    });

    if request.method() == Method::OPTIONS {
        let response = client_preflight_response(
            request.uri().path(),
            request.headers(),
            allowed_origin.as_deref(),
            host_matches,
        );
        return finish_client_response(response, allowed_origin.as_deref()).await;
    }
    let gateway_binding_matches = current_config.as_ref().is_some_and(|config| {
        config.enabled
            && config.canonical_origin == namespace.context.config.canonical_origin
            && config.gateway_identity_fingerprint
                == namespace.context.config.gateway_identity_fingerprint
            && config.gateway_identity_public_key
                == namespace.context.config.gateway_identity_public_key
    });
    if !gateway_binding_matches {
        let state = namespace.context.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        crate::remote::auth::invalidate_all_client_runtime_authority(&mut runtime);
    } else if allowed_origin.is_none()
        && origin.as_deref().is_some_and(|origin| {
            crate::remote::policy::is_approved_client_origin(
                &namespace.context.config.client_origin_allowlist,
                origin,
            )
        })
    {
        if let Some(origin) = origin.as_deref() {
            let state = namespace.context.app.state::<crate::state::AppState>();
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::invalidate_client_origin_authority(&mut runtime, origin);
        }
    }
    let Some(origin) = allowed_origin.as_deref() else {
        return finish_client_response(
            client_error_response(StatusCode::FORBIDDEN, "client_origin_forbidden"),
            None,
        )
        .await;
    };
    if !host_matches {
        return finish_client_response(
            client_error_response(StatusCode::FORBIDDEN, "gateway_host_forbidden"),
            Some(origin),
        )
        .await;
    }

    let response = match namespace.routes.clone().oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    finish_client_response(response, Some(origin)).await
}

fn client_preflight_response(
    path: &str,
    headers: &HeaderMap,
    allowed_origin: Option<&str>,
    host_matches: bool,
) -> Response {
    let Some(origin) = allowed_origin else {
        return client_error_response(StatusCode::FORBIDDEN, "client_origin_forbidden");
    };
    if !host_matches {
        return client_error_response(StatusCode::FORBIDDEN, "gateway_host_forbidden");
    }
    let requested_method = headers
        .get("access-control-request-method")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Method::from_bytes(value.as_bytes()).ok());
    let Some(method) = requested_method else {
        return client_error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if !client_namespace_path_registered(path) {
        return client_error_response(StatusCode::NOT_FOUND, "not_found");
    }
    if client_namespace_method(path, &method).is_none() {
        return client_error_response(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    }
    if let Some(value) = headers
        .get("access-control-request-headers")
        .and_then(|value| value.to_str().ok())
    {
        for name in value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let lower = name.to_ascii_lowercase();
            if lower != "authorization" && lower != "content-type" && lower != "idempotency-key" {
                return client_error_response(StatusCode::BAD_REQUEST, "invalid_request");
            }
        }
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin).expect("validated origin is a header value"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Authorization, Content-Type, Idempotency-Key"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("300"),
    );
    response.headers_mut().insert(
        header::VARY,
        HeaderValue::from_static(
            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
        ),
    );
    response
}

fn client_namespace_method(path: &str, method: &Method) -> Option<&'static str> {
    let expected_method = match path {
        "/remote/client/v1/pairing/submit"
        | "/remote/client/v1/auth/challenge"
        | "/remote/client/v1/auth/token" => Some("POST"),
        _ if client_agent_messages_path(path) => Some("POST"),
        "/remote/client/v1/sharing" | "/remote/client/v1/agents" => Some("GET"),
        "/remote/client/v1/message-operations/by-key" => Some("GET"),
        _ if client_agent_chat_path(path) => Some("GET"),
        _ if client_pairing_status_path(path) => Some("GET"),
        _ => None,
    };
    expected_method.filter(|expected| method.as_str() == *expected)
}

fn client_namespace_path_registered(path: &str) -> bool {
    matches!(
        path,
        "/remote/client/v1/pairing/submit"
            | "/remote/client/v1/auth/challenge"
            | "/remote/client/v1/auth/token"
            | "/remote/client/v1/sharing"
            | "/remote/client/v1/agents"
            | "/remote/client/v1/message-operations/by-key"
    ) || client_pairing_status_path(path)
        || client_agent_chat_path(path)
        || client_agent_messages_path(path)
}

fn client_pairing_status_path(path: &str) -> bool {
    path.strip_prefix("/remote/client/v1/pairing/")
        .is_some_and(|request_id| {
            !request_id.is_empty() && request_id != "submit" && !request_id.contains('/')
        })
}

fn client_agent_chat_path(path: &str) -> bool {
    let Some(path) = path.strip_prefix("/remote/client/v1/agents/") else {
        return false;
    };
    let Some(session_id) = path.strip_suffix("/chat") else {
        return false;
    };
    !session_id.is_empty() && !session_id.contains('/')
}

fn client_agent_messages_path(path: &str) -> bool {
    let Some(path) = path.strip_prefix("/remote/client/v1/agents/") else {
        return false;
    };
    let Some(session_id) = path.strip_suffix("/messages") else {
        return false;
    };
    !session_id.is_empty() && !session_id.contains('/')
}

fn client_error_response(status: StatusCode, code: impl Into<String>) -> Response {
    let (status, code) = normalize_client_error(status, &code.into());
    let mut response = (status, axum::Json(serde_json::json!({ "error": code }))).into_response();
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    }
    response
}

async fn finish_client_response(mut response: Response, allowed_origin: Option<&str>) -> Response {
    if response.status().is_client_error() || response.status().is_server_error() {
        let status = response.status();
        let body = std::mem::replace(response.body_mut(), Body::empty());
        let code = to_bytes(body, 16 * 1024)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| {
                value
                    .get("error")
                    .or_else(|| value.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(String::new);
        let retry_after = response.headers().get(header::RETRY_AFTER).cloned();
        response = client_error_response(status, code);
        if let Some(retry_after) = retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, retry_after);
        }
    }
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.remove(header::SET_COOKIE);
    headers.remove(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
    if let Some(origin) = allowed_origin {
        if let Ok(origin) = HeaderValue::from_str(origin) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            let existing_vary = headers
                .get(header::VARY)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !existing_vary
                .split(',')
                .any(|item| item.trim().eq_ignore_ascii_case("origin"))
            {
                let value = if existing_vary.trim().is_empty() {
                    "Origin".to_string()
                } else {
                    format!("{existing_vary}, Origin")
                };
                if let Ok(value) = HeaderValue::from_str(&value) {
                    headers.insert(header::VARY, value);
                }
            }
        }
    }
    response
}

fn normalize_client_error(status: StatusCode, code: &str) -> (StatusCode, &'static str) {
    match code {
        "client_origin_forbidden" | "gateway_host_forbidden" => {
            (StatusCode::FORBIDDEN, "origin_forbidden")
        }
        "remote_device_revoked" | "device_revoked" => (StatusCode::FORBIDDEN, "device_revoked"),
        "client_token_required" | "auth_required" => (StatusCode::UNAUTHORIZED, "auth_required"),
        "client_token_expired" | "token_expired" => (StatusCode::UNAUTHORIZED, "token_expired"),
        "client_token_invalid" | "invalid_token" => (StatusCode::UNAUTHORIZED, "auth_required"),
        "invalid_auth_signature" => (StatusCode::UNAUTHORIZED, "invalid_auth_signature"),
        "auth_challenge_expired" | "challenge_expired" => {
            (StatusCode::UNAUTHORIZED, "challenge_expired")
        }
        "auth_challenge_not_found"
        | "auth_challenge_used"
        | "auth_challenge_mismatch"
        | "auth_challenge_invalid"
        | "invalid_challenge" => (StatusCode::UNAUTHORIZED, "invalid_challenge"),
        "device_not_found" | "remote_device_not_found" => (StatusCode::NOT_FOUND, "not_found"),
        "origin_forbidden" => (StatusCode::FORBIDDEN, "origin_forbidden"),
        "client_session_capacity"
        | "device_store_failed"
        | "remote_config_unavailable"
        | "remote_access_disabled"
        | "device_store_publish_unknown" => {
            (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable")
        }
        "rate_limited" => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        "pairing_offer_conflict" | "pairing_request_conflict" => (StatusCode::CONFLICT, "conflict"),
        "invalid_request" | "invalid_pairing_material" => {
            (StatusCode::BAD_REQUEST, "invalid_request")
        }
        "not_found" => (StatusCode::NOT_FOUND, "not_found"),
        "method_not_allowed" => (StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
        "payload_too_large" | "request_too_large" => {
            (StatusCode::PAYLOAD_TOO_LARGE, "request_too_large")
        }
        "conflict" => (StatusCode::CONFLICT, "conflict"),
        "idempotency_namespace_expired_or_unknown" => {
            (StatusCode::GONE, "idempotency_namespace_expired_or_unknown")
        }
        "idempotency_conflict" => (StatusCode::CONFLICT, "idempotency_conflict"),
        "agent_not_found" => (StatusCode::NOT_FOUND, "agent_not_found"),
        "operation_not_found" => (StatusCode::NOT_FOUND, "operation_not_found"),
        "grant_required" => (StatusCode::FORBIDDEN, "grant_required"),
        "invalid_idempotency_key" => (StatusCode::BAD_REQUEST, "invalid_idempotency_key"),
        "service_unavailable" => (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
        _ => match status {
            StatusCode::BAD_REQUEST => (status, "invalid_request"),
            StatusCode::UNAUTHORIZED => (status, "auth_required"),
            StatusCode::FORBIDDEN => (status, "origin_forbidden"),
            StatusCode::NOT_FOUND => (status, "not_found"),
            StatusCode::METHOD_NOT_ALLOWED => (status, "method_not_allowed"),
            StatusCode::CONFLICT => (status, "conflict"),
            StatusCode::GONE => (status, "idempotency_namespace_expired_or_unknown"),
            StatusCode::PAYLOAD_TOO_LARGE => (status, "request_too_large"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE => (StatusCode::BAD_REQUEST, "invalid_request"),
            StatusCode::UNPROCESSABLE_ENTITY => (StatusCode::BAD_REQUEST, "invalid_request"),
            StatusCode::TOO_MANY_REQUESTS => (status, "rate_limited"),
            StatusCode::SERVICE_UNAVAILABLE => (status, "service_unavailable"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
        },
    }
}

async fn client_namespace_not_found() -> Response {
    client_error_response(StatusCode::NOT_FOUND, "not_found")
}

pub(super) struct RemoteGatewayContext<R: Runtime = tauri::Wry> {
    pub(super) app: AppHandle<R>,
    pub(super) config: RemoteGatewayConfig,
}

impl<R: Runtime> Clone for RemoteGatewayContext<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            config: self.config.clone(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct RemoteTerminalQuery {
    since: Option<String>,
    tail_bytes: Option<usize>,
}

async fn remote_health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "ok": true }))
}

async fn serve_remote_shell(
    State(ctx): State<RemoteGatewayContext>,
) -> Result<Response, RemoteGatewayError> {
    serve_tauri_asset(&ctx.app, "index.html")
}

async fn serve_manifest(
    State(ctx): State<RemoteGatewayContext>,
) -> Result<Response, RemoteGatewayError> {
    serve_tauri_asset(&ctx.app, "manifest.webmanifest")
}

async fn serve_remote_sw(
    State(ctx): State<RemoteGatewayContext>,
) -> Result<Response, RemoteGatewayError> {
    serve_tauri_asset(&ctx.app, "remote-sw.js")
}

async fn serve_icon(
    State(ctx): State<RemoteGatewayContext>,
) -> Result<Response, RemoteGatewayError> {
    serve_tauri_asset(&ctx.app, "icon.png")
}

async fn serve_maskable_icon(
    State(ctx): State<RemoteGatewayContext>,
) -> Result<Response, RemoteGatewayError> {
    serve_tauri_asset(&ctx.app, "icon-maskable.png")
}

async fn serve_asset_path(
    State(ctx): State<RemoteGatewayContext>,
    AxumPath(asset_path): AxumPath<String>,
) -> Result<Response, RemoteGatewayError> {
    let path = static_asset_path_for_route(&format!("/assets/{asset_path}"))
        .map_err(RemoteGatewayError::bad_request)?;
    serve_tauri_asset(&ctx.app, &path)
}

fn serve_tauri_asset(app: &AppHandle, asset_path: &str) -> Result<Response, RemoteGatewayError> {
    let asset = app
        .asset_resolver()
        .get(asset_path.to_string())
        .ok_or_else(|| RemoteGatewayError::not_found("asset_not_found"))?;
    let content_type = static_asset_content_type(asset_path, asset.mime_type().to_string());
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CACHE_CONTROL,
            static_asset_cache_control(asset_path),
        );
    if let Some(csp_header) = asset.csp_header() {
        builder = builder.header(header::CONTENT_SECURITY_POLICY, csp_header.to_string());
    }
    builder
        .body(Body::from(asset.bytes))
        .map_err(|_| RemoteGatewayError::bad_request("asset_response_failed"))
}

fn static_asset_content_type(asset_path: &str, detected_content_type: String) -> String {
    if asset_path == "manifest.webmanifest" {
        "application/manifest+json".to_string()
    } else {
        detected_content_type
    }
}

fn static_asset_cache_control(asset_path: &str) -> &'static str {
    if asset_path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    }
}

fn static_asset_path_for_route(route_path: &str) -> Result<String, &'static str> {
    let route_path = route_path.trim();
    if matches!(route_path, "/remote" | "/remote/") {
        return Ok("index.html".to_string());
    }
    let asset_path = route_path.strip_prefix('/').ok_or("asset_path_forbidden")?;
    if asset_path.is_empty()
        || asset_path.starts_with("remote/api/")
        || asset_path.contains('\\')
        || asset_path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("asset_path_forbidden");
    }
    match asset_path {
        "manifest.webmanifest" | "remote-sw.js" | "icon.png" | "icon-maskable.png" => {
            Ok(asset_path.to_string())
        }
        path if path.starts_with("assets/") => Ok(path.to_string()),
        _ => Err("asset_path_forbidden"),
    }
}

fn active_gateway_config_for_bound_server(
    loaded_config: Option<RemoteGatewayConfig>,
    bound_config: &RemoteGatewayConfig,
) -> Result<RemoteGatewayConfig, RemoteGatewayError> {
    let config = loaded_config
        .ok_or_else(|| RemoteGatewayError::service_unavailable("remote_access_disabled"))?;
    if !config.enabled {
        return Err(RemoteGatewayError::service_unavailable(
            "remote_access_disabled",
        ));
    }
    validate_gateway_bind_config(&config)
        .map_err(|_| RemoteGatewayError::service_unavailable("remote_gateway_invalid"))?;
    if config.loopback_port != bound_config.loopback_port
        || !config
            .loopback_host
            .eq_ignore_ascii_case(&bound_config.loopback_host)
        || config.canonical_origin != bound_config.canonical_origin
        || config.gateway_identity_fingerprint != bound_config.gateway_identity_fingerprint
    {
        return Err(RemoteGatewayError::service_unavailable(
            "remote_gateway_reconfigured",
        ));
    }
    Ok(config)
}

fn load_active_gateway_config_for_bound_server(
    bound_config: &RemoteGatewayConfig,
) -> Result<RemoteGatewayConfig, RemoteGatewayError> {
    let loaded_config = crate::remote::storage::load_remote_config()
        .map_err(|_| RemoteGatewayError::service_unavailable("remote_config_unavailable"))?;
    active_gateway_config_for_bound_server(loaded_config, bound_config)
}

async fn current_remote_session(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<AuthSessionResponse>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "current_session")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "session_read", "current_session")
            .await?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("session_read", "current_session"),
    );
    Ok(Json(session_response_from_record(&session)))
}

async fn submit_pairing(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<PairingSubmitRequest>,
) -> Result<Response, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, true, "submit_pairing")?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let offer = {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        if let Err(error) = check_runtime_rate_limit(
            &mut runtime,
            &format!("pairing_submit:{}", request.pairing_offer_id.trim()),
            now_ms,
            10,
            60_000,
        ) {
            audit_gateway_event_without_session(
                Some(&origin),
                GatewayAuditEvent::rejected("pairing", "submit", "rate_limited"),
            );
            return Err(error);
        }
        let offer = match crate::remote::auth::consume_pairing_offer(
            &mut runtime,
            request.pairing_offer_id.trim(),
            now_ms,
        ) {
            Ok(offer) => offer,
            Err(code) => {
                audit_gateway_event_without_session(
                    Some(&origin),
                    GatewayAuditEvent::rejected(
                        "pairing",
                        "submit",
                        gateway_static_error_code(&code),
                    ),
                );
                return Err(RemoteGatewayError::bad_request(gateway_static_error_code(
                    &code,
                )));
            }
        };
        if offer.canonical_origin != ctx.config.canonical_origin
            || offer.nonce != request.nonce.trim()
        {
            audit_gateway_event_without_session(
                Some(&origin),
                GatewayAuditEvent::rejected("pairing", "submit", "pairing_offer_invalid"),
            );
            return Err(RemoteGatewayError::bad_request("pairing_offer_invalid"));
        }
        offer
    };

    let public_key_der = decode_base64_standard(&request.public_key_spki_der_base64)
        .map_err(|_| RemoteGatewayError::bad_request("invalid_device_public_key"))?;
    if VerifyingKey::from_public_key_der(&public_key_der).is_err() {
        audit_gateway_event_without_session(
            Some(&origin),
            GatewayAuditEvent::rejected("pairing", "submit", "invalid_device_public_key"),
        );
        return Err(RemoteGatewayError::bad_request("invalid_device_public_key"));
    }

    let public_key_fingerprint = crate::remote::crypto::sha256_fingerprint(&public_key_der);
    let device_id = format!("dev_{}", crate::remote::crypto::random_url_token(18));
    let label = sanitize_device_label(&request.device_label);
    let store = crate::remote::storage::load_device_store()
        .map_err(|_| RemoteGatewayError::bad_request("device_store_failed"))?;
    if store.devices.iter().any(|device| {
        device.revoked_at.is_none() && device.public_key_fingerprint == public_key_fingerprint
    }) {
        audit_gateway_event_without_session(
            Some(&origin),
            GatewayAuditEvent::rejected("pairing", "submit", "device_already_paired"),
        );
        return Err(RemoteGatewayError::bad_request("device_already_paired"));
    }
    let pending = {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        crate::remote::auth::create_pending_pairing_request(
            &mut runtime,
            &offer,
            &device_id,
            &label,
            request.public_key_spki_der_base64.trim(),
            &public_key_fingerprint,
            now_ms,
        )
    };
    audit_gateway_event_without_session(
        Some(&origin),
        GatewayAuditEvent::accepted("pairing", "submit").target("device", &device_id),
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(pairing_response_from_pending_record(&pending)),
    )
        .into_response())
}

async fn pairing_status(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    AxumPath(request_id): AxumPath<String>,
) -> Result<Json<PairingSubmitResponse>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "pairing_status")?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let request_id = request_id.trim().to_string();
    let state = ctx.app.state::<crate::state::AppState>();
    let pending = {
        let mut runtime = state.remote_runtime.lock().await;
        crate::remote::auth::prune_expired_runtime_state(&mut runtime, now_ms);
        let pending = runtime
            .pending_pairing_requests
            .get(&request_id)
            .cloned()
            .ok_or_else(|| RemoteGatewayError::not_found("pending_pairing_not_found"))?;
        if let Err(error) = check_runtime_rate_limit(
            &mut runtime,
            &format!("pairing_status:{request_id}"),
            now_ms,
            120,
            60_000,
        ) {
            audit_gateway_event_without_session(
                Some(&origin),
                GatewayAuditEvent::rejected("pairing", "status", "rate_limited")
                    .target("pairing_request", &request_id),
            );
            return Err(error);
        }
        pending
    };
    Ok(Json(pairing_response_from_pending_record(&pending)))
}

async fn client_submit_pairing<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    Json(request): Json<RemoteClientPairingSubmitRequest>,
) -> Result<Response, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let state = ctx.app.state::<crate::state::AppState>();
    let _config_guard = state.remote_gateway_config_lock.lock().await;
    load_current_client_gateway_config(&ctx.config, client_origin)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let offer = {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        let rate_key = format!("client_pairing_submit:{}", request.pairing_offer_id.trim());
        check_runtime_rate_limit(&mut runtime, &rate_key, now_ms, 10, 60_000)?;
        crate::remote::auth::consume_client_pairing_offer(
            &mut runtime,
            request.pairing_offer_id.trim(),
            request.nonce.trim(),
            &ctx.config.canonical_origin,
            &ctx.config.gateway_identity_fingerprint,
            client_origin,
            now_ms,
        )
        .map_err(|code| RemoteGatewayError::bad_request(gateway_static_error_code(&code)))?
    };

    let public_key_der = decode_base64_standard(&request.public_key_spki_der_base64)
        .map_err(|_| RemoteGatewayError::bad_request("invalid_device_public_key"))?;
    if VerifyingKey::from_public_key_der(&public_key_der).is_err() {
        return Err(RemoteGatewayError::bad_request("invalid_device_public_key"));
    }
    let public_key_fingerprint = crate::remote::crypto::sha256_fingerprint(&public_key_der);
    let store = crate::remote::storage::load_device_store()
        .map_err(|_| RemoteGatewayError::service_unavailable("device_store_failed"))?;
    if store.devices.iter().any(|device| {
        device.revoked_at.is_none() && device.public_key_fingerprint == public_key_fingerprint
    }) {
        return Err(RemoteGatewayError::bad_request("device_already_paired"));
    }
    let device_id = format!("dev_{}", crate::remote::crypto::random_url_token(18));
    let label = sanitize_device_label(&request.device_label);
    let pending = {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        crate::remote::auth::create_pending_client_pairing_request(
            &mut runtime,
            &offer,
            crate::remote::auth::PairingDeviceIdentity {
                device_id: &device_id,
                device_label: &label,
                public_key_spki_der_base64: request.public_key_spki_der_base64.trim(),
                public_key_fingerprint: &public_key_fingerprint,
            },
            request.requested_scopes.as_deref(),
            now_ms,
        )
        .map_err(|code| RemoteGatewayError::bad_request(gateway_static_error_code(&code)))?
    };
    audit_gateway_event_without_session(
        Some(client_origin),
        GatewayAuditEvent::accepted("pairing", "submit_client").target("device", &device_id),
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(client_pairing_response_from_pending_record(&pending)),
    )
        .into_response())
}

async fn client_pairing_status<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    AxumPath(ClientPairingStatusPath { request_id }): AxumPath<ClientPairingStatusPath>,
) -> Result<Json<RemoteClientPairingSubmitResponse>, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let state = ctx.app.state::<crate::state::AppState>();
    let _config_guard = state.remote_gateway_config_lock.lock().await;
    load_current_client_gateway_config(&ctx.config, client_origin)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = ctx.app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    crate::remote::auth::prune_expired_runtime_state(&mut runtime, now_ms);
    let pending = runtime
        .pending_pairing_requests
        .get(request_id.trim())
        .filter(|request| {
            request.credential_kind == DeviceCredentialKind::PairedClient
                && request.audience == RemoteCredentialAudience::PairedClientV1
                && request.client_origin.as_deref() == Some(client_origin)
        })
        .cloned()
        .ok_or_else(|| RemoteGatewayError::not_found("pending_pairing_not_found"))?;
    check_runtime_rate_limit(
        &mut runtime,
        &format!("client_pairing_status:{}", pending.request_id),
        now_ms,
        120,
        60_000,
    )?;
    audit_gateway_event_without_session(
        Some(client_origin),
        GatewayAuditEvent::accepted("pairing", "client_status")
            .target("pairing_request", &pending.request_id),
    );
    Ok(Json(client_pairing_response_from_pending_record(&pending)))
}

async fn create_client_auth_challenge<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    Json(request): Json<AuthChallengeRequest>,
) -> Result<Json<RemoteClientAuthChallengeResponse>, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let state = ctx.app.state::<crate::state::AppState>();
    let _config_guard = state.remote_gateway_config_lock.lock().await;
    load_current_client_gateway_config(&ctx.config, client_origin)?;
    let _device_transition = state.remote_device_store_lock.lock().await;
    let device = active_client_device(request.device_id.trim(), client_origin)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut runtime = state.remote_runtime.lock().await;
    check_runtime_rate_limit(
        &mut runtime,
        &format!("client_auth_challenge:{}", device.device_id),
        now_ms,
        5,
        60_000,
    )?;
    let challenge = crate::remote::auth::create_client_auth_challenge(
        &mut runtime,
        &device.device_id,
        &ctx.config.canonical_origin,
        &ctx.config.gateway_identity_fingerprint,
        client_origin,
        now_ms,
    )
    .map_err(|_| RemoteGatewayError::too_many_requests("client_auth_challenge_capacity"))?;
    let signing_payload = client_auth_signing_payload(&challenge)
        .map_err(|_| RemoteGatewayError::bad_request("auth_challenge_invalid"))?;
    audit_gateway_event_without_session(
        Some(client_origin),
        GatewayAuditEvent::accepted("authentication", "client_challenge")
            .target("device", &device.device_id),
    );
    Ok(Json(RemoteClientAuthChallengeResponse {
        challenge_id: challenge.challenge_id,
        device_id: challenge.device_id,
        origin: challenge.canonical_origin,
        client_origin: client_origin.to_string(),
        server_identity_fingerprint: challenge.gateway_identity_fingerprint,
        nonce: challenge.nonce,
        expires_at: millis_to_rfc3339(challenge.expires_at_ms),
        audience: RemoteCredentialAudience::PairedClientV1
            .as_str()
            .to_string(),
        signing_payload,
    }))
}

async fn create_client_auth_token<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    Json(request): Json<AuthSessionRequest>,
) -> Result<Json<RemoteClientAuthSessionResponse>, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let state = ctx.app.state::<crate::state::AppState>();
    // Keep origin removal/re-addition serialized with challenge consumption and
    // final token issuance so a detached challenge cannot outlive its authority.
    let _config_guard = state.remote_gateway_config_lock.lock().await;
    let current_config = load_current_client_gateway_config(&ctx.config, client_origin)?;
    let device = active_client_device(request.device_id.trim(), client_origin)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let failure_key = format!("client_auth_session_failure:{}", device.device_id);
    let challenge = {
        let mut runtime = state.remote_runtime.lock().await;
        crate::remote::auth::check_rate_limit_available(
            &mut runtime,
            &failure_key,
            now_ms,
            5,
            10 * 60_000,
        )
        .map_err(|_| RemoteGatewayError::too_many_requests("rate_limited"))?;
        crate::remote::auth::consume_bound_client_auth_challenge(
            &mut runtime,
            request.challenge_id.trim(),
            &device.device_id,
            &ctx.config.canonical_origin,
            &ctx.config.gateway_identity_fingerprint,
            client_origin,
            now_ms,
        )
        .map_err(|code| client_auth_challenge_error(&code))?
    };
    let signature_der = decode_base64_standard(&request.signature_der_base64)
        .map_err(|_| RemoteGatewayError::bad_request("invalid_auth_signature"))?;
    let signing_payload = client_auth_signing_payload(&challenge)
        .map_err(|_| RemoteGatewayError::unauthorized("auth_challenge_invalid"))?;
    if crate::remote::crypto::verify_p256_sha256_signature(
        &decode_base64_standard(&device.public_key_spki_der_base64)
            .map_err(|_| RemoteGatewayError::bad_request("invalid_device_public_key"))?,
        signing_payload.as_bytes(),
        &signature_der,
    )
    .is_err()
    {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        let _ = crate::remote::auth::check_rate_limit(
            &mut runtime,
            &failure_key,
            now_ms,
            5,
            10 * 60_000,
        );
        audit_gateway_event_without_session(
            Some(client_origin),
            GatewayAuditEvent::rejected(
                "authentication",
                "client_session",
                "invalid_auth_signature",
            )
            .target("device", &device.device_id),
        );
        return Err(RemoteGatewayError::unauthorized("invalid_auth_signature"));
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let (access_token, session) = crate::remote::auth::issue_client_access_token_for_active_device(
        state.remote_device_store_lock.clone(),
        &state.remote_runtime,
        crate::remote::auth::ClientAccessTokenIssue {
            device_id: device.device_id.clone(),
            client_origin: client_origin.to_string(),
            gateway_origin: current_config.canonical_origin.clone(),
            gateway_identity_fingerprint: current_config.gateway_identity_fingerprint,
            challenge_expires_at_ms: challenge.expires_at_ms,
            last_used_at: millis_to_rfc3339(now_ms),
        },
    )
    .await
    .map_err(|code| client_auth_issue_error(&code))?;
    audit_client_gateway_event(
        &session,
        client_origin,
        GatewayAuditEvent::accepted("authentication", "client_session")
            .target("device", &device.device_id),
    );
    Ok(Json(RemoteClientAuthSessionResponse {
        token_type: "Bearer".to_string(),
        access_token,
        expires_at: millis_to_rfc3339(session.expires_at_ms),
        device_id: session.device_id,
        scopes: session
            .read_grants
            .iter()
            .copied()
            .map(RemoteClientScope::from)
            .chain(
                session
                    .send_grants
                    .iter()
                    .copied()
                    .map(RemoteClientScope::from),
            )
            .collect(),
        idempotency_namespace: session.idempotency_namespace,
        idempotency_expires_at: session.idempotency_expires_at_ms.map(millis_to_rfc3339),
    }))
}

async fn client_sharing_status<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
) -> Result<Json<RemoteClientSharingStatus>, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let session = require_client_session(&ctx, &headers, client_origin, None).await?;
    audit_client_gateway_event(
        &session,
        client_origin,
        GatewayAuditEvent::accepted("sharing_read", "client_sharing_status")
            .target("device", &session.device_id),
    );
    Ok(Json(RemoteClientSharingStatus {
        status: "active".to_string(),
        device_id: session.device_id,
        client_origin: session.client_origin,
        audience: session.audience.as_str().to_string(),
        read_grants: session.read_grants,
        send_grants: session.send_grants,
        expires_at: millis_to_rfc3339(session.expires_at_ms),
    }))
}

async fn list_client_agents<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let session = require_client_session(
        &ctx,
        &headers,
        client_origin,
        Some(RemoteClientScope::AgentsRead),
    )
    .await?;
    let state = ctx.app.state::<crate::state::AppState>();
    let agents = crate::remote::operations::remote_agent_roster(&state).await;
    audit_client_gateway_event(
        &session,
        client_origin,
        GatewayAuditEvent::accepted("agents_read", "list_client_agents"),
    );
    Ok(Json(serde_json::json!({ "agents": agents })))
}

async fn load_client_agent_chat<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    AxumPath(ClientAgentSessionPath { session_id }): AxumPath<ClientAgentSessionPath>,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<wardian_core::models::chat::AgentChatPage>, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let session = require_client_session(
        &ctx,
        &headers,
        client_origin,
        Some(RemoteClientScope::ChatRead),
    )
    .await?;
    // The final active-device snapshot in require_client_session admits this
    // read. Later revocation blocks new requests but does not cancel this one.
    let query = parse_remote_chat_query(raw_query.as_deref())
        .map_err(|_| RemoteGatewayError::bad_request("agent_chat_projection_unavailable"))?;
    let requested_ids = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref())
        .map_err(|_| RemoteGatewayError::bad_request("agent_chat_projection_unavailable"))?;
    let revalidate_ids =
        crate::state::conversation_archive::chat_read::validate_revalidation_request(
            &session_id,
            query.before.as_deref(),
            query.revision.as_deref(),
            query.detail.as_deref(),
            requested_ids,
        )
        .map_err(|_| RemoteGatewayError::bad_request("agent_chat_projection_unavailable"))?;
    let state = ctx.app.state::<crate::state::AppState>();
    if query.limit.is_some_and(|limit| {
        !(1..=crate::state::conversation_archive::chat_read::PAGE_ROWS).contains(&limit)
    }) {
        return Err(RemoteGatewayError::bad_request(
            "agent_chat_projection_unavailable",
        ));
    }
    let queue_error = if query.recent_preview == Some(true)
        && query.before.is_none()
        && query.detail.is_none()
        && revalidate_ids.is_none()
        && !session_id.trim().is_empty()
        && query
            .revision
            .as_ref()
            .is_none_or(|value| value.len() <= 512)
    {
        crate::commands::chat::queue_recent_capture_for_app(&state, &ctx.app, &session_id).err()
    } else {
        None
    };
    let page = crate::remote::operations::remote_agent_chat_page_with_recent(
        &state,
        &session_id,
        query.before,
        query.revision,
        query.detail,
        revalidate_ids,
        crate::commands::chat::RecentChatRead {
            limit: query.limit,
            preview: query.recent_preview.unwrap_or(false),
        },
    )
    .await
    .map_err(|error| match error.as_str() {
        "chat snapshot temporarily busy"
        | "chat config temporarily busy"
        | "chat status temporarily busy"
        | "chat source temporarily busy"
        | "chat runtime changed during read"
        | "chat read task failed"
        | "chat projection read unavailable" => {
            RemoteGatewayError::service_unavailable("agent_chat_read_retryable")
        }
        _ => RemoteGatewayError::bad_request("agent_chat_projection_unavailable"),
    })?;
    if page.events.is_empty() && !page.reset && queue_error.is_some() {
        return Err(RemoteGatewayError::service_unavailable(
            "agent_chat_read_retryable",
        ));
    }
    audit_client_gateway_event(
        &session,
        client_origin,
        GatewayAuditEvent::accepted("chat_read", "load_client_agent_chat")
            .target("agent", &session_id),
    );
    Ok(Json(page))
}

async fn submit_client_agent_message<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    AxumPath(ClientAgentSessionPath { session_id }): AxumPath<ClientAgentSessionPath>,
    Json(request): Json<RemoteClientMessageSubmitRequest>,
) -> Result<Response, RemoteGatewayError> {
    let client_origin = client_request_origin(&headers)?;
    let session = require_client_session(
        &ctx,
        &headers,
        client_origin,
        Some(RemoteClientScope::ChatSend),
    )
    .await?;
    let rate_now_ms = chrono::Utc::now().timestamp_millis();
    {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        check_runtime_rate_limit(
            &mut runtime,
            &format!("client_message_submit:{}", session.device_id),
            rate_now_ms,
            30,
            60_000,
        )?;
    }
    let key = client_idempotency_key(&headers)?;
    let (namespace_id, request_id) =
        crate::remote::client_messages::parse_idempotency_key(&session, key)
            .map_err(client_message_key_error)?;
    let now_ms = rate_now_ms;
    let existing = crate::remote::client_messages::find_existing_operation(
        &session,
        &namespace_id,
        &request_id,
        now_ms,
    )
    .map_err(client_message_store_error)?;
    if let Some(existing) = existing {
        if existing.session_id != session_id
            || existing.text != request.text
            || existing.input_mode != request.input_mode
            || existing.payload_hash
                != crate::remote::client_messages::payload_hash(
                    &session_id,
                    &request.text,
                    &request.input_mode,
                )
        {
            return Err(RemoteGatewayError::conflict("idempotency_conflict"));
        }
        if existing.delivery_state == "admitted" {
            schedule_client_message_dispatch(&ctx.app, existing.clone());
        }
        return Ok((StatusCode::ACCEPTED, Json(existing.to_dto())).into_response());
    }

    if request.input_mode != "message"
        || !crate::remote::client_messages::valid_message_text(&request.text)
    {
        return Err(RemoteGatewayError::bad_request("invalid_request"));
    }
    let state = ctx.app.state::<crate::state::AppState>();
    let roster = crate::remote::operations::remote_agent_roster(&state).await;
    if !roster.iter().any(|agent| agent.session_id == session_id) {
        return Err(RemoteGatewayError::not_found("agent_not_found"));
    }
    let (record, _) = crate::remote::client_messages::reserve_operation(
        &session,
        &namespace_id,
        &request_id,
        &session_id,
        &request.text,
        now_ms,
    )
    .map_err(client_message_store_error)?;
    schedule_client_message_dispatch(&ctx.app, record.clone());
    audit_client_gateway_event(
        &session,
        client_origin,
        GatewayAuditEvent::accepted("chat_send", "submit_client_agent_message")
            .target("agent", &session_id),
    );
    Ok((StatusCode::ACCEPTED, Json(record.to_dto())).into_response())
}

fn schedule_client_message_dispatch<R: Runtime>(
    app: &AppHandle<R>,
    operation: crate::remote::client_messages::ClientMessageOperationRecord,
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::state::AppState>();
        if crate::remote::client_messages::dispatch_if_admitted(&state, operation)
            .await
            .is_err()
        {
            crate::utils::logging::log_debug(
                "[Wardian] remote client message dispatch state update failed",
            );
        }
    });
}

async fn load_client_message_operation<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Result<Json<RemoteClientMessageOperation>, RemoteGatewayError> {
    if raw_query.is_some() || !body.is_empty() {
        return Err(RemoteGatewayError::bad_request("invalid_request"));
    }
    let client_origin = client_request_origin(&headers)?;
    let session = require_client_session(
        &ctx,
        &headers,
        client_origin,
        Some(RemoteClientScope::ChatSend),
    )
    .await?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        check_runtime_rate_limit(
            &mut runtime,
            &format!("client_message_status:{}", session.device_id),
            now_ms,
            120,
            60_000,
        )?;
    }
    let key = client_idempotency_key(&headers)?;
    let (namespace_id, request_id) =
        crate::remote::client_messages::parse_idempotency_key(&session, key)
            .map_err(client_message_key_error)?;
    let operation = crate::remote::client_messages::load_operation_for_key(
        &session,
        &namespace_id,
        &request_id,
        chrono::Utc::now().timestamp_millis(),
    )
    .map_err(client_message_store_error)?;
    Ok(Json(operation.to_dto()))
}

fn client_idempotency_key(headers: &HeaderMap) -> Result<&str, RemoteGatewayError> {
    let mut keys = headers.get_all("idempotency-key").iter();
    let key = keys
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RemoteGatewayError::bad_request("invalid_idempotency_key"))?;
    if keys.next().is_some() || key.is_empty() {
        return Err(RemoteGatewayError::bad_request("invalid_idempotency_key"));
    }
    Ok(key)
}

fn client_message_store_error(error: String) -> RemoteGatewayError {
    if error.contains("idempotency_namespace_expired_or_unknown") {
        RemoteGatewayError::gone("idempotency_namespace_expired_or_unknown")
    } else if error.contains("idempotency_conflict") {
        RemoteGatewayError::conflict("idempotency_conflict")
    } else if error.contains("operation_not_found") {
        RemoteGatewayError::not_found("operation_not_found")
    } else {
        RemoteGatewayError::service_unavailable("service_unavailable")
    }
}

fn client_message_key_error(code: &'static str) -> RemoteGatewayError {
    if code == "idempotency_namespace_expired_or_unknown" {
        RemoteGatewayError::gone(code)
    } else {
        RemoteGatewayError::bad_request(code)
    }
}

fn client_request_origin(headers: &HeaderMap) -> Result<&str, RemoteGatewayError> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RemoteGatewayError::forbidden("client_origin_forbidden"))
}

fn active_client_device(
    device_id: &str,
    client_origin: &str,
) -> Result<DeviceRecord, RemoteGatewayError> {
    let store = crate::remote::storage::load_device_store()
        .map_err(|_| RemoteGatewayError::service_unavailable("device_store_failed"))?;
    let device = store
        .devices
        .into_iter()
        .find(|device| {
            device.device_id == device_id
                && device.credential_kind == DeviceCredentialKind::PairedClient
                && device.audience == RemoteCredentialAudience::PairedClientV1
        })
        .ok_or_else(|| RemoteGatewayError::not_found("device_not_found"))?;
    if device.revoked_at.is_some() {
        return Err(RemoteGatewayError::forbidden("device_revoked"));
    }
    if device.client_origin.as_deref() != Some(client_origin) {
        return Err(RemoteGatewayError::forbidden("origin_forbidden"));
    }
    if device.credential_kind != DeviceCredentialKind::PairedClient
        || device.audience != RemoteCredentialAudience::PairedClientV1
        || !client_device_grants_are_valid(&device)
    {
        return Err(RemoteGatewayError::forbidden("grant_required"));
    }
    Ok(device)
}

async fn require_client_session<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    headers: &HeaderMap,
    client_origin: &str,
    required_grant: Option<RemoteClientScope>,
) -> Result<RemoteClientSessionRecord, RemoteGatewayError> {
    let state = ctx.app.state::<crate::state::AppState>();
    let _config_guard = state.remote_gateway_config_lock.lock().await;
    load_current_client_gateway_config(&ctx.config, client_origin)?;
    let mut authorization = headers.get_all(header::AUTHORIZATION).iter();
    let value = authorization
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RemoteGatewayError::unauthorized("client_token_required"))?;
    if authorization.next().is_some() {
        return Err(RemoteGatewayError::unauthorized("client_token_invalid"));
    }
    let token = value
        .strip_prefix("Bearer ")
        .filter(|token| {
            token.len() == 43
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
        .ok_or_else(|| RemoteGatewayError::unauthorized("client_token_invalid"))?;
    let verifier = crate::remote::crypto::sha256_token_verifier(token);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let session = {
        let mut runtime = state.remote_runtime.lock().await;
        let Some(session) = runtime.client_sessions.get(&verifier).cloned() else {
            crate::remote::auth::prune_expired_runtime_state(&mut runtime, now_ms);
            return Err(RemoteGatewayError::unauthorized("client_token_invalid"));
        };
        if session.revoked {
            return Err(RemoteGatewayError::forbidden("device_revoked"));
        }
        if !crate::remote::auth::client_session_is_active(&session, now_ms) {
            runtime.client_sessions.remove(&verifier);
            return Err(RemoteGatewayError::unauthorized("client_token_expired"));
        }
        crate::remote::auth::prune_expired_runtime_state(&mut runtime, now_ms);
        session
    };
    if !crate::remote::auth::client_session_is_active(&session, now_ms)
        || session.client_origin != client_origin
        || session.gateway_origin != ctx.config.canonical_origin
        || session.audience != RemoteCredentialAudience::PairedClientV1
        || session.gateway_identity_fingerprint != ctx.config.gateway_identity_fingerprint
    {
        return Err(RemoteGatewayError::unauthorized("client_token_invalid"));
    }
    let device = active_client_device(&session.device_id, client_origin)?;
    if session.read_grants != device.read_grants || session.send_grants != device.send_grants {
        return Err(RemoteGatewayError::forbidden("grant_required"));
    }
    if required_grant.is_some_and(|grant| !client_session_has_scope(&session, grant)) {
        return Err(RemoteGatewayError::forbidden("grant_required"));
    }
    Ok(session)
}

fn client_device_grants_are_valid(device: &DeviceRecord) -> bool {
    !device
        .read_grants
        .iter()
        .enumerate()
        .any(|(index, grant)| device.read_grants[..index].contains(grant))
        && !device
            .send_grants
            .iter()
            .enumerate()
            .any(|(index, grant)| device.send_grants[..index].contains(grant))
}

fn client_session_has_scope(session: &RemoteClientSessionRecord, scope: RemoteClientScope) -> bool {
    match scope {
        RemoteClientScope::AgentsRead => session.read_grants.contains(&RemoteReadGrant::AgentsRead),
        RemoteClientScope::ChatRead => session.read_grants.contains(&RemoteReadGrant::ChatRead),
        RemoteClientScope::ChatSend => session.send_grants.contains(&RemoteSendGrant::ChatSend),
    }
}

fn load_current_client_gateway_config(
    bound_config: &RemoteGatewayConfig,
    client_origin: &str,
) -> Result<RemoteGatewayConfig, RemoteGatewayError> {
    let current_config = crate::remote::storage::load_remote_config()
        .map_err(|_| RemoteGatewayError::service_unavailable("remote_config_unavailable"))?
        .ok_or_else(|| RemoteGatewayError::service_unavailable("remote_access_disabled"))?;
    if !current_config.enabled
        || current_config.canonical_origin != bound_config.canonical_origin
        || current_config.gateway_identity_fingerprint != bound_config.gateway_identity_fingerprint
        || current_config.gateway_identity_public_key != bound_config.gateway_identity_public_key
        || !crate::remote::policy::is_approved_client_origin(
            &current_config.client_origin_allowlist,
            client_origin,
        )
    {
        return Err(RemoteGatewayError::forbidden("client_origin_forbidden"));
    }
    Ok(current_config)
}

fn client_auth_signing_payload(
    challenge: &crate::remote::models::AuthChallengeRecord,
) -> Result<String, String> {
    if challenge.audience != RemoteCredentialAudience::PairedClientV1 {
        return Err("wrong_client_auth_audience".to_string());
    }
    let client_origin = challenge
        .client_origin
        .as_deref()
        .ok_or_else(|| "client_auth_origin_missing".to_string())?;
    for field in [
        challenge.canonical_origin.as_str(),
        client_origin,
        challenge.device_id.as_str(),
        challenge.challenge_id.as_str(),
        challenge.nonce.as_str(),
    ] {
        if field.contains('\r') || field.contains('\n') {
            return Err("client_auth_field_invalid".to_string());
        }
    }
    Ok(format!(
        "wardian.remote.client.auth.v1\norigin:{}\nclient_origin:{}\naudience:{}\ndevice:{}\nchallenge:{}\nnonce:{}",
        challenge.canonical_origin,
        client_origin,
        RemoteCredentialAudience::PairedClientV1.as_str(),
        challenge.device_id,
        challenge.challenge_id,
        challenge.nonce,
    ))
}

fn client_auth_issue_error(code: &str) -> RemoteGatewayError {
    match code {
        "remote_device_not_found" | "remote_device_revoked" => {
            RemoteGatewayError::forbidden("device_revoked")
        }
        "auth_challenge_expired" => RemoteGatewayError::unauthorized("auth_challenge_expired"),
        "client_session_capacity" => {
            RemoteGatewayError::service_unavailable("client_session_capacity")
        }
        _ => RemoteGatewayError::service_unavailable("device_store_failed"),
    }
}

fn client_auth_challenge_error(code: &str) -> RemoteGatewayError {
    match code {
        "auth_challenge_expired" => RemoteGatewayError::unauthorized("auth_challenge_expired"),
        "auth_challenge_not_found" | "auth_challenge_used" | "auth_challenge_mismatch" => {
            RemoteGatewayError::unauthorized("auth_challenge_invalid")
        }
        _ => RemoteGatewayError::unauthorized("auth_challenge_invalid"),
    }
}

async fn create_auth_challenge<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    Json(request): Json<AuthChallengeRequest>,
) -> Result<Json<AuthChallengeResponse>, RemoteGatewayError> {
    let origin =
        require_audited_request_boundary(&ctx.config, &headers, true, "create_auth_challenge")?;
    let device = active_remote_device(request.device_id.trim())?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = ctx.app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    if let Err(error) = check_runtime_rate_limit(
        &mut runtime,
        &format!("auth_challenge:{}", device.device_id),
        now_ms,
        5,
        60_000,
    ) {
        audit_gateway_event_without_session(
            Some(&origin),
            GatewayAuditEvent::rejected("authentication", "challenge", "rate_limited")
                .target("device", &device.device_id),
        );
        return Err(error);
    }
    let challenge = crate::remote::auth::create_auth_challenge(
        &mut runtime,
        &device.device_id,
        &ctx.config.canonical_origin,
        &ctx.config.gateway_identity_fingerprint,
        now_ms,
    );
    audit_gateway_event_without_session(
        Some(&origin),
        GatewayAuditEvent::accepted("authentication", "challenge")
            .target("device", &device.device_id),
    );
    Ok(Json(AuthChallengeResponse {
        challenge_id: challenge.challenge_id,
        device_id: challenge.device_id,
        origin: challenge.canonical_origin,
        server_identity_fingerprint: ctx.config.gateway_identity_fingerprint.clone(),
        nonce: challenge.nonce,
        expires_at: millis_to_rfc3339(challenge.expires_at_ms),
        audience: "wardian_remote_pwa".to_string(),
    }))
}

async fn create_auth_session<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    Json(request): Json<AuthSessionRequest>,
) -> Result<Response, RemoteGatewayError> {
    let origin =
        require_audited_request_boundary(&ctx.config, &headers, true, "create_auth_session")?;
    let device = active_remote_device(request.device_id.trim())?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let failure_rate_limit_key = format!("auth_session_failure:{}", device.device_id);
    let challenge = {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        if let Err(error) = crate::remote::auth::check_rate_limit_available(
            &mut runtime,
            &failure_rate_limit_key,
            now_ms,
            5,
            10 * 60_000,
        )
        .map_err(|_| RemoteGatewayError::too_many_requests("rate_limited"))
        {
            audit_gateway_event_without_session(
                Some(&origin),
                GatewayAuditEvent::rejected("authentication", "session", "rate_limited")
                    .target("device", &device.device_id),
            );
            return Err(error);
        }
        match crate::remote::auth::consume_auth_challenge(
            &mut runtime,
            request.challenge_id.trim(),
            now_ms,
        ) {
            Ok(challenge) => challenge,
            Err(code) => {
                audit_gateway_event_without_session(
                    Some(&origin),
                    GatewayAuditEvent::rejected(
                        "authentication",
                        "session",
                        gateway_static_error_code(&code),
                    )
                    .target("device", &device.device_id),
                );
                return Err(RemoteGatewayError::bad_request(gateway_static_error_code(
                    &code,
                )));
            }
        }
    };
    if challenge.device_id != device.device_id
        || challenge.canonical_origin != ctx.config.canonical_origin
    {
        audit_gateway_event_without_session(
            Some(&origin),
            GatewayAuditEvent::rejected("authentication", "session", "auth_challenge_mismatch")
                .target("device", &device.device_id),
        );
        return Err(RemoteGatewayError::bad_request("auth_challenge_mismatch"));
    }
    let public_key_der = decode_base64_standard(&device.public_key_spki_der_base64)
        .map_err(|_| RemoteGatewayError::bad_request("invalid_device_public_key"))?;
    let signature_der = decode_base64_standard(&request.signature_der_base64)
        .map_err(|_| RemoteGatewayError::bad_request("invalid_auth_signature"))?;
    if crate::remote::crypto::verify_p256_sha256_signature(
        &public_key_der,
        &auth_signature_message(&challenge),
        &signature_der,
    )
    .is_err()
    {
        {
            let state = ctx.app.state::<crate::state::AppState>();
            let mut runtime = state.remote_runtime.lock().await;
            let _ = crate::remote::auth::check_rate_limit(
                &mut runtime,
                &failure_rate_limit_key,
                now_ms,
                5,
                10 * 60_000,
            );
        }
        audit_gateway_event_without_session(
            Some(&origin),
            GatewayAuditEvent::rejected("authentication", "session", "invalid_auth_signature")
                .target("device", &device.device_id),
        );
        return Err(RemoteGatewayError::unauthorized("invalid_auth_signature"));
    }

    let state = ctx.app.state::<crate::state::AppState>();
    let session = crate::remote::auth::issue_session_for_active_device(
        state.remote_device_store_lock.clone(),
        &state.remote_runtime,
        device.device_id.clone(),
        millis_to_rfc3339(now_ms),
        now_ms,
    )
    .await
    .map_err(|error| match error.message.as_str() {
        "remote_device_revoked" | "remote_device_not_found" => {
            RemoteGatewayError::not_found("device_not_found")
        }
        _ => RemoteGatewayError::bad_request("device_store_failed"),
    })?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("authentication", "session")
            .target("device", &device.device_id),
    );
    let mut response = Json(session_response_from_record(&session)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        remote_session_cookie_header_value(&session.session_id)?,
    );
    Ok(response)
}

async fn list_remote_agents(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "list_agents")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "roster_read", "list_agents")
            .await?;
    let state = ctx.app.state::<crate::state::AppState>();
    let agents = crate::remote::operations::remote_agent_roster(&state).await;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("roster_read", "list_agents"),
    );
    Ok(Json(serde_json::json!({ "agents": agents })))
}

async fn load_remote_queue(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "load_queue")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "roster_read", "load_queue")
            .await?;
    let state = ctx.app.state::<crate::state::AppState>();
    let items = crate::remote::operations::remote_queue_items_for_app(&ctx.app, &state)
        .await
        .map_err(|_| {
            RemoteGatewayError::service_unavailable(
                crate::remote::operations::REMOTE_AUTOMATION_INBOX_UNAVAILABLE,
            )
        })?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("roster_read", "load_queue"),
    );
    Ok(Json(serde_json::json!({ "items": items })))
}

type RemoteInboxActionExecutor<R> = fn(
    AppHandle<R>,
    RemoteInboxActionRequest,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'static>,
>;

async fn run_inbox_action(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteInboxActionRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    run_inbox_action_with_executor(ctx, headers, request, execute_remote_inbox_action).await
}

async fn run_inbox_action_with_executor<R: Runtime>(
    ctx: RemoteGatewayContext<R>,
    headers: HeaderMap,
    request: RemoteInboxActionRequest,
    execute: RemoteInboxActionExecutor<R>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let action = request.action.clone();
    let target = request.item_id.clone().unwrap_or_else(|| "all".to_string());
    let (origin, session) = authorize_inbox_action(&ctx, &headers, &action).await?;
    if let Err(error) = execute(ctx.app.clone(), request).await {
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("inbox_action", &action, "inbox_action_failed")
                .target("inbox", &target),
        );
        return Err(remote_inbox_action_gateway_error(&error));
    }
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("inbox_action", &action).target("inbox", &target),
    );
    let _ = ctx.app.emit("inbox-updated", ());
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn remote_inbox_action_gateway_error(error: &str) -> RemoteGatewayError {
    if error == crate::remote::operations::REMOTE_AUTOMATION_INBOX_UNAVAILABLE {
        RemoteGatewayError::service_unavailable(
            crate::remote::operations::REMOTE_AUTOMATION_INBOX_UNAVAILABLE,
        )
    } else {
        RemoteGatewayError::bad_request("inbox_action_failed")
    }
}

async fn load_remote_watchlists(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<crate::remote::models::RemoteWatchlistResponse>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "load_watchlists")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "roster_read", "load_watchlists")
            .await?;
    let response = crate::remote::operations::remote_watchlist_state()
        .map_err(|_| RemoteGatewayError::service_unavailable("watchlists_unavailable"))?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("roster_read", "load_watchlists"),
    );
    Ok(Json(response))
}

async fn list_remote_automations(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin =
        require_audited_request_boundary(&ctx.config, &headers, false, "list_automations")?;
    let session = require_audited_remote_session(
        &ctx,
        &headers,
        &origin,
        "automation_read",
        "list_automations",
    )
    .await?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("automation_read", "list_automations"),
    );
    Ok(Json(remote_automation_compat_empty_list_response()))
}

fn remote_automation_compat_empty_list_response() -> serde_json::Value {
    serde_json::json!({ "automations": [] })
}

#[derive(Debug, Default, serde::Deserialize)]
struct RemoteAutomationMonitorQuery {
    active_offset: Option<usize>,
    recent_offset: Option<usize>,
    schedule_offset: Option<usize>,
}

async fn load_remote_automation_monitor(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<crate::remote::models::RemoteAutomationMonitorSnapshot>, RemoteGatewayError> {
    let origin =
        require_audited_request_boundary(&ctx.config, &headers, false, "load_automation_monitor")?;
    let session = require_audited_remote_session(
        &ctx,
        &headers,
        &origin,
        "automation_read",
        "load_automation_monitor",
    )
    .await?;
    let query = match parse_remote_automation_monitor_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(code) => {
            audit_gateway_event(
                &session,
                &origin,
                GatewayAuditEvent::rejected("automation_read", "load_automation_monitor", code),
            );
            return Err(RemoteGatewayError::bad_request(code));
        }
    };
    let result = tokio::task::spawn_blocking(move || {
        crate::remote::operations::remote_automation_monitor_snapshot(
            query.active_offset.unwrap_or(0),
            query.recent_offset.unwrap_or(0),
            query.schedule_offset.unwrap_or(0),
        )
    })
    .await
    .map_err(|_| "automation_monitor_unavailable".to_string())
    .and_then(|result| result);

    match result {
        Ok(snapshot) => {
            audit_gateway_event(
                &session,
                &origin,
                GatewayAuditEvent::accepted("automation_read", "load_automation_monitor"),
            );
            Ok(Json(snapshot))
        }
        Err(_) => {
            audit_gateway_event(
                &session,
                &origin,
                GatewayAuditEvent::rejected(
                    "automation_read",
                    "load_automation_monitor",
                    "automation_monitor_unavailable",
                ),
            );
            Err(RemoteGatewayError::service_unavailable(
                "automation_monitor_unavailable",
            ))
        }
    }
}

fn parse_remote_automation_monitor_query(
    raw_query: Option<&str>,
) -> Result<RemoteAutomationMonitorQuery, &'static str> {
    let mut query = RemoteAutomationMonitorQuery::default();
    for pair in raw_query.unwrap_or_default().split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let slot = match key {
            "active_offset" => &mut query.active_offset,
            "recent_offset" => &mut query.recent_offset,
            "schedule_offset" => &mut query.schedule_offset,
            _ => continue,
        };
        if slot.is_some() || value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid_automation_monitor_query");
        }
        *slot = Some(
            value
                .parse::<usize>()
                .map_err(|_| "invalid_automation_monitor_query")?,
        );
    }
    Ok(query)
}

#[derive(Debug, serde::Deserialize)]
struct RemoteChatQuery {
    before: Option<String>,
    limit: Option<usize>,
    recent_preview: Option<bool>,
    revision: Option<String>,
    detail: Option<String>,
    // Keep this a single canonical JSON-list value; Axum's `Query<Vec<_>>`
    // does not define repeated-key list extraction.
    #[serde(default)]
    revalidate_ids: Option<String>,
}

const REMOTE_CHAT_REVALIDATION_JSON_MAX_BYTES: usize =
    crate::state::conversation_archive::chat_read::MAX_REVALIDATED_SOURCE_IDS
        * (crate::state::conversation_archive::chat_read::MAX_REVALIDATED_SOURCE_ID_BYTES + 2)
        + crate::state::conversation_archive::chat_read::MAX_REVALIDATED_SOURCE_IDS
        - 1
        + 2;
const REMOTE_CHAT_QUERY_MAX_BYTES: usize = REMOTE_CHAT_REVALIDATION_JSON_MAX_BYTES * 3
    + crate::state::conversation_archive::chat_read::MAX_REVALIDATED_SOURCE_ID_BYTES * 3 * 3
    + 128;

fn encode_remote_chat_query_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

fn has_canonical_remote_chat_revalidation_value(
    raw_query: Option<&str>,
    decoded_value: &str,
) -> bool {
    let Some(raw_query) = raw_query else {
        return false;
    };
    let mut values = raw_query
        .split('&')
        .filter(|part| part.split('=').next() == Some("revalidate_ids"));
    let expected = encode_remote_chat_query_component(decoded_value);
    let Some(value) = values
        .next()
        .and_then(|part| part.strip_prefix("revalidate_ids="))
    else {
        return false;
    };
    value == expected && values.next().is_none()
}

fn decode_remote_chat_revalidation_ids(
    value: Option<&str>,
) -> Result<Option<Vec<String>>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > REMOTE_CHAT_REVALIDATION_JSON_MAX_BYTES {
        return Err("invalid chat revalidation query");
    }
    let ids: Vec<String> =
        serde_json::from_str(value).map_err(|_| "invalid chat revalidation query")?;
    let canonical = serde_json::to_string(&ids).map_err(|_| "invalid chat revalidation query")?;
    if canonical != value {
        return Err("invalid chat revalidation query");
    }
    Ok(Some(ids))
}

fn parse_remote_chat_query(raw_query: Option<&str>) -> Result<RemoteChatQuery, ()> {
    if raw_query.is_some_and(|query| query.len() > REMOTE_CHAT_QUERY_MAX_BYTES) {
        return Err(());
    }
    let uri: axum::http::Uri = format!("/?{}", raw_query.unwrap_or_default())
        .parse()
        .map_err(|_| ())?;
    let Query(query) = Query::<RemoteChatQuery>::try_from_uri(&uri).map_err(|_| ())?;
    if query
        .revalidate_ids
        .as_deref()
        .is_some_and(|value| !has_canonical_remote_chat_revalidation_value(raw_query, value))
    {
        return Err(());
    }
    Ok(query)
}

async fn load_remote_agent_chat<R: Runtime>(
    State(ctx): State<RemoteGatewayContext<R>>,
    headers: HeaderMap,
    AxumPath(session_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let query = parse_remote_chat_query(raw_query.as_deref())
        .map_err(|_| RemoteGatewayError::bad_request("agent_chat_projection_unavailable"))?;
    let requested_ids = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref())
        .map_err(|_| RemoteGatewayError::bad_request("agent_chat_projection_unavailable"))?;
    let revalidate_ids =
        crate::state::conversation_archive::chat_read::validate_revalidation_request(
            &session_id,
            query.before.as_deref(),
            query.revision.as_deref(),
            query.detail.as_deref(),
            requested_ids,
        )
        .map_err(|_| RemoteGatewayError::bad_request("agent_chat_projection_unavailable"))?;
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "load_agent_chat")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "chat_read", "load_agent_chat")
            .await?;
    let state = ctx.app.state::<crate::state::AppState>();
    if query.limit.is_some_and(|limit| {
        !(1..=crate::state::conversation_archive::chat_read::PAGE_ROWS).contains(&limit)
    }) {
        return Err(RemoteGatewayError::bad_request(
            "agent_chat_projection_unavailable",
        ));
    }
    let queue_error = if query.recent_preview == Some(true)
        && query.before.is_none()
        && query.detail.is_none()
        && revalidate_ids.is_none()
        && !session_id.trim().is_empty()
        && query
            .revision
            .as_ref()
            .is_none_or(|value| value.len() <= 512)
    {
        crate::commands::chat::queue_recent_capture_for_app(&state, &ctx.app, &session_id).err()
    } else {
        None
    };
    let page = crate::remote::operations::remote_agent_chat_page_with_recent(
        &state,
        &session_id,
        query.before,
        query.revision,
        query.detail,
        revalidate_ids,
        crate::commands::chat::RecentChatRead {
            limit: query.limit,
            preview: query.recent_preview.unwrap_or(false),
        },
    )
    .await
    .map_err(|error| match error.as_str() {
        "chat snapshot temporarily busy"
        | "chat config temporarily busy"
        | "chat status temporarily busy"
        | "chat source temporarily busy"
        | "chat runtime changed during read"
        | "chat read task failed"
        | "chat projection read unavailable" => {
            RemoteGatewayError::service_unavailable("agent_chat_read_retryable")
        }
        _ => RemoteGatewayError::bad_request("agent_chat_projection_unavailable"),
    })?;
    if page.events.is_empty() && !page.reset && queue_error.is_some() {
        return Err(RemoteGatewayError::service_unavailable(
            "agent_chat_read_retryable",
        ));
    }
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("chat_read", "load_agent_chat").target("agent", &session_id),
    );
    Ok(Json(serde_json::json!(page)))
}

async fn load_remote_agent_terminal(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<RemoteTerminalQuery>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin =
        require_audited_request_boundary(&ctx.config, &headers, false, "load_agent_terminal")?;
    let session = require_audited_remote_session(
        &ctx,
        &headers,
        &origin,
        "terminal_read",
        "load_agent_terminal",
    )
    .await?;
    let state = ctx.app.state::<crate::state::AppState>();
    let tail_bytes = query
        .tail_bytes
        .unwrap_or(REMOTE_TERMINAL_DEFAULT_TAIL_BYTES)
        .min(REMOTE_TERMINAL_MAX_TAIL_BYTES);
    let since = query
        .since
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let snapshot = crate::remote::operations::remote_agent_terminal_snapshot(
        &state,
        &session_id,
        since,
        Some(tail_bytes),
    )
    .await
    .map_err(|_| RemoteGatewayError::bad_request("agent_terminal_failed"))?;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("terminal_read", "load_agent_terminal")
            .target("agent", &session_id),
    );
    Ok(Json(serde_json::json!({ "snapshot": snapshot })))
}

async fn run_agent_action(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteAgentActionRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, true, "agent_action")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "agent_action", &request.action)
            .await?;
    require_mutation_rate_limit(&ctx, &session, &origin, "agent_action", &request.action).await?;
    require_agent_action_specific_rate_limit(&ctx, &session, &origin, &request.action).await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("agent_action", &request.action, error.code)
                .target("agent", &request.target),
        );
        return Err(error);
    }
    let receipt =
        match crate::remote::operations::run_remote_agent_action(&ctx.app, request.clone()).await {
            Ok(receipt) => receipt,
            Err(error) => {
                let code = "agent_action_failed";
                audit_gateway_event(
                    &session,
                    &origin,
                    GatewayAuditEvent::rejected("agent_action", &request.action, code)
                        .target("agent", &request.target),
                );
                return Err(RemoteGatewayError::bad_request_with_detail(code, error));
            }
        };
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("agent_action", &request.action)
            .target("agent", &request.target),
    );
    Ok(Json(serde_json::json!({
        "ok": true,
        "chat_event_id": receipt.as_ref().map(|receipt| &receipt.chat_event_id),
        "chat_agent_id": receipt.as_ref().map(|receipt| &receipt.chat_agent_id),
        "chat_conversation_id": receipt.as_ref().map(|receipt| &receipt.chat_conversation_id),
        "chat_source_epoch": receipt.as_ref().and_then(|receipt| receipt.chat_source_epoch.as_ref()),
    })))
}

fn execute_remote_inbox_action(
    app: AppHandle,
    request: RemoteInboxActionRequest,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'static>> {
    Box::pin(async move {
        let state = app.state::<crate::state::AppState>();
        crate::remote::operations::apply_remote_inbox_action(&state, &app, request).await
    })
}

async fn require_agent_action_specific_rate_limit(
    ctx: &RemoteGatewayContext,
    session: &RemoteSessionRecord,
    origin: &str,
    action: &str,
) -> Result<(), RemoteGatewayError> {
    let Some((max_attempts, window_ms)) = remote_agent_action_specific_limit(action) else {
        return Ok(());
    };
    let state = ctx.app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    match crate::remote::auth::check_rate_limit(
        &mut runtime,
        &format!("agent_action:{action}:{}", session.session_id),
        now_ms,
        max_attempts,
        window_ms,
    ) {
        Ok(()) => Ok(()),
        Err(_) => {
            audit_gateway_event(
                session,
                origin,
                GatewayAuditEvent::rejected("agent_action", action, "rate_limited"),
            );
            Err(RemoteGatewayError::too_many_requests("rate_limited"))
        }
    }
}

fn remote_agent_action_specific_limit(action: &str) -> Option<(usize, i64)> {
    match action {
        "clear" | "kill" => Some((10, 10 * 60_000)),
        _ => None,
    }
}

async fn create_ws_ticket(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteWebSocketTicketRequest>,
) -> Result<Json<RemoteWebSocketTicketResponse>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, true, "websocket_ticket")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "websocket_ticket", "create")
            .await?;
    require_mutation_rate_limit(&ctx, &session, &origin, "websocket_ticket", "create").await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("websocket_ticket", "create", error.code)
                .target("stream", &request.stream),
        );
        return Err(error);
    }
    let stream = match validate_remote_stream(&request.stream) {
        Ok(stream) => stream,
        Err(code) => {
            audit_gateway_event(
                &session,
                &origin,
                GatewayAuditEvent::rejected("websocket_ticket", "create", code)
                    .target("stream", &request.stream),
            );
            return Err(RemoteGatewayError::bad_request(code));
        }
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = ctx.app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    let ticket = crate::remote::auth::create_websocket_ticket(
        &mut runtime,
        &session,
        stream,
        &ctx.config.canonical_origin,
        now_ms,
    );
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("websocket_ticket", "create").target("stream", stream),
    );
    Ok(Json(RemoteWebSocketTicketResponse {
        ticket: ticket.ticket,
        expires_at: millis_to_rfc3339(ticket.expires_at_ms),
    }))
}

async fn status_stream_upgrade(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, RemoteGatewayError> {
    require_audited_request_boundary(&ctx.config, &headers, true, "status_stream_upgrade")?;
    Ok(ws.on_upgrade(move |socket| async move {
        handle_status_socket(ctx, socket).await;
    }))
}

async fn terminal_stream_upgrade(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    AxumPath(session_id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, RemoteGatewayError> {
    require_audited_request_boundary(&ctx.config, &headers, true, "terminal_stream_upgrade")?;
    Ok(ws.on_upgrade(move |socket| async move {
        crate::remote::terminal_stream::handle_terminal_socket(ctx, session_id, socket).await;
    }))
}

async fn handle_status_socket(ctx: RemoteGatewayContext, mut socket: WebSocket) {
    let first_message =
        match tokio::time::timeout(WEBSOCKET_FIRST_TICKET_TIMEOUT, socket.recv()).await {
            Ok(Some(Ok(first_message))) => first_message,
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => {
                send_status_socket_error(&mut socket, "ticket_timeout").await;
                return;
            }
        };
    let ticket = match parse_status_socket_ticket_message(first_message) {
        Ok(ticket) => ticket,
        Err(code) => {
            send_status_socket_error(&mut socket, code).await;
            return;
        }
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = ctx.app.state::<crate::state::AppState>();
    let ticket_record = {
        let mut runtime = state.remote_runtime.lock().await;
        match crate::remote::auth::consume_websocket_ticket(&mut runtime, &ticket, now_ms) {
            Ok(record) => record,
            Err(_) => {
                drop(runtime);
                send_status_socket_error(&mut socket, "invalid_websocket_ticket").await;
                return;
            }
        }
    };
    if ticket_record.stream != REMOTE_STATUS_STREAM_NAME
        || ticket_record.canonical_origin != ctx.config.canonical_origin
    {
        send_status_socket_error(&mut socket, "invalid_websocket_ticket").await;
        return;
    }
    {
        let mut runtime = state.remote_runtime.lock().await;
        if crate::remote::auth::try_open_status_stream(&mut runtime, &ticket_record.session_id)
            .is_err()
        {
            drop(runtime);
            send_status_socket_error(&mut socket, "websocket_connection_limit").await;
            return;
        }
    }

    'stream: loop {
        let session_active = remote_session_is_active(&state, &ticket_record.session_id).await;
        if !session_active {
            send_status_socket_error(&mut socket, "session_expired").await;
            break;
        }

        let agents = crate::remote::operations::remote_agent_roster(&state).await;
        let payload = serde_json::json!({
            "type": "agent_status",
            "agents": agents,
        });
        if socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .is_err()
        {
            break;
        }

        let next_tick = tokio::time::sleep(std::time::Duration::from_secs(2));
        tokio::pin!(next_tick);
        loop {
            tokio::select! {
                _ = &mut next_tick => break,
                message = socket.recv() => {
                    let action = match message {
                        Some(Ok(message)) => status_stream_client_message_action(Some(message)),
                        None | Some(Err(_)) => status_stream_client_message_action(None),
                    };
                    match action {
                        StatusStreamClientMessageAction::IgnoreUntilNextTick => continue,
                        StatusStreamClientMessageAction::Close => break 'stream,
                    }
                }
            }
        }
    }
    let mut runtime = state.remote_runtime.lock().await;
    crate::remote::auth::close_status_stream(&mut runtime, &ticket_record.session_id);
}

#[derive(Debug, PartialEq, Eq)]
enum StatusStreamClientMessageAction {
    IgnoreUntilNextTick,
    Close,
}

fn status_stream_client_message_action(
    message: Option<Message>,
) -> StatusStreamClientMessageAction {
    match message {
        Some(Message::Close(_)) | None => StatusStreamClientMessageAction::Close,
        Some(_) => StatusStreamClientMessageAction::IgnoreUntilNextTick,
    }
}

fn parse_status_socket_ticket_message(message: Message) -> Result<String, &'static str> {
    let Message::Text(text) = message else {
        return Err("invalid_ticket_message");
    };
    let value = serde_json::from_str::<serde_json::Value>(text.as_str())
        .map_err(|_| "invalid_ticket_message")?;
    let ticket = value
        .get("ticket")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("invalid_ticket_message")?;
    Ok(ticket.to_string())
}

#[cfg(test)]
fn status_stream_session_is_active(
    runtime: &crate::remote::models::RemoteRuntimeState,
    session_id: &str,
    now_ms: i64,
) -> bool {
    runtime
        .sessions
        .get(session_id)
        .is_some_and(|session| crate::remote::auth::session_is_active(session, now_ms))
}

async fn remote_session_is_active(state: &crate::state::AppState, session_id: &str) -> bool {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let session = {
        let runtime = state.remote_runtime.lock().await;
        runtime
            .sessions
            .get(session_id)
            .filter(|session| crate::remote::auth::session_is_active(session, now_ms))
            .cloned()
    };
    let Some(session) = session else {
        return false;
    };
    remote_device_authority_active(session.device_id)
        .await
        .is_ok()
}

async fn send_status_socket_error(socket: &mut WebSocket, code: &'static str) {
    send_socket_error(socket, code).await;
}

async fn send_socket_error(socket: &mut WebSocket, code: &'static str) {
    let payload = serde_json::json!({
        "type": "error",
        "code": code,
    });
    let _ = socket.send(Message::Text(payload.to_string().into())).await;
}

fn require_audited_request_boundary(
    config: &RemoteGatewayConfig,
    headers: &HeaderMap,
    require_origin: bool,
    action: &'static str,
) -> Result<String, RemoteGatewayError> {
    let config = match load_active_gateway_config_for_bound_server(config) {
        Ok(config) => config,
        Err(error) => {
            audit_gateway_event_without_session(
                canonical_origin_for_audit(config).as_deref(),
                GatewayAuditEvent::rejected("gateway_policy", action, error.code),
            );
            return Err(error);
        }
    };
    match require_request_boundary(&config, headers, require_origin) {
        Ok(origin) => Ok(origin),
        Err(error) => {
            audit_gateway_event_without_session(
                canonical_origin_for_audit(&config).as_deref(),
                GatewayAuditEvent::rejected("gateway_policy", action, error.code),
            );
            Err(error)
        }
    }
}

async fn require_audited_remote_session<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    headers: &HeaderMap,
    origin: &str,
    event_type: &str,
    action: &str,
) -> Result<RemoteSessionRecord, RemoteGatewayError> {
    match require_remote_session(ctx, headers).await {
        Ok(session) => Ok(session),
        Err(error) => {
            audit_gateway_event_without_session(
                Some(origin),
                GatewayAuditEvent::rejected(event_type, action, error.code),
            );
            Err(error)
        }
    }
}

fn require_request_boundary(
    config: &RemoteGatewayConfig,
    headers: &HeaderMap,
    require_origin: bool,
) -> Result<String, RemoteGatewayError> {
    let origin = crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)
        .map_err(|_| RemoteGatewayError::forbidden("origin_forbidden"))?;
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RemoteGatewayError::forbidden("origin_forbidden"))?;
    if !host.eq_ignore_ascii_case(origin.host()) {
        return Err(RemoteGatewayError::forbidden("origin_forbidden"));
    }
    if let Some(fetch_site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        if fetch_site != "same-origin" {
            return Err(RemoteGatewayError::forbidden("origin_forbidden"));
        }
    }
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(request_origin) if request_origin == origin.raw() => Ok(origin.raw().to_string()),
        Some(_) => Err(RemoteGatewayError::forbidden("origin_forbidden")),
        None if require_origin => Err(RemoteGatewayError::forbidden("origin_forbidden")),
        None => Ok(origin.raw().to_string()),
    }
}

fn validate_remote_stream(stream: &str) -> Result<&'static str, &'static str> {
    match stream.trim() {
        REMOTE_STATUS_STREAM_NAME => Ok(REMOTE_STATUS_STREAM_NAME),
        "terminal_attach" => Ok("terminal_attach"),
        _ => Err("unsupported_stream"),
    }
}

async fn require_remote_session<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    headers: &HeaderMap,
) -> Result<RemoteSessionRecord, RemoteGatewayError> {
    let session_id = remote_session_cookie_value(headers)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = ctx.app.state::<crate::state::AppState>();
    let session = {
        let mut runtime = state.remote_runtime.lock().await;
        let session = runtime
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| RemoteGatewayError::unauthorized("session_not_found"))?;
        if crate::remote::auth::refresh_session_activity(session, now_ms).is_err() {
            return Err(RemoteGatewayError::unauthorized("session_expired"));
        }
        session.clone()
    };

    match remote_device_authority_active(session.device_id.clone()).await {
        Ok(()) => Ok(session),
        Err("device_not_found") => {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::revoke_sessions_for_device(&mut runtime, &session.device_id);
            Err(RemoteGatewayError::unauthorized("session_expired"))
        }
        Err(_) => Err(RemoteGatewayError::bad_request("device_store_failed")),
    }
}

async fn require_mutation_rate_limit<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    session: &RemoteSessionRecord,
    origin: &str,
    event_type: &str,
    action: &str,
) -> Result<(), RemoteGatewayError> {
    let state = ctx.app.state::<crate::state::AppState>();
    let mut runtime = state.remote_runtime.lock().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    match crate::remote::auth::check_rate_limit(
        &mut runtime,
        &format!("mutation:{}", session.session_id),
        now_ms,
        120,
        60_000,
    ) {
        Ok(()) => Ok(()),
        Err(_) => {
            audit_gateway_event(
                session,
                origin,
                GatewayAuditEvent::rejected(event_type, action, "rate_limited"),
            );
            Err(RemoteGatewayError::too_many_requests("rate_limited"))
        }
    }
}

async fn authorize_inbox_action<R: Runtime>(
    ctx: &RemoteGatewayContext<R>,
    headers: &HeaderMap,
    action: &str,
) -> Result<(String, RemoteSessionRecord), RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, headers, true, "inbox_action")?;
    let session =
        require_audited_remote_session(ctx, headers, &origin, "inbox_action", action).await?;
    require_mutation_rate_limit(ctx, &session, &origin, "inbox_action", action).await?;
    require_csrf_header(&session, headers)?;
    Ok((origin, session))
}

fn remote_session_cookie_value(headers: &HeaderMap) -> Result<String, RemoteGatewayError> {
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RemoteGatewayError::unauthorized("missing_session_cookie"))?;
    cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| {
            (name == REMOTE_SESSION_COOKIE_NAME && !value.is_empty()).then(|| value.to_string())
        })
        .ok_or_else(|| RemoteGatewayError::unauthorized("missing_session_cookie"))
}

fn remote_session_cookie_header_value(
    session_id: &str,
) -> Result<axum::http::HeaderValue, RemoteGatewayError> {
    axum::http::HeaderValue::from_str(&format!(
        "{REMOTE_SESSION_COOKIE_NAME}={session_id}; Path=/; Secure; HttpOnly; SameSite=Strict"
    ))
    .map_err(|_| RemoteGatewayError::bad_request("session_cookie_failed"))
}

fn session_response_from_record(session: &RemoteSessionRecord) -> AuthSessionResponse {
    AuthSessionResponse {
        csrf_nonce: session.csrf_nonce.clone(),
        expires_at: millis_to_rfc3339(session.expires_at_ms),
        absolute_expires_at: millis_to_rfc3339(session.absolute_expires_at_ms),
    }
}

fn pairing_response_from_pending_record(
    request: &PendingPairingRequestRecord,
) -> PairingSubmitResponse {
    let status = match request.decision {
        PendingPairingDecision::Pending | PendingPairingDecision::Approving => "pending",
        PendingPairingDecision::Approved => "approved",
        PendingPairingDecision::Rejected => "rejected",
    };
    PairingSubmitResponse {
        status: status.to_string(),
        pairing_request_id: request.request_id.clone(),
        device_id: request.device_id.clone(),
        public_key_fingerprint: request.public_key_fingerprint.clone(),
        paired_at: request.paired_at.clone(),
        expires_at: millis_to_rfc3339(request.expires_at_ms),
    }
}

fn client_pairing_response_from_pending_record(
    request: &PendingPairingRequestRecord,
) -> RemoteClientPairingSubmitResponse {
    let status = match request.decision {
        PendingPairingDecision::Pending | PendingPairingDecision::Approving => "pending",
        PendingPairingDecision::Approved => "approved",
        PendingPairingDecision::Rejected => "rejected",
    };
    RemoteClientPairingSubmitResponse {
        status: status.to_string(),
        pairing_request_id: request.request_id.clone(),
        device_id: request.device_id.clone(),
        public_key_fingerprint: request.public_key_fingerprint.clone(),
        expires_at: millis_to_rfc3339(request.expires_at_ms),
    }
}

fn require_csrf_header(
    session: &RemoteSessionRecord,
    headers: &HeaderMap,
) -> Result<(), RemoteGatewayError> {
    let submitted_nonce = headers
        .get(REMOTE_CSRF_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| RemoteGatewayError::unauthorized("csrf_failed"))?;
    if crate::remote::auth::csrf_nonce_matches(session, submitted_nonce) {
        return Ok(());
    }
    Err(RemoteGatewayError::unauthorized("csrf_failed"))
}

fn auth_signature_message(challenge: &crate::remote::models::AuthChallengeRecord) -> Vec<u8> {
    format!(
        "wardian.remote.auth.v1\norigin:{}\ndevice:{}\nchallenge:{}\nnonce:{}",
        challenge.canonical_origin, challenge.device_id, challenge.challenge_id, challenge.nonce
    )
    .into_bytes()
}

fn decode_base64_standard(value: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|error| error.to_string())
}

fn sanitize_device_label(value: &str) -> String {
    let mut label = String::new();
    let mut char_count = 0usize;
    for ch in value.trim().chars() {
        if ch.is_control() || is_bidirectional_control(ch) {
            continue;
        }
        let sanitized = if ch.is_whitespace() { ' ' } else { ch };
        if sanitized == ' ' && label.ends_with(' ') {
            continue;
        }
        if char_count >= 80 {
            break;
        }
        label.push(sanitized);
        char_count += 1;
    }
    let label = label.trim();
    if label.is_empty() {
        return "Paired device".to_string();
    }
    label.to_string()
}

fn is_bidirectional_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn active_remote_device(device_id: &str) -> Result<DeviceRecord, RemoteGatewayError> {
    let store = crate::remote::storage::load_device_store()
        .map_err(|_| RemoteGatewayError::bad_request("device_store_failed"))?;
    store
        .devices
        .into_iter()
        .find(|device| {
            device.device_id == device_id
                && device.revoked_at.is_none()
                && device.credential_kind == crate::remote::models::DeviceCredentialKind::Pwa
                && device.audience == crate::remote::models::RemoteCredentialAudience::Pwa
                && device.client_origin.is_none()
                && device.read_grants.is_empty()
        })
        .ok_or_else(|| RemoteGatewayError::not_found("device_not_found"))
}

/// Checks the persisted device authority off the async executor and outside the
/// runtime lock, so other processes' revocations govern later authorizations.
pub(super) async fn remote_device_authority_active(device_id: String) -> Result<(), &'static str> {
    tokio::task::spawn_blocking(move || {
        active_remote_device(&device_id)
            .map(|_| ())
            .map_err(|error| error.code)
    })
    .await
    .map_err(|_| "device_store_failed")?
}

fn check_runtime_rate_limit(
    runtime: &mut crate::remote::models::RemoteRuntimeState,
    key: &str,
    now_ms: i64,
    max_attempts: usize,
    window_ms: i64,
) -> Result<(), RemoteGatewayError> {
    crate::remote::auth::check_rate_limit(runtime, key, now_ms, max_attempts, window_ms)
        .map_err(|_| RemoteGatewayError::too_many_requests("rate_limited"))
}

fn gateway_static_error_code(code: &str) -> &'static str {
    match code {
        "pairing_offer_not_found" => "pairing_offer_not_found",
        "pairing_offer_used" => "pairing_offer_used",
        "pairing_offer_expired" => "pairing_offer_expired",
        "pairing_offer_invalid" => "pairing_offer_invalid",
        "auth_challenge_not_found" => "auth_challenge_not_found",
        "auth_challenge_used" => "auth_challenge_used",
        "auth_challenge_expired" => "auth_challenge_expired",
        "auth_challenge_mismatch" => "auth_challenge_mismatch",
        _ => "remote_gateway_error",
    }
}

fn canonical_origin_for_audit(config: &RemoteGatewayConfig) -> Option<String> {
    crate::remote::policy::CanonicalOrigin::parse(&config.canonical_origin)
        .ok()
        .map(|origin| origin.raw().to_string())
}

#[derive(Clone, Copy)]
struct GatewayAuditEvent<'a> {
    event_type: &'a str,
    action: &'a str,
    target_type: Option<&'a str>,
    target_id: Option<&'a str>,
    outcome: &'a str,
    error_code: Option<&'a str>,
}

impl<'a> GatewayAuditEvent<'a> {
    fn accepted(event_type: &'a str, action: &'a str) -> Self {
        Self {
            event_type,
            action,
            target_type: None,
            target_id: None,
            outcome: "accepted",
            error_code: None,
        }
    }

    fn rejected(event_type: &'a str, action: &'a str, error_code: &'a str) -> Self {
        Self {
            event_type,
            action,
            target_type: None,
            target_id: None,
            outcome: "rejected",
            error_code: Some(error_code),
        }
    }

    fn target(mut self, target_type: &'a str, target_id: &'a str) -> Self {
        self.target_type = Some(target_type);
        self.target_id = Some(target_id);
        self
    }
}

fn audit_gateway_event(session: &RemoteSessionRecord, origin: &str, event: GatewayAuditEvent<'_>) {
    let record = build_gateway_audit_record(session, origin, event);
    if let Err(error) = crate::remote::audit::append_audit_record(&record) {
        crate::utils::logging::log_debug(&format!("[Wardian] remote audit append failed: {error}"));
    }
}

fn audit_client_gateway_event(
    session: &RemoteClientSessionRecord,
    origin: &str,
    event: GatewayAuditEvent<'_>,
) {
    let record = RemoteAuditRecord {
        schema_version: REMOTE_AUDIT_SCHEMA_VERSION,
        event_id: crate::remote::crypto::random_url_token(12),
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        request_id: crate::remote::crypto::random_url_token(12),
        device_id: Some(session.device_id.clone()),
        session_id: Some(session.session_id.clone()),
        origin: Some(origin.to_string()),
        event_type: event.event_type.to_string(),
        action: event.action.to_string(),
        target_type: event.target_type.map(str::to_string),
        target_id: event.target_id.map(str::to_string),
        outcome: event.outcome.to_string(),
        error_code: event.error_code.map(str::to_string),
    };
    if crate::remote::audit::append_audit_record(&record).is_err() {
        crate::utils::logging::log_debug("[Wardian] remote client audit append failed");
    }
}

fn audit_gateway_event_without_session(origin: Option<&str>, event: GatewayAuditEvent<'_>) {
    let record = build_gateway_rejection_audit_record(origin, event);
    if let Err(error) = crate::remote::audit::append_audit_record(&record) {
        crate::utils::logging::log_debug(&format!("[Wardian] remote audit append failed: {error}"));
    }
}

fn build_gateway_audit_record(
    session: &RemoteSessionRecord,
    origin: &str,
    event: GatewayAuditEvent<'_>,
) -> RemoteAuditRecord {
    RemoteAuditRecord {
        schema_version: REMOTE_AUDIT_SCHEMA_VERSION,
        event_id: crate::remote::crypto::random_url_token(12),
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        request_id: crate::remote::crypto::random_url_token(12),
        device_id: Some(session.device_id.clone()),
        session_id: Some(session.session_id.clone()),
        origin: Some(origin.to_string()),
        event_type: event.event_type.to_string(),
        action: event.action.to_string(),
        target_type: event.target_type.map(str::to_string),
        target_id: event.target_id.map(str::to_string),
        outcome: event.outcome.to_string(),
        error_code: event.error_code.map(str::to_string),
    }
}

fn build_gateway_rejection_audit_record(
    origin: Option<&str>,
    event: GatewayAuditEvent<'_>,
) -> RemoteAuditRecord {
    RemoteAuditRecord {
        schema_version: REMOTE_AUDIT_SCHEMA_VERSION,
        event_id: crate::remote::crypto::random_url_token(12),
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        request_id: crate::remote::crypto::random_url_token(12),
        device_id: None,
        session_id: None,
        origin: origin.map(str::to_string),
        event_type: event.event_type.to_string(),
        action: event.action.to_string(),
        target_type: event.target_type.map(str::to_string),
        target_id: event.target_id.map(str::to_string),
        outcome: event.outcome.to_string(),
        error_code: event.error_code.map(str::to_string),
    }
}

fn millis_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Debug)]
struct RemoteGatewayError {
    status: StatusCode,
    code: &'static str,
    detail: Option<String>,
}

impl RemoteGatewayError {
    fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            detail: None,
        }
    }

    /// Carries an operator-readable reason (e.g. a delivery failure message)
    /// alongside the stable machine code so remote clients can distinguish
    /// "agent busy" from "agent unreachable" instead of guessing.
    fn bad_request_with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            detail: Some(detail.into()),
        }
    }

    fn unauthorized(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
            detail: None,
        }
    }

    fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            detail: None,
        }
    }

    fn too_many_requests(code: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code,
            detail: None,
        }
    }

    fn service_unavailable(code: &'static str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            detail: None,
        }
    }

    fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            detail: None,
        }
    }

    fn conflict(code: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            detail: None,
        }
    }

    fn gone(code: &'static str) -> Self {
        Self {
            status: StatusCode::GONE,
            code,
            detail: None,
        }
    }
}

impl IntoResponse for RemoteGatewayError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({
            "ok": false,
            "code": self.code,
        });
        if let Some(detail) = self
            .detail
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["detail"] = serde_json::Value::String(detail.to_string());
        }
        let body = axum::Json(body);
        (self.status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{
        DeviceCredentialKind, DeviceRecord, RemoteCredentialAudience, RemoteGatewayConfig,
        RemoteReadGrant, RemoteSessionRecord, REMOTE_AUDIT_SCHEMA_VERSION,
        REMOTE_SETTINGS_SCHEMA_VERSION,
    };
    use crate::state::AppState;
    use axum::http::{header, HeaderMap, HeaderValue};
    use tauri::Manager;

    struct WardianHomeEnvGuard {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
    }

    impl WardianHomeEnvGuard {
        async fn set_async(home: &std::path::Path) -> Self {
            let guard = Self {
                _lock: crate::utils::wardian_test_env_lock_async().await,
                previous_home: std::env::var_os("WARDIAN_HOME"),
            };
            std::env::set_var("WARDIAN_HOME", home);
            guard
        }
    }

    impl Drop for WardianHomeEnvGuard {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn config() -> RemoteGatewayConfig {
        RemoteGatewayConfig {
            schema_version: REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://wardian.tailnet.ts.net".to_string(),
            client_origin_allowlist: Vec::new(),
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 0,
            gateway_identity_public_key: "pub".to_string(),
            gateway_identity_fingerprint: "fp".to_string(),
        }
    }

    fn source_observation_id(offset: u64) -> String {
        format!(
            "source:agent-1:{}:{offset}:{}:0",
            "a".repeat(64),
            "b".repeat(64)
        )
    }

    fn encode_query_component(value: &str) -> String {
        super::encode_remote_chat_query_component(value)
    }

    #[test]
    fn remote_chat_query_decodes_one_json_array_and_rejects_repeated_keys() {
        let first = source_observation_id(10);
        let second = source_observation_id(20);
        let wire_ids = vec![first.clone(), first.clone(), second.clone()];
        let wire_value = serde_json::to_string(&wire_ids).unwrap();
        let uri: axum::http::Uri = format!(
            "/remote/api/agents/agent-1/chat?revalidate_ids={}",
            encode_query_component(&wire_value)
        )
        .parse()
        .unwrap();
        let query = parse_remote_chat_query(uri.query())
            .expect("the existing Axum GET query accepts the JSON-list encoding");
        let decoded = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref())
            .expect("canonical JSON list decodes");
        let validated =
            crate::state::conversation_archive::chat_read::validate_revalidation_request(
                "agent-1",
                query.before.as_deref(),
                Some("current-revision"),
                query.detail.as_deref(),
                decoded,
            )
            .expect("the shared validator accepts and deduplicates backend-shaped IDs");
        assert_eq!(validated, Some(vec![first, second]));

        let noncanonical_value: axum::http::Uri =
            "/remote/api/agents/agent-1/chat?revalidate_ids=%5b%5d"
                .parse()
                .unwrap();
        assert!(parse_remote_chat_query(noncanonical_value.query()).is_err());
        let encoded_key: axum::http::Uri =
            "/remote/api/agents/agent-1/chat?%72evalidate_ids=%5B%5D"
                .parse()
                .unwrap();
        assert!(parse_remote_chat_query(encoded_key.query()).is_err());

        let eight_raw_duplicates =
            serde_json::to_string(&vec![source_observation_id(10); 8]).unwrap();
        let eight_uri: axum::http::Uri = format!(
            "/remote/api/agents/agent-1/chat?revalidate_ids={}",
            encode_query_component(&eight_raw_duplicates)
        )
        .parse()
        .unwrap();
        let query = parse_remote_chat_query(eight_uri.query()).unwrap();
        let decoded = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref()).unwrap();
        assert_eq!(
            crate::state::conversation_archive::chat_read::validate_revalidation_request(
                "agent-1",
                None,
                Some("current-revision"),
                None,
                decoded,
            )
            .unwrap(),
            Some(vec![source_observation_id(10)])
        );

        let nine_raw_duplicates =
            serde_json::to_string(&vec![source_observation_id(10); 9]).unwrap();
        let nine_uri: axum::http::Uri = format!(
            "/remote/api/agents/agent-1/chat?revalidate_ids={}",
            encode_query_component(&nine_raw_duplicates)
        )
        .parse()
        .unwrap();
        let query = parse_remote_chat_query(nine_uri.query()).unwrap();
        let decoded = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref()).unwrap();
        assert!(
            crate::state::conversation_archive::chat_read::validate_revalidation_request(
                "agent-1",
                None,
                Some("current-revision"),
                None,
                decoded,
            )
            .is_err()
        );

        let invalid_id = serde_json::to_string(&["not-a-source-id"]).unwrap();
        let invalid_id_uri: axum::http::Uri = format!(
            "/remote/api/agents/agent-1/chat?revalidate_ids={}",
            encode_query_component(&invalid_id)
        )
        .parse()
        .unwrap();
        let query = parse_remote_chat_query(invalid_id_uri.query()).unwrap();
        let decoded = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref()).unwrap();
        assert!(
            crate::state::conversation_archive::chat_read::validate_revalidation_request(
                "agent-1",
                None,
                Some("current-revision"),
                None,
                decoded,
            )
            .is_err()
        );

        let oversized_id = format!("{}{}", source_observation_id(10), "x".repeat(512));
        let oversized_json = serde_json::to_string(&[oversized_id]).unwrap();
        let oversized_uri: axum::http::Uri = format!(
            "/remote/api/agents/agent-1/chat?revalidate_ids={}",
            encode_query_component(&oversized_json)
        )
        .parse()
        .unwrap();
        let query = parse_remote_chat_query(oversized_uri.query()).unwrap();
        let decoded = decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref()).unwrap();
        assert!(
            crate::state::conversation_archive::chat_read::validate_revalidation_request(
                "agent-1",
                None,
                Some("current-revision"),
                None,
                decoded,
            )
            .is_err()
        );
        let over_limit_json = format!(
            "\"{}\"",
            "x".repeat(REMOTE_CHAT_REVALIDATION_JSON_MAX_BYTES + 1)
        );
        assert!(decode_remote_chat_revalidation_ids(Some(&over_limit_json)).is_err());

        let repeated_key: axum::http::Uri =
            "/remote/api/agents/agent-1/chat?revalidate_ids=%5B%5D&revalidate_ids=%5B%5D"
                .parse()
                .unwrap();
        assert!(parse_remote_chat_query(repeated_key.query()).is_err());

        let oversized_query = format!(
            "revalidate_ids={}",
            "x".repeat(REMOTE_CHAT_QUERY_MAX_BYTES + 1)
        );
        assert!(parse_remote_chat_query(Some(&oversized_query)).is_err());

        let malformed_json: axum::http::Uri =
            "/remote/api/agents/agent-1/chat?revalidate_ids=not-json"
                .parse()
                .unwrap();
        let query = parse_remote_chat_query(malformed_json.query()).unwrap();
        assert!(decode_remote_chat_revalidation_ids(query.revalidate_ids.as_deref()).is_err());
    }

    fn client_config() -> RemoteGatewayConfig {
        let mut config = config();
        config.client_origin_allowlist = vec!["https://client.example".to_string()];
        config
    }

    #[test]
    fn paired_client_auth_responses_match_the_frozen_v2_json_fields() {
        let challenge = serde_json::to_value(RemoteClientAuthChallengeResponse {
            challenge_id: "challenge-id".to_string(),
            device_id: "device-id".to_string(),
            origin: "https://gateway.example".to_string(),
            client_origin: "https://client.example".to_string(),
            server_identity_fingerprint: "server-fingerprint".to_string(),
            nonce: "nonce".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            audience: "wardian_remote_client_v1".to_string(),
            signing_payload: "payload".to_string(),
        })
        .expect("challenge response JSON");
        assert_eq!(
            challenge,
            serde_json::json!({
                "challenge_id": "challenge-id",
                "device_id": "device-id",
                "origin": "https://gateway.example",
                "client_origin": "https://client.example",
                "server_identity_fingerprint": "server-fingerprint",
                "nonce": "nonce",
                "expires_at": "2030-01-01T00:00:00Z",
                "audience": "wardian_remote_client_v1",
                "signing_payload": "payload"
            })
        );

        let token = serde_json::to_value(RemoteClientAuthSessionResponse {
            token_type: "Bearer".to_string(),
            access_token: "opaque-token".to_string(),
            expires_at: "2030-01-01T00:15:00Z".to_string(),
            device_id: "device-id".to_string(),
            scopes: vec![RemoteClientScope::AgentsRead, RemoteClientScope::ChatRead],
            idempotency_namespace: None,
            idempotency_expires_at: None,
        })
        .expect("token response JSON");
        assert_eq!(
            token,
            serde_json::json!({
                "token_type": "Bearer",
                "access_token": "opaque-token",
                "expires_at": "2030-01-01T00:15:00Z",
                "device_id": "device-id",
                "scopes": ["agents:read", "chat:read"]
            })
        );

        let pairing = serde_json::to_value(RemoteClientPairingSubmitResponse {
            status: "pending".to_string(),
            pairing_request_id: "request-id".to_string(),
            device_id: "device-id".to_string(),
            public_key_fingerprint: "fingerprint".to_string(),
            expires_at: "2030-01-01T00:02:00Z".to_string(),
        })
        .expect("pairing response JSON");
        assert_eq!(
            pairing,
            serde_json::json!({
                "status": "pending",
                "pairing_request_id": "request-id",
                "device_id": "device-id",
                "public_key_fingerprint": "fingerprint",
                "expires_at": "2030-01-01T00:02:00Z"
            })
        );

        let pwa_pairing = serde_json::to_value(PairingSubmitResponse {
            status: "pending".to_string(),
            pairing_request_id: "pwa-request-id".to_string(),
            device_id: "pwa-device-id".to_string(),
            public_key_fingerprint: "pwa-fingerprint".to_string(),
            paired_at: None,
            expires_at: "2030-01-01T00:02:00Z".to_string(),
        })
        .expect("PWA pairing response JSON");
        assert_eq!(
            pwa_pairing,
            serde_json::json!({
                "status": "pending",
                "pairing_request_id": "pwa-request-id",
                "device_id": "pwa-device-id",
                "public_key_fingerprint": "pwa-fingerprint",
                "paired_at": null,
                "expires_at": "2030-01-01T00:02:00Z"
            })
        );
    }

    #[test]
    fn paired_client_errors_use_only_stable_contract_codes() {
        assert_eq!(
            normalize_client_error(StatusCode::BAD_REQUEST, "client_origin_forbidden"),
            (StatusCode::FORBIDDEN, "origin_forbidden")
        );
        assert_eq!(
            normalize_client_error(StatusCode::BAD_REQUEST, "remote_device_revoked"),
            (StatusCode::FORBIDDEN, "device_revoked")
        );
        assert_eq!(
            normalize_client_error(StatusCode::BAD_REQUEST, "auth_challenge_used"),
            (StatusCode::UNAUTHORIZED, "invalid_challenge")
        );
        assert_eq!(
            normalize_client_error(StatusCode::BAD_REQUEST, "auth_challenge_expired"),
            (StatusCode::UNAUTHORIZED, "challenge_expired")
        );
        assert_eq!(
            normalize_client_error(StatusCode::BAD_REQUEST, "invalid_auth_signature"),
            (StatusCode::UNAUTHORIZED, "invalid_auth_signature")
        );
        assert_eq!(
            normalize_client_error(StatusCode::UNAUTHORIZED, "client_token_required"),
            (StatusCode::UNAUTHORIZED, "auth_required")
        );
        assert_eq!(
            normalize_client_error(StatusCode::UNPROCESSABLE_ENTITY, "framework rejection"),
            (StatusCode::BAD_REQUEST, "invalid_request")
        );
        assert_eq!(
            normalize_client_error(StatusCode::INTERNAL_SERVER_ERROR, "internal detail"),
            (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable")
        );
        let rate_limited = client_error_response(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
        assert_eq!(rate_limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(rate_limited.headers()[header::RETRY_AFTER], "60");
    }

    fn client_request(method: Method, path: &str, origin: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "wardian.tailnet.ts.net")
            .header(header::ORIGIN, origin)
            .body(body)
            .expect("valid test request")
    }

    fn client_bearer_request(path: &str, origin: &str, token: &str) -> Request<Body> {
        let mut request = client_request(Method::GET, path, origin, Body::empty());
        let authorization = format!("Bearer {token}");
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_bytes(authorization.as_bytes())
                .expect("valid synthetic bearer header"),
        );
        request
    }

    fn seed_client_device(
        home: &std::path::Path,
        device_id: &str,
        public_key_spki_der_base64: &str,
        public_key_fingerprint: &str,
    ) {
        crate::remote::storage::update_device_store_at(home, |store| {
            store.devices.push(DeviceRecord {
                device_id: device_id.to_string(),
                label: "Companion".to_string(),
                public_key_spki_der_base64: public_key_spki_der_base64.to_string(),
                public_key_fingerprint: public_key_fingerprint.to_string(),
                credential_kind: DeviceCredentialKind::PairedClient,
                audience: RemoteCredentialAudience::PairedClientV1,
                client_origin: Some("https://client.example".to_string()),
                read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
                send_grants: Vec::new(),
                created_at: "2026-10-29T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("seed client device");
    }

    async fn seed_client_access_token(
        state: &AppState,
        device_id: &str,
        client_origin: &str,
        read_grants: Vec<RemoteReadGrant>,
        suffix: char,
    ) -> String {
        let token = format!("{}{}", "x".repeat(42), suffix);
        let token_verifier = crate::remote::crypto::sha256_token_verifier(&token);
        let now_ms = chrono::Utc::now().timestamp_millis();
        state.remote_runtime.lock().await.client_sessions.insert(
            token_verifier.clone(),
            RemoteClientSessionRecord {
                session_id: format!("synthetic-audit-{suffix}"),
                device_id: device_id.to_string(),
                client_origin: client_origin.to_string(),
                gateway_origin: "https://wardian.tailnet.ts.net".to_string(),
                audience: RemoteCredentialAudience::PairedClientV1,
                gateway_identity_fingerprint: "fp".to_string(),
                read_grants,
                send_grants: Vec::new(),
                idempotency_namespace: None,
                idempotency_expires_at_ms: None,
                expires_at_ms: now_ms + 15 * 60 * 1000,
                revoked: false,
            },
        );
        token
    }

    fn seed_active_remote_devices(home: &std::path::Path, device_ids: &[&str]) {
        crate::remote::storage::update_device_store_at(home, |store| {
            for device_id in device_ids {
                store.devices.push(DeviceRecord {
                    device_id: (*device_id).to_string(),
                    label: format!("Test device {device_id}"),
                    public_key_spki_der_base64: format!("key-{device_id}"),
                    public_key_fingerprint: format!("fingerprint-{device_id}"),
                    credential_kind: DeviceCredentialKind::Pwa,
                    audience: RemoteCredentialAudience::Pwa,
                    client_origin: None,
                    read_grants: Vec::new(),
                    send_grants: Vec::new(),
                    created_at: "2026-10-29T00:00:00Z".to_string(),
                    last_used_at: None,
                    revoked_at: None,
                });
            }
            Ok(())
        })
        .expect("seed active remote devices");
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

    #[tokio::test]
    async fn client_pairing_status_uses_inner_request_id_from_router() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let config = client_config();
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save gateway config");

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut runtime = state.remote_runtime.lock().await;
        for (request_id, fingerprint) in [
            ("outer-request", "outer-fingerprint"),
            ("selected-request", "selected-fingerprint"),
        ] {
            runtime.pending_pairing_requests.insert(
                request_id.to_string(),
                PendingPairingRequestRecord {
                    request_id: request_id.to_string(),
                    pairing_offer_id: format!("offer-{request_id}"),
                    device_id: format!("device-{request_id}"),
                    device_label: format!("Device {request_id}"),
                    public_key_spki_der_base64: "cHVi".to_string(),
                    public_key_fingerprint: fingerprint.to_string(),
                    canonical_origin: config.canonical_origin.clone(),
                    credential_kind: DeviceCredentialKind::PairedClient,
                    audience: RemoteCredentialAudience::PairedClientV1,
                    client_origin: Some("https://client.example".to_string()),
                    read_grants: Vec::new(),
                    send_grants: Vec::new(),
                    submitted_at_ms: now_ms,
                    expires_at_ms: now_ms + 60_000,
                    decision: PendingPairingDecision::Pending,
                    paired_at: None,
                },
            );
        }
        drop(runtime);

        let router = client_namespace_router(RemoteGatewayContext {
            app: app.handle().clone(),
            config,
        });
        let response = router
            .oneshot(client_request(
                Method::GET,
                "/remote/client/v1/pairing/selected-request",
                "https://client.example",
                Body::empty(),
            ))
            .await
            .expect("selected client pairing request response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096)
            .await
            .expect("selected pairing response body");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("selected pairing response JSON");
        assert_eq!(payload["pairing_request_id"], "selected-request");
        assert_eq!(payload["public_key_fingerprint"], "selected-fingerprint");
        assert_eq!(payload["status"], "pending");
    }

    #[tokio::test]
    async fn read_only_client_token_cannot_admit_message_send() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let config = client_config();
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save gateway config");
        seed_client_device(temp.path(), "client-device", "cHVi", "client-fp");

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let token = seed_client_access_token(
            &state,
            "client-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            'r',
        )
        .await;
        let router = client_namespace_router(RemoteGatewayContext {
            app: app.handle().clone(),
            config,
        });
        let mut request = client_request(
            Method::POST,
            "/remote/client/v1/agents/agent-1/messages",
            "https://client.example",
            Body::from(r#"{"text":"hello","input_mode":"message"}"#),
        );
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("bearer header"),
        );
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        request.headers_mut().insert(
            "idempotency-key",
            HeaderValue::from_static("namespace.namespace-request"),
        );

        let response = router
            .oneshot(request)
            .await
            .expect("message-send response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("stable grant error body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("error JSON"),
            serde_json::json!({ "error": "grant_required" })
        );
    }

    #[tokio::test]
    async fn client_namespace_preflight_and_framework_errors_are_scoped_and_no_store() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let config = client_config();
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save gateway config");
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let router = client_namespace_router(RemoteGatewayContext {
            app: app.handle().clone(),
            config: config.clone(),
        });

        // Browser fetch cannot send a GET body. Exercise the malformed HTTP
        // request directly without exporting any browser-held bearer token.
        let with_body = router
            .clone()
            .oneshot(client_request(
                Method::GET,
                "/remote/client/v1/message-operations/by-key",
                "https://client.example",
                Body::from("{}"),
            ))
            .await
            .expect("GET body rejection");
        assert_eq!(with_body.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(with_body.into_body(), 4096)
            .await
            .expect("rejection body");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"error": "invalid_request"})
        );

        let mut preflight = client_request(
            Method::OPTIONS,
            "/remote/client/v1/auth/challenge",
            "https://client.example",
            Body::empty(),
        );
        preflight.headers_mut().insert(
            "access-control-request-method",
            HeaderValue::from_static("POST"),
        );
        preflight.headers_mut().insert(
            "access-control-request-headers",
            HeaderValue::from_static("authorization, content-type, idempotency-key"),
        );
        let response = router
            .clone()
            .oneshot(preflight)
            .await
            .expect("preflight response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_METHODS],
            "GET, POST, OPTIONS"
        );
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "Authorization, Content-Type, Idempotency-Key"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS));

        let mut chat_preflight = client_request(
            Method::OPTIONS,
            "/remote/client/v1/agents/agent-1/chat",
            "https://client.example",
            Body::empty(),
        );
        chat_preflight.headers_mut().insert(
            "access-control-request-method",
            HeaderValue::from_static("GET"),
        );
        chat_preflight.headers_mut().insert(
            "access-control-request-headers",
            HeaderValue::from_static("authorization"),
        );
        let response = router
            .clone()
            .oneshot(chat_preflight)
            .await
            .expect("client chat preflight response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        assert!(!response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS));
        let mut chat_post_preflight = client_request(
            Method::OPTIONS,
            "/remote/client/v1/agents/agent-1/chat",
            "https://client.example",
            Body::empty(),
        );
        chat_post_preflight.headers_mut().insert(
            "access-control-request-method",
            HeaderValue::from_static("POST"),
        );
        let response = router
            .clone()
            .oneshot(chat_post_preflight)
            .await
            .expect("rejected client chat method preflight");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");

        let mut disallowed_headers = client_request(
            Method::OPTIONS,
            "/remote/client/v1/auth/challenge",
            "https://client.example",
            Body::empty(),
        );
        disallowed_headers.headers_mut().insert(
            "access-control-request-method",
            HeaderValue::from_static("POST"),
        );
        disallowed_headers.headers_mut().insert(
            "access-control-request-headers",
            HeaderValue::from_static("cookie"),
        );
        let response = router
            .clone()
            .oneshot(disallowed_headers)
            .await
            .expect("rejected preflight response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let preflight_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("preflight error body"),
        )
        .expect("JSON preflight error body");
        assert_eq!(
            preflight_error,
            serde_json::json!({ "error": "invalid_request" })
        );

        let response = router
            .clone()
            .oneshot(client_request(
                Method::GET,
                "/remote/client/v1/unknown",
                "https://client.example",
                Body::empty(),
            ))
            .await
            .expect("unknown path response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("error body"),
        )
        .expect("JSON not-found body");
        assert_eq!(error["error"], "not_found");

        let legacy_session = router
            .clone()
            .oneshot(client_request(
                Method::POST,
                "/remote/client/v1/auth/session",
                "https://client.example",
                Body::empty(),
            ))
            .await
            .expect("legacy auth session path response");
        assert_eq!(legacy_session.status(), StatusCode::NOT_FOUND);
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(legacy_session.into_body(), 4096)
                .await
                .expect("legacy path error body"),
        )
        .expect("JSON legacy path error body");
        assert_eq!(error, serde_json::json!({ "error": "not_found" }));

        let response = router
            .clone()
            .oneshot(client_request(
                Method::PUT,
                "/remote/client/v1/auth/challenge",
                "https://client.example",
                Body::empty(),
            ))
            .await
            .expect("unsupported method response");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("error body"),
        )
        .expect("JSON method error body");
        assert_eq!(error["error"], "method_not_allowed");

        let mut at_limit = client_request(
            Method::POST,
            "/remote/client/v1/agents/agent-1/messages",
            "https://client.example",
            Body::from(vec![b'x'; 512 * 1024]),
        );
        at_limit.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let response = router
            .clone()
            .oneshot(at_limit)
            .await
            .expect("at-limit malformed JSON response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("at-limit JSON error body"),
        )
        .expect("at-limit JSON error response");
        assert_eq!(error, serde_json::json!({ "error": "invalid_request" }));
        let mut oversized = client_request(
            Method::POST,
            "/remote/client/v1/auth/challenge",
            "https://client.example",
            Body::from(vec![b'x'; 70 * 1024]),
        );
        oversized.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let response = router
            .clone()
            .oneshot(oversized)
            .await
            .expect("oversized request response");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("error body"),
        )
        .expect("JSON payload error body");
        assert_eq!(error["error"], "request_too_large");

        let mut message_oversized = client_request(
            Method::POST,
            "/remote/client/v1/agents/agent-1/messages",
            "https://client.example",
            Body::from(vec![b'x'; 512 * 1024 + 1]),
        );
        message_oversized.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let response = router
            .clone()
            .oneshot(message_oversized)
            .await
            .expect("oversized message response");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("oversized message error body"),
        )
        .expect("JSON oversized message error body");
        assert_eq!(error["error"], "request_too_large");

        let mut cookie_only = client_request(
            Method::GET,
            "/remote/client/v1/sharing",
            "https://client.example",
            Body::empty(),
        );
        cookie_only.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-wardian_remote_session=pwa-session"),
        );
        let response = router
            .oneshot(cookie_only)
            .await
            .expect("cookie-only client request");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("error body"),
        )
        .expect("JSON cookie error body");
        assert_eq!(error["error"], "auth_required");
        let runtime = app.state::<AppState>();
        assert!(runtime
            .remote_runtime
            .lock()
            .await
            .client_auth_challenges
            .is_empty());
        assert!(runtime
            .remote_runtime
            .lock()
            .await
            .client_sessions
            .is_empty());
    }

    #[tokio::test]
    async fn client_chat_route_rechecks_token_scope_origin_kind_and_revocation() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let mut config = client_config();
        config
            .client_origin_allowlist
            .push("https://other.example".to_string());
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save gateway config");
        seed_client_device(temp.path(), "client-device", "cHVi", "client-fp");
        seed_active_remote_devices(temp.path(), &["pwa-device"]);

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let router = client_namespace_router(RemoteGatewayContext {
            app: app.handle().clone(),
            config,
        });
        let path = "/remote/client/v1/agents/agent-1/chat";

        let mut cookie_only =
            client_request(Method::GET, path, "https://client.example", Body::empty());
        cookie_only.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-wardian_remote_session=pwa-session"),
        );
        let response = router
            .clone()
            .oneshot(cookie_only)
            .await
            .expect("cookie-only Chat response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let cookie_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("cookie error body"),
        )
        .expect("cookie error JSON");
        assert_eq!(cookie_error["error"], "auth_required");

        let original_client_device = crate::remote::storage::load_device_store_at(temp.path())
            .expect("load primary client device before read-only fixture")
            .devices
            .into_iter()
            .find(|device| device.device_id == "client-device")
            .expect("primary client device remains present");
        assert_eq!(
            original_client_device.read_grants,
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead]
        );

        let read_only_client_device = DeviceRecord {
            device_id: "read-only-client-device".to_string(),
            label: "Read-only Companion".to_string(),
            public_key_spki_der_base64: "cHVi".to_string(),
            public_key_fingerprint: "read-only-client-fp".to_string(),
            credential_kind: DeviceCredentialKind::PairedClient,
            audience: RemoteCredentialAudience::PairedClientV1,
            client_origin: Some("https://client.example".to_string()),
            read_grants: vec![RemoteReadGrant::AgentsRead],
            send_grants: Vec::new(),
            created_at: "2026-10-29T00:00:00Z".to_string(),
            last_used_at: None,
            revoked_at: None,
        };
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            if store
                .devices
                .iter()
                .any(|device| device.device_id == "read-only-client-device")
            {
                return Err("read-only client fixture already exists".to_string());
            }
            store.devices.push(read_only_client_device.clone());
            Ok(())
        })
        .expect("insert approved read-only client device");

        let device_store_after_fixture = crate::remote::storage::load_device_store_at(temp.path())
            .expect("load client devices after read-only fixture insertion");
        assert_eq!(
            device_store_after_fixture
                .devices
                .iter()
                .find(|device| device.device_id == original_client_device.device_id),
            Some(&original_client_device),
            "read-only fixture insertion must preserve the primary client device"
        );
        assert_eq!(
            device_store_after_fixture
                .devices
                .iter()
                .find(|device| device.device_id == read_only_client_device.device_id),
            Some(&read_only_client_device),
            "read-only fixture must be a distinct client with only AgentsRead"
        );

        let no_chat_grant = seed_client_access_token(
            &state,
            "read-only-client-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead],
            'a',
        )
        .await;
        let response = router
            .clone()
            .oneshot(client_bearer_request(
                path,
                "https://client.example",
                &no_chat_grant,
            ))
            .await
            .expect("missing Chat grant response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("missing Chat grant error body"),
        )
        .expect("missing Chat grant error JSON");
        assert_eq!(error, serde_json::json!({ "error": "grant_required" }));

        let primary_after_missing_grant = crate::remote::storage::load_device_store_at(temp.path())
            .expect("load primary client device after missing-Chat check")
            .devices
            .into_iter()
            .find(|device| device.device_id == "client-device")
            .expect("primary client device remains present after missing-Chat check");
        assert_eq!(
            primary_after_missing_grant, original_client_device,
            "missing-Chat fixture must leave the primary full-grant device unchanged"
        );

        let wrong_origin_token = seed_client_access_token(
            &state,
            "client-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            'b',
        )
        .await;
        let response = router
            .clone()
            .oneshot(client_bearer_request(
                path,
                "https://other.example",
                &wrong_origin_token,
            ))
            .await
            .expect("wrong bound origin response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let pwa_device_token = seed_client_access_token(
            &state,
            "pwa-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            'c',
        )
        .await;
        let response = router
            .clone()
            .oneshot(client_bearer_request(
                path,
                "https://client.example",
                &pwa_device_token,
            ))
            .await
            .expect("PWA device on client Chat route");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let pwa_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("foreign PWA client-route error body"),
        )
        .expect("foreign PWA client-route error JSON");
        assert_eq!(pwa_error, serde_json::json!({ "error": "not_found" }));

        let unknown_device_token = seed_client_access_token(
            &state,
            "unknown-client-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            'f',
        )
        .await;
        let response = router
            .clone()
            .oneshot(client_bearer_request(
                path,
                "https://client.example",
                &unknown_device_token,
            ))
            .await
            .expect("unknown client device response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let unknown_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("unknown client device error body"),
        )
        .expect("unknown client device error JSON");
        assert_eq!(unknown_error, serde_json::json!({ "error": "not_found" }));

        let revoked_token = seed_client_access_token(
            &state,
            "client-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            'd',
        )
        .await;
        crate::remote::auth::revoke_device_and_sessions(
            state.remote_device_store_lock.clone(),
            &state.remote_runtime,
            "client-device".to_string(),
            chrono::Utc::now().to_rfc3339(),
        )
        .await
        .expect("revoke client device");
        let response = router
            .oneshot(client_bearer_request(
                path,
                "https://client.example",
                &revoked_token,
            ))
            .await
            .expect("revoked client Chat response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let revoked_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 4096)
                .await
                .expect("revocation error body"),
        )
        .expect("revocation error JSON");
        assert_eq!(revoked_error["error"], "device_revoked");
    }

    #[tokio::test]
    async fn pwa_and_client_credentials_are_rejected_by_both_auth_namespaces() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let config = client_config();
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save gateway config");
        seed_active_remote_devices(temp.path(), &["pwa-device"]);
        seed_client_device(temp.path(), "client-device", "cHVi", "client-fp");
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let ctx = RemoteGatewayContext {
            app: app.handle().clone(),
            config: config.clone(),
        };
        let client_router = client_namespace_router(ctx.clone());

        for path in [
            "/remote/client/v1/auth/challenge",
            "/remote/client/v1/auth/token",
        ] {
            let request_body = if path.ends_with("challenge") {
                serde_json::json!({ "device_id": "pwa-device" })
            } else {
                serde_json::json!({
                    "challenge_id": "unused-client-challenge",
                    "device_id": "pwa-device",
                    "signature_der_base64": "AA=="
                })
            };
            let mut request = client_request(
                Method::POST,
                path,
                "https://client.example",
                Body::from(request_body.to_string()),
            );
            request.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            let response = client_router
                .clone()
                .oneshot(request)
                .await
                .expect("client endpoint response");
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        let mut pwa_headers = HeaderMap::new();
        pwa_headers.insert(
            header::HOST,
            HeaderValue::from_static("wardian.tailnet.ts.net"),
        );
        pwa_headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://wardian.tailnet.ts.net"),
        );
        pwa_headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        let challenge_error = create_auth_challenge(
            State(ctx.clone()),
            pwa_headers.clone(),
            Json(AuthChallengeRequest {
                device_id: "client-device".to_string(),
            }),
        )
        .await
        .expect_err("PWA challenge rejects paired-client credential");
        assert_eq!(challenge_error.code, "device_not_found");
        let session_error = create_auth_session(
            State(ctx),
            pwa_headers,
            Json(AuthSessionRequest {
                challenge_id: "unused-pwa-challenge".to_string(),
                device_id: "client-device".to_string(),
                signature_der_base64: "AA==".to_string(),
            }),
        )
        .await
        .expect_err("PWA session rejects paired-client credential");
        assert_eq!(session_error.code, "device_not_found");

        let state = app.state::<AppState>();
        let runtime = state.remote_runtime.lock().await;
        assert!(runtime.auth_challenges.is_empty());
        assert!(runtime.client_auth_challenges.is_empty());
        assert!(runtime.sessions.is_empty());
        assert!(runtime.client_sessions.is_empty());
    }

    #[tokio::test]
    async fn concurrent_client_auth_submissions_issue_one_memory_only_bearer() {
        use base64::Engine as _;
        use p256::{
            ecdsa::{signature::Signer, Signature, SigningKey},
            pkcs8::EncodePublicKey,
        };

        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let config = client_config();
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save gateway config");
        let signing_key = SigningKey::from_bytes((&[9_u8; 32]).into()).expect("signing key");
        let public_key_der = signing_key
            .verifying_key()
            .to_public_key_der()
            .expect("SPKI public key");
        let public_key_base64 =
            base64::engine::general_purpose::STANDARD.encode(public_key_der.as_bytes());
        let public_key_fingerprint =
            crate::remote::crypto::sha256_fingerprint(public_key_der.as_bytes());
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            store.devices.push(DeviceRecord {
                device_id: "client-device".to_string(),
                label: "Companion".to_string(),
                public_key_spki_der_base64: public_key_base64.clone(),
                public_key_fingerprint: public_key_fingerprint.clone(),
                credential_kind: DeviceCredentialKind::PairedClient,
                audience: RemoteCredentialAudience::PairedClientV1,
                client_origin: Some("https://client.example".to_string()),
                read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
                send_grants: Vec::new(),
                created_at: "2026-10-29T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("seed paired client");

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let challenge = {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::create_client_auth_challenge(
                &mut runtime,
                "client-device",
                &config.canonical_origin,
                &config.gateway_identity_fingerprint,
                "https://client.example",
                chrono::Utc::now().timestamp_millis(),
            )
            .expect("client challenge")
        };
        let payload = client_auth_signing_payload(&challenge).expect("frozen signing payload");
        let signature: Signature = signing_key.sign(payload.as_bytes());
        let signature_der_base64 =
            base64::engine::general_purpose::STANDARD.encode(signature.to_der().as_bytes());
        let session_request = AuthSessionRequest {
            challenge_id: challenge.challenge_id,
            device_id: "client-device".to_string(),
            signature_der_base64,
        };
        let body = serde_json::to_vec(&session_request).expect("serialize session request");
        let request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/remote/client/v1/auth/token")
                .header(header::HOST, "wardian.tailnet.ts.net")
                .header(header::ORIGIN, "https://client.example")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.clone()))
                .expect("session request")
        };
        let router = client_namespace_router(RemoteGatewayContext {
            app: app.handle().clone(),
            config: config.clone(),
        });
        let (first, second) = tokio::join!(
            router.clone().oneshot(request()),
            router.clone().oneshot(request())
        );
        let first = first.expect("first response");
        let second = second.expect("second response");
        let (success, rejected) = if first.status() == StatusCode::OK {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(success.status(), StatusCode::OK);
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(success.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            success.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        let response: serde_json::Value = serde_json::from_slice(
            &to_bytes(success.into_body(), 4096)
                .await
                .expect("auth response body"),
        )
        .expect("JSON auth response");
        let access_token = response["access_token"]
            .as_str()
            .expect("one-time bearer token");
        assert_eq!(response["token_type"], "Bearer");
        assert_eq!(response["device_id"], "client-device");
        assert_eq!(
            response["scopes"],
            serde_json::json!(["agents:read", "chat:read"])
        );
        let rejected_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(rejected.into_body(), 4096)
                .await
                .expect("rejected auth response body"),
        )
        .expect("JSON rejected auth response");
        assert_eq!(
            rejected_error,
            serde_json::json!({ "error": "invalid_challenge" })
        );

        let runtime = state.remote_runtime.lock().await;
        assert!(runtime.sessions.is_empty());
        assert_eq!(runtime.client_sessions.len(), 1);
        let (token_verifier, session) =
            runtime.client_sessions.iter().next().expect("token record");
        assert_eq!(
            token_verifier,
            &crate::remote::crypto::sha256_token_verifier(access_token)
        );
        assert_ne!(session.session_id, access_token);
        drop(runtime);
        let device_bytes =
            std::fs::read_to_string(crate::remote::storage::remote_devices_path(temp.path()))
                .expect("device store bytes");
        assert!(!device_bytes.contains(access_token));
        let audit_bytes =
            std::fs::read_to_string(crate::remote::audit::audit_log_path(temp.path()))
                .expect("audit bytes");
        assert!(!audit_bytes.contains(access_token));

        let bearer_request = || {
            let mut request = client_request(
                Method::GET,
                "/remote/client/v1/sharing",
                "https://client.example",
                Body::empty(),
            );
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {access_token}"))
                    .expect("bearer authorization header"),
            );
            request
        };
        let authorized = router
            .clone()
            .oneshot(bearer_request())
            .await
            .expect("authorized sharing request");
        assert_eq!(authorized.status(), StatusCode::OK);

        let mut wrong_host = bearer_request();
        wrong_host
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("untrusted.example"));
        let denied_for_host = router
            .clone()
            .oneshot(wrong_host)
            .await
            .expect("wrong Host response");
        assert_eq!(denied_for_host.status(), StatusCode::FORBIDDEN);
        let host_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(denied_for_host.into_body(), 4096)
                .await
                .expect("wrong Host response body"),
        )
        .expect("JSON wrong Host response");
        assert_eq!(
            host_error,
            serde_json::json!({ "error": "origin_forbidden" })
        );
        assert_eq!(state.remote_runtime.lock().await.client_sessions.len(), 1);

        {
            let mut runtime = state.remote_runtime.lock().await;
            assert_eq!(
                crate::remote::auth::revoke_sessions_for_device(&mut runtime, "client-device"),
                1
            );
        }
        let denied_after_runtime_revocation = router
            .clone()
            .oneshot(bearer_request())
            .await
            .expect("request after runtime revocation");
        assert_eq!(
            denied_after_runtime_revocation.status(),
            StatusCode::FORBIDDEN
        );
        let revoked_error: serde_json::Value = serde_json::from_slice(
            &to_bytes(denied_after_runtime_revocation.into_body(), 4096)
                .await
                .expect("revoked token response body"),
        )
        .expect("JSON revoked token response");
        assert_eq!(
            revoked_error,
            serde_json::json!({ "error": "device_revoked" })
        );

        let mut allowlist_removed = config.clone();
        allowlist_removed.client_origin_allowlist.clear();
        crate::remote::storage::save_remote_config_at(temp.path(), &allowlist_removed)
            .expect("remove client origin");
        let denied_after_origin_removal = router
            .clone()
            .oneshot(bearer_request())
            .await
            .expect("request after origin removal");
        assert_eq!(denied_after_origin_removal.status(), StatusCode::FORBIDDEN);

        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("restore gateway config");
        let mut identity_changed = config.clone();
        identity_changed.gateway_identity_fingerprint = "replacement-fingerprint".to_string();
        crate::remote::storage::save_remote_config_at(temp.path(), &identity_changed)
            .expect("change gateway identity");
        let denied_after_identity_change = router
            .clone()
            .oneshot(bearer_request())
            .await
            .expect("request after gateway identity change");
        assert_eq!(denied_after_identity_change.status(), StatusCode::FORBIDDEN);

        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("restore gateway identity");
        let restarted_app = tauri::test::mock_app();
        restarted_app.manage(AppState::new());
        let restarted_router = client_namespace_router(RemoteGatewayContext {
            app: restarted_app.handle().clone(),
            config: config.clone(),
        });
        let denied_after_restart = restarted_router
            .oneshot(bearer_request())
            .await
            .expect("request after simulated restart");
        assert_eq!(denied_after_restart.status(), StatusCode::UNAUTHORIZED);

        crate::remote::auth::revoke_device_and_sessions(
            state.remote_device_store_lock.clone(),
            &state.remote_runtime,
            "client-device".to_string(),
            chrono::Utc::now().to_rfc3339(),
        )
        .await
        .expect("revoke paired client");
        let denied_after_revocation = router
            .oneshot(bearer_request())
            .await
            .expect("request after revocation");
        assert_eq!(denied_after_revocation.status(), StatusCode::UNAUTHORIZED);

        let audit_bytes =
            std::fs::read_to_string(crate::remote::audit::audit_log_path(temp.path()))
                .expect("updated audit bytes");
        assert!(!audit_bytes.contains(access_token));
    }

    #[test]
    fn automation_monitor_query_accepts_offsets_and_rejects_ambiguous_values() {
        let parsed = parse_remote_automation_monitor_query(Some(
            "active_offset=0&recent_offset=25&schedule_offset=50",
        ))
        .expect("valid monitor query");
        assert_eq!(parsed.active_offset, Some(0));
        assert_eq!(parsed.recent_offset, Some(25));
        assert_eq!(parsed.schedule_offset, Some(50));

        for invalid in [
            "active_offset=-1",
            "recent_offset=not-a-number",
            "schedule_offset=",
            "active_offset=1&active_offset=2",
        ] {
            assert_eq!(
                parse_remote_automation_monitor_query(Some(invalid)).unwrap_err(),
                "invalid_automation_monitor_query"
            );
        }
    }

    #[test]
    fn gateway_bad_request_errors_preserve_machine_code() {
        let error = RemoteGatewayError::bad_request("device_not_found");

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "device_not_found");
        assert!(error.detail.is_none());
    }

    #[test]
    fn gateway_error_detail_is_serialized_into_response_body() {
        let error = RemoteGatewayError::bad_request_with_detail(
            "agent_action_failed",
            "Timed out waiting for ses_1 OpenCode terminal to become ready",
        );

        let response = error.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body =
            tauri::async_runtime::block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
                .expect("response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(payload["code"], "agent_action_failed");
        assert_eq!(
            payload["detail"],
            "Timed out waiting for ses_1 OpenCode terminal to become ready"
        );
    }

    #[test]
    fn gateway_error_without_detail_omits_detail_field() {
        let response = RemoteGatewayError::bad_request("agent_action_failed").into_response();
        let body =
            tauri::async_runtime::block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
                .expect("response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body");

        assert!(payload.get("detail").is_none());
    }

    #[test]
    fn gateway_unauthorized_errors_preserve_machine_code() {
        let error = RemoteGatewayError::unauthorized("missing_session_cookie");

        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "missing_session_cookie");
    }

    #[test]
    fn session_cookie_extracts_host_prefixed_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; __Host-wardian_remote_session=sess-1; other=x"),
        );

        assert_eq!(
            remote_session_cookie_value(&headers).expect("session cookie"),
            "sess-1"
        );
    }

    #[test]
    fn missing_session_cookie_returns_unauthorized_code() {
        let headers = HeaderMap::new();
        let error = remote_session_cookie_value(&headers).expect_err("missing cookie");

        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "missing_session_cookie");
    }

    #[test]
    fn csrf_header_must_match_session_nonce() {
        let session = session_record("sess-1", "dev-1", "csrf-1");
        let mut headers = HeaderMap::new();
        headers.insert("x-wardian-csrf", HeaderValue::from_static("csrf-1"));

        assert!(require_csrf_header(&session, &headers).is_ok());

        headers.insert("x-wardian-csrf", HeaderValue::from_static("wrong"));
        let error = require_csrf_header(&session, &headers).expect_err("csrf mismatch");
        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "csrf_failed");
    }

    #[test]
    fn request_boundary_requires_canonical_host_origin_and_fetch_site() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("wardian.tailnet.ts.net"),
        );
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://wardian.tailnet.ts.net"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));

        assert_eq!(
            require_request_boundary(&config(), &headers, true).expect("canonical request"),
            "https://wardian.tailnet.ts.net"
        );

        headers.insert(
            header::HOST,
            HeaderValue::from_static("other.tailnet.ts.net"),
        );
        let error = require_request_boundary(&config(), &headers, true).expect_err("host mismatch");
        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.code, "origin_forbidden");

        headers.insert(
            header::HOST,
            HeaderValue::from_static("wardian.tailnet.ts.net"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        let error = require_request_boundary(&config(), &headers, true).expect_err("cross-site");
        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.code, "origin_forbidden");

        headers.insert("sec-fetch-site", HeaderValue::from_static("none"));
        let error = require_request_boundary(&config(), &headers, true).expect_err("fetch-none");
        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.code, "origin_forbidden");
    }

    #[test]
    fn mutating_request_boundary_requires_origin_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("wardian.tailnet.ts.net"),
        );

        let error =
            require_request_boundary(&config(), &headers, true).expect_err("missing origin");
        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.code, "origin_forbidden");

        assert_eq!(
            require_request_boundary(&config(), &headers, false).expect("safe request"),
            "https://wardian.tailnet.ts.net"
        );
    }

    #[test]
    fn websocket_ticket_stream_is_allowlisted() {
        assert_eq!(
            validate_remote_stream("agent_status").expect("status stream"),
            "agent_status"
        );
        assert_eq!(
            validate_remote_stream("terminal_attach").expect("terminal attach stream"),
            "terminal_attach"
        );
        assert_eq!(
            validate_remote_stream("terminal").expect_err("unsupported stream"),
            "unsupported_stream"
        );
    }

    #[test]
    fn destructive_agent_actions_have_endpoint_specific_limits() {
        assert_eq!(
            remote_agent_action_specific_limit("kill"),
            Some((10, 10 * 60_000))
        );
        assert_eq!(
            remote_agent_action_specific_limit("clear"),
            Some((10, 10 * 60_000))
        );
        assert_eq!(remote_agent_action_specific_limit("send_prompt"), None);
    }

    #[test]
    fn device_label_strips_control_and_bidirectional_marks() {
        assert_eq!(
            sanitize_device_label(" \u{202e}Pixel\u{0007} Phone\u{2066} "),
            "Pixel Phone"
        );
        assert_eq!(sanitize_device_label("\u{202e}\u{0007}"), "Paired device");
    }

    #[test]
    fn status_socket_first_message_requires_text_json_ticket() {
        let message = axum::extract::ws::Message::Text(r#"{"ticket":"ticket-1"}"#.into());
        assert_eq!(
            parse_status_socket_ticket_message(message).expect("ticket message"),
            "ticket-1"
        );

        let message = axum::extract::ws::Message::Text(r#"{"ticket":""}"#.into());
        assert_eq!(
            parse_status_socket_ticket_message(message).expect_err("empty ticket"),
            "invalid_ticket_message"
        );
    }

    #[test]
    fn status_stream_session_active_reflects_runtime_revocation() {
        let mut runtime = crate::remote::models::RemoteRuntimeState::default();
        let session = crate::remote::auth::create_session(&mut runtime, "dev-1", 1_000_000);

        assert!(status_stream_session_is_active(
            &runtime,
            &session.session_id,
            1_001_000,
        ));

        crate::remote::auth::revoke_sessions_for_device(&mut runtime, "dev-1");

        assert!(!status_stream_session_is_active(
            &runtime,
            &session.session_id,
            1_002_000,
        ));
    }

    #[tokio::test]
    async fn persistent_revocation_blocks_cached_request_and_socket_authority() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        seed_active_remote_devices(temp.path(), &["device-1", "device-2"]);

        let revoker = AppState::new();
        let serving_app = tauri::test::mock_app();
        serving_app.manage(AppState::new());
        let serving = serving_app.state::<AppState>();
        assert!(!std::sync::Arc::ptr_eq(
            &revoker.remote_device_store_lock,
            &serving.remote_device_store_lock,
        ));
        let (revoked_device_session, unaffected_device_session) = {
            let mut runtime = serving.remote_runtime.lock().await;
            let revoked = crate::remote::auth::create_session(
                &mut runtime,
                "device-1",
                chrono::Utc::now().timestamp_millis(),
            );
            let unaffected = crate::remote::auth::create_session(
                &mut runtime,
                "device-2",
                chrono::Utc::now().timestamp_millis(),
            );
            (revoked, unaffected)
        };
        let ctx = RemoteGatewayContext {
            app: serving_app.handle().clone(),
            config: config(),
        };

        assert!(
            require_remote_session(&ctx, &action_headers(&revoked_device_session))
                .await
                .is_ok()
        );
        assert!(
            crate::remote::terminal_stream::ticket_session_is_active(
                &serving,
                &revoked_device_session.session_id,
                "device-1",
            )
            .await
        );
        assert!(
            crate::remote::terminal_stream::ticket_session_is_active(
                &serving,
                &unaffected_device_session.session_id,
                "device-2",
            )
            .await
        );

        crate::remote::auth::revoke_device_and_sessions(
            revoker.remote_device_store_lock.clone(),
            &revoker.remote_runtime,
            "device-1".to_string(),
            "2026-10-29T00:01:00Z".to_string(),
        )
        .await
        .expect("revoke from independent runtime state");

        let issuance_error = crate::remote::auth::issue_session_for_active_device(
            serving.remote_device_store_lock.clone(),
            &serving.remote_runtime,
            "device-1".to_string(),
            "2026-10-29T00:02:00Z".to_string(),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .expect_err("a second runtime cannot issue against revoked persistent authority");
        assert_eq!(issuance_error.message, "remote_device_revoked");

        let now_ms = chrono::Utc::now().timestamp_millis();
        let runtime = serving.remote_runtime.lock().await;
        assert!(status_stream_session_is_active(
            &runtime,
            &revoked_device_session.session_id,
            now_ms,
        ));
        drop(runtime);

        assert!(
            !crate::remote::terminal_stream::ticket_session_is_active(
                &serving,
                &revoked_device_session.session_id,
                "device-1",
            )
            .await
        );
        assert!(!remote_session_is_active(&serving, &revoked_device_session.session_id).await);
        assert!(
            crate::remote::terminal_stream::ticket_session_is_active(
                &serving,
                &unaffected_device_session.session_id,
                "device-2",
            )
            .await
        );
        assert!(remote_session_is_active(&serving, &unaffected_device_session.session_id).await);
        assert!(
            require_remote_session(&ctx, &action_headers(&unaffected_device_session))
                .await
                .is_ok()
        );

        let error = require_remote_session(&ctx, &action_headers(&revoked_device_session))
            .await
            .expect_err("a cached session cannot authorize after persistent revocation");
        assert_eq!(error.code, "session_expired");

        let device_store_path = crate::remote::storage::remote_devices_path(temp.path());
        let valid_store = std::fs::read(&device_store_path).expect("read valid device store");
        std::fs::write(&device_store_path, b"not valid json").expect("corrupt device store");
        assert_eq!(
            remote_device_authority_active("device-2".to_string()).await,
            Err("device_store_failed")
        );
        assert!(
            !crate::remote::terminal_stream::ticket_session_is_active(
                &serving,
                &unaffected_device_session.session_id,
                "device-2",
            )
            .await
        );
        assert!(!remote_session_is_active(&serving, &unaffected_device_session.session_id).await);
        let error = require_remote_session(&ctx, &action_headers(&unaffected_device_session))
            .await
            .expect_err("an unreadable authority store must fail closed");
        assert_eq!(error.code, "device_store_failed");

        std::fs::write(&device_store_path, valid_store).expect("restore valid device store");
        std::fs::remove_file(&device_store_path).expect("remove device store");
        assert_eq!(
            remote_device_authority_active("device-2".to_string()).await,
            Err("device_not_found")
        );
        assert!(
            !crate::remote::terminal_stream::ticket_session_is_active(
                &serving,
                &unaffected_device_session.session_id,
                "device-2",
            )
            .await
        );
        assert!(!remote_session_is_active(&serving, &unaffected_device_session.session_id).await);
        let error = require_remote_session(&ctx, &action_headers(&unaffected_device_session))
            .await
            .expect_err("a missing device store must fail closed");
        assert_eq!(error.code, "session_expired");
    }

    #[tokio::test]
    async fn inbox_action_authorization_enforces_origin_session_and_csrf_boundary() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        crate::remote::storage::save_remote_config_at(temp.path(), &config())
            .expect("remote config");
        seed_active_remote_devices(temp.path(), &["device-1"]);

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let session = {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::create_session(
                &mut runtime,
                "device-1",
                chrono::Utc::now().timestamp_millis(),
            )
        };
        let ctx = RemoteGatewayContext {
            app: app.handle().clone(),
            config: config(),
        };

        let authorized = authorize_inbox_action(&ctx, &action_headers(&session), "mark_read")
            .await
            .expect("authorized inbox action");
        assert_eq!(authorized.1.session_id, session.session_id);

        let mut missing_session = action_headers(&session);
        missing_session.remove(header::COOKIE);
        let error = authorize_inbox_action(&ctx, &missing_session, "mark_all_read")
            .await
            .expect_err("missing session rejected");
        assert_eq!(error.code, "missing_session_cookie");

        let mut invalid_origin = action_headers(&session);
        invalid_origin.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        let error = authorize_inbox_action(&ctx, &invalid_origin, "mark_all_read")
            .await
            .expect_err("invalid origin rejected");
        assert_eq!(error.code, "origin_forbidden");

        let mut missing_csrf = action_headers(&session);
        missing_csrf.remove(REMOTE_CSRF_HEADER_NAME);
        let error = authorize_inbox_action(&ctx, &missing_csrf, "mark_all_read")
            .await
            .expect_err("missing csrf rejected");
        assert_eq!(error.code, "csrf_failed");
    }

    #[tokio::test]
    async fn inbox_action_exposes_automation_projection_unavailability() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        crate::remote::storage::save_remote_config_at(temp.path(), &config())
            .expect("remote config");
        seed_active_remote_devices(temp.path(), &["device-1"]);
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let session = {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::create_session(
                &mut runtime,
                "device-1",
                chrono::Utc::now().timestamp_millis(),
            )
        };
        let ctx = RemoteGatewayContext {
            app: app.handle().clone(),
            config: config(),
        };

        let unavailable = run_inbox_action_with_executor(
            ctx.clone(),
            action_headers(&session),
            RemoteInboxActionRequest {
                action: "mark_all_read".to_string(),
                item_id: None,
                choice: None,
            },
            reject_inbox_action_projection_unavailable,
        )
        .await
        .expect_err("failed run-index refresh should reject the action");
        assert_eq!(unavailable.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            unavailable.code,
            crate::remote::operations::REMOTE_AUTOMATION_INBOX_UNAVAILABLE
        );

        state.set_remote_inbox_runtime_items(0, Vec::new());
        let generic = run_inbox_action_with_executor(
            ctx,
            action_headers(&session),
            RemoteInboxActionRequest {
                action: "mark_read".to_string(),
                item_id: Some("missing-item".to_string()),
                choice: None,
            },
            reject_inbox_action_generically,
        )
        .await
        .expect_err("unknown Inbox item should retain generic action failure");
        assert_eq!(generic.status, StatusCode::BAD_REQUEST);
        assert_eq!(generic.code, "inbox_action_failed");
    }

    #[tokio::test]
    async fn remote_chat_snapshot_failure_returns_a_sanitized_projection_code() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        crate::remote::storage::save_remote_config_at(temp.path(), &config())
            .expect("remote config");
        seed_active_remote_devices(temp.path(), &["device-1"]);

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let session = {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::create_session(
                &mut runtime,
                "device-1",
                chrono::Utc::now().timestamp_millis(),
            )
        };
        let ctx = RemoteGatewayContext {
            app: app.handle().clone(),
            config: config(),
        };

        let error = load_remote_agent_chat(
            State(ctx),
            action_headers(&session),
            AxumPath("sanitized-codex-agent".to_string()),
            RawQuery(None),
        )
        .await
        .expect_err("missing fixture agent should fail before display read");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body_text = String::from_utf8(body.to_vec()).expect("utf-8 response");
        let payload: serde_json::Value = serde_json::from_str(&body_text).expect("json response");
        assert_eq!(payload["code"], "agent_chat_projection_unavailable");
        assert!(payload.get("detail").is_none());
        assert!(!body_text.contains("sanitized-codex-agent"));
    }

    fn has_display_group_identity(metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
        metadata.contains_key("chat_display_member_ids")
            || metadata.contains_key("chat_display_physical_id")
    }

    fn is_unqualified_original_event_msg_identity(
        seed_metadata: &serde_json::Map<String, serde_json::Value>,
        retained_metadata: &serde_json::Map<String, serde_json::Value>,
    ) -> bool {
        !has_display_group_identity(seed_metadata) && !has_display_group_identity(retained_metadata)
    }

    #[test]
    fn original_event_msg_seed_rejects_physical_id_only_projection() {
        let seed_metadata = serde_json::Map::new();
        let raw_metadata = serde_json::Map::new();
        assert!(is_unqualified_original_event_msg_identity(
            &seed_metadata,
            &raw_metadata
        ));

        let physical_id_only = serde_json::json!({
            "chat_display_physical_id": "physical-only"
        });
        assert!(!is_unqualified_original_event_msg_identity(
            &seed_metadata,
            physical_id_only.as_object().expect("object metadata")
        ));
    }

    #[tokio::test]
    async fn remote_chat_recent_limit_preserves_retryable_and_query_statuses() {
        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        crate::remote::storage::save_remote_config_at(temp.path(), &config()).unwrap();
        seed_active_remote_devices(temp.path(), &["device-1"]);
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let session = {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::create_session(
                &mut runtime,
                "device-1",
                chrono::Utc::now().timestamp_millis(),
            )
        };
        // The ordinary reader must fail promptly on this held roster gate.
        // Query validation remains a client error under the same contention.
        let roster = state.agents.lock().await;
        for (query, status, code) in [
            (
                "limit=8",
                StatusCode::SERVICE_UNAVAILABLE,
                "agent_chat_read_retryable",
            ),
            (
                "limit=0",
                StatusCode::BAD_REQUEST,
                "agent_chat_projection_unavailable",
            ),
            (
                "limit=81",
                StatusCode::BAD_REQUEST,
                "agent_chat_projection_unavailable",
            ),
            (
                "limit=eight",
                StatusCode::BAD_REQUEST,
                "agent_chat_projection_unavailable",
            ),
        ] {
            let error = load_remote_agent_chat(
                State(RemoteGatewayContext {
                    app: app.handle().clone(),
                    config: config(),
                }),
                action_headers(&session),
                AxumPath("private-agent".into()),
                RawQuery(Some(query.into())),
            )
            .await
            .expect_err("held reader or invalid query must not qualify a page");
            let response = error.into_response();
            assert_eq!(response.status(), status);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(payload["code"], code);
            assert!(payload.get("detail").is_none());
            assert!(!String::from_utf8(body.to_vec())
                .unwrap()
                .contains("private-agent"));
        }
        drop(roster);
    }

    #[tokio::test]
    async fn remote_chat_gateway_returns_repeated_codex_turns_after_legacy_archive_identity() {
        run_remote_chat_gateway_repeated_codex_turns(false).await;
    }

    #[tokio::test]
    async fn remote_chat_gateway_preserves_original_event_msg_legacy_identity() {
        run_remote_chat_gateway_repeated_codex_turns(true).await;
    }

    async fn run_remote_chat_gateway_repeated_codex_turns(original_event_msg_seed: bool) {
        use std::io::Write as _;

        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set_async(temp.path()).await;
        let gateway_config = client_config();
        crate::remote::storage::save_remote_config_at(temp.path(), &gateway_config)
            .expect("remote config");
        seed_active_remote_devices(temp.path(), &["synthetic-device"]);
        seed_client_device(temp.path(), "synthetic-client-device", "cHVi", "client-fp");
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("enable conversation logging");

        let log_path = temp.path().join("codex.jsonl");
        let source_key = "codex:session:codex-session-one";
        let prompt = "Repeated synthetic request";
        let turn_lines = |turn_id: &str, message_id: &str, message: &str| {
            vec![
                serde_json::json!({
                    "type": "turn_context",
                    "payload": {"turn_id": turn_id},
                })
                .to_string(),
                serde_json::json!({
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": message_id,
                        "role": "user",
                        "content": [{"type": "input_text", "text": message}],
                        "internal_chat_message_metadata_passthrough": {
                            "turn_id": turn_id,
                            "content_item_kinds": ["user.text"],
                        },
                    },
                })
                .to_string(),
                serde_json::json!({
                    "type": "event_msg",
                    "payload": {"type": "user_message", "message": message},
                })
                .to_string(),
            ]
        };
        let first_lines = turn_lines(
            "provider-turn-old",
            "native-message-old",
            "Legacy synthetic request",
        );
        let first_contents = format!(
            "{}\n{}",
            serde_json::json!({"type":"session_meta","payload":{"id":"codex-session-one"}}),
            first_lines.join("\n")
        );
        std::fs::write(&log_path, format!("{first_contents}\n"))
            .expect("write first synthetic turn");

        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let context = crate::state::conversation_archive::ConversationArchiveContext {
            agent_id: "synthetic-codex-agent".to_string(),
            agent_name: "Synthetic Codex".to_string(),
            agent_class: "Coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["codex-session-one".to_string()],
            provider_source_key: Some(source_key.to_string()),
        };

        let mut separate_historical_body_bytes = 0u64;
        let (first, legacy_id, original_seed_json) = if original_event_msg_seed {
            let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
                "synthetic-codex-agent",
                "codex",
                &log_path,
                source_key,
                None,
                true,
            )
            .expect("capture first synthetic turn for the original archive seed");
            let canonical_log_path = std::fs::canonicalize(&log_path)
                .expect("canonicalize synthetic log")
                .to_string_lossy()
                .to_string();
            for event in &mut first.events {
                event.metadata["provider_log"] = serde_json::json!(true);
                event.metadata["log_source"] = serde_json::json!("active_agent_log_path");
                event.metadata["log_path"] = serde_json::json!(canonical_log_path.clone());
                event.id = crate::commands::chat::archive_identity::stable_provider_log_event_id(
                    event, &log_path,
                );
            }
            let old_mirror = first
                .events
                .iter_mut()
                .find(|event| event.source.as_deref() == Some("event_msg"))
                .expect("first user mirror");
            let mut legacy_identity = old_mirror.clone();
            legacy_identity.metadata["input_origin"] = serde_json::json!("provider_internal");
            old_mirror.id = crate::commands::chat::archive_identity::stable_provider_log_event_id(
                &legacy_identity,
                &log_path,
            );
            assert_ne!(
                    old_mirror.id.as_str(),
                    old_mirror.metadata["chat_source_ref"]
                        .as_str()
                        .expect("original mirror retains its native source reference"),
                    "provider_internal-derived archive identity stays distinct from native source identity"
                );
            let source_start = old_mirror.metadata["chat_source_start"]
                .as_u64()
                .expect("original mirror retains its native source start");
            let source_end = old_mirror.metadata["chat_source_end"]
                .as_u64()
                .expect("original mirror retains its native source end");
            assert!(source_start < source_end);
            assert!(
                old_mirror.metadata["chat_source_epoch"]
                    .as_str()
                    .is_some_and(|epoch| !epoch.is_empty()),
                "original mirror retains its native source epoch"
            );
            old_mirror
                .metadata
                .as_object_mut()
                .expect("mirror metadata")
                .remove("legacy_event_ids");
            let legacy_id = old_mirror.id.clone();
            assert!(
                old_mirror.sequence.is_some(),
                "original event mirror retains its source sequence"
            );
            let original_seed_json = Some(
                serde_json::to_value(&*old_mirror)
                    .expect("serialize the exact original archive seed"),
            );
            state
                .conversation_archive
                .append_provider_log_batch_with_context(
                    context.clone(),
                    &first.events,
                    None,
                    &first.next,
                )
                .expect("seed the retained pre-fix archive identity and cursor");
            (first, legacy_id, original_seed_json)
        } else {
            let mut legacy = crate::providers::chat_transcript::normalize_chat_lines(
                "synthetic-codex-agent",
                "codex",
                first_contents.lines(),
            )
            .into_iter()
            .find(|event| {
                event.source.as_deref() == Some("response_item")
                    && event.text.as_deref() == Some("Legacy synthetic request")
            })
            .expect("normalize the original pre-coordinate Codex archive record");
            assert_eq!(
                legacy.kind,
                wardian_core::models::chat::AgentChatEventKind::Message
            );
            assert_eq!(
                legacy.role,
                Some(wardian_core::models::chat::AgentChatRole::User)
            );
            let legacy_text = legacy
                .text
                .as_deref()
                .expect("controlled historical user row has a full text body");
            assert!(legacy_text.is_ascii());
            separate_historical_body_bytes = legacy_text.len() as u64;
            legacy.id = crate::commands::chat::archive_identity::legacy_provider_log_event_id(
                &legacy, &log_path,
            );
            let legacy_id = legacy.id.clone();
            let legacy_sequence = legacy
                .sequence
                .expect("legacy record retains its original source sequence");
            legacy.metadata["provider_log"] = serde_json::json!(true);
            legacy.metadata["log_path"] = serde_json::json!(log_path.to_string_lossy().to_string());
            legacy.metadata["chat_legacy_source_sequence"] = serde_json::json!(legacy_sequence);
            assert!(
                [
                    "chat_source_ref",
                    "chat_source_start",
                    "chat_source_end",
                    "chat_source_epoch",
                    "provider_log_row_offset",
                    "chat_compatibility_legacy_id",
                    "chat_compatibility_source_sequence",
                ]
                .iter()
                .all(|key| legacy
                    .metadata
                    .get(*key)
                    .is_none_or(|value| value.is_null())),
                "pre-coordinate archive fixture carries no current native source binding"
            );
            state
                .conversation_archive
                .append_chat_events_with_context(context.clone(), std::slice::from_ref(&legacy))
                .expect("append historical Codex identity through the normal archive writer");
            let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
                "synthetic-codex-agent",
                "codex",
                &log_path,
                source_key,
                None,
                true,
            )
            .expect("capture first synthetic turn");
            crate::commands::chat::decorate_forward_provider_log_events(
                &mut first.events,
                "codex",
                &log_path,
            );
            for event in first.events.iter().filter(|event| {
                event.kind == wardian_core::models::chat::AgentChatEventKind::Message
            }) {
                let source_ref = event.metadata["chat_source_ref"]
                    .as_str()
                    .expect("native message has a framed source reference");
                assert_eq!(
                    event.id.as_str(),
                    source_ref,
                    "native message ID is its actual source reference"
                );
                let start = event.metadata["chat_source_start"]
                    .as_u64()
                    .expect("native source start");
                let end = event.metadata["chat_source_end"]
                    .as_u64()
                    .expect("native source end");
                assert!(
                    start < end,
                    "native source coordinates form a nonempty frame"
                );
            }
            let first_native_request = first
                .events
                .iter()
                .find(|event| {
                    event.source.as_deref() == Some("response_item")
                        && event.text.as_deref() == Some("Legacy synthetic request")
                })
                .expect("first native request");
            assert_eq!(
                first_native_request.metadata["chat_compatibility_legacy_id"],
                serde_json::json!(legacy_id),
                "normal native decoration retains the established legacy identity"
            );
            assert_eq!(
                first_native_request.metadata["chat_compatibility_source_sequence"],
                serde_json::json!(legacy_sequence),
                "normal native decoration retains the original source sequence"
            );
            state
                .conversation_archive
                .append_provider_log_batch_with_context(
                    context.clone(),
                    &first.events,
                    first.previous.as_ref(),
                    &first.next,
                )
                .expect("publish first native batch through the normal archive path");
            (first, legacy_id, None)
        };

        // Keep 82 earlier turns outside the newest 80 physical observations.
        let mut later_lines = Vec::new();
        let mut declared_later_turns = Vec::with_capacity(85);
        for index in 0..82 {
            let turn_id = format!("provider-turn-history-{index}");
            let message_id = format!("native-message-history-{index}");
            let message = format!("Synthetic history request {index}");
            declared_later_turns.push((turn_id.clone(), message.clone()));
            later_lines.extend(turn_lines(&turn_id, &message_id, &message));
        }
        let mut long_text = "bounded synthetic detail body ".repeat(2_000);
        long_text.replace_range(long_text.len() - 1.., ".");
        assert_eq!(long_text.len(), 60_000);
        assert_eq!(long_text.trim(), long_text.as_str());
        for (turn_id, message_id, message) in [
            (
                "provider-turn-repeated-a",
                "native-message-repeated-a",
                prompt,
            ),
            (
                "provider-turn-repeated-b",
                "native-message-repeated-b",
                prompt,
            ),
            (
                "provider-turn-large-detail",
                "native-message-large-detail",
                long_text.as_str(),
            ),
        ] {
            declared_later_turns.push((turn_id.to_string(), message.to_string()));
            later_lines.extend(turn_lines(turn_id, message_id, message));
        }
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .expect("open synthetic log for append"),
            "{}",
            later_lines.join("\n")
        )
        .expect("append synthetic history and repeated turns");
        let mut later = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "synthetic-codex-agent",
            "codex",
            &log_path,
            source_key,
            Some(first.next.clone()),
            true,
        )
        .expect("capture synthetic history and repeated turns");
        crate::commands::chat::decorate_forward_provider_log_events(
            &mut later.events,
            "codex",
            &log_path,
        );
        for event in later
            .events
            .iter()
            .filter(|event| event.kind == wardian_core::models::chat::AgentChatEventKind::Message)
        {
            let source_ref = event.metadata["chat_source_ref"]
                .as_str()
                .expect("native message has a framed source reference");
            assert_eq!(
                event.id.as_str(),
                source_ref,
                "native message ID is its actual source reference"
            );
            let start = event.metadata["chat_source_start"]
                .as_u64()
                .expect("native source start");
            let end = event.metadata["chat_source_end"]
                .as_u64()
                .expect("native source end");
            assert!(
                start < end,
                "native source coordinates form a nonempty frame"
            );
        }
        assert_eq!(
            later.events.len(),
            declared_later_turns.len() * 2,
            "each declared later turn has its response_item and event_msg observations"
        );
        let mut declared_native_groups = Vec::with_capacity(declared_later_turns.len());
        for (index, (turn_id, expected_text)) in declared_later_turns.iter().enumerate() {
            let pair = &later.events[index * 2..index * 2 + 2];
            assert_eq!(pair[0].source.as_deref(), Some("response_item"));
            assert_eq!(pair[1].source.as_deref(), Some("event_msg"));
            let mut source_refs = Vec::with_capacity(2);
            let mut source_frames = Vec::with_capacity(2);
            for event in pair {
                assert_eq!(
                    event.metadata["provider_turn_id"],
                    serde_json::json!(turn_id),
                    "declared provider turn order matches its native observations"
                );
                assert_eq!(event.text.as_deref(), Some(expected_text.as_str()));
                let source_ref = event.metadata["chat_source_ref"]
                    .as_str()
                    .expect("declared turn has a native source reference")
                    .to_string();
                let start = event.metadata["chat_source_start"]
                    .as_u64()
                    .expect("declared turn has a native frame start");
                let end = event.metadata["chat_source_end"]
                    .as_u64()
                    .expect("declared turn has a native frame end");
                assert!(start < end, "declared turn has a nonempty native frame");
                source_refs.push(source_ref);
                source_frames.push((start, end));
            }
            assert_ne!(source_refs[0], source_refs[1]);
            assert!(source_frames[0].0 < source_frames[0].1);
            assert!(source_frames[0].1 <= source_frames[1].0);
            assert!(source_frames[1].0 < source_frames[1].1);
            declared_native_groups.push((
                turn_id.clone(),
                expected_text.clone(),
                source_refs,
                source_frames,
            ));
        }
        state
            .conversation_archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &later.events,
                Some(&first.next),
                &later.next,
            )
            .expect("append synthetic history to Chat archive");
        let archive_events = state
            .conversation_archive
            .chat_events_for_agent("synthetic-codex-agent")
            .expect("read retained historical archive identity");
        let retained_legacy = archive_events
            .iter()
            .find(|event| event.id == legacy_id)
            .cloned()
            .expect("normal archive retains the historical identity");
        let expected_archive_id = state
            .conversation_archive
            .active_conversation_id("synthetic-codex-agent")
            .expect("read the ordinary archive handle")
            .expect("normal archive append retains its active conversation");
        assert_eq!(
            retained_legacy.metadata["conversation_archive_id"].as_str(),
            Some(expected_archive_id.as_str()),
            "normal DTO overlay identifies the actual active archive conversation"
        );
        if let Some(original_seed_json) = original_seed_json.as_ref() {
            let mut expected_seed_with_overlay = original_seed_json.clone();
            let expected_metadata = expected_seed_with_overlay
                .get_mut("metadata")
                .and_then(serde_json::Value::as_object_mut)
                .expect("pre-append original event_msg metadata");
            assert!(
                expected_metadata
                    .insert(
                        "conversation_archive_id".to_string(),
                        serde_json::Value::String(expected_archive_id.clone()),
                    )
                    .is_none(),
                "pre-append seed does not already contain the normal DTO overlay"
            );
            let retained_json =
                serde_json::to_value(&retained_legacy).expect("serialize retained old event_msg");
            assert_eq!(
                &retained_json,
                &expected_seed_with_overlay,
                "later batches preserve every original event_msg field plus only the normal DTO overlay"
            );
            assert_eq!(retained_legacy.source.as_deref(), Some("event_msg"));
            assert_eq!(retained_legacy.id, legacy_id);
            assert!(retained_legacy.metadata["chat_source_ref"].is_string());
            assert!(retained_legacy.metadata["chat_source_start"].is_u64());
            assert!(retained_legacy.metadata["chat_source_end"].is_u64());
            assert!(retained_legacy.metadata["chat_source_epoch"].is_string());
        } else {
            assert!(
                [
                    "chat_source_ref",
                    "chat_source_start",
                    "chat_source_end",
                    "chat_source_epoch",
                    "provider_log_row_offset",
                ]
                .iter()
                .all(|key| retained_legacy
                    .metadata
                    .get(*key)
                    .is_none_or(|value| value.is_null())),
                "retained historical identity remains distinct from current native coordinates"
            );
        }

        state.agents.lock().await.insert(
            "synthetic-codex-agent".to_string(),
            crate::state::ActiveAgent {
                config: std::sync::Arc::new(std::sync::Mutex::new(
                    wardian_core::models::AgentConfig {
                        session_id: "synthetic-codex-agent".to_string(),
                        session_name: "Synthetic Codex".to_string(),
                        agent_class: "Coder".to_string(),
                        provider: "codex".to_string(),
                        folder: temp.path().to_string_lossy().to_string(),
                        fresh_provider_session_id: Some("codex-session-one".to_string()),
                        conversation_logging:
                            wardian_core::conversations::AgentConversationLoggingSetting::Default,
                        ..Default::default()
                    },
                )),
                child_process: None,
                background_processes: Vec::new(),
                memory_capability: None,
                runtime_generation: None,
                process_id: None,
                query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
                init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
                last_query_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
                current_status: std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string())),
                last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
                watch_state: std::sync::Arc::new(std::sync::Mutex::new(
                    crate::state::AgentWatchState::new(
                        "synthetic-codex-agent".to_string(),
                        32,
                        4096,
                    ),
                )),
                terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
                last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
                log_path: std::sync::Arc::new(std::sync::Mutex::new(Some(log_path.clone()))),
                log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
                #[cfg(windows)]
                job_object: None,
            },
        );

        let session = {
            let mut runtime = state.remote_runtime.lock().await;
            crate::remote::auth::create_session(
                &mut runtime,
                "synthetic-device",
                chrono::Utc::now().timestamp_millis(),
            )
        };
        let ctx = RemoteGatewayContext {
            app: app.handle().clone(),
            config: gateway_config.clone(),
        };

        let (chat_snapshot, _) =
            crate::commands::chat::chat_read_snapshot_for_state(&state, "synthetic-codex-agent")
                .expect("synthetic chat source snapshot");
        assert_eq!(first.next.status, "complete");
        assert!(!first.next.normalizer_has_pending_events());
        assert_eq!(
            first.next.committed_offset,
            first_contents.len() as u64 + 1,
            "first normalizer cohort ends at its complete initial file extent"
        );
        let source_bytes = std::fs::read(&log_path).expect("read the controlled source cohort");
        assert!(
            source_bytes.ends_with(b"\n"),
            "source cohort is newline framed"
        );
        assert_eq!(later.next.status, "complete");
        assert!(!later.next.normalizer_has_pending_events());
        assert_eq!(later.next.committed_offset, source_bytes.len() as u64);
        let physical_records = source_bytes
            .split(|byte| *byte == b'\n')
            .filter(|record| !record.is_empty())
            .count();
        assert!(
            physical_records > 0,
            "the captured source cohort is nonempty"
        );
        let normalized_source_rows = first
            .events
            .len()
            .checked_add(later.events.len())
            .expect("normalized source row count does not overflow");
        let source_event_rows =
            u64::try_from(normalized_source_rows).expect("normalized source row count fits u64");
        let physical_record_count =
            u64::try_from(physical_records).expect("physical record count fits u64");
        let max_record_bytes = source_bytes
            .split(|byte| *byte == b'\n')
            .filter(|record| !record.is_empty())
            .map(|record| record.len())
            .max()
            .expect("controlled source has a framed record");
        let acquisition_chunk_bytes =
            crate::commands::provider_log_acquisition::PROVIDER_LOG_BATCH_BYTES;
        assert!(
            u64::try_from(max_record_bytes).expect("record length fits u64")
                < acquisition_chunk_bytes,
            "every controlled framed record fits within one real provider acquisition chunk"
        );
        // Source replay reads at most PROVIDER_LOG_BATCH_BYTES per input
        // chunk. Count actual input bytes, physical records, three fixed
        // observe keys and six bounded queue entries per normalized row, plus
        // explicit lifecycle work.
        let source_input_chunks = (source_bytes.len() as u64).div_ceil(acquisition_chunk_bytes);
        let source_lifecycle_steps = 4u64; // head initialization, two queue clears, complete-scope publication
        let source_work_ceiling = source_event_rows
            .checked_mul(9)
            .and_then(|work| work.checked_add(physical_record_count))
            .and_then(|work| work.checked_add(source_input_chunks))
            .and_then(|work| work.checked_add(source_lifecycle_steps))
            .expect("finite source-index work bound");
        let archive_rows = u64::try_from(archive_events.len()).expect("archive row count fits u64");
        for event in first.events.iter().chain(&later.events) {
            if event.text.is_some() {
                assert_eq!(
                    event.kind,
                    wardian_core::models::chat::AgentChatEventKind::Message
                );
                assert_eq!(
                    event.role,
                    Some(wardian_core::models::chat::AgentChatRole::User)
                );
                assert!(event.text.as_deref().is_some_and(str::is_ascii));
            }
        }
        let provider_body_bytes = first
            .events
            .iter()
            .chain(&later.events)
            .try_fold(0u64, |total, event| {
                total
                    .checked_add(event.text.as_deref().map_or(0, |text| text.len() as u64))
                    .ok_or_else(|| std::io::Error::other("controlled body byte count overflow"))
            })
            .expect("bounded full provider-input body bytes");
        let body_text_bytes = provider_body_bytes
            .checked_add(separate_historical_body_bytes)
            .expect("full controlled body byte count does not overflow");
        let body_chunks = body_text_bytes
            .div_ceil(crate::state::conversation_archive::chat_read::BODY_BYTES as u64);
        // For n body-bearing rows, sum(ceil(body_i / BODY_BYTES)) is at most
        // n + ceil(total_body_bytes / BODY_BYTES). The 2*n row term therefore
        // covers one todo plus one base body chunk per archive row; body_chunks
        // covers the remaining chunks, including bodies spilled from 8 KiB
        // inline previews into text artifacts by the archive writer.
        let projection_logical_items = archive_rows
            .checked_add(source_event_rows)
            .expect("finite logical contributor count");
        let projection_lifecycle_steps = 4u64; // source-proof qualification, two queue clears, ready publication
        let projection_work_ceiling = archive_rows
            .checked_mul(2)
            .and_then(|work| {
                projection_logical_items
                    .checked_mul(6)
                    .and_then(|queue_work| work.checked_add(queue_work))
            })
            .and_then(|work| work.checked_add(body_chunks))
            .and_then(|work| work.checked_add(projection_lifecycle_steps))
            .expect("finite projection work bound");
        assert!(archive_rows > 0, "the projection cohort is nonempty");

        let queue_transition = |label: &str,
                                before_root: &serde_json::Value,
                                after_root: &serde_json::Value,
                                before_cursor: &serde_json::Value,
                                after_cursor: &serde_json::Value,
                                initialized: &mut bool,
                                cursor_initialized: &mut bool,
                                cleared: &mut bool|
         -> (bool, bool) {
            if before_root.is_null() {
                assert!(before_cursor.is_null(), "{label} empty queue has no cursor");
                if after_root.is_null() {
                    assert!(after_cursor.is_null(), "{label} empty queue has no cursor");
                    return (false, false);
                }
                assert!(
                    !*initialized && !*cleared,
                    "{label} queue cannot restart after initialization or clear"
                );
                assert!(
                    after_root.is_string(),
                    "{label} queue root is a stored reference"
                );
                *initialized = true;
                if !after_cursor.is_null() {
                    assert!(after_cursor.is_string(), "{label} cursor is a stored key");
                    *cursor_initialized = true;
                }
                return (true, false);
            }

            assert!(!*cleared, "{label} queue cannot reopen after its clear");
            *initialized = true;
            if after_root.is_null() {
                assert!(
                    after_cursor.is_null(),
                    "{label} cleared queue has no cursor"
                );
                *cleared = true;
                return (true, true);
            }
            assert_eq!(after_root, before_root, "{label} queue root remains fixed");
            let moved = match (before_cursor.as_str(), after_cursor.as_str()) {
                (None, None) => false,
                (None, Some(_)) => {
                    assert!(!*cursor_initialized, "{label} cursor initializes only once");
                    *cursor_initialized = true;
                    true
                }
                (Some(before), Some(after)) => {
                    assert!(after <= before, "{label} cursor cannot move forward");
                    after < before
                }
                (Some(_), None) => panic!("{label} cursor clears only with its queue root"),
            };
            (moved, false)
        };

        let mut source_calls = 0u64;
        let mut source_done = false;
        let mut previous_source =
            crate::state::conversation_archive::chat_read::source_index_test_snapshot(
                &chat_snapshot,
            )
            .expect("read initial source-index scalar state");
        let mut source_narrative_queue_initialized = previous_source
            .as_ref()
            .is_some_and(|source| source.scalars["narrative_queue"].is_string());
        let mut source_narrative_cursor_initialized = previous_source
            .as_ref()
            .is_some_and(|source| source.scalars["narrative_before"].is_string());
        let mut source_narrative_queue_cleared = false;
        let mut source_queue_clears = 0u64;
        while !source_done {
            assert!(
                source_calls < source_work_ceiling,
                "source index exceeded cohort-derived work bound: calls={source_calls}, bound={source_work_ceiling}, records={physical_record_count}, normalized_rows={source_event_rows}, input_chunks={source_input_chunks}"
            );
            let pending =
                crate::state::conversation_archive::chat_read::advance_source_index(&chat_snapshot)
                    .expect("advance synthetic source index");
            source_calls += 1;
            let current =
                crate::state::conversation_archive::chat_read::source_index_test_snapshot(
                    &chat_snapshot,
                )
                .expect("read source-index scalar state after checkpoint")
                .expect("source index publishes a head for the controlled cohort");
            assert_eq!(
                current.scalars["target_extent"].as_u64(),
                Some(source_bytes.len() as u64),
                "source-index extent remains bound to the complete controlled file"
            );
            if let Some(before) = previous_source.as_ref() {
                assert_eq!(
                    current.generation, before.generation,
                    "source generation is stable"
                );
                assert_eq!(current.epoch, before.epoch, "source epoch is stable");
                assert_eq!(
                    current.scalars["invalidations"], before.scalars["invalidations"],
                    "source admission is not reset during the bounded walk"
                );
                assert_eq!(
                    current.scalars["target_extent"], before.scalars["target_extent"],
                    "source-index input watermark is stable"
                );
                for key in [
                    "replay_bytes_read",
                    "replay_records_decoded",
                    "replay_rows_admitted",
                ] {
                    let before_count = before.scalars[key]
                        .as_u64()
                        .expect("source progress counter is present");
                    let current_count = current.scalars[key]
                        .as_u64()
                        .expect("source progress counter is present");
                    assert!(
                        current_count >= before_count,
                        "source progress counter {key} is monotonic"
                    );
                }
                let replay_was_settled = before.scalars["replay_settled"] == true;
                let replay_is_settled = current.scalars["replay_settled"] == true;
                if replay_was_settled {
                    assert!(
                        replay_is_settled,
                        "settled source replay cannot reset during queue activation"
                    );
                    for key in [
                        "replay_bytes_read",
                        "replay_records_decoded",
                        "replay_rows_admitted",
                    ] {
                        assert_eq!(
                            current.scalars[key], before.scalars[key],
                            "settled source replay counter {key} remains fixed"
                        );
                    }
                    let expected_watermark = serde_json::json!(source_bytes.len() as u64);
                    assert_eq!(current.scalars["narrative_watermark"], expected_watermark);
                    assert_eq!(
                        before.scalars["narrative_watermark"],
                        current.scalars["narrative_watermark"],
                        "settled source narrative watermark remains fixed"
                    );
                    assert!(before.scalars["framed"] == true);
                    assert!(current.scalars["framed"] == true);
                    assert_eq!(before.scalars["before"], serde_json::Value::Null);
                    assert_eq!(current.scalars["before"], serde_json::Value::Null);
                    for key in [
                        "logical_buckets",
                        "logical_claims",
                        "logical_pending",
                        "logical_before",
                    ] {
                        assert_eq!(
                            current.scalars[key], before.scalars[key],
                            "settled source logical root/cursor {key} remains fixed"
                        );
                    }
                    let (queue_moved, cleared) = queue_transition(
                        "SourceIndex narrative",
                        &before.scalars["narrative_queue"],
                        &current.scalars["narrative_queue"],
                        &before.scalars["narrative_before"],
                        &current.scalars["narrative_before"],
                        &mut source_narrative_queue_initialized,
                        &mut source_narrative_cursor_initialized,
                        &mut source_narrative_queue_cleared,
                    );
                    if cleared {
                        source_queue_clears += 1;
                        assert!(
                            source_queue_clears <= 1,
                            "SourceIndex narrative queue clears once"
                        );
                    }
                    if pending {
                        assert!(
                            queue_moved,
                            "settled SourceIndex returned pending without descending narrative cursor or one clear"
                        );
                    }
                } else if pending {
                    let input_progress = [
                        "replay_bytes_read",
                        "replay_records_decoded",
                        "replay_rows_admitted",
                    ]
                    .iter()
                    .any(|key| {
                        current.scalars[*key]
                            .as_u64()
                            .expect("source progress counter is present")
                            > before.scalars[*key]
                                .as_u64()
                                .expect("source progress counter is present")
                    });
                    let settled = !replay_was_settled && replay_is_settled;
                    assert!(
                        input_progress || settled,
                        "SourceIndex pending requires input work or one settled-phase transition"
                    );
                }
            } else if pending {
                assert!(
                    !current.generation.is_empty() && !current.epoch.is_empty(),
                    "initial SourceIndex checkpoint publishes its owned source identity"
                );
            }
            if !pending {
                assert!(
                    current.complete,
                    "false source-index pending is accepted only with current complete scope (progress={})",
                    current.progress
                );
                source_done = true;
            }
            previous_source = Some(current);
        }
        assert!(
            previous_source.as_ref().is_some_and(|source| source.complete),
            "source completion is established by current_complete_scope, including provisional progress"
        );
        let completed_source = previous_source
            .as_ref()
            .expect("completed SourceIndex retains its scalar receipt");
        assert_eq!(completed_source.scalars["replay_settled"], true);
        assert_eq!(
            completed_source.scalars["narrative_watermark"],
            serde_json::json!(source_bytes.len() as u64)
        );
        assert_eq!(
            completed_source.scalars["target_extent"],
            serde_json::json!(source_bytes.len() as u64)
        );
        assert_eq!(completed_source.scalars["framed"], true);
        assert_eq!(completed_source.scalars["before"], serde_json::Value::Null);
        assert!(completed_source.scalars["narrative_queue"].is_null());
        let current_source_epoch = completed_source.epoch.clone();

        let owner = &state.conversation_archive.chat_projection;
        let mut projection_calls = 0u64;
        let mut projection_done = false;
        let initial_owner = owner
            .test_work_snapshot("synthetic-codex-agent")
            .expect("snapshot initial projection work")
            .expect("committed archive has projection-owner work");
        assert_eq!(initial_owner.agent_id, "synthetic-codex-agent");
        assert_eq!(initial_owner.source_key.as_deref(), Some(source_key));
        assert_eq!(initial_owner.row_count as u64, archive_rows);
        assert_eq!(
            initial_owner.source_epoch.as_deref(),
            Some(current_source_epoch.as_str())
        );
        assert_eq!(
            initial_owner.head_source_epoch.as_deref(),
            Some(current_source_epoch.as_str()),
            "projection starts under the completed SourceIndex epoch"
        );
        let mut projection_pending_initialized =
            initial_owner.logical_scalars["pending"].is_string();
        let mut projection_pending_cursor_initialized =
            initial_owner.logical_scalars["before"].is_string();
        let mut projection_pending_cleared = false;
        let mut projection_narrative_initialized =
            initial_owner.logical_scalars["narrative_queue"].is_string();
        let mut projection_narrative_cursor_initialized =
            initial_owner.logical_scalars["narrative_before"].is_string();
        let mut projection_narrative_cleared = false;
        let mut projection_queue_clears = 0u64;
        while !projection_done {
            assert!(
                projection_calls < projection_work_ceiling,
                "projection owner exceeded cohort-derived work bound: calls={projection_calls}, bound={projection_work_ceiling}, rows={archive_rows}, body_bytes={body_text_bytes}, body_chunks={body_chunks}"
            );
            let before = owner
                .test_work_snapshot("synthetic-codex-agent")
                .expect("snapshot projection work before checkpoint")
                .expect("projection work remains owned until ready");
            let pending = owner
                .advance("synthetic-codex-agent")
                .expect("advance synthetic archive projection");
            projection_calls += 1;
            let after = owner
                .test_work_snapshot("synthetic-codex-agent")
                .expect("snapshot projection work after checkpoint")
                .expect("completed projection retains its owner receipt");
            assert_eq!(after.agent_id, before.agent_id);
            assert_eq!(after.conversation_id, before.conversation_id);
            assert_eq!(after.source_key, before.source_key);
            assert_eq!(after.source_epoch, before.source_epoch);
            assert_eq!(after.head_generation, before.head_generation);
            assert_eq!(after.head_source_epoch, before.head_source_epoch);
            assert_eq!(after.row_count, before.row_count);
            assert_eq!(after.committed_output_bytes, before.committed_output_bytes);
            assert!(after.todo_count <= before.todo_count);
            assert!(after.candidate_events <= before.candidate_events);
            assert_eq!(
                after.source_epoch.as_deref(),
                Some(current_source_epoch.as_str())
            );
            assert_eq!(
                after.head_source_epoch.as_deref(),
                Some(current_source_epoch.as_str()),
                "projection stays fenced to the completed SourceIndex epoch"
            );
            let ingestion_pending =
                before.todo_count > 0 || before.body_jobs > 0 || before.remaining_body_bytes > 0;
            if ingestion_pending {
                if pending {
                    assert!(
                        after.todo_count < before.todo_count
                            || after.body_jobs < before.body_jobs
                            || after.remaining_body_bytes < before.remaining_body_bytes,
                        "projection ingestion returned pending without reducing todo/body work"
                    );
                }
            } else {
                assert_eq!(after.todo_count, 0, "stable queue phase has no todo work");
                assert_eq!(after.body_jobs, 0, "stable queue phase has no body jobs");
                assert_eq!(
                    after.remaining_body_bytes, 0,
                    "stable queue phase has no remaining body bytes"
                );
                assert_eq!(
                    before.logical_scalars["narrative_watermark"].as_u64(),
                    Some(later.next.committed_offset),
                    "stable narrative watermark matches the committed source extent"
                );
                assert_eq!(
                    after.logical_scalars["narrative_watermark"].as_u64(),
                    Some(later.next.committed_offset),
                    "stable narrative watermark matches the committed source extent"
                );
                assert_eq!(
                    after.logical_scalars["narrative_watermark"],
                    before.logical_scalars["narrative_watermark"],
                    "stable narrative watermark cannot oscillate"
                );
                for key in ["buckets", "claims"] {
                    assert_eq!(
                        after.logical_scalars[key], before.logical_scalars[key],
                        "stable logical root {key} cannot change during queue activation"
                    );
                }
                let (pending_moved, pending_cleared) = queue_transition(
                    "projection pending",
                    &before.logical_scalars["pending"],
                    &after.logical_scalars["pending"],
                    &before.logical_scalars["before"],
                    &after.logical_scalars["before"],
                    &mut projection_pending_initialized,
                    &mut projection_pending_cursor_initialized,
                    &mut projection_pending_cleared,
                );
                let (narrative_moved, narrative_cleared) = queue_transition(
                    "projection narrative",
                    &before.logical_scalars["narrative_queue"],
                    &after.logical_scalars["narrative_queue"],
                    &before.logical_scalars["narrative_before"],
                    &after.logical_scalars["narrative_before"],
                    &mut projection_narrative_initialized,
                    &mut projection_narrative_cursor_initialized,
                    &mut projection_narrative_cleared,
                );
                if pending_cleared {
                    projection_queue_clears += 1;
                }
                if narrative_cleared {
                    projection_queue_clears += 1;
                }
                assert!(
                    projection_queue_clears <= 2,
                    "projection activation clears at most its two bounded queues"
                );
                if pending {
                    assert!(
                        pending_moved || narrative_moved,
                        "stable projection returned pending without a descending queue cursor or clear"
                    );
                }
            }
            if !pending {
                assert_eq!(after.head_progress, "ready");
                assert_eq!(after.todo_count, 0);
                assert_eq!(after.body_jobs, 0);
                assert_eq!(after.remaining_body_bytes, 0);
                assert_eq!(after.candidate_events, 0);
                assert!(after.logical_scalars["pending"].is_null());
                assert!(after.logical_scalars["narrative_queue"].is_null());
                assert_eq!(
                    after.logical_scalars["narrative_watermark"].as_u64(),
                    Some(later.next.committed_offset),
                    "ready projection watermark matches the committed source extent"
                );
                assert_eq!(
                    after.source_epoch.as_deref(),
                    Some(current_source_epoch.as_str())
                );
                assert_eq!(
                    after.head_source_epoch.as_deref(),
                    Some(current_source_epoch.as_str()),
                    "ready projection retains the completed SourceIndex epoch"
                );
                projection_done = true;
            }
        }
        assert!(projection_calls > 0);

        let _capture_guard = state.conversation_capture_policy_lock.lock().await;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            load_remote_agent_chat(
                State(ctx),
                action_headers(&session),
                AxumPath("synthetic-codex-agent".to_string()),
                RawQuery(Some("limit=80".to_string())),
            ),
        )
        .await
        .expect("normal Chat read must not join the capture gate")
        .expect("authenticated gateway chat read")
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("gateway response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("gateway JSON");

        let repeated_users = payload["events"]
            .as_array()
            .expect("chat event page")
            .iter()
            .filter(|event| event["role"] == "user" && event["text"] == prompt)
            .collect::<Vec<_>>();
        let repeated_diagnostics = repeated_users
            .iter()
            .map(|event| {
                (
                    event["metadata"]["provider_turn_id"].as_str(),
                    event["metadata"]["chat_source_ref"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty()),
                    event["metadata"]["chat_source_start"].as_u64(),
                    event["metadata"]["chat_source_end"].as_u64(),
                    event["id"].as_str().is_some(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            repeated_users.len(),
            2,
            "the two identical-text native turns remain separate: {:?}",
            repeated_diagnostics
        );
        let repeated_a = repeated_users
            .iter()
            .find(|event| event["metadata"]["provider_turn_id"] == "provider-turn-repeated-a")
            .expect("canonical page retains repeated provider turn A");
        let repeated_b = repeated_users
            .iter()
            .find(|event| event["metadata"]["provider_turn_id"] == "provider-turn-repeated-b")
            .expect("canonical page retains repeated provider turn B");
        let repeated_a_source = repeated_a["metadata"]["chat_source_ref"]
            .as_str()
            .expect("turn A framed source observation");
        let repeated_b_source = repeated_b["metadata"]["chat_source_ref"]
            .as_str()
            .expect("turn B framed source observation");
        assert_ne!(
            repeated_a_source, repeated_b_source,
            "separate provider turns retain distinct framed source observations"
        );
        let repeated_a_start = repeated_a["metadata"]["chat_source_start"]
            .as_u64()
            .expect("turn A native source start");
        let repeated_a_end = repeated_a["metadata"]["chat_source_end"]
            .as_u64()
            .expect("turn A native source end");
        let repeated_b_start = repeated_b["metadata"]["chat_source_start"]
            .as_u64()
            .expect("turn B native source start");
        let repeated_b_end = repeated_b["metadata"]["chat_source_end"]
            .as_u64()
            .expect("turn B native source end");
        assert!(
            repeated_a_start < repeated_a_end
                && repeated_a_end <= repeated_b_start
                && repeated_b_start < repeated_b_end,
            "source-frame coordinates preserve A-before-B order: {:?}",
            repeated_diagnostics
        );
        let repeated_turn_order = repeated_users
            .iter()
            .map(|event| {
                event["metadata"]["provider_turn_id"]
                    .as_str()
                    .expect("provider turn metadata")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            repeated_turn_order,
            vec!["provider-turn-repeated-a", "provider-turn-repeated-b"],
            "normal Chat page preserves source order without collapsing equal text: {:?}",
            repeated_diagnostics
        );
        let ids = repeated_users
            .iter()
            .filter_map(|event| event["id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), repeated_users.len());
        assert_eq!(
            state
                .conversation_archive
                .provider_log_capture_state("synthetic-codex-agent", source_key)
                .unwrap(),
            Some(later.next.clone())
        );
        assert!(payload["bytes_read"].as_u64().unwrap() <= 2 * 1024 * 1024);
        assert!(repeated_users
            .iter()
            .all(|event| event["id"].as_str().is_some()));
        assert_ne!(repeated_users[0]["id"], repeated_users[1]["id"]);

        let client_token = seed_client_access_token(
            &state,
            "synthetic-client-device",
            "https://client.example",
            vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            'e',
        )
        .await;
        let client_router = client_namespace_router(RemoteGatewayContext {
            app: app.handle().clone(),
            config: gateway_config,
        });
        let chat_path = "/remote/client/v1/agents/synthetic-codex-agent/chat";
        let response = client_router
            .clone()
            .oneshot(client_bearer_request(
                chat_path,
                "https://client.example",
                &client_token,
            ))
            .await
            .expect("paired-client Chat route response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://client.example"
        );
        assert!(!response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS));
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("paired-client Chat page body");
        let page: serde_json::Value = serde_json::from_slice(&body).expect("Chat page JSON");
        assert_eq!(page["session_id"], "synthetic-codex-agent");
        let page_session_id = page["session_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .expect("initial page session scope")
            .to_string();
        let page_conversation_id = page["conversation_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .expect("initial page conversation scope")
            .to_string();
        assert_eq!(
            page_conversation_id, expected_archive_id,
            "normal page scope identifies the actual archive handle"
        );
        let page_generation = page["generation"]
            .as_str()
            .filter(|value| !value.is_empty())
            .expect("initial page generation scope")
            .to_string();
        let page_source_epoch = page["source_epoch"]
            .as_str()
            .filter(|value| !value.is_empty())
            .expect("initial page source epoch")
            .to_string();
        assert_eq!(
            page_source_epoch, current_source_epoch,
            "initial page belongs to the positively completed SourceIndex epoch"
        );
        let page_events = page["events"].as_array().expect("populated Chat page");
        assert_eq!(
            page_events.len(),
            40,
            "the newest 80 physical observations project to 40 two-member logical rows"
        );
        let revision = page["revision"]
            .as_str()
            .filter(|value| !value.is_empty())
            .expect("opaque current revision")
            .to_string();
        let mut paging_revision = revision.clone();
        let older_cursor = page["next_before"]
            .as_str()
            .filter(|value| !value.is_empty())
            .expect("bounded page exposes older cursor")
            .to_string();
        let large_event = page_events
            .iter()
            .find(|event| event["metadata"]["chat_detail_ref"].is_string())
            .expect("large event exposes lazy detail reference");
        let detail_ref = large_event["metadata"]["chat_detail_ref"]
            .as_str()
            .expect("opaque detail reference")
            .to_string();
        assert!(large_event["text"].as_str().unwrap_or_default().len() <= 1024);
        assert!(!page.to_string().contains(&long_text));

        let mut detail_cursor = detail_ref;
        let mut assembled_detail = String::new();
        let mut detail_complete = false;
        for _ in 0..8 {
            let query = format!(
                "revision={}&detail={}",
                encode_remote_chat_query_component(&revision),
                encode_remote_chat_query_component(&detail_cursor),
            );
            let response = client_router
                .clone()
                .oneshot(client_bearer_request(
                    &format!("{chat_path}?{query}"),
                    "https://client.example",
                    &client_token,
                ))
                .await
                .expect("lazy detail response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("lazy detail body");
            let detail_page: serde_json::Value =
                serde_json::from_slice(&body).expect("lazy detail JSON");
            let detail = detail_page["detail"].as_object().expect("detail chunk");
            assert_eq!(detail["event_id"], large_event["id"]);
            assembled_detail.push_str(detail["text"].as_str().expect("detail text"));
            if detail["complete"] == true {
                detail_complete = true;
                break;
            }
            detail_cursor = detail["next"]
                .as_str()
                .expect("continuation detail cursor")
                .to_string();
        }
        assert!(detail_complete, "bounded detail chunks reach completion");
        assert_eq!(assembled_detail, long_text);

        let turn_index_by_id = declared_native_groups
            .iter()
            .enumerate()
            .map(|(index, group)| (group.0.clone(), index))
            .collect::<std::collections::HashMap<_, _>>();
        let validate_declared_row = |event: &serde_json::Value,
                                     aliases: &serde_json::Value,
                                     index: usize|
         -> (String, serde_json::Value) {
            let (turn_id, expected_text, source_refs, source_frames) =
                &declared_native_groups[index];
            let metadata = event["metadata"]
                .as_object()
                .expect("declared logical row metadata");
            assert_eq!(
                metadata["provider_turn_id"],
                serde_json::json!(turn_id),
                "logical row retains its declared provider turn"
            );
            if index + 1 == declared_native_groups.len() {
                let preview = event["text"].as_str().unwrap_or_default();
                assert!(
                    preview.len() <= 1024,
                    "large body stays lazy in page headers"
                );
            } else {
                assert_eq!(event["text"].as_str(), Some(expected_text.as_str()));
            }
            let public_id = event["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .expect("logical row has a public identity")
                .to_string();
            let members = metadata["chat_display_member_ids"]
                .as_array()
                .expect("logical row retains physical member identities")
                .iter()
                .map(|member| {
                    member
                        .as_str()
                        .expect("physical member identity is a string")
                        .to_string()
                })
                .collect::<std::collections::HashSet<_>>();
            let expected_members = source_refs
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(members.len(), 2, "logical row has exactly two members");
            assert_eq!(members, expected_members);
            let source_ref = metadata["chat_source_ref"]
                .as_str()
                .expect("logical row retains a source-bound observation");
            assert!(source_refs
                .iter()
                .any(|member| member.as_str() == source_ref));
            let physical_id = metadata["chat_display_physical_id"]
                .as_str()
                .expect("logical row retains its selected physical identity");
            assert!(source_refs.iter().any(|member| member == physical_id));
            let source_start = metadata["chat_source_start"]
                .as_u64()
                .expect("logical row source frame start");
            let source_end = metadata["chat_source_end"]
                .as_u64()
                .expect("logical row source frame end");
            assert!(source_refs
                .iter()
                .zip(source_frames)
                .any(|(member, (start, end))| {
                    member == source_ref && *start == source_start && *end == source_end
                }));
            assert_eq!(
                metadata["chat_source_epoch"].as_str(),
                Some(current_source_epoch.as_str()),
                "logical row remains in the completed current source epoch"
            );
            let page_aliases = aliases.as_array().expect("page source aliases");
            for member in source_refs {
                let matching = page_aliases
                    .iter()
                    .filter(|alias| alias["observation_id"].as_str() == Some(member.as_str()))
                    .collect::<Vec<_>>();
                assert_eq!(matching.len(), 1, "each physical member has one page alias");
                assert_eq!(
                    matching[0]["canonical_id"].as_str(),
                    Some(public_id.as_str()),
                    "page alias resolves to this row's public identity"
                );
            }
            let signature = serde_json::json!({
                "id": public_id.clone(),
                "provider_turn_id": turn_id,
                "text": event["text"].clone(),
                "source_ref": metadata["chat_source_ref"].clone(),
                "source_start": metadata["chat_source_start"].clone(),
                "source_end": metadata["chat_source_end"].clone(),
                "source_epoch": metadata["chat_source_epoch"].clone(),
                "physical_id": metadata["chat_display_physical_id"].clone(),
                "members": metadata["chat_display_member_ids"].clone(),
            });
            (public_id, signature)
        };

        let physical_archive_entries = first
            .events
            .len()
            .checked_add(later.events.len())
            .and_then(|count| {
                if original_seed_json.is_none() {
                    count.checked_add(1)
                } else {
                    Some(count)
                }
            })
            .expect("declared physical archive cohort does not overflow");
        assert_eq!(
            archive_events.len(),
            physical_archive_entries,
            "archive contains the declared native rows plus only the pre-coordinate seed"
        );
        // One extra page permits the terminal clear/overlap boundary after
        // the declared 80-entry physical slices.
        let maximum_page_count = u64::try_from(physical_archive_entries)
            .expect("physical archive entries fit u64")
            .div_ceil(80)
            .checked_add(1)
            .expect("one terminal page fits the declared physical cohort");
        let mut pages = vec![page.clone()];
        let mut issued_cursors = std::collections::HashSet::new();
        assert!(issued_cursors.insert(older_cursor.clone()));
        let mut next_cursor = Some(older_cursor.clone());
        while let Some(cursor) = next_cursor {
            assert!(
                (pages.len() as u64) < maximum_page_count,
                "opaque older cursor walk stays within the declared 80-physical-entry cohort"
            );
            let older_query = format!(
                "before={}&revision={}",
                encode_remote_chat_query_component(&cursor),
                encode_remote_chat_query_component(&paging_revision),
            );
            let response = client_router
                .clone()
                .oneshot(client_bearer_request(
                    &format!("{chat_path}?{older_query}"),
                    "https://client.example",
                    &client_token,
                ))
                .await
                .expect("older Chat page response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("older Chat page body");
            let older_page: serde_json::Value =
                serde_json::from_slice(&body).expect("older Chat page JSON");
            assert_eq!(older_page["older_page"], true);
            assert_eq!(
                older_page["session_id"].as_str(),
                Some(page_session_id.as_str())
            );
            assert_eq!(
                older_page["conversation_id"].as_str(),
                Some(page_conversation_id.as_str())
            );
            assert_eq!(
                older_page["generation"].as_str(),
                Some(page_generation.as_str())
            );
            assert_eq!(
                older_page["source_epoch"].as_str(),
                Some(page_source_epoch.as_str())
            );
            assert_eq!(
                older_page["source_epoch"].as_str(),
                Some(current_source_epoch.as_str()),
                "older page remains in the positively completed SourceIndex epoch"
            );
            let older_revision = older_page["revision"]
                .as_str()
                .filter(|value| !value.is_empty())
                .expect("each older page issues its own opaque revision")
                .to_string();
            assert!(!older_page["events"]
                .as_array()
                .expect("older Chat page events")
                .is_empty());
            paging_revision.clone_from(&older_revision);
            let continuation = older_page
                .get("next_before")
                .expect("older page carries its opaque continuation field");
            next_cursor = if continuation.is_null() {
                None
            } else {
                let next = continuation
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .expect("older page returns a nonempty opaque continuation");
                assert!(
                    issued_cursors.insert(next.to_string()),
                    "opaque older cursor does not repeat"
                );
                Some(next.to_string())
            };
            pages.push(older_page);
        }

        let first_native_refs = first
            .events
            .iter()
            .filter(|event| event.kind == wardian_core::models::chat::AgentChatEventKind::Message)
            .map(|event| {
                event.metadata["chat_source_ref"]
                    .as_str()
                    .expect("initial native observation reference")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(first_native_refs.len(), 2);
        let first_native_frames = first
            .events
            .iter()
            .filter(|event| event.kind == wardian_core::models::chat::AgentChatEventKind::Message)
            .map(|event| {
                (
                    event.metadata["chat_source_ref"]
                        .as_str()
                        .expect("initial native observation reference")
                        .to_string(),
                    event.metadata["chat_source_start"]
                        .as_u64()
                        .expect("initial native frame start"),
                    event.metadata["chat_source_end"]
                        .as_u64()
                        .expect("initial native frame end"),
                )
            })
            .collect::<Vec<_>>();
        let first_native_ref_set = first_native_refs
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(first_native_ref_set.len(), 2);
        let validate_declared_first_row =
            |event: &serde_json::Value,
             aliases: &[serde_json::Value],
             allowed_members: &std::collections::HashSet<String>|
             -> (std::collections::HashSet<String>, String, serde_json::Value) {
                let metadata = event["metadata"]
                    .as_object()
                    .expect("declared first-seed row metadata");
                let public_id = event["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .expect("first-seed row has a public identity")
                    .to_string();
                let member_ids = metadata["chat_display_member_ids"]
                    .as_array()
                    .expect("first-seed row retains physical identities")
                    .iter()
                    .map(|member| {
                        member
                            .as_str()
                            .expect("first-seed member identity is a string")
                            .to_string()
                    })
                    .collect::<Vec<_>>();
                let members = member_ids
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>();
                assert!(!members.is_empty());
                assert_eq!(members.len(), member_ids.len());
                assert!(
                    members.is_subset(allowed_members),
                    "first-seed row contains only independently declared physical identities"
                );
                let source_ref = metadata["chat_source_ref"]
                    .as_str()
                    .expect("first-seed row is source-bound");
                assert!(members.contains(source_ref));
                let source_start = metadata["chat_source_start"]
                    .as_u64()
                    .expect("first-seed source frame start");
                let source_end = metadata["chat_source_end"]
                    .as_u64()
                    .expect("first-seed source frame end");
                assert!(
                    first_native_frames
                        .iter()
                        .any(|(declared_ref, start, end)| {
                            declared_ref.as_str() == source_ref
                                && *start == source_start
                                && *end == source_end
                        }),
                    "first-seed source reference and coordinates match the declared input frame"
                );
                assert_eq!(
                    metadata["chat_source_epoch"].as_str(),
                    Some(current_source_epoch.as_str()),
                    "first-seed row is admitted under the current source epoch"
                );
                let physical_id = metadata["chat_display_physical_id"]
                    .as_str()
                    .expect("first-seed row retains its physical display identity");
                assert!(members.contains(physical_id));
                for member in &member_ids {
                    let matching = aliases
                        .iter()
                        .filter(|alias| alias["observation_id"].as_str() == Some(member.as_str()))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        matching.len(),
                        1,
                        "each declared first-seed member has one source alias"
                    );
                    assert_eq!(
                        matching[0]["canonical_id"].as_str(),
                        Some(public_id.as_str()),
                        "first-seed aliases certify the row's public identity"
                    );
                }
                let signature = serde_json::json!({
                    "id": public_id,
                    "text": event["text"].clone(),
                    "source_ref": metadata["chat_source_ref"].clone(),
                    "source_start": metadata["chat_source_start"].clone(),
                    "source_end": metadata["chat_source_end"].clone(),
                    "source_epoch": metadata["chat_source_epoch"].clone(),
                    "physical_id": metadata["chat_display_physical_id"].clone(),
                    "members": metadata["chat_display_member_ids"].clone(),
                });
                (members, public_id, signature)
            };
        let matches_declared_first_observation =
            |event: &serde_json::Value, source_ref: &str, source_start: u64, source_end: u64| {
                first.events.iter().any(|candidate| {
                    candidate.kind == wardian_core::models::chat::AgentChatEventKind::Message
                        && serde_json::to_value(&candidate.kind)
                            .is_ok_and(|kind| event.get("kind") == Some(&kind))
                        && candidate.metadata["chat_source_ref"].as_str() == Some(source_ref)
                        && candidate.metadata["chat_source_start"].as_u64() == Some(source_start)
                        && candidate.metadata["chat_source_end"].as_u64() == Some(source_end)
                        && candidate.metadata["chat_source_epoch"].as_str()
                            == Some(current_source_epoch.as_str())
                        && event["metadata"]["chat_source_epoch"].as_str()
                            == Some(current_source_epoch.as_str())
                        && candidate.source.as_deref() == event["source"].as_str()
                        && serde_json::to_value(&candidate.role)
                            .is_ok_and(|role| event.get("role") == Some(&role))
                        && candidate.text.as_deref() == event["text"].as_str()
                })
            };
        let matches_declared_first_archive_identity =
            |event: &serde_json::Value,
             public_id: &str,
             source_ref: &str,
             source_start: u64,
             source_end: u64| {
                original_seed_json.is_some()
                    && public_id != legacy_id
                    && first.events.iter().any(|candidate| {
                        candidate.id == public_id
                            && candidate.source.as_deref() != Some("event_msg")
                            && candidate.kind
                                == wardian_core::models::chat::AgentChatEventKind::Message
                            && candidate.metadata["chat_source_ref"].as_str() == Some(source_ref)
                            && candidate.metadata["chat_source_start"].as_u64()
                                == Some(source_start)
                            && candidate.metadata["chat_source_end"].as_u64() == Some(source_end)
                            && candidate.metadata["chat_source_epoch"].as_str()
                                == Some(current_source_epoch.as_str())
                            && event["metadata"]["chat_source_epoch"].as_str()
                                == Some(current_source_epoch.as_str())
                            && candidate.source.as_deref() == event["source"].as_str()
                            && serde_json::to_value(&candidate.kind)
                                .is_ok_and(|kind| event.get("kind") == Some(&kind))
                            && serde_json::to_value(&candidate.role)
                                .is_ok_and(|role| event.get("role") == Some(&role))
                            && candidate.text.as_deref() == event["text"].as_str()
                    })
            };
        let page_sequence_matches_row_key =
            |event: &serde_json::Value, metadata: &serde_json::Map<String, serde_json::Value>| {
                metadata
                    .get("chat_page_key")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|key| key.parse::<u64>().ok())
                    .and_then(|key| key.checked_add(1))
                    .is_some_and(|sequence| {
                        event.get("sequence").and_then(serde_json::Value::as_u64) == Some(sequence)
                    })
            };
        let report_unclassified_first_row = |event: &serde_json::Value| {
            let metadata = event.get("metadata").and_then(serde_json::Value::as_object);
            let bounded = |value: Option<&str>| {
                value
                    .map(|value| {
                        value
                            .chars()
                            .filter(|character| !character.is_control())
                            .take(80)
                            .collect::<String>()
                    })
                    .unwrap_or_else(|| "<missing>".to_string())
            };
            let number = |value: Option<u64>| {
                value
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<missing>".to_string())
            };
            let member_kind =
                match metadata.and_then(|metadata| metadata.get("chat_display_member_ids")) {
                    None => "absent",
                    Some(serde_json::Value::Array(values)) if values.is_empty() => "empty-array",
                    Some(serde_json::Value::Array(_)) => "array",
                    Some(_) => "malformed",
                };
            let original_seed_comparison = original_seed_json
                .as_ref()
                .map(|seed| {
                    let seed_metadata = seed.get("metadata").and_then(serde_json::Value::as_object);
                    serde_json::json!({
                        "id_matches": event.get("id") == seed.get("id"),
                        "kind_matches": event.get("kind") == seed.get("kind"),
                        "source_matches": event.get("source") == seed.get("source"),
                        "role_matches": event.get("role") == seed.get("role"),
                        "text_matches": event.get("text") == seed.get("text"),
                        "sequence_matches": event.get("sequence") == seed.get("sequence"),
                        "sequence_page": event.get("sequence"),
                        "sequence_seed": seed.get("sequence"),
                        "source_ref_matches": metadata.and_then(|metadata| metadata.get("chat_source_ref")) == seed_metadata.and_then(|metadata| metadata.get("chat_source_ref")),
                        "source_start_matches": metadata.and_then(|metadata| metadata.get("chat_source_start")) == seed_metadata.and_then(|metadata| metadata.get("chat_source_start")),
                        "source_end_matches": metadata.and_then(|metadata| metadata.get("chat_source_end")) == seed_metadata.and_then(|metadata| metadata.get("chat_source_end")),
                        "source_epoch_matches": metadata.and_then(|metadata| metadata.get("chat_source_epoch")) == seed_metadata.and_then(|metadata| metadata.get("chat_source_epoch")),
                        "archive_id_matches": metadata.and_then(|metadata| metadata.get("conversation_archive_id")).and_then(serde_json::Value::as_str) == Some(expected_archive_id.as_str()),
                        "member_ids_present": metadata.is_some_and(|metadata| metadata.contains_key("chat_display_member_ids")),
                        "physical_id_present": metadata.is_some_and(|metadata| metadata.contains_key("chat_display_physical_id")),
                    })
                })
                .unwrap_or(serde_json::Value::Null);
            eprintln!(
                "unclassified_first_seed_row id={} source={} source_ref={} start={} end={} epoch={} seed_id={} seed_source={} seed_source_ref={} seed_start={} seed_end={} seed_archive_id={} page_archive_id={} member_kind={} original_seed_comparison={}",
                bounded(event.get("id").and_then(serde_json::Value::as_str)),
                bounded(event.get("source").and_then(serde_json::Value::as_str)),
                bounded(metadata.and_then(|metadata| metadata.get("chat_source_ref")).and_then(serde_json::Value::as_str)),
                number(metadata.and_then(|metadata| metadata.get("chat_source_start")).and_then(serde_json::Value::as_u64)),
                number(metadata.and_then(|metadata| metadata.get("chat_source_end")).and_then(serde_json::Value::as_u64)),
                bounded(metadata.and_then(|metadata| metadata.get("chat_source_epoch")).and_then(serde_json::Value::as_str)),
                bounded(original_seed_json.as_ref().and_then(|seed| seed.get("id")).and_then(serde_json::Value::as_str)),
                bounded(original_seed_json.as_ref().and_then(|seed| seed.get("source")).and_then(serde_json::Value::as_str)),
                bounded(original_seed_json.as_ref().and_then(|seed| seed.get("metadata")).and_then(serde_json::Value::as_object).and_then(|metadata| metadata.get("chat_source_ref")).and_then(serde_json::Value::as_str)),
                number(original_seed_json.as_ref().and_then(|seed| seed.get("metadata")).and_then(serde_json::Value::as_object).and_then(|metadata| metadata.get("chat_source_start")).and_then(serde_json::Value::as_u64)),
                number(original_seed_json.as_ref().and_then(|seed| seed.get("metadata")).and_then(serde_json::Value::as_object).and_then(|metadata| metadata.get("chat_source_end")).and_then(serde_json::Value::as_u64)),
                bounded(original_seed_json.as_ref().and_then(|seed| seed.get("metadata")).and_then(serde_json::Value::as_object).and_then(|metadata| metadata.get("conversation_archive_id")).and_then(serde_json::Value::as_str)),
                bounded(metadata.and_then(|metadata| metadata.get("conversation_archive_id")).and_then(serde_json::Value::as_str)),
                member_kind,
                original_seed_comparison,
            );
        };
        let mut first_reference_owners = std::collections::HashMap::<
            String,
            (usize, Vec<String>, String, serde_json::Value),
        >::new();
        let mut seen_first_native_refs = std::collections::HashSet::new();
        let mut original_legacy_row: Option<(usize, serde_json::Value)> = None;
        let mut seen_turns = std::collections::HashMap::new();
        let mut public_ids = std::collections::HashMap::new();
        let mut previous_page_oldest = Some(45usize);
        let mut legacy_alias_targets = Vec::new();
        for (page_index, current_page) in pages.iter().enumerate() {
            assert_eq!(current_page["session_id"], "synthetic-codex-agent");
            assert!(
                current_page["revision"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "each normal page carries its own opaque revision"
            );
            assert_eq!(
                current_page["conversation_id"].as_str(),
                Some(page_conversation_id.as_str())
            );
            assert_eq!(
                current_page["generation"].as_str(),
                Some(page_generation.as_str())
            );
            assert_eq!(
                current_page["source_epoch"].as_str(),
                Some(page_source_epoch.as_str())
            );
            let events = current_page["events"]
                .as_array()
                .expect("normal Chat page events");
            let aliases = current_page["aliases"]
                .as_array()
                .expect("normal Chat page aliases");
            if page_index > 0 {
                let (turn_index, source_id, older_public_id, older_signature) = events
                    .iter()
                    .find_map(|event| {
                        let turn_id = event["metadata"]["provider_turn_id"].as_str()?;
                        let turn_index = turn_index_by_id.get(turn_id).copied()?;
                        let source_id = event["metadata"]["chat_source_ref"]
                            .as_str()
                            .filter(|value| !value.is_empty())?
                            .to_string();
                        let (public_id, signature) =
                            validate_declared_row(event, &current_page["aliases"], turn_index);
                        Some((turn_index, source_id, public_id, signature))
                    })
                    .expect("each older page contains a declared native observation");
                let source_ids_json = serde_json::to_string(&vec![source_id.clone()])
                    .expect("canonical older source ID list");
                let older_renewal_query = format!(
                    "revision={}&revalidate_ids={}",
                    encode_remote_chat_query_component(
                        current_page["revision"]
                            .as_str()
                            .expect("older page revision")
                    ),
                    encode_remote_chat_query_component(&source_ids_json),
                );
                let response = client_router
                    .clone()
                    .oneshot(client_bearer_request(
                        &format!("{chat_path}?{older_renewal_query}"),
                        "https://client.example",
                        &client_token,
                    ))
                    .await
                    .expect("older-page authenticated revalidation response");
                assert_eq!(response.status(), StatusCode::OK);
                let body = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("older-page revalidation body");
                let renewed_page: serde_json::Value =
                    serde_json::from_slice(&body).expect("older-page revalidation JSON");
                let older_revision = current_page["revision"]
                    .as_str()
                    .expect("older page revision");
                assert_eq!(
                    renewed_page["revalidated_ids"],
                    serde_json::json!([source_id.clone()])
                );
                assert_eq!(
                    renewed_page["revision"].as_str(),
                    Some(older_revision),
                    "revalidation acknowledges the revision issued by this older page"
                );
                assert_eq!(
                    renewed_page["session_id"].as_str(),
                    Some(page_session_id.as_str())
                );
                assert_eq!(
                    renewed_page["conversation_id"].as_str(),
                    Some(page_conversation_id.as_str())
                );
                assert_eq!(
                    renewed_page["generation"].as_str(),
                    Some(page_generation.as_str())
                );
                assert_eq!(
                    renewed_page["source_epoch"].as_str(),
                    Some(page_source_epoch.as_str())
                );
                assert_eq!(renewed_page["reset"], false);
                let renewed_events = renewed_page["events"]
                    .as_array()
                    .expect("older-page revalidation events");
                let matching_events = renewed_events
                    .iter()
                    .filter(|event| {
                        event["metadata"]["chat_source_ref"].as_str() == Some(source_id.as_str())
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    matching_events.len(),
                    1,
                    "revalidation returns the requested native observation exactly once"
                );
                let (renewed_public_id, renewed_signature) =
                    validate_declared_row(matching_events[0], &renewed_page["aliases"], turn_index);
                let expected_native_members = declared_native_groups[turn_index]
                    .2
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>();
                assert_eq!(expected_native_members.len(), 2);
                let signature_members = |signature: &serde_json::Value| {
                    let member_ids = signature["members"]
                        .as_array()
                        .expect("lineage signature physical members");
                    let members = member_ids
                        .iter()
                        .map(|member| {
                            member
                                .as_str()
                                .expect("lineage member identity is a string")
                                .to_string()
                        })
                        .collect::<std::collections::HashSet<_>>();
                    assert_eq!(
                        members.len(),
                        member_ids.len(),
                        "lineage signatures contain no duplicate physical member"
                    );
                    members
                };
                assert_eq!(signature_members(&older_signature), expected_native_members);
                assert_eq!(
                    signature_members(&renewed_signature),
                    expected_native_members
                );
                for field in [
                    "provider_turn_id",
                    "text",
                    "source_ref",
                    "source_start",
                    "source_end",
                    "source_epoch",
                    "physical_id",
                ] {
                    assert_eq!(
                        older_signature[field], renewed_signature[field],
                        "only a response's public ID may be rekeyed"
                    );
                }
                let mut component_ids = expected_native_members.clone();
                component_ids.insert(older_public_id.clone());
                component_ids.insert(renewed_public_id.clone());
                let older_public_is_live_native =
                    [current_page, &renewed_page]
                        .into_iter()
                        .any(|response_page| {
                            response_page["events"]
                                .as_array()
                                .expect("lineage response events")
                                .iter()
                                .any(|event| {
                                    let metadata = &event["metadata"];
                                    metadata["chat_source_ref"].as_str()
                                        == Some(older_public_id.as_str())
                                        || metadata["chat_display_physical_id"].as_str()
                                            == Some(older_public_id.as_str())
                                        || metadata["chat_display_member_ids"]
                                            .as_array()
                                            .is_some_and(|members| {
                                                members.iter().any(|member| {
                                                    member.as_str()
                                                        == Some(older_public_id.as_str())
                                                })
                                            })
                                })
                        });
                let older_public_tombstone_allowed =
                    older_public_id != renewed_public_id && !older_public_is_live_native;
                let assert_response_lineage =
                    |response_page: &serde_json::Value, response_public_id: &str| {
                        let response_events = response_page["events"]
                            .as_array()
                            .expect("lineage response events");
                        let incident_rows = response_events
                            .iter()
                            .filter(|event| {
                                let metadata = &event["metadata"];
                                let member_incident = metadata["chat_display_member_ids"]
                                    .as_array()
                                    .is_some_and(|members| {
                                        members.iter().any(|member| {
                                            member
                                                .as_str()
                                                .is_some_and(|id| component_ids.contains(id))
                                        })
                                    });
                                let source_incident = [
                                    metadata["chat_source_ref"].as_str(),
                                    metadata["chat_display_physical_id"].as_str(),
                                ]
                                .into_iter()
                                .flatten()
                                .any(|id| component_ids.contains(id));
                                let public_incident = event["id"]
                                    .as_str()
                                    .is_some_and(|id| component_ids.contains(id));
                                let turn_incident = metadata["provider_turn_id"].as_str()
                                    == Some(declared_native_groups[turn_index].0.as_str());
                                member_incident
                                    || source_incident
                                    || public_incident
                                    || turn_incident
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(
                            incident_rows.len(),
                            1,
                            "each response has one row incident to the selected native component"
                        );
                        let response_row = incident_rows[0];
                        assert_eq!(response_row["id"].as_str(), Some(response_public_id));
                        assert_eq!(
                            response_row["metadata"]["provider_turn_id"].as_str(),
                            Some(declared_native_groups[turn_index].0.as_str())
                        );
                        let response_member_ids = response_row["metadata"]
                            ["chat_display_member_ids"]
                            .as_array()
                            .expect("lineage row physical members");
                        let response_members = response_member_ids
                            .iter()
                            .map(|member| {
                                member
                                    .as_str()
                                    .expect("lineage row member identity is a string")
                                    .to_string()
                            })
                            .collect::<std::collections::HashSet<_>>();
                        assert_eq!(
                            response_members.len(),
                            response_member_ids.len(),
                            "lineage rows contain no duplicate physical member"
                        );
                        assert_eq!(response_members, expected_native_members);
                        let response_aliases = response_page["aliases"]
                            .as_array()
                            .expect("lineage response aliases");
                        for member in &expected_native_members {
                            let member_edges = response_aliases
                                .iter()
                                .filter(|alias| {
                                    alias["observation_id"].as_str() == Some(member.as_str())
                                })
                                .collect::<Vec<_>>();
                            assert_eq!(
                                member_edges.len(),
                                1,
                                "each declared native member has one response-local alias edge"
                            );
                            assert_eq!(
                                member_edges[0]["canonical_id"].as_str(),
                                Some(response_public_id)
                            );
                        }
                        for alias in response_aliases {
                            let observation_id = alias["observation_id"]
                                .as_str()
                                .expect("lineage alias observation identity");
                            let canonical_id = alias["canonical_id"]
                                .as_str()
                                .expect("lineage alias canonical identity");
                            if component_ids.contains(observation_id)
                                || component_ids.contains(canonical_id)
                            {
                                let native_edge = expected_native_members.contains(observation_id)
                                    && canonical_id == response_public_id;
                                let current_self_edge = observation_id == response_public_id
                                    && canonical_id == response_public_id;
                                let old_tombstone_edge = older_public_tombstone_allowed
                                    && observation_id == older_public_id.as_str()
                                    && (canonical_id == older_public_id.as_str()
                                        || canonical_id == renewed_public_id.as_str());
                                assert!(
                                    native_edge || current_self_edge || old_tombstone_edge,
                                    "lineage aliases stay within declared members and actual response IDs"
                                );
                            }
                        }
                        for removed in response_page["removed_ids"]
                            .as_array()
                            .expect("lineage response removed IDs")
                        {
                            let removed_id = removed
                                .as_str()
                                .expect("lineage removed identity is a string");
                            if component_ids.contains(removed_id) {
                                let allowed_old_tombstone = older_public_tombstone_allowed
                                    && removed_id == older_public_id.as_str()
                                    && removed_id != response_public_id;
                                assert!(
                                    allowed_old_tombstone,
                                    "responses retain every live component identity"
                                );
                            }
                        }
                    };
                assert_response_lineage(current_page, &older_public_id);
                assert_response_lineage(&renewed_page, &renewed_public_id);
            }
            // Group membership is full certified identity, which may cross a physical page boundary.
            let page_legacy_target = if original_seed_json.is_some() {
                None
            } else {
                let page_legacy_aliases = aliases
                    .iter()
                    .filter(|alias| alias["observation_id"].as_str() == Some(legacy_id.as_str()))
                    .collect::<Vec<_>>();
                assert!(
                    page_legacy_aliases.len() <= 1,
                    "pre-coordinate identity has at most one alias on a page"
                );
                page_legacy_aliases.first().map(|alias| {
                    alias["canonical_id"]
                        .as_str()
                        .expect("pre-coordinate alias has a canonical identity")
                        .to_string()
                })
            };
            let mut page_turn_indices = Vec::new();
            let mut local_indices = std::collections::HashSet::new();
            let mut repeated_boundary_indices = Vec::new();
            let mut page_first_native_refs = std::collections::HashSet::new();
            let mut repeated_first_group_keys = std::collections::HashSet::new();
            for event in events {
                let turn_id = event["metadata"]["provider_turn_id"].as_str();
                if let Some(index) = turn_id.and_then(|id| turn_index_by_id.get(id).copied()) {
                    let (public_id, signature) =
                        validate_declared_row(event, &current_page["aliases"], index);
                    assert!(
                        local_indices.insert(index),
                        "a page contains each declared turn once"
                    );
                    page_turn_indices.push(index);
                    if let Some((previous_public_id, previous_signature)) = seen_turns.get(&index) {
                        repeated_boundary_indices.push(index);
                        assert_eq!(
                            previous_page_oldest,
                            Some(index),
                            "only the adjacent older-page boundary may overlap"
                        );
                        assert_eq!(previous_public_id, &public_id);
                        assert_eq!(previous_signature, &signature);
                    } else {
                        if page_index > 0 {
                            assert!(
                                previous_page_oldest.is_some_and(|oldest| index < oldest),
                                "older-page turns precede the already loaded cohort"
                            );
                        }
                        assert!(
                            public_ids.insert(public_id.clone(), index).is_none(),
                            "distinct declared turns keep distinct public identities"
                        );
                        seen_turns.insert(index, (public_id, signature));
                    }
                } else {
                    let Some(public_id) = event
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                    else {
                        report_unclassified_first_row(event);
                        panic!("first-seed row has no declared public identity");
                    };
                    let is_declared_original = original_seed_json.as_ref().is_some_and(|seed| {
                        let Some(seed_metadata) =
                            seed.get("metadata").and_then(serde_json::Value::as_object)
                        else {
                            return false;
                        };
                        let Some(metadata) =
                            event.get("metadata").and_then(serde_json::Value::as_object)
                        else {
                            return false;
                        };
                        event.get("id") == seed.get("id")
                            && event.get("kind") == seed.get("kind")
                            && event.get("source") == seed.get("source")
                            && event.get("role") == seed.get("role")
                            && event.get("text") == seed.get("text")
                            && page_sequence_matches_row_key(event, metadata)
                            && [
                                "chat_source_ref",
                                "chat_source_start",
                                "chat_source_end",
                                "chat_source_epoch",
                            ]
                            .iter()
                            .all(|key| metadata.get(*key) == seed_metadata.get(*key))
                            && metadata
                                .get("conversation_archive_id")
                                .is_none_or(|archive_id| {
                                    archive_id.as_str() == Some(expected_archive_id.as_str())
                                })
                    });
                    if is_declared_original {
                        let seed = original_seed_json
                            .as_ref()
                            .expect("exact original event_msg seed remains declared");
                        let seed_metadata = seed["metadata"]
                            .as_object()
                            .expect("declared original event_msg metadata");
                        let metadata = event["metadata"]
                            .as_object()
                            .expect("exact retained original event_msg metadata");
                        let source_ref = seed_metadata["chat_source_ref"]
                            .as_str()
                            .expect("original event_msg seed source reference");
                        let source_start = seed_metadata["chat_source_start"]
                            .as_u64()
                            .expect("original event_msg seed source start");
                        let source_end = seed_metadata["chat_source_end"]
                            .as_u64()
                            .expect("original event_msg seed source end");
                        assert_eq!(public_id, legacy_id);
                        assert_eq!(event["id"], seed["id"]);
                        assert_eq!(event["kind"], seed["kind"]);
                        assert_eq!(event["source"], seed["source"]);
                        assert_eq!(event["role"], seed["role"]);
                        assert_eq!(event["text"], seed["text"]);
                        assert!(
                            page_sequence_matches_row_key(event, metadata),
                            "page sequence follows its persisted archive page key"
                        );
                        assert!(
                            metadata
                                .get("conversation_archive_id")
                                .is_none_or(|archive_id| {
                                    archive_id.as_str() == Some(expected_archive_id.as_str())
                                }),
                            "an event-level archive handle, when present, matches page scope"
                        );
                        assert_eq!(
                            metadata["chat_source_ref"],
                            seed_metadata["chat_source_ref"]
                        );
                        assert_eq!(
                            metadata["chat_source_start"],
                            seed_metadata["chat_source_start"]
                        );
                        assert_eq!(
                            metadata["chat_source_end"],
                            seed_metadata["chat_source_end"]
                        );
                        assert_eq!(
                            metadata["chat_source_epoch"],
                            seed_metadata["chat_source_epoch"]
                        );
                        assert_eq!(
                            metadata["chat_source_epoch"].as_str(),
                            Some(current_source_epoch.as_str())
                        );
                        assert!(first_native_ref_set.contains(source_ref));
                        assert!(first_native_frames
                            .iter()
                            .any(|(declared_ref, start, end)| {
                                declared_ref.as_str() == source_ref
                                    && *start == source_start
                                    && *end == source_end
                            }));
                        assert!(matches_declared_first_observation(
                            event,
                            source_ref,
                            source_start,
                            source_end
                        ));
                        if !is_unqualified_original_event_msg_identity(seed_metadata, metadata) {
                            report_unclassified_first_row(event);
                            panic!("unqualified original event_msg row gained group identity");
                        }
                        let signature = serde_json::json!({
                            "id": public_id,
                            "kind": event["kind"].clone(),
                            "source": event["source"].clone(),
                            "role": event["role"].clone(),
                            "text": event["text"].clone(),
                            "sequence": event["sequence"].clone(),
                            "source_ref": metadata["chat_source_ref"].clone(),
                            "source_start": metadata["chat_source_start"].clone(),
                            "source_end": metadata["chat_source_end"].clone(),
                            "source_epoch": metadata["chat_source_epoch"].clone(),
                        });
                        seen_first_native_refs.insert(source_ref.to_string());
                        if let Some((previous_page, previous_signature)) =
                            original_legacy_row.as_ref()
                        {
                            assert_eq!(
                            *previous_page + 1,
                            page_index,
                            "unqualified original row overlaps only at an adjacent page boundary"
                        );
                            assert_eq!(previous_signature, &signature);
                        } else {
                            original_legacy_row = Some((page_index, signature));
                        }
                    } else {
                        let metadata = event.get("metadata").and_then(serde_json::Value::as_object);
                        let source_ref = metadata
                            .and_then(|metadata| metadata.get("chat_source_ref"))
                            .and_then(serde_json::Value::as_str)
                            .filter(|source_ref| !source_ref.is_empty());
                        let source_start = metadata
                            .and_then(|metadata| metadata.get("chat_source_start"))
                            .and_then(serde_json::Value::as_u64);
                        let source_end = metadata
                            .and_then(|metadata| metadata.get("chat_source_end"))
                            .and_then(serde_json::Value::as_u64);
                        let is_precoordinate_group =
                            page_legacy_target.as_deref() == Some(public_id.as_str());
                        let has_group_identity = metadata.is_some_and(has_display_group_identity);
                        let is_raw_native = !is_precoordinate_group
                            && !has_group_identity
                            && source_start.zip(source_end).is_some_and(|(start, end)| {
                                let Some(source_ref) = source_ref else {
                                    return false;
                                };
                                (source_ref == public_id.as_str()
                                    && matches_declared_first_observation(
                                        event, source_ref, start, end,
                                    ))
                                    || matches_declared_first_archive_identity(
                                        event,
                                        public_id.as_str(),
                                        source_ref,
                                        start,
                                        end,
                                    )
                            });
                        let (members, validated_id, signature) = if is_raw_native {
                            assert!(
                                metadata.is_some_and(|metadata| {
                                    page_sequence_matches_row_key(event, metadata)
                                }),
                                "bounded page sequence follows its persisted row key independently of provider sequence"
                            );
                            let source_ref = source_ref
                                .expect("raw native identity is its declared source reference");
                            let source_start = source_start.expect("raw native source start");
                            let source_end = source_end.expect("raw native source end");
                            let members = std::collections::HashSet::from([source_ref.to_string()]);
                            let signature = serde_json::json!({
                                "id": public_id,
                                "kind": event["kind"].clone(),
                                "source": event["source"].clone(),
                                "role": event["role"].clone(),
                                "text": event["text"].clone(),
                                "source_ref": source_ref,
                                "source_start": source_start,
                                "source_end": source_end,
                                "source_epoch": event["metadata"]["chat_source_epoch"].clone(),
                                "raw_native_observation": source_ref == public_id,
                                "legacy_archive_identity": source_ref != public_id,
                            });
                            (members, public_id.clone(), signature)
                        } else {
                            let Some(member_ids) = metadata
                                .and_then(|metadata| metadata.get("chat_display_member_ids"))
                                .and_then(serde_json::Value::as_array)
                            else {
                                report_unclassified_first_row(event);
                                panic!("first-seed row is neither a declared raw observation nor a certified group");
                            };
                            if member_ids.is_empty()
                                || member_ids.iter().any(|member| {
                                    member.as_str().is_none_or(|member| member.is_empty())
                                })
                            {
                                report_unclassified_first_row(event);
                                panic!("first-seed group has malformed member identities");
                            }
                            let (Some(source_ref), Some(source_start), Some(source_end)) =
                                (source_ref, source_start, source_end)
                            else {
                                report_unclassified_first_row(event);
                                panic!("first-seed group has no complete source frame");
                            };
                            if !matches_declared_first_observation(
                                event,
                                source_ref,
                                source_start,
                                source_end,
                            ) {
                                report_unclassified_first_row(event);
                                panic!(
                                    "first-seed group does not match a declared source observation"
                                );
                            }
                            let mut allowed_members = first_native_ref_set.clone();
                            if is_precoordinate_group {
                                assert!(allowed_members.insert(legacy_id.clone()));
                            }
                            let (members, validated_id, signature) =
                                validate_declared_first_row(event, aliases, &allowed_members);
                            assert_eq!(validated_id, public_id);
                            if is_precoordinate_group {
                                assert_eq!(
                                members, allowed_members,
                                "replacement legacy group has both declared native members and its legacy owner"
                            );
                            } else {
                                assert!(
                                members.is_subset(&first_native_ref_set),
                                "older source groups contain only declared first-seed native members"
                            );
                            }
                            (members, validated_id, signature)
                        };
                        assert_eq!(validated_id, public_id);
                        let mut group_members = members.iter().cloned().collect::<Vec<_>>();
                        group_members.sort();
                        for member in &group_members {
                            if !first_native_ref_set.contains(member) {
                                continue;
                            }
                            seen_first_native_refs.insert(member.clone());
                            assert!(
                                page_first_native_refs.insert(member.clone()),
                                "each first-seed native observation appears once per page"
                            );
                            if let Some((
                                previous_page,
                                previous_members,
                                previous_id,
                                previous_signature,
                            )) = first_reference_owners.get(member)
                            {
                                assert_eq!(
                                    *previous_page + 1,
                                    page_index,
                                    "first-seed overlap is limited to adjacent pages"
                                );
                                assert_eq!(previous_members, &group_members);
                                assert_eq!(previous_id, &public_id);
                                assert_eq!(previous_signature, &signature);
                                repeated_first_group_keys.insert(group_members.clone());
                            } else {
                                first_reference_owners.insert(
                                    member.clone(),
                                    (
                                        page_index,
                                        group_members.clone(),
                                        public_id.clone(),
                                        signature.clone(),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            if original_seed_json.is_some() {
                assert!(
                    aliases.iter().all(|alias| {
                        alias["observation_id"].as_str() != Some(legacy_id.as_str())
                            && alias["canonical_id"].as_str() != Some(legacy_id.as_str())
                    }),
                    "unqualified original identity receives no synthetic alias"
                );
            }
            assert!(page_turn_indices
                .windows(2)
                .all(|pair| pair[1] == pair[0] + 1));
            if page_index == 0 {
                assert_eq!(page_turn_indices, (45..85).collect::<Vec<_>>());
                let declared_native_observations = page_turn_indices
                    .iter()
                    .map(|index| declared_native_groups[*index].2.len())
                    .sum::<usize>();
                assert_eq!(
                    declared_native_observations, 80,
                    "the 40 declared current rows account for exactly 80 native observations"
                );
            } else {
                assert!(
                    repeated_boundary_indices.len() <= 1,
                    "an older page has at most one certified current-group boundary overlap"
                );
                assert!(
                    repeated_first_group_keys.len() <= 1,
                    "an older page has at most one certified first-seed boundary overlap"
                );
                if let Some(overlap) = repeated_boundary_indices.first() {
                    assert_eq!(page_turn_indices.last(), Some(overlap));
                }
                if let Some(oldest_new_turn) = page_turn_indices
                    .iter()
                    .filter(|index| !repeated_boundary_indices.contains(*index))
                    .min()
                {
                    previous_page_oldest = Some(*oldest_new_turn);
                }
            }

            if original_seed_json.is_none() {
                for alias in aliases
                    .iter()
                    .filter(|alias| alias["observation_id"].as_str() == Some(legacy_id.as_str()))
                {
                    let canonical_id = alias["canonical_id"]
                        .as_str()
                        .expect("pre-coordinate alias has a canonical identity");
                    let targets = events
                        .iter()
                        .filter(|event| event["id"].as_str() == Some(canonical_id))
                        .collect::<Vec<_>>();
                    assert_eq!(targets.len(), 1, "legacy alias targets one row on its page");
                    let target = targets[0];
                    assert_eq!(target["text"], "Legacy synthetic request");
                    let target_metadata = target["metadata"]
                        .as_object()
                        .expect("source-bound historical group metadata");
                    let target_members = target_metadata["chat_display_member_ids"]
                        .as_array()
                        .expect("historical group physical members")
                        .iter()
                        .map(|value| value.as_str().expect("member source identity").to_string())
                        .collect::<std::collections::HashSet<_>>();
                    let mut expected_first_members = first_native_ref_set.clone();
                    assert!(
                        expected_first_members.insert(legacy_id.clone()),
                        "legacy owner identity is distinct from both native observations"
                    );
                    assert_eq!(
                        target_members, expected_first_members,
                        "canonical archive group retains both native members and its legacy owner"
                    );
                    let target_source_ref = target_metadata["chat_source_ref"]
                        .as_str()
                        .expect("historical group source reference");
                    assert!(first_native_frames.iter().any(|(source_ref, start, end)| {
                        target_source_ref == source_ref
                            && target_metadata["chat_source_start"].as_u64() == Some(*start)
                            && target_metadata["chat_source_end"].as_u64() == Some(*end)
                    }));
                    assert_eq!(
                        target_metadata["chat_source_epoch"].as_str(),
                        Some(current_source_epoch.as_str())
                    );
                    legacy_alias_targets.push(canonical_id.to_string());
                }
            }
        }
        assert_eq!(
            seen_first_native_refs, first_native_ref_set,
            "older pages retain exactly the declared first-seed native identities"
        );
        assert_eq!(
            seen_turns.len(),
            declared_native_groups.len(),
            "opaque cursor walk retains the complete declared 85-turn cohort"
        );
        assert!((0..declared_native_groups.len()).all(|index| seen_turns.contains_key(&index)));
        assert_eq!(
            public_ids.len(),
            declared_native_groups.len(),
            "the declared logical turns remain distinct by public identity"
        );
        if original_seed_json.is_none() {
            assert_eq!(
                legacy_alias_targets.len(),
                1,
                "the pre-coordinate identity has one alias on its actual source-bound page"
            );
        }

        let source_id = page_events
            .iter()
            .find_map(|event| event["metadata"]["chat_source_ref"].as_str())
            .expect("admitted source row reference")
            .to_string();
        let source_ids_json =
            serde_json::to_string(&vec![source_id.clone()]).expect("canonical source id list");
        let renewal_query = format!(
            "revision={}&revalidate_ids={}",
            encode_remote_chat_query_component(&revision),
            encode_remote_chat_query_component(&source_ids_json),
        );
        let response = client_router
            .oneshot(client_bearer_request(
                &format!("{chat_path}?{renewal_query}"),
                "https://client.example",
                &client_token,
            ))
            .await
            .expect("sequential Chat renewal response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("sequential Chat renewal body");
        let renewal: serde_json::Value =
            serde_json::from_slice(&body).expect("sequential Chat renewal JSON");
        assert_eq!(
            renewal["revalidated_ids"],
            serde_json::json!([source_id.clone()])
        );
        assert!(renewal["events"].as_array().is_some_and(|events| {
            events
                .iter()
                .any(|event| event["metadata"]["chat_source_ref"] == source_id)
        }));
    }

    #[test]
    fn status_stream_client_messages_do_not_request_immediate_snapshot() {
        assert_eq!(
            status_stream_client_message_action(Some(axum::extract::ws::Message::Text(
                "ignored".into()
            ))),
            StatusStreamClientMessageAction::IgnoreUntilNextTick
        );
        assert_eq!(
            status_stream_client_message_action(Some(axum::extract::ws::Message::Close(None))),
            StatusStreamClientMessageAction::Close
        );
        assert_eq!(
            status_stream_client_message_action(None),
            StatusStreamClientMessageAction::Close
        );
    }

    #[test]
    fn session_response_exposes_csrf_nonce_and_lifetimes() {
        let session = session_record("sess-1", "dev-1", "csrf-1");
        let response = session_response_from_record(&session);

        assert_eq!(response.csrf_nonce, "csrf-1");
        assert!(response.expires_at.ends_with('Z'));
        assert!(response.absolute_expires_at.ends_with('Z'));
    }

    #[test]
    fn remote_automation_compat_empty_list_preserves_v1_shape() {
        assert_eq!(
            remote_automation_compat_empty_list_response(),
            serde_json::json!({ "automations": [] })
        );
    }

    #[test]
    fn session_cookie_header_is_host_prefixed_secure_and_http_only() {
        let header = remote_session_cookie_header_value("sess-1").expect("cookie header");
        let value = header.to_str().expect("header string");

        assert!(value.starts_with("__Host-wardian_remote_session=sess-1;"));
        assert!(value.contains("Path=/"));
        assert!(value.contains("Secure"));
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Strict"));
        assert!(!value.contains("Domain="));
    }

    fn action_headers(session: &RemoteSessionRecord) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("wardian.tailnet.ts.net"),
        );
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://wardian.tailnet.ts.net"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "{REMOTE_SESSION_COOKIE_NAME}={}",
                session.session_id
            ))
            .expect("cookie header"),
        );
        headers.insert(
            REMOTE_CSRF_HEADER_NAME,
            HeaderValue::from_str(&session.csrf_nonce).expect("csrf header"),
        );
        headers
    }

    fn reject_inbox_action_projection_unavailable<R: Runtime>(
        _app: AppHandle<R>,
        _request: RemoteInboxActionRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'static>>
    {
        Box::pin(async {
            Err(crate::remote::operations::REMOTE_AUTOMATION_INBOX_UNAVAILABLE.to_string())
        })
    }

    fn reject_inbox_action_generically<R: Runtime>(
        _app: AppHandle<R>,
        _request: RemoteInboxActionRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'static>>
    {
        Box::pin(async { Err("inbox_item_not_found".to_string()) })
    }

    #[test]
    fn auth_signature_message_is_bound_to_origin_device_and_challenge() {
        let challenge = crate::remote::models::AuthChallengeRecord {
            challenge_id: "challenge-1".to_string(),
            device_id: "dev-1".to_string(),
            nonce: "nonce-1".to_string(),
            canonical_origin: "https://wardian.tailnet.ts.net".to_string(),
            gateway_identity_fingerprint: "test-fingerprint".to_string(),
            audience: RemoteCredentialAudience::Pwa,
            client_origin: None,
            expires_at_ms: 1_060_000,
            used: false,
        };

        assert_eq!(
            auth_signature_message(&challenge),
            b"wardian.remote.auth.v1\norigin:https://wardian.tailnet.ts.net\ndevice:dev-1\nchallenge:challenge-1\nnonce:nonce-1".to_vec()
        );
    }

    #[test]
    fn client_auth_signing_payload_matches_the_frozen_lf_delimited_vector() {
        let challenge = crate::remote::models::AuthChallengeRecord {
            challenge_id: "challenge_123".to_string(),
            device_id: "device_456".to_string(),
            nonce: "nonce_789".to_string(),
            canonical_origin: "https://gateway.example".to_string(),
            gateway_identity_fingerprint: "server-fp".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            client_origin: Some("https://client.example".to_string()),
            expires_at_ms: 60_000,
            used: false,
        };

        assert_eq!(
            client_auth_signing_payload(&challenge).expect("valid challenge payload"),
            "wardian.remote.client.auth.v1\norigin:https://gateway.example\nclient_origin:https://client.example\naudience:wardian_remote_client_v1\ndevice:device_456\nchallenge:challenge_123\nnonce:nonce_789"
        );
        let mut invalid = challenge;
        invalid.device_id = "device\nnonce:forged".to_string();
        assert!(client_auth_signing_payload(&invalid).is_err());
    }

    #[test]
    fn gateway_rejection_audit_record_has_no_session_identity() {
        let record = build_gateway_rejection_audit_record(
            Some("https://wardian.tailnet.ts.net"),
            GatewayAuditEvent::rejected("gateway_policy", "request_boundary", "origin_forbidden"),
        );

        assert_eq!(record.schema_version, REMOTE_AUDIT_SCHEMA_VERSION);
        assert_eq!(record.device_id, None);
        assert_eq!(record.session_id, None);
        assert_eq!(
            record.origin.as_deref(),
            Some("https://wardian.tailnet.ts.net")
        );
        assert_eq!(record.outcome, "rejected");
        assert_eq!(record.error_code.as_deref(), Some("origin_forbidden"));
    }

    #[test]
    fn static_asset_route_maps_remote_shell_to_index_and_rejects_traversal() {
        assert_eq!(
            static_asset_path_for_route("/remote").expect("remote shell"),
            "index.html"
        );
        assert_eq!(
            static_asset_path_for_route("/remote/").expect("remote shell"),
            "index.html"
        );
        assert_eq!(
            static_asset_path_for_route("/manifest.webmanifest").expect("manifest"),
            "manifest.webmanifest"
        );
        assert_eq!(
            static_asset_path_for_route("/icon-maskable.png").expect("maskable icon"),
            "icon-maskable.png"
        );
        assert_eq!(
            static_asset_path_for_route("/assets/index.js").expect("asset"),
            "assets/index.js"
        );
        assert_eq!(
            static_asset_path_for_route("/assets/../secret").expect_err("traversal rejected"),
            "asset_path_forbidden"
        );
    }

    #[test]
    fn manifest_is_served_with_a_pwa_content_type() {
        assert_eq!(
            static_asset_content_type("manifest.webmanifest", "text/html".to_string()),
            "application/manifest+json"
        );
        assert_eq!(
            static_asset_content_type("remote-sw.js", "text/javascript".to_string()),
            "text/javascript"
        );
    }

    #[test]
    fn active_gateway_config_rejects_disabled_or_rebound_saved_config() {
        let bound = config();
        assert_eq!(
            active_gateway_config_for_bound_server(Some(bound.clone()), &bound)
                .expect("enabled matching config"),
            bound
        );

        let mut disabled = config();
        disabled.enabled = false;
        let error = active_gateway_config_for_bound_server(Some(disabled), &config())
            .expect_err("disabled config rejected");
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "remote_access_disabled");

        let mut rebound = config();
        rebound.loopback_port += 1;
        let error = active_gateway_config_for_bound_server(Some(rebound), &config())
            .expect_err("rebound config rejected");
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "remote_gateway_reconfigured");
    }

    #[test]
    fn gateway_audit_record_includes_session_and_target_identity() {
        let session = session_record("sess-1", "dev-1", "csrf-1");
        let record = build_gateway_audit_record(
            &session,
            "https://wardian.tailnet.ts.net",
            GatewayAuditEvent::accepted("agent_action", "pause").target("agent", "agent-1"),
        );

        assert_eq!(record.schema_version, REMOTE_AUDIT_SCHEMA_VERSION);
        assert_eq!(record.device_id.as_deref(), Some("dev-1"));
        assert_eq!(record.session_id.as_deref(), Some("sess-1"));
        assert_eq!(
            record.origin.as_deref(),
            Some("https://wardian.tailnet.ts.net")
        );
        assert_eq!(record.event_type, "agent_action");
        assert_eq!(record.action, "pause");
        assert_eq!(record.target_type.as_deref(), Some("agent"));
        assert_eq!(record.target_id.as_deref(), Some("agent-1"));
    }

    fn session_record(session_id: &str, device_id: &str, csrf_nonce: &str) -> RemoteSessionRecord {
        RemoteSessionRecord {
            session_id: session_id.to_string(),
            device_id: device_id.to_string(),
            created_at_ms: 1_000_000,
            last_seen_at_ms: 1_000_000,
            expires_at_ms: 1_900_000,
            absolute_expires_at_ms: 44_200_000,
            csrf_nonce: csrf_nonce.to_string(),
            revoked: false,
        }
    }
}
