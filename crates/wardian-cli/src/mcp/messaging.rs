use crate::live;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use wardian_core::agent_messaging::{AgentMessagingRequest, AgentMessagingResponse};

pub(super) trait Backend {
    fn require_sender(&self) -> io::Result<()>;
    fn invoke(&mut self, request: AgentMessagingRequest) -> io::Result<Value>;
    fn identity_failure(&self) -> Value {
        failure(
            "missing_managed_sender",
            "A managed Wardian sender is required. No control request was sent.",
        )
    }
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
        return backend.identity_failure();
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

/// External controller identity is explicit and never falls back to managed mode.
pub(super) struct ControllerLive {
    token: wardian_core::control::ControllerSecret,
    owner: String,
    resource: String,
    request_key: Option<String>,
}
impl ControllerLive {
    pub(super) fn from_env() -> io::Result<Self> {
        if std::env::var_os("WARDIAN_SESSION_ID").is_some() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let token = std::env::var("WARDIAN_CONTROLLER_TOKEN")
            .map_err(|_| io::ErrorKind::PermissionDenied)?;
        let resource = std::env::var("WARDIAN_CONTROLLER_RESOURCE")
            .map_err(|_| io::ErrorKind::PermissionDenied)?;
        let principal = wardian_core::controllers::ControllerStore::current()?
            .authenticate_token_for_resource(&token, &resource)?;
        let owner = principal.mailbox_owner();
        let request_key = std::env::var("WARDIAN_CONTROLLER_REQUEST_KEY").ok();
        if request_key
            .as_ref()
            .is_some_and(|key| !valid_controller_key(&owner, key))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid controller request key",
            ));
        }
        Ok(Self {
            token: wardian_core::control::ControllerSecret::new(token),
            owner,
            resource,
            request_key,
        })
    }
}
fn valid_controller_key(owner: &str, key: &str) -> bool {
    key.strip_prefix(&format!("{owner}:"))
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
impl Backend for ControllerLive {
    fn identity_failure(&self) -> Value {
        failure("unauthorized_controller", "The owner-issued controller grant is invalid, expired, revoked or intended for another resource. No request was sent.")
    }
    fn require_sender(&self) -> io::Result<()> {
        wardian_core::controllers::ControllerStore::current()?
            .authenticate_token_for_resource(self.token.expose(), &self.resource)
            .map(|_| ())
    }
    fn invoke(&mut self, mut request: AgentMessagingRequest) -> io::Result<Value> {
        match &mut request {
            AgentMessagingRequest::SendMessage {
                idempotency_key, ..
            }
            | AgentMessagingRequest::FollowupTask {
                idempotency_key, ..
            } => {
                *idempotency_key = Some(self.request_key.clone().unwrap_or_else(|| {
                    wardian_core::control::controller_request_key(
                        &self.owner,
                        idempotency_key.as_deref().unwrap_or(""),
                    )
                }));
            }
            _ => {}
        }
        live::messaging::controller_request(request, self.token.clone(), self.resource.clone())
    }
}
#[cfg(test)]
mod controller_tests {
    use super::*;
    #[test]
    fn stable_controller_admission_key_is_bounded_and_principal_owned() {
        let owner = "controller:437c7c93-e849-467c-9162-048bbfc8aef9";
        let key = wardian_core::control::controller_request_key(owner, "\"request-1\"");
        assert!(valid_controller_key(owner, &key));
        assert!(!valid_controller_key("controller:different", &key));
        assert!(!valid_controller_key(
            owner,
            &format!("{owner}:{}", "F".repeat(64))
        ));
        assert_eq!(
            key,
            wardian_core::control::controller_request_key(owner, "\"request-1\"")
        );
        assert_ne!(
            key,
            wardian_core::control::controller_request_key(owner, "1")
        );
        assert!(key.len() < 256);
    }
}
