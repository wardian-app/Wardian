use serde_json::{json, Value};

pub(super) const NAMES: [&str; 5] = [
    "send_message",
    "followup_task",
    "reply",
    "interrupt_agent",
    "list_agents",
];

pub(super) fn definitions() -> Vec<Value> {
    NAMES.into_iter().map(definition).collect()
}

/// Expose only the operations authorized by an external controller grant.
pub(super) fn controller_definitions(
    principal: &wardian_core::controllers::ControllerPrincipal,
) -> Vec<Value> {
    use wardian_core::controllers::ControllerScope as Scope;

    let has = |scope| principal.scopes.contains(&scope);
    let mut names = Vec::new();
    if has(Scope::AgentsRead) {
        names.push("list_agents");
    }
    if has(Scope::MessagesSend) {
        names.push("send_message");
    }
    if has(Scope::TasksSubmit) && has(Scope::AgentsWake) {
        names.push("followup_task");
    }
    if has(Scope::ControllerReceive) {
        names.push("receive_messages");
    }
    if has(Scope::ControllerWait) {
        names.push("wait_agent");
    }
    if has(Scope::AgentsInterrupt) {
        names.push("interrupt_agent");
    }
    names
        .into_iter()
        .map(|name| controller_definition(name, principal))
        .collect()
}

fn controller_definition(
    name: &str,
    principal: &wardian_core::controllers::ControllerPrincipal,
) -> Value {
    use wardian_core::controllers::ControllerScope as Scope;

    let mut value = match name {
        "receive_messages" => {
            let mut properties = json!({
                "cursor":{"type":"string","minLength":1,"description":"Opaque cursor returned by this controller's mailbox."},
                "ack_cursor":{"type":"string","minLength":1,"description":"Acknowledge through the returned cursor; requires controller:ack."},
                "limit":{"type":"integer","minimum":1,"maximum":100},
                "timeout_ms":{"type":"integer","minimum":0,"maximum":60000}
            });
            if !principal.scopes.contains(&Scope::ControllerAck) {
                properties
                    .as_object_mut()
                    .expect("receive properties are an object")
                    .remove("ack_cursor");
            }
            json!({
                "name":name,
                "description":"Read this controller's durable mailbox. Reading does not acknowledge messages; ack_cursor is a separate granted operation.",
                "inputSchema":{"type":"object","properties":properties,"additionalProperties":false},
                "annotations":{"readOnlyHint":!principal.scopes.contains(&Scope::ControllerAck),"idempotentHint":true,"openWorldHint":false}
            })
        }
        "wait_agent" => definition_with_schema(
            name,
            "Wait for activity in this controller's durable mailbox. Timeout does not cancel admitted work.",
            json!({"timeout_ms":{"type":"integer","minimum":0,"maximum":60000}}),
            Vec::<&str>::new(),
            true,
        ),
        _ => {
            let mut value = definition(name);
            value["description"] = match name {
                "list_agents" => "List only agents selected by this owner-issued controller grant.".into(),
                "send_message" => "Admit an informational message to one selected agent. Wardian queues it and wakes the agent when its original conversation is idle and available. It creates no task or reply obligation; admission is not provider visibility.".into(),
                "followup_task" => "Submit work to one selected agent and return the canonical request ID. Wardian queues it for the worker's supported delivery boundary and returns its terminal reply through this controller's mailbox when available. The receipt reports admission, not completion. Do not replay uncertain calls.".into(),
                "interrupt_agent" => "Request interruption of a selected agent's current turn while retaining its session. This is not cancellation of a particular task or process termination.".into(),
                _ => unreachable!(),
            };
            if matches!(name, "send_message" | "followup_task" | "interrupt_agent") {
                value["inputSchema"]["properties"]["target"]["enum"] =
                    json!(principal.agent_ids);
            }
            value
        }
    };
    if name == "receive_messages" && principal.scopes.contains(&Scope::ControllerAck) {
        value["annotations"]["readOnlyHint"] = false.into();
    }
    value
}

fn definition_with_schema(
    name: &str,
    description: &str,
    properties: Value,
    required: Vec<&str>,
    read_only: bool,
) -> Value {
    json!({
        "name":name,
        "description":description,
        "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":read_only,"idempotentHint":true,"openWorldHint":false}
    })
}

fn definition(name: &str) -> Value {
    let target =
        json!({"type":"string", "minLength":1, "description":"Exact Wardian agent name or UUID."});
    let message = json!({"type":"string", "minLength":1, "description":"Literal message text."});
    let (description, properties, required) = match name {
        "send_message" => (
            "Admit an informational message to one Wardian agent. Wardian automatically delivers it and wakes an idle recipient when its original conversation is available. It creates no task or reply obligation; the receipt reports admission, not provider visibility.",
            json!({"target":target,"message":message}), vec!["target", "message"],
        ),
        "followup_task" => (
            "Assign a task to one Wardian agent and return its request receipt. Wardian resumes the requester's original provider conversation with the terminal reply when that provider route is available; the reply keeps its task correlation and creates no followup obligation. The receipt reports admission, not completion. Do not poll or replay when no result is immediately visible.",
            json!({"target":target,"message":message}), vec!["target", "message"],
        ),
        "reply" => (
            "Reply to a Wardian task request as its authorized recipient. Wardian automatically resumes the original requester conversation with this terminal reply when a safe provider route is available. The request determines the destination. Ordinary messages do not complete a request.",
            json!({"request_id":{"type":"string","minLength":1},"status":{"type":"string","enum":["done","blocked","failed"]},"message":message}), vec!["request_id","status","message"],
        ),
        "interrupt_agent" => (
            "Request interruption of an agent's current turn while retaining its session. Use only when stopping its work is intended, never to poll or speed up an agent after task admission. The result distinguishes capabilities and observed outcomes; it does not imply shutdown or confirmed interruption.",
            json!({"target":target}), vec!["target"],
        ),
        "list_agents" => (
            "List Wardian agents available to this managed sender, with their identities, providers and current statuses.",
            json!({}), vec![],
        ),
        "receive_messages" => return definition_with_schema(
            name,
            "Read messages from this controller's durable mailbox.",
            json!({
                "cursor":{"type":"string","minLength":1},
                "ack_cursor":{"type":"string","minLength":1},
                "limit":{"type":"integer","minimum":1,"maximum":100},
                "timeout_ms":{"type":"integer","minimum":0,"maximum":60000}
            }),
            Vec::<&str>::new(),
            false,
        ),
        "wait_agent" => return definition_with_schema(
            name,
            "Wait for mailbox activity.",
            json!({"timeout_ms":{"type":"integer","minimum":0,"maximum":60000}}),
            Vec::<&str>::new(),
            true,
        ),
        _ => unreachable!(),
    };
    json!({
        "name":name, "description":description,
        "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{
            "readOnlyHint":name == "list_agents",
            "destructiveHint":name == "interrupt_agent",
            "idempotentHint":matches!(name,"list_agents"|"reply"),
            "openWorldHint":name != "list_agents"
        }
    })
}
