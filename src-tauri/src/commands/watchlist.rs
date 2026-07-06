use std::collections::BTreeSet;
use std::path::Path;
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub async fn load_watchlists(_app: AppHandle) -> Result<serde_json::Value, String> {
    if let Some(app_dir) = crate::utils::fs::get_wardian_home() {
        let path = app_dir.join("watchlists/index.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or_else(|_| serde_json::json!([]));
            return Ok(parsed);
        }
    }
    Ok(serde_json::json!([]))
}

#[tauri::command]
pub async fn save_watchlists(watchlists: serde_json::Value, app: AppHandle) -> Result<(), String> {
    let app_dir = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(&app_dir);
    let _ = std::fs::create_dir_all(app_dir.join("watchlists"));
    let path = app_dir.join("watchlists/index.json");
    let json = serde_json::to_string_pretty(&watchlists).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;

    // Seed team cliques into topology when teams are created or members are
    // added. Save and notify only when seeding actually added edges — plain
    // watchlist saves (reorders, renames) must not churn topology.json or
    // trigger graph refreshes.
    if let Some(teams) = watchlists.get("teams").and_then(|v| v.as_array()) {
        let mut topology = wardian_core::topology::load_topology(&app_dir);
        let now = chrono::Utc::now().to_rfc3339();
        let mut edges_added = 0;
        for team in teams {
            let agent_ids = team_agent_ids(team);
            if !agent_ids.is_empty() {
                edges_added +=
                    wardian_core::topology::seed_team_clique(&mut topology, &agent_ids, &now);
            }
        }
        if edges_added > 0 {
            if let Err(e) = wardian_core::topology::save_topology(&app_dir, &topology) {
                crate::manager::log_debug(&format!(
                    "[Wardian] topology seeding on watchlist save failed: {e}"
                ));
            } else {
                let _ = app.emit("topology-changed", ());
            }
        }
    }

    Ok(())
}

fn watchlist_index_path(home: &Path) -> std::path::PathBuf {
    home.join("watchlists").join("index.json")
}

fn team_agent_ids(team: &serde_json::Value) -> Vec<String> {
    team.get("agentIds")
        .or_else(|| team.get("agent_ids"))
        .and_then(|value| value.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn preserve_clone_team_placement_in_watchlist_state(
    state: &mut serde_json::Value,
    source_agent_id: &str,
    clone_agent_id: &str,
) -> bool {
    if source_agent_id.is_empty()
        || clone_agent_id.is_empty()
        || source_agent_id == clone_agent_id
        || state.get("version").and_then(|value| value.as_u64()) != Some(2)
    {
        return false;
    }

    let Some(teams) = state
        .get_mut("teams")
        .and_then(|value| value.as_array_mut())
    else {
        return false;
    };
    let Some(source_team_index) = teams
        .iter()
        .position(|team| team_agent_ids(team).iter().any(|id| id == source_agent_id))
    else {
        return false;
    };

    let before = serde_json::Value::Array(teams.clone());
    for (index, team) in teams.iter_mut().enumerate() {
        let mut agent_ids = team_agent_ids(team)
            .into_iter()
            .filter(|id| id != clone_agent_id)
            .collect::<Vec<_>>();
        if index == source_team_index {
            if let Some(source_index) = agent_ids.iter().position(|id| id == source_agent_id) {
                agent_ids.insert(source_index + 1, clone_agent_id.to_string());
            }
        }
        if let Some(object) = team.as_object_mut() {
            object.remove("agent_ids");
            object.insert(
                "agentIds".to_string(),
                serde_json::Value::Array(agent_ids.into_iter().map(Into::into).collect()),
            );
        }
    }
    teams.retain(|team| !team_agent_ids(team).is_empty());

    serde_json::Value::Array(teams.clone()) != before
}

pub(crate) fn preserve_clone_team_placement(
    app: &AppHandle,
    source_agent_id: &str,
    clone_agent_id: &str,
) -> Result<bool, String> {
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        return Ok(false);
    };
    let changed = preserve_clone_team_placement_in_home(&home, source_agent_id, clone_agent_id)?;
    if changed {
        let _ = app.emit("watchlists-updated", ());
    }
    Ok(changed)
}

pub(crate) fn preserve_clone_team_placement_in_home(
    home: &Path,
    source_agent_id: &str,
    clone_agent_id: &str,
) -> Result<bool, String> {
    let path = watchlist_index_path(home);
    if !path.exists() {
        return Ok(false);
    }

    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut state = serde_json::from_str::<serde_json::Value>(&data).map_err(|e| e.to_string())?;
    if !preserve_clone_team_placement_in_watchlist_state(
        &mut state,
        source_agent_id,
        clone_agent_id,
    ) {
        return Ok(false);
    }

    let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(true)
}

pub(crate) fn retain_known_agent_references_in_home(
    home: &Path,
    known_agent_ids: &BTreeSet<String>,
) -> Result<bool, String> {
    let path = watchlist_index_path(home);
    if !path.exists() {
        return Ok(false);
    }

    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut state = serde_json::from_str::<serde_json::Value>(&data).map_err(|e| e.to_string())?;
    if !retain_known_agent_references_in_watchlist_state(&mut state, known_agent_ids) {
        return Ok(false);
    }

    let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(true)
}

pub(crate) fn retain_known_agent_references_in_watchlist_state(
    state: &mut serde_json::Value,
    known_agent_ids: &BTreeSet<String>,
) -> bool {
    let before = state.clone();
    let remaining_team_ids = retain_known_agents_in_teams(state, known_agent_ids);
    retain_known_agents_in_watchlists(state, known_agent_ids, &remaining_team_ids);
    *state != before
}

fn retain_known_agents_in_teams(
    state: &mut serde_json::Value,
    known_agent_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    let Some(teams) = state
        .get_mut("teams")
        .and_then(|value| value.as_array_mut())
    else {
        return BTreeSet::new();
    };

    for team in teams.iter_mut() {
        let agent_ids = team_agent_ids(team)
            .into_iter()
            .filter(|id| known_agent_ids.contains(id))
            .collect::<Vec<_>>();
        if let Some(object) = team.as_object_mut() {
            object.remove("agent_ids");
            object.insert(
                "agentIds".to_string(),
                serde_json::Value::Array(agent_ids.into_iter().map(Into::into).collect()),
            );
        }
    }
    teams.retain(|team| !team_agent_ids(team).is_empty());

    teams
        .iter()
        .filter_map(|team| {
            team.get("id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn retain_known_agents_in_watchlists(
    state: &mut serde_json::Value,
    known_agent_ids: &BTreeSet<String>,
    remaining_team_ids: &BTreeSet<String>,
) {
    let Some(watchlists) = state
        .get_mut("watchlists")
        .and_then(|value| value.as_array_mut())
    else {
        return;
    };

    for watchlist in watchlists {
        retain_known_agents_in_watchlist(watchlist, known_agent_ids, remaining_team_ids);
    }
}

fn retain_known_agents_in_watchlist(
    watchlist: &mut serde_json::Value,
    known_agent_ids: &BTreeSet<String>,
    remaining_team_ids: &BTreeSet<String>,
) {
    let Some(object) = watchlist.as_object_mut() else {
        return;
    };

    let direct_agent_ids = object
        .get("agentIds")
        .or_else(|| object.get("agent_ids"))
        .and_then(|value| value.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str())
                .filter(|id| known_agent_ids.contains(*id))
                .map(str::to_string)
                .collect::<Vec<_>>()
        });

    if let Some(agent_ids) = direct_agent_ids {
        object.remove("agent_ids");
        object.insert(
            "agentIds".to_string(),
            serde_json::Value::Array(agent_ids.into_iter().map(Into::into).collect()),
        );
    }

    if let Some(entries) = object
        .get_mut("entries")
        .and_then(|value| value.as_array_mut())
    {
        entries.retain(|entry| {
            let entry_type = entry.get("type").and_then(|value| value.as_str());
            match entry_type {
                Some("agent") => entry
                    .get("agentId")
                    .or_else(|| entry.get("agent_id"))
                    .and_then(|value| value.as_str())
                    .is_some_and(|id| known_agent_ids.contains(id)),
                Some("team") => entry
                    .get("teamId")
                    .or_else(|| entry.get("team_id"))
                    .and_then(|value| value.as_str())
                    .is_some_and(|id| remaining_team_ids.contains(id)),
                _ => true,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::preserve_clone_team_placement_in_home;
    use super::preserve_clone_team_placement_in_watchlist_state;
    use super::retain_known_agent_references_in_watchlist_state;
    use std::collections::BTreeSet;

    #[test]
    fn clone_team_placement_inserts_clone_after_source_and_removes_from_other_teams() {
        let mut state = serde_json::json!({
            "version": 2,
            "teams": [
                { "id": "team-a", "name": "Wardian Dev", "agentIds": ["source", "beta"] },
                { "id": "team-b", "name": "Other", "agentIds": ["clone", "gamma"] }
            ],
            "watchlists": [
                { "id": "main", "name": "Main", "entries": [{ "type": "team", "teamId": "team-a" }] }
            ]
        });

        let changed =
            preserve_clone_team_placement_in_watchlist_state(&mut state, "source", "clone");

        assert!(changed);
        assert_eq!(
            state["teams"][0]["agentIds"],
            serde_json::json!(["source", "clone", "beta"])
        );
        assert_eq!(state["teams"][1]["agentIds"], serde_json::json!(["gamma"]));
        assert_eq!(
            state["watchlists"][0]["entries"][0],
            serde_json::json!({ "type": "team", "teamId": "team-a" })
        );
    }

    #[test]
    fn clone_team_placement_noops_when_source_is_not_in_team() {
        let mut state = serde_json::json!({
            "version": 2,
            "teams": [{ "id": "team-a", "name": "Wardian Dev", "agentIds": ["beta"] }],
            "watchlists": []
        });
        let original = state.clone();

        let changed =
            preserve_clone_team_placement_in_watchlist_state(&mut state, "source", "clone");

        assert!(!changed);
        assert_eq!(state, original);
    }

    #[test]
    fn clone_team_placement_updates_persisted_v2_state() {
        let temp = tempfile::tempdir().expect("temp dir");
        let watchlists_dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&watchlists_dir).expect("watchlists dir");
        std::fs::write(
            watchlists_dir.join("index.json"),
            serde_json::json!({
                "version": 2,
                "teams": [{ "id": "team-a", "name": "Wardian Dev", "agentIds": ["source", "beta"] }],
                "watchlists": []
            })
            .to_string(),
        )
        .expect("seed watchlist");

        let changed =
            preserve_clone_team_placement_in_home(temp.path(), "source", "clone").unwrap();

        assert!(changed);
        let saved = std::fs::read_to_string(watchlists_dir.join("index.json")).expect("saved");
        let state: serde_json::Value = serde_json::from_str(&saved).expect("json");
        assert_eq!(
            state["teams"][0]["agentIds"],
            serde_json::json!(["source", "clone", "beta"])
        );
    }

    #[test]
    fn deleted_agent_cleanup_prunes_persisted_watchlists_and_teams() {
        let mut state = serde_json::json!({
            "version": 2,
            "teams": [
                { "id": "team-a", "name": "Core", "agentIds": ["deleted", "kept"] },
                { "id": "team-empty", "name": "Empty", "agent_ids": ["deleted"] }
            ],
            "watchlists": [
                {
                    "id": "main",
                    "name": "Main",
                    "agent_ids": ["deleted", "kept"],
                    "entries": [
                        { "type": "agent", "agentId": "deleted" },
                        { "type": "agent", "agentId": "kept" },
                        { "type": "team", "teamId": "team-a" },
                        { "type": "team", "teamId": "team-empty" }
                    ]
                }
            ]
        });
        let known_ids = BTreeSet::from(["kept".to_string()]);

        let changed = retain_known_agent_references_in_watchlist_state(&mut state, &known_ids);

        assert!(changed);
        assert_eq!(
            state["teams"],
            serde_json::json!([{ "id": "team-a", "name": "Core", "agentIds": ["kept"] }])
        );
        assert_eq!(
            state["watchlists"][0]["agentIds"],
            serde_json::json!(["kept"])
        );
        assert!(state["watchlists"][0].get("agent_ids").is_none());
        assert_eq!(
            state["watchlists"][0]["entries"],
            serde_json::json!([
                { "type": "agent", "agentId": "kept" },
                { "type": "team", "teamId": "team-a" }
            ])
        );
    }
}

#[tauri::command]
pub async fn load_watchlist_prefs(_app: AppHandle) -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("watchlists/prefs.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
            return Ok(parsed);
        }
    }
    Ok(serde_json::Value::Null)
}

#[tauri::command]
pub async fn save_watchlist_prefs(prefs: serde_json::Value, _app: AppHandle) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("watchlists"));
    let path = home.join("watchlists/prefs.json");
    let json = serde_json::to_string_pretty(&prefs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn load_agent_interactions(_app: AppHandle) -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("watchlists/interactions.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or(serde_json::json!({}));
            return Ok(parsed);
        }
    }
    Ok(serde_json::json!({}))
}

#[tauri::command]
pub async fn save_agent_interactions(
    interactions: serde_json::Value,
    _app: AppHandle,
) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("watchlists"));
    let path = home.join("watchlists/interactions.json");
    let json = serde_json::to_string_pretty(&interactions).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn load_queue_items(_app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("queue/items.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or_else(|_| serde_json::json!([]));
            return Ok(parsed);
        }
    }
    Ok(serde_json::json!([]))
}

#[tauri::command]
pub async fn save_queue_items(
    items: serde_json::Value,
    _app: tauri::AppHandle,
) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("queue"));
    let path = home.join("queue/items.json");
    let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn load_queue_preferences(_app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("queue/preferences.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            let parsed: serde_json::Value =
                serde_json::from_str(&data).unwrap_or_else(|_| serde_json::json!({}));
            return Ok(parsed);
        }
    }
    Ok(serde_json::json!({}))
}

#[tauri::command]
pub async fn save_queue_preferences(
    preferences: serde_json::Value,
    _app: tauri::AppHandle,
) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("queue"));
    let path = home.join("queue/preferences.json");
    let json = serde_json::to_string_pretty(&preferences).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn load_opencode_last_assistant_text(
    session_id: String,
    _app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    crate::manager::opencode_last_assistant_text(&session_id)
}
