use crate::remote::models::{
    RemoteAgentActionRequest, RemoteAuditRecord, RemoteGatewayConfig, RemoteSessionRecord,
    RemoteWebSocketTicketRequest, RemoteWebSocketTicketResponse, RemoteWorkflowRunRequest,
    RemoteWorkflowStopRequest, REMOTE_AUDIT_SCHEMA_VERSION,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Json, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tauri::{AppHandle, Manager};

const REMOTE_SESSION_COOKIE_NAME: &str = "__Host-wardian_remote_session";
const REMOTE_CSRF_HEADER_NAME: &str = "x-wardian-csrf";
const REMOTE_STATUS_STREAM_NAME: &str = "agent_status";

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
        .route("/remote/api/pairing/submit", post(submit_pairing))
        .route("/remote/api/auth/challenge", post(create_auth_challenge))
        .route("/remote/api/auth/session", post(create_auth_session))
        .route("/remote/api/agents", get(list_remote_agents))
        .route("/remote/api/agents/action", post(run_agent_action))
        .route("/remote/api/workflows", get(list_remote_workflows))
        .route("/remote/api/workflows/run", post(run_workflow))
        .route("/remote/api/workflows/stop", post(stop_workflow))
        .route("/remote/api/ws-ticket", post(create_ws_ticket))
        .route("/remote/api/status-stream", get(status_stream_upgrade))
        .with_state(RemoteGatewayContext { app, config })
}

#[derive(Clone)]
struct RemoteGatewayContext {
    app: AppHandle,
    config: RemoteGatewayConfig,
}

async fn remote_health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "ok": true }))
}

async fn submit_pairing(
    State(_ctx): State<RemoteGatewayContext>,
) -> Result<axum::Json<serde_json::Value>, RemoteGatewayError> {
    Err(RemoteGatewayError::bad_request(
        "pairing_approval_not_ready",
    ))
}

async fn create_auth_challenge(
    State(_ctx): State<RemoteGatewayContext>,
) -> Result<axum::Json<serde_json::Value>, RemoteGatewayError> {
    Err(RemoteGatewayError::bad_request("device_not_found"))
}

async fn create_auth_session(
    State(_ctx): State<RemoteGatewayContext>,
) -> Result<axum::Json<serde_json::Value>, RemoteGatewayError> {
    Err(RemoteGatewayError::bad_request("device_not_found"))
}

async fn list_remote_agents(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_request_boundary(&ctx.config, &headers, false)?;
    let session = require_remote_session(&ctx, &headers).await?;
    let state = ctx.app.state::<crate::state::AppState>();
    let agents = crate::remote::operations::remote_agent_roster(&state).await;
    audit_gateway_event(
        &session,
        &origin,
        "roster_read",
        "list_agents",
        None,
        None,
        "accepted",
        None,
    );
    Ok(Json(serde_json::json!({ "agents": agents })))
}

async fn run_agent_action(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteAgentActionRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_request_boundary(&ctx.config, &headers, true)?;
    let session = require_remote_session(&ctx, &headers).await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            "agent_action",
            &request.action,
            Some("agent"),
            Some(&request.target),
            "rejected",
            Some(error.code),
        );
        return Err(error);
    }
    if crate::remote::operations::run_remote_agent_action(&ctx.app, request.clone())
        .await
        .is_err()
    {
        let code = "agent_action_failed";
        audit_gateway_event(
            &session,
            &origin,
            "agent_action",
            &request.action,
            Some("agent"),
            Some(&request.target),
            "rejected",
            Some(code),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    audit_gateway_event(
        &session,
        &origin,
        "agent_action",
        &request.action,
        Some("agent"),
        Some(&request.target),
        "accepted",
        None,
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_remote_workflows(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_request_boundary(&ctx.config, &headers, false)?;
    let session = require_remote_session(&ctx, &headers).await?;
    let workflows = match crate::remote::operations::remote_workflow_summaries() {
        Ok(workflows) => workflows,
        Err(_) => {
            let code = "workflow_list_failed";
            audit_gateway_event(
                &session,
                &origin,
                "workflow_read",
                "list_workflows",
                None,
                None,
                "rejected",
                Some(code),
            );
            return Err(RemoteGatewayError::bad_request(code));
        }
    };
    audit_gateway_event(
        &session,
        &origin,
        "workflow_read",
        "list_workflows",
        None,
        None,
        "accepted",
        None,
    );
    Ok(Json(serde_json::json!({ "workflows": workflows })))
}

async fn run_workflow(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteWorkflowRunRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_request_boundary(&ctx.config, &headers, true)?;
    let session = require_remote_session(&ctx, &headers).await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            "workflow_action",
            "run",
            Some("workflow"),
            Some(&request.workflow_id),
            "rejected",
            Some(error.code),
        );
        return Err(error);
    }
    if request.workflow_id.trim().is_empty() {
        let code = "workflow_id_required";
        audit_gateway_event(
            &session,
            &origin,
            "workflow_action",
            "run",
            Some("workflow"),
            Some(&request.workflow_id),
            "rejected",
            Some(code),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    if crate::workflow_engine::run_workflow(
        ctx.app.clone(),
        request.workflow_id.clone(),
        request.payload.clone(),
    )
    .await
    .is_err()
    {
        let code = "workflow_run_failed";
        audit_gateway_event(
            &session,
            &origin,
            "workflow_action",
            "run",
            Some("workflow"),
            Some(&request.workflow_id),
            "rejected",
            Some(code),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    audit_gateway_event(
        &session,
        &origin,
        "workflow_action",
        "run",
        Some("workflow"),
        Some(&request.workflow_id),
        "accepted",
        None,
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn stop_workflow(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteWorkflowStopRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_request_boundary(&ctx.config, &headers, true)?;
    let session = require_remote_session(&ctx, &headers).await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            "workflow_action",
            "stop",
            Some("workflow_run"),
            Some(&request.run_instance_id),
            "rejected",
            Some(error.code),
        );
        return Err(error);
    }
    if request.run_instance_id.trim().is_empty() {
        let code = "run_instance_id_required";
        audit_gateway_event(
            &session,
            &origin,
            "workflow_action",
            "stop",
            Some("workflow_run"),
            Some(&request.run_instance_id),
            "rejected",
            Some(code),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    crate::workflow_engine::stop_workflow_run(ctx.app.clone(), &request.run_instance_id).await;
    audit_gateway_event(
        &session,
        &origin,
        "workflow_action",
        "stop",
        Some("workflow_run"),
        Some(&request.run_instance_id),
        "accepted",
        None,
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn create_ws_ticket(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteWebSocketTicketRequest>,
) -> Result<Json<RemoteWebSocketTicketResponse>, RemoteGatewayError> {
    let origin = require_request_boundary(&ctx.config, &headers, true)?;
    let session = require_remote_session(&ctx, &headers).await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            "websocket_ticket",
            "create",
            Some("stream"),
            Some(&request.stream),
            "rejected",
            Some(error.code),
        );
        return Err(error);
    }
    let stream = match validate_remote_stream(&request.stream) {
        Ok(stream) => stream,
        Err(code) => {
            audit_gateway_event(
                &session,
                &origin,
                "websocket_ticket",
                "create",
                Some("stream"),
                Some(&request.stream),
                "rejected",
                Some(code),
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
        "websocket_ticket",
        "create",
        Some("stream"),
        Some(stream),
        "accepted",
        None,
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
    require_request_boundary(&ctx.config, &headers, true)?;
    Ok(ws.on_upgrade(move |socket| async move {
        handle_status_socket(ctx, socket).await;
    }))
}

async fn handle_status_socket(ctx: RemoteGatewayContext, mut socket: WebSocket) {
    let Some(Ok(first_message)) = socket.recv().await else {
        return;
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

    'stream: loop {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let session_active = {
            let runtime = state.remote_runtime.lock().await;
            status_stream_session_is_active(&runtime, &ticket_record.session_id, now_ms)
        };
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

async fn send_status_socket_error(socket: &mut WebSocket, code: &'static str) {
    let payload = serde_json::json!({
        "type": "error",
        "code": code,
    });
    let _ = socket.send(Message::Text(payload.to_string().into())).await;
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
        if !matches!(fetch_site, "same-origin" | "none") {
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
        _ => Err("unsupported_stream"),
    }
}

async fn require_remote_session(
    ctx: &RemoteGatewayContext,
    headers: &HeaderMap,
) -> Result<RemoteSessionRecord, RemoteGatewayError> {
    let session_id = remote_session_cookie_value(headers)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let state = ctx.app.state::<crate::state::AppState>();
    let runtime = state.remote_runtime.lock().await;
    let session = runtime
        .sessions
        .get(&session_id)
        .cloned()
        .ok_or_else(|| RemoteGatewayError::unauthorized("session_not_found"))?;
    if !crate::remote::auth::session_is_active(&session, now_ms) {
        return Err(RemoteGatewayError::unauthorized("session_expired"));
    }
    Ok(session)
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

fn audit_gateway_event(
    session: &RemoteSessionRecord,
    origin: &str,
    event_type: &str,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    outcome: &str,
    error_code: Option<&str>,
) {
    let record = build_gateway_audit_record(
        session,
        origin,
        event_type,
        action,
        target_type,
        target_id,
        outcome,
        error_code,
    );
    if let Err(error) = crate::remote::audit::append_audit_record(&record) {
        crate::utils::logging::log_debug(&format!("[Wardian] remote audit append failed: {error}"));
    }
}

fn build_gateway_audit_record(
    session: &RemoteSessionRecord,
    origin: &str,
    event_type: &str,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    outcome: &str,
    error_code: Option<&str>,
) -> RemoteAuditRecord {
    RemoteAuditRecord {
        schema_version: REMOTE_AUDIT_SCHEMA_VERSION,
        event_id: crate::remote::crypto::random_url_token(12),
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        request_id: crate::remote::crypto::random_url_token(12),
        device_id: Some(session.device_id.clone()),
        session_id: Some(session.session_id.clone()),
        origin: Some(origin.to_string()),
        event_type: event_type.to_string(),
        action: action.to_string(),
        target_type: target_type.map(str::to_string),
        target_id: target_id.map(str::to_string),
        outcome: outcome.to_string(),
        error_code: error_code.map(str::to_string),
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
}

impl RemoteGatewayError {
    fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
        }
    }

    fn unauthorized(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
        }
    }

    fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
        }
    }
}

impl IntoResponse for RemoteGatewayError {
    fn into_response(self) -> Response {
        let body = axum::Json(serde_json::json!({
            "ok": false,
            "code": self.code,
        }));
        (self.status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{
        RemoteGatewayConfig, RemoteSessionRecord, REMOTE_AUDIT_SCHEMA_VERSION,
        REMOTE_SETTINGS_SCHEMA_VERSION,
    };
    use axum::http::{header, HeaderMap, HeaderValue};

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

    #[test]
    fn gateway_bad_request_errors_preserve_machine_code() {
        let error = RemoteGatewayError::bad_request("device_not_found");

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "device_not_found");
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
            validate_remote_stream("terminal").expect_err("unsupported stream"),
            "unsupported_stream"
        );
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
    fn gateway_audit_record_includes_session_and_target_identity() {
        let session = session_record("sess-1", "dev-1", "csrf-1");
        let record = build_gateway_audit_record(
            &session,
            "https://wardian.tailnet.ts.net",
            "agent_action",
            "pause",
            Some("agent"),
            Some("agent-1"),
            "accepted",
            None,
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
