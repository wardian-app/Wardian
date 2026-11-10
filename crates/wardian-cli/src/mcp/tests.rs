use super::*;
use messaging::Backend;
use std::io::{self, BufRead, Read, Write};
use wardian_core::agent_messaging::AgentMessagingRequest;

trait SynchronousSessionHandle {
    fn handle<B: Backend>(&mut self, value: Value, backend: &mut B) -> Option<Value>;
}

impl SynchronousSessionHandle for Session {
    fn handle<B: Backend>(&mut self, value: Value, backend: &mut B) -> Option<Value> {
        match self.admit(value) {
            Admission::Immediate(response) => response,
            Admission::Call(call) => Some(call.execute(backend)),
        }
    }
}

fn serve<R: BufRead, W: Write, B: Backend>(
    mut input: R,
    mut output: W,
    backend: &mut B,
) -> io::Result<()> {
    let mut session = Session::default();
    loop {
        let mut line = Vec::new();
        let mut limited = input.by_ref().take((MAX_LINE_BYTES + 1) as u64);
        let length = limited.read_until(b'\n', &mut line)?;
        if length == 0 {
            return Ok(());
        }
        if length > MAX_LINE_BYTES {
            write_response(&mut output, error(Value::Null, -32600, "Message too large"))?;
            return Ok(());
        }
        if line.last() != Some(&b'\n') {
            write_response(&mut output, error(Value::Null, -32700, "Truncated message"))?;
            return Ok(());
        }
        let response = match serde_json::from_slice(&line) {
            Ok(value) => session.handle(value, backend),
            Err(_) => Some(error(Value::Null, -32700, "Parse error")),
        };
        if let Some(response) = response {
            write_response(&mut output, response)?;
        }
    }
}

struct Fake {
    managed: bool,
    calls: Vec<AgentMessagingRequest>,
    fail: bool,
    response: Value,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            managed: true,
            calls: vec![],
            fail: false,
            response: json!({"operation":"send_message","interaction_id":"real-id","delivery_state":"pending","duplicate":false}),
        }
    }
}

impl Backend for Fake {
    fn require_sender(&self) -> io::Result<()> {
        if self.managed {
            Ok(())
        } else {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }
    fn invoke(&mut self, request: AgentMessagingRequest) -> io::Result<Value> {
        self.calls.push(request);
        if self.fail {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "private payload must not leak",
            ))
        } else {
            Ok(self.response.clone())
        }
    }
}

fn initialize(version: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}})
}

fn ready() -> Session {
    let mut session = Session::default();
    let mut backend = Fake::default();
    session.handle(initialize(VERSION), &mut backend);
    session.handle(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        &mut backend,
    );
    session
}

fn call(id: Value, name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
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

#[derive(Clone)]
struct MissingSenderBackend {
    sender_checks: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    invocations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Backend for MissingSenderBackend {
    fn require_sender(&self) -> io::Result<()> {
        self.sender_checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(io::ErrorKind::PermissionDenied.into())
    }

    fn invoke(&mut self, _request: AgentMessagingRequest) -> io::Result<Value> {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

#[test]
fn five_tools_dispatch_typed_requests_and_literal_content() {
    let literal = "  中文 λ😀\r\nsecond\n\"quoted\" C:\\folder\\file\t\0\u{1b}  ";
    let mut session = ready();
    let mut backend = Fake::default();
    for (name, args) in [
        (
            "send_message",
            json!({"target":"Exact Peer","message":literal}),
        ),
        (
            "followup_task",
            json!({"target":"peer-uuid","message":literal}),
        ),
        (
            "reply",
            json!({"request_id":"task-1","status":"blocked","message":literal}),
        ),
        ("interrupt_agent", json!({"target":"peer-uuid"})),
        ("list_agents", json!({})),
    ] {
        backend.response = match name {
            "send_message" => {
                json!({"operation":name,"interaction_id":"info-1","delivery_state":"pending","duplicate":false})
            }
            "followup_task" => {
                json!({"operation":name,"request_id":"task-1","delivery_owner":"unclaimed","delivery_state":"pending","duplicate":false})
            }
            "reply" => {
                json!({"operation":name,"request_id":"task-1","interaction_id":"reply-1","delivery_state":"pending","duplicate":false})
            }
            "interrupt_agent" => {
                json!({"error":{"code":"unsupported_capability","message":"Unsupported"}})
            }
            "list_agents" => json!({"operation":name,"agents":[]}),
            _ => unreachable!(),
        };
        let result = session
            .handle(call(json!(name), name, args), &mut backend)
            .unwrap();
        assert_eq!(result["id"], name);
        assert_eq!(result["result"]["structuredContent"], backend.response);
        assert_eq!(
            serde_json::from_str::<Value>(result["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap(),
            backend.response
        );
    }
    assert_eq!(backend.calls.len(), 5);
    for index in [0, 1, 2] {
        assert_eq!(
            serde_json::to_value(&backend.calls[index]).unwrap()["message"],
            literal
        );
    }
    assert_eq!(
        serde_json::to_value(&backend.calls[0]).unwrap()["target"],
        "Exact Peer"
    );
    assert_eq!(
        serde_json::to_value(&backend.calls[1]).unwrap()["target"],
        "peer-uuid"
    );
    assert!(matches!(
        backend.calls[4],
        AgentMessagingRequest::ListAgents
    ));
}

#[test]
fn invalid_arguments_and_missing_sender_never_reach_control() {
    for (name, args) in [
        (
            "send_message",
            json!({"target":"Peer","message":"x","origin":"forged"}),
        ),
        (
            "send_message",
            json!({"target":"Peer","message":"x","idempotency_key":"forged"}),
        ),
        (
            "send_message",
            json!({"target":"Peer","message":"x","interrupt":true}),
        ),
        ("followup_task", json!({"target":"Peer","message":false})),
        ("followup_task", json!({"target":"Peer","message":" \r\n"})),
        (
            "reply",
            json!({"request_id":"x","status":"completed","message":"x"}),
        ),
        (
            "reply",
            json!({"request_id":"x","status":"done","message":"x","target":"foreign"}),
        ),
        ("interrupt_agent", json!({"target":"Peer","message":"x"})),
        ("list_agents", json!({"scope":"all"})),
    ] {
        let mut backend = Fake::default();
        let result = messaging::call(name, args, "key", &mut backend);
        assert_eq!(
            result["structuredContent"]["error"]["code"], "invalid_arguments",
            "{name}"
        );
        assert!(backend.calls.is_empty());
    }
    for target in [
        "all",
        "ALL",
        "class:Test",
        "*",
        "broadcast",
        " Peer",
        "",
        "\n",
    ] {
        let mut backend = Fake::default();
        assert_eq!(
            messaging::call(
                "send_message",
                json!({"target":target,"message":"x"}),
                "key",
                &mut backend
            )["isError"],
            true
        );
        assert!(backend.calls.is_empty());
    }
    for name in definitions::NAMES {
        let args = match name {
            "send_message" | "followup_task" => json!({"target":"Peer","message":"x"}),
            "interrupt_agent" => json!({"target":"Peer"}),
            "reply" => json!({"request_id":"task","status":"done","message":"x"}),
            _ => json!({}),
        };
        let mut backend = Fake {
            managed: false,
            ..Default::default()
        };
        assert_eq!(
            messaging::call(name, args, "key", &mut backend)["structuredContent"]["error"]["code"],
            "missing_managed_sender"
        );
        assert!(backend.calls.is_empty());
    }
}

#[test]
fn retired_receive_tools_are_unknown_and_never_reach_control() {
    for name in ["receive_messages", "wait_agent"] {
        let mut backend = Fake::default();
        let result = messaging::call(name, json!({}), "key", &mut backend);
        assert_eq!(
            result["structuredContent"]["error"]["code"],
            "invalid_arguments"
        );
        assert!(backend.calls.is_empty(), "{name} reached the backend");
    }
}

#[test]
fn admission_keys_and_call_ids_never_replay_uncertain_operations() {
    let mut backend = Fake::default();
    let mut first = ready();
    let mut second = ready();
    let args = json!({"target":"Peer","message":"literal"});
    first.handle(call(json!(7), "send_message", args.clone()), &mut backend);
    second.handle(call(json!(7), "send_message", args.clone()), &mut backend);
    first.handle(call(json!("7"), "send_message", args.clone()), &mut backend);
    let keys: HashSet<String> = backend
        .calls
        .iter()
        .map(|request| {
            serde_json::to_value(request).unwrap()["idempotency_key"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(keys.len(), 3);
    for args in [args, json!({"target":"Peer","message":"changed"})] {
        assert_eq!(
            first
                .handle(call(json!(7), "send_message", args), &mut backend)
                .unwrap()["error"]["code"],
            -32600
        );
    }
    assert_eq!(backend.calls.len(), 3);
    backend.fail = true;
    let request = call(
        json!(8),
        "followup_task",
        json!({"target":"Peer","message":"private payload"}),
    );
    let result = first.handle(request.clone(), &mut backend).unwrap();
    assert_eq!(result["result"]["isError"], true);
    assert!(!result.to_string().contains("private payload"));
    first.handle(request, &mut backend);
    assert_eq!(backend.calls.len(), 4);
}

#[test]
fn runtime_rejection_and_malformed_receipt_are_not_success() {
    for response in [
        json!({"error":{"code":"ambiguous_target","message":"Use a UUID"}}),
        json!({"ok":false}),
        json!([]),
        json!({}),
        json!({"operation":"list_agents","agents":[]}),
        json!({"operation":"send_message","interaction_id":"","delivery_state":"pending","duplicate":false}),
    ] {
        let mut backend = Fake {
            response,
            ..Default::default()
        };
        assert_eq!(
            messaging::call(
                "send_message",
                json!({"target":"Peer","message":"x"}),
                "key",
                &mut backend
            )["isError"],
            true
        );
        assert_eq!(backend.calls.len(), 1);
    }
}

#[test]
fn admission_key_limit_includes_namespace_and_json_id_encoding() {
    let mut session = ready();
    let mut backend = Fake::default();
    let max_string_len = 256 - session.admission_namespace.len() - 3;
    let args = json!({"target":"Peer","message":"x"});
    session.handle(
        call(
            json!("x".repeat(max_string_len)),
            "send_message",
            args.clone(),
        ),
        &mut backend,
    );
    assert_eq!(backend.calls.len(), 1);
    assert_eq!(
        serde_json::to_value(&backend.calls[0]).unwrap()["idempotency_key"]
            .as_str()
            .unwrap()
            .len(),
        256
    );
    let result = session
        .handle(
            call(json!("x".repeat(max_string_len + 1)), "send_message", args),
            &mut backend,
        )
        .unwrap();
    assert_eq!(result["error"]["code"], -32600);
    assert_eq!(backend.calls.len(), 1);
}

#[test]
fn lifecycle_lists_exactly_five_tools_with_honest_annotations() {
    for version in [VERSION, "2025-06-18", "unknown"] {
        let mut session = Session::default();
        let mut backend = Fake::default();
        let list = json!({"jsonrpc":"2.0","id":"list","method":"tools/list"});
        assert_eq!(
            session.handle(list.clone(), &mut backend).unwrap()["error"]["code"],
            -32000
        );
        let result = session.handle(initialize(version), &mut backend).unwrap();
        assert_eq!(
            result["result"]["protocolVersion"],
            if version == "2025-06-18" {
                version
            } else {
                VERSION
            }
        );
        session.handle(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &mut backend,
        );
        let listed = session.handle(list, &mut backend).unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
        for (tool, name) in tools.iter().zip(definitions::NAMES) {
            assert_eq!(tool["name"], name);
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert_eq!(tool["annotations"]["readOnlyHint"], name == "list_agents");
        }
        assert!(!tools.iter().any(|tool| {
            matches!(
                tool["name"].as_str(),
                Some("receive_messages" | "wait_agent")
            )
        }));
        assert!(session
            .handle(initialize(version), &mut backend)
            .unwrap()
            .get("error")
            .is_some());
        assert!(backend.calls.is_empty());
    }
}

#[test]
fn protocol_errors_notifications_and_cancellation_never_dispatch() {
    let mut session = ready();
    let mut backend = Fake::default();
    for request in [
        json!([]),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        call(json!(2), "send_input", json!({})),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"send_message","arguments":{},"task":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":[]}),
        json!({"jsonrpc":"2.0","id":5,"method":"not-a-method"}),
    ] {
        assert!(session
            .handle(request, &mut backend)
            .unwrap()
            .get("error")
            .is_some());
    }
    for notification in [
        json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"send_message","arguments":{"target":"Peer","message":"x"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}),
        json!({"jsonrpc":"2.0","id":42,"result":{}}),
    ] {
        assert!(session.handle(notification, &mut backend).is_none());
    }
    assert!(backend.calls.is_empty());
}

#[test]
fn framing_handles_parse_error_ping_eof_and_bounds() {
    let mut backend = Fake::default();
    let mut output = Vec::new();
    serve(
        &b"{bad}\n{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n"[..],
        &mut output,
        &mut backend,
    )
    .unwrap();
    let values: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(values[0]["error"]["code"], -32700);
    assert_eq!(values[1], json!({"jsonrpc":"2.0","id":8,"result":{}}));
    for input in [
        vec![b'x'; MAX_LINE_BYTES + 8],
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}".to_vec(),
        vec![0xff, b'\n'],
    ] {
        let mut output = Vec::new();
        serve(input.as_slice(), &mut output, &mut backend).unwrap();
        assert!(serde_json::from_slice::<Value>(&output)
            .unwrap()
            .get("error")
            .is_some());
    }
    assert!(backend.calls.is_empty());
}

#[derive(Clone)]
struct ConcurrentBackend {
    calls: std::sync::Arc<std::sync::Mutex<Vec<AgentMessagingRequest>>>,
    started: std::sync::mpsc::Sender<AgentMessagingRequest>,
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl Backend for ConcurrentBackend {
    fn require_sender(&self) -> io::Result<()> {
        Ok(())
    }

    fn invoke(&mut self, request: AgentMessagingRequest) -> io::Result<Value> {
        self.calls.lock().unwrap().push(request.clone());
        let _ = self.started.send(request.clone());
        if matches!(
            &request,
            AgentMessagingRequest::FollowupTask { message, .. } if message == "hold"
        ) {
            let (released, wake) = &*self.gate;
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
        }
        Ok(match request {
            AgentMessagingRequest::SendMessage { .. } => json!({
                "operation":"send_message","interaction_id":"interaction-1",
                "delivery_state":"pending","duplicate":false
            }),
            AgentMessagingRequest::FollowupTask { .. } => json!({
                "operation":"followup_task","request_id":"task-1",
                "delivery_owner":"unclaimed","delivery_state":"pending","duplicate":false
            }),
            AgentMessagingRequest::ListAgents => json!({"operation":"list_agents","agents":[]}),
            AgentMessagingRequest::Reply { request_id, .. } => json!({
                "operation":"reply","request_id":request_id,"interaction_id":"reply-1",
                "delivery_state":"pending","duplicate":false
            }),
            AgentMessagingRequest::InterruptAgent { .. } => json!({
                "operation":"interrupt_agent","target":{"uuid":"peer","name":"Peer"},
                "outcome":"unsupported","interrupted":false
            }),
            _ => unreachable!("the MCP surface has five tools"),
        })
    }
}

#[derive(Clone, Default)]
struct CaptureWriter(std::sync::Arc<(std::sync::Mutex<Vec<u8>>, std::sync::Condvar)>);

impl Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (output, wake) = &*self.0;
        output.lock().unwrap().extend_from_slice(bytes);
        wake.notify_all();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CaptureWriter {
    fn wait_for_id(&self, id: Value, timeout: Duration) -> bool {
        self.wait_for_response(timeout, |response| response["id"] == id)
    }

    fn wait_for_response(&self, timeout: Duration, matches: impl Fn(&Value) -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let (output, wake) = &*self.0;
        let mut output = output.lock().unwrap();
        loop {
            if output
                .split(|byte| *byte == b'\n')
                .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
                .any(|response| matches(&response))
            {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = wake.wait_timeout(output, deadline - now).unwrap();
            output = next;
        }
    }

    fn values(&self) -> Vec<Value> {
        self.0
             .0
            .lock()
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .collect()
    }
}

fn input_frames(values: impl IntoIterator<Item = Value>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in values {
        serde_json::to_writer(&mut bytes, &value).unwrap();
        bytes.push(b'\n');
    }
    bytes
}

struct ChannelReader {
    frames: std::sync::mpsc::Receiver<Vec<u8>>,
    buffered: std::collections::VecDeque<u8>,
}

struct FailOnSecondResponse {
    completed_responses: usize,
    line: Vec<u8>,
}

struct BlockingWriter {
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    started: std::sync::mpsc::Sender<()>,
    finished: std::sync::mpsc::Sender<()>,
}

struct GatedCaptureWriter {
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    started: std::sync::mpsc::Sender<()>,
    blocked: bool,
}

impl Write for FailOnSecondResponse {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes == b"\n" && self.completed_responses == 1 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test output closed",
            ));
        }
        self.line.extend_from_slice(bytes);
        if bytes == b"\n" {
            self.completed_responses += 1;
            self.line.clear();
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (released, wake) = &*self.gate;
        let mut released = released.lock().unwrap();
        if !*released {
            let _ = self.started.send(());
            while !*released {
                released = wake.wait(released).unwrap();
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for GatedCaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.blocked {
            self.blocked = true;
            let (released, wake) = &*self.gate;
            let mut released = released.lock().unwrap();
            let _ = self.started.send(());
            while !*released {
                released = wake.wait(released).unwrap();
            }
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BlockingWriter {
    fn drop(&mut self) {
        let _ = self.finished.send(());
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.buffered.is_empty() {
            match self.frames.recv() {
                Ok(frame) => self.buffered.extend(frame),
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.buffered.len());
        for byte in &mut buffer[..count] {
            *byte = self.buffered.pop_front().unwrap();
        }
        Ok(count)
    }
}

fn send_frame(sender: &std::sync::mpsc::Sender<Vec<u8>>, value: Value) {
    sender.send(input_frames([value])).unwrap();
}

fn slow_call(id: i64, message: &str) -> Value {
    call(
        json!(id),
        "followup_task",
        json!({"target":"Peer","message":message}),
    )
}

struct ConcurrentBackendParts {
    calls: std::sync::Arc<std::sync::Mutex<Vec<AgentMessagingRequest>>>,
    started_rx: std::sync::mpsc::Receiver<AgentMessagingRequest>,
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    started: std::sync::mpsc::Sender<AgentMessagingRequest>,
}

fn concurrent_backend_parts() -> ConcurrentBackendParts {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (started, started_rx) = std::sync::mpsc::channel();
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    ConcurrentBackendParts {
        calls,
        started_rx,
        gate,
        started,
    }
}

fn release_gate(gate: &std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>) {
    let (released, wake) = &**gate;
    *released.lock().unwrap() = true;
    wake.notify_all();
}

struct ReleaseGateOnDrop(std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);

impl Drop for ReleaseGateOnDrop {
    fn drop(&mut self) {
        release_gate(&self.0);
    }
}

#[test]
fn a_slow_admitted_task_does_not_block_independent_calls_or_get_cancelled() {
    let ConcurrentBackendParts {
        calls,
        started_rx,
        gate,
        started,
    } = concurrent_backend_parts();
    let _release_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&gate));
    let backend_calls = std::sync::Arc::clone(&calls);
    let backend_gate = std::sync::Arc::clone(&gate);
    let output = CaptureWriter::default();
    let server_output = output.clone();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        serve_concurrent(
            ChannelReader {
                frames: input_rx,
                buffered: std::collections::VecDeque::new(),
            },
            server_output,
            move || ConcurrentBackend {
                calls: std::sync::Arc::clone(&backend_calls),
                started: started.clone(),
                gate: std::sync::Arc::clone(&backend_gate),
            },
        )
    });

    send_frame(&input_tx, initialize(VERSION));
    assert!(output.wait_for_id(json!(1), Duration::from_secs(3)));
    send_frame(
        &input_tx,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    send_frame(&input_tx, slow_call(2, "hold"));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut slow_started = false;
    while std::time::Instant::now() < deadline {
        if let Ok(request) = started_rx.recv_timeout(Duration::from_millis(20)) {
            if matches!(request, AgentMessagingRequest::FollowupTask { message, .. } if message == "hold")
            {
                slow_started = true;
                break;
            }
        }
    }
    assert!(
        slow_started,
        "the first mutation should be executing and held"
    );
    send_frame(
        &input_tx,
        call(
            json!(3),
            "send_message",
            json!({"target":"OtherPeer","message":"independent"}),
        ),
    );
    send_frame(
        &input_tx,
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}),
    );
    send_frame(&input_tx, json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
    drop(input_tx);

    assert!(output.wait_for_id(json!(3), Duration::from_secs(3)));
    let independent_response = output
        .values()
        .into_iter()
        .find(|response| response["id"] == 3)
        .unwrap();
    assert_eq!(
        independent_response["result"]["structuredContent"]["operation"], "send_message",
        "independent tool returned {independent_response}"
    );
    let independent_request = started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(
        independent_request,
        AgentMessagingRequest::SendMessage { message, .. } if message == "independent"
    ));
    assert!(output.wait_for_id(json!(4), Duration::from_secs(3)));
    assert!(!output.wait_for_id(json!(2), Duration::from_millis(30)));
    assert!(
        !server.is_finished(),
        "EOF must wait for admitted work to finish"
    );

    release_gate(&gate);
    assert!(output.wait_for_id(json!(2), Duration::from_secs(3)));
    assert!(server.join().unwrap().is_ok());
    let responses = output.values();
    assert_eq!(
        responses
            .iter()
            .filter(|response| response["id"] == 2)
            .count(),
        1
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| response["id"] == 3)
            .count(),
        1
    );
    assert_eq!(
        calls.lock().unwrap().len(),
        2,
        "cancel notification must not dispatch or revoke work"
    );
}

#[test]
fn full_worker_queue_rejects_before_execution() {
    let ConcurrentBackendParts {
        calls,
        started_rx,
        gate,
        started,
    } = concurrent_backend_parts();
    let _release_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&gate));
    let backend_calls = std::sync::Arc::clone(&calls);
    let backend_gate = std::sync::Arc::clone(&gate);
    let output = CaptureWriter::default();
    let server_output = output.clone();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        serve_concurrent(
            ChannelReader {
                frames: input_rx,
                buffered: std::collections::VecDeque::new(),
            },
            server_output,
            move || ConcurrentBackend {
                calls: std::sync::Arc::clone(&backend_calls),
                started: started.clone(),
                gate: std::sync::Arc::clone(&backend_gate),
            },
        )
    });

    send_frame(&input_tx, initialize(VERSION));
    assert!(output.wait_for_id(json!(1), Duration::from_secs(3)));
    send_frame(
        &input_tx,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    for id in 10..14 {
        send_frame(&input_tx, slow_call(id, "hold"));
        let request = started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            matches!(request, AgentMessagingRequest::FollowupTask { message, .. } if message == "hold")
        );
    }
    send_frame(&input_tx, slow_call(14, "queued"));
    send_frame(&input_tx, slow_call(15, "overflow"));
    assert!(output.wait_for_id(json!(15), Duration::from_secs(3)));
    let overflow = output
        .values()
        .into_iter()
        .find(|response| response["id"] == 15)
        .unwrap();
    assert_eq!(overflow["error"]["code"], -32002);
    assert!(overflow["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no operation was executed"));
    assert_eq!(
        calls.lock().unwrap().len(),
        4,
        "rejected work must not reach the backend"
    );

    send_frame(&input_tx, slow_call(15, "replay-with-same-id"));
    assert!(
        output.wait_for_response(Duration::from_secs(3), |response| {
            response["id"] == 15 && response["error"]["code"] == -32600
        }),
        "duplicate request ID rejection must be written before releasing workers"
    );
    release_gate(&gate);
    drop(input_tx);
    let server_result = server.join().expect("MCP server thread panicked");
    assert!(
        server_result.is_ok(),
        "MCP server failed: {:?}",
        server_result.err()
    );
    let overflow_responses: Vec<_> = output
        .values()
        .into_iter()
        .filter(|response| response["id"] == 15)
        .collect();
    assert_eq!(overflow_responses.len(), 2);
    assert!(overflow_responses
        .iter()
        .any(|response| response["error"]["code"] == -32002));
    assert!(overflow_responses
        .iter()
        .any(|response| response["error"]["code"] == -32600));
    let executed: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| match request {
            AgentMessagingRequest::FollowupTask { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(executed.len(), 5);
    assert_eq!(
        executed.iter().filter(|message| *message == "hold").count(),
        4
    );
    assert!(executed.contains(&"queued".to_string()));
    assert!(!executed.contains(&"overflow".to_string()));
}

#[test]
fn late_worker_output_saturation_after_eof_is_reported() {
    let ConcurrentBackendParts {
        calls,
        started_rx,
        gate: worker_gate,
        started,
    } = concurrent_backend_parts();
    let _release_workers_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&worker_gate));
    let output_gate =
        std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let _release_output_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&output_gate));
    let output_bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let backend_calls = std::sync::Arc::clone(&calls);
    let backend_gate = std::sync::Arc::clone(&worker_gate);
    let writer_gate = std::sync::Arc::clone(&output_gate);
    let writer_bytes = std::sync::Arc::clone(&output_bytes);
    let (writer_started, writer_started_rx) = std::sync::mpsc::channel();
    let (input_eof, input_eof_rx) = std::sync::mpsc::channel();
    let (workers_joined, workers_joined_rx) = std::sync::mpsc::channel();
    let input = input_frames([
        initialize(VERSION),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        slow_call(10, "hold"),
        slow_call(11, "hold"),
        slow_call(12, "hold"),
        slow_call(13, "hold"),
        slow_call(14, "queued"),
        slow_call(15, "overflow"),
    ]);
    let server = std::thread::spawn(move || {
        serve_concurrent_with_hooks(
            std::io::Cursor::new(input),
            GatedCaptureWriter {
                bytes: writer_bytes,
                gate: writer_gate,
                started: writer_started,
                blocked: false,
            },
            move || ConcurrentBackend {
                calls: std::sync::Arc::clone(&backend_calls),
                started: started.clone(),
                gate: std::sync::Arc::clone(&backend_gate),
            },
            || {},
            move || {
                let _ = input_eof.send(());
            },
            move || {
                let _ = workers_joined.send(());
            },
        )
    });

    assert!(
        writer_started_rx
            .recv_timeout(Duration::from_secs(3))
            .is_ok(),
        "output writer must be held before worker responses"
    );
    assert!(
        input_eof_rx.recv_timeout(Duration::from_secs(3)).is_ok(),
        "server must consume EOF while workers remain gated"
    );
    for _ in 0..TOOL_WORKERS {
        let request = started_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("each worker must reach its held backend call");
        assert!(matches!(
            request,
            AgentMessagingRequest::FollowupTask { message, .. } if message == "hold"
        ));
    }

    release_gate(&worker_gate);
    assert!(
        workers_joined_rx
            .recv_timeout(Duration::from_secs(3))
            .is_ok(),
        "all workers must join after their bounded output attempts"
    );
    release_gate(&output_gate);

    let failure = server
        .join()
        .expect("MCP server thread panicked")
        .expect_err("late worker output saturation must fail the server");
    assert_eq!(failure.error.kind(), io::ErrorKind::BrokenPipe);
    assert!(failure.reader_joined);
    assert!(failure.writer_joined);

    let responses: Vec<Value> = output_bytes
        .lock()
        .unwrap()
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect();
    assert_eq!(responses.len(), OUTPUT_QUEUE + 1);
    assert_eq!(
        responses
            .iter()
            .filter(|response| { (10..=14).contains(&response["id"].as_i64().unwrap_or_default()) })
            .count(),
        TOOL_WORKERS,
        "one of the five admitted worker responses must be lost to saturation"
    );
    assert_eq!(
        responses
            .iter()
            .filter(|response| response["id"] == 15 && response["error"]["code"] == -32002)
            .count(),
        1,
        "the work-queue rejection must occupy the remaining output slot"
    );
}

#[test]
fn idle_workers_do_not_report_queue_full_before_dequeue() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let output = CaptureWriter::default();
    let server_output = output.clone();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let (worker_ready_tx, worker_ready_rx) = std::sync::mpsc::channel();
    let worker_gate =
        std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let _release_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&worker_gate));
    let sender_checks = std::sync::Arc::new(AtomicUsize::new(0));
    let invocations = std::sync::Arc::new(AtomicUsize::new(0));
    let backend = MissingSenderBackend {
        sender_checks: std::sync::Arc::clone(&sender_checks),
        invocations: std::sync::Arc::clone(&invocations),
    };
    let start_gate = std::sync::Arc::clone(&worker_gate);
    let server = std::thread::spawn(move || {
        serve_concurrent_with_worker_start(
            ChannelReader {
                frames: input_rx,
                buffered: std::collections::VecDeque::new(),
            },
            server_output,
            move || backend.clone(),
            move || {
                let _ = worker_ready_tx.send(());
                let (released, wake) = &*start_gate;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = wake.wait(released).unwrap();
                }
            },
        )
    });

    for _ in 0..TOOL_WORKERS {
        worker_ready_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("each worker must reach the dequeue gate");
    }
    send_frame(&input_tx, initialize(VERSION));
    assert!(output.wait_for_id(json!(1), Duration::from_secs(3)));
    send_frame(
        &input_tx,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    for id in 10..15 {
        send_frame(
            &input_tx,
            call(
                json!(id),
                "send_message",
                json!({"target":"Peer","message":"fixture"}),
            ),
        );
    }
    send_frame(
        &input_tx,
        call(
            json!(15),
            "send_message",
            json!({"target":"Peer","message":"overflow"}),
        ),
    );
    send_frame(&input_tx, json!({"jsonrpc":"2.0","id":16,"method":"ping"}));
    assert!(output.wait_for_id(json!(16), Duration::from_secs(3)));

    let before_release: Vec<_> = output
        .values()
        .into_iter()
        .filter(|response| (10..=15).contains(&response["id"].as_i64().unwrap_or_default()))
        .collect();
    let before_release_shapes: Vec<_> = before_release.iter().map(response_shape).collect();
    release_gate(&worker_gate);
    drop(input_tx);
    assert!(server.join().unwrap().is_ok());

    assert_eq!(
        before_release.len(),
        1,
        "only the sixth request may be rejected before workers dequeue: {before_release_shapes:?}"
    );
    assert_eq!(before_release[0]["id"], 15);
    assert_eq!(before_release[0]["error"]["code"], -32002);
    let responses = output.values();
    for id in 10..15 {
        let response = responses
            .iter()
            .find(|response| response["id"] == id)
            .unwrap();
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "missing_managed_sender",
            "{}",
            response_shape(response)
        );
    }
    assert_eq!(
        sender_checks.load(Ordering::Relaxed),
        TOOL_WORKERS + TOOL_QUEUE
    );
    assert_eq!(invocations.load(Ordering::Relaxed), 0);
}

#[test]
fn sequentially_awaited_mutations_keep_their_wire_order() {
    let ConcurrentBackendParts {
        calls,
        started_rx: _started_rx,
        gate,
        started,
    } = concurrent_backend_parts();
    let backend_calls = std::sync::Arc::clone(&calls);
    let backend_gate = std::sync::Arc::clone(&gate);
    let output = CaptureWriter::default();
    let server_output = output.clone();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        serve_concurrent(
            ChannelReader {
                frames: input_rx,
                buffered: std::collections::VecDeque::new(),
            },
            server_output,
            move || ConcurrentBackend {
                calls: std::sync::Arc::clone(&backend_calls),
                started: started.clone(),
                gate: std::sync::Arc::clone(&backend_gate),
            },
        )
    });

    send_frame(&input_tx, initialize(VERSION));
    assert!(output.wait_for_id(json!(1), Duration::from_secs(3)));
    send_frame(
        &input_tx,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    send_frame(&input_tx, slow_call(20, "first"));
    assert!(output.wait_for_id(json!(20), Duration::from_secs(3)));
    send_frame(&input_tx, slow_call(21, "second"));
    assert!(output.wait_for_id(json!(21), Duration::from_secs(3)));
    drop(input_tx);
    assert!(server.join().unwrap().is_ok());
    let messages: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| match request {
            AgentMessagingRequest::FollowupTask { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["first", "second"]);
}

#[test]
fn output_failure_joins_already_admitted_work() {
    let ConcurrentBackendParts {
        calls,
        started_rx: _started_rx,
        gate,
        started,
    } = concurrent_backend_parts();
    let _release_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&gate));
    let backend_calls = std::sync::Arc::clone(&calls);
    let backend_gate = std::sync::Arc::clone(&gate);
    let frames = input_frames([
        initialize(VERSION),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        slow_call(30, "hold"),
        json!({"jsonrpc":"2.0","id":31,"method":"ping"}),
    ]);
    let server = std::thread::spawn(move || {
        serve_concurrent(
            std::io::Cursor::new(frames),
            FailOnSecondResponse {
                completed_responses: 0,
                line: Vec::new(),
            },
            move || ConcurrentBackend {
                calls: std::sync::Arc::clone(&backend_calls),
                started: started.clone(),
                gate: std::sync::Arc::clone(&backend_gate),
            },
        )
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && calls.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "tool call must be admitted before output failure"
    );
    release_gate(&gate);
    let failure = server.join().unwrap().unwrap_err();
    assert_eq!(failure.error.kind(), io::ErrorKind::BrokenPipe);
    assert!(
        failure.reader_joined,
        "finite input reader should be joined"
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn a_stalled_output_writer_is_detached_only_after_the_drain_deadline() {
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let _release_on_failure = ReleaseGateOnDrop(std::sync::Arc::clone(&gate));
    let (started, started_rx) = std::sync::mpsc::channel();
    let (finished, finished_rx) = std::sync::mpsc::channel();
    let server_gate = std::sync::Arc::clone(&gate);
    let requests =
        (1..=OUTPUT_QUEUE + 3).map(|id| json!({"jsonrpc":"2.0","id":id,"method":"ping"}));
    let input = input_frames(requests);
    let server = std::thread::spawn(move || {
        serve_concurrent(
            std::io::Cursor::new(input),
            BlockingWriter {
                gate: server_gate,
                started,
                finished,
            },
            Fake::default,
        )
    });

    assert!(started_rx.recv_timeout(Duration::from_secs(3)).is_ok());
    let before = std::time::Instant::now();
    let failure = server.join().unwrap().unwrap_err();
    assert_eq!(failure.error.kind(), io::ErrorKind::WouldBlock);
    assert!(failure.reader_joined);
    assert!(!failure.writer_joined);
    assert!(before.elapsed() >= Duration::from_secs(4));
    assert!(before.elapsed() < Duration::from_secs(8));

    release_gate(&gate);
    assert!(finished_rx.recv_timeout(Duration::from_secs(3)).is_ok());
}
