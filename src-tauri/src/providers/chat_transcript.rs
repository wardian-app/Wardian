use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

use crate::providers::claude::{
    classify_claude_user_event, claude_context_purpose, claude_provider_causal_ref,
    ClaudeUserEventKind,
};

pub fn normalize_chat_lines(
    session_id: &str,
    provider: &str,
    lines: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<AgentChatEvent> {
    let normalized_provider = normalize_provider(provider);
    let mut seen_gemini_messages = HashSet::new();
    let mut events: Vec<AgentChatEvent> = Vec::new();
    let mut request_root_id: Option<String> = None;
    let mut codex_provider_turn_id: Option<String> = None;
    let mut pending_context_events: Vec<usize> = Vec::new();
    let mut tool_request_roots: HashMap<String, (Option<String>, bool)> = HashMap::new();

    for (index, line) in lines.into_iter().enumerate() {
        let sequence = index as u64 + 1;
        let Some(mut event) = normalize_chat_line(
            session_id,
            normalized_provider.as_str(),
            line.as_ref(),
            sequence,
        ) else {
            continue;
        };

        if normalized_provider == "gemini"
            && event.kind == AgentChatEventKind::Message
            && event.role == Some(AgentChatRole::Assistant)
        {
            let key = format!(
                "{}\n{}",
                event.turn_id.as_deref().unwrap_or(""),
                event.text.as_deref().unwrap_or("")
            );
            if !seen_gemini_messages.insert(key) {
                continue;
            }
        }

        let input_origin = metadata_string(&event.metadata, "input_origin");
        if matches!(input_origin.as_deref(), Some("human_input" | "agent_input")) {
            let root_id = metadata_string(&event.metadata, "request_root_id")
                .or_else(|| event.turn_id.clone())
                .or_else(|| (normalized_provider == "codex").then(|| event.id.clone()));
            if let Some(root_id) = root_id.as_deref() {
                set_metadata_string(&mut event.metadata, "request_root_id", root_id);
                request_root_id = Some(root_id.to_string());
            }

            if normalized_provider == "codex" {
                let pending_turn_id = pending_context_events.iter().find_map(|index| {
                    events
                        .get(*index)
                        .and_then(|context| metadata_string(&context.metadata, "provider_turn_id"))
                });
                if let Some(root_id) = root_id {
                    for index in pending_context_events.drain(..) {
                        if let Some(context) = events.get_mut(index) {
                            set_metadata_string(&mut context.metadata, "request_root_id", &root_id);
                            if metadata_string(&context.metadata, "causal_ref").is_none() {
                                set_metadata_string(
                                    &mut context.metadata,
                                    "causal_ref",
                                    &format!("request:{root_id}"),
                                );
                            }
                        }
                    }
                }
                codex_provider_turn_id = pending_turn_id;
            }
        }

        if event.kind == AgentChatEventKind::ToolCall {
            if let Some(tool_id) = event.turn_id.clone() {
                let is_skill = metadata_string(&event.metadata, "tool_name")
                    .or_else(|| event.title.clone())
                    .is_some_and(|tool| tool.eq_ignore_ascii_case("skill"));
                tool_request_roots.insert(tool_id, (request_root_id.clone(), is_skill));
            }
        }

        if input_origin.as_deref() == Some("context_injection") {
            if let Some(causal_ref) = metadata_string(&event.metadata, "causal_ref") {
                if let Some(tool_id) = causal_ref.strip_prefix("provider:tool_use:") {
                    if let Some((root_id, is_skill)) = tool_request_roots.get(tool_id) {
                        if let Some(root_id) = root_id {
                            set_metadata_string(&mut event.metadata, "request_root_id", root_id);
                            request_root_id = Some(root_id.clone());
                        }
                        if *is_skill {
                            set_metadata_string(&mut event.metadata, "input_purpose", "skill");
                        }
                    }
                }
            }
            let provider_turn_matches = if normalized_provider == "codex" {
                match metadata_string(&event.metadata, "provider_turn_id") {
                    Some(context_turn) => {
                        codex_provider_turn_id.as_deref() == Some(context_turn.as_str())
                    }
                    None => true,
                }
            } else {
                true
            };
            if metadata_string(&event.metadata, "request_root_id").is_none()
                && provider_turn_matches
            {
                if let Some(root_id) = request_root_id.as_deref() {
                    set_metadata_string(&mut event.metadata, "request_root_id", root_id);
                }
            }
            if metadata_string(&event.metadata, "causal_ref").is_none()
                && metadata_string(&event.metadata, "request_root_id").is_some()
            {
                if let Some(root_id) = request_root_id.as_deref() {
                    set_metadata_string(
                        &mut event.metadata,
                        "causal_ref",
                        &format!("request:{root_id}"),
                    );
                }
            }
        }

        let pending_context = normalized_provider == "codex"
            && input_origin.as_deref() == Some("context_injection")
            && metadata_string(&event.metadata, "request_root_id").is_none()
            && metadata_string(&event.metadata, "provider_turn_id").is_some();
        let event_index = events.len();
        events.push(event);
        if pending_context {
            pending_context_events.push(event_index);
        }
    }

    events
}

pub fn normalize_chat_line(
    session_id: &str,
    provider: &str,
    raw_line: &str,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let provider = normalize_provider(provider);
    let raw_line = raw_line.trim();
    if raw_line.is_empty() {
        return None;
    }

    let parsed = match serde_json::from_str::<Value>(raw_line) {
        Ok(parsed) => parsed,
        Err(_) => return fallback_terminal_event(session_id, &provider, raw_line, sequence),
    };

    match provider.as_str() {
        "codex" => normalize_codex(session_id, &provider, &parsed, sequence),
        "claude" => normalize_claude(session_id, &provider, &parsed, sequence),
        "gemini" => normalize_gemini(session_id, &provider, &parsed, sequence),
        "antigravity" => normalize_antigravity(session_id, &provider, &parsed, sequence),
        "opencode" => normalize_opencode(session_id, &provider, &parsed, sequence),
        "pi" => normalize_pi(session_id, &provider, &parsed, sequence),
        "mock" => normalize_mock(session_id, &provider, &parsed, sequence),
        _ => normalize_fallback_json(session_id, &provider, &parsed, raw_line, sequence),
    }
}

fn normalize_pi(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type")?;
    match msg_type {
        "session" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Idle,
            msg_type,
            parsed,
        )),
        "agent_start" | "turn_start" | "message_start" | "message_update" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Processing,
            msg_type,
            parsed,
        )),
        "agent_end" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Succeeded,
            msg_type,
            parsed,
        )),
        "message" | "message_end" => {
            let message = parsed.get("message")?;
            match str_field(message, "role")? {
                "user" => message_event(
                    session_id,
                    provider,
                    sequence,
                    AgentChatRole::User,
                    text_from_value(message)?,
                    msg_type.into(),
                    turn_id_from(message),
                    "message",
                ),
                "assistant" => {
                    if let Some(text) = text_from_value(message) {
                        return message_event(
                            session_id,
                            provider,
                            sequence,
                            AgentChatRole::Assistant,
                            text,
                            msg_type.into(),
                            turn_id_from(message),
                            "message",
                        );
                    }
                    let tool = content_array(message)?
                        .iter()
                        .find(|block| str_field(block, "type") == Some("toolCall"))?;
                    let tool_name = str_field(tool, "name").unwrap_or("tool");
                    let input = tool.get("arguments").or_else(|| tool.get("input"));
                    let mut event = tool_call_event(
                        session_id,
                        provider,
                        sequence,
                        msg_type.into(),
                        first_string(&[tool.get("id"), message.get("id")]),
                        (tool_name == "bash")
                            .then(|| input.and_then(tool_input_command))
                            .flatten()
                            .map(str::to_string),
                        None,
                        tool_name,
                        AgentChatStatus::Running,
                    );
                    attach_tool_input_metadata(&mut event, tool_name, input);
                    Some(event)
                }
                "toolResult" | "bashExecution" => Some(event(
                    session_id,
                    provider,
                    sequence,
                    AgentChatEventKind::ToolResult,
                    EventFields {
                        role: Some(AgentChatRole::Tool),
                        text: text_from_value(message),
                        title: str_field(message, "toolName")
                            .or_else(|| str_field(message, "name"))
                            .map(str::to_string),
                        status: Some(
                            if message.get("isError").and_then(Value::as_bool) == Some(true) {
                                AgentChatStatus::Failed
                            } else {
                                AgentChatStatus::Succeeded
                            },
                        ),
                        turn_id: first_string(&[message.get("toolCallId"), message.get("id")]),
                        source: Some(msg_type.into()),
                        metadata: json!({"raw_type": "toolResult"}),
                        ..Default::default()
                    },
                )),
                _ => None,
            }
        }
        "tool_execution_start" => {
            let tool_name = str_field(parsed, "toolName").unwrap_or("tool");
            let input = parsed.get("args").or_else(|| parsed.get("input"));
            let mut event = tool_call_event(
                session_id,
                provider,
                sequence,
                msg_type.into(),
                first_string(&[parsed.get("toolCallId"), parsed.get("id")]),
                input.and_then(tool_input_command).map(str::to_string),
                None,
                tool_name,
                AgentChatStatus::Running,
            );
            attach_tool_input_metadata(&mut event, tool_name, input);
            Some(event)
        }
        "tool_execution_end" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::ToolResult,
            EventFields {
                role: Some(AgentChatRole::Tool),
                text: text_from_value(parsed),
                title: str_field(parsed, "toolName").map(str::to_string),
                status: Some(
                    if parsed.get("isError").and_then(Value::as_bool) == Some(true) {
                        AgentChatStatus::Failed
                    } else {
                        AgentChatStatus::Succeeded
                    },
                ),
                turn_id: first_string(&[parsed.get("toolCallId"), parsed.get("id")]),
                source: Some(msg_type.into()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        _ => None,
    }
}

pub fn visible_chat_text(role: &AgentChatRole, text: &str) -> Option<String> {
    visible_chat_text_impl(role, text, false)
}

fn visible_chat_text_impl(
    role: &AgentChatRole,
    text: &str,
    extract_user_request_from_original_text: bool,
) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut visible = trimmed.to_string();
    visible = remove_tag_block(&visible, "environment_context");
    visible = remove_tag_block(&visible, "ADDITIONAL_METADATA");
    visible = remove_tag_block(&visible, "USER_SETTINGS_CHANGE");
    visible = remove_tag_block(&visible, "subagent_notification");

    // Codex appends this internal provenance block to completed assistant
    // responses. Its lightweight `agent_message` event contains the same
    // visible response without the block, so retaining it would both expose
    // implementation metadata and prevent the two records from deduplicating.
    if *role == AgentChatRole::Assistant {
        visible = remove_tag_block(&visible, "oai-mem-citation");
    }

    if *role == AgentChatRole::User {
        let user_request_source = if extract_user_request_from_original_text {
            trimmed
        } else {
            visible.as_str()
        };
        if let Some(user_request) = extract_tag_block(user_request_source, "USER_REQUEST") {
            visible = user_request;
        }
        // Codex records an image or file prompt twice: once as a provider
        // response item with this inline transport marker and once as the
        // canonical event_msg/user_message. Remove the marker before the
        // shared merge boundary compares the two visible prompts.
        visible = remove_attachment_transport_markers(&visible);
        visible = visible
            .lines()
            .filter(|line| !is_internal_wardian_probe_line(line))
            .collect::<Vec<_>>()
            .join("\n");
    }

    let visible = visible.trim();
    if visible.is_empty() {
        return None;
    }

    Some(visible.to_string())
}

/// Applies provider-specific transport cleanup before common visible-text
/// normalization. The provider argument keeps transport markers scoped to the
/// adapter that emits them.
pub fn visible_chat_text_for_provider(
    provider: &str,
    role: &AgentChatRole,
    text: &str,
) -> Option<String> {
    let text = if provider.eq_ignore_ascii_case("claude") && *role == AgentChatRole::User {
        remove_wardian_delivery_envelope(text)
    } else {
        text.to_string()
    };
    visible_chat_text(role, &text)
}

/// Returns the visible text produced by the pre-provider-normalization
/// algorithm when cleanup changed a Claude user message. Callers use this only
/// to recognize event IDs written by older versions of the provider-log loader.
pub fn legacy_visible_chat_text_for_provider(
    provider: &str,
    role: &AgentChatRole,
    text: &str,
) -> Option<String> {
    if !provider.eq_ignore_ascii_case("claude") || *role != AgentChatRole::User {
        return None;
    }
    let cleaned = remove_wardian_delivery_envelope(text);
    (cleaned != text)
        .then(|| visible_chat_text_impl(role, text, true))
        .flatten()
}

fn remove_wardian_delivery_envelope(text: &str) -> String {
    let Some((header, body)) = text.split_once('\n') else {
        return text.to_string();
    };
    let Some(header_fields) = header.strip_prefix("[Wardian ") else {
        return text.to_string();
    };
    let Some(close_bracket) = header_fields.find(']') else {
        return text.to_string();
    };

    let required_fields = &header_fields[..close_bracket];
    let optional_fields = header_fields[close_bracket + 1..].trim();
    let mut fields = required_fields.split_whitespace();
    let required_keys = ["message_id", "interaction_id", "generation", "target"];
    let has_required_fields = required_keys.iter().all(|expected_key| {
        let Some(field) = fields.next() else {
            return false;
        };
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        key == *expected_key && !value.is_empty()
    }) && fields.next().is_none();
    if !has_required_fields {
        return text.to_string();
    }

    let has_invalid_optional_field = optional_fields.split_whitespace().any(|field| {
        let Some((key, value)) = field.split_once('=') else {
            return true;
        };
        !matches!(key, "sender" | "reply_to" | "deadline") || value.is_empty()
    });
    if has_invalid_optional_field {
        return text.to_string();
    }

    body.to_string()
}

fn remove_attachment_transport_markers(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut remainder = text;

    loop {
        let Some(start) = ["<image ", "<file "]
            .into_iter()
            .filter_map(|tag| remainder.find(tag))
            .min()
        else {
            output.push_str(remainder);
            break;
        };

        output.push_str(&remainder[..start]);
        let Some(end_offset) = remainder[start..].find('>') else {
            output.push_str(&remainder[start..]);
            break;
        };
        let end = start + end_offset + 1;
        let marker = &remainder[start..end];
        let marker_lower = marker.to_ascii_lowercase();
        if !marker_lower.contains(" name=") || !marker_lower.contains(" path=") {
            output.push_str(marker);
        }
        remainder = remainder[end..].trim_start_matches([' ', '\t']);
    }

    output
}

fn is_internal_wardian_probe_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("WARDIAN_") && trimmed.ends_with("_PROBE")
}

fn normalize_codex(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type")?;
    match msg_type {
        "thread.started" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Status,
            EventFields {
                status: Some(AgentChatStatus::Idle),
                turn_id: str_field(parsed, "thread_id").map(str::to_string),
                source: Some(msg_type.to_string()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        "turn.started" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Processing,
            msg_type,
            parsed,
        )),
        "turn.completed" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Succeeded,
            msg_type,
            parsed,
        )),
        "event_msg" => normalize_codex_payload(session_id, provider, parsed, "payload", sequence),
        "response_item" => {
            normalize_codex_payload(session_id, provider, parsed, "payload", sequence)
        }
        "item.completed" => normalize_codex_payload(session_id, provider, parsed, "item", sequence),
        _ => None,
    }
}

fn normalize_codex_payload(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    payload_key: &str,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let payload = parsed.get(payload_key)?;
    let payload_type = str_field(payload, "type")?;
    let source = str_field(parsed, "type").unwrap_or(payload_key).to_string();
    let turn_id = first_string(&[
        parsed.get("turn_id"),
        payload.get("turn_id"),
        payload.get("call_id"),
        payload.get("id"),
    ]);

    match payload_type {
        "user_message" => message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::User,
            text_from_value(payload)?,
            source,
            turn_id,
            payload_type,
        ),
        "agent_message" | "assistant_message" => message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::Assistant,
            text_from_value(payload)?,
            source,
            turn_id,
            payload_type,
        ),
        "message" => {
            let role = role_from_str(str_field(payload, "role")?)?;
            if codex_response_item_user_context(payload, &source, &role) {
                let mut metadata = json!({
                    "raw_type": payload_type,
                    "input_origin": "context_injection",
                    "input_purpose": "context",
                    "context_observation": "provider_native",
                });
                if let Some(turn_id) = turn_id.as_deref() {
                    set_metadata_string(
                        &mut metadata,
                        "causal_ref",
                        &format!("provider:message:{turn_id}"),
                    );
                }
                if let Some(provider_turn_id) = codex_provider_turn_id(payload) {
                    set_metadata_string(&mut metadata, "provider_turn_id", &provider_turn_id);
                }
                return message_event_with_metadata(
                    session_id,
                    provider,
                    sequence,
                    role,
                    text_from_value(payload)?,
                    source,
                    turn_id,
                    metadata,
                );
            }
            message_event(
                session_id,
                provider,
                sequence,
                role,
                text_from_value(payload)?,
                source,
                turn_id,
                payload_type,
            )
        }
        "task_started" | "exec_command_begin" | "exec_command_start" => Some(tool_call_event(
            session_id,
            provider,
            sequence,
            source,
            turn_id,
            str_field(payload, "command").map(str::to_string),
            None,
            payload_type,
            AgentChatStatus::Running,
        )),
        "exec_approval_request" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Approval,
            EventFields {
                text: text_from_value(payload)
                    .or_else(|| str_field(payload, "command").map(str::to_string)),
                title: Some("Approval required".to_string()),
                status: Some(AgentChatStatus::ActionRequired),
                turn_id,
                source: Some(source),
                command: str_field(payload, "command").map(str::to_string),
                metadata: json!({"raw_type": payload_type}),
                ..Default::default()
            },
        )),
        "task_complete" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Succeeded,
            payload_type,
            payload,
        )),
        "function_call" | "custom_tool_call" => {
            let arguments = codex_tool_call_input(payload);
            let tool_name = str_field(payload, "name").unwrap_or(payload_type);
            let raw_input_text = codex_tool_call_raw_input_text(payload);
            let command = arguments
                .as_ref()
                .and_then(tool_input_command)
                .map(str::to_string);
            let needs_approval = arguments.as_ref().is_some_and(|value| {
                str_field(value, "sandbox_permissions") == Some("require_escalated")
            });
            let text = arguments
                .as_ref()
                .and_then(|value| str_field(value, "justification").map(str::to_string));
            let mut metadata = json!({"raw_type": payload_type, "tool_name": tool_name});
            if let Some(input_text) = raw_input_text {
                metadata["tool_input_text"] = json!(input_text);
            }
            let mut tool_event = event(
                session_id,
                provider,
                sequence,
                AgentChatEventKind::ToolCall,
                EventFields {
                    text,
                    title: Some(tool_name.to_string()),
                    status: Some(if needs_approval {
                        AgentChatStatus::ActionRequired
                    } else {
                        AgentChatStatus::Running
                    }),
                    turn_id,
                    source: Some(source),
                    command: command.clone(),
                    language: command.as_ref().map(|_| "shell".to_string()),
                    metadata,
                    ..Default::default()
                },
            );
            attach_tool_input_metadata(&mut tool_event, tool_name, arguments.as_ref());
            Some(tool_event)
        }
        "function_call_output" | "custom_tool_call_output" => {
            let raw_text = text_from_value(payload);
            let subagent_summary = raw_text.as_deref().and_then(subagent_completion_summary);
            Some(event(
                session_id,
                provider,
                sequence,
                AgentChatEventKind::ToolResult,
                EventFields {
                    role: Some(AgentChatRole::Tool),
                    text: subagent_summary.clone().or(raw_text),
                    title: subagent_summary
                        .as_ref()
                        .map(|_| "Subagent completed".to_string()),
                    status: Some(AgentChatStatus::Succeeded),
                    turn_id,
                    source: Some(source),
                    metadata: json!({"raw_type": payload_type}),
                    ..Default::default()
                },
            ))
        }
        _ => None,
    }
}

fn normalize_claude(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type")?;
    match msg_type {
        "system" => normalize_claude_system(session_id, provider, parsed, sequence),
        "user" => match classify_claude_user_event(parsed) {
            ClaudeUserEventKind::RealQuery => {
                let message = parsed.get("message").unwrap_or(parsed);
                let mut event = message_event(
                    session_id,
                    provider,
                    sequence,
                    AgentChatRole::User,
                    text_from_value(message)?,
                    "stream_json".to_string(),
                    turn_id_from(message).or_else(|| turn_id_from(parsed)),
                    msg_type,
                )?;
                if let Some(causal_ref) = claude_provider_causal_ref(parsed) {
                    set_metadata_string(&mut event.metadata, "causal_ref", &causal_ref);
                }
                set_metadata_string(
                    &mut event.metadata,
                    "context_observation",
                    "provider_native",
                );
                Some(event)
            }
            ClaudeUserEventKind::ContextInjection => {
                let message = parsed.get("message").unwrap_or(parsed);
                let mut metadata = json!({
                    "raw_type": msg_type,
                    "input_origin": "context_injection",
                    "input_purpose": claude_context_purpose(parsed),
                    "context_observation": "provider_native",
                });
                if let Some(causal_ref) = claude_provider_causal_ref(parsed) {
                    set_metadata_string(&mut metadata, "causal_ref", &causal_ref);
                }
                message_event_with_metadata(
                    session_id,
                    provider,
                    sequence,
                    // Claude labels native context as a `user` message in
                    // stream-json, but it is provider-supplied context, not
                    // an operator prompt. Keep the provenance in metadata
                    // and use the system role so consumers do not split the
                    // transcript into a false turn.
                    AgentChatRole::System,
                    text_from_value(message)?,
                    "stream_json".to_string(),
                    turn_id_from(message).or_else(|| turn_id_from(parsed)),
                    metadata,
                )
            }
            ClaudeUserEventKind::ProviderInternal => {
                let message = parsed.get("message").unwrap_or(parsed);
                let mut metadata = json!({
                    "raw_type": msg_type,
                    "input_origin": "provider_internal",
                    "input_purpose": "internal",
                    "context_observation": "provider_native",
                });
                if let Some(causal_ref) = claude_provider_causal_ref(parsed) {
                    set_metadata_string(&mut metadata, "causal_ref", &causal_ref);
                }
                message_event_with_metadata(
                    session_id,
                    provider,
                    sequence,
                    // Interruption markers and other provider-internal
                    // messages must not participate in user-prompt matching
                    // or turn-boundary detection.
                    AgentChatRole::System,
                    text_from_value(message)?,
                    "stream_json".to_string(),
                    turn_id_from(message).or_else(|| turn_id_from(parsed)),
                    metadata,
                )
            }
            ClaudeUserEventKind::ToolResult => {
                let item = content_array(parsed.get("message").unwrap_or(parsed))?
                    .iter()
                    .find(|item| str_field(item, "type") == Some("tool_result"))?;
                Some(event(
                    session_id,
                    provider,
                    sequence,
                    AgentChatEventKind::ToolResult,
                    EventFields {
                        role: Some(AgentChatRole::Tool),
                        text: text_from_value(item),
                        status: Some(AgentChatStatus::Succeeded),
                        turn_id: str_field(item, "tool_use_id").map(str::to_string),
                        source: Some("stream_json".to_string()),
                        metadata: json!({"raw_type": "tool_result"}),
                        ..Default::default()
                    },
                ))
            }
            ClaudeUserEventKind::LocalCommand | ClaudeUserEventKind::Ignored => None,
        },
        "assistant" => normalize_claude_assistant(session_id, provider, parsed, sequence),
        "message_stream" | "progress" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Processing,
            msg_type,
            parsed,
        )),
        "result" => {
            let status = match str_field(parsed, "subtype").or_else(|| str_field(parsed, "status"))
            {
                Some("error") | Some("failed") => AgentChatStatus::Failed,
                Some("cancelled") | Some("canceled") => AgentChatStatus::Cancelled,
                _ => AgentChatStatus::Succeeded,
            };
            Some(event(
                session_id,
                provider,
                sequence,
                AgentChatEventKind::Status,
                EventFields {
                    text: text_from_value(parsed),
                    status: Some(status),
                    turn_id: turn_id_from(parsed),
                    source: Some("stream_json".to_string()),
                    metadata: json!({"raw_type": msg_type}),
                    ..Default::default()
                },
            ))
        }
        _ => None,
    }
}

fn normalize_claude_system(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    match str_field(parsed, "subtype")? {
        "init" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Status,
            EventFields {
                title: Some("Initialized".to_string()),
                status: Some(AgentChatStatus::Idle),
                turn_id: turn_id_from(parsed),
                source: Some("stream_json".to_string()),
                created_at: str_field(parsed, "timestamp").map(str::to_string),
                metadata: json!({"raw_type": "system", "subtype": "init"}),
                ..Default::default()
            },
        )),
        "permission_request" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Approval,
            EventFields {
                text: text_from_value(parsed),
                title: str_field(parsed, "tool_name")
                    .map(str::to_string)
                    .or_else(|| Some("Tool approval required".to_string())),
                status: Some(AgentChatStatus::ActionRequired),
                turn_id: turn_id_from(parsed),
                source: Some("stream_json".to_string()),
                command: str_field(parsed, "command").map(str::to_string),
                metadata: json!({"raw_type": "system", "subtype": "permission_request"}),
                ..Default::default()
            },
        )),
        "turn_duration" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Succeeded,
            "turn_duration",
            parsed,
        )),
        _ => None,
    }
}

fn normalize_claude_assistant(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let message = parsed.get("message").unwrap_or(parsed);
    if let Some(tool_use) = content_array(message).and_then(|items| {
        items
            .iter()
            .find(|item| str_field(item, "type") == Some("tool_use"))
    }) {
        let tool_name = str_field(tool_use, "name").unwrap_or("tool_use");
        let command = tool_use
            .get("input")
            .and_then(|input| str_field(input, "command").map(str::to_string));
        let mut event = tool_call_event(
            session_id,
            provider,
            sequence,
            "stream_json".to_string(),
            str_field(tool_use, "id")
                .map(str::to_string)
                .or_else(|| turn_id_from(message)),
            command,
            text_from_value(tool_use),
            tool_name,
            AgentChatStatus::Running,
        );
        event.metadata["tool_name"] = json!(tool_name);
        if let Some(input) = tool_use.get("input") {
            event.metadata["tool_input"] = input.clone();
            if let Some(file_path) = str_field(input, "file_path") {
                event.metadata["file_path"] = json!(file_path);
                if claude_tool_reads_file(tool_name) {
                    event.metadata["files_read"] = json!([file_path]);
                } else if claude_tool_writes_file(tool_name) {
                    event.metadata["files_written"] = json!([file_path]);
                }
            }
        }
        return Some(event);
    }

    if let Some(text) = text_from_value(message) {
        return message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::Assistant,
            text,
            "stream_json".to_string(),
            turn_id_from(message).or_else(|| turn_id_from(parsed)),
            "assistant",
        );
    }

    match str_field(message, "stop_reason") {
        Some("end_turn") | Some("stop_sequence") => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Succeeded,
            "assistant",
            message,
        )),
        Some("tool_use") => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Processing,
            "assistant",
            message,
        )),
        _ => None,
    }
}

fn claude_tool_reads_file(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("Read")
}

fn claude_tool_writes_file(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "edit" | "write" | "multiedit" | "notebookedit"
    )
}

fn normalize_gemini(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type").or_else(|| str_field(parsed, "role"))?;
    match msg_type {
        "init" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Status,
            EventFields {
                title: Some("Initialized".to_string()),
                status: Some(AgentChatStatus::Idle),
                turn_id: turn_id_from(parsed),
                source: Some("gemini_log".to_string()),
                created_at: str_field(parsed, "timestamp").map(str::to_string),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        "user" => message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::User,
            text_from_value(parsed)?,
            "gemini_log".to_string(),
            turn_id_from(parsed),
            msg_type,
        ),
        "message" => {
            let role = role_from_str(str_field(parsed, "role")?)?;
            if role == AgentChatRole::User {
                return message_event(
                    session_id,
                    provider,
                    sequence,
                    role,
                    text_from_value(parsed)?,
                    "gemini_log".to_string(),
                    turn_id_from(parsed),
                    msg_type,
                );
            }
            if !gemini_completed_message(parsed) {
                return None;
            }
            message_event(
                session_id,
                provider,
                sequence,
                role,
                text_from_value(parsed)?,
                "gemini_log".to_string(),
                turn_id_from(parsed),
                msg_type,
            )
        }
        "gemini" | "model" | "assistant" => {
            if !gemini_completed_message(parsed) {
                return None;
            }
            message_event(
                session_id,
                provider,
                sequence,
                AgentChatRole::Assistant,
                text_from_value(parsed)?,
                "gemini_log".to_string(),
                turn_id_from(parsed),
                msg_type,
            )
        }
        "tool_use" => {
            let tool_name = str_field(parsed, "tool_name").unwrap_or("tool_use");
            let mut tool = tool_call_event(
                session_id,
                provider,
                sequence,
                "gemini_log".to_string(),
                turn_id_from(parsed),
                str_field(parsed, "command").map(str::to_string),
                text_from_value(parsed),
                tool_name,
                AgentChatStatus::ActionRequired,
            );
            attach_tool_input_metadata(
                &mut tool,
                tool_name,
                parsed.get("args").or_else(|| parsed.get("input")),
            );
            Some(tool)
        }
        "tool_result" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::ToolResult,
            EventFields {
                role: Some(AgentChatRole::Tool),
                text: text_from_value(parsed),
                status: Some(
                    status_from_str(str_field(parsed, "status"))
                        .unwrap_or(AgentChatStatus::Succeeded),
                ),
                turn_id: turn_id_from(parsed),
                source: Some("gemini_log".to_string()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        "result" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Status,
            EventFields {
                text: text_from_value(parsed),
                status: Some(
                    status_from_str(str_field(parsed, "status"))
                        .unwrap_or(AgentChatStatus::Unknown),
                ),
                turn_id: turn_id_from(parsed),
                source: Some("gemini_log".to_string()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        _ => None,
    }
}

fn normalize_antigravity(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type")?;
    match msg_type {
        "USER_INPUT" => message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::User,
            text_from_value(parsed)?,
            "transcript".to_string(),
            step_index(parsed),
            msg_type,
        ),
        "PLANNER_RESPONSE" => {
            if str_field(parsed, "source") != Some("MODEL") {
                return None;
            }
            if let Some(tool_call) =
                antigravity_tool_call_event(session_id, provider, parsed, sequence)
            {
                return Some(tool_call);
            }
            if str_field(parsed, "status") != Some("DONE") {
                return Some(status_event(
                    session_id,
                    provider,
                    sequence,
                    AgentChatStatus::Processing,
                    msg_type,
                    parsed,
                ));
            }
            message_event(
                session_id,
                provider,
                sequence,
                AgentChatRole::Assistant,
                text_from_value(parsed)?,
                "transcript".to_string(),
                step_index(parsed),
                msg_type,
            )
        }
        "ASK_QUESTION" | "CODE_ACTION" | "GENERIC" | "GREP_SEARCH" | "LIST_DIRECTORY"
        | "READ_URL_CONTENT" | "RUN_COMMAND" | "SEARCH_WEB" | "VIEW_FILE" => {
            antigravity_tool_result_event(session_id, provider, parsed, sequence, msg_type)
        }
        _ => None,
    }
}

fn antigravity_tool_call_event(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let tool_call = parsed
        .get("tool_calls")
        .and_then(|value| value.as_array())
        .and_then(|items| items.iter().find(|item| item.is_object()))?;
    let tool_name = str_field(tool_call, "name").unwrap_or("tool_call");
    let args = tool_call.get("args");
    let file_paths = antigravity_tool_arg_paths(args);
    let metadata = antigravity_tool_metadata(tool_name, args, &file_paths);
    let command = args
        .and_then(|value| value.get("command"))
        .or_else(|| args.and_then(|value| value.get("CommandLine")))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let text = command.clone().or_else(|| args.and_then(compact_json_text));

    Some(tool_call_event_with_metadata(
        session_id,
        provider,
        sequence,
        tool_name.to_string(),
        step_index(parsed),
        command,
        text,
        antigravity_tool_title(tool_name),
        AgentChatStatus::Running,
        metadata,
    ))
}

fn antigravity_tool_metadata(
    tool_name: &str,
    args: Option<&Value>,
    file_paths: &[String],
) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("raw_type".to_string(), Value::String(tool_name.to_string()));
    metadata.insert(
        "tool_name".to_string(),
        Value::String(tool_name.to_string()),
    );
    if let Some(args) = args {
        metadata.insert("tool_input".to_string(), args.clone());
    }
    if let Some(first_path) = file_paths.first() {
        metadata.insert("file_path".to_string(), Value::String(first_path.clone()));
        let file_list_key = if antigravity_tool_reads_file(tool_name) {
            Some("files_read")
        } else if antigravity_tool_writes_file(tool_name) {
            Some("files_written")
        } else {
            None
        };
        if let Some(file_list_key) = file_list_key {
            metadata.insert(
                file_list_key.to_string(),
                Value::Array(
                    file_paths
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect::<Vec<_>>(),
                ),
            );
        }
    }
    Value::Object(metadata)
}

fn antigravity_tool_arg_paths(args: Option<&Value>) -> Vec<String> {
    let mut paths = Vec::new();
    let Some(args) = args.and_then(Value::as_object) else {
        return paths;
    };
    for key in [
        "AbsolutePath",
        "TargetFile",
        "FilePath",
        "file_path",
        "path",
        "uri",
        "fileUri",
    ] {
        if let Some(path) = args.get(key).and_then(tool_arg_string) {
            push_unique(&mut paths, path);
        }
    }
    paths
}

fn tool_arg_string(value: &Value) -> Option<String> {
    let mut text = value.as_str()?.trim().to_string();
    for _ in 0..3 {
        let trimmed = text.trim();
        if !(trimmed.starts_with('"') && trimmed.ends_with('"')) {
            break;
        }
        let Ok(decoded) = serde_json::from_str::<String>(trimmed) else {
            break;
        };
        text = decoded;
    }
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn antigravity_tool_reads_file(tool_name: &str) -> bool {
    matches!(tool_name.to_ascii_lowercase().as_str(), "view_file")
}

fn antigravity_tool_writes_file(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "write_to_file" | "replace_file_content" | "multi_replace_file_content"
    )
}

fn antigravity_tool_result_event(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
    msg_type: &str,
) -> Option<AgentChatEvent> {
    Some(event(
        session_id,
        provider,
        sequence,
        AgentChatEventKind::ToolResult,
        EventFields {
            text: text_from_value(parsed),
            title: Some(antigravity_tool_title(msg_type).to_string()),
            status: status_from_str(str_field(parsed, "status")).or(Some(AgentChatStatus::Unknown)),
            turn_id: step_index(parsed),
            source: Some(msg_type.to_string()),
            created_at: created_at(parsed),
            metadata: json!({"raw_type": msg_type}),
            ..Default::default()
        },
    ))
}

fn antigravity_tool_title(tool_name: &str) -> &'static str {
    match tool_name.to_ascii_uppercase().as_str() {
        "ASK_QUESTION" => "Ask question",
        "CODE_ACTION" => "Code action",
        "GENERIC" => "Generic action",
        "GREP_SEARCH" => "Search files",
        "LIST_DIRECTORY" => "List directory",
        "READ_URL_CONTENT" => "Read URL",
        "RUN_COMMAND" => "Run command",
        "SEARCH_WEB" => "Search web",
        "VIEW_FILE" => "View file",
        "WRITE_TO_FILE" => "Write file",
        "REPLACE_FILE_CONTENT" | "MULTI_REPLACE_FILE_CONTENT" => "Edit file",
        _ => "Tool call",
    }
}

fn normalize_opencode(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type")?;
    match msg_type {
        "step_start" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Processing,
            msg_type,
            parsed,
        )),
        "text" => {
            let part = parsed.get("part").unwrap_or(parsed);
            message_event(
                session_id,
                provider,
                sequence,
                AgentChatRole::Assistant,
                text_from_value(part)?,
                "stream_json".to_string(),
                turn_id_from(parsed),
                msg_type,
            )
        }
        "tool_use" => {
            let part = parsed.get("part").unwrap_or(parsed);
            let tool_name = str_field(part, "name")
                .or_else(|| str_field(parsed, "tool"))
                .unwrap_or("tool_use");
            let mut event = tool_call_event(
                session_id,
                provider,
                sequence,
                "stream_json".to_string(),
                turn_id_from(parsed),
                str_field(part, "command").map(str::to_string).or_else(|| {
                    part.get("input")
                        .and_then(|input| str_field(input, "command").map(str::to_string))
                }),
                text_from_value(part),
                tool_name,
                AgentChatStatus::Running,
            );
            attach_tool_input_metadata(&mut event, tool_name, part.get("input"));
            Some(event)
        }
        "tool_result" => {
            let part = parsed.get("part").unwrap_or(parsed);
            Some(event(
                session_id,
                provider,
                sequence,
                AgentChatEventKind::ToolResult,
                EventFields {
                    role: Some(AgentChatRole::Tool),
                    text: text_from_value(part),
                    status: Some(AgentChatStatus::Succeeded),
                    turn_id: turn_id_from(parsed),
                    source: Some("stream_json".to_string()),
                    metadata: json!({"raw_type": msg_type}),
                    ..Default::default()
                },
            ))
        }
        "step_finish" => {
            let reason = parsed
                .get("part")
                .and_then(|part| str_field(part, "reason"))
                .unwrap_or("");
            let status = match reason {
                "stop" => AgentChatStatus::Succeeded,
                "tool-calls" => AgentChatStatus::Processing,
                _ => AgentChatStatus::Unknown,
            };
            Some(status_event(
                session_id, provider, sequence, status, msg_type, parsed,
            ))
        }
        "error" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Error,
            EventFields {
                text: text_from_value(parsed),
                status: Some(AgentChatStatus::Failed),
                turn_id: turn_id_from(parsed),
                source: Some("stream_json".to_string()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        _ => None,
    }
}

/// Preserves a tool call's structured input on the normalized event.
///
/// Several providers describe a file edit only in the tool's input object and
/// emit no patch text at all, so discarding the input discards the change
/// itself. Path keys are scanned rather than assumed because providers disagree
/// on casing; the same list backs turn attribution in
/// `state/conversation_archive/turns.rs`.
fn attach_tool_input_metadata(event: &mut AgentChatEvent, tool_name: &str, input: Option<&Value>) {
    event.metadata["tool_name"] = json!(tool_name);

    let Some(input) = input.filter(|value| value.is_object()) else {
        return;
    };
    event.metadata["tool_input"] = input.clone();

    if event.command.is_none() {
        if let Some(command) = tool_input_command(input) {
            event.command = Some(command.to_string());
            event.language = Some("shell".to_string());
        }
    }

    let Some(path) = tool_input_file_path(input) else {
        return;
    };
    event.metadata["file_path"] = json!(path);
    if generic_tool_reads_file(tool_name) {
        event.metadata["files_read"] = json!([path]);
    } else if generic_tool_writes_file(tool_name) {
        event.metadata["files_written"] = json!([path]);
    }
}

/// Returns the command-like argument used by shell tools across providers.
///
/// Codex's current `exec` tool calls use `cmd`, while older and other provider
/// payloads use `command`. Keep the alias handling at normalization time so
/// every client receives the same canonical `AgentChatEvent.command` field.
fn tool_input_command(input: &Value) -> Option<&str> {
    ["command", "cmd", "CommandLine", "script"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|command| !command.is_empty())
}

fn tool_input_file_path(input: &Value) -> Option<String> {
    [
        "file_path",
        "filePath",
        "AbsolutePath",
        "TargetFile",
        "FilePath",
        "path",
        "uri",
        "fileUri",
    ]
    .iter()
    .find_map(|key| input.get(key).and_then(tool_arg_string))
}

fn generic_tool_reads_file(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "read" | "read_file" | "readfile" | "view" | "view_file"
    )
}

fn generic_tool_writes_file(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "edit"
            | "write"
            | "multiedit"
            | "multi_edit"
            | "notebookedit"
            | "patch"
            | "apply_patch"
            | "create_file"
            | "write_file"
    )
}

fn normalize_mock(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    sequence: u64,
) -> Option<AgentChatEvent> {
    let msg_type = str_field(parsed, "type")?;
    match msg_type {
        "init" => Some(status_event(
            session_id,
            provider,
            sequence,
            AgentChatStatus::Idle,
            msg_type,
            parsed,
        )),
        "user" => message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::User,
            text_from_value(parsed)?,
            msg_type.to_string(),
            turn_id_from(parsed),
            msg_type,
        ),
        "model" | "info" => message_event(
            session_id,
            provider,
            sequence,
            AgentChatRole::Assistant,
            text_from_value(parsed)?,
            msg_type.to_string(),
            turn_id_from(parsed),
            msg_type,
        ),
        "message" => message_event(
            session_id,
            provider,
            sequence,
            role_from_str(str_field(parsed, "role")?)?,
            text_from_value(parsed)?,
            msg_type.to_string(),
            turn_id_from(parsed),
            msg_type,
        ),
        "result" => Some(status_event(
            session_id,
            provider,
            sequence,
            status_from_str(str_field(parsed, "status")).unwrap_or(AgentChatStatus::Succeeded),
            msg_type,
            parsed,
        )),
        // The mock provider is the only offline way to exercise the chat
        // transcript's tool surface, so its tool calls carry real structured
        // input rather than a summary string.
        "tool_call" => {
            let tool_name = str_field(parsed, "tool_name").unwrap_or("tool_call");
            let mut tool = tool_call_event(
                session_id,
                provider,
                sequence,
                msg_type.to_string(),
                turn_id_from(parsed),
                str_field(parsed, "command").map(str::to_string),
                None,
                tool_name,
                AgentChatStatus::Running,
            );
            attach_tool_input_metadata(&mut tool, tool_name, parsed.get("input"));
            Some(tool)
        }
        "tool_result" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::ToolResult,
            EventFields {
                role: Some(AgentChatRole::Tool),
                text: text_from_value(parsed),
                title: str_field(parsed, "tool_name").map(str::to_string),
                status: Some(
                    status_from_str(str_field(parsed, "status"))
                        .unwrap_or(AgentChatStatus::Succeeded),
                ),
                turn_id: turn_id_from(parsed),
                source: Some(msg_type.to_string()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        "action_required" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::Approval,
            EventFields {
                text: text_from_value(parsed),
                title: Some("Action required".to_string()),
                status: Some(AgentChatStatus::ActionRequired),
                turn_id: turn_id_from(parsed),
                source: Some(msg_type.to_string()),
                metadata: json!({"raw_type": msg_type}),
                ..Default::default()
            },
        )),
        _ => None,
    }
}

fn normalize_fallback_json(
    session_id: &str,
    provider: &str,
    parsed: &Value,
    raw_line: &str,
    sequence: u64,
) -> Option<AgentChatEvent> {
    if let Some(role) = str_field(parsed, "role").and_then(role_from_str) {
        return message_event(
            session_id,
            provider,
            sequence,
            role,
            text_from_value(parsed)?,
            "json".to_string(),
            turn_id_from(parsed),
            str_field(parsed, "type").unwrap_or("message"),
        );
    }

    fallback_terminal_event(session_id, provider, raw_line, sequence)
}

fn fallback_terminal_event(
    session_id: &str,
    provider: &str,
    raw_line: &str,
    sequence: u64,
) -> Option<AgentChatEvent> {
    if matches!(
        provider,
        "codex" | "claude" | "gemini" | "antigravity" | "opencode" | "pi" | "mock"
    ) {
        return None;
    }

    Some(event(
        session_id,
        provider,
        sequence,
        AgentChatEventKind::TerminalOutput,
        EventFields {
            text: Some(raw_line.to_string()),
            source: Some("terminal".to_string()),
            metadata: json!({}),
            ..Default::default()
        },
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps provider parser call sites readable while centralizing DTO defaults"
)]
fn message_event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    role: AgentChatRole,
    text: String,
    source: String,
    turn_id: Option<String>,
    raw_type: &str,
) -> Option<AgentChatEvent> {
    let mut metadata = json!({"raw_type": raw_type});
    match &role {
        AgentChatRole::User => {
            set_metadata_string(&mut metadata, "input_origin", "human_input");
            set_metadata_string(&mut metadata, "input_purpose", "request");
            set_metadata_string(
                &mut metadata,
                "context_observation",
                if provider.eq_ignore_ascii_case("claude") || provider.eq_ignore_ascii_case("codex")
                {
                    "provider_native"
                } else {
                    "unreported"
                },
            );
            if let Some(turn_id) = turn_id.as_deref() {
                set_metadata_string(&mut metadata, "request_root_id", turn_id);
            }
        }
        AgentChatRole::System => {
            set_metadata_string(&mut metadata, "input_origin", "provider_internal");
            set_metadata_string(&mut metadata, "input_purpose", "internal");
        }
        AgentChatRole::Assistant | AgentChatRole::Tool => {}
    }
    message_event_with_metadata(
        session_id, provider, sequence, role, text, source, turn_id, metadata,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps provider parser call sites readable while centralizing DTO defaults"
)]
fn message_event_with_metadata(
    session_id: &str,
    provider: &str,
    sequence: u64,
    role: AgentChatRole,
    text: String,
    source: String,
    turn_id: Option<String>,
    metadata: Value,
) -> Option<AgentChatEvent> {
    let text = visible_chat_text_for_provider(provider, &role, &text)?;
    Some(event(
        session_id,
        provider,
        sequence,
        AgentChatEventKind::Message,
        EventFields {
            role: Some(role),
            text: Some(text),
            turn_id,
            source: Some(source),
            metadata,
            ..Default::default()
        },
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps provider parser call sites readable while centralizing DTO defaults"
)]
fn tool_call_event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    source: String,
    turn_id: Option<String>,
    command: Option<String>,
    text: Option<String>,
    title: &str,
    status: AgentChatStatus,
) -> AgentChatEvent {
    tool_call_event_with_metadata(
        session_id,
        provider,
        sequence,
        source,
        turn_id,
        command,
        text,
        title,
        status,
        json!({"raw_type": title}),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps provider parser call sites readable while centralizing DTO defaults"
)]
fn tool_call_event_with_metadata(
    session_id: &str,
    provider: &str,
    sequence: u64,
    source: String,
    turn_id: Option<String>,
    command: Option<String>,
    text: Option<String>,
    title: &str,
    status: AgentChatStatus,
    metadata: Value,
) -> AgentChatEvent {
    let language = command.as_ref().map(|_| "shell".to_string());
    event(
        session_id,
        provider,
        sequence,
        AgentChatEventKind::ToolCall,
        EventFields {
            text,
            title: Some(title.to_string()),
            status: Some(status),
            turn_id,
            source: Some(source),
            command,
            language,
            metadata,
            ..Default::default()
        },
    )
}

fn status_event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    status: AgentChatStatus,
    raw_type: &str,
    parsed: &Value,
) -> AgentChatEvent {
    event(
        session_id,
        provider,
        sequence,
        AgentChatEventKind::Status,
        EventFields {
            text: text_from_value(parsed),
            status: Some(status),
            turn_id: turn_id_from(parsed).or_else(|| step_index(parsed)),
            source: Some(raw_type.to_string()),
            created_at: created_at(parsed),
            metadata: json!({"raw_type": raw_type}),
            ..Default::default()
        },
    )
}

struct EventFields {
    role: Option<AgentChatRole>,
    text: Option<String>,
    title: Option<String>,
    status: Option<AgentChatStatus>,
    turn_id: Option<String>,
    source: Option<String>,
    command: Option<String>,
    exit_code: Option<i32>,
    path: Option<String>,
    language: Option<String>,
    created_at: Option<String>,
    metadata: Value,
}

impl Default for EventFields {
    fn default() -> Self {
        Self {
            role: None,
            text: None,
            title: None,
            status: None,
            turn_id: None,
            source: None,
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: None,
            metadata: json!({}),
        }
    }
}

fn event(
    session_id: &str,
    provider: &str,
    sequence: u64,
    kind: AgentChatEventKind,
    fields: EventFields,
) -> AgentChatEvent {
    AgentChatEvent {
        id: format!("{session_id}:{sequence}"),
        session_id: session_id.to_string(),
        provider: provider.to_string(),
        kind,
        role: fields.role,
        text: fields.text,
        title: fields.title,
        status: fields.status,
        turn_id: fields.turn_id,
        source: fields.source,
        command: fields.command,
        exit_code: fields.exit_code,
        path: fields.path,
        language: fields.language,
        created_at: fields.created_at,
        sequence: Some(sequence),
        metadata: fields.metadata,
    }
}

fn text_from_value(value: &Value) -> Option<String> {
    for key in ["text", "content", "message", "summary", "result", "output"] {
        match value.get(key) {
            Some(Value::String(text)) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            Some(Value::Array(items)) => {
                let text = text_from_array(items);
                if text.is_some() {
                    return text;
                }
            }
            _ => {}
        }
    }

    None
}

fn compact_json_text(value: &Value) -> Option<String> {
    let text = serde_json::to_string(value).ok()?;
    (!text.trim().is_empty() && text != "null").then_some(text)
}

fn text_from_array(items: &[Value]) -> Option<String> {
    let parts = items
        .iter()
        .filter_map(|item| match item {
            Value::String(text) => Some(text.trim()),
            Value::Object(_) => item
                .get("text")
                .or_else(|| item.get("content"))
                .and_then(|value| value.as_str())
                .map(str::trim),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();

    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn extract_tag_block(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim().to_string())
}

fn remove_tag_block(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut rest = text;
    let mut output = String::new();

    while let Some(start) = rest.find(&open) {
        output.push_str(&rest[..start]);
        let after_open = start + open.len();
        if let Some(end) = rest[after_open..].find(&close) {
            rest = &rest[after_open + end + close.len()..];
        } else {
            rest = &rest[after_open..];
            break;
        }
    }

    output.push_str(rest);
    output
}

fn subagent_completion_summary(text: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(text).ok()?;
    let status = parsed.get("status")?.as_object()?;
    status.values().find_map(|entry| {
        entry
            .get("completed")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn content_array(value: &Value) -> Option<&Vec<Value>> {
    value.get("content").and_then(|content| content.as_array())
}

fn gemini_completed_message(value: &Value) -> bool {
    value.get("tokens").is_some()
        || value.get("usage").is_some()
        || value.get("finishReason").is_some()
        || value.get("finish_reason").is_some()
        || value.get("is_final").and_then(|value| value.as_bool()) == Some(true)
}

fn role_from_str(value: &str) -> Option<AgentChatRole> {
    match value.to_ascii_lowercase().as_str() {
        "user" => Some(AgentChatRole::User),
        "assistant" | "model" => Some(AgentChatRole::Assistant),
        "system" => Some(AgentChatRole::System),
        "tool" => Some(AgentChatRole::Tool),
        _ => None,
    }
}

fn status_from_str(value: Option<&str>) -> Option<AgentChatStatus> {
    match value?.to_ascii_lowercase().as_str() {
        "running" => Some(AgentChatStatus::Running),
        "success" | "succeeded" | "done" | "completed" => Some(AgentChatStatus::Succeeded),
        "failure" | "failed" | "error" => Some(AgentChatStatus::Failed),
        "action_required" | "action needed" | "approval_required" => {
            Some(AgentChatStatus::ActionRequired)
        }
        "cancelled" | "canceled" => Some(AgentChatStatus::Cancelled),
        "idle" => Some(AgentChatStatus::Idle),
        "processing" | "working" => Some(AgentChatStatus::Processing),
        "unknown" => Some(AgentChatStatus::Unknown),
        _ => None,
    }
}

fn normalize_provider(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn set_metadata_string(metadata: &mut Value, key: &str, value: &str) {
    if !metadata.is_object() {
        *metadata = json!({});
    }
    metadata[key] = json!(value);
}

fn codex_tool_call_input(payload: &Value) -> Option<Value> {
    json_object_or_encoded_json(payload.get("arguments"))
        .or_else(|| json_object_or_encoded_json(payload.get("input")))
}

fn codex_provider_turn_id(payload: &Value) -> Option<String> {
    payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|value| str_field(value, "turn_id"))
        .map(str::to_string)
}

fn codex_response_item_user_context(payload: &Value, source: &str, role: &AgentChatRole) -> bool {
    // Codex emits provider-supplied host context as a response_item message
    // with batched content. The canonical human prompt is the separate
    // event_msg/user_message record, so this boundary is structural rather
    // than based on the injected text.
    source == "response_item"
        && matches!(role, AgentChatRole::User)
        && matches!(payload.get("content"), Some(Value::Array(_)))
}

fn codex_tool_call_raw_input_text(payload: &Value) -> Option<String> {
    for key in ["input", "arguments"] {
        let Some(Value::String(raw)) = payload.get(key) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() || parse_json_string(trimmed).is_some() {
            continue;
        }
        return Some(trimmed.to_string());
    }
    None
}

fn json_object_or_encoded_json(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(_)) => value.cloned(),
        Some(Value::String(raw)) => parse_json_string(raw),
        _ => None,
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|value| value.as_str())
}

fn first_string(values: &[Option<&Value>]) -> Option<String> {
    values
        .iter()
        .filter_map(|value| value.and_then(value_to_string))
        .next()
}

fn value_to_string(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    value.as_u64().map(|number| number.to_string())
}

fn turn_id_from(value: &Value) -> Option<String> {
    first_string(&[
        value.get("turn_id"),
        value.get("id"),
        value.get("message_id"),
        value.get("uuid"),
        value.get("sessionID"),
        value.get("session_id"),
        value.get("request_id"),
        value.get("call_id"),
        value.get("tool_use_id"),
    ])
}

fn step_index(value: &Value) -> Option<String> {
    value.get("step_index").and_then(value_to_string)
}

fn created_at(value: &Value) -> Option<String> {
    first_string(&[
        value.get("created_at"),
        value.get("timestamp"),
        value.get("time"),
    ])
}

fn parse_json_string(value: &str) -> Option<Value> {
    serde_json::from_str(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(provider: &str, line: &str) -> AgentChatEvent {
        normalize_chat_line("agent-1", provider, line, 7).expect("event")
    }

    #[test]
    fn codex_message_tool_and_approval_events_are_normalized() {
        let message = one(
            "codex",
            r#"{"type":"response_item","turn_id":"turn-1","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Codex answer"}]}}"#,
        );
        assert_eq!(message.kind, AgentChatEventKind::Message);
        assert_eq!(message.role, Some(AgentChatRole::Assistant));
        assert_eq!(message.text.as_deref(), Some("Codex answer"));
        assert_eq!(message.turn_id.as_deref(), Some("turn-1"));

        let tool = one(
            "codex",
            r#"{"type":"event_msg","payload":{"type":"exec_command_begin","command":"npm test","turn_id":"turn-1"}}"#,
        );
        assert_eq!(tool.kind, AgentChatEventKind::ToolCall);
        assert_eq!(tool.command.as_deref(), Some("npm test"));
        assert_eq!(tool.status, Some(AgentChatStatus::Running));

        let approval = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"git status\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"Need git status\"}"}}"#,
        );
        assert_eq!(approval.kind, AgentChatEventKind::ToolCall);
        assert_eq!(approval.title.as_deref(), Some("shell_command"));
        assert_eq!(approval.status, Some(AgentChatStatus::ActionRequired));
        assert_eq!(approval.command.as_deref(), Some("git status"));
        assert_eq!(approval.text.as_deref(), Some("Need git status"));
        assert_eq!(approval.metadata["raw_type"], "function_call");
        assert_eq!(approval.metadata["tool_name"], "shell_command");
    }

    #[test]
    fn codex_response_item_user_context_attaches_to_the_canonical_request() {
        let lines = [
            r#"{"type":"response_item","payload":{"type":"message","id":"context-1","role":"user","content":[{"type":"input_text","text":"Host context."}],"internal_chat_message_metadata_passthrough":{"turn_id":"codex-turn-1"}}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"context-2","role":"user","content":[{"type":"input_text","text":"Skill context."}],"internal_chat_message_metadata_passthrough":{"turn_id":"codex-turn-1"}}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"Inspect the archive."}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"context-3","role":"user","content":[{"type":"input_text","text":"More host context."}],"internal_chat_message_metadata_passthrough":{"turn_id":"codex-turn-1"}}}"#,
        ];

        let events = normalize_chat_lines("agent-1", "codex", lines);
        assert_eq!(events.len(), 4);
        assert!(events[..2].iter().all(|event| {
            event.metadata["input_origin"] == "context_injection"
                && event.metadata["input_purpose"] == "context"
                && event.metadata["context_observation"] == "provider_native"
                && event.metadata["request_root_id"] == "agent-1:3"
                && event.metadata["provider_turn_id"] == "codex-turn-1"
        }));
        assert_eq!(
            events[0].metadata["causal_ref"],
            "provider:message:context-1"
        );
        assert_eq!(events[2].role, Some(AgentChatRole::User));
        assert_eq!(events[2].metadata["input_origin"], "human_input");
        assert_eq!(events[2].metadata["context_observation"], "provider_native");
        assert_eq!(events[2].metadata["request_root_id"], "agent-1:3");
        assert_eq!(events[3].metadata["request_root_id"], "agent-1:3");
    }

    #[test]
    fn codex_assistant_memory_citation_is_not_visible_chat_text() {
        let message = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer for the user.\n\n<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-1|note=[internal]\n</citation_entries>\n</oai-mem-citation>"}]}}"#,
        );

        assert_eq!(message.text.as_deref(), Some("Answer for the user."));
    }

    #[test]
    fn codex_tool_call_input_object_exposes_command() {
        let tool = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"shell_command","input":{"command":"Get-ChildItem src-tauri","sandbox_permissions":"read-only"}}}"#,
        );

        assert_eq!(tool.kind, AgentChatEventKind::ToolCall);
        assert_eq!(tool.title.as_deref(), Some("shell_command"));
        assert_eq!(tool.command.as_deref(), Some("Get-ChildItem src-tauri"));
        assert_eq!(tool.language.as_deref(), Some("shell"));
        assert_eq!(tool.metadata["tool_name"], "shell_command");
    }

    #[test]
    fn codex_exec_tool_call_normalizes_cmd_as_the_command() {
        let tool = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec","arguments":"{\"cmd\":\"rg -n AgentChatView src/features\"}"}}"#,
        );

        assert_eq!(tool.title.as_deref(), Some("exec"));
        assert_eq!(
            tool.command.as_deref(),
            Some("rg -n AgentChatView src/features")
        );
        assert_eq!(tool.language.as_deref(), Some("shell"));
        assert_eq!(
            tool.metadata["tool_input"]["cmd"],
            "rg -n AgentChatView src/features"
        );
    }

    #[test]
    fn pi_tool_execution_start_normalizes_command_from_tool_input() {
        let tool = one(
            "pi",
            r#"{"type":"tool_execution_start","toolName":"exec","toolCallId":"call-1","args":{"cmd":"npm test -- chat"}}"#,
        );

        assert_eq!(tool.command.as_deref(), Some("npm test -- chat"));
        assert_eq!(tool.language.as_deref(), Some("shell"));
    }

    #[test]
    fn codex_apply_patch_tool_call_preserves_raw_patch_input() {
        let tool = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: src-tauri/src/state/conversation_archive/turns.rs\n@@\n-old\n+new\n*** End Patch"}}"#,
        );

        assert_eq!(tool.kind, AgentChatEventKind::ToolCall);
        assert_eq!(tool.title.as_deref(), Some("apply_patch"));
        assert_eq!(tool.text, None);
        assert_eq!(
            tool.metadata["tool_input_text"],
            "*** Begin Patch\n*** Update File: src-tauri/src/state/conversation_archive/turns.rs\n@@\n-old\n+new\n*** End Patch"
        );
    }

    #[test]
    fn codex_subagent_notifications_are_hidden_and_results_are_summarized() {
        assert!(normalize_chat_line(
            "agent-1",
            "codex",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":"<subagent_notification>\n{\"agent_path\":\"agent-1\",\"status\":{\"completed\":\"The subagent was spawned successfully.\"}}\n</subagent_notification>"}}"#,
            1
        )
        .is_none());

        let result = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"function_call_output","output":"{\"status\":{\"019e\":{\"completed\":\"The subagent was spawned successfully.\"}},\"timed_out\":false}"}}"#,
        );

        assert_eq!(result.kind, AgentChatEventKind::ToolResult);
        assert_eq!(result.title.as_deref(), Some("Subagent completed"));
        assert_eq!(
            result.text.as_deref(),
            Some("The subagent was spawned successfully.")
        );
    }

    #[test]
    fn claude_messages_tools_and_local_commands_are_normalized_defensively() {
        let message = one(
            "claude",
            r#"{"type":"assistant","message":{"id":"msg-1","role":"assistant","content":[{"type":"text","text":"Claude answer"}]}}"#,
        );
        assert_eq!(message.kind, AgentChatEventKind::Message);
        assert_eq!(message.role, Some(AgentChatRole::Assistant));
        assert_eq!(message.text.as_deref(), Some("Claude answer"));
        assert_eq!(message.turn_id.as_deref(), Some("msg-1"));

        let string_message = one(
            "claude",
            r#"{"type":"assistant","message":{"id":"msg-2","role":"assistant","content":"String answer"}}"#,
        );
        assert_eq!(string_message.kind, AgentChatEventKind::Message);
        assert_eq!(string_message.text.as_deref(), Some("String answer"));

        let tool = one(
            "claude",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"git status"}}],"stop_reason":"tool_use"}}"#,
        );
        assert_eq!(tool.kind, AgentChatEventKind::ToolCall);
        assert_eq!(tool.title.as_deref(), Some("Bash"));
        assert_eq!(tool.command.as_deref(), Some("git status"));

        assert!(normalize_chat_line(
            "agent-1",
            "claude",
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to Opus 4.6</local-command-stdout>"}}"#,
            8
        )
        .is_none());
    }

    #[test]
    fn claude_native_delivery_envelope_is_hidden_from_user_messages() {
        let message = one(
            "claude",
            r#"{"type":"user","session_id":"claude-1","message":{"role":"user","content":"[Wardian message_id=msg-1 interaction_id=ask-1 generation=7 target=agent-1] sender=agent-2 reply_to=parent-1 deadline=2026-09-05T12:00:00Z\nReview this patch"}}"#,
        );

        assert_eq!(message.kind, AgentChatEventKind::Message);
        assert_eq!(message.role, Some(AgentChatRole::User));
        assert_eq!(message.text.as_deref(), Some("Review this patch"));
    }

    #[test]
    fn claude_legacy_delivery_alias_preserves_nested_user_request_order() {
        let content = "[Wardian message_id=msg-1 interaction_id=ask-1 generation=7 target=agent-1]\n<environment_context>\n<USER_REQUEST>\nReview this patch\n</USER_REQUEST>\n</environment_context>\nVisible tail";
        let message = one(
            "claude",
            &serde_json::json!({
                "type": "user",
                "message": {"role": "user", "content": content}
            })
            .to_string(),
        );

        assert_eq!(message.text.as_deref(), Some("Visible tail"));
        assert_eq!(
            legacy_visible_chat_text_for_provider("claude", &AgentChatRole::User, content)
                .as_deref(),
            Some("Review this patch")
        );
    }

    #[test]
    fn claude_file_tool_calls_preserve_input_paths() {
        let read = one(
            "claude",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-read","name":"Read","input":{"file_path":"D:\\Development\\Wardian\\src-tauri\\src\\state\\conversation_archive\\turns.rs"}}],"stop_reason":"tool_use"}}"#,
        );
        assert_eq!(read.kind, AgentChatEventKind::ToolCall);
        assert_eq!(read.title.as_deref(), Some("Read"));
        assert_eq!(
            read.metadata["file_path"],
            r#"D:\Development\Wardian\src-tauri\src\state\conversation_archive\turns.rs"#
        );
        assert_eq!(
            read.metadata["tool_input"]["file_path"],
            r#"D:\Development\Wardian\src-tauri\src\state\conversation_archive\turns.rs"#
        );

        let edit = one(
            "claude",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-edit","name":"Edit","input":{"file_path":"docs/specs/2026-06-25-turns-jsonl-request-index.md","old_string":"old","new_string":"new"}}],"stop_reason":"tool_use"}}"#,
        );
        assert_eq!(edit.kind, AgentChatEventKind::ToolCall);
        assert_eq!(edit.title.as_deref(), Some("Edit"));
        assert_eq!(
            edit.metadata["file_path"],
            "docs/specs/2026-06-25-turns-jsonl-request-index.md"
        );
        assert_eq!(edit.metadata["tool_input"]["old_string"], "old");
    }

    #[test]
    fn gemini_ignores_partial_chunks_and_deduplicates_completed_messages() {
        assert!(normalize_chat_line(
            "agent-1",
            "gemini",
            r#"{"id":"gem-1","type":"model","content":"partial"}"#,
            1
        )
        .is_none());

        let lines = [
            r#"{"id":"gem-1","type":"model","content":"Gemini answer","tokens":{"total":4}}"#,
            r#"{"id":"gem-1","type":"model","content":"Gemini answer","tokens":{"total":4}}"#,
            r#"{"type":"tool_use","tool_name":"read_file"}"#,
        ];

        let events = normalize_chat_lines("agent-1", "gemini", lines);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, AgentChatEventKind::Message);
        assert_eq!(events[0].text.as_deref(), Some("Gemini answer"));
        assert_eq!(events[1].kind, AgentChatEventKind::ToolCall);
        assert_eq!(events[1].status, Some(AgentChatStatus::ActionRequired));
    }

    #[test]
    fn user_prompt_wrappers_are_removed_from_visible_messages() {
        let gemini_user = one(
            "gemini",
            r#"{"type":"user","content":"<USER_REQUEST>\nList 50 numbers.\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is internal.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nThe user changed model.\n</USER_SETTINGS_CHANGE>"}"#,
        );
        assert_eq!(gemini_user.kind, AgentChatEventKind::Message);
        assert_eq!(gemini_user.role, Some(AgentChatRole::User));
        assert_eq!(gemini_user.text.as_deref(), Some("List 50 numbers."));

        let codex_user = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":"Actual prompt\n<environment_context>\n{\"cwd\":\"D:\\Development\\Wardian\"}\n</environment_context>"}}"#,
        );
        assert_eq!(codex_user.text.as_deref(), Some("Actual prompt"));
    }

    #[test]
    fn user_attachment_transport_markers_are_removed_from_visible_messages() {
        let text = r#"<image name=[Image #1] path="C:\Temp\codex-clipboard.png"> Review this screenshot. [Image #1]"#;
        assert_eq!(
            visible_chat_text(&AgentChatRole::User, text).as_deref(),
            Some("Review this screenshot. [Image #1]")
        );

        let text =
            r#"Please inspect this file. <file name=[File #1] path="C:\Temp\notes.txt"> [File #1]"#;
        assert_eq!(
            visible_chat_text(&AgentChatRole::User, text).as_deref(),
            Some("Please inspect this file. [File #1]")
        );
    }

    #[test]
    fn internal_only_user_prompt_is_dropped() {
        assert!(normalize_chat_line(
            "agent-1",
            "gemini",
            r#"{"type":"user","content":"<USER_REQUEST>\nWARDIAN_ADD_DIR_PROBE\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\ninternal\n</ADDITIONAL_METADATA>"}"#,
            1
        )
        .is_none());
    }

    #[test]
    fn wardian_named_user_content_is_not_broadly_filtered() {
        let message = one(
            "gemini",
            r#"{"type":"user","content":"<USER_REQUEST>\nWARDIAN_HOME is wrong\n</USER_REQUEST>"}"#,
        );

        assert_eq!(message.kind, AgentChatEventKind::Message);
        assert_eq!(message.role, Some(AgentChatRole::User));
        assert_eq!(message.text.as_deref(), Some("WARDIAN_HOME is wrong"));
    }

    #[test]
    fn antigravity_transcript_user_and_done_model_records_are_normalized() {
        let user = one(
            "antigravity",
            r#"{"step_index":0,"source":"USER_INPUT","type":"USER_INPUT","status":"DONE","content":"Build it"}"#,
        );
        assert_eq!(user.kind, AgentChatEventKind::Message);
        assert_eq!(user.role, Some(AgentChatRole::User));
        assert_eq!(user.turn_id.as_deref(), Some("0"));

        let assistant = one(
            "antigravity",
            r#"{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-05-20T09:21:54Z","content":"Antigravity answer"}"#,
        );
        assert_eq!(assistant.kind, AgentChatEventKind::Message);
        assert_eq!(assistant.role, Some(AgentChatRole::Assistant));
        assert_eq!(assistant.text.as_deref(), Some("Antigravity answer"));
        assert_eq!(assistant.source.as_deref(), Some("transcript"));
    }

    #[test]
    fn antigravity_planner_tool_calls_are_normalized() {
        let tool = one(
            "antigravity",
            r#"{"step_index":3,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"RUN_COMMAND","args":{"command":"npm run test -- --run"}}]}"#,
        );

        assert_eq!(tool.kind, AgentChatEventKind::ToolCall);
        assert_eq!(tool.title.as_deref(), Some("Run command"));
        assert_eq!(tool.command.as_deref(), Some("npm run test -- --run"));
        assert_eq!(tool.turn_id.as_deref(), Some("3"));
    }

    #[test]
    fn antigravity_file_tool_calls_preserve_path_metadata() {
        let read = one(
            "antigravity",
            r#"{"step_index":5,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"view_file","args":{"AbsolutePath":"\"C:/Users/tgemi/Hivemind/!Daily/2026-06-24.md\""}}]}"#,
        );
        assert_eq!(read.kind, AgentChatEventKind::ToolCall);
        assert_eq!(read.title.as_deref(), Some("View file"));
        assert_eq!(
            read.metadata["file_path"].as_str(),
            Some("C:/Users/tgemi/Hivemind/!Daily/2026-06-24.md")
        );
        assert_eq!(
            read.metadata["files_read"][0].as_str(),
            Some("C:/Users/tgemi/Hivemind/!Daily/2026-06-24.md")
        );

        let write = one(
            "antigravity",
            r#"{"step_index":6,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"write_to_file","args":{"TargetFile":"\"C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/find_modified.py\""}}]}"#,
        );
        assert_eq!(write.kind, AgentChatEventKind::ToolCall);
        assert_eq!(write.title.as_deref(), Some("Write file"));
        assert_eq!(
            write.metadata["file_path"].as_str(),
            Some("C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/find_modified.py")
        );
        assert_eq!(
            write.metadata["files_written"][0].as_str(),
            Some("C:/Users/tgemi/.gemini/antigravity-cli/brain/session/scratch/find_modified.py")
        );
    }

    #[test]
    fn antigravity_model_action_records_are_normalized_as_tool_results() {
        let result = one(
            "antigravity",
            r#"{"step_index":4,"source":"MODEL","type":"RUN_COMMAND","status":"DONE","content":"3 tests passed"}"#,
        );

        assert_eq!(result.kind, AgentChatEventKind::ToolResult);
        assert_eq!(result.title.as_deref(), Some("Run command"));
        assert_eq!(result.text.as_deref(), Some("3 tests passed"));
        assert_eq!(result.status, Some(AgentChatStatus::Succeeded));
        assert_eq!(result.turn_id.as_deref(), Some("4"));
    }

    #[test]
    fn opencode_text_tool_and_finish_events_are_normalized() {
        let text = one(
            "opencode",
            r#"{"type":"text","sessionID":"ses_test","part":{"type":"text","text":"OpenCode answer"}}"#,
        );
        assert_eq!(text.kind, AgentChatEventKind::Message);
        assert_eq!(text.role, Some(AgentChatRole::Assistant));
        assert_eq!(text.turn_id.as_deref(), Some("ses_test"));

        let tool = one(
            "opencode",
            r#"{"type":"tool_use","sessionID":"ses_test","part":{"name":"bash","input":{"command":"npm run lint"}}}"#,
        );
        assert_eq!(tool.kind, AgentChatEventKind::ToolCall);
        assert_eq!(tool.command.as_deref(), Some("npm run lint"));

        let finish = one(
            "opencode",
            r#"{"type":"step_finish","sessionID":"ses_test","part":{"reason":"stop"}}"#,
        );
        assert_eq!(finish.kind, AgentChatEventKind::Status);
        assert_eq!(finish.status, Some(AgentChatStatus::Succeeded));
    }

    #[test]
    fn opencode_tool_calls_preserve_structured_input_and_written_paths() {
        // OpenCode describes an edit only in the tool's input object. Dropping
        // it left the chat transcript unable to say which files a turn changed.
        let edit = one(
            "opencode",
            r#"{"type":"tool_use","sessionID":"ses_test","part":{"name":"edit","input":{"filePath":"src/app.ts","oldString":"const a = 1;","newString":"const a = 2;"}}}"#,
        );

        assert_eq!(edit.kind, AgentChatEventKind::ToolCall);
        assert_eq!(edit.metadata["tool_name"], "edit");
        assert_eq!(edit.metadata["tool_input"]["oldString"], "const a = 1;");
        assert_eq!(edit.metadata["file_path"], "src/app.ts");
        assert_eq!(edit.metadata["files_written"][0], "src/app.ts");

        let read = one(
            "opencode",
            r#"{"type":"tool_use","sessionID":"ses_test","part":{"name":"read","input":{"filePath":"src/app.ts"}}}"#,
        );
        assert_eq!(read.metadata["files_read"][0], "src/app.ts");
        assert!(read.metadata.get("files_written").is_none());
    }

    #[test]
    fn tool_calls_report_their_name_in_metadata_for_every_provider() {
        // Presentation resolves a tool by `metadata.tool_name`; providers that
        // reported the name only in the title silently lost that classification.
        let claude = one(
            "claude",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Write","input":{"file_path":"a.ts","content":"x"}}],"stop_reason":"tool_use"}}"#,
        );
        assert_eq!(claude.metadata["tool_name"], "Write");

        let gemini = one("gemini", r#"{"type":"tool_use","tool_name":"read_file"}"#);
        assert_eq!(gemini.metadata["tool_name"], "read_file");

        let antigravity = one(
            "antigravity",
            r#"{"step_index":3,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","tool_calls":[{"name":"RUN_COMMAND","args":{"command":"npm test"}}]}"#,
        );
        assert_eq!(antigravity.metadata["tool_name"], "RUN_COMMAND");

        let codex = one(
            "codex",
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"shell_command","input":{"command":"npm test"}}}"#,
        );
        assert_eq!(codex.metadata["tool_name"], "shell_command");
        assert_eq!(codex.metadata["tool_input"]["command"], "npm test");
    }

    #[test]
    fn claude_native_context_records_keep_origin_and_skill_causality() {
        let lines = [
            r#"{"type":"user","uuid":"request-1","message":{"role":"user","content":"Fix the archive."}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"skill-call-1","name":"Skill","input":{}}]}}"#,
            r#"{"type":"user","isMeta":true,"parent_tool_use_id":"skill-call-1","uuid":"context-1","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill\nUse the procedure."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"skill-call-2","name":"Skill","input":{}}]}}"#,
            r#"{"type":"user","isMeta":true,"parent_tool_use_id":"skill-call-2","uuid":"context-2","message":{"role":"user","content":[{"type":"text","text":"A second injected procedure."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Archive fixed."}]}}"#,
        ];

        let events = normalize_chat_lines("agent-1", "claude", lines);
        assert_eq!(events.len(), 6);
        assert_eq!(events[0].metadata["input_origin"], "human_input");
        assert_eq!(events[0].metadata["request_root_id"], "request-1");
        for (event, context_id) in events
            .iter()
            .skip(2)
            .step_by(2)
            .zip(["context-1", "context-2"])
        {
            assert_eq!(event.role, Some(AgentChatRole::System));
            assert_eq!(event.metadata["input_origin"], "context_injection");
            assert_eq!(event.metadata["input_purpose"], "skill");
            assert_eq!(event.metadata["request_root_id"], "request-1");
            assert!(event.metadata["causal_ref"]
                .as_str()
                .is_some_and(|value| value.starts_with("provider:tool_use:")));
            assert_eq!(event.turn_id.as_deref(), Some(context_id));
        }
    }

    #[test]
    fn claude_context_without_skill_tool_is_still_provider_typed() {
        let event = one(
            "claude",
            r#"{"type":"user","isMeta":true,"parentUuid":"request-1","uuid":"context-1","message":{"role":"user","content":"Host context supplied by Claude."}}"#,
        );

        assert_eq!(event.metadata["input_origin"], "context_injection");
        assert_eq!(event.metadata["input_purpose"], "context");
        assert_eq!(event.metadata["causal_ref"], "provider:uuid:request-1");
    }

    #[test]
    fn claude_parent_uuid_query_retains_normalized_causal_ref() {
        let event = one(
            "claude",
            r#"{"type":"user","parentUuid":"assistant-1","message":{"role":"user","content":"Inspect the archive."}}"#,
        );

        assert_eq!(event.metadata["input_origin"], "human_input");
        assert_eq!(event.metadata["input_purpose"], "request");
        assert_eq!(event.metadata["causal_ref"], "provider:uuid:assistant-1");
    }

    #[test]
    fn claude_interruption_records_are_provider_internal_messages() {
        let lines = [
            r#"{"type":"user","parentUuid":"assistant-1","message":{"role":"user","content":"[Request interrupted by user]"}}"#,
            r#"{"type":"user","parentUuid":"assistant-1","message":{"role":"user","content":"[Request interrupted by user for tool use]"}}"#,
        ];

        let events = normalize_chat_lines("agent-1", "claude", lines);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| {
            event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::System)
                && event.metadata["input_origin"] == "provider_internal"
                && event.metadata["input_purpose"] == "internal"
                && event.metadata["causal_ref"] == "provider:uuid:assistant-1"
        }));
        assert_eq!(
            events[0].text.as_deref(),
            Some("[Request interrupted by user]")
        );
        assert_eq!(
            events[1].text.as_deref(),
            Some("[Request interrupted by user for tool use]")
        );
    }

    #[test]
    fn claude_real_stream_provenance_does_not_promote_context_or_tool_results_to_prompts() {
        // The tool-result line is captured from Claude Code 2.1.263 with the
        // haiku model. Claude emits it as a native `user` record even though
        // its content is the result of the preceding Read tool call.
        let lines = [
            r#"{"type":"user","uuid":"request-1","message":{"role":"user","content":"Inspect the evidence file."}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01evidence","name":"Read","input":{"file_path":"provider-evidence.txt"}}]}}"#,
            r#"{"type":"user","isMeta":true,"parent_tool_use_id":"toolu_01evidence","uuid":"context-1","message":{"role":"user","content":[{"type":"text","text":"Native provider context."}]}}"#,
            r#"{"type":"user","uuid":"tool-result-1","parent_tool_use_id":null,"message":{"role":"user","content":[{"tool_use_id":"toolu_01evidence","type":"tool_result","content":"1\tWARDIAN_CLAUDE_REAL_EVIDENCE\n2\t"}]}}"#,
            r#"{"type":"user","parentUuid":"request-1","message":{"role":"user","content":"[Request interrupted by user]"}}"#,
        ];

        let events = normalize_chat_lines("agent-1", "claude", lines);

        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == AgentChatEventKind::Message
                    && event.role == Some(AgentChatRole::User))
                .count(),
            1
        );
        assert_eq!(events[2].role, Some(AgentChatRole::System));
        assert_eq!(events[2].metadata["input_origin"], "context_injection");
        assert_eq!(events[3].kind, AgentChatEventKind::ToolResult);
        assert_eq!(events[3].role, Some(AgentChatRole::Tool));
        assert_eq!(events[3].metadata["raw_type"], "tool_result");
        assert_eq!(events[4].role, Some(AgentChatRole::System));
        assert_eq!(events[4].metadata["input_origin"], "provider_internal");
    }

    #[test]
    fn provider_without_context_evidence_keeps_normal_user_input_and_reports_no_boundary() {
        let event = one(
            "gemini",
            r#"{"type":"user","id":"request-1","text":"Inspect the archive."}"#,
        );

        assert_eq!(event.metadata["input_origin"], "human_input");
        assert_eq!(event.metadata["input_purpose"], "request");
        assert_eq!(event.metadata["context_observation"], "unreported");
        assert_ne!(event.metadata["input_origin"], "context_injection");
    }

    #[test]
    fn mock_tool_calls_carry_structured_input_for_offline_chat_coverage() {
        // Without this the chat transcript's file-change surface was reachable
        // only through a real provider subscription.
        let edit = one(
            "mock",
            r#"{"type":"tool_call","tool_name":"Edit","input":{"file_path":"src/app.ts","old_string":"a","new_string":"b"}}"#,
        );
        assert_eq!(edit.kind, AgentChatEventKind::ToolCall);
        assert_eq!(edit.title.as_deref(), Some("Edit"));
        assert_eq!(edit.metadata["tool_name"], "Edit");
        assert_eq!(edit.metadata["tool_input"]["old_string"], "a");
        assert_eq!(edit.metadata["files_written"][0], "src/app.ts");

        let shell = one(
            "mock",
            r#"{"type":"tool_call","tool_name":"Bash","input":{},"command":"npm run test"}"#,
        );
        assert_eq!(shell.command.as_deref(), Some("npm run test"));

        let result = one(
            "mock",
            r#"{"type":"tool_result","tool_name":"Edit","content":"applied","status":"success"}"#,
        );
        assert_eq!(result.kind, AgentChatEventKind::ToolResult);
        assert_eq!(result.text.as_deref(), Some("applied"));
        assert_eq!(result.status, Some(AgentChatStatus::Succeeded));
    }

    #[test]
    fn mock_and_fallback_are_normalized_without_accepting_malformed_known_provider_json() {
        let mock = one("mock", r#"{"type":"model","content":"Mock answer"}"#);
        assert_eq!(mock.kind, AgentChatEventKind::Message);
        assert_eq!(mock.role, Some(AgentChatRole::Assistant));
        assert_eq!(mock.text.as_deref(), Some("Mock answer"));

        assert!(normalize_chat_line("agent-1", "codex", "not json", 1).is_none());

        let fallback = one("unknown-provider", "plain terminal output");
        assert_eq!(fallback.kind, AgentChatEventKind::TerminalOutput);
        assert_eq!(fallback.text.as_deref(), Some("plain terminal output"));
        assert_eq!(fallback.source.as_deref(), Some("terminal"));
    }

    #[test]
    fn pi_session_messages_render_user_assistant_and_tool_rows() {
        let lines = [
            r#"{"type":"session","id":"pi-session"}"#,
            r#"{"type":"message","message":{"role":"user","content":"Inspect the file"}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"src/main.rs"}}],"stopReason":"toolUse"}}"#,
            r#"{"type":"message","message":{"role":"toolResult","toolCallId":"call-1","toolName":"read","content":[{"type":"text","text":"file contents"}],"isError":false}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Pi answer"}],"stopReason":"stop"}}"#,
        ];

        let events = normalize_chat_lines("agent-1", "pi", lines);
        assert_eq!(events[1].role, Some(AgentChatRole::User));
        assert_eq!(events[2].kind, AgentChatEventKind::ToolCall);
        assert_eq!(events[2].metadata["files_read"][0], "src/main.rs");
        assert_eq!(events[3].kind, AgentChatEventKind::ToolResult);
        assert_eq!(events[4].text.as_deref(), Some("Pi answer"));
    }
}
