use crate::providers::claude::{classify_claude_user_event, ClaudeUserEventKind};

/// Converts a workspace absolute path into Claude Code's project directory name.
/// Claude replaces each of `:`, `\`, `/`, `.` with `-`.
/// e.g. `D:\Development\Wardian` → `D--Development-Wardian`
pub(crate) fn claude_project_dir_name(workspace: &str) -> String {
    workspace
        .chars()
        .map(|c| match c {
            ':' | '\\' | '/' | '.' => '-',
            _ => c,
        })
        .collect()
}

pub(crate) fn claude_is_real_user_query(line: &serde_json::Value) -> bool {
    classify_claude_user_event(line) == ClaudeUserEventKind::RealQuery
}

pub(crate) fn claude_permission_hook_matches_session(event: &serde_json::Value, session_id: &str) -> bool {
    if session_id.trim().is_empty() {
        return false;
    }

    if event
        .get("session_id")
        .and_then(|v| v.as_str())
        .is_some_and(|sid| sid == session_id)
    {
        return true;
    }

    event
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .and_then(|path| std::path::Path::new(path).file_stem())
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem == session_id)
}

pub(crate) fn claude_status_from_log(lines: &[serde_json::Value]) -> Option<String> {
    let mut has_activity = false;

    for line in lines.iter().rev() {
        let msg_type = line.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "system" => {
                let subtype = line.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                if subtype == "permission_request" {
                    return Some("Action Needed".to_string());
                }
                if subtype == "turn_duration" {
                    return Some("Idle".to_string());
                }
            }
            "result" => {
                return Some("Idle".to_string());
            }
            "assistant" => {
                let stop_reason = line
                    .get("message")
                    .and_then(|m| m.get("stop_reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if !stop_reason.is_empty() {
                    if stop_reason == "tool_use" {
                        // Activity signal, but keep searching for permission_request in this turn
                        has_activity = true;
                    } else {
                        // Definitive end of turn (end_turn, stop_sequence, etc.)
                        return Some("Idle".to_string());
                    }
                } else {
                    // Streaming or incomplete assistant message
                    return Some("Processing...".to_string());
                }
            }
            "user" => {
                let kind = classify_claude_user_event(line);
                if kind == ClaudeUserEventKind::RealQuery || kind == ClaudeUserEventKind::ToolResult
                {
                    // Start of turn or handled tool result
                    return Some("Processing...".to_string());
                }
                // Other user events are just activity
                has_activity = true;
            }
            "progress" => {
                return Some("Processing...".to_string());
            }
            _ => {}
        }
    }

    if has_activity {
        Some("Processing...".to_string())
    } else {
        None
    }
}


#[cfg(test)]
mod tests {
    use super::*;
        #[test]
    fn claude_status_from_log_ignores_local_commands_after_idle() {
        let lines = vec![
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "done" }],
                    "stop_reason": "end_turn"
                }
            }),
            serde_json::json!({ "type": "system", "subtype": "turn_duration" }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<local-command-caveat>Do not respond.</local-command-caveat>"
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<command-name>/model</command-name><command-message>model</command-message>"
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "<local-command-stdout>Set model to Opus 4.6</local-command-stdout>"
                }
            }),
            serde_json::json!({ "type": "custom-title" }),
            serde_json::json!({ "type": "file-history-snapshot" }),
        ];

        assert_eq!(claude_status_from_log(&lines), Some("Idle".to_string()));
    }

    #[test]
    fn claude_status_from_log_treats_real_user_prompt_as_processing() {
        let lines = vec![
            serde_json::json!({
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "done" }],
                    "stop_reason": "end_turn"
                }
            }),
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": "Please continue." }
            }),
        ];

        assert_eq!(
            claude_status_from_log(&lines),
            Some("Processing...".to_string())
        );
    }

        #[test]
    fn claude_permission_hook_ignores_other_transcript_sessions() {
        let event = serde_json::json!({
            "session_id": "other-session",
            "transcript_path": "/tmp/claude-projects/wardian/other-session.jsonl",
            "tool_name": "Bash"
        });

        assert!(!claude_permission_hook_matches_session(
            &event,
            "expected-session"
        ));
    }

    #[test]
    fn claude_permission_hook_accepts_matching_transcript_session() {
        let event = serde_json::json!({
            "session_id": "expected-session",
            "transcript_path": "/tmp/claude-projects/wardian/expected-session.jsonl",
            "tool_name": "Bash"
        });

        assert!(claude_permission_hook_matches_session(
            &event,
            "expected-session"
        ));
    }

        #[test]
    fn claude_status_from_log_does_not_look_past_turn_boundary() {
        let lines = vec![
            serde_json::json!({ "type": "system", "subtype": "turn_duration" }), // Turn 1
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": "Query 2" }
            }), // Turn 2 start
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [], "stop_reason": "tool_use" }
            }), // Turn 2 tool use
        ];

        // Should be Processing..., NOT Idle (from turn 1)
        assert_eq!(
            claude_status_from_log(&lines),
            Some("Processing...".to_string())
        );
    }

    #[test]
    fn claude_status_from_log_detects_action_needed_in_current_turn() {
        let lines = vec![
            serde_json::json!({ "type": "system", "subtype": "turn_duration" }), // Turn 1
            serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": "Query 2" }
            }),
            serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": [], "stop_reason": "tool_use" }
            }),
            serde_json::json!({ "type": "system", "subtype": "permission_request" }),
        ];

        assert_eq!(
            claude_status_from_log(&lines),
            Some("Action Needed".to_string())
        );
    }
}
