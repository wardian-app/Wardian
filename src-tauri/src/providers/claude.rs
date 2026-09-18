use wardian_core::models::provider::{AgentEvent, AgentProvider};
use wardian_core::models::AgentConfig;

use crate::utils::strip_ansi_controls;

/// The concrete `AgentProvider` implementation for Claude Code CLI.
pub struct ClaudeProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeUserEventKind {
    RealQuery,
    ContextInjection,
    ToolResult,
    ProviderInternal,
    LocalCommand,
    Ignored,
}

pub(crate) fn classify_claude_user_event(parsed: &serde_json::Value) -> ClaudeUserEventKind {
    let Some(message) = parsed.get("message") else {
        return ClaudeUserEventKind::Ignored;
    };
    let Some(content) = message.get("content") else {
        return ClaudeUserEventKind::Ignored;
    };

    if content.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("type").and_then(|value| value.as_str()) == Some("tool_result"))
    }) {
        return ClaudeUserEventKind::ToolResult;
    }

    if has_claude_context_evidence(parsed) {
        return ClaudeUserEventKind::ContextInjection;
    }

    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return ClaudeUserEventKind::Ignored;
        }
        if is_claude_interruption_content(trimmed) {
            return ClaudeUserEventKind::ProviderInternal;
        }
        if is_claude_non_query_content(trimmed) {
            return ClaudeUserEventKind::LocalCommand;
        }
        return ClaudeUserEventKind::RealQuery;
    }

    let Some(items) = content.as_array() else {
        return ClaudeUserEventKind::Ignored;
    };

    if items.is_empty() {
        return ClaudeUserEventKind::Ignored;
    }
    if items.iter().any(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("text")
            && item
                .get("text")
                .and_then(|v| v.as_str())
                .is_some_and(|text| is_claude_interruption_content(text.trim()))
    }) {
        return ClaudeUserEventKind::ProviderInternal;
    }
    if items.iter().any(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("text")
            && item
                .get("text")
                .and_then(|v| v.as_str())
                .is_some_and(|text| !text.trim().is_empty())
    }) {
        return ClaudeUserEventKind::RealQuery;
    }

    ClaudeUserEventKind::Ignored
}

/// Returns the provider-native causal reference for a Claude transcript record.
///
/// Claude uses parent tool-use and transcript UUID fields for records that are
/// injected into the provider conversation or linked to an earlier transcript
/// event. These references are retained as normalized provenance. The injected
/// text itself is never inspected to classify a record.
pub(crate) fn claude_provider_causal_ref(parsed: &serde_json::Value) -> Option<String> {
    let message = parsed.get("message");
    first_nonempty_string(parsed, &["parent_tool_use_id", "parentToolUseId"])
        .or_else(|| {
            message.and_then(|value| {
                first_nonempty_string(value, &["parent_tool_use_id", "parentToolUseId"])
            })
        })
        .map(|value| format!("provider:tool_use:{value}"))
        .or_else(|| {
            first_nonempty_string(parsed, &["parent_uuid", "parentUuid"])
                .or_else(|| {
                    message.and_then(|value| {
                        first_nonempty_string(value, &["parent_uuid", "parentUuid"])
                    })
                })
                .map(|value| format!("provider:uuid:{value}"))
        })
}

pub(crate) fn claude_context_purpose(parsed: &serde_json::Value) -> &'static str {
    let skill_name = ["tool_name", "toolName", "name"]
        .iter()
        .filter_map(|key| parsed.get(*key).and_then(|value| value.as_str()))
        .chain(parsed.get("message").into_iter().flat_map(|message| {
            ["tool_name", "toolName", "name"]
                .iter()
                .filter_map(|key| message.get(*key).and_then(|value| value.as_str()))
        }))
        .any(|value| value.eq_ignore_ascii_case("skill"));
    if skill_name {
        "skill"
    } else {
        "context"
    }
}

fn has_claude_context_evidence(parsed: &serde_json::Value) -> bool {
    // parentUuid is ordinary transcript lineage on normal Claude user
    // records as well as context records. It is retained as normalized causal
    // reference, but cannot identify context without an explicit marker.
    let message = parsed.get("message");
    [parsed, message.unwrap_or(&serde_json::Value::Null)]
        .into_iter()
        .any(|value| {
            ["is_meta", "isMeta", "is_context", "isContext"]
                .iter()
                .any(|key| value.get(*key).and_then(|flag| flag.as_bool()) == Some(true))
                || first_nonempty_string(value, &["parent_tool_use_id", "parentToolUseId"])
                    .is_some()
        })
}

fn first_nonempty_string<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|candidate| candidate.as_str())
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
    })
}

fn is_claude_interruption_content(content: &str) -> bool {
    content.starts_with("[Request interrupted by user]")
        || content.starts_with("[Request interrupted by user for tool use]")
}

fn is_claude_non_query_content(content: &str) -> bool {
    content.starts_with("<local-command-caveat>")
        || content.starts_with("<command-name>")
        || content.starts_with("<command-message>")
        || content.starts_with("<command-args>")
        || content.starts_with("<local-command-stdout>")
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeProvider {
    pub fn new() -> Self {
        ClaudeProvider
    }

    #[cfg(not(target_os = "windows"))]
    fn find_unix_claude_in_paths<I>(paths: I) -> Option<String>
    where
        I: IntoIterator<Item = std::path::PathBuf>,
    {
        for path in paths {
            let full_path = path.join("claude");
            if full_path.exists() {
                return Some(full_path.to_string_lossy().to_string());
            }
        }

        None
    }

    fn assistant_event(parsed: &serde_json::Value) -> AgentEvent {
        let stop_reason = parsed
            .get("message")
            .and_then(|v| v.get("stop_reason"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if stop_reason == "end_turn" || stop_reason == "stop_sequence" {
            return AgentEvent::ModelResponse;
        }

        AgentEvent::Generating
    }
}

impl AgentProvider for ClaudeProvider {
    fn name(&self) -> &str {
        "Claude"
    }

    fn get_executable(&self) -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                let path_exts = std::env::var("PATHEXT")
                    .ok()
                    .map(|value| {
                        value
                            .split(';')
                            .filter_map(|segment| {
                                let trimmed = segment.trim();
                                if trimmed.is_empty() {
                                    None
                                } else {
                                    Some(trimmed.to_ascii_lowercase())
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|exts| !exts.is_empty())
                    .unwrap_or_else(|| {
                        vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()]
                    });

                for path in std::env::split_paths(&paths) {
                    if let Some(launch) =
                        crate::providers::npm::node_launch_from_npm_cmd_shim(&path, "claude")
                    {
                        return launch;
                    }

                    let direct = path.join("claude");
                    if direct.exists() {
                        return (direct.to_string_lossy().to_string(), vec![]);
                    }
                    for ext in &path_exts {
                        let candidate = path.join(format!("claude{ext}"));
                        if candidate.exists() {
                            return (candidate.to_string_lossy().to_string(), vec![]);
                        }
                    }
                }
            }

            if let Some(appdata) = dirs::data_dir() {
                let npm_dir = appdata.join("npm");
                if let Some(launch) =
                    crate::providers::npm::node_launch_from_npm_cmd_shim(&npm_dir, "claude")
                {
                    return launch;
                }

                let npm_claude = npm_dir.join("claude.cmd");
                if npm_claude.exists() {
                    return (npm_claude.to_string_lossy().to_string(), vec![]);
                }
            }

            ("claude".to_string(), vec![])
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Some(paths) = std::env::var_os("PATH") {
                if let Some(executable) =
                    Self::find_unix_claude_in_paths(std::env::split_paths(&paths))
                {
                    return (executable, vec![]);
                }
            }

            let home = dirs::home_dir().unwrap_or_default();
            let fallbacks = vec![
                home.join(".npm-global/bin/claude"),
                std::path::PathBuf::from("/usr/local/bin/claude"),
                std::path::PathBuf::from("/opt/homebrew/bin/claude"),
            ];
            for path in fallbacks {
                if path.exists() {
                    return (path.to_string_lossy().to_string(), vec![]);
                }
            }

            ("claude".to_string(), vec![])
        }
    }

    fn get_spawn_args(&self, config: &AgentConfig, is_resume: bool) -> Vec<String> {
        let claude = config.claude_config();
        let mut args: Vec<String> = vec![
            "--verbose".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--output-format".into(),
            "stream-json".into(),
        ];

        if let Some(ref model) = config.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if let Some(effort) = claude
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("--effort".into());
            args.push(effort.to_string());
        }

        if is_resume {
            // Rule: Old session -> --resume
            let resume_id = config
                .resume_session
                .as_deref()
                .unwrap_or(config.session_id.as_str());
            args.push("--resume".into());
            args.push(resume_id.to_string());
        } else {
            // Rule: New session -> --session-id
            let new_id = config
                .fresh_provider_session_id
                .as_deref()
                .unwrap_or(config.session_id.as_str());
            args.push("--session-id".into());
            args.push(new_id.to_string());

            if !config.session_name.trim().is_empty() {
                args.push("--name".into());
                args.push(config.session_name.clone());
            }
        }

        let mut final_includes = config
            .system_include_directories
            .clone()
            .unwrap_or_default();
        if let Some(ref user_dirs) = config.include_directories {
            for dir in user_dirs {
                if !final_includes.contains(dir) {
                    final_includes.push(dir.clone());
                }
            }
        }
        if !final_includes.is_empty() {
            for dir in final_includes {
                args.push("--add-dir".into());
                args.push(dir);
            }
        }

        if config.debug.unwrap_or(false) {
            args.push("--debug".into());
            args.push("api,hooks".into());
        }

        // Claude-specific parameters
        let permission_mode = effective_claude_permission_mode(claude.permission_mode.as_deref());
        args.push("--permission-mode".into());
        args.push(permission_mode.to_string());
        if let Some(ref tools) = claude.tools {
            if !tools.is_empty() {
                args.push("--tools".into());
                args.push(tools.join(","));
            }
        }
        if let Some(ref tools) = claude.allowed_tools {
            for tool in tools {
                args.push("--allowedTools".into());
                args.push(tool.clone());
            }
        }
        if let Some(ref tools) = claude.disallowed_tools {
            for tool in tools {
                args.push("--disallowedTools".into());
                args.push(tool.clone());
            }
        }
        if let Some(ref prompt) = claude.append_system_prompt {
            if !prompt.trim().is_empty() {
                args.push("--append-system-prompt".into());
                args.push(prompt.clone());
            }
        }
        if let Some(ref path) = claude.mcp_config {
            if !path.trim().is_empty() {
                args.push("--mcp-config".into());
                args.push(path.clone());
            }
        }
        if claude.strict_mcp_config.unwrap_or(false) {
            args.push("--strict-mcp-config".into());
        }

        // Custom args (shell-parsed) - users can supply additional flags here
        if let Some(ref custom) = config.custom_args {
            if let Some(parsed) = shlex::split(custom) {
                args.extend(parsed);
            }
        }

        args
    }

    fn parse_output(&self, line: &str) -> Option<AgentEvent> {
        let trimmed = line.trim();
        if trimmed.contains("Do you want to proceed?")
            || trimmed.contains("Allow reading from")
            || trimmed.contains("requires approval")
        {
            return Some(AgentEvent::ActionRequired { message: "".into() });
        }

        let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
        let msg_type = parsed.get("type")?.as_str()?;

        // Example Claude stream-json mapping:
        // Claude's exact JSON format is undocumented, so we will pass
        // most events directly mapping known keys, or fallback to returning Unknown
        // so that the frontend terminal logic can just render the raw JSON payload.
        match msg_type {
            "system" => {
                let subtype = parsed.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                match subtype {
                    "init" => {
                        let session_id = parsed
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let timestamp = parsed
                            .get("timestamp")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        Some(AgentEvent::Init {
                            session_id,
                            timestamp,
                        })
                    }
                    // Claude Code emits this when a tool call needs explicit permission
                    "permission_request" => {
                        let message = parsed
                            .get("tool_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        Some(AgentEvent::ActionRequired { message })
                    }
                    "turn_duration" => Some(AgentEvent::ModelResponse),
                    _ => Some(AgentEvent::Unknown),
                }
            }
            // Only count real user prompts as queries. Tool results are part of the same turn.
            "user" => match classify_claude_user_event(&parsed) {
                ClaudeUserEventKind::RealQuery => Some(AgentEvent::UserQuery),
                ClaudeUserEventKind::ContextInjection | ClaudeUserEventKind::ToolResult => {
                    Some(AgentEvent::Generating)
                }
                ClaudeUserEventKind::ProviderInternal
                | ClaudeUserEventKind::LocalCommand
                | ClaudeUserEventKind::Ignored => Some(AgentEvent::Unknown),
            },
            // Claude is actively streaming a response
            "assistant" => Some(Self::assistant_event(&parsed)),
            "message_stream" => Some(AgentEvent::Generating),
            "progress" => Some(AgentEvent::Generating),
            // Claude finished the full response turn
            "result" => Some(AgentEvent::TurnCompleted),
            _ => Some(AgentEvent::Unknown),
        }
    }

    fn get_instruction_filename(&self) -> &str {
        "AGENTS.md"
    }
}

fn normalize_claude_permission_mode(mode: &str) -> Option<&str> {
    match mode.trim() {
        "manual" | "acceptEdits" | "plan" | "auto" | "dontAsk" | "bypassPermissions" => {
            Some(mode.trim())
        }
        // Wardian used these non-provider values before Claude exposed its current modes.
        "default" => Some("manual"),
        "auto-accept" => Some("acceptEdits"),
        _ => None,
    }
}

pub(crate) fn effective_claude_permission_mode(mode: Option<&str>) -> &str {
    mode.and_then(normalize_claude_permission_mode)
        .unwrap_or("bypassPermissions")
}

pub(crate) fn claude_output_has_bypass_permissions_consent_prompt(output: &str) -> bool {
    // Ink's full-screen selector positions each word with cursor controls, so
    // match the exact modal phrases after removing terminal layout separators.
    let compact = strip_ansi_controls(output)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    compact.contains("claudecoderunninginbypasspermissionsmode")
        && compact.contains("byproceedingyouacceptallresponsibility")
        && compact.contains("yesiaccept")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::{ClaudeProviderConfig, ProviderConfig};

    fn make_provider() -> ClaudeProvider {
        ClaudeProvider::new()
    }

    fn make_claude_config(claude: ClaudeProviderConfig) -> AgentConfig {
        AgentConfig {
            provider: "claude".into(),
            provider_config: ProviderConfig::Claude(claude),
            ..Default::default()
        }
    }

    #[test]
    fn name_returns_claude() {
        let p = make_provider();
        assert_eq!(p.name(), "Claude");
    }

    #[test]
    fn instruction_filename_is_agents_md() {
        let p = make_provider();
        assert_eq!(p.get_instruction_filename(), "AGENTS.md");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_resolution_prefers_node_entrypoint_over_cmd_shim() {
        let _lock = crate::utils::wardian_test_env_lock();
        let previous_path = std::env::var_os("PATH");
        let temp = tempfile::tempdir().unwrap();
        let claude_js = temp
            .path()
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("cli.js");
        std::fs::create_dir_all(claude_js.parent().unwrap()).unwrap();
        std::fs::write(
            temp.path().join("claude.cmd"),
            r#"@ECHO off
SET dp0=%~dp0
"%dp0%\node.exe" "%dp0%\node_modules\@anthropic-ai\claude-code\cli.js" %*
"#,
        )
        .unwrap();
        std::fs::write(&claude_js, "console.log('claude')").unwrap();

        unsafe {
            std::env::set_var("PATH", temp.path());
        }

        let (executable, args) = ClaudeProvider::new().get_executable();

        assert_eq!(executable, "node");
        assert_eq!(args, vec![claude_js.to_string_lossy().to_string()]);

        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }

    #[test]
    fn spawn_args_minimal_config() {
        let p = make_provider();
        let config = AgentConfig::default();
        let args = p.get_spawn_args(&config, false);
        // Base persistent session arguments (no --print)
        assert_eq!(
            args[0..5],
            vec![
                "--verbose",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json"
            ]
        );
    }

    #[test]
    fn spawn_args_include_model_reasoning_effort() {
        let provider = make_provider();
        let config = AgentConfig {
            model: Some("sonnet".into()),
            provider_config: ProviderConfig::Claude(ClaudeProviderConfig {
                reasoning_effort: Some("high".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let args = provider.get_spawn_args(&config, false);

        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--model" && pair[1] == "sonnet"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--effort" && pair[1] == "high"));
    }

    #[test]
    fn parse_output_init_event() {
        let p = make_provider();
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","timestamp":"2026-01-01T00:00:00Z"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::Init {
                session_id: "abc-123".into(),
                timestamp: Some("2026-01-01T00:00:00Z".into()),
            }
        );
    }

    #[test]
    fn parse_output_permission_request() {
        let p = make_provider();
        let line = r#"{"type":"system","subtype":"permission_request","tool_name":"bash"}"#;
        let event = p.parse_output(line).unwrap();
        assert_eq!(
            event,
            AgentEvent::ActionRequired {
                message: "bash".into()
            }
        );
    }

    #[test]
    fn parse_output_assistant_is_generating() {
        let p = make_provider();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[]}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_assistant_end_turn_is_idle() {
        let p = make_provider();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::ModelResponse);
    }

    #[test]
    fn parse_output_assistant_tool_use_is_generating() {
        let p = make_provider();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","id":"tool-1","input":{"command":"git status"}}],"stop_reason":"tool_use"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_result_is_turn_completed() {
        let p = make_provider();
        let line = r#"{"type":"result","subtype":"success","result":"done"}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnCompleted);
    }

    #[test]
    fn parse_output_user_is_query() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":"hello"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_user_with_transcript_parent_uuid_is_still_a_query() {
        let p = make_provider();
        let line = r#"{"type":"user","parentUuid":"assistant-1","message":{"role":"user","content":"hello"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::UserQuery);
    }

    #[test]
    fn parse_output_user_tool_result_is_generating() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
    }

    #[test]
    fn parse_output_user_empty_content_is_not_query() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":[]}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_user_local_command_is_not_query() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_user_local_command_stdout_is_not_query() {
        let p = make_provider();
        let line = r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to Opus 4.6</local-command-stdout>"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_user_interruption_is_not_query() {
        let p = make_provider();
        let line = r#"{"type":"user","parentUuid":"assistant-1","message":{"role":"user","content":"[Request interrupted by user for tool use]"}}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn latest_query_timestamp_ignores_claude_interruptions_after_a_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("conversation.jsonl");
        let prompt = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-31T12:00:00.000Z",
            "parentUuid": "assistant-1",
            "message": { "role": "user", "content": "hello" },
        });
        let short_interruption = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-31T12:01:00.000Z",
            "parentUuid": "assistant-1",
            "message": {
                "role": "user",
                "content": "[Request interrupted by user]",
            },
        });
        let long_interruption = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-31T12:02:00.000Z",
            "parentUuid": "assistant-1",
            "message": {
                "role": "user",
                "content": "[Request interrupted by user for tool use]",
            },
        });
        std::fs::write(
            &log,
            [prompt, short_interruption, long_interruption]
                .into_iter()
                .map(|record| serde_json::to_string(&record).unwrap())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        assert_eq!(
            crate::manager::telemetry::latest_query_timestamp_from_log_suffix(&log, "claude")
                .as_deref(),
            Some("2026-08-31T12:00:00.000Z")
        );
    }

    #[test]
    fn classify_claude_interruption_records_as_provider_internal() {
        for content in [
            "[Request interrupted by user]",
            "[Request interrupted by user for tool use]",
        ] {
            let parsed = serde_json::json!({
                "type": "user",
                "parentUuid": "assistant-1",
                "message": { "role": "user", "content": content },
            });
            assert_eq!(
                classify_claude_user_event(&parsed),
                ClaudeUserEventKind::ProviderInternal
            );
        }
    }

    #[test]
    fn parse_output_turn_duration_is_idle() {
        let p = make_provider();
        let line = r#"{"type":"system","subtype":"turn_duration","durationMs":1234}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::ModelResponse);
    }

    #[test]
    fn parse_output_unknown_type() {
        let p = make_provider();
        let line = r#"{"type":"something_new","data":42}"#;
        assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
    }

    #[test]
    fn parse_output_invalid_json() {
        let p = make_provider();
        assert!(p.parse_output("not json").is_none());
    }

    #[test]
    fn spawn_args_permission_mode() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            permission_mode: Some("auto-accept".into()),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"acceptEdits".to_string()));
    }

    #[test]
    fn spawn_args_bypass_permissions_by_default() {
        let args = make_provider().get_spawn_args(&make_claude_config(Default::default()), false);

        assert!(args
            .windows(2)
            .any(|pair| { pair[0] == "--permission-mode" && pair[1] == "bypassPermissions" }));
    }

    #[test]
    fn bypass_permissions_consent_classifier_handles_ink_cursor_layout() {
        let consent = "WARNING:\x1b[12GClaude\x1b[19GCode\x1b[24Grunning\x1b[32Gin\x1b[35GBypass\x1b[42GPermissions\x1b[54Gmode\r\nBy\x1b[6Gproceeding,\x1b[18Gyou\x1b[22Gaccept\x1b[29Gall\x1b[33Gresponsibility\r\n❯ No, exit\r\n  Yes, I accept";
        assert!(claude_output_has_bypass_permissions_consent_prompt(consent));
        assert!(!claude_output_has_bypass_permissions_consent_prompt(
            "Allow Bash command? Yes / No",
        ));
    }

    #[test]
    fn legacy_default_permission_mode_maps_to_manual() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            permission_mode: Some("default".into()),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"manual".to_string()));
        assert!(!args.contains(&"default".to_string()));
    }

    #[test]
    fn interactive_spawn_omits_print_only_max_turns() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            max_turns: Some(10),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        assert!(!args.contains(&"--max-turns".to_string()));
    }

    #[test]
    fn spawn_args_allowed_tools() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            allowed_tools: Some(vec!["Read".into(), "Write".into()]),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        let count = args.iter().filter(|a| *a == "--allowedTools").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn spawn_args_disallowed_tools() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            disallowed_tools: Some(vec!["Bash".into()]),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--disallowedTools".to_string()));
        assert!(args.contains(&"Bash".to_string()));
    }

    #[test]
    fn spawn_args_append_system_prompt() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            append_system_prompt: Some("Always respond in JSON".into()),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--append-system-prompt".to_string()));
        assert!(args.contains(&"Always respond in JSON".to_string()));
    }

    #[test]
    fn spawn_args_mcp_config() {
        let p = make_provider();
        let config = make_claude_config(ClaudeProviderConfig {
            tools: Some(vec!["Read".into(), "Edit".into()]),
            mcp_config: Some("/path/to/mcp.json".into()),
            strict_mcp_config: Some(true),
            ..Default::default()
        });
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/path/to/mcp.json".to_string()));
        assert!(args.contains(&"--tools".to_string()));
        assert!(args.contains(&"Read,Edit".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
    }

    #[test]
    fn spawn_args_name_skipped_on_resume() {
        let p = make_provider();
        let config = AgentConfig {
            session_name: "MyAgent".into(),
            resume_session: Some("session-abc".into()),
            ..Default::default()
        };
        // Fresh spawn includes --name
        let args_fresh = p.get_spawn_args(&config, false);
        assert!(args_fresh.contains(&"--name".to_string()));
        // Resume omits --name
        let args_resume = p.get_spawn_args(&config, true);
        assert!(!args_resume.contains(&"--name".to_string()));
        assert!(args_resume.contains(&"--resume".to_string()));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_path_lookup_prefers_discovered_claude_binary() {
        let temp = tempfile::tempdir().expect("temp dir");
        let claude_path = temp.path().join("claude");
        std::fs::write(&claude_path, "").expect("create fake claude");

        let resolved = ClaudeProvider::find_unix_claude_in_paths(vec![temp.path().to_path_buf()]);

        assert_eq!(resolved, Some(claude_path.to_string_lossy().to_string()));
    }

    #[test]
    fn fresh_spawn_uses_explicit_session_id() {
        let p = make_provider();
        let config = AgentConfig {
            session_id: "019d331a-0500-7592-969f-8f437886f42b".into(),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&"019d331a-0500-7592-969f-8f437886f42b".to_string()));
    }

    #[test]
    fn fresh_spawn_prefers_transient_provider_session_id() {
        let p = make_provider();
        let config = AgentConfig {
            session_id: "stable-wardian-id".into(),
            fresh_provider_session_id: Some("fresh-claude-id".into()),
            ..Default::default()
        };
        let args = p.get_spawn_args(&config, false);
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&"fresh-claude-id".to_string()));
        assert!(!args.contains(&"stable-wardian-id".to_string()));
    }
}
