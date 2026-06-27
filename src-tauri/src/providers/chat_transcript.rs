use std::collections::HashSet;

use serde_json::{json, Value};
use wardian_core::models::chat::{
    AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus,
};

use crate::providers::claude::{classify_claude_user_event, ClaudeUserEventKind};

pub fn normalize_chat_lines(
    session_id: &str,
    provider: &str,
    lines: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<AgentChatEvent> {
    let normalized_provider = normalize_provider(provider);
    let mut seen_gemini_messages = HashSet::new();
    let mut events = Vec::new();

    for (index, line) in lines.into_iter().enumerate() {
        let sequence = index as u64 + 1;
        let Some(event) = normalize_chat_line(
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

        events.push(event);
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
        "mock" => normalize_mock(session_id, &provider, &parsed, sequence),
        _ => normalize_fallback_json(session_id, &provider, &parsed, raw_line, sequence),
    }
}

pub fn visible_chat_text(role: &AgentChatRole, text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut visible = trimmed.to_string();
    visible = remove_tag_block(&visible, "environment_context");
    visible = remove_tag_block(&visible, "ADDITIONAL_METADATA");
    visible = remove_tag_block(&visible, "USER_SETTINGS_CHANGE");
    visible = remove_tag_block(&visible, "subagent_notification");

    if *role == AgentChatRole::User {
        if let Some(user_request) = extract_tag_block(trimmed, "USER_REQUEST") {
            visible = user_request;
        }
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
                .and_then(|value| str_field(value, "command").map(str::to_string));
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
            Some(event(
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
            ))
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
                message_event(
                    session_id,
                    provider,
                    sequence,
                    AgentChatRole::User,
                    text_from_value(message)?,
                    "stream_json".to_string(),
                    turn_id_from(message).or_else(|| turn_id_from(parsed)),
                    msg_type,
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
        let command = tool_use
            .get("input")
            .and_then(|input| str_field(input, "command").map(str::to_string));
        return Some(tool_call_event(
            session_id,
            provider,
            sequence,
            "stream_json".to_string(),
            str_field(tool_use, "id")
                .map(str::to_string)
                .or_else(|| turn_id_from(message)),
            command,
            text_from_value(tool_use),
            str_field(tool_use, "name").unwrap_or("tool_use"),
            AgentChatStatus::Running,
        ));
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
        "tool_use" => Some(tool_call_event(
            session_id,
            provider,
            sequence,
            "gemini_log".to_string(),
            turn_id_from(parsed),
            str_field(parsed, "command").map(str::to_string),
            text_from_value(parsed),
            str_field(parsed, "tool_name").unwrap_or("tool_use"),
            AgentChatStatus::ActionRequired,
        )),
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
    let command = args
        .and_then(|value| value.get("command"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let text = command.clone().or_else(|| args.and_then(compact_json_text));

    Some(tool_call_event(
        session_id,
        provider,
        sequence,
        tool_name.to_string(),
        step_index(parsed),
        command,
        text,
        antigravity_tool_title(tool_name),
        AgentChatStatus::Running,
    ))
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
    match tool_name {
        "ASK_QUESTION" => "Ask question",
        "CODE_ACTION" => "Code action",
        "GENERIC" => "Generic action",
        "GREP_SEARCH" => "Search files",
        "LIST_DIRECTORY" => "List directory",
        "READ_URL_CONTENT" => "Read URL",
        "RUN_COMMAND" => "Run command",
        "SEARCH_WEB" => "Search web",
        "VIEW_FILE" => "View file",
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
            Some(tool_call_event(
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
                str_field(part, "name")
                    .or_else(|| str_field(parsed, "tool"))
                    .unwrap_or("tool_use"),
                AgentChatStatus::Running,
            ))
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
        "codex" | "claude" | "gemini" | "antigravity" | "opencode" | "mock"
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
    let text = visible_chat_text(&role, &text)?;
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
            metadata: json!({"raw_type": raw_type}),
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
            metadata: json!({"raw_type": title}),
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

fn codex_tool_call_input(payload: &Value) -> Option<Value> {
    json_object_or_encoded_json(payload.get("arguments"))
        .or_else(|| json_object_or_encoded_json(payload.get("input")))
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
}
