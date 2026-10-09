//! Verify the official New Session request and fail-closed transport behavior.
use std::{fs, path::Path, process::Command, sync::mpsc, thread, time::Duration};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-cli"));
    command
        .args(["agent", "new-session", "paused-agent"])
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID");
    command
}

async fn serve_stream(mut stream: impl AsyncRead + AsyncWrite + Unpin, response: &str) -> Value {
    let mut request = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut request)
        .await
        .unwrap();
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    stream.flush().await.unwrap();
    // Named-pipe buffers must remain alive until the client consumes the reply.
    let mut closed = [0_u8; 1];
    assert_eq!(stream.read(&mut closed).await.unwrap(), 0);
    serde_json::from_str(&request).unwrap()
}

fn endpoint(home: &Path, response: String) -> thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(15), async {
                    #[cfg(windows)]
                    {
                        let hash = home
                            .to_string_lossy()
                            .as_bytes()
                            .iter()
                            .fold(0xcbf29ce484222325_u64, |hash, byte| {
                                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                            });
                        let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                            .first_pipe_instance(true)
                            .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                            .unwrap();
                        ready_tx.send(()).unwrap();
                        pipe.connect().await.unwrap();
                        serve_stream(pipe, &response).await
                    }
                    #[cfg(unix)]
                    {
                        fs::create_dir_all(home.join("run")).unwrap();
                        let listener =
                            tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                        ready_tx.send(()).unwrap();
                        let (stream, _) = listener.accept().await.unwrap();
                        serve_stream(stream, &response).await
                    }
                })
                .await
                .expect("one bounded CLI request must finish")
            })
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    server
}

#[test]
fn new_session_sends_one_explicit_request() {
    let home = TempDir::new().unwrap();
    let server = endpoint(home.path(), json!({"schema":1,"ok":true}).to_string());
    let output = command(home.path()).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        server.join().unwrap(),
        json!({"command":"agent_new_session","target":"paused-agent"})
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["target"], "paused-agent");
    assert_eq!(result["ok"], true);
}

#[test]
fn new_session_preserves_backend_rejections_without_fallback_or_disk_mutation() {
    for (code, message) in [
        ("bad_request", "unknown variant agent_new_session"),
        ("request_failed", "conversation lifecycle lease is held"),
        ("not_found", "agent not found: paused-agent"),
    ] {
        let home = TempDir::new().unwrap();
        let state = home.path().join("settings/state.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        let original =
            b"[{\"session_id\":\"saved-agent\",\"is_off\":true,\"resume_session\":null}]";
        fs::write(&state, original).unwrap();
        let server = endpoint(
            home.path(),
            json!({"schema":1,"error":{"code":code,"message":message}}).to_string(),
        );
        let output = command(home.path()).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
        assert_eq!(error["error"]["message"], message);
        assert_eq!(
            server.join().unwrap(),
            json!({"command":"agent_new_session","target":"paused-agent"})
        );
        assert_eq!(fs::read(&state).unwrap(), original);
    }
}

#[test]
fn new_session_requires_the_app_even_when_saved_state_exists() {
    let home = TempDir::new().unwrap();
    let state = home.path().join("settings/state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    let original = b"[{\"session_id\":\"paused-agent\",\"is_off\":true,\"resume_session\":null}]";
    fs::write(&state, original).unwrap();
    let output = command(home.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "app_not_running");
    assert_eq!(fs::read(&state).unwrap(), original);
}
