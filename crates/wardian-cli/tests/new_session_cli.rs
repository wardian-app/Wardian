//! Verify normal lifecycle CLI requests against the bounded synthetic endpoint.
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

async fn serve_stream(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    response: Option<&Value>,
    receipt_home: Option<&Path>,
) -> Value {
    let mut request = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut request)
        .await
        .unwrap();
    let request: Value = serde_json::from_str(&request).unwrap();
    if let Some(home) = receipt_home {
        if request["command"] == "agent_spawn_async" {
            let operation_id = request["operation_id"].as_str().unwrap();
            let receipt: Value = serde_json::from_slice(
                &fs::read(
                    home.join("spawn-submissions")
                        .join(format!("{operation_id}.json")),
                )
                .expect("generated key must be durable before admission"),
            )
            .unwrap();
            assert_eq!(receipt["operation_id"], operation_id);
        }
    }
    let Some(response) = response else {
        return request;
    };
    let mut response = response.clone();
    if let Some(operation) = response.get_mut("operation") {
        operation["operation_id"] = request["operation_id"].clone();
    }
    stream
        .write_all(response.to_string().as_bytes())
        .await
        .unwrap();
    stream.write_all(b"\n").await.unwrap();
    stream.flush().await.unwrap();
    // Named-pipe buffers must remain alive until the client consumes the reply.
    let mut closed = [0_u8; 1];
    assert_eq!(stream.read(&mut closed).await.unwrap(), 0);
    request
}

fn endpoint(home: &Path, response: String) -> thread::JoinHandle<Vec<Value>> {
    endpoint_sequence(
        home,
        vec![Some(serde_json::from_str(&response).unwrap())],
        false,
    )
}

fn endpoint_sequence(
    home: &Path,
    responses: Vec<Option<Value>>,
    require_receipt: bool,
) -> thread::JoinHandle<Vec<Value>> {
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
                        let mut pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                            .first_pipe_instance(true)
                            .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                            .unwrap();
                        ready_tx.send(()).unwrap();
                        let mut requests = Vec::new();
                        for response in &responses {
                            pipe.connect().await.unwrap();
                            let next = tokio::net::windows::named_pipe::ServerOptions::new()
                                .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                                .unwrap();
                            requests.push(
                                serve_stream(
                                    pipe,
                                    response.as_ref(),
                                    require_receipt.then_some(home.as_path()),
                                )
                                .await,
                            );
                            pipe = next;
                        }
                        assert!(
                            tokio::time::timeout(Duration::from_millis(250), pipe.connect())
                                .await
                                .is_err(),
                            "CLI replayed a completed or uncertain submission"
                        );
                        requests
                    }
                    #[cfg(unix)]
                    {
                        fs::create_dir_all(home.join("run")).unwrap();
                        let listener =
                            tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                        ready_tx.send(()).unwrap();
                        let mut requests = Vec::new();
                        for response in &responses {
                            let (stream, _) = listener.accept().await.unwrap();
                            requests.push(
                                serve_stream(
                                    stream,
                                    response.as_ref(),
                                    require_receipt.then_some(home.as_path()),
                                )
                                .await,
                            );
                        }
                        assert!(
                            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                                .await
                                .is_err(),
                            "CLI replayed a completed or uncertain submission"
                        );
                        requests
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
        vec![json!({"command":"agent_new_session","target":"paused-agent"})]
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
            vec![json!({"command":"agent_new_session","target":"paused-agent"})]
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

fn spawn_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-cli"));
    command
        .args([
            "agent",
            "spawn",
            "--provider",
            "codex",
            "--class",
            "Coder",
            "--name",
            "Fixture",
            "--workspace",
        ])
        .arg(home)
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID");
    command
}

fn operation(state: &str) -> Value {
    json!({
        "operation_id":"filled-from-request",
        "agent_id":"9f13776e-df94-4bc4-a72b-2d8ea25948ce",
        "agent_name":"Fixture", "provider":"codex", "class":"Coder", "state":state,
        "created_at":"2026-11-10T00:00:00Z", "updated_at":"2026-11-10T00:00:00Z"
    })
}

#[test]
fn asynchronous_spawn_persists_generated_key_before_one_admission() {
    let home = TempDir::new().unwrap();
    let server = endpoint_sequence(
        home.path(),
        vec![Some(json!({"schema":1,"operation":operation("queued")}))],
        true,
    );
    let output = spawn_command(home.path()).arg("--async").output().unwrap();
    let requests = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["command"], "agent_spawn_async");
    let key = requests[0]["operation_id"].as_str().unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["operation_id"], key);
    assert_eq!(result["operation"]["state"], "queued");
    assert!(result["lookup"].as_str().unwrap().contains(key));
    assert!(requests[0]["request"].get("async_mode").is_none());
    assert!(requests[0]["request"].get("wait_timeout").is_none());
}

#[test]
fn normal_spawn_admits_waits_then_preserves_identity_field_output() {
    let home = TempDir::new().unwrap();
    let key = "5d4ebea1-1ce4-44fa-a67e-410f74aa1e0c";
    let server = endpoint_sequence(
        home.path(),
        vec![
            Some(json!({"schema":1,"operation":operation("queued")})),
            Some(
                json!({"schema":1,"operation":operation("registered"),"timed_out":false,
                    "availability":"current", "current_agent":{
                    "name":"Fixture", "uuid":"9f13776e-df94-4bc4-a72b-2d8ea25948ce",
                    "class":"Coder", "provider":"codex", "status":"idle", "status_source":"live",
                    "pid":null, "started_at":null, "workspace":home.path(), "last_status_at":null
                }}),
            ),
        ],
        false,
    );
    let output = spawn_command(home.path())
        .args(["--operation-id", key, "--fields", "uuid,name"])
        .output()
        .unwrap();
    let requests = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request["command"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["agent_spawn_async", "agent_spawn_operation_wait"]
    );
    assert_eq!(requests[0]["operation_id"], key);
    assert_eq!(requests[1]["operation_id"], key);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({"schema":1,"agent":{"uuid":"9f13776e-df94-4bc4-a72b-2d8ea25948ce","name":"Fixture"}})
    );
}

#[test]
fn registered_retry_requires_authoritative_current_roster_identity() {
    for availability in ["missing", "incompatible", "unavailable"] {
        let home = TempDir::new().unwrap();
        let key = "5d4ebea1-1ce4-44fa-a67e-410f74aa1e0c";
        let server = endpoint_sequence(
            home.path(),
            vec![
                Some(json!({"schema":1,"operation":operation("registered")})),
                Some(
                    json!({"schema":1,"operation":operation("registered"),"timed_out":false,"availability":availability}),
                ),
            ],
            false,
        );
        let output = spawn_command(home.path())
            .args(["--operation-id", key])
            .output()
            .unwrap();
        let requests = server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(requests.len(), 2);
        assert!(requests
            .iter()
            .all(|request| request["operation_id"] == key));
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().contains(key));
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("current roster identity"));
    }
}

#[test]
fn rejected_or_lost_spawn_admission_retains_key_without_legacy_fallback_or_replay() {
    for response in [
        Some(
            json!({"schema":1,"error":{"code":"bad_request", "message":"malformed control request JSON: unknown variant `agent_spawn_async`"}}),
        ),
        None,
    ] {
        let home = TempDir::new().unwrap();
        let state = home.path().join("settings/state.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, "[]").unwrap();
        let old_app = response.is_some();
        let server = endpoint_sequence(home.path(), vec![response], true);
        let output = spawn_command(home.path()).output().unwrap();
        let requests = server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["command"], "agent_spawn_async");
        let key = requests[0]["operation_id"].as_str().unwrap();
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().contains(key));
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("wardian agent spawn-status"));
        if old_app {
            assert_eq!(error["error"]["code"], "unsupported_version");
        }
        assert_eq!(fs::read_to_string(&state).unwrap(), "[]");
        let receipts = fs::read_dir(home.path().join("spawn-submissions"))
            .unwrap()
            .count();
        assert_eq!(receipts, 1);
    }
}
