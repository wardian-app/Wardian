//! Exercise Inbox fallback and provenance through the real CLI transport.
//! Each server and persisted queue belongs to a separate temporary home.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

fn seed_home() -> TempDir {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join("queue")).unwrap();
    fs::write(
        home.path().join("queue/items.json"),
        serde_json::to_vec(&json!([{
            "id": "persisted-only",
            "type": "action_needed",
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "read": false,
            "evidence_source": "provider_runtime"
        }]))
        .unwrap(),
    )
    .unwrap();
    home
}

fn run_cli(home: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .args(["inbox", "list"])
        .args(extra)
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap()
}

async fn serve_stream(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    response: Option<&str>,
    response_delay: Duration,
) -> Value {
    let mut request = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut request)
        .await
        .unwrap();
    tokio::time::sleep(response_delay).await;
    if let Some(response) = response {
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(b"\n").await.unwrap();
        stream.flush().await.unwrap();
        // Keep a named pipe alive until the client consumes the response and
        // disconnects; dropping the server early can discard buffered bytes.
        let mut closed = [0u8; 1];
        let _ = stream.read(&mut closed).await;
    } else {
        // Exceed the Inbox-specific deadline while keeping the endpoint open.
        tokio::time::sleep(Duration::from_secs(11)).await;
    }
    serde_json::from_str(&request).unwrap()
}

fn spawn_endpoint(home: &Path, response: Option<&'static str>) -> thread::JoinHandle<Value> {
    spawn_endpoint_after(home, response, Duration::ZERO)
}

fn spawn_endpoint_after(
    home: &Path,
    response: Option<&'static str>,
    response_delay: Duration,
) -> thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(15), async {
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
                    serve_stream(pipe, response, response_delay).await
                }
                #[cfg(unix)]
                {
                    fs::create_dir_all(home.join("run")).unwrap();
                    let listener =
                        tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                    ready_tx.send(()).unwrap();
                    let (stream, _) = listener.accept().await.unwrap();
                    serve_stream(stream, response, response_delay).await
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
fn missing_endpoint_returns_persisted_source_and_items() {
    let home = seed_home();
    let output = run_cli(home.path(), &[]);
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema"], 1);
    assert_eq!(response["status_source"], "persisted");
    assert_eq!(response["items"][0]["id"], "persisted-only");
    assert_eq!(response["truncated"], false);
    assert_eq!(response["next_offset"], Value::Null);
}

#[cfg(unix)]
#[test]
fn refused_endpoint_returns_persisted_source() {
    let home = seed_home();
    fs::create_dir_all(home.path().join("run")).unwrap();
    let listener =
        std::os::unix::net::UnixListener::bind(home.path().join("run/control.sock")).unwrap();
    drop(listener); // Leave the socket path but no listening endpoint.
    let output = run_cli(home.path(), &[]);
    assert!(output.status.success(), "{:?}", output);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status_source"], "persisted");
    assert_eq!(response["items"][0]["id"], "persisted-only");
}

#[test]
fn live_response_preserves_filters_paging_and_source_without_merging_disk() {
    let home = seed_home();
    let server = spawn_endpoint(
        home.path(),
        Some(r#"{"schema":1,"items":[{"id":"live-only"}],"truncated":true,"next_offset":3}"#),
    );
    let output = run_cli(
        home.path(),
        &[
            "--limit",
            "1",
            "--offset",
            "2",
            "--type",
            "action_needed",
            "--source",
            "live_runtime",
            "--unread",
        ],
    );
    let request = server.join().unwrap();
    assert_eq!(request["command"], "inbox_list");
    assert_eq!(request["limit"], 1);
    assert_eq!(request["offset"], 2);
    assert_eq!(request["types"], json!(["action_needed"]));
    assert_eq!(request["sources"], json!(["live_runtime"]));
    assert_eq!(request["unread"], true);
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response,
        json!({
            "schema": 1,
            "status_source": "live",
            "items": [{"id": "live-only"}],
            "truncated": true,
            "next_offset": 3,
        })
    );
}

#[test]
fn empty_live_response_does_not_fall_back_to_available_disk_items() {
    let home = seed_home();
    let server = spawn_endpoint(
        home.path(),
        Some(r#"{"schema":1,"items":[],"truncated":false,"next_offset":null}"#),
    );
    let output = run_cli(home.path(), &[]);
    server.join().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status_source"], "live");
    assert_eq!(response["items"], json!([]));
}

#[test]
fn delayed_live_inbox_response_remains_live_instead_of_timing_out() {
    let home = seed_home();
    let server = spawn_endpoint_after(
        home.path(),
        Some(
            r#"{"schema":1,"items":[{"id":"live-after-work"}],"truncated":false,"next_offset":null}"#,
        ),
        Duration::from_millis(800),
    );
    let output = run_cli(home.path(), &["--limit", "200"]);
    let request = server.join().unwrap();
    assert_eq!(request["command"], "inbox_list");
    assert_eq!(request["limit"], 200);
    assert!(output.status.success(), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status_source"], "live");
    assert_eq!(response["items"][0]["id"], "live-after-work");
}

#[test]
fn semantic_permission_and_protocol_errors_do_not_fall_back() {
    for response in [
        r#"{"error":{"code":"bad_request","message":"fixture rejection"}}"#,
        r#"{"error":{"code":"unauthorized","message":"fixture rejection"}}"#,
        r#"{"error":{"code":"not_found","message":"fixture rejection"}}"#,
        "not valid json",
        r#"{"schema":1,"items":"invalid item collection"}"#,
    ] {
        let home = seed_home();
        let server = spawn_endpoint(home.path(), Some(response));
        let output = run_cli(home.path(), &[]);
        server.join().unwrap();
        assert!(!output.status.success(), "accepted {response}: {output:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["schema"], 1);
        assert!(error["error"]["code"].is_string());
        if response.contains("fixture rejection") {
            assert_eq!(error["error"]["message"], "fixture rejection");
        }
    }
}

#[test]
fn timeout_propagates_instead_of_returning_persisted_success() {
    let home = seed_home();
    let server = spawn_endpoint(home.path(), None);
    let output = run_cli(home.path(), &[]);
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "control_endpoint_timeout");
}
