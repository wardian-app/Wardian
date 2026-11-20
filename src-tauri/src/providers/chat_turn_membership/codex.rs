//! Membership backed by an explicit Codex rollout bracket, never row adjacency.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

const ADAPTER: &str = "codex_bracket_v1";
const MAX_ID_BYTES: usize = 512;
const MAX_REQUESTS: usize = 1_024;
// Leave room in the normalizer's byte budget even for JSON-escaped identities.
const MAX_STORED_ID_BYTES: usize = 128 * 1024;

/// Source-local continuation state. Only identities and proof flags are retained.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CodexState {
    source_session_id: Option<String>,
    agent_id: Option<String>,
    context_turn_id: Option<String>,
    active: Option<Bracket>,
    requests: BTreeMap<String, Request>,
    blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
struct Bracket {
    turn_id: String,
    context_seen: bool,
    broken: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
struct Request {
    root: Option<String>,
    conflicting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Context,
    Started,
    Complete,
    Prompt,
    Assistant,
    Call,
    Result,
    Approval,
    Unsupported,
}

impl CodexState {
    /// Observe every raw record after the normalizer has classified real input
    /// and assigned its request root, including records with no normalized row.
    ///
    /// A typed request matching its context can admit the prompt before start;
    /// work requires the matching context/start/request bracket. An open bracket
    /// never certifies totals; only its exact task_complete can do so. A prompt
    /// preceding its context retains its own root without gaining adapter proof.
    /// Capacity exhaustion fails closed until a new source session.
    pub(crate) fn observe(&mut self, raw: &Value, event: Option<&mut AgentChatEvent>) {
        if !self.valid() {
            *self = Self {
                blocked: true,
                ..Self::default()
            };
        }
        if raw["type"] == "session_meta" {
            let session = id(raw.get("payload").and_then(|payload| payload.get("id")));
            if session != self.source_session_id.as_deref() || session.is_none() {
                *self = Self {
                    source_session_id: session.map(str::to_owned),
                    blocked: session.is_none(),
                    ..Self::default()
                };
            }
            clear(event, false);
            return;
        }

        let kind = kind(raw);
        let turn = match explicit_turn(raw) {
            Ok(turn) if !self.blocked => turn,
            _ => {
                self.break_active();
                if kind == Kind::Context {
                    self.context_turn_id = None;
                }
                clear(event, true);
                return;
            }
        };

        if kind == Kind::Context {
            if let Some(active) = &mut self.active {
                if turn == Some(active.turn_id.as_str()) {
                    active.context_seen = true;
                } else {
                    active.broken = true;
                }
            }
            self.context_turn_id = turn.map(str::to_owned);
            if !self.valid() {
                self.block();
                clear(event, true);
                return;
            }
            clear(event, turn.is_none());
            return;
        }
        if kind == Kind::Started {
            let Some(turn) = turn else {
                self.break_active();
                clear(event, true);
                return;
            };
            if self
                .active
                .as_ref()
                .is_none_or(|active| active.turn_id != turn)
            {
                if let Some(previous) = self.active.take() {
                    self.requests.remove(&previous.turn_id);
                }
                self.active = Some(Bracket {
                    turn_id: turn.to_owned(),
                    context_seen: self.context_turn_id.as_deref() == Some(turn),
                    broken: false,
                });
            }
            if !self.valid() {
                self.block();
                clear(event, true);
                return;
            }
        }

        let explicit_conflict = self.active.as_ref().is_some_and(|active| {
            turn.is_some_and(|turn| turn != active.turn_id)
                || (kind == Kind::Complete && turn.is_none())
        });
        if explicit_conflict {
            self.break_active();
        }
        let closes = kind == Kind::Complete
            && self
                .active
                .as_ref()
                .is_some_and(|active| turn == Some(active.turn_id.as_str()));

        if let Some(event) = event {
            self.observe_event(raw, kind, turn, event, explicit_conflict);
        } else if matches!(
            kind,
            Kind::Prompt | Kind::Assistant | Kind::Call | Kind::Result | Kind::Approval
        ) {
            // A supported source item that cannot be represented is a known gap.
            self.break_active();
        }

        if closes {
            if let Some(active) = self.active.take() {
                self.requests.remove(&active.turn_id);
                if self.context_turn_id.as_deref() == Some(active.turn_id.as_str()) {
                    self.context_turn_id = None;
                }
            }
        }
    }

    fn observe_event(
        &mut self,
        raw: &Value,
        kind: Kind,
        turn: Option<&str>,
        event: &mut AgentChatEvent,
        explicit_conflict: bool,
    ) {
        let root = id(event.metadata.get("request_root_id")).map(str::to_owned);
        let root_invalid = event.metadata.get("request_root_id").is_some() && root.is_none();
        let provider_turn_invalid = event.metadata.get("provider_turn_id").is_some_and(|value| {
            id(Some(value)).is_none_or(|known| {
                turn.or_else(|| self.active.as_ref().map(|active| active.turn_id.as_str()))
                    != Some(known)
            })
        });
        let supported = supported(raw, kind, event);
        let foreign = event.provider != "codex"
            || !bounded(&event.session_id)
            || self
                .agent_id
                .as_deref()
                .is_some_and(|agent| agent != event.session_id);
        let invalid = foreign
            || root_invalid
            || provider_turn_invalid
            || event.metadata["codex_display_turn_conflict"] == true;
        let call_id = payload(raw).get("call_id");
        let call_conflict = matches!(kind, Kind::Call | Kind::Result | Kind::Approval)
            && call_id.is_some_and(|value| {
                id(Some(value)).is_none_or(|call| {
                    event
                        .metadata
                        .get("call_id")
                        .is_some_and(|known| known.as_str() != Some(call))
                })
            });
        clear(
            Some(&mut *event),
            explicit_conflict || invalid || call_conflict,
        );
        if invalid || call_conflict || (kind != Kind::Unsupported && !supported) {
            self.break_active();
            event.metadata["chat_turn_membership_partial"] = json!(true);
            return;
        }
        if !supported {
            return;
        }
        self.agent_id
            .get_or_insert_with(|| event.session_id.clone());
        if self.stored_id_bytes() > MAX_STORED_ID_BYTES {
            self.block();
            event.metadata["chat_turn_membership_partial"] = json!(true);
            return;
        }

        if kind == Kind::Prompt {
            // Inherited normalizer turn metadata and mirror adjacency are not
            // request evidence. An untyped observation can reuse a request only
            // inside its explicit context/start bracket, after a typed request
            // established that root. It cannot establish a new request itself.
            let prompt_turn = turn.map(str::to_owned).or_else(|| {
                self.active
                    .as_ref()
                    .filter(|active| {
                        active.context_seen
                            && self.context_turn_id.as_deref() == Some(active.turn_id.as_str())
                            && self.requests.get(&active.turn_id).is_some_and(|request| {
                                request.root.is_some() && !request.conflicting
                            })
                    })
                    .map(|active| active.turn_id.clone())
            });
            let Some((turn, root)) = prompt_turn.as_deref().zip(root.as_deref()) else {
                return;
            };
            let extra_bytes = self
                .requests
                .get(turn)
                .map_or(turn.len() + root.len(), |request| {
                    if request.root.is_none() {
                        root.len()
                    } else {
                        0
                    }
                });
            if (!self.requests.contains_key(turn) && self.requests.len() == MAX_REQUESTS)
                || self.stored_id_bytes() + extra_bytes > MAX_STORED_ID_BYTES
            {
                self.block();
                event.metadata["chat_turn_membership_partial"] = json!(true);
                return;
            }
            let request = self.requests.entry(turn.to_owned()).or_default();
            if request.root.as_deref().is_some_and(|known| known != root) {
                request.root = None;
                request.conflicting = true;
            }
            if request.conflicting {
                self.break_active();
                event.metadata["chat_turn_membership_partial"] = json!(true);
                return;
            }
            request.root = Some(root.to_owned());
            if !explicit_conflict {
                event.metadata["request_root_id"] = json!(root);
            }
        }

        let Some(active) = self.active.as_ref() else {
            if kind == Kind::Prompt && turn.is_some() && turn == self.context_turn_id.as_deref() {
                // The already-returned prompt cannot be retroactively tagged
                // when task_started arrives. Its typed context is sufficient
                // for prompt admission, never work or closed membership counts.
                event.metadata["chat_turn_adapter"] = json!(ADAPTER);
                event.metadata["provider_turn_id"] = json!(turn);
            } else if kind == Kind::Complete {
                event.metadata["chat_turn_membership_partial"] = json!(true);
            }
            return;
        };
        if explicit_conflict
            || !active.context_seen
            || self.context_turn_id.as_deref() != Some(active.turn_id.as_str())
        {
            if kind == Kind::Complete {
                event.metadata["chat_turn_membership_partial"] = json!(true);
            } else if matches!(
                kind,
                Kind::Assistant | Kind::Call | Kind::Result | Kind::Approval
            ) {
                self.break_active();
                event.metadata["chat_turn_membership_partial"] = json!(true);
            }
            return;
        }
        let Some(request_root) = self.requests.get(&active.turn_id).and_then(|request| {
            (!request.conflicting)
                .then_some(request.root.as_deref())
                .flatten()
        }) else {
            if kind == Kind::Complete {
                event.metadata["chat_turn_membership_partial"] = json!(true);
            } else if matches!(
                kind,
                Kind::Assistant | Kind::Call | Kind::Result | Kind::Approval
            ) {
                self.break_active();
            }
            return;
        };
        if root.as_deref().is_some_and(|root| root != request_root) {
            self.break_active();
            clear(Some(event), true);
            return;
        }
        event.metadata["chat_turn_adapter"] = json!(ADAPTER);
        event.metadata["request_root_id"] = json!(request_root);
        event.metadata["provider_turn_id"] = json!(active.turn_id);
        event.metadata["chat_turn_membership_complete"] =
            json!(kind == Kind::Complete && !active.broken);
        event.metadata["chat_turn_membership_partial"] = json!(active.broken);
        if matches!(kind, Kind::Call | Kind::Result | Kind::Approval) {
            if let Some(call) = id(call_id) {
                // Some rollout rows carry both turn_id and call_id. The legacy
                // normalizer's turn_id field cannot retain both identities.
                event.metadata["call_id"] = json!(call);
            }
        }
    }

    fn break_active(&mut self) {
        if let Some(active) = &mut self.active {
            active.broken = true;
        }
    }

    fn block(&mut self) {
        self.blocked = true;
        self.requests.clear();
        self.active = None;
        self.context_turn_id = None;
    }

    fn stored_id_bytes(&self) -> usize {
        [
            self.source_session_id.as_deref(),
            self.agent_id.as_deref(),
            self.context_turn_id.as_deref(),
            self.active.as_ref().map(|active| active.turn_id.as_str()),
        ]
        .into_iter()
        .flatten()
        .map(str::len)
        .sum::<usize>()
            + self
                .requests
                .iter()
                .map(|(turn, request)| turn.len() + request.root.as_deref().map_or(0, str::len))
                .sum::<usize>()
    }

    fn valid(&self) -> bool {
        [
            &self.source_session_id,
            &self.agent_id,
            &self.context_turn_id,
        ]
        .into_iter()
        .all(|value| value.as_deref().is_none_or(bounded))
            && self
                .active
                .as_ref()
                .is_none_or(|active| bounded(&active.turn_id))
            && self.requests.len() <= MAX_REQUESTS
            && self
                .requests
                .iter()
                .all(|(turn, request)| bounded(turn) && request.root.as_deref().is_none_or(bounded))
            && self.stored_id_bytes() <= MAX_STORED_ID_BYTES
    }
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES
}

fn id(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|value| bounded(value))
}

fn payload(raw: &Value) -> &Value {
    if raw["type"] == "item.completed" {
        &raw["item"]
    } else {
        &raw["payload"]
    }
}

fn explicit_turn(raw: &Value) -> Result<Option<&str>, ()> {
    let payload = payload(raw);
    let mut turn = None;
    for value in [
        raw.get("turn_id"),
        payload.get("turn_id"),
        payload
            .get("internal_chat_message_metadata_passthrough")
            .and_then(|value| value.get("turn_id")),
    ]
    .into_iter()
    .flatten()
    {
        let candidate = id(Some(value)).ok_or(())?;
        if turn.is_some_and(|known| known != candidate) {
            return Err(());
        }
        turn = Some(candidate);
    }
    Ok(turn)
}

fn kind(raw: &Value) -> Kind {
    if raw["type"] == "turn_context" {
        return Kind::Context;
    }
    let payload = payload(raw);
    match (raw["type"].as_str(), payload["type"].as_str()) {
        (Some("event_msg"), Some("task_started")) => Kind::Started,
        (Some("event_msg"), Some("task_complete")) => Kind::Complete,
        (Some("event_msg"), Some("user_message")) => Kind::Prompt,
        (Some("event_msg"), Some("agent_message" | "assistant_message")) => Kind::Assistant,
        (Some("response_item" | "item.completed"), Some("message")) => {
            match payload["role"].as_str() {
                Some("user") => Kind::Prompt,
                Some("assistant") => Kind::Assistant,
                _ => Kind::Unsupported,
            }
        }
        (Some("event_msg"), Some("exec_command_begin" | "exec_command_start"))
        | (Some("response_item" | "item.completed"), Some("function_call" | "custom_tool_call")) => {
            Kind::Call
        }
        (
            Some("response_item" | "item.completed"),
            Some("function_call_output" | "custom_tool_call_output"),
        ) => Kind::Result,
        (Some("event_msg"), Some("exec_approval_request")) => Kind::Approval,
        _ => Kind::Unsupported,
    }
}

fn supported(raw: &Value, kind: Kind, event: &AgentChatEvent) -> bool {
    let source = if kind == Kind::Complete {
        Some("task_complete")
    } else {
        raw["type"].as_str()
    };
    if event.source.as_deref() != source {
        return false;
    }
    match kind {
        Kind::Prompt => {
            event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::User)
                && matches!(
                    event.metadata["input_origin"].as_str(),
                    Some("human_input" | "agent_input")
                )
                && event.metadata["input_purpose"] == "request"
        }
        Kind::Assistant => {
            event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::Assistant)
        }
        Kind::Started | Kind::Call => event.kind == AgentChatEventKind::ToolCall,
        Kind::Result => event.kind == AgentChatEventKind::ToolResult,
        Kind::Approval => event.kind == AgentChatEventKind::Approval,
        Kind::Complete => event.kind == AgentChatEventKind::Status,
        Kind::Context | Kind::Unsupported => false,
    }
}

fn clear(event: Option<&mut AgentChatEvent>, partial: bool) {
    if let Some(event) = event {
        if !event.metadata.is_object() {
            event.metadata = json!({});
        }
        if let Some(metadata) = event.metadata.as_object_mut() {
            // Legacy consumers still own their existing identity fields. The
            // additive adapter withholds proof without rewriting that contract.
            metadata.remove("chat_turn_adapter");
        }
        event.metadata["chat_turn_membership_complete"] = json!(false);
        event.metadata["chat_turn_membership_partial"] = json!(partial);
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
