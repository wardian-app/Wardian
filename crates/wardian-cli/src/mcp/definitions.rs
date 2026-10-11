use serde_json::{json, Value};

pub(super) const NAMES: [&str; 8] = [
    "send_message",
    "followup_task",
    "receive_messages",
    "wait_agent",
    "reply",
    "interrupt_agent",
    "list_agents",
    "read_task_context",
];

pub(super) fn definitions() -> Vec<Value> {
    NAMES.into_iter().map(definition).collect()
}

fn definition(name: &str) -> Value {
    let target =
        json!({"type":"string", "minLength":1, "description":"Exact Wardian agent name or UUID."});
    let message = json!({"type":"string", "minLength":1, "description":"Literal message text."});
    let (description, properties, required) = match name {
        "read_task_context" => (
            "Recover unresolved Wardian peer tasks bound to this exact active native Codex turn after compaction. Requires provider call metadata verified against the current owner's native event. Human instructions always prevail. This read does not claim, acknowledge, complete, replay or start work. Errors return no partial task list; do not guess another turn.",
            json!({}), vec![],
        ),
        "send_message" => (
            "Send information to one Wardian agent's durable inbox without starting or interrupting a turn. The receipt reports admission, not provider visibility.",
            json!({"target":target,"message":message}), vec!["target", "message"],
        ),
        "followup_task" => (
            "Assign a task to one Wardian agent and return its request receipt without waiting for a reply. Exact native Codex turns automatically return only positively attributed per-request outcomes from the host-instructed final appendix, unless an explicit reply already completed the task. Generic final prose leaves tasks unresolved. Other delivery paths require explicit reply. Starting an inactive receiver can take time. If your assignment requires its result, use wait_agent for mailbox activity, then receive_messages to inspect the correlated reply. A timeout does not mean the task failed.",
            json!({"target":target,"message":message}), vec!["target", "message"],
        ),
        "receive_messages" => (
            "Read a bounded batch of information, tasks and replies addressed to this managed Wardian agent. Reuse the returned cursor; ack_cursor acknowledges a previously returned batch. A nonzero timeout waits for mailbox activity and then returns the page. Use wait_agent when you need a wake without reading, claiming, or acknowledging inbox records.",
            json!({
                "cursor":{"type":"string","minLength":1},
                "ack_cursor":{"type":"string","minLength":1},
                "limit":{"type":"integer","minimum":1,"maximum":100,"default":100},
                "timeout_ms":{"type":"integer","minimum":0,"maximum":60000,"default":0}
            }), vec![],
        ),
        "wait_agent" => (
            "Wait for mailbox activity addressed to this managed Wardian agent, including information and task-completion replies. This returns no message bodies and does not read, claim, or acknowledge inbox records; call receive_messages to inspect and acknowledge them. It returns immediately when unacknowledged inbox activity already exists. Waiting never starts or interrupts an idle agent. timeout_ms is bounded to 60000; a timeout does not mean a task failed.",
            json!({"timeout_ms":{"type":"integer","minimum":0,"maximum":60000,"default":60000}}), vec![],
        ),
        "reply" => (
            "Reply to a Wardian task request as its authorized recipient. The request determines the destination. A committed explicit reply takes precedence over automatic attributed native Codex outcomes; manual receive and unsupported provider delivery require this tool. Ordinary messages do not complete a request.",
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
            "readOnlyHint":matches!(name,"list_agents"|"read_task_context"),
            "destructiveHint":name == "interrupt_agent",
            "idempotentHint":matches!(name,"list_agents"|"reply"),
            "openWorldHint":!matches!(name,"list_agents"|"read_task_context")
        }
    })
}
