//! Loopback OTLP/HTTP JSON log receiver for one Claude launch.
//!
//! The receiver accepts only `POST /claude-telemetry/<token>/v1/logs` on
//! 127.0.0.1, where the token is generated per launch and passed to Claude
//! through that launch's `--settings`. It keeps only the event metadata the
//! verifier needs and appends it to the launch's ledger, synced to disk,
//! before acknowledging, so an acknowledged batch survives a crash. A failed
//! write answers 503, which Claude's exporter retries; the ledger treats the
//! retried batch as duplicates.

use super::{parse_otlp_logs, TelemetryEvent};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as RoutePath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Largest OTLP request accepted. Claude batches at most a few hundred small
/// metadata records per export interval.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// Largest ledger kept for one launch. Past it, events are refused; the
/// resulting gap leaves later prompts unverified rather than unbounded disk use.
const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;

struct ReceiverState {
    token: String,
    ledger_path: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
}

/// A running receiver. Dropping it stops the server.
pub(crate) struct TelemetryReceiver {
    endpoint: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TelemetryReceiver {
    /// Binds an ephemeral loopback port and starts serving on the app runtime.
    pub(crate) fn start(ledger_path: PathBuf) -> Result<Self, String> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("could not bind Claude telemetry receiver: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("could not configure Claude telemetry receiver: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("could not read Claude telemetry receiver address: {error}"))?
            .port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let endpoint = format!("http://127.0.0.1:{port}/claude-telemetry/{token}/v1/logs");
        let state = Arc::new(ReceiverState {
            token,
            ledger_path,
            write_lock: tokio::sync::Mutex::new(()),
        });
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(error) => {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Claude telemetry receiver could not start: {error}"
                    ));
                    return;
                }
            };
            let served = axum::serve(listener, router(state))
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
            if let Err(error) = served {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] Claude telemetry receiver stopped: {error}"
                ));
            }
        });
        Ok(Self {
            endpoint,
            shutdown: Some(shutdown),
        })
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for TelemetryReceiver {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn router(state: Arc<ReceiverState>) -> Router {
    Router::new()
        .route("/claude-telemetry/{token}/v1/logs", post(receive))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state)
}

async fn receive(
    State(state): State<Arc<ReceiverState>>,
    RoutePath(token): RoutePath<String>,
    body: Bytes,
) -> Response {
    if token != state.token {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(request) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(events) = parse_otlp_logs(&request) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let _write = state.write_lock.lock().await;
    let ledger_path = state.ledger_path.clone();
    match tokio::task::spawn_blocking(move || append_events(&ledger_path, &events)).await {
        Ok(Ok(())) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            "{}",
        )
            .into_response(),
        Ok(Err(AppendError::LedgerFull)) => StatusCode::INSUFFICIENT_STORAGE.into_response(),
        Ok(Err(AppendError::Io(error))) => {
            crate::utils::logging::log_debug(&format!(
                "[Wardian] Claude telemetry ledger write failed; the exporter will retry: {error}"
            ));
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Debug)]
enum AppendError {
    LedgerFull,
    Io(std::io::Error),
}

/// Appends `events` as JSONL and syncs them before returning.
fn append_events(ledger_path: &Path, events: &[TelemetryEvent]) -> Result<(), AppendError> {
    append_events_within(ledger_path, events, MAX_LEDGER_BYTES)
}

fn append_events_within(
    ledger_path: &Path,
    events: &[TelemetryEvent],
    max_ledger_bytes: u64,
) -> Result<(), AppendError> {
    if events.is_empty() {
        return Ok(());
    }
    let mut lines = String::new();
    for event in events {
        let line = serde_json::to_string(event)
            .map_err(|error| AppendError::Io(std::io::Error::other(error)))?;
        lines.push_str(&line);
        lines.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path)
        .map_err(AppendError::Io)?;
    let current = file.metadata().map_err(AppendError::Io)?.len();
    if current.saturating_add(lines.len() as u64) > max_ledger_bytes {
        return Err(AppendError::LedgerFull);
    }
    file.write_all(lines.as_bytes()).map_err(AppendError::Io)?;
    file.sync_data().map_err(AppendError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn post_batch(state: Arc<ReceiverState>, token: &str, body: &str) -> StatusCode {
        receive(
            State(state),
            RoutePath(token.to_string()),
            Bytes::from(body.to_string()),
        )
        .await
        .status()
    }

    fn state(dir: &Path) -> Arc<ReceiverState> {
        Arc::new(ReceiverState {
            token: "launch-token".to_string(),
            ledger_path: dir.join(super::super::TELEMETRY_FILE),
            write_lock: tokio::sync::Mutex::new(()),
        })
    }

    const BATCH: &str = r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[{
        "body":{"stringValue":"claude_code.user_prompt"},
        "attributes":[
            {"key":"event.name","value":{"stringValue":"user_prompt"}},
            {"key":"event.sequence","value":{"intValue":3}},
            {"key":"prompt","value":{"stringValue":"secret prompt"}}
        ]}]}]}]}"#;

    #[tokio::test]
    async fn accepted_batches_are_durable_before_acknowledgement() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            post_batch(state(dir.path()), "launch-token", BATCH).await,
            StatusCode::OK
        );

        let ledger = std::fs::read_to_string(dir.path().join(super::super::TELEMETRY_FILE))
            .expect("ledger written");
        let lines = ledger.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        let event: TelemetryEvent = serde_json::from_str(lines[0]).expect("ledger line");
        assert_eq!(event.sequence, 3);
        assert!(!ledger.contains("secret prompt"));
    }

    #[tokio::test]
    async fn a_foreign_token_or_invalid_body_is_refused_without_writing() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            post_batch(state(dir.path()), "other-token", BATCH).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            post_batch(state(dir.path()), "launch-token", "{not json").await,
            StatusCode::BAD_REQUEST
        );
        assert!(!dir.path().join(super::super::TELEMETRY_FILE).exists());
    }

    #[test]
    fn a_full_ledger_refuses_further_events() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(super::super::TELEMETRY_FILE);
        let events = parse_otlp_logs(&serde_json::from_str(BATCH).expect("batch")).expect("events");
        append_events_within(&path, &events, 4096).expect("first batch fits");
        let used = std::fs::metadata(&path).expect("ledger").len();
        assert!(matches!(
            append_events_within(&path, &events, used + 1),
            Err(AppendError::LedgerFull)
        ));
        assert_eq!(std::fs::metadata(&path).expect("ledger").len(), used);
    }
}
