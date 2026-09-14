use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde_json::Value;
use tokio::sync::Mutex;

use super::protocol::{OpenCodeHttpBinding, OpenCodePrompt, OpenCodeStoredMessage};
use super::sse::{OpenCodeEvent, OpenCodeEventStream};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_BODY_LIMIT: usize = 1024 * 1024;

/// Errors from the OpenCode HTTP owner.
///
/// Errors after `prompt_async` has been invoked never authorize a retry. The
/// integration layer maps `provider_boundary_crossed` to its durable
/// `SubmittedUnconfirmed` or terminal failed state and keeps the composer
/// closed for that attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeHttpErrorCode {
    BindingInvalid,
    Closed,
    StaleGeneration,
    ReplayRefused,
    TransportUnavailable,
    RedirectRejected,
    AuthenticationFailed,
    SessionMismatch,
    HealthCheckFailed,
    Busy,
    MalformedResponse,
    MalformedEvent,
    ProviderRejectedAfterSubmit,
    SubmittedUnconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeHttpError {
    pub code: OpenCodeHttpErrorCode,
    pub provider_boundary_crossed: bool,
    pub status: Option<u16>,
    detail: &'static str,
}

impl OpenCodeHttpError {
    pub(crate) const fn new(
        code: OpenCodeHttpErrorCode,
        provider_boundary_crossed: bool,
        status: Option<u16>,
        detail: &'static str,
    ) -> Self {
        Self {
            code,
            provider_boundary_crossed,
            status,
            detail,
        }
    }
}

impl fmt::Display for OpenCodeHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "OpenCode HTTP {:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for OpenCodeHttpError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeHttpVerification {
    pub server_version: String,
    pub provider_session_id: String,
    pub directory: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeSessionActivity {
    Idle,
    Busy,
    Retry,
    Unknown,
}

impl OpenCodeSessionActivity {
    fn from_value(value: &Value) -> Self {
        let value = value
            .as_str()
            .or_else(|| value.get("type").and_then(Value::as_str))
            .unwrap_or_default();
        match value {
            "idle" => Self::Idle,
            "busy" => Self::Busy,
            "retry" => Self::Retry,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeHttpReceipt {
    pub provider_session_id: String,
    pub provider_message_id: String,
    pub body_sha256: String,
    pub accepted_at: String,
    pub response_status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeUserMessageProof {
    pub provider_session_id: String,
    pub provider_message_id: String,
    pub body_sha256: String,
    pub observed_at: String,
}

/// One authenticated HTTP owner for the already-running interactive TUI.
///
/// `OpenCodeHttpOwner` does not spawn, attach, resume, or kill OpenCode. The
/// interactive lifecycle owner supplies a launch-verified binding, and this
/// type serializes machine prompts into that exact session. A submission ID is
/// recorded before the network call and cannot be submitted twice by this
/// owner, including after a timeout or connection loss.
pub struct OpenCodeHttpOwner {
    binding: Arc<OpenCodeHttpBinding>,
    client: reqwest::Client,
    submit_lock: Mutex<()>,
    attempted_message_ids: Mutex<HashSet<String>>,
    closed: AtomicBool,
}

impl fmt::Debug for OpenCodeHttpOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeHttpOwner")
            .field("binding", &self.binding)
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl OpenCodeHttpOwner {
    /// Bind to a verified launch proof without contacting the server.
    pub fn bind(binding: OpenCodeHttpBinding) -> Result<Self, OpenCodeHttpError> {
        if binding.generation == 0
            || binding.runtime_generation == 0
            || super::protocol::validate_session_id(&binding.provider_session_id).is_err()
            || binding.username.trim().is_empty()
            || binding.password.is_empty()
            || binding.agent_id.trim().is_empty()
            || binding.process_identity.trim().is_empty()
            || binding.listener_identity.trim().is_empty()
            || binding.config_fingerprint.trim().is_empty()
            || !binding.workspace.is_absolute()
        {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::BindingInvalid,
                false,
                None,
                "OpenCode HTTP binding is incomplete",
            ));
        }
        if binding.endpoint.scheme() != "http"
            || binding.endpoint.host_str() != Some("127.0.0.1")
            || binding.endpoint.port().is_none()
        {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::BindingInvalid,
                false,
                None,
                "OpenCode HTTP binding must use a loopback HTTP endpoint",
            ));
        }

        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| {
                OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::TransportUnavailable,
                    false,
                    None,
                    "OpenCode HTTP client could not be built",
                )
            })?;
        Ok(Self {
            binding: Arc::new(binding),
            client,
            submit_lock: Mutex::new(()),
            attempted_message_ids: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
        })
    }

    pub fn binding(&self) -> &OpenCodeHttpBinding {
        &self.binding
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Revoke this generation's machine writer. This does not affect the TUI
    /// process or an external server.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub fn event_belongs_to_binding(&self, event: &OpenCodeEvent) -> bool {
        !self.is_closed() && event.session_id() == Some(self.binding.provider_session_id())
    }

    /// Prove authentication, server health/version, exact session identity,
    /// and directory binding before this owner is registered as native.
    pub async fn verify_binding(&self) -> Result<OpenCodeHttpVerification, OpenCodeHttpError> {
        self.ensure_open()?;
        let health = self.get_json(self.root_url("global/health")?).await?;
        if health.get("healthy").and_then(Value::as_bool) != Some(true) {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::HealthCheckFailed,
                false,
                None,
                "OpenCode server did not report healthy",
            ));
        }
        let server_version = health
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::MalformedResponse,
                    false,
                    None,
                    "OpenCode health response omitted its version",
                )
            })?
            .to_string();
        if server_version.trim().is_empty() {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode health response had an empty version",
            ));
        }
        if let Some(expected) = self.binding.expected_server_version.as_deref() {
            if expected != server_version {
                return Err(OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::SessionMismatch,
                    false,
                    None,
                    "OpenCode server version did not match the launch proof",
                ));
            }
        }

        let mut url = self.session_url("")?;
        url.query_pairs_mut()
            .append_pair("directory", &self.binding.workspace.to_string_lossy());
        let session = self.get_json(url).await?;
        let session_id = session
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| session.get("sessionID").and_then(Value::as_str));
        let directory = session.get("directory").and_then(Value::as_str);
        if session_id != Some(self.binding.provider_session_id())
            || directory.is_none()
            || directory.is_some_and(|value| !same_directory(value, &self.binding.workspace))
        {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::SessionMismatch,
                false,
                None,
                "OpenCode session or directory did not match the launch proof",
            ));
        }
        Ok(OpenCodeHttpVerification {
            server_version,
            provider_session_id: self.binding.provider_session_id().to_string(),
            directory: directory.unwrap_or_default().to_string(),
        })
    }

    /// Prove that the launch listener does not accept an ambient unauthenticated
    /// request. A healthy response here would mean that the child-only writer
    /// credential is not an effective boundary, so registration fails closed.
    pub async fn verify_unauthenticated_probe(&self) -> Result<(), OpenCodeHttpError> {
        self.ensure_open()?;
        let response = self
            .client
            .get(self.root_url("global/health")?)
            .send()
            .await
            .map_err(|_| {
                OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::TransportUnavailable,
                    false,
                    None,
                    "OpenCode unauthenticated probe could not reach the listener",
                )
            })?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Ok(());
        }
        Err(OpenCodeHttpError::new(
            OpenCodeHttpErrorCode::AuthenticationFailed,
            false,
            Some(status.as_u16()),
            "OpenCode listener accepted an unauthenticated request",
        ))
    }

    /// Read the authenticated OpenCode status map for this exact bound session.
    /// OpenCode removes idle entries, so an absent key is idle only after the
    /// response has been validated as a JSON object. Unknown explicit values
    /// remain fail-closed for the native admission caller.
    pub async fn session_activity(&self) -> Result<OpenCodeSessionActivity, OpenCodeHttpError> {
        self.ensure_open()?;
        let value = self.get_json(self.root_url("session/status")?).await?;
        let statuses = value.as_object().ok_or_else(|| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode session status response was not a JSON object",
            )
        })?;
        Ok(statuses
            .get(self.binding.provider_session_id())
            .map(OpenCodeSessionActivity::from_value)
            .unwrap_or(OpenCodeSessionActivity::Idle))
    }

    pub async fn open_events(&self) -> Result<OpenCodeEventStream, OpenCodeHttpError> {
        self.ensure_open()?;
        let response = self
            .request(Method::GET, self.root_url("global/event")?)
            .send()
            .await
            .map_err(|_| {
                OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::TransportUnavailable,
                    false,
                    None,
                    "OpenCode event stream could not be opened",
                )
            })?;
        self.require_status(response.status(), false)?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type.starts_with("text/event-stream") {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                Some(StatusCode::OK.as_u16()),
                "OpenCode event endpoint did not return SSE",
            ));
        }
        Ok(OpenCodeEventStream::new(response))
    }

    pub async fn list_messages(
        &self,
        limit: u32,
    ) -> Result<Vec<OpenCodeStoredMessage>, OpenCodeHttpError> {
        self.ensure_open()?;
        let mut url = self.session_url("message")?;
        url.query_pairs_mut()
            .append_pair("limit", &limit.clamp(1, 200).to_string());
        let value = self.get_json(url).await?;
        serde_json::from_value(value).map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode message list had an unexpected shape",
            )
        })
    }

    /// Reconcile a previously accepted or uncertain submission without
    /// submitting anything. `None` is absence of proof, not permission to
    /// replay the prompt.
    pub async fn reconcile_user_message(
        &self,
        prompt: &OpenCodePrompt,
    ) -> Result<Option<OpenCodeUserMessageProof>, OpenCodeHttpError> {
        if prompt.message_id.trim().is_empty() {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::BindingInvalid,
                false,
                None,
                "OpenCode reconciliation requires a message id",
            ));
        }
        let messages = self.list_messages(200).await?;
        let Some(_) = messages.iter().find(|message| {
            message.is_exact_user_prompt(self.binding.provider_session_id(), prompt)
        }) else {
            return Ok(None);
        };
        let body_sha256 = prompt.body_sha256().map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode prompt body could not be encoded",
            )
        })?;
        Ok(Some(OpenCodeUserMessageProof {
            provider_session_id: self.binding.provider_session_id().to_string(),
            provider_message_id: prompt.message_id.clone(),
            body_sha256,
            observed_at: chrono::Utc::now().to_rfc3339(),
        }))
    }

    /// Submit exactly one HTTP request for this message ID. A 204 is only an
    /// API acknowledgement; provider turn start and completion require later
    /// SSE/GET reconciliation by the integration layer.
    pub async fn submit_once(
        &self,
        expected_generation: u64,
        expected_runtime_generation: u64,
        prompt: &OpenCodePrompt,
    ) -> Result<OpenCodeHttpReceipt, OpenCodeHttpError> {
        self.ensure_open()?;
        self.ensure_generation(expected_generation, expected_runtime_generation)?;
        let body = prompt.request_body().map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode prompt body could not be encoded",
            )
        })?;
        let body_sha256 = prompt.body_sha256().map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode prompt body could not be encoded",
            )
        })?;
        let _submit_guard = self.submit_lock.lock().await;
        {
            let mut attempted = self.attempted_message_ids.lock().await;
            if !attempted.insert(prompt.message_id.clone()) {
                return Err(OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::ReplayRefused,
                    false,
                    None,
                    "OpenCode message id was already attempted; replay is forbidden",
                ));
            }
        }
        self.ensure_open()?;
        self.ensure_generation(expected_generation, expected_runtime_generation)?;

        let response = self
            .request(Method::POST, self.session_url("prompt_async")?)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| {
                OpenCodeHttpError::new(
                    OpenCodeHttpErrorCode::SubmittedUnconfirmed,
                    true,
                    None,
                    "OpenCode prompt submission became uncertain after request start",
                )
            })?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(OpenCodeHttpReceipt {
                provider_session_id: self.binding.provider_session_id().to_string(),
                provider_message_id: prompt.message_id.clone(),
                body_sha256,
                accepted_at: chrono::Utc::now().to_rfc3339(),
                response_status: response.status().as_u16(),
            });
        }
        let status = response.status();
        let code = if status.is_redirection() {
            OpenCodeHttpErrorCode::RedirectRejected
        } else if status.is_client_error() {
            OpenCodeHttpErrorCode::ProviderRejectedAfterSubmit
        } else {
            OpenCodeHttpErrorCode::SubmittedUnconfirmed
        };
        Err(OpenCodeHttpError::new(
            code,
            true,
            Some(status.as_u16()),
            "OpenCode prompt request was not accepted; replay is forbidden",
        ))
    }

    fn ensure_open(&self) -> Result<(), OpenCodeHttpError> {
        if self.is_closed() {
            Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::Closed,
                false,
                None,
                "OpenCode HTTP owner is closed",
            ))
        } else {
            Ok(())
        }
    }

    fn ensure_generation(
        &self,
        generation: u64,
        runtime_generation: u64,
    ) -> Result<(), OpenCodeHttpError> {
        if self.binding.generation != generation
            || self.binding.runtime_generation != runtime_generation
        {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::StaleGeneration,
                false,
                None,
                "OpenCode HTTP owner belongs to another runtime generation",
            ));
        }
        Ok(())
    }

    fn root_url(&self, path: &str) -> Result<Url, OpenCodeHttpError> {
        let mut url = self.binding.endpoint.clone();
        url.set_path(&format!("/{path}"));
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }

    fn session_url(&self, suffix: &str) -> Result<Url, OpenCodeHttpError> {
        let session_id = self.binding.provider_session_id();
        let path = if suffix.is_empty() {
            format!("/session/{session_id}")
        } else {
            format!("/session/{session_id}/{suffix}")
        };
        let mut url = self.binding.endpoint.clone();
        url.set_path(&path);
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .basic_auth(&self.binding.username, Some(&self.binding.password))
    }

    async fn get_json(&self, url: Url) -> Result<Value, OpenCodeHttpError> {
        let response = self.request(Method::GET, url).send().await.map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::TransportUnavailable,
                false,
                None,
                "OpenCode HTTP request failed before a response",
            )
        })?;
        self.require_status(response.status(), false)?;
        let body = read_response_body(response).await?;
        serde_json::from_slice(&body).map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode HTTP response was not valid JSON",
            )
        })
    }

    fn require_status(&self, status: StatusCode, crossed: bool) -> Result<(), OpenCodeHttpError> {
        if status.is_success() {
            return Ok(());
        }
        let code = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                OpenCodeHttpErrorCode::AuthenticationFailed
            }
            status if status.is_redirection() => OpenCodeHttpErrorCode::RedirectRejected,
            _ => OpenCodeHttpErrorCode::TransportUnavailable,
        };
        Err(OpenCodeHttpError::new(
            code,
            crossed,
            Some(status.as_u16()),
            "OpenCode HTTP endpoint returned an unexpected status",
        ))
    }
}

async fn read_response_body(response: reqwest::Response) -> Result<Vec<u8>, OpenCodeHttpError> {
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        OpenCodeHttpError::new(
            OpenCodeHttpErrorCode::TransportUnavailable,
            false,
            None,
            "OpenCode HTTP response body could not be read",
        )
    })? {
        if body.len().saturating_add(chunk.len()) > RESPONSE_BODY_LIMIT {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedResponse,
                false,
                None,
                "OpenCode HTTP response exceeded the size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn same_directory(remote: &str, expected: &std::path::Path) -> bool {
    let normalize = |value: String| {
        value
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    normalize(remote.to_string()) == normalize(expected.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use super::*;
    use crate::delivery::opencode_http::{
        OpenCodeHttpLaunchPlan, OpenCodeHttpLaunchProof, OpenCodePromptOptions,
    };

    fn owner_for_test(port: u16, generation: u64, runtime_generation: u64) -> OpenCodeHttpOwner {
        let workspace = std::env::current_dir().expect("test workspace");
        let plan =
            OpenCodeHttpLaunchPlan::new(generation, runtime_generation, port).expect("launch plan");
        let binding = plan
            .bind(OpenCodeHttpLaunchProof {
                agent_id: "agent-1".into(),
                provider_session_id: "ses_test".into(),
                process_identity: "pid:1".into(),
                listener_identity: "listener:1".into(),
                workspace,
                config_fingerprint: "config-fingerprint".into(),
                server_version: None,
            })
            .expect("launch proof");
        OpenCodeHttpOwner::bind(binding).expect("owner binding")
    }

    fn prompt_for_test() -> OpenCodePrompt {
        OpenCodePrompt::new("msg_test", "native task", Default::default()).expect("prompt")
    }

    fn status_owner_for_test(body: impl Into<String>) -> (OpenCodeHttpOwner, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("status listener");
        let port = listener
            .local_addr()
            .expect("status listener address")
            .port();
        let body = body.into();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("status request accepted");
            let mut request = [0_u8; 8192];
            let size = stream.read(&mut request).expect("status request read");
            let request = String::from_utf8_lossy(&request[..size]);
            assert!(request.starts_with("GET /session/status HTTP/1.1"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: basic "));

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            stream
                .write_all(response.as_bytes())
                .expect("status response write");
        });
        (owner_for_test(port, 7, 11), server)
    }

    #[test]
    fn launch_plan_only_adds_loopback_network_arguments() {
        let plan = OpenCodeHttpLaunchPlan::new(2, 3, 4097).expect("plan");
        assert_eq!(
            plan.network_args(),
            vec!["--hostname", "127.0.0.1", "--port", "4097"]
        );
        assert_eq!(plan.environment()[0].0, "OPENCODE_SERVER_USERNAME");
        assert_eq!(plan.environment()[1].0, "OPENCODE_SERVER_PASSWORD");
    }

    #[test]
    fn prompt_body_uses_documented_prompt_async_shape() {
        let prompt = OpenCodePrompt::new(
            "msg_1",
            "canonical task",
            OpenCodePromptOptions {
                model: Some(super::super::protocol::OpenCodeModel {
                    provider_id: "anthropic".into(),
                    model_id: "claude-sonnet".into(),
                }),
                agent: Some("build".into()),
                ..Default::default()
            },
        )
        .expect("prompt");
        let value: Value =
            serde_json::from_slice(&prompt.request_body().expect("body")).expect("json");
        assert_eq!(value["messageID"], "msg_1");
        assert_eq!(value["model"]["providerID"], "anthropic");
        assert_eq!(value["agent"], "build");
        assert_eq!(value["parts"][0]["type"], "text");
        assert_eq!(value["parts"][0]["text"], "canonical task");
        assert!(value.get("noReply").is_none());
        assert!(value.get("system").is_none());
    }

    #[test]
    fn exact_user_message_requires_session_id_and_text_part() {
        let prompt =
            OpenCodePrompt::new("msg_1", "canonical task", Default::default()).expect("prompt");
        let message = OpenCodeStoredMessage {
            info: serde_json::json!({"id":"msg_1","sessionID":"ses_1","role":"user"}),
            parts: vec![serde_json::json!({"type":"text","text":"canonical task"})],
        };
        assert!(message.is_exact_user_prompt("ses_1", &prompt));
        assert!(!message.is_exact_user_prompt("ses_2", &prompt));
    }

    #[tokio::test]
    async fn submit_rejects_stale_session_generation_before_network() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("listener address").port();
        let owner = owner_for_test(port, 7, 11);

        let error = owner
            .submit_once(8, 11, &prompt_for_test())
            .await
            .expect_err("stale generation");

        assert_eq!(error.code, OpenCodeHttpErrorCode::StaleGeneration);
        assert!(!error.provider_boundary_crossed);
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        assert!(
            listener.accept().is_err(),
            "stale request touched the network"
        );
    }

    #[tokio::test]
    async fn session_activity_treats_an_empty_valid_status_map_as_idle() {
        let (owner, server) = status_owner_for_test("{}");

        assert_eq!(
            owner.session_activity().await.expect("status activity"),
            OpenCodeSessionActivity::Idle
        );

        server.join().expect("status server");
    }

    #[tokio::test]
    async fn session_activity_ignores_foreign_session_entries() {
        let (owner, server) = status_owner_for_test(r#"{"ses_foreign":{"type":"busy"}}"#);

        assert_eq!(
            owner.session_activity().await.expect("status activity"),
            OpenCodeSessionActivity::Idle
        );

        server.join().expect("status server");
    }

    #[tokio::test]
    async fn session_activity_reads_only_the_exact_bound_session_entry() {
        let (owner, server) =
            status_owner_for_test(r#"{"ses_foreign":{"type":"busy"},"ses_test":{"type":"retry"}}"#);

        assert_eq!(
            owner.session_activity().await.expect("status activity"),
            OpenCodeSessionActivity::Retry
        );

        server.join().expect("status server");
    }

    #[tokio::test]
    async fn session_activity_preserves_explicit_busy_and_retry_states() {
        for (body, expected) in [
            (
                r#"{"ses_test":{"type":"busy"}}"#,
                OpenCodeSessionActivity::Busy,
            ),
            (
                r#"{"ses_test":{"type":"retry"}}"#,
                OpenCodeSessionActivity::Retry,
            ),
        ] {
            let (owner, server) = status_owner_for_test(body);

            assert_eq!(
                owner.session_activity().await.expect("status activity"),
                expected
            );

            server.join().expect("status server");
        }
    }

    #[tokio::test]
    async fn session_activity_rejects_malformed_status_roots() {
        for body in ["null", "[]", "\"idle\""] {
            let (owner, server) = status_owner_for_test(body);

            let error = owner
                .session_activity()
                .await
                .expect_err("malformed status root");
            assert_eq!(error.code, OpenCodeHttpErrorCode::MalformedResponse);
            assert!(!error.provider_boundary_crossed);

            server.join().expect("status server");
        }
    }

    #[tokio::test]
    async fn session_activity_keeps_unrecognized_bound_state_fail_closed() {
        let (owner, server) = status_owner_for_test(r#"{"ses_test":{"type":"stalled"}}"#);

        assert_eq!(
            owner.session_activity().await.expect("status activity"),
            OpenCodeSessionActivity::Unknown
        );

        server.join().expect("status server");
    }

    #[tokio::test]
    async fn uncertain_submission_marks_message_attempted_and_forbids_replay() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("listener address").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            drop(stream);
        });
        let owner = owner_for_test(port, 7, 11);
        let prompt = prompt_for_test();

        let first = owner
            .submit_once(7, 11, &prompt)
            .await
            .expect_err("closed owner response is uncertain");
        assert_eq!(first.code, OpenCodeHttpErrorCode::SubmittedUnconfirmed);
        assert!(first.provider_boundary_crossed);

        let second = owner
            .submit_once(7, 11, &prompt)
            .await
            .expect_err("uncertain message must not replay");
        assert_eq!(second.code, OpenCodeHttpErrorCode::ReplayRefused);
        assert!(!second.provider_boundary_crossed);

        server.join().expect("server thread");
    }
}
