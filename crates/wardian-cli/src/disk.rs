use std::io;

use serde::Serialize;
use wardian_core::control::ConversationShowResponse;
use wardian_core::conversations::{
    read_jsonl_records, read_latest_index_entries, ConversationIndexEntry, ConversationManifest,
    ConversationNarrativeRecord,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchlistState {
    pub version: u8,
    pub watchlists: Vec<WatchlistSummary>,
    pub teams: Vec<TeamSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TeamSummary {
    pub id: String,
    pub name: String,
    pub agent_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchlistSummary {
    pub id: String,
    pub name: String,
    pub entries: Vec<WatchlistEntry>,
    pub agent_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WatchlistEntry {
    Agent { agent_id: String },
    Team { team_id: String },
}

impl WatchlistEntry {
    #[cfg(test)]
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Agent { agent_id } => Some(agent_id),
            Self::Team { .. } => None,
        }
    }

    #[cfg(test)]
    pub fn team_id(&self) -> Option<&str> {
        match self {
            Self::Agent { .. } => None,
            Self::Team { team_id } => Some(team_id),
        }
    }
}

pub fn load_watchlist_state() -> io::Result<WatchlistState> {
    let home = wardian_core::paths::wardian_home()
        .ok_or_else(|| io::Error::other("WARDIAN_HOME not set"))?;
    let path = home.join("watchlists").join("index.json");
    if !path.exists() {
        return Ok(empty_watchlist_state());
    }

    let content = std::fs::read_to_string(path)?;
    let value: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(normalize_watchlist_state(&value))
}

pub fn load_conversation_list(
    agent: Option<&str>,
    scope_all: bool,
) -> io::Result<Vec<ConversationIndexEntry>> {
    if let Some(agent_id) = agent {
        return read_agent_conversation_index(agent_id);
    }

    if scope_all {
        return read_all_agent_conversation_indexes();
    }

    let agent_id = std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--agent, --scope all, or WARDIAN_SESSION_ID is required",
            )
        })?;
    read_agent_conversation_index(&agent_id)
}

pub fn load_conversation_show(conversation_id: &str) -> io::Result<ConversationShowResponse> {
    let entry = read_all_agent_conversation_indexes()?
        .into_iter()
        .find(|entry| entry.conversation_id == conversation_id)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("conversation not found: {conversation_id}"),
            )
        })?;
    let conversation_dir =
        wardian_core::paths::agent_conversation_dir(&entry.agent_id, &entry.conversation_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "unsafe conversation path")
            })?;
    let manifest = read_conversation_manifest(&conversation_dir.join("manifest.json"))?;
    let conversation = read_jsonl_records::<ConversationNarrativeRecord>(
        &conversation_dir.join("conversation.jsonl"),
    )?;
    Ok(ConversationShowResponse::new(manifest, conversation))
}

fn read_agent_conversation_index(agent_id: &str) -> io::Result<Vec<ConversationIndexEntry>> {
    let index_path = wardian_core::paths::agent_conversations_dir(agent_id)
        .map(|dir| dir.join("index.jsonl"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsafe agent path"))?;
    read_latest_index_entries(&index_path)
}

fn read_all_agent_conversation_indexes() -> io::Result<Vec<ConversationIndexEntry>> {
    let Some(agents_dir) = wardian_core::paths::agents_dir() else {
        return Ok(Vec::new());
    };
    if !agents_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(agents_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(agent_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        entries.extend(read_agent_conversation_index(&agent_id)?);
    }
    sort_conversation_entries(&mut entries);
    Ok(entries)
}

fn sort_conversation_entries(entries: &mut [ConversationIndexEntry]) {
    entries.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then_with(|| left.conversation_id.cmp(&right.conversation_id))
    });
}

fn read_conversation_manifest(path: &std::path::Path) -> io::Result<ConversationManifest> {
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content).map_err(io::Error::other)
}

fn empty_watchlist_state() -> WatchlistState {
    WatchlistState {
        version: 2,
        watchlists: Vec::new(),
        teams: Vec::new(),
    }
}

fn normalize_watchlist_state(value: &serde_json::Value) -> WatchlistState {
    if value.get("version").and_then(|v| v.as_u64()) == Some(2) {
        return WatchlistState {
            version: 2,
            teams: value
                .get("teams")
                .and_then(|teams| teams.as_array())
                .map(|teams| teams.iter().filter_map(parse_team).collect())
                .unwrap_or_default(),
            watchlists: value
                .get("watchlists")
                .and_then(|watchlists| watchlists.as_array())
                .map(|watchlists| watchlists.iter().filter_map(parse_watchlist).collect())
                .unwrap_or_default(),
        };
    }

    WatchlistState {
        version: 2,
        teams: Vec::new(),
        watchlists: value
            .as_array()
            .map(|watchlists| watchlists.iter().filter_map(parse_watchlist).collect())
            .unwrap_or_default(),
    }
}

fn parse_team(value: &serde_json::Value) -> Option<TeamSummary> {
    let id = value.get("id")?.as_str()?.to_string();
    let name = value
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or("Team")
        .to_string();
    Some(TeamSummary {
        id,
        name,
        agent_ids: string_array(value, "agentIds")
            .or_else(|| string_array(value, "agent_ids"))
            .unwrap_or_default(),
    })
}

fn parse_watchlist(value: &serde_json::Value) -> Option<WatchlistSummary> {
    let id = value.get("id")?.as_str()?.to_string();
    let name = value
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or("List")
        .to_string();
    let agent_ids = string_array(value, "agentIds")
        .or_else(|| string_array(value, "agent_ids"))
        .unwrap_or_default();
    let entries = value
        .get("entries")
        .and_then(|entries| entries.as_array())
        .map(|entries| entries.iter().filter_map(parse_watchlist_entry).collect())
        .unwrap_or_else(|| {
            agent_ids
                .iter()
                .map(|agent_id| WatchlistEntry::Agent {
                    agent_id: agent_id.clone(),
                })
                .collect()
        });

    Some(WatchlistSummary {
        id,
        name,
        entries,
        agent_ids,
    })
}

fn parse_watchlist_entry(value: &serde_json::Value) -> Option<WatchlistEntry> {
    match value.get("type").and_then(|kind| kind.as_str()) {
        Some("agent") => value
            .get("agentId")
            .or_else(|| value.get("agent_id"))
            .and_then(|id| id.as_str())
            .map(|agent_id| WatchlistEntry::Agent {
                agent_id: agent_id.to_string(),
            }),
        Some("team") => value
            .get("teamId")
            .or_else(|| value.get("team_id"))
            .and_then(|id| id.as_str())
            .map(|team_id| WatchlistEntry::Team {
                team_id: team_id.to_string(),
            }),
        _ => None,
    }
}

fn string_array(value: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    Some(
        value
            .get(key)?
            .as_array()?
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct WardianHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
        previous_session_id: Option<std::ffi::OsString>,
    }

    impl WardianHomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let guard = Self {
                _lock: crate::test_env_lock(),
                previous_home: std::env::var_os("WARDIAN_HOME"),
                previous_session_id: std::env::var_os("WARDIAN_SESSION_ID"),
            };
            std::env::set_var("WARDIAN_HOME", path);
            std::env::remove_var("WARDIAN_SESSION_ID");
            guard
        }
    }

    impl Drop for WardianHomeGuard {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
            match self.previous_session_id.take() {
                Some(value) => std::env::set_var("WARDIAN_SESSION_ID", value),
                None => std::env::remove_var("WARDIAN_SESSION_ID"),
            }
        }
    }

    fn write_conversation_fixture(
        home: &std::path::Path,
        agent_id: &str,
        conversation_id: &str,
        started_at: &str,
        unsafe_index_path: Option<&str>,
    ) {
        let conversation_dir = home
            .join("agents")
            .join(agent_id)
            .join("conversations")
            .join(conversation_id);
        std::fs::create_dir_all(&conversation_dir).unwrap();
        std::fs::write(
            conversation_dir.join("manifest.json"),
            format!(
                r#"{{
                  "schema": 1,
                  "conversation_id": "{conversation_id}",
                  "agent_id": "{agent_id}",
                  "agent_name": "{agent_id}",
                  "agent_class": "Coder",
                  "workspace": "<absolute-workspace-path>",
                  "provider": "codex",
                  "provider_session_ids": ["{conversation_id}"],
                  "effective_logging": "enabled",
                  "created_at": "{started_at}",
                  "updated_at": "{started_at}",
                  "closed_at": null,
                  "status": "open",
                  "boundary_reason": "spawn",
                  "format_versions": {{
                    "manifest": 1,
                    "conversation": 1,
                    "events": 1,
                    "sources": 1
                  }}
                }}"#
            ),
        )
        .unwrap();
        std::fs::write(
            conversation_dir.join("conversation.jsonl"),
            format!(
                r#"{{"schema":1,"seq":1,"at":"{started_at}","kind":"message","role":"assistant","speaker_type":"assistant","text":"hello from {conversation_id}","tool":null,"status":null,"summary":null,"excerpt":null,"event_refs":[],"source_refs":[],"artifact_refs":[]}}"#
            ),
        )
        .unwrap();

        let index_path = home
            .join("agents")
            .join(agent_id)
            .join("conversations")
            .join("index.jsonl");
        std::fs::write(
            index_path,
            format!(
                r#"{{"schema":1,"conversation_id":"{conversation_id}","agent_id":"{agent_id}","agent_name":"{agent_id}","agent_class":"Coder","workspace":"<absolute-workspace-path>","provider":"codex","provider_session_ids":["{conversation_id}"],"started_at":"{started_at}","ended_at":null,"status":"open","boundary_reason":"spawn","first_prompt_excerpt":null,"last_record_excerpt":"hello from {conversation_id}","record_count":1,"artifact_count":0,"path":"{}"}}"#,
                unsafe_index_path.unwrap_or("ignored")
            ),
        )
        .unwrap();
    }

    #[test]
    fn load_watchlist_state_accepts_v2_teams_and_entries() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = WardianHomeGuard::set(temp.path());
        let dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.json"),
            r#"{
              "version": 2,
              "teams": [{"id":"team-1","name":"Review","agentIds":["agent-1","agent-2"]}],
              "watchlists": [{"id":"list-1","name":"Main","entries":[{"type":"team","teamId":"team-1"}]}]
            }"#,
        )
        .unwrap();

        let state = load_watchlist_state().unwrap();

        assert_eq!(state.version, 2);
        assert_eq!(state.teams[0].agent_ids, vec!["agent-1", "agent-2"]);
        assert_eq!(state.watchlists[0].entries[0].team_id(), Some("team-1"));
    }

    #[test]
    fn load_watchlist_state_accepts_legacy_array() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = WardianHomeGuard::set(temp.path());
        let dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.json"),
            r#"[{"id":"list-1","name":"Main","agentIds":["agent-1"]}]"#,
        )
        .unwrap();

        let state = load_watchlist_state().unwrap();

        assert_eq!(state.version, 2);
        assert!(state.teams.is_empty());
        assert_eq!(state.watchlists[0].agent_ids, vec!["agent-1"]);
        assert_eq!(state.watchlists[0].entries[0].agent_id(), Some("agent-1"));
    }

    #[test]
    fn conversation_list_current_uses_wardian_session_id() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = WardianHomeGuard::set(temp.path());
        std::env::set_var("WARDIAN_SESSION_ID", "agent-current");
        write_conversation_fixture(
            temp.path(),
            "agent-current",
            "conv-current",
            "2026-06-16T10:00:00.000Z",
            None,
        );
        write_conversation_fixture(
            temp.path(),
            "agent-other",
            "conv-other",
            "2026-06-16T11:00:00.000Z",
            None,
        );

        let conversations = load_conversation_list(None, false).unwrap();

        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].conversation_id, "conv-current");
        assert_eq!(conversations[0].agent_id, "agent-current");
    }

    #[test]
    fn conversation_list_scope_all_scans_agent_indexes() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = WardianHomeGuard::set(temp.path());
        write_conversation_fixture(
            temp.path(),
            "agent-a",
            "conv-a",
            "2026-06-16T10:00:00.000Z",
            None,
        );
        write_conversation_fixture(
            temp.path(),
            "agent-b",
            "conv-b",
            "2026-06-16T11:00:00.000Z",
            None,
        );

        let conversations = load_conversation_list(None, true).unwrap();

        assert_eq!(
            conversations
                .iter()
                .map(|entry| entry.conversation_id.as_str())
                .collect::<Vec<_>>(),
            vec!["conv-b", "conv-a"]
        );
    }

    #[test]
    fn conversation_show_reads_safe_agent_owned_path() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = WardianHomeGuard::set(temp.path());
        write_conversation_fixture(
            temp.path(),
            "agent-a",
            "conv-a",
            "2026-06-16T10:00:00.000Z",
            Some("../../outside"),
        );

        let response = load_conversation_show("conv-a").unwrap();

        assert_eq!(response.manifest.conversation_id, "conv-a");
        assert_eq!(response.manifest.agent_id, "agent-a");
        assert_eq!(
            response.conversation[0].text.as_deref(),
            Some("hello from conv-a")
        );
    }
}
