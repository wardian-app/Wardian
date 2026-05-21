use std::collections::HashSet;

use serde_json::{json, Value};
use wardian_core::models::{AgentChatEvent, AgentChatEventKind, AgentChatRole, AgentChatStatus};

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
            let arguments = str_field(payload, "arguments").and_then(parse_json_string);
            let command = arguments
                .as_ref()
                .and_then(|value| str_field(value, "command").map(str::to_string))
                .or_else(|| str_field(payload, "name").map(str::to_string));
            let needs_approval = arguments.as_ref().is_some_and(|value| {
                str_field(value, "sandbox_permissions") == Some("require_escalated")
            });
            let text = arguments
                .as_ref()
                .and_then(|value| str_field(value, "justification").map(str::to_string));
            Some(tool_call_event(
                session_id,
                provider,
                sequence,
                source,
                turn_id,
                command,
                text,
                payload_type,
                if needs_approval {
                    AgentChatStatus::ActionRequired
                } else {
                    AgentChatStatus::Running
                },
            ))
        }
        "function_call_output" | "custom_tool_call_output" => Some(event(
            session_id,
            provider,
            sequence,
            AgentChatEventKind::ToolResult,
            EventFields {
                role: Some(AgentChatRole::Tool),
                text: text_from_value(payload),
                status: Some(AgentChatStatus::Succeeded),
                turn_id,
                source: Some(source),
                metadata: json!({"raw_type": payload_type}),
                ..Default::default()
            },
        )),
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
        _ => None,
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
            r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"git status\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"Need git status\"}"}}"#,
        );
        assert_eq!(approval.kind, AgentChatEventKind::ToolCall);
        assert_eq!(approval.status, Some(AgentChatStatus::ActionRequired));
        assert_eq!(approval.command.as_deref(), Some("git status"));
        assert_eq!(approval.text.as_deref(), Some("Need git status"));
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
