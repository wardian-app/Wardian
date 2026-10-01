use crate::providers::claude::{classify_claude_user_event, ClaudeUserEventKind};
use wardian_core::models::AgentConfig;

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

/// Finds the new Claude transcript whose provider-owned title identifies this
/// Wardian runtime. Claude Code may allocate a different session UUID than the
/// one requested at interactive launch, so filename-only lookup is not enough.
pub(crate) fn discover_claude_log_for_session_name(
    project_dir: &std::path::Path,
    session_name: &str,
    ignored_paths: &std::collections::HashSet<std::path::PathBuf>,
) -> Option<(std::path::PathBuf, String)> {
    let mut candidates = std::fs::read_dir(project_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("jsonl")
                && !ignored_paths.contains(&path))
            .then_some(path)
        })
        .filter_map(|path| {
            let file = std::fs::File::open(&path).ok()?;
            let reader = std::io::BufReader::new(file);
            let mut session_id = None;
            for line in std::io::BufRead::lines(reader).take(32) {
                let parsed: serde_json::Value = serde_json::from_str(&line.ok()?).ok()?;
                let matches_name = parsed
                    .get("customTitle")
                    .or_else(|| parsed.get("agentName"))
                    .and_then(|value| value.as_str())
                    == Some(session_name);
                if matches_name {
                    session_id = parsed
                        .get("sessionId")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                        .or_else(|| {
                            path.file_stem()
                                .and_then(|value| value.to_str())
                                .map(str::to_string)
                        });
                    break;
                }
            }
            session_id.map(|session_id| {
                let modified = std::fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                (path, session_id, modified)
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, _, modified)| *modified);
    candidates
        .pop()
        .map(|(path, session_id, _)| (path, session_id))
}

/// Records the Claude logs that existed before a fresh launch. A fresh
/// conversation must never adopt a paused conversation solely because the
/// provider has not written its new transcript yet.
pub(crate) fn claude_log_paths(
    project_dir: &std::path::Path,
) -> std::collections::HashSet<std::path::PathBuf> {
    std::fs::read_dir(project_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension().and_then(|value| value.to_str()) == Some("jsonl")).then_some(path)
        })
        .collect()
}

pub(crate) fn claude_is_real_user_query(line: &serde_json::Value) -> bool {
    classify_claude_user_event(line) == ClaudeUserEventKind::RealQuery
}

pub(crate) fn claude_permission_hook_matches_session(
    event: &serde_json::Value,
    session_id: &str,
) -> bool {
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

/// Returns the Wardian and provider session identities accepted in Claude hook records.
/// Fresh Claude launches use `session_id` when no provider-specific identity is set.
pub(crate) fn claude_accepted_sessions(config: &AgentConfig) -> Vec<String> {
    let mut sessions = Vec::with_capacity(3);
    for session_id in [
        Some(config.session_id.as_str()),
        config.resume_session.as_deref(),
        config.fresh_provider_session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let session_id = if session_id == config.session_id {
            session_id
        } else {
            session_id.trim()
        };
        if !session_id.is_empty() && !sessions.iter().any(|accepted| accepted == session_id) {
            sessions.push(session_id.to_string());
        }
    }
    sessions
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
    fn discovers_new_log_by_provider_owned_agent_name() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("provider-session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"custom-title\",\"customTitle\":\"Wardian test\",\"sessionId\":\"provider-session\"}\n",
        )
        .expect("log");

        assert_eq!(
            discover_claude_log_for_session_name(
                root.path(),
                "Wardian test",
                &std::collections::HashSet::new(),
            ),
            Some((path, "provider-session".to_string()))
        );
    }

    #[test]
    fn fresh_discovery_ignores_logs_that_existed_before_launch() {
        let root = tempfile::tempdir().expect("temp dir");
        let stale_path = root.path().join("paused-provider-session.jsonl");
        std::fs::write(
            &stale_path,
            "{\"type\":\"custom-title\",\"customTitle\":\"Wardian test\",\"sessionId\":\"paused-provider-session\"}\n",
        )
        .expect("stale log");
        let ignored = claude_log_paths(root.path());

        let fresh_path = root.path().join("fresh-provider-session.jsonl");
        std::fs::write(
            &fresh_path,
            "{\"type\":\"custom-title\",\"customTitle\":\"Wardian test\",\"sessionId\":\"fresh-provider-session\"}\n",
        )
        .expect("fresh log");

        assert_eq!(
            discover_claude_log_for_session_name(root.path(), "Wardian test", &ignored),
            Some((fresh_path, "fresh-provider-session".to_string()))
        );
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
