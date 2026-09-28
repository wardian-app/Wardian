use fs2::FileExt;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

static WATCHLIST_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn watchlist_write_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    WATCHLIST_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Watchlist state lock is poisoned".to_string())
}

fn watchlist_process_lock(home: &Path) -> Result<std::fs::File, String> {
    let lock_path = home.join("watchlists").join("index.lock");
    std::fs::create_dir_all(lock_path.parent().expect("watchlist lock parent"))
        .map_err(|error| error.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| error.to_string())?;
    lock.lock_exclusive().map_err(|error| error.to_string())?;
    Ok(lock)
}

fn topology_process_lock(home: &Path) -> Result<std::fs::File, String> {
    let lock_path = home.join("topology.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| error.to_string())?;
    lock.lock_exclusive().map_err(|error| error.to_string())?;
    Ok(lock)
}

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
    let _write_lock = watchlist_write_lock()?;
    let app_dir = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(&app_dir);
    let _ = std::fs::create_dir_all(app_dir.join("watchlists"));
    let _process_lock = watchlist_process_lock(&app_dir)?;
    let path = app_dir.join("watchlists/index.json");
    let previous_bytes = std::fs::read(&path).ok();
    let migration_state = previous_bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .unwrap_or_else(|| watchlists.clone());
    wardian_core::conversations::write_json_atomic(&path, &watchlists)
        .map_err(|error| error.to_string())?;

    // Seed team cliques into topology when teams are created or members are
    // added. Save and notify only when seeding actually added edges — plain
    // watchlist saves (reorders, renames) must not churn topology.json or
    // trigger graph refreshes.
    match seed_team_topology_from_watchlist_state_with_migration(
        &app_dir,
        &watchlists,
        &migration_state,
    ) {
        Ok(true) => {
            let _ = app.emit("topology-changed", ());
        }
        Ok(false) => {}
        Err(topology_error) => {
            let restore_result = match previous_bytes {
                Some(previous_bytes) => {
                    if let Ok(previous_state) =
                        serde_json::from_slice::<serde_json::Value>(&previous_bytes)
                    {
                        wardian_core::conversations::write_json_atomic(&path, &previous_state)
                    } else {
                        std::fs::write(&path, previous_bytes)
                    }
                }
                None => std::fs::remove_file(&path),
            };
            if let Err(restore_error) = restore_result {
                return Err(format!(
                    "topology seeding failed: {topology_error}; restoring watchlist failed: {restore_error}"
                ));
            }
            return Err(topology_error);
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

fn team_memberships(state: &serde_json::Value) -> Vec<wardian_core::topology::TeamMembership> {
    state
        .get("teams")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|team| {
            let id = team.get("id").and_then(|value| value.as_str())?;
            let agent_ids = team_agent_ids(team);
            (!id.is_empty() && !agent_ids.is_empty()).then_some(
                wardian_core::topology::TeamMembership {
                    id: id.to_string(),
                    agent_ids,
                },
            )
        })
        .collect()
}

fn seed_team_topology_from_watchlist_state_with_migration(
    home: &Path,
    state: &serde_json::Value,
    migration_state: &serde_json::Value,
) -> Result<bool, String> {
    let _topology_lock = topology_process_lock(home)?;
    let mut topology = wardian_core::topology::load_topology(home);
    let topology_needs_migration =
        wardian_core::topology::needs_seed_suppression_migration(&topology);
    let mut topology_changed = false;
    if topology_needs_migration {
        let migration_teams = team_memberships(migration_state);
        wardian_core::topology::suppress_missing_team_seed_pairs(&mut topology, &migration_teams);
        topology.version = wardian_core::topology::TOPOLOGY_SCHEMA_VERSION;
        topology_changed = true;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut edges_added = 0;
    let Some(teams) = state.get("teams").and_then(|value| value.as_array()) else {
        return Ok(topology_changed);
    };
    for team in teams {
        let agent_ids = team_agent_ids(team);
        if !agent_ids.is_empty() {
            edges_added +=
                wardian_core::topology::seed_team_clique(&mut topology, &agent_ids, &now);
        }
    }
    if edges_added == 0 && !topology_changed {
        return Ok(false);
    }

    wardian_core::topology::save_topology(home, &topology).map_err(|error| error.to_string())?;
    Ok(true)
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
    let source_team_indices = teams
        .iter()
        .enumerate()
        .filter_map(|(index, team)| {
            team_agent_ids(team)
                .iter()
                .any(|id| id == source_agent_id)
                .then_some(index)
        })
        .collect::<BTreeSet<_>>();
    if source_team_indices.is_empty() {
        return false;
    }

    let before = serde_json::Value::Array(teams.clone());
    for (index, team) in teams.iter_mut().enumerate() {
        let mut agent_ids = team_agent_ids(team)
            .into_iter()
            .filter(|id| id != clone_agent_id)
            .collect::<Vec<_>>();
        if source_team_indices.contains(&index) {
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
        let _ = app.emit("topology-changed", ());
    }
    Ok(changed)
}

pub(crate) fn preserve_clone_team_placement_in_home(
    home: &Path,
    source_agent_id: &str,
    clone_agent_id: &str,
) -> Result<bool, String> {
    let _write_lock = watchlist_write_lock()?;
    let path = watchlist_index_path(home);
    let _process_lock = watchlist_process_lock(home)?;
    if !path.exists() {
        return Ok(false);
    }

    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut state = serde_json::from_str::<serde_json::Value>(&data).map_err(|e| e.to_string())?;
    let migration_state = state.clone();
    if !preserve_clone_team_placement_in_watchlist_state(
        &mut state,
        source_agent_id,
        clone_agent_id,
    ) {
        return Ok(false);
    }

    wardian_core::conversations::write_json_atomic(&path, &state)
        .map_err(|error| error.to_string())?;
    if let Err(topology_error) =
        seed_team_topology_from_watchlist_state_with_migration(home, &state, &migration_state)
    {
        // save_topology is atomic, so a failed topology write leaves the
        // previous topology intact. Restore the watchlist as well instead of
        // leaving durable membership ahead of the graph relation state.
        if let Err(restore_error) =
            wardian_core::conversations::write_json_atomic(&path, &migration_state)
        {
            return Err(format!(
                "topology seeding failed: {topology_error}; restoring watchlist failed: {restore_error}"
            ));
        }
        return Err(topology_error);
    }
    Ok(true)
}

pub(crate) fn retain_known_agent_references_in_home(
    home: &Path,
    known_agent_ids: &BTreeSet<String>,
) -> Result<bool, String> {
    let _write_lock = watchlist_write_lock()?;
    let path = watchlist_index_path(home);
    let _process_lock = watchlist_process_lock(home)?;
    if !path.exists() {
        return Ok(false);
    }

    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut state = serde_json::from_str::<serde_json::Value>(&data).map_err(|e| e.to_string())?;
    if !retain_known_agent_references_in_watchlist_state(&mut state, known_agent_ids) {
        return Ok(false);
    }

    wardian_core::conversations::write_json_atomic(&path, &state)
        .map_err(|error| error.to_string())?;
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
    fn clone_team_placement_preserves_every_source_team_and_seeds_each_clique() {
        let temp = tempfile::tempdir().expect("temp dir");
        let watchlists_dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&watchlists_dir).expect("watchlists dir");
        std::fs::write(
            watchlists_dir.join("index.json"),
            serde_json::json!({
                "version": 2,
                "teams": [
                    { "id": "team-a", "name": "Core", "agentIds": ["source", "alpha"] },
                    { "id": "team-b", "name": "Review", "agentIds": ["beta", "source"] },
                    { "id": "team-c", "name": "Old clone", "agentIds": ["clone"] }
                ],
                "watchlists": []
            })
            .to_string(),
        )
        .expect("seed watchlist");

        let changed =
            preserve_clone_team_placement_in_home(temp.path(), "source", "clone").unwrap();

        assert!(changed);
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(watchlists_dir.join("index.json")).expect("saved watchlist"),
        )
        .expect("valid watchlist");
        assert_eq!(
            state["teams"],
            serde_json::json!([
                { "id": "team-a", "name": "Core", "agentIds": ["source", "clone", "alpha"] },
                { "id": "team-b", "name": "Review", "agentIds": ["beta", "source", "clone"] }
            ])
        );

        let topology = wardian_core::topology::load_topology(temp.path());
        assert!(topology
            .edges
            .iter()
            .any(|edge| edge.a == "alpha" && edge.b == "clone"));
        assert!(topology
            .edges
            .iter()
            .any(|edge| edge.a == "beta" && edge.b == "clone"));
        assert!(topology
            .edges
            .iter()
            .any(|edge| edge.a == "clone" && edge.b == "source"));
    }

    #[test]
    fn clone_team_placement_rolls_back_membership_when_topology_save_fails() {
        let temp = tempfile::tempdir().expect("temp dir");
        let watchlists_dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&watchlists_dir).expect("watchlists dir");
        let original = serde_json::json!({
            "version": 2,
            "teams": [{ "id": "team-a", "name": "Wardian Dev", "agentIds": ["source", "beta"] }],
            "watchlists": []
        });
        std::fs::write(
            watchlists_dir.join("index.json"),
            serde_json::to_string_pretty(&original).expect("serialize watchlist"),
        )
        .expect("seed watchlist");
        std::fs::create_dir(temp.path().join("topology.json")).expect("block topology save");

        let error = preserve_clone_team_placement_in_home(temp.path(), "source", "clone")
            .expect_err("topology failure should be returned");
        assert!(!error.is_empty());
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(watchlists_dir.join("index.json")).expect("saved watchlist"),
        )
        .expect("valid watchlist");
        assert_eq!(saved, original);
    }

    #[test]
    fn clone_team_placement_migrates_old_suppression_before_seeding_clone_edges() {
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
        std::fs::write(
            temp.path().join("topology.json"),
            serde_json::json!({
                "version": 2,
                "edges": [],
                "ignored_pairs": [],
                "suppressed_seed_pairs": []
            })
            .to_string(),
        )
        .expect("seed topology");

        preserve_clone_team_placement_in_home(temp.path(), "source", "clone").expect("place clone");

        let topology = wardian_core::topology::load_topology(temp.path());
        assert_eq!(
            topology.version,
            wardian_core::topology::TOPOLOGY_SCHEMA_VERSION
        );
        assert!(topology.is_seed_suppressed("source", "beta"));
        assert!(!topology.is_seed_suppressed("source", "clone"));
        assert!(topology
            .edges
            .iter()
            .any(|edge| edge.a == "beta" && edge.b == "clone"));
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
pub async fn load_queue_items(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let _queue_guard = state.queue_io_lock.lock().await;
    let items = crate::utils::queue::load_items()
        .into_iter()
        .filter(|item| {
            item["type"] != "agent_completed" || item["dismissed"].as_bool() != Some(true)
        })
        .collect::<Vec<_>>();
    *state.queue_loaded_snapshot.lock().await = Some(items.clone());
    Ok(serde_json::json!(items))
}

#[tauri::command]
pub async fn dismiss_agent_completions(
    ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _queue_guard = state.queue_io_lock.lock().await;
    let mut items = crate::utils::queue::load_items();
    let mut changed = false;
    for item in &mut items {
        if ids.iter().any(|id| item["id"] == *id)
            && item["type"] == "agent_completed"
            && item["dismissed"].as_bool() != Some(true)
        {
            item["dismissed"] = serde_json::Value::Bool(true);
            item["read"] = serde_json::Value::Bool(true);
            changed = true;
        }
    }
    if changed {
        crate::utils::queue::save_items(&items)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn save_queue_items(
    items: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let persisted = items
        .as_array()
        .ok_or_else(|| "queue items must be an array".to_string())?;
    let _queue_guard = state.queue_io_lock.lock().await;
    let latest = crate::utils::queue::load_items();
    let base = state.queue_loaded_snapshot.lock().await.clone();
    let merged = crate::utils::queue::merge_desktop_snapshot(base.as_deref(), persisted, &latest);
    crate::utils::queue::save_items(&merged)?;
    *state.queue_loaded_snapshot.lock().await = Some(merged);
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
