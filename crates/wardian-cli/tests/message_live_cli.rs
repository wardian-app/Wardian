//! Real CLI wire adapter against a bounded synthetic endpoint; not provider acceptance.
use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Command, Output},
    sync::mpsc,
    thread,
    time::Duration,
};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

fn run_cli(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .args(args)
        .env("WARDIAN_HOME", home)
        .env("WARDIAN_SESSION_ID", "managed-sender")
        .output()
        .unwrap()
}

async fn serve_stream(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    response: Option<&str>,
) -> Value {
    let mut request = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut request)
        .await
        .unwrap();
    if let Some(response) = response {
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(b"\n").await.unwrap();
        stream.flush().await.unwrap();
        // Keep a named pipe alive until the client consumes the response and
        // disconnects; dropping the server early can discard buffered bytes.
        let mut closed = [0u8; 1];
        let _ = stream.read(&mut closed).await;
    } else {
        // Exceed the CLI's 500 ms read timeout while keeping the endpoint open.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    serde_json::from_str(&request).unwrap()
}

fn spawn_endpoint(home: &Path, response: Option<&'static str>) -> thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                #[cfg(windows)]
                {
                    // Match the home-specific endpoint without changing the test
                    // process environment (other tests can run concurrently).
                    let hash = home
                        .to_string_lossy()
                        .as_bytes()
                        .iter()
                        .fold(0xcbf29ce484222325u64, |hash, byte| {
                            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                        });
                    let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                        .first_pipe_instance(true)
                        .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                        .unwrap();
                    ready_tx.send(()).unwrap();
                    pipe.connect().await.unwrap();
                    let probe = response.is_none().then(|| {
                        tokio::net::windows::named_pipe::ServerOptions::new()
                            .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                            .unwrap()
                    });
                    let wire = serve_stream(pipe, response).await;
                    if let Some(probe) = probe {
                        assert!(
                            tokio::time::timeout(Duration::from_millis(500), probe.connect())
                                .await
                                .is_err(),
                            "lost receipt caused another connection"
                        );
                    }
                    wire
                }
                #[cfg(unix)]
                {
                    std::fs::create_dir_all(home.join("run")).unwrap();
                    let listener =
                        tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                    ready_tx.send(()).unwrap();
                    let (stream, _) = listener.accept().await.unwrap();
                    let wire = serve_stream(stream, response).await;
                    if response.is_none() {
                        assert!(
                            tokio::time::timeout(Duration::from_millis(500), listener.accept())
                                .await
                                .is_err(),
                            "lost receipt caused another connection"
                        );
                    }
                    wire
                }
            })
            .await
            .expect("CLI must contact and finish the synthetic endpoint")
        })
    });
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("synthetic endpoint must start");
    server
}

#[test]
fn send_and_followup_preserve_admission_keys_and_return_canonical_ids() {
    for (verb, operation, receipt) in [
        (
            "send",
            "send_message",
            r#"{"operation":"send_message","interaction_id":"canonical-info","delivery_state":"pending","duplicate":false}"#,
        ),
        (
            "followup",
            "followup_task",
            r#"{"operation":"followup_task","request_id":"canonical-task","delivery_state":"pending","delivery_owner":"unclaimed","duplicate":false}"#,
        ),
    ] {
        let home = TempDir::new().unwrap();
        let server = spawn_endpoint(home.path(), Some(receipt));
        let output = run_cli(
            home.path(),
            &[
                "message",
                verb,
                "Peer",
                "literal\nbody",
                "--idempotency-key",
                "Stable-Key:1288",
            ],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wire = server.join().unwrap();
        assert_eq!(
            wire,
            json!({"command":"agent_messaging","origin":{"kind":"wardian_agent","session_id":"managed-sender"},"request":{"operation":operation,"target":"Peer","message":"literal\nbody","idempotency_key":"Stable-Key:1288"}})
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::from_str::<Value>(receipt).unwrap()
        );
    }
}

#[test]
fn reply_uses_exact_canonical_request_and_receive_preserves_cursors() {
    let home = TempDir::new().unwrap();
    let server = spawn_endpoint(
        home.path(),
        Some(
            r#"{"operation":"reply","request_id":"canonical-task","interaction_id":"canonical-reply","delivery_state":"pending","duplicate":false}"#,
        ),
    );
    let output = run_cli(
        home.path(),
        &[
            "message",
            "reply",
            "canonical-task",
            "result",
            "--status",
            "done",
        ],
    );
    assert!(output.status.success());
    let wire = server.join().unwrap();
    assert_eq!(
        wire["request"],
        json!({"operation":"reply","request_id":"canonical-task","status":"done","message":"result"})
    );
    let home = TempDir::new().unwrap();
    let server = spawn_endpoint(
        home.path(),
        Some(
            r#"{"operation":"receive_messages","messages":[],"next_cursor":"next:2","ack_cursor":"ack:2","has_more":false,"timed_out":true}"#,
        ),
    );
    let output = run_cli(
        home.path(),
        &[
            "message",
            "receive",
            "--cursor",
            "next:1",
            "--ack-cursor",
            "ack:1",
            "--limit",
            "7",
            "--timeout-ms",
            "25",
        ],
    );
    assert!(output.status.success());
    let wire = server.join().unwrap();
    assert_eq!(
        wire["request"],
        json!({"operation":"receive_messages","cursor":"next:1","ack_cursor":"ack:1","limit":7,"timeout_ms":25})
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["next_cursor"], "next:2");
    assert_eq!(result["timed_out"], true);
}

#[test]
fn mismatched_reply_receipt_is_uncertainty_not_success() {
    let home = TempDir::new().unwrap();
    let server = spawn_endpoint(
        home.path(),
        Some(
            r#"{"operation":"reply","request_id":"other-task","interaction_id":"reply-id","delivery_state":"pending","duplicate":false}"#,
        ),
    );
    let output = run_cli(
        home.path(),
        &[
            "message",
            "reply",
            "canonical-task",
            "result",
            "--status",
            "done",
        ],
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("do not replay automatically"));
    assert_eq!(
        server.join().unwrap()["request"]["request_id"],
        "canonical-task"
    );
}

#[test]
fn lost_receipt_is_not_replayed_or_reported_as_success() {
    let home = TempDir::new().unwrap();
    let server = spawn_endpoint(home.path(), None);
    let output = run_cli(home.path(), &["message", "followup", "Peer", "task"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("do not replay automatically"));
    assert_eq!(
        server.join().unwrap()["request"]["operation"],
        "followup_task"
    );
}
