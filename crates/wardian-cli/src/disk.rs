use std::io;

use serde::Serialize;
use wardian_core::models::WorkflowDefinition;

pub fn list_workflows_from_disk() -> io::Result<Vec<WorkflowDefinition>> {
    let home = wardian_core::paths::wardian_home()
        .ok_or_else(|| io::Error::other("WARDIAN_HOME not set"))?;
    let workflows_dir = home.join("workflows");
    if !workflows_dir.exists() {
        return Ok(vec![]);
    }

    let mut workflows = vec![];
    for entry in std::fs::read_dir(&workflows_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let content = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<WorkflowDefinition>(&content) {
            Ok(workflow) => workflows.push(workflow),
            Err(error) => eprintln!(
                "warning: skipped malformed workflow file {}: {}",
                path.display(),
                error
            ),
        }
    }
    Ok(workflows)
}

pub fn workflow_summaries(
    workflows: &[WorkflowDefinition],
) -> Vec<wardian_core::control::WorkflowSummary> {
    workflows
        .iter()
        .map(|workflow| wardian_core::control::WorkflowSummary {
            id: workflow.id.clone(),
            name: workflow.name.clone(),
            node_count: workflow.nodes.len(),
        })
        .collect()
}

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
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn load_watchlist_state_accepts_v2_teams_and_entries() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", temp.path());
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
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn load_watchlist_state_accepts_legacy_array() {
        let _guard = env_lock();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", temp.path());
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
        std::env::remove_var("WARDIAN_HOME");
    }
}
