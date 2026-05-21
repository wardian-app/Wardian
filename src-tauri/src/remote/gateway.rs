use crate::remote::models::{
    AuthChallengeRequest, AuthChallengeResponse, AuthSessionRequest, AuthSessionResponse,
    DeviceRecord, PairingSubmitRequest, PairingSubmitResponse, PendingPairingDecision,
    PendingPairingRequestRecord, RemoteAgentActionRequest, RemoteAuditRecord, RemoteGatewayConfig,
    RemoteSessionRecord, RemoteWebSocketTicketRequest, RemoteWebSocketTicketResponse,
    RemoteWorkflowRunRequest, RemoteWorkflowStopRequest, REMOTE_AUDIT_SCHEMA_VERSION,
};
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Json, Path as AxumPath, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use base64::Engine;
use p256::ecdsa::VerifyingKey;
use p256::pkcs8::DecodePublicKey;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tauri::{AppHandle, Manager};

const REMOTE_SESSION_COOKIE_NAME: &str = "__Host-wardian_remote_session";
const REMOTE_CSRF_HEADER_NAME: &str = "x-wardian-csrf";
const REMOTE_STATUS_STREAM_NAME: &str = "agent_status";
const WEBSOCKET_FIRST_TICKET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
        .route("/remote", get(serve_remote_shell))
        .route("/remote/", get(serve_remote_shell))
        .route("/manifest.webmanifest", get(serve_manifest))
        .route("/remote-sw.js", get(serve_remote_sw))
        .route("/icon.png", get(serve_icon))
        .route("/assets/{*asset_path}", get(serve_asset_path))
        .route("/remote/api/health", get(remote_health))
        .route("/remote/api/session", get(current_remote_session))
        .route("/remote/api/pairing/submit", post(submit_pairing))
        .route("/remote/api/pairing/{request_id}", get(pairing_status))
        .route("/remote/api/auth/challenge", post(create_auth_challenge))
        .route("/remote/api/auth/session", post(create_auth_session))
        .route("/remote/api/agents", get(list_remote_agents))
        .route("/remote/api/agents/action", post(run_agent_action))
        .route("/remote/api/workflows", get(list_remote_workflows))
        .route("/remote/api/workflows/run", post(run_workflow))
        .route("/remote/api/workflows/stop", post(stop_workflow))
        .route("/remote/api/ws-ticket", post(create_ws_ticket))
        .route("/remote/api/status-stream", get(status_stream_upgrade))
        .layer(DefaultBodyLimit::max(64 * 1024))
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
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.mime_type().to_string())
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
        "manifest.webmanifest" | "remote-sw.js" | "icon.png" => Ok(asset_path.to_string()),
        path if path.starts_with("assets/") => Ok(path.to_string()),
        _ => Err("asset_path_forbidden"),
    }
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

async fn create_auth_challenge(
    State(ctx): State<RemoteGatewayContext>,
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

async fn create_auth_session(
    State(ctx): State<RemoteGatewayContext>,
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

    let session = {
        let state = ctx.app.state::<crate::state::AppState>();
        let mut runtime = state.remote_runtime.lock().await;
        crate::remote::auth::create_session(&mut runtime, &device.device_id, now_ms)
    };
    update_device_last_used(&device.device_id, &millis_to_rfc3339(now_ms))?;
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
    if crate::remote::operations::run_remote_agent_action(&ctx.app, request.clone())
        .await
        .is_err()
    {
        let code = "agent_action_failed";
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("agent_action", &request.action, code)
                .target("agent", &request.target),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("agent_action", &request.action)
            .target("agent", &request.target),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
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

async fn list_remote_workflows(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, false, "list_workflows")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "workflow_read", "list_workflows")
            .await?;
    let workflows = match crate::remote::operations::remote_workflow_summaries() {
        Ok(workflows) => workflows,
        Err(_) => {
            let code = "workflow_list_failed";
            audit_gateway_event(
                &session,
                &origin,
                GatewayAuditEvent::rejected("workflow_read", "list_workflows", code),
            );
            return Err(RemoteGatewayError::bad_request(code));
        }
    };
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("workflow_read", "list_workflows"),
    );
    Ok(Json(serde_json::json!({ "workflows": workflows })))
}

async fn run_workflow(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteWorkflowRunRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, true, "run_workflow")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "workflow_action", "run").await?;
    require_mutation_rate_limit(&ctx, &session, &origin, "workflow_action", "run").await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("workflow_action", "run", error.code)
                .target("workflow", &request.workflow_id),
        );
        return Err(error);
    }
    if request.workflow_id.trim().is_empty() {
        let code = "workflow_id_required";
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("workflow_action", "run", code)
                .target("workflow", &request.workflow_id),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    if request.payload.is_some() {
        let code = "remote_workflow_payload_not_supported";
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("workflow_action", "run", code)
                .target("workflow", &request.workflow_id),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    if crate::workflow_engine::run_workflow(ctx.app.clone(), request.workflow_id.clone(), None)
        .await
        .is_err()
    {
        let code = "workflow_run_failed";
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("workflow_action", "run", code)
                .target("workflow", &request.workflow_id),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("workflow_action", "run")
            .target("workflow", &request.workflow_id),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn stop_workflow(
    State(ctx): State<RemoteGatewayContext>,
    headers: HeaderMap,
    Json(request): Json<RemoteWorkflowStopRequest>,
) -> Result<Json<serde_json::Value>, RemoteGatewayError> {
    let origin = require_audited_request_boundary(&ctx.config, &headers, true, "stop_workflow")?;
    let session =
        require_audited_remote_session(&ctx, &headers, &origin, "workflow_action", "stop").await?;
    require_mutation_rate_limit(&ctx, &session, &origin, "workflow_action", "stop").await?;
    if let Err(error) = require_csrf_header(&session, &headers) {
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("workflow_action", "stop", error.code)
                .target("workflow_run", &request.run_instance_id),
        );
        return Err(error);
    }
    if request.run_instance_id.trim().is_empty() {
        let code = "run_instance_id_required";
        audit_gateway_event(
            &session,
            &origin,
            GatewayAuditEvent::rejected("workflow_action", "stop", code)
                .target("workflow_run", &request.run_instance_id),
        );
        return Err(RemoteGatewayError::bad_request(code));
    }
    crate::workflow_engine::stop_workflow_run(ctx.app.clone(), &request.run_instance_id).await;
    audit_gateway_event(
        &session,
        &origin,
        GatewayAuditEvent::accepted("workflow_action", "stop")
            .target("workflow_run", &request.run_instance_id),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
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

fn require_audited_request_boundary(
    config: &RemoteGatewayConfig,
    headers: &HeaderMap,
    require_origin: bool,
    action: &'static str,
) -> Result<String, RemoteGatewayError> {
    match require_request_boundary(config, headers, require_origin) {
        Ok(origin) => Ok(origin),
        Err(error) => {
            audit_gateway_event_without_session(
                canonical_origin_for_audit(config).as_deref(),
                GatewayAuditEvent::rejected("gateway_policy", action, error.code),
            );
            Err(error)
        }
    }
}

async fn require_audited_remote_session(
    ctx: &RemoteGatewayContext,
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
    let mut runtime = state.remote_runtime.lock().await;
    let session = runtime
        .sessions
        .get_mut(&session_id)
        .ok_or_else(|| RemoteGatewayError::unauthorized("session_not_found"))?;
    if crate::remote::auth::refresh_session_activity(session, now_ms).is_err() {
        return Err(RemoteGatewayError::unauthorized("session_expired"));
    }
    Ok(session.clone())
}

async fn require_mutation_rate_limit(
    ctx: &RemoteGatewayContext,
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
        PendingPairingDecision::Pending => "pending",
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
        .find(|device| device.device_id == device_id && device.revoked_at.is_none())
        .ok_or_else(|| RemoteGatewayError::not_found("device_not_found"))
}

fn update_device_last_used(device_id: &str, last_used_at: &str) -> Result<(), RemoteGatewayError> {
    let mut store = crate::remote::storage::load_device_store()
        .map_err(|_| RemoteGatewayError::bad_request("device_store_failed"))?;
    let Some(device) = store
        .devices
        .iter_mut()
        .find(|device| device.device_id == device_id)
    else {
        return Err(RemoteGatewayError::bad_request("device_not_found"));
    };
    device.last_used_at = Some(last_used_at.to_string());
    crate::remote::storage::save_device_store(&store)
        .map_err(|_| RemoteGatewayError::bad_request("device_store_failed"))?;
    Ok(())
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
        "auth_challenge_not_found" => "auth_challenge_not_found",
        "auth_challenge_used" => "auth_challenge_used",
        "auth_challenge_expired" => "auth_challenge_expired",
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

    fn too_many_requests(code: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code,
        }
    }

    fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
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

    #[test]
    fn auth_signature_message_is_bound_to_origin_device_and_challenge() {
        let challenge = crate::remote::models::AuthChallengeRecord {
            challenge_id: "challenge-1".to_string(),
            device_id: "dev-1".to_string(),
            nonce: "nonce-1".to_string(),
            canonical_origin: "https://wardian.tailnet.ts.net".to_string(),
            expires_at_ms: 1_060_000,
            used: false,
        };

        assert_eq!(
            auth_signature_message(&challenge),
            b"wardian.remote.auth.v1\norigin:https://wardian.tailnet.ts.net\ndevice:dev-1\nchallenge:challenge-1\nnonce:nonce-1".to_vec()
        );
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
            static_asset_path_for_route("/assets/index.js").expect("asset"),
            "assets/index.js"
        );
        assert_eq!(
            static_asset_path_for_route("/assets/../secret").expect_err("traversal rejected"),
            "asset_path_forbidden"
        );
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
