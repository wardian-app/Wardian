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
