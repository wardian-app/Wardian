use crate::state::{
    AppState, FileResourceRuntime, FileResourceSnapshotV1, FileResourceTextV1,
    FileResourceTicketV1, UserFileGrantV1,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::http::{header, Request, Response, StatusCode};
use tauri_plugin_dialog::DialogExt as _;
use wardian_core::files::{AuthorizedRootService, FileResourceErrorV1};
use wardian_core::models::AgentConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OpenFileResourceRequestV1 {
    pub path: String,
    pub agent_id: Option<String>,
    pub user_file_capability_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CloseFileResourceRequestV1 {
    pub subscription_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReadFileResourceTextRequestV1 {
    pub resource_id: String,
    pub subscription_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueFileResourceTicketRequestV1 {
    pub resource_id: String,
    pub subscription_id: String,
    pub revision: u64,
    pub renderer_lease_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CloseFileRendererLeaseRequestV1 {
    pub resource_id: String,
    pub subscription_id: String,
    pub renderer_lease_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PickFileResourceRequestV1 {
    pub title: Option<String>,
}

#[tauri::command]
pub async fn open_file_resource(
    request: OpenFileResourceRequestV1,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
    open_file_resource_for_app(request, &state, Some(app)).await
}

async fn open_file_resource_for_app(
    request: OpenFileResourceRequestV1,
    state: &AppState,
    app: Option<tauri::AppHandle>,
) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
    match (
        request.agent_id.as_deref(),
        request.user_file_capability_id.as_deref(),
    ) {
        (Some(agent_id), None) => {
            let config = current_agent_config(state, agent_id).await?;
            state
                .file_resources
                .open_agent_file(agent_id, &config, Path::new(&request.path), app)
                .await
        }
        (None, Some(capability_id)) => {
            state
                .file_resources
                .open_user_file(capability_id, Path::new(&request.path), app)
                .await
        }
        (None, None) => open_trusted_workbench_file(state, Path::new(&request.path), app).await,
        (Some(_), Some(_)) => Err(resource_error(
            "invalid_request",
            "agent and user file authorization capabilities are mutually exclusive",
        )),
    }
}

/// Resolves durable Workbench file identity against current backend-owned
/// authorization. No authority is inferred or persisted by the frontend.
async fn open_trusted_workbench_file(
    state: &AppState,
    path: &Path,
    app: Option<tauri::AppHandle>,
) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
    let mut agent_configs = {
        let agents = state.agents.lock().await;
        agents
            .iter()
            .filter_map(|(agent_id, agent)| {
                agent
                    .config
                    .lock()
                    .ok()
                    .map(|config| (agent_id.clone(), config.clone()))
            })
            .collect::<Vec<_>>()
    };
    agent_configs.sort_by(|left, right| left.0.cmp(&right.0));

    for (agent_id, config) in agent_configs {
        if config.session_id != agent_id {
            continue;
        }
        let authorized = AuthorizedRootService::from_agent_config(&config)
            .and_then(|roots| roots.authorize_existing_file(path));
        if authorized.is_err() {
            continue;
        }
        return state
            .file_resources
            .open_agent_file(&agent_id, &config, path, app.clone())
            .await;
    }

    if let Some(snapshot) = state
        .file_resources
        .open_matching_user_file(path, app)
        .await?
    {
        return Ok(snapshot);
    }
    Err(resource_error(
        "unauthorized_path",
        "file is outside every current agent root and exact live picker grant",
    ))
}

#[tauri::command]
pub async fn close_file_resource(
    request: CloseFileResourceRequestV1,
    state: tauri::State<'_, AppState>,
) -> Result<(), FileResourceErrorV1> {
    state.file_resources.close(&request.subscription_id).await
}

#[tauri::command]
pub async fn read_file_resource_text(
    request: ReadFileResourceTextRequestV1,
    state: tauri::State<'_, AppState>,
) -> Result<FileResourceTextV1, FileResourceErrorV1> {
    let config =
        current_subscription_agent_config(&state, &request.resource_id, &request.subscription_id)
            .await?;
    state
        .file_resources
        .read_text(
            &request.resource_id,
            &request.subscription_id,
            request.revision,
            config.as_ref(),
        )
        .await
}

#[tauri::command]
pub async fn issue_file_resource_ticket(
    request: IssueFileResourceTicketRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<FileResourceTicketV1, FileResourceErrorV1> {
    let config =
        current_subscription_agent_config(&state, &request.resource_id, &request.subscription_id)
            .await?;
    state
        .file_resources
        .issue_ticket_for_webview(
            &request.resource_id,
            &request.subscription_id,
            request.revision,
            config.as_ref(),
            &request.renderer_lease_id,
            Some(webview.label()),
        )
        .await
}

#[tauri::command]
pub async fn close_file_renderer_lease(
    request: CloseFileRendererLeaseRequestV1,
    state: tauri::State<'_, AppState>,
    webview: tauri::WebviewWindow,
) -> Result<(), FileResourceErrorV1> {
    state
        .file_resources
        .close_renderer_lease(
            &request.resource_id,
            &request.subscription_id,
            &request.renderer_lease_id,
            Some(webview.label()),
        )
        .await
}

#[tauri::command]
pub async fn pick_file_resource(
    request: PickFileResourceRequestV1,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Option<UserFileGrantV1>, FileResourceErrorV1> {
    let mut picker = app.dialog().file();
    if let Some(title) = request.title {
        picker = picker.set_title(title);
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    picker.pick_file(move |selected| {
        let _ = sender.send(selected);
    });
    let selected = receiver
        .await
        .map_err(|_| resource_error("picker_unavailable", "native file picker did not respond"))?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected.into_path().map_err(|cause| {
        resource_error(
            "unavailable_path",
            format!("selected file is not a local filesystem path: {cause}"),
        )
    })?;
    record_picked_file(&state.file_resources, &selected)
        .await
        .map(Some)
}

pub(crate) async fn record_picked_file(
    runtime: &FileResourceRuntime,
    selected: &Path,
) -> Result<UserFileGrantV1, FileResourceErrorV1> {
    runtime.record_user_file(selected).await
}

async fn current_subscription_agent_config(
    state: &AppState,
    resource_id: &str,
    subscription_id: &str,
) -> Result<Option<AgentConfig>, FileResourceErrorV1> {
    let agent_id = state
        .file_resources
        .authorization_agent_id(resource_id, subscription_id)
        .await?;
    match agent_id {
        Some(agent_id) => current_agent_config(state, &agent_id).await.map(Some),
        None => Ok(None),
    }
}

async fn current_agent_config(
    state: &AppState,
    agent_id: &str,
) -> Result<AgentConfig, FileResourceErrorV1> {
    let config = {
        let agents = state.agents.lock().await;
        let agent = agents.get(agent_id).ok_or_else(|| {
            resource_error(
                "unauthorized_path",
                "agent authorization is no longer active",
            )
        })?;
        agent.config.clone()
    };
    config.lock().map(|config| config.clone()).map_err(|_| {
        resource_error(
            "runtime_unavailable",
            "agent configuration lock is unavailable",
        )
    })
}

#[cfg(test)]
pub(crate) fn parse_byte_range(
    header: Option<&str>,
    size_bytes: u64,
) -> Result<(u64, u64), FileResourceErrorV1> {
    crate::state::file_resources::parse_byte_range(header, size_bytes)
}

pub async fn file_resource_protocol_response(
    runtime: &FileResourceRuntime,
    webview_label: &str,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let is_head = request.method() == tauri::http::Method::HEAD;
    let ticket_id = request
        .uri()
        .path()
        .trim_matches('/')
        .split('/')
        .next()
        .filter(|value| !value.is_empty());
    let Some(ticket_id) = ticket_id else {
        return head_aware_response(
            protocol_error(
                StatusCode::BAD_REQUEST,
                resource_error("invalid_ticket", "resource URL has no ticket"),
            ),
            is_head,
        );
    };
    if request.method() != tauri::http::Method::GET && request.method() != tauri::http::Method::HEAD
    {
        let mut response = protocol_error(
            StatusCode::METHOD_NOT_ALLOWED,
            resource_error(
                "invalid_request",
                "resource protocol supports GET and HEAD only",
            ),
        );
        response.headers_mut().insert(
            header::ALLOW,
            tauri::http::HeaderValue::from_static("GET, HEAD"),
        );
        return response;
    }
    let range = match request.headers().get(header::RANGE) {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => {
                let total_size = runtime
                    .ticket_size_for_webview(ticket_id, Some(webview_label))
                    .await
                    .ok();
                return head_aware_response(
                    protocol_range_error(
                        resource_error("invalid_range", "range header is not valid HTTP text"),
                        total_size,
                    ),
                    is_head,
                );
            }
        },
        None => None,
    };
    let read = if request.method() == tauri::http::Method::HEAD {
        runtime
            .verify_ticket_range_for_webview(ticket_id, range, Some(webview_label))
            .await
    } else {
        runtime
            .read_ticket_range_for_webview(ticket_id, range, Some(webview_label))
            .await
    };
    match read {
        Ok(read) => {
            let content_length = read.end - read.start + 1;
            let mut builder = Response::builder()
                .status(if read.partial {
                    StatusCode::PARTIAL_CONTENT
                } else {
                    StatusCode::OK
                })
                .header(header::CONTENT_TYPE, read.mime_type)
                .header("X-Content-Type-Options", "nosniff")
                .header(header::CACHE_CONTROL, "no-store")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, content_length.to_string());
            if read.partial {
                builder = builder.header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", read.start, read.end, read.total_size),
                );
            }
            builder
                .body(read.bytes)
                .unwrap_or_else(|_| fallback_protocol_error())
        }
        Err(error) => {
            let status = match error.code() {
                "range_not_satisfiable" | "invalid_range" => StatusCode::RANGE_NOT_SATISFIABLE,
                "expired_ticket" => StatusCode::GONE,
                "stale_revision" => StatusCode::CONFLICT,
                "unauthorized_ticket" | "unauthorized_path" => StatusCode::FORBIDDEN,
                "invalid_ticket" => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            };
            if status == StatusCode::RANGE_NOT_SATISFIABLE {
                let total_size = runtime
                    .ticket_size_for_webview(ticket_id, Some(webview_label))
                    .await
                    .ok();
                return head_aware_response(protocol_range_error(error, total_size), is_head);
            }
            head_aware_response(protocol_error(status, error), is_head)
        }
    }
}

fn head_aware_response(mut response: Response<Vec<u8>>, is_head: bool) -> Response<Vec<u8>> {
    if is_head {
        response.body_mut().clear();
    }
    response
}

fn protocol_range_error(error: FileResourceErrorV1, total_size: Option<u64>) -> Response<Vec<u8>> {
    let mut response = protocol_error(StatusCode::RANGE_NOT_SATISFIABLE, error);
    if let Some(total_size) = total_size {
        let content_range = format!("bytes */{total_size}");
        if let Ok(value) = tauri::http::HeaderValue::from_bytes(content_range.as_bytes()) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }
    response
}

fn protocol_error(status: StatusCode, error: FileResourceErrorV1) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(error.to_string().into_bytes())
        .unwrap_or_else(|_| fallback_protocol_error())
}

fn fallback_protocol_error() -> Response<Vec<u8>> {
    Response::new(b"file resource response unavailable".to_vec())
}

fn resource_error(code: &str, message: impl Into<String>) -> FileResourceErrorV1 {
    FileResourceErrorV1::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use wardian_core::models::AgentConfig;

    async fn install_test_agent(state: &AppState, config: AgentConfig) {
        let session_id = config.session_id.clone();
        state.agents.lock().await.insert(
            session_id,
            crate::restored_agent_without_process(config, "idle", String::new(), None, None),
        );
    }

    async fn open_trusted(
        state: &AppState,
        path: &Path,
    ) -> Result<FileResourceSnapshotV1, FileResourceErrorV1> {
        open_file_resource_for_app(
            OpenFileResourceRequestV1 {
                path: path.to_string_lossy().into_owned(),
                agent_id: None,
                user_file_capability_id: None,
            },
            state,
            None,
        )
        .await
    }

    #[test]
    fn file_resources_command_requests_are_typed_snake_case_dtos() {
        let open: OpenFileResourceRequestV1 = serde_json::from_value(json!({
            "path": "/workspace/report.md",
            "agent_id": "agent-a",
            "user_file_capability_id": null
        }))
        .expect("open request");
        assert_eq!(open.agent_id.as_deref(), Some("agent-a"));

        let read: ReadFileResourceTextRequestV1 = serde_json::from_value(json!({
            "resource_id": "file:/workspace/report.md",
            "subscription_id": "subscription-a",
            "revision": 7
        }))
        .expect("read request");
        assert_eq!(read.revision, 7);

        let issue: IssueFileResourceTicketRequestV1 = serde_json::from_value(json!({
            "resource_id": "file:/workspace/figure.pdf",
            "subscription_id": "subscription-b",
            "revision": 3,
            "renderer_lease_id": "renderer-lease-b"
        }))
        .expect("ticket request");
        assert_eq!(issue.renderer_lease_id, "renderer-lease-b");

        let close_lease: CloseFileRendererLeaseRequestV1 = serde_json::from_value(json!({
            "resource_id": "file:/workspace/figure.pdf",
            "subscription_id": "subscription-b",
            "renderer_lease_id": "renderer-lease-b"
        }))
        .expect("close renderer lease request");
        assert_eq!(close_lease.subscription_id, "subscription-b");
    }

    #[tokio::test]
    async fn trusted_workbench_open_resolves_primary_and_additional_but_not_system_roots() {
        let temp = tempfile::tempdir().expect("temp root");
        let primary = temp.path().join("primary");
        let additional = temp.path().join("additional");
        let system = temp.path().join("system");
        fs::create_dir_all(&primary).expect("primary");
        fs::create_dir_all(&additional).expect("additional");
        fs::create_dir_all(&system).expect("system");
        let primary_file = primary.join("primary.txt");
        let additional_file = additional.join("additional.txt");
        let system_file = system.join("secret.txt");
        fs::write(&primary_file, "primary\n").expect("primary fixture");
        fs::write(&additional_file, "additional\n").expect("additional fixture");
        fs::write(&system_file, "system\n").expect("system fixture");
        let state = AppState::new();
        install_test_agent(
            &state,
            AgentConfig {
                session_id: "agent-a".to_string(),
                folder: primary.to_string_lossy().into_owned(),
                include_directories: Some(vec![additional.to_string_lossy().into_owned()]),
                system_include_directories: Some(vec![system.to_string_lossy().into_owned()]),
                ..AgentConfig::default()
            },
        )
        .await;

        let primary_open = open_trusted(&state, &primary_file)
            .await
            .expect("primary open");
        let additional_open = open_trusted(&state, &additional_file)
            .await
            .expect("additional open");
        assert_eq!(
            open_trusted(&state, &system_file)
                .await
                .expect_err("system include must not authorize publication")
                .code(),
            "unauthorized_path"
        );

        state
            .file_resources
            .close(&primary_open.subscription_id)
            .await
            .expect("close primary");
        state
            .file_resources
            .close(&additional_open.subscription_id)
            .await
            .expect("close additional");
    }

    #[tokio::test]
    async fn trusted_workbench_open_chooses_matching_agents_deterministically() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("shared.txt");
        fs::write(&path, "shared\n").expect("fixture");
        let state = AppState::new();
        for agent_id in ["agent-z", "agent-a"] {
            install_test_agent(
                &state,
                AgentConfig {
                    session_id: agent_id.to_string(),
                    folder: temp.path().to_string_lossy().into_owned(),
                    ..AgentConfig::default()
                },
            )
            .await;
        }

        let opened = open_trusted(&state, &path).await.expect("trusted open");
        assert_eq!(
            state
                .file_resources
                .authorization_agent_id(&opened.resource_id, &opened.subscription_id,)
                .await
                .expect("claim")
                .as_deref(),
            Some("agent-a")
        );
        state
            .file_resources
            .close(&opened.subscription_id)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn trusted_workbench_open_uses_only_an_exact_live_picker_grant() {
        let temp = tempfile::tempdir().expect("temp root");
        let selected = temp.path().join("selected.txt");
        let sibling = temp.path().join("sibling.txt");
        fs::write(&selected, "selected\n").expect("selected fixture");
        fs::write(&sibling, "sibling\n").expect("sibling fixture");
        let state = AppState::new();
        state
            .file_resources
            .record_user_file(&selected)
            .await
            .expect("picker grant");

        let opened = open_trusted(&state, &selected)
            .await
            .expect("exact picker restore");
        assert_eq!(
            open_trusted(&state, &sibling)
                .await
                .expect_err("picker sibling must stay unauthorized")
                .code(),
            "unauthorized_path"
        );
        state
            .file_resources
            .close(&opened.subscription_id)
            .await
            .expect("close");

        state.file_resources.close_all().await;
        assert_eq!(
            open_trusted(&state, &selected)
                .await
                .expect_err("cleared picker grant must be revoked")
                .code(),
            "unauthorized_path"
        );
    }

    #[tokio::test]
    async fn trusted_workbench_open_rejects_missing_or_ambiguous_explicit_authorization() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("unclaimed.txt");
        fs::write(&path, "unclaimed\n").expect("fixture");
        let state = AppState::new();

        assert_eq!(
            open_trusted(&state, &path)
                .await
                .expect_err("missing auth")
                .code(),
            "unauthorized_path"
        );
        assert_eq!(
            open_file_resource_for_app(
                OpenFileResourceRequestV1 {
                    path: path.to_string_lossy().into_owned(),
                    agent_id: Some("agent-a".to_string()),
                    user_file_capability_id: Some("capability-a".to_string()),
                },
                &state,
                None,
            )
            .await
            .expect_err("two explicit auth fields are invalid")
            .code(),
            "invalid_request"
        );
    }

    #[tokio::test]
    async fn file_resources_native_picker_grant_authorizes_only_the_selected_file() {
        let temp = tempfile::tempdir().expect("temp root");
        let selected = temp.path().join("selected.txt");
        let sibling = temp.path().join("sibling.txt");
        fs::write(&selected, "selected\n").expect("selected fixture");
        fs::write(&sibling, "sibling\n").expect("sibling fixture");
        let runtime = crate::state::FileResourceRuntime::default();

        let grant = record_picked_file(&runtime, &selected)
            .await
            .expect("grant selected file");
        runtime
            .open_user_file(&grant.capability_id, &selected, None)
            .await
            .expect("selected file open");

        let error = runtime
            .open_user_file(&grant.capability_id, &sibling, None)
            .await
            .expect_err("sibling must not inherit picker grant");
        assert_eq!(error.code(), "unauthorized_path");
    }

    #[tokio::test]
    async fn file_resources_command_lookup_rejects_removed_agent_authorization() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("revoked.txt");
        fs::write(&path, "authorized\n").expect("fixture");
        let config = AgentConfig {
            session_id: "agent-a".to_string(),
            folder: temp.path().to_string_lossy().into_owned(),
            ..AgentConfig::default()
        };
        let state = AppState::new();
        state.agents.lock().await.insert(
            "agent-a".to_string(),
            crate::restored_agent_without_process(
                config.clone(),
                "idle",
                String::new(),
                None,
                None,
            ),
        );
        let subscription = state
            .file_resources
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");

        assert!(current_subscription_agent_config(
            &state,
            &subscription.resource_id,
            &subscription.subscription_id,
        )
        .await
        .expect("active authorization")
        .is_some());
        state.agents.lock().await.remove("agent-a");
        assert_eq!(
            current_subscription_agent_config(
                &state,
                &subscription.resource_id,
                &subscription.subscription_id,
            )
            .await
            .expect_err("removed agent must revoke command authorization")
            .code(),
            "unauthorized_path"
        );
    }

    #[test]
    fn file_resources_protocol_ranges_are_bounded_and_reject_multiple_ranges() {
        assert_eq!(
            parse_byte_range(Some("bytes=2-5"), 10).expect("range"),
            (2, 5)
        );
        assert_eq!(
            parse_byte_range(Some("bytes=7-"), 10).expect("open range"),
            (7, 9)
        );
        assert_eq!(
            parse_byte_range(Some("bytes=-3"), 10).expect("suffix"),
            (7, 9)
        );
        assert_eq!(
            parse_byte_range(Some("bytes=0-1,4-5"), 10)
                .expect_err("multiple ranges unsupported")
                .code(),
            "invalid_range"
        );
        assert_eq!(
            parse_byte_range(Some("bytes=10-12"), 10)
                .expect_err("out of bounds")
                .code(),
            "range_not_satisfiable"
        );
    }

    #[tokio::test]
    async fn file_resources_protocol_enforces_head_range_and_method_semantics() {
        let temp = tempfile::tempdir().expect("temp root");
        let path = temp.path().join("protocol.pdf");
        fs::write(&path, b"%PDF-1.7 protocol payload").expect("fixture");
        let config = AgentConfig {
            session_id: "agent-a".to_string(),
            folder: temp.path().to_string_lossy().into_owned(),
            ..AgentConfig::default()
        };
        let runtime = crate::state::FileResourceRuntime::default();
        let subscription = runtime
            .open_agent_file("agent-a", &config, &path, None)
            .await
            .expect("open");
        let ticket = runtime
            .issue_ticket_for_webview(
                &subscription.resource_id,
                &subscription.subscription_id,
                subscription.revision,
                Some(&config),
                "lease-a",
                Some("main"),
            )
            .await
            .expect("ticket");
        let uri = format!("wardian-resource://localhost/{}", ticket.ticket_id);

        let get = Request::builder()
            .method(tauri::http::Method::GET)
            .uri(&uri)
            .header(header::RANGE, "bytes=0-3")
            .body(Vec::new())
            .expect("GET request");
        let get = file_resource_protocol_response(&runtime, "main", get).await;
        assert_eq!(get.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(get.body(), b"%PDF");
        assert_eq!(get.headers()[header::CONTENT_LENGTH], "4");
        assert_eq!(get.headers()[header::CONTENT_RANGE], "bytes 0-3/25");
        assert_eq!(get.headers()[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(get.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(get.headers()["X-Content-Type-Options"], "nosniff");

        let head = Request::builder()
            .method(tauri::http::Method::HEAD)
            .uri(&uri)
            .header(header::RANGE, "bytes=-4")
            .body(Vec::new())
            .expect("HEAD request");
        let head = file_resource_protocol_response(&runtime, "main", head).await;
        assert_eq!(head.status(), StatusCode::PARTIAL_CONTENT);
        assert!(head.body().is_empty());
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "4");
        assert_eq!(head.headers()[header::CONTENT_RANGE], "bytes 21-24/25");

        let unsatisfiable = Request::builder()
            .method(tauri::http::Method::GET)
            .uri(&uri)
            .header(header::RANGE, "bytes=25-")
            .body(Vec::new())
            .expect("unsatisfiable request");
        let unsatisfiable = file_resource_protocol_response(&runtime, "main", unsatisfiable).await;
        assert_eq!(unsatisfiable.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(unsatisfiable.headers()[header::CONTENT_RANGE], "bytes */25");

        let method = Request::builder()
            .method(tauri::http::Method::POST)
            .uri(&uri)
            .body(Vec::new())
            .expect("method request");
        let method = file_resource_protocol_response(&runtime, "main", method).await;
        assert_eq!(method.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(method.headers()[header::ALLOW], "GET, HEAD");

        let mut malformed = Request::builder()
            .method(tauri::http::Method::HEAD)
            .uri(&uri)
            .body(Vec::new())
            .expect("malformed request");
        malformed.headers_mut().insert(
            header::RANGE,
            tauri::http::HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
        );
        let malformed = file_resource_protocol_response(&runtime, "main", malformed).await;
        assert_eq!(malformed.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(malformed.body().is_empty());

        let wrong_webview = Request::builder()
            .method(tauri::http::Method::GET)
            .uri(&uri)
            .body(Vec::new())
            .expect("wrong-webview request");
        let wrong_webview =
            file_resource_protocol_response(&runtime, "secondary", wrong_webview).await;
        assert_eq!(wrong_webview.status(), StatusCode::FORBIDDEN);

        fs::write(&path, b"%PDF-1.7 modified payload").expect("mutate revision");
        let issued_revision = Request::builder()
            .method(tauri::http::Method::GET)
            .uri(&uri)
            .body(Vec::new())
            .expect("issued revision request");
        let issued_revision =
            file_resource_protocol_response(&runtime, "main", issued_revision).await;
        assert_eq!(issued_revision.status(), StatusCode::OK);
        assert_eq!(issued_revision.body(), b"%PDF-1.7 protocol payload");
    }
}
