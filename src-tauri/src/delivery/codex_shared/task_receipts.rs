//! Native accepted-input evidence, separate from RPC acceptance and final text.
use super::*;
use sha2::{Digest, Sha256};
use wardian_core::db::agent_messaging::{TaskDeliveryReceipt, TaskReceiptSource};

const MAX_TASKS: usize = 64;
const MAX_INPUT_ITEMS: usize = 16;

fn body_hash(body: &str) -> String {
    format!("{:x}", Sha256::digest(body.as_bytes()))
}

#[derive(Debug)]
struct AcceptedInput {
    turn_id: String,
    source_id: String,
}

#[derive(Debug)]
struct TaskInput {
    generation: u64,
    thread_id: String,
    input_hash: String,
    task_body_hash: String,
    rpc_id: Option<String>,
    turn_id: Option<String>,
    items: Vec<AcceptedInput>,
    invalid: bool,
}

/// Bounded, connection-local evidence retained until its completion consumer reads it.
#[derive(Debug, Default)]
pub(super) struct TaskReceipts {
    tasks: HashMap<String, TaskInput>,
}

impl TaskReceipts {
    pub(super) fn prepare(
        &mut self,
        request_id: &str,
        generation: u64,
        thread_id: &str,
        input: &str,
        task_body: &str,
    ) {
        if let Some(previous) = self.tasks.get_mut(request_id) {
            // Reusing an issued task ID cannot borrow the previous admission's input.
            previous.invalid = true;
            return;
        }
        if self.tasks.len() >= MAX_TASKS {
            // Never expire an earlier admitted task before its completion consumer.
            // Capacity exhaustion gives the new task no receipt authority.
            return;
        }
        self.tasks.insert(
            request_id.to_owned(),
            TaskInput {
                generation,
                thread_id: thread_id.to_owned(),
                input_hash: body_hash(input),
                task_body_hash: body_hash(task_body),
                rpc_id: None,
                turn_id: None,
                items: Vec::new(),
                invalid: generation == 0 || thread_id.is_empty(),
            },
        );
    }

    /// Called only under the native writer fence immediately before submission.
    pub(super) fn issue(&mut self, rpc_id: &str, method: &str, params: &Value) {
        if !matches!(method, "turn/start" | "turn/steer") {
            return;
        }
        let Some(request_id) = params["clientUserMessageId"].as_str() else {
            return;
        };
        let Some(task) = self.tasks.get_mut(request_id) else {
            return;
        };
        let input = &params["input"];
        if task.rpc_id.is_some()
            || params["threadId"].as_str() != Some(task.thread_id.as_str())
            || input.as_array().is_none_or(|parts| parts.len() != 1)
            || input[0]["type"] != "text"
            || input[0]["text"]
                .as_str()
                .is_none_or(|text| body_hash(text) != task.input_hash)
        {
            task.invalid = true;
            return;
        }
        task.rpc_id = Some(rpc_id.to_owned());
    }

    pub(super) fn cancel(&mut self, request_id: &str) {
        self.tasks.remove(request_id);
    }

    /// A correlated RPC response supplies the turn; its actual items supply input proof.
    pub(super) fn acknowledge(&mut self, request_id: &str, result: &Value, turn_id: &str) {
        let Some(task) = self.tasks.get_mut(request_id) else {
            return;
        };
        if task.rpc_id.is_none()
            || result["turn"]["id"]
                .as_str()
                .or_else(|| result["turnId"].as_str())
                != Some(turn_id)
            || result["threadId"]
                .as_str()
                .is_some_and(|id| id != task.thread_id)
        {
            task.invalid = true;
            return;
        }
        task.turn_id = Some(turn_id.to_owned());
        if let Some(items) = result["turn"]["items"].as_array() {
            for item in items {
                Self::observe_item(task, request_id, turn_id, item);
            }
        }
    }

    pub(super) fn observe(&mut self, event: &Value) {
        let params = &event["params"];
        if !matches!(
            event["method"].as_str(),
            Some("item/started" | "item/completed")
        ) {
            return;
        }
        let (Some(thread_id), Some(turn_id)) =
            (params["threadId"].as_str(), params["turnId"].as_str())
        else {
            return;
        };
        let Some(request_id) = params["item"]["clientId"].as_str() else {
            return;
        };
        let Some(task) = self.tasks.get_mut(request_id) else {
            return;
        };
        if task.rpc_id.is_some()
            && task.thread_id == thread_id
            && task.turn_id.as_deref().is_none_or(|id| id == turn_id)
        {
            Self::observe_item(task, request_id, turn_id, &params["item"]);
        }
    }

    fn observe_item(task: &mut TaskInput, request_id: &str, turn_id: &str, item: &Value) {
        if task.invalid
            || item["type"] != "userMessage"
            || item["clientId"].as_str() != Some(request_id)
            || turn_id.is_empty()
            || turn_id.len() > 1024
        {
            return;
        }
        let Some(source_id) = item["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 1024)
        else {
            return;
        };
        let Some(content) = item["content"].as_array().filter(|parts| parts.len() == 1) else {
            return;
        };
        let Some(text) = content[0]["text"]
            .as_str()
            .filter(|_| content[0]["type"] == "text")
        else {
            return;
        };
        let prior = task
            .items
            .iter()
            .any(|input| input.turn_id == turn_id && input.source_id == source_id);
        if body_hash(text) != task.input_hash {
            if prior {
                task.invalid = true;
            }
            return;
        }
        if prior {
            return;
        }
        if task.items.len() >= MAX_INPUT_ITEMS {
            task.invalid = true;
            return;
        }
        task.items.push(AcceptedInput {
            turn_id: turn_id.to_owned(),
            source_id: source_id.to_owned(),
        });
    }

    pub(super) fn receipt(
        &self,
        request_id: &str,
        turn_id: &str,
        generation: u64,
        thread_id: &str,
    ) -> Option<TaskDeliveryReceipt> {
        let task = self.tasks.get(request_id)?;
        if task.invalid
            || task.generation != generation
            || task.thread_id != thread_id
            || task.turn_id.as_deref() != Some(turn_id)
        {
            return None;
        }
        let mut inputs = task.items.iter().filter(|input| input.turn_id == turn_id);
        let input = inputs.next()?;
        if inputs.next().is_some() {
            return None;
        }
        Some(TaskDeliveryReceipt {
            generation,
            provider_session_id: thread_id.to_owned(),
            provider_turn_id: turn_id.to_owned(),
            source: TaskReceiptSource::AcceptedUserMessage {
                source_id: input.source_id.clone(),
                task_body_sha256: task.task_body_hash.clone(),
            },
        })
    }
}

impl CodexSharedClient {
    /// Return exact accepted task input evidence, never an outgoing prompt or bare ack.
    /// The terminal-result consumer retires this request's slot, including absent proof;
    /// calls before terminal observation do not consume it.
    pub(crate) fn task_delivery_receipt(
        &self,
        request_id: &str,
        turn_id: &str,
    ) -> Option<TaskDeliveryReceipt> {
        let mut receipt = None;
        self.observation.send_modify(|state| {
            if (!state.closed && !state.stopped) || state.has_terminal_evidence(turn_id) {
                if let Some(thread_id) = state.thread_id.as_deref() {
                    receipt = state.task_receipts.receipt(
                        request_id,
                        turn_id,
                        self.generation,
                        thread_id,
                    );
                }
            }
            // Only the final-result consumer retires evidence. Early probes do not.
            if state.has_terminal_evidence(turn_id)
                && state
                    .task_receipts
                    .tasks
                    .get(request_id)
                    .is_some_and(|task| task.turn_id.as_deref() == Some(turn_id))
            {
                state.task_receipts.cancel(request_id);
            }
        });
        receipt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure native-source checker only. No retired receive/wait tool is registered
    // or dispatched by the production shared owner.
    struct IssuedMailboxCall {
        generation: u64,
        thread_id: String,
        call_id: String,
    }

    impl IssuedMailboxCall {
        fn observe(generation: u64, thread_id: &str, item: &Value) -> Option<Self> {
            if generation == 0
                || thread_id.is_empty()
                || item["type"] != "function_call"
                || !matches!(
                    item["name"].as_str(),
                    Some("mcp__wardian__receive_messages" | "mcp__wardian__wait_agent")
                )
            {
                return None;
            }
            Some(Self {
                generation,
                thread_id: thread_id.to_owned(),
                call_id: item["call_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())?
                    .to_owned(),
            })
        }
    }

    fn tool_output_receipt(
        issued: Option<&IssuedMailboxCall>,
        generation: u64,
        thread_id: &str,
        turn_id: &str,
        item: &Value,
        request_id: &str,
        task_body: &str,
    ) -> Option<TaskDeliveryReceipt> {
        let issued = issued?;
        if issued.generation != generation
            || issued.thread_id != thread_id
            || turn_id.is_empty()
            || item["type"] != "function_call_output"
            || item["call_id"].as_str() != Some(issued.call_id.as_str())
        {
            return None;
        }
        let page: Value = serde_json::from_str(item["output"].as_str()?).ok()?;
        let mut matches = page["messages"].as_array()?.iter().filter(|message| {
            message["kind"] == "task"
                && message["interaction_id"] == request_id
                && message["message"].as_str() == Some(task_body)
        });
        matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(TaskDeliveryReceipt {
            generation,
            provider_session_id: thread_id.to_owned(),
            provider_turn_id: turn_id.to_owned(),
            source: TaskReceiptSource::ToolCallOutput {
                call_id: issued.call_id.clone(),
                issued_generation: issued.generation,
                issued_thread_id: issued.thread_id.clone(),
                task_body_sha256: body_hash(task_body),
            },
        })
    }

    fn call() -> Value {
        json!({"type":"function_call", "call_id":"issued-call", "name":"mcp__wardian__receive_messages", "arguments":"{}"})
    }

    fn output() -> Value {
        json!({"type":"function_call_output", "call_id":"issued-call", "output":json!({
            "messages":[{"kind":"task", "interaction_id":"task", "message":"literal task"}]
        }).to_string()})
    }

    #[test]
    fn same_generation_tool_output_requires_observed_exact_issuing_call() {
        let issued = IssuedMailboxCall::observe(7, "thread", &call()).unwrap();
        let receipt = tool_output_receipt(
            Some(&issued),
            7,
            "thread",
            "turn",
            &output(),
            "task",
            "literal task",
        )
        .unwrap();
        assert_eq!(receipt.generation, 7);
        assert!(
            matches!(receipt.source, TaskReceiptSource::ToolCallOutput { call_id, issued_generation: 7, issued_thread_id, task_body_sha256 }
            if call_id == "issued-call" && issued_thread_id == "thread" && task_body_sha256 == body_hash("literal task"))
        );
    }

    #[test]
    fn stale_unissued_foreign_and_missing_tool_call_ids_never_authorize() {
        let issued = IssuedMailboxCall::observe(6, "thread", &call()).unwrap();
        assert!(tool_output_receipt(
            Some(&issued),
            7,
            "thread",
            "turn",
            &output(),
            "task",
            "literal task"
        )
        .is_none());
        let issued = IssuedMailboxCall::observe(7, "thread", &call()).unwrap();
        assert!(
            tool_output_receipt(None, 7, "thread", "turn", &output(), "task", "literal task")
                .is_none()
        );
        assert!(tool_output_receipt(
            Some(&issued),
            7,
            "foreign",
            "turn",
            &output(),
            "task",
            "literal task"
        )
        .is_none());
        for call_id in [Value::Null, json!(""), json!("old-call")] {
            let mut output = output();
            output["call_id"] = call_id;
            assert!(tool_output_receipt(
                Some(&issued),
                7,
                "thread",
                "turn",
                &output,
                "task",
                "literal task"
            )
            .is_none());
        }
        let mut missing = call();
        missing.as_object_mut().unwrap().remove("call_id");
        assert!(IssuedMailboxCall::observe(7, "thread", &missing).is_none());
    }

    #[test]
    fn capacity_preserves_earlier_receipts_until_completion_consumer() {
        let mut receipts = TaskReceipts::default();
        for index in 0..MAX_TASKS {
            receipts.prepare(&format!("task-{index}"), 7, "thread", "input", "input");
        }
        receipts.prepare("overflow", 7, "thread", "input", "input");
        assert_eq!(receipts.tasks.len(), MAX_TASKS);
        assert!(receipts.tasks.contains_key("task-0"));
        assert!(!receipts.tasks.contains_key("overflow"));
        receipts.issue("rpc", "turn/start", &json!({
            "threadId":"thread", "clientUserMessageId":"task-0", "input":[{"type":"text","text":"input"}]
        }));
        receipts.acknowledge("task-0", &json!({"turn":{"id":"turn"}}), "turn");
        receipts.observe(&json!({"method":"item/started", "params":{
            "threadId":"thread", "turnId":"turn", "item":{
                "type":"userMessage", "id":"source", "clientId":"task-0", "content":[{"type":"text", "text":"input"}]
            }
        }}));
        assert!(receipts.receipt("task-0", "turn", 7, "thread").is_some());
        assert!(receipts.receipt("task-0", "turn", 8, "thread").is_none());
        assert!(receipts.receipt("task-0", "turn", 7, "foreign").is_none());
        receipts.cancel("task-0");
        receipts.prepare("new", 7, "thread", "input", "input");
        assert!(receipts.tasks.contains_key("new"));
    }
}
