//! Owned local proxy transport. The child copies raw WebSocket bytes between
//! stdin/stdout and the caller's private socket; it is not a JSONL server.

use std::future::Future;
use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

use super::*;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

impl CodexSharedClient {
    /// Launch a direct Codex executable with `app-server proxy --sock <path>`.
    /// The caller prepares executable, prefix arguments, environment, cwd and
    /// security policy, and owns the absolute private socket and its permissions.
    /// This adds only proxy arguments and process/pipe lifecycle settings. It
    /// never starts an app-server, discovers a socket, or submits an RPC.
    ///
    /// Cancellation through `cancelled` joins cleanup before returning. Dropping
    /// this future also signals cleanup, but the caller must keep the Tokio
    /// runtime alive for the supervisor to reap the child. Hold the generation
    /// gate and await explicit cancellation to fence replacement on observed exit.
    pub async fn connect_proxy(
        agent_id: String,
        generation: u64,
        command: Command,
        private_socket: &Path,
        cancelled: impl Future<Output = ()>,
    ) -> Result<Arc<Self>, CodexSharedError> {
        if !private_socket.is_absolute() {
            return Err(CodexSharedError::unsupported(
                "Codex proxy requires an absolute private socket path",
            ));
        }
        // Check an already-requested cancellation before spawning anything.
        tokio::pin!(cancelled);
        tokio::select! {
            biased;
            _ = &mut cancelled => return Err(cancelled_error()),
            _ = std::future::ready(()) => {}
        }
        let (proxy, stream, setup_error) = OwnedProxy::spawn(command, private_socket)?;
        if let Some(error) = setup_error {
            proxy.close().await;
            return Err(error);
        }
        let connected = {
            // This URI supplies HTTP handshake headers only. client_async uses
            // the supplied duplex pipes and never performs a DNS or TCP connect.
            let handshake = tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                tokio_tungstenite::client_async("ws://localhost", stream),
            );
            tokio::pin!(handshake);
            tokio::select! {
                biased;
                _ = &mut cancelled => Err(cancelled_error()),
                result = &mut handshake => match result {
                    Ok(Ok((socket, _))) => Ok(socket),
                    Ok(Err(_)) => Err(CodexSharedError::unsupported(
                        "Codex proxy WebSocket handshake failed; not retried",
                    )),
                    Err(_) => Err(CodexSharedError::unsupported(
                        "Codex proxy WebSocket handshake timed out; not retried",
                    )),
                }
            }
        };
        match connected {
            Ok(socket) => Ok(Self::from_connected(
                agent_id,
                generation,
                socket,
                Some(proxy),
            )),
            Err(error) => {
                proxy.close().await;
                Err(error)
            }
        }
    }
}

fn cancelled_error() -> CodexSharedError {
    CodexSharedError::unsupported("Codex proxy connection cancelled; no RPC was submitted")
}

type ProxyStream = tokio::io::Join<tokio::process::ChildStdout, tokio::process::ChildStdin>;

/// A non-cancellable supervisor retains the Child until exit has been observed.
/// The handle stays in its slot across cancelled and concurrent close calls.
pub(super) struct OwnedProxy {
    stop: watch::Sender<bool>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl OwnedProxy {
    fn spawn(
        mut command: Command,
        private_socket: &Path,
    ) -> Result<(Self, ProxyStream, Option<CodexSharedError>), CodexSharedError> {
        command
            .args(["app-server", "proxy", "--sock"])
            .arg(private_socket);
        Self::spawn_prepared(command)
    }

    fn spawn_prepared(
        mut command: Command,
    ) -> Result<(Self, ProxyStream, Option<CodexSharedError>), CodexSharedError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Never leave an undrained stderr pipe capable of blocking the proxy.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        #[cfg(windows)]
        let job = crate::utils::process::create_kill_on_close_job("Codex proxy")
            .map_err(CodexSharedError::unsupported)?;
        let mut child = command.spawn().map_err(|_| {
            CodexSharedError::unsupported("Codex proxy spawn failed; no handshake was written")
        })?;
        #[cfg(windows)]
        let setup_error = child
            .id()
            .ok_or_else(|| "Codex proxy PID missing".to_owned())
            .and_then(|pid| crate::utils::process::assign_pid_to_job(&job, pid, "Codex proxy"))
            .err()
            .map(CodexSharedError::unsupported);
        #[cfg(not(windows))]
        let setup_error = None;
        // Piped stdio is guaranteed by Command above; no await or cancellation
        // point exists between spawn and transferring the child to its supervisor.
        let stdout = child
            .stdout
            .take()
            .expect("proxy stdout configured as piped");
        let stdin = child.stdin.take().expect("proxy stdin configured as piped");
        let stream = tokio::io::join(stdout, stdin);
        let (stop, mut stopped) = watch::channel(false);
        let supervisor = tokio::spawn(async move {
            let exited = tokio::select! {
                biased;
                _ = async {
                    loop {
                        if *stopped.borrow_and_update() {
                            break;
                        }
                        if stopped.changed().await.is_err() {
                            break;
                        }
                    }
                } => false,
                result = child.wait() => result.is_ok(),
            };
            #[cfg(windows)]
            drop(job);
            if !exited {
                terminate_and_join(&mut child).await;
            }
        });
        Ok((
            Self {
                stop,
                supervisor: Mutex::new(Some(supervisor)),
            },
            stream,
            setup_error,
        ))
    }

    pub(super) fn stop_signal(&self) -> watch::Sender<bool> {
        self.stop.clone()
    }

    pub(super) fn stop(&self) {
        self.stop.send_replace(true);
    }

    pub(super) async fn close(&self) {
        self.stop();
        let mut slot = self.supervisor.lock().await;
        if let Some(supervisor) = slot.as_mut() {
            let _ = supervisor.await;
        }
        slot.take();
    }
}

impl Drop for OwnedProxy {
    fn drop(&mut self) {
        // Dropping JoinHandle detaches, never aborts: the supervisor still reaps.
        self.stop();
    }
}

async fn terminate_and_join(child: &mut Child) {
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.start_kill();
        if matches!(
            tokio::time::timeout(Duration::from_secs(1), child.wait()).await,
            Ok(Ok(_))
        ) {
            return;
        }
        // Do not claim cleanup or release ownership on a failed kill/wait.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn metadata_resume_keeps_policy_without_hydrating_history() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut socket = tokio_tungstenite::accept_async(server_io).await.unwrap();
            let request: Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["method"], "thread/resume");
            assert_eq!(
                request["params"],
                json!({
                    "threadId":"large-history", "model":"configured",
                    "config":{"model_reasoning_effort":"low"}, "excludeTurns":true,
                })
            );
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":{
                        "thread":{"id":"large-history","turns":[],"canAcceptDirectInput":true},
                        "model":"configured","reasoningEffort":"low",
                    }})
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });
        let (socket, _) = tokio_tungstenite::client_async("ws://localhost", client_io)
            .await
            .unwrap();
        let client = CodexSharedClient::from_connected("test".into(), 7, socket, None);
        let response = client
            .resume_metadata(json!({
                "threadId":"large-history","model":"configured",
                "config":{"model_reasoning_effort":"low"},
            }))
            .await
            .unwrap();
        assert_eq!(response["thread"]["id"], "large-history");
        assert_eq!(response["thread"]["canAcceptDirectInput"], true);
        client.close().await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_reply_retains_the_transport_failure_reason() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut socket = tokio_tungstenite::accept_async(server_io).await.unwrap();
            let _request = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text("x".repeat(2048).into()))
                .await
                .unwrap();
        });
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(1024));
        let (socket, _) =
            tokio_tungstenite::client_async_with_config("ws://localhost", client_io, Some(config))
                .await
                .unwrap();
        let client = CodexSharedClient::from_connected("test".into(), 7, socket, None);
        let error = client
            .request("test/oversized", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "submitted_unconfirmed");
        assert!(
            error.message.contains("Message too long"),
            "{}",
            error.message
        );
        assert!(error.provider_boundary_crossed);
        client.close().await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn joined_pipes_preserve_websocket_rpc_and_uncertain_disconnect() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (client_io, server_io) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(client_io);
            let server = tokio::spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(server_io).await.unwrap();
                let first: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(first["method"], "test/echo");
                socket
                    .send(Message::Text(
                        json!({"id":first["id"],"result":first["params"]})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                let second: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(second["method"], "test/disconnect");
                // EOF after accepting a request must remain uncertain.
            });
            let (socket, _) =
                tokio_tungstenite::client_async("ws://localhost", tokio::io::join(read, write))
                    .await
                    .unwrap();
            let client = CodexSharedClient::from_connected("test".into(), 7, socket, None);
            let body = json!({"body":"  literal\nUnicode λ\t  "});
            assert_eq!(
                client.request("test/echo", body.clone()).await.unwrap(),
                body
            );
            let error = client
                .request("test/disconnect", json!({}))
                .await
                .unwrap_err();
            assert_eq!(error.code, "submitted_unconfirmed");
            assert!(error.provider_boundary_crossed);
            client.close().await;
            server.await.unwrap();
            assert!(client.pending.lock().unwrap().is_empty());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn invalid_path_and_existing_cancellation_never_spawn() {
        let error = CodexSharedClient::connect_proxy(
            "test".into(),
            7,
            Command::new("missing-proxy-executable"),
            Path::new("relative.sock"),
            std::future::pending(),
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("absolute private socket"));
        let directory = tempfile::tempdir().unwrap();
        let socket = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("private.sock");
        let error = CodexSharedClient::connect_proxy(
            "test".into(),
            7,
            Command::new("missing-proxy-executable"),
            &socket,
            std::future::ready(()),
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("cancelled"));
        assert!(!error.provider_boundary_crossed);
    }

    // A direct copy of this Rust test binary supplies a child without Python,
    // shell wrappers, provider executables, sockets, or additional descendants.
    #[test]
    fn proxy_test_child() {
        let Some(marker) = std::env::var_os("WARDIAN_PROXY_UNIT_CHILD_MARKER") else {
            return;
        };
        std::fs::write(marker, std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[tokio::test]
    async fn proxy_close_and_dropped_guard_both_join_child_exit() {
        tokio::time::timeout(Duration::from_secs(10), async {
            for explicit_close in [true, false] {
                let directory = tempfile::tempdir().unwrap();
                let marker = directory.path().join("started");
                let mut command = Command::new(std::env::current_exe().unwrap());
                let module = module_path!().split_once("::").unwrap().1;
                command
                    .args([
                        "--exact",
                        &format!("{module}::proxy_test_child"),
                        "--nocapture",
                    ])
                    .env("WARDIAN_PROXY_UNIT_CHILD_MARKER", &marker);
                let (proxy, _stream, setup_error) = OwnedProxy::spawn_prepared(command).unwrap();
                if let Some(error) = setup_error {
                    proxy.close().await;
                    panic!("{error}");
                }
                while !marker.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                if explicit_close {
                    // Both callers must observe supervisor completion, including
                    // when one caller already holds the join slot.
                    tokio::join!(proxy.close(), proxy.close());
                    assert!(proxy.supervisor.lock().await.is_none());
                } else {
                    let supervisor = proxy.supervisor.lock().await.take().unwrap();
                    drop(proxy);
                    // This handle returns only after Child::wait observed exit.
                    supervisor.await.unwrap();
                }
            }
        })
        .await
        .unwrap();
    }
}
