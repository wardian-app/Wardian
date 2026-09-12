//! Real CLI process and local control transport; no provider or Wardian app runs.
use serde_json::{json, Value};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

fn handshake() -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ]
}

fn run(test_home: &Path, requests: Vec<Value>) -> Vec<Value> {
    run_with_sender(test_home, requests, true)
}

fn run_with_sender(test_home: &Path, requests: Vec<Value>, managed: bool) -> Vec<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-cli"));
    if managed {
        command.env("WARDIAN_SESSION_ID", "sender-managed-uuid");
    } else {
        command.env_remove("WARDIAN_SESSION_ID");
    }
    let mut child = command
        .args(["mcp", "serve"])
        .env("WARDIAN_HOME", test_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("MCP child did not exit at EOF");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout must contain only JSON-RPC"))
        .collect()
}

#[test]
fn stdio_discovery_and_rejections_do_not_create_home_or_need_app() {
    let root = tempfile::tempdir().unwrap();
    let test_home = root.path().join("absent-home");
    let mut requests = handshake();
    requests.extend([
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"send_message","arguments":{"target":"Peer","message":"x","interrupt":true}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
    ]);
    let responses = run(&test_home, requests);
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 6);
    let error: Value = serde_json::from_str(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(error["error"]["code"], "invalid_arguments");
    assert_eq!(responses[3]["result"], json!({}));
    assert!(!test_home.exists(), "MCP entry must bypass CLI migrations");
}

async fn exchange(stream: impl AsyncRead + AsyncWrite + Unpin, response: Value) -> Value {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    let request = serde_json::from_str(&line).unwrap();
    stream
        .write_all(format!("{response}\n").as_bytes())
        .await
        .unwrap();
    stream.flush().await.unwrap();
    request
}

fn control_fixture(test_home: &Path, responses: Vec<Value>) -> thread::JoinHandle<Vec<Value>> {
    let test_home = test_home.to_path_buf();
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let operation = async move {
                let mut requests = Vec::new();
                #[cfg(windows)]
                {
                    // Same public endpoint-key algorithm, isolated without mutating process env.
                    let key = test_home
                        .to_string_lossy()
                        .bytes()
                        .fold(0xcbf29ce484222325u64, |hash, byte| {
                            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
                        });
                    let name = format!(r"\\.\pipe\wardian-control-{key:016x}");
                    let pipes = responses
                        .iter()
                        .map(|_| {
                            tokio::net::windows::named_pipe::ServerOptions::new()
                                .create(&name)
                                .unwrap()
                        })
                        .collect::<Vec<_>>();
                    ready_tx.send(()).unwrap();
                    for (pipe, response) in pipes.into_iter().zip(responses) {
                        pipe.connect().await.unwrap();
                        requests.push(exchange(pipe, response).await);
                    }
                }
                #[cfg(unix)]
                {
                    std::fs::create_dir_all(test_home.join("run")).unwrap();
                    let listener =
                        tokio::net::UnixListener::bind(test_home.join("run/control.sock")).unwrap();
                    ready_tx.send(()).unwrap();
                    for response in responses {
                        let (stream, _) = listener.accept().await.unwrap();
                        requests.push(exchange(stream, response).await);
                    }
                }
                requests
            };
            tokio::time::timeout(Duration::from_secs(10), operation)
                .await
                .expect("control fixture timed out")
        })
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    worker
}

#[test]
fn stdio_six_calls_use_typed_control_and_preserve_literal_body_origin_and_receipts() {
    let root = tempfile::tempdir().unwrap();
    let message =
        "  \u{4e2d}\u{6587} \u{3bb}\u{1f600}\nline two\r\n\"quotes\" C:\\path\\file\t\0\u{1b}  ";
    let specs = [
        (
            "send_message",
            json!({"target":"Exact Peer","message":message}),
            json!({"operation":"send_message","interaction_id":"info-1","delivery_state":"pending","duplicate":false}),
        ),
        (
            "followup_task",
            json!({"target":"peer-uuid","message":message}),
            json!({"operation":"followup_task","request_id":"task-1","delivery_owner":"receiver","delivery_state":"pending","duplicate":false}),
        ),
        (
            "receive_messages",
            json!({"cursor":"cursor-1","ack_cursor":"ack-1","limit":7,"timeout_ms":0}),
            json!({"operation":"receive_messages","messages":[],"next_cursor":"cursor-2","ack_cursor":"ack-2","has_more":false,"timed_out":true}),
        ),
        (
            "reply",
            json!({"request_id":"task-1","status":"done","message":message}),
            json!({"operation":"reply","request_id":"task-1","interaction_id":"reply-1","delivery_state":"pending","duplicate":false}),
        ),
        (
            "interrupt_agent",
            json!({"target":"peer-uuid"}),
            json!({"error":{"code":"unsupported_capability","message":"No supported interrupt transport"}}),
        ),
        (
            "list_agents",
            json!({}),
            json!({"operation":"list_agents","agents":[]}),
        ),
    ];
    let server = control_fixture(
        root.path(),
        specs
            .iter()
            .map(|(_, _, response)| response.clone())
            .collect(),
    );
    let mut requests = handshake();
    for (name, arguments, _) in &specs {
        requests.push(json!({"jsonrpc":"2.0","id":name,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
    }
    // Notifications cannot create a seventh control operation or interrupt.
    requests.push(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"followup_task"}}));
    let responses = run(root.path(), requests);
    let sent = server.join().unwrap();
    assert_eq!(sent.len(), 6);
    assert_eq!(responses.len(), 7);
    for (index, (name, args, receipt)) in specs.iter().enumerate() {
        assert_eq!(sent[index]["command"], "agent_messaging");
        assert_eq!(
            sent[index]["origin"],
            json!({"kind":"wardian_agent","session_id":"sender-managed-uuid"})
        );
        assert_eq!(sent[index]["request"]["operation"], *name);
        for (key, value) in args.as_object().unwrap() {
            assert_eq!(&sent[index]["request"][key], value);
        }
        let _: wardian_core::control::ControlRequest =
            serde_json::from_value(sent[index].clone()).unwrap();
        assert_eq!(responses[index + 1]["id"], *name);
        if *name == "interrupt_agent" {
            assert_eq!(responses[index + 1]["result"]["isError"], true);
            assert_eq!(
                responses[index + 1]["result"]["structuredContent"]["error"]["code"],
                "unsupported_capability"
            );
        } else {
            assert_eq!(
                responses[index + 1]["result"]["structuredContent"],
                *receipt
            );
        }
    }
    let first = sent[0]["request"]["idempotency_key"].as_str().unwrap();
    let second = sent[1]["request"]["idempotency_key"].as_str().unwrap();
    assert!(first.starts_with("mcp-"));
    assert_ne!(first, second);
    assert!(!root.path().join("state.db").exists());
}

#[test]
fn stdio_interrupt_receipts_preserve_actor_facts_without_claiming_confirmation() {
    use wardian_core::agent_messaging::AgentMessagingResponse;

    let root = tempfile::tempdir().unwrap();
    let receipts: Vec<Value> = [
        ("no_active_turn", false, None),
        ("interrupt_requested", false, Some("native-turn")),
        ("interrupted", true, Some("native-turn")),
    ]
    .into_iter()
    .map(|(state, confirmed, turn)| {
        serde_json::to_value(AgentMessagingResponse::InterruptAgent {
            target_agent_id: "peer-uuid".into(),
            generation: 47,
            delivery_state: state.into(),
            interruption_confirmed: confirmed,
            provider_session_id: "native-thread".into(),
            provider_turn_id: turn.map(str::to_owned),
        })
        .unwrap()
    })
    .collect();
    let server = control_fixture(root.path(), receipts.clone());
    let mut requests = handshake();
    for index in 0..receipts.len() {
        requests.push(json!({"jsonrpc":"2.0","id":index + 2,"method":"tools/call",
            "params":{"name":"interrupt_agent","arguments":{"target":"peer-uuid"}}}));
    }
    let responses = run(root.path(), requests);
    let sent = server.join().unwrap();
    assert_eq!(sent.len(), receipts.len());
    assert_eq!(responses.len(), receipts.len() + 1);
    for (index, expected) in receipts.iter().enumerate() {
        assert_eq!(
            sent[index],
            json!({"command":"agent_messaging",
            "origin":{"kind":"wardian_agent","session_id":"sender-managed-uuid"},
            "request":{"operation":"interrupt_agent","target":"peer-uuid"}})
        );
        let response = &responses[index + 1];
        assert_eq!(response["id"], index + 2);
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(response["result"]["structuredContent"], *expected);
        let text: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(text, *expected);
        assert!(text.get("interaction_id").is_none());
        assert!(text.get("message_id").is_none());
    }
    // These are transport fixtures, not evidence of a real vendor interruption.
    assert!(!root.path().join("state.db").exists());
}

#[test]
fn stdio_missing_sender_and_unknown_tools_fail_without_opening_control() {
    let root = tempfile::tempdir().unwrap();
    let test_home = root.path().join("absent");
    let mut requests = handshake();
    requests.extend([
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"send_message","arguments":{"target":"Peer","message":"x"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_agents","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"send_input","arguments":{}}}),
    ]);
    let responses = run_with_sender(&test_home, requests, false);
    for response in &responses[1..3] {
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "missing_managed_sender"
        );
    }
    assert_eq!(responses[3]["error"]["code"], -32602);
    assert!(!test_home.exists());
}

#[test]
fn stdio_backend_ambiguity_and_lost_receipt_preserve_errors_without_replay() {
    let root = tempfile::tempdir().unwrap();
    let server = control_fixture(
        root.path(),
        vec![
            json!({"error":{"code":"ambiguous_target","message":"Use UUID"}}),
            json!([]),
        ],
    );
    let first = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"send_message","arguments":{"target":"Peer","message":"literal"}}});
    let second = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"followup_task","arguments":{"target":"peer-uuid","message":"literal"}}});
    let mut requests = handshake();
    requests.extend([first.clone(), first, second.clone(), second]);
    let responses = run(root.path(), requests);
    assert_eq!(server.join().unwrap().len(), 2);
    assert_eq!(
        responses[1]["result"]["structuredContent"]["error"]["code"],
        "ambiguous_target"
    );
    assert_eq!(responses[2]["error"]["code"], -32600);
    assert_eq!(
        responses[3]["result"]["structuredContent"]["error"]["code"],
        "invalid_receipt"
    );
    assert_eq!(responses[4]["error"]["code"], -32600);
}
