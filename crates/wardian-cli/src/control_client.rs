//! Shared authenticated client for Wardian's local control endpoint.

pub mod messaging;
#[cfg(windows)]
mod windows_pipe;

use std::{fmt, io};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{ControlRequest, MessageOrigin};

/// A typed error returned by Wardian's local control endpoint.
#[derive(Debug)]
pub struct ControlEndpointError {
    code: String,
    message: String,
    details: Option<serde_json::Value>,
}

impl ControlEndpointError {
    /// Construct a control error without a structured details payload.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    /// Construct a control error while preserving the backend details payload.
    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Some(details),
        }
    }

    /// Return the stable endpoint error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Return structured backend details when the endpoint supplied them.
    pub fn details(&self) -> Option<&serde_json::Value> {
        self.details.as_ref()
    }
}

impl fmt::Display for ControlEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ControlEndpointError {}

/// Build the current-thread runtime used for local control IPC.
pub fn build_runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| io::Error::other(error.to_string()))
}

/// Require the authenticated sender identity from the managed session environment.
pub fn require_current_message_origin() -> io::Result<MessageOrigin> {
    current_message_origin().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "WARDIAN_SESSION_ID environment variable is not set",
        )
    })
}

fn current_message_origin() -> Option<MessageOrigin> {
    std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|session_id| session_id.trim().to_string())
        .filter(|session_id| !session_id.is_empty())
        .map(|session_id| MessageOrigin::WardianAgent { session_id })
}

/// Send one control request over the platform endpoint.
///
/// Windows retries only a busy pipe open. A connected request is never replayed.
pub async fn send_request(req: ControlRequest) -> io::Result<serde_json::Value> {
    #[cfg(windows)]
    {
        send_request_windows(req).await
    }
    #[cfg(unix)]
    {
        send_request_unix(req).await
    }
}

#[cfg(windows)]
async fn send_request_windows(req: ControlRequest) -> io::Result<serde_json::Value> {
    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| io::Error::other("could not resolve Wardian control pipe"))?;
    windows_pipe::send_request(&pipe_name, req).await
}

#[cfg(unix)]
async fn send_request_unix(req: ControlRequest) -> io::Result<serde_json::Value> {
    use tokio::net::UnixStream;

    let socket_path = wardian_core::control::socket_path()
        .ok_or_else(|| io::Error::other("could not resolve Wardian control socket"))?;
    let mut stream = UnixStream::connect(socket_path).await?;
    exchange_json(&mut stream, req).await
}

async fn exchange_json<T>(stream: &mut T, req: ControlRequest) -> io::Result<serde_json::Value>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let request =
        serde_json::to_string(&req).map_err(|error| io::Error::other(error.to_string()))?;
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut line).await?;

    // Detect backend error envelope {"error": {...}}.
    let value: serde_json::Value =
        serde_json::from_str(&line).map_err(|error| io::Error::other(error.to_string()))?;
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(|code| code.as_str())
            .unwrap_or("request_failed");
        let message = error
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        let endpoint_error = error.get("details").cloned().map_or_else(
            || ControlEndpointError::new(code, message),
            |details| ControlEndpointError::with_details(code, message, details),
        );
        return Err(io::Error::other(endpoint_error));
    }

    Ok(value)
}

#[cfg(test)]
mod tests;
