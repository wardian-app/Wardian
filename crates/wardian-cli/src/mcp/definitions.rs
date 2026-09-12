use serde_json::{json, Value};

pub(super) const NAMES: [&str; 6] = [
    "send_message",
    "followup_task",
    "receive_messages",
    "reply",
    "interrupt_agent",
    "list_agents",
];

pub(super) fn definitions() -> Vec<Value> {
    NAMES.into_iter().map(definition).collect()
}

fn definition(name: &str) -> Value {
    let target =
        json!({"type":"string", "minLength":1, "description":"Exact Wardian agent name or UUID."});
    let message = json!({"type":"string", "minLength":1, "description":"Literal message text."});
    let (description, properties, required) = match name {
        "send_message" => (
            "Send information to one Wardian agent's durable inbox without starting or interrupting a turn. The receipt reports admission, not provider visibility.",
            json!({"target":target,"message":message}), vec!["target", "message"],
        ),
        "followup_task" => (
            "Assign a task to one Wardian agent and return its request receipt without waiting for a reply. Starting an inactive receiver can take time. If your assignment requires its result, wait with receive_messages until the correlated reply arrives or the caller's deadline expires; do not finish merely because early inbox polls are empty.",
            json!({"target":target,"message":message}), vec!["target", "message"],
        ),
        "receive_messages" => (
            "Receive information, tasks and replies addressed to this managed Wardian agent. Reuse the returned cursor; ack_cursor acknowledges a previously returned batch. When waiting for assigned work, use timeout_ms=60000 and repeat within the caller's deadline. A timeout means no message arrived during that wait, not that the task failed. It does not cancel tasks or authorize resending them.",
            json!({
                "cursor":{"type":"string","minLength":1},
                "ack_cursor":{"type":"string","minLength":1},
                "limit":{"type":"integer","minimum":1,"maximum":100,"default":100},
                "timeout_ms":{"type":"integer","minimum":0,"maximum":60000,"default":0}
            }), vec![],
        ),
        "reply" => (
            "Reply to a Wardian task request as its authorized recipient. The request determines the destination; ordinary messages and assistant completion do not complete the request.",
            json!({"request_id":{"type":"string","minLength":1},"status":{"type":"string","enum":["done","blocked","failed"]},"message":message}), vec!["request_id","status","message"],
        ),
        "interrupt_agent" => (
            "Request interruption of an agent's current turn while retaining its session. Use only when stopping its work is intended, never to poll, wake or speed up an agent after an empty receive wait. The result distinguishes capabilities and observed outcomes; it does not imply shutdown or confirmed interruption.",
            json!({"target":target}), vec!["target"],
        ),
        "list_agents" => (
            "List Wardian agents available to this managed sender, with their identities, providers and current statuses.",
            json!({}), vec![],
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
