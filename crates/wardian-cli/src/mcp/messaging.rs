use crate::live;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use wardian_core::agent_messaging::{AgentMessagingRequest, AgentMessagingResponse};

pub(super) trait Backend {
    fn require_sender(&self) -> io::Result<()>;
    fn invoke(&mut self, request: AgentMessagingRequest) -> io::Result<Value>;
}

pub(super) struct Live;

impl Backend for Live {
    fn require_sender(&self) -> io::Result<()> {
        live::require_current_message_origin().map(|_| ())
    }

    fn invoke(&mut self, request: AgentMessagingRequest) -> io::Result<Value> {
        live::messaging::request(request)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageArgs {
    target: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetArgs {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyArgs {
    request_id: String,
    status: wardian_core::control::ReplyStatus,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveArgs {
    cursor: Option<String>,
    ack_cursor: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    timeout_ms: u64,
}

fn default_limit() -> u32 {
    100
}

pub(super) fn call(
    name: &str,
    arguments: Value,
    idempotency_key: &str,
    backend: &mut impl Backend,
) -> Value {
    let request = match parse(name, arguments, idempotency_key) {
        Ok(request) => request,
        Err(error) => return failure("invalid_arguments", &error),
    };
    if backend.require_sender().is_err() {
        return failure(
            "missing_managed_sender",
            "A managed Wardian sender is required. No control request was sent.",
        );
    }
    let reply_request_id = match &request {
        AgentMessagingRequest::Reply { request_id, .. } => Some(request_id.clone()),
        _ => None,
    };
    match backend.invoke(request) {
        Ok(value) if value.is_object() => {
            let failed = value.get("error").is_some() || value.get("ok") == Some(&Value::Bool(false));
            if !failed && (value["operation"] != name
                || serde_json::from_value::<AgentMessagingResponse>(value.clone()).is_err()
                || ["interaction_id", "request_id"].iter().any(|key| value.get(key).is_some_and(|id| id.as_str().is_none_or(|id| id.trim().is_empty())))
                || reply_request_id.as_ref().is_some_and(|id| value["request_id"] != *id)) {
                return failure("invalid_receipt", "Runtime receipt did not match the operation. Delivery may be uncertain; do not replay automatically.");
            }
            tool_result(value, failed)
        }
        Ok(_) => failure("invalid_receipt", "Expected a structured runtime receipt. Delivery may be uncertain; do not replay automatically."),
        Err(error) => transport_error(error),
    }
}

fn decode<T: serde::de::DeserializeOwned>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|_| {
        "Arguments must match the tool schema; unknown fields and incorrect types are rejected."
            .into()
    })
}

fn valid_target(target: &str) -> Result<(), String> {
    let lower = target.to_ascii_lowercase();
    if target.trim().is_empty()
        || target != target.trim()
        || matches!(lower.as_str(), "all" | "*" | "broadcast")
        || lower.starts_with("class:")
    {
        return Err(
            "Use one exact agent name or UUID; broadcast selectors are unsupported.".into(),
        );
    }
    Ok(())
}

fn nonempty(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err("Text must not be empty.".into())
    } else {
        Ok(())
    }
}

fn parse(name: &str, arguments: Value, key: &str) -> Result<AgentMessagingRequest, String> {
    // These are the model-facing arguments. Admission keys and authenticated
    // origin are supplied by the adapter, never accepted from the model.
    if arguments
        .as_object()
        .is_none_or(|args| args.values().any(Value::is_null))
    {
        return Err(
            "Arguments must be an object; omit optional fields instead of passing null.".into(),
        );
    }
    let request = match name {
        "send_message" | "followup_task" => {
            let args: MessageArgs = decode(arguments)?;
            valid_target(&args.target)?;
            nonempty(&args.message)?;
            if name == "send_message" {
                AgentMessagingRequest::SendMessage {
                    target: args.target,
                    message: args.message,
                    idempotency_key: Some(key.into()),
                }
            } else {
                AgentMessagingRequest::FollowupTask {
                    target: args.target,
                    message: args.message,
                    idempotency_key: Some(key.into()),
                }
            }
        }
        "receive_messages" => {
            let args: ReceiveArgs = decode(arguments)?;
            if !(1..=100).contains(&args.limit) || args.timeout_ms > 60_000 {
                return Err("Receive limit must be 1..100 and timeout_ms 0..60000.".into());
            }
            for cursor in [&args.cursor, &args.ack_cursor].into_iter().flatten() {
                nonempty(cursor)?;
            }
            AgentMessagingRequest::ReceiveMessages {
                cursor: args.cursor,
                ack_cursor: args.ack_cursor,
                limit: Some(args.limit),
                timeout_ms: Some(args.timeout_ms),
            }
        }
        "reply" => {
            let args: ReplyArgs = decode(arguments)?;
            nonempty(&args.request_id)?;
            nonempty(&args.message)?;
            AgentMessagingRequest::Reply {
                request_id: args.request_id,
                status: args.status,
                message: args.message,
            }
        }
        "interrupt_agent" => {
            let args: TargetArgs = decode(arguments)?;
            valid_target(&args.target)?;
            AgentMessagingRequest::InterruptAgent {
                target: args.target,
            }
        }
        "list_agents" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Empty {}
            let _: Empty = decode(arguments)?;
            AgentMessagingRequest::ListAgents
        }
        _ => return Err("Unknown tool.".into()),
    };
    Ok(request)
}

fn transport_error(error: io::Error) -> Value {
    let code = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<live::ControlEndpointError>())
        .map(live::ControlEndpointError::code)
        .unwrap_or("control_transport_error");
    failure(code, "The operation failed or its receipt was lost. Delivery may be uncertain; do not replay automatically.")
}

pub(super) fn failure(code: &str, message: &str) -> Value {
    tool_result(json!({"error":{"code":code,"message":message}}), true)
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":is_error})
}
