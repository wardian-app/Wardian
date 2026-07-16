use crate::state::{
    AppState, FileResourceRuntime, FileResourceSnapshotV1, FileResourceTextV1,
    FileResourceTicketV1, UserFileGrantV1,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::http::{header, Request, Response, StatusCode};
use tauri_plugin_dialog::DialogExt as _;
use wardian_core::files::FileResourceErrorV1;
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
    match (
        request.agent_id.as_deref(),
        request.user_file_capability_id.as_deref(),
    ) {
        (Some(agent_id), None) => {
            let config = current_agent_config(&state, agent_id).await?;
            state
                .file_resources
                .open_agent_file(agent_id, &config, Path::new(&request.path), Some(app))
                .await
        }
        (None, Some(capability_id)) => {
            state
                .file_resources
                .open_user_file(capability_id, Path::new(&request.path), Some(app))
                .await
        }
        _ => Err(resource_error(
            "invalid_request",
            "exactly one file authorization capability is required",
        )),
    }
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
        let stale = Request::builder()
            .method(tauri::http::Method::GET)
            .uri(&uri)
            .body(Vec::new())
            .expect("stale request");
        let stale = file_resource_protocol_response(&runtime, "main", stale).await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
    }
}
