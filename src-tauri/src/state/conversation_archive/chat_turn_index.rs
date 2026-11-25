//! Incremental canonical turn metadata in the existing immutable Store.
//! Admission, source qualification and publication remain with the archive owner.
//! No transcript bodies, raw records or delivery inference live here. Preview
//! hints are bounded metadata retained only by the existing in-memory owner.
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::io;

#[cfg(test)]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};
#[cfg(test)]
use wardian_core::models::chat::{AgentChatRole, AgentChatStatus};
use wardian_core::models::chat_presentation::*;

#[cfg(test)]
use super::chat_read_store::{digest, valid_ref, SeekCache, Store};

const LABEL_BYTES: usize = 160;

/// Only roots belong in the Head. Their AVL values contain bounded metadata;
/// normal page reads never enumerate a turn or open archive evidence.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Index {
    version: u8,
    scope: Option<String>,
    turns: Option<String>,
    items: Option<String>,
    events: Option<String>,
}

impl Index {
    /// Allows an empty additive index to be omitted without changing old Heads.
    pub(super) fn is_empty(&self) -> bool {
        self.version == 0
            && self.scope.is_none()
            && self.turns.is_none()
            && self.items.is_none()
            && self.events.is_none()
    }
}

fn string<'a>(event: &'a AgentChatEvent, field: &str) -> Option<&'a str> {
    event.metadata[field]
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

fn safe_label(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= LABEL_BYTES
        && !value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '{' | '}' | '[' | ']')))
    .then_some(value)
}

pub(super) fn label(event: &AgentChatEvent) -> (String, ChatToolCategory, String) {
    let tool = string(event, "tool_name")
        .or_else(|| {
            (event.kind == AgentChatEventKind::ToolCall)
                .then_some(event.title.as_deref())
                .flatten()
        })
        .filter(|tool| {
            tool.len() <= 64
                && tool
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"_-.:".contains(&ch))
                && !matches!(
                    *tool,
                    "function_call"
                        | "custom_tool_call"
                        | "tool_call"
                        | "tool_use"
                        | "tool_result"
                        | "function_call_output"
                        | "custom_tool_call_output"
                        | "assistant"
                        | "result"
                )
        })
        .unwrap_or("tool");
    let normalized = tool.to_ascii_lowercase();
    let category = match normalized.as_str() {
        "bash" | "shell" | "shell_command" | "exec_command" | "exec" | "run_command"
        | "terminal" => ChatToolCategory::Shell,
        "read" | "read_file" | "readfile" | "view_file" => ChatToolCategory::Read,
        "edit"
        | "write"
        | "edit_file"
        | "write_file"
        | "apply_patch"
        | "multiedit"
        | "write_to_file"
        | "replace_file_content" => ChatToolCategory::Edit,
        "browser" | "navigate" | "list_tabs" | "open_tab" | "browser_snapshot" => {
            ChatToolCategory::Browser
        }
        _ => ChatToolCategory::Tool,
    };
    let input = &event.metadata["tool_input"];
    let basename = event
        .path
        .as_deref()
        .or_else(|| string(event, "file_path"))
        .or_else(|| {
            ["file_path", "path", "AbsolutePath", "FilePath"]
                .into_iter()
                .find_map(|field| input[field].as_str())
        })
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|path| !matches!(*path, "." | ".."))
        .and_then(safe_label);
    let description = match category {
        ChatToolCategory::Read => {
            basename.map_or_else(|| "Read file".into(), |name| format!("Read {name}"))
        }
        ChatToolCategory::Edit => {
            basename.map_or_else(|| "Update files".into(), |name| format!("Update {name}"))
        }
        ChatToolCategory::Shell => input["description"]
            .as_str()
            .and_then(safe_label)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let executable = event
                    .command
                    .as_deref()
                    .and_then(|command| command.split_whitespace().next())
                    .and_then(|path| path.rsplit(['/', '\\']).next())
                    .filter(|name| {
                        name.len() <= 48
                            && name
                                .bytes()
                                .all(|ch| ch.is_ascii_alphanumeric() || b"_.-".contains(&ch))
                    });
                executable.map_or_else(|| "Run command".into(), |name| format!("Run {name}"))
            }),
        ChatToolCategory::Browser
            if normalized == "list_tabs" || input["action"] == "list_tabs" =>
        {
            "Inspect open tabs".into()
        }
        ChatToolCategory::Browser => "Inspect browser".into(),
        ChatToolCategory::Tool => format!("Run {tool}"),
    };
    (tool.into(), category, description)
}

#[cfg(test)]
#[path = "chat_turn_index_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "chat_turn_preview.rs"]
mod chat_turn_preview;
#[cfg(test)]
pub(super) use chat_turn_preview::preview;

#[cfg(test)]
include!("chat_turn_index_legacy_fixture.inc.rs");
