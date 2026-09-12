use super::*;
use messaging::Backend;
use wardian_core::agent_messaging::AgentMessagingRequest;

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

#[test]
fn six_tools_dispatch_typed_requests_and_literal_content() {
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
            "receive_messages",
            json!({"cursor":"cursor-1","ack_cursor":"ack-1","limit":100,"timeout_ms":60000}),
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
            "receive_messages" => {
                json!({"operation":name,"messages":[],"next_cursor":"c2","ack_cursor":"a2","has_more":false,"timed_out":true})
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
    assert_eq!(backend.calls.len(), 6);
    for index in [0, 1, 3] {
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
        backend.calls[5],
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
        ("receive_messages", json!({"limit":101})),
        ("receive_messages", json!({"limit":0})),
        ("receive_messages", json!({"timeout_ms":60001})),
        ("receive_messages", json!({"timeout_ms":-1})),
        ("receive_messages", json!({"cursor":25})),
        ("receive_messages", json!({"cursor":""})),
        ("receive_messages", json!({"cursor":null})),
        ("receive_messages", json!({"target":"foreign"})),
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
fn lifecycle_lists_exactly_six_tools_with_honest_annotations() {
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
        assert_eq!(tools.len(), 6);
        for (tool, name) in tools.iter().zip(definitions::NAMES) {
            assert_eq!(tool["name"], name);
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert_eq!(tool["annotations"]["readOnlyHint"], name == "list_agents");
        }
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
