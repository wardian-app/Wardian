//! Real CLI process and local control transport; no provider or Wardian app runs.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader as StdBufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

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

fn response_shape(response: &Value) -> String {
    let response_type = if response.get("error").is_some() {
        "rpc_error"
    } else if response.get("result").is_some() {
        "tool_result"
    } else {
        "unknown"
    };
    format!(
        "id={} type={} rpc_error_code={} result_is_error={} structured_error_code={}",
        response["id"],
        response_type,
        response["error"]["code"],
        response["result"]["isError"],
        response["result"]["structuredContent"]["error"]["code"]
    )
}

fn run_with_sender(test_home: &Path, requests: Vec<Value>, managed: bool) -> Vec<Value> {
    let executable = env!("CARGO_BIN_EXE_wardian-mcp");
    let mut command = Command::new(executable);
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

fn spawn_interactive_mcp(
    test_home: &Path,
) -> (
    std::process::Child,
    std::process::ChildStdin,
    StdBufReader<std::process::ChildStdout>,
) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wardian-mcp"))
        .args(["mcp", "serve"])
        .env("WARDIAN_HOME", test_home)
        .env("WARDIAN_SESSION_ID", "sender-managed-uuid")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = StdBufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn write_rpc(stdin: &mut std::process::ChildStdin, request: &Value) {
    writeln!(stdin, "{request}").unwrap();
}

fn read_rpc(stdout: &mut StdBufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn wait_success(child: &mut std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("MCP child did not exit after stdin EOF");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "{}",
        child
            .stderr
            .take()
            .map(|mut stderr| {
                let mut text = String::new();
                let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
                text
            })
            .unwrap_or_default()
    );
}

#[test]
fn output_failure_ends_the_direct_server_while_stdin_remains_open() {
    let test_home = tempfile::tempdir().unwrap();
    let (mut child, mut stdin, mut stdout) = spawn_interactive_mcp(test_home.path());
    write_rpc(&mut stdin, &handshake()[0]);
    assert_eq!(
        read_rpc(&mut stdout)["result"]["serverInfo"]["name"],
        "wardian"
    );
    drop(stdout);
    write_rpc(&mut stdin, &handshake()[1]);
    write_rpc(&mut stdin, &json!({"jsonrpc":"2.0","id":2,"method":"ping"}));

    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            drop(stdin);
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("MCP server stayed alive after its output pipe failed");
        }
        thread::sleep(Duration::from_millis(10));
    };
    drop(stdin);
    assert_eq!(status.code(), Some(1));
}

#[test]
fn unread_stdout_does_not_keep_the_direct_server_alive() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wardian-mcp"))
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _stdout = child.stdout.take().unwrap();
    let sender = thread::spawn(move || {
        for id in 0..100_000 {
            if writeln!(
                stdin,
                "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}"
            )
            .is_err()
            {
                return;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            sender.join().unwrap();
            panic!("MCP server stayed alive while stdout remained unread");
        }
        thread::sleep(Duration::from_millis(10));
    };
    sender.join().unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn direct_entry_keeps_the_explicit_mcp_serve_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_wardian-mcp"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    assert!(output.stdout.is_empty());
}

#[test]
fn console_cli_keeps_the_explicit_mcp_serve_command() {
    let test_home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .args(["mcp", "serve"])
        .env("WARDIAN_HOME", test_home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = StdBufReader::new(child.stdout.take().unwrap());
    write_rpc(&mut stdin, &handshake()[0]);
    assert_eq!(
        read_rpc(&mut stdout)["result"]["serverInfo"]["name"],
        "wardian"
    );
    write_rpc(&mut stdin, &handshake()[1]);
    write_rpc(&mut stdin, &json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    assert_eq!(read_rpc(&mut stdout)["result"], json!({}));
    drop(stdin);
    wait_success(&mut child);
}

#[cfg(windows)]
#[test]
fn self_contained_mcp_server_uses_the_windows_gui_subsystem() {
    let image = std::fs::read(env!("CARGO_BIN_EXE_wardian-mcp")).unwrap();
    let pe_offset = u32::from_le_bytes(image[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&image[pe_offset..pe_offset + 4], b"PE\0\0");
    let subsystem_offset = pe_offset + 24 + 68;
    let subsystem = u16::from_le_bytes(
        image[subsystem_offset..subsystem_offset + 2]
            .try_into()
            .unwrap(),
    );
    assert_eq!(subsystem, 2, "MCP server must use the GUI subsystem");
}

#[cfg(windows)]
#[test]
fn self_contained_mcp_server_has_no_adjacent_cli_child() {
    let test_home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wardian-mcp"))
        .args(["mcp", "serve"])
        .env("WARDIAN_HOME", test_home.path())
        .env("WARDIAN_SESSION_ID", "sender-managed-uuid")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = StdBufReader::new(child.stdout.take().unwrap());
    let initialization = handshake().remove(0);
    writeln!(stdin, "{initialization}").unwrap();
    let mut response = String::new();
    stdout.read_line(&mut response).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["result"]["serverInfo"]["name"],
        "wardian"
    );

    thread::sleep(Duration::from_millis(100));
    assert!(
        !has_wardian_cli_child(child.id()),
        "wardian-mcp must run the shared MCP server without spawning wardian-cli"
    );
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":2,"method":"ping"})).unwrap();
    response.clear();
    stdout.read_line(&mut response).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["result"],
        json!({})
    );
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("direct MCP server did not terminate after stdin EOF");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
}

#[cfg(windows)]
fn has_wardian_cli_child(parent_pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        assert_ne!(snapshot, INVALID_HANDLE_VALUE);
        let mut entry = std::mem::zeroed::<PROCESSENTRY32W>();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut available = Process32FirstW(snapshot, &mut entry) != 0;
        while available {
            let length = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let filename = String::from_utf16_lossy(&entry.szExeFile[..length]);
            if entry.th32ParentProcessID == parent_pid
                && filename.eq_ignore_ascii_case("wardian-cli.exe")
            {
                found = true;
                break;
            }
            available = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
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
    let list = responses
        .iter()
        .find(|response| response["id"] == 2)
        .unwrap();
    let invalid = responses
        .iter()
        .find(|response| response["id"] == 3)
        .unwrap();
    let ping = responses
        .iter()
        .find(|response| response["id"] == 4)
        .unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "send_message",
            "followup_task",
            "reply",
            "interrupt_agent",
            "list_agents",
        ]
    );
    assert!(!names.contains(&"receive_messages"));
    assert!(!names.contains(&"wait_agent"));
    let error: Value =
        serde_json::from_str(invalid["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "invalid_arguments");
    assert_eq!(ping["result"], json!({}));
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
    // Tokio flush hands bytes to the pipe but does not prove that the client
    // consumed them. Keep the server endpoint alive until the client closes
    // its side so a malformed receipt remains invalid_receipt rather than
    // being misreported as a lost transport response.
    let mut closed = [0u8; 1];
    let _ = stream.read(&mut closed).await;
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
fn stdio_five_calls_use_typed_control_and_preserve_literal_body_origin_and_receipts() {
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
    let (mut child, mut stdin, mut stdout) = spawn_interactive_mcp(root.path());
    write_rpc(&mut stdin, &handshake()[0]);
    let mut responses = vec![read_rpc(&mut stdout)];
    write_rpc(&mut stdin, &handshake()[1]);
    for (name, arguments, _) in &specs {
        write_rpc(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":name,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        );
        responses.push(read_rpc(&mut stdout));
    }
    // Each dependent mutation is awaited before the next request is written.
    write_rpc(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"followup_task"}}),
    );
    drop(stdin);
    wait_success(&mut child);
    let sent = server.join().unwrap();
    assert_eq!(sent.len(), specs.len());
    assert_eq!(responses.len(), specs.len() + 1);
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
    let (mut child, mut stdin, mut stdout) = spawn_interactive_mcp(root.path());
    write_rpc(&mut stdin, &handshake()[0]);
    let mut responses = vec![read_rpc(&mut stdout)];
    write_rpc(&mut stdin, &handshake()[1]);
    for index in 0..receipts.len() {
        write_rpc(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":index + 2,"method":"tools/call",
                "params":{"name":"interrupt_agent","arguments":{"target":"peer-uuid"}}}),
        );
        responses.push(read_rpc(&mut stdout));
    }
    drop(stdin);
    wait_success(&mut child);
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
    let send = responses
        .iter()
        .find(|response| response["id"] == 2)
        .unwrap();
    let list = responses
        .iter()
        .find(|response| response["id"] == 3)
        .unwrap();
    let unknown = responses
        .iter()
        .find(|response| response["id"] == 4)
        .unwrap();
    for response in [send, list] {
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "missing_managed_sender",
            "{}",
            response_shape(response)
        );
    }
    assert_eq!(
        unknown["error"]["code"],
        -32602,
        "{}",
        response_shape(unknown)
    );
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
    let (mut child, mut stdin, mut stdout) = spawn_interactive_mcp(root.path());
    write_rpc(&mut stdin, &handshake()[0]);
    let _ = read_rpc(&mut stdout);
    write_rpc(&mut stdin, &handshake()[1]);
    let mut responses = Vec::with_capacity(4);
    for request in [&first, &first, &second, &second] {
        write_rpc(&mut stdin, request);
        responses.push(read_rpc(&mut stdout));
    }
    drop(stdin);
    wait_success(&mut child);
    assert_eq!(server.join().unwrap().len(), 2);
    assert_eq!(
        responses[0]["result"]["structuredContent"]["error"]["code"],
        "ambiguous_target"
    );
    assert_eq!(responses[1]["error"]["code"], -32600);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["error"]["code"],
        "invalid_receipt"
    );
    assert_eq!(responses[3]["error"]["code"], -32600);
}
