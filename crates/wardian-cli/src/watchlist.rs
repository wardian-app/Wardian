use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::args::{TeamArgs, TeamCommand, WatchlistArgs, WatchlistCommand};
use crate::disk::{TeamSummary, WatchlistEntry, WatchlistState, WatchlistSummary};
use crate::errors::{CliError, ExitCode};
use crate::{identity_error, live, open_db};
use wardian_core::identity::{self, AgentIdentity, ListFilters, Scope};

pub fn handle_team(args: TeamArgs) -> Result<String, CliError> {
    let mut state = load_state()?;
    match args.command {
        TeamCommand::List => render_json(serde_json::json!({
            "schema": 1,
            "teams": state.teams,
        })),
        TeamCommand::Show { target } => {
            let index = resolve_team_index(&state, &target)?;
            render_json(serde_json::json!({
                "schema": 1,
                "team": state.teams[index],
            }))
        }
        TeamCommand::Create { name, agents } => {
            ensure_unique_team_name(&state, &name, None)?;
            let agent_ids = resolve_agent_ids(&agents)?;
            ensure_non_empty_team(&agent_ids)?;
            remove_agents_from_other_teams(&mut state, &agent_ids, None);
            let team = TeamSummary {
                id: unique_id(
                    "team",
                    &name,
                    state.teams.iter().map(|team| team.id.as_str()),
                ),
                name,
                agent_ids,
            };
            state.teams.push(team.clone());
            prune_dangling_watchlist_entries(&mut state);
            persist_state(&state)?;
            render_mutation("team_create", serde_json::json!({ "team": team }))
        }
        TeamCommand::Rename { target, new_name } => {
            let index = resolve_team_index(&state, &target)?;
            let current_id = state.teams[index].id.clone();
            ensure_unique_team_name(&state, &new_name, Some(&current_id))?;
            state.teams[index].name = new_name;
            let team = state.teams[index].clone();
            persist_state(&state)?;
            render_mutation("team_rename", serde_json::json!({ "team": team }))
        }
        TeamCommand::Add { target, agents } => {
            let index = resolve_team_index(&state, &target)?;
            let team_id = state.teams[index].id.clone();
            let agent_ids = resolve_agent_ids(&agents)?;
            ensure_non_empty_team(&agent_ids)?;
            remove_agents_from_other_teams(&mut state, &agent_ids, Some(&team_id));
            let index = state
                .teams
                .iter()
                .position(|team| team.id == team_id)
                .ok_or_else(|| CliError::not_found_entity("Team", &target))?;
            for agent_id in agent_ids {
                if !state.teams[index].agent_ids.contains(&agent_id) {
                    state.teams[index].agent_ids.push(agent_id);
                }
            }
            let team = state.teams[index].clone();
            prune_dangling_watchlist_entries(&mut state);
            persist_state(&state)?;
            render_mutation("team_add", serde_json::json!({ "team": team }))
        }
        TeamCommand::Remove { target, agents } => {
            let index = resolve_team_index(&state, &target)?;
            let agent_ids = resolve_agent_ids(&agents)?;
            ensure_non_empty_team(&agent_ids)?;
            let team_id = state.teams[index].id.clone();
            let remaining = state.teams[index]
                .agent_ids
                .iter()
                .filter(|id| !agent_ids.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            ensure_non_empty_team(&remaining)?;
            state.teams[index].agent_ids = remaining;
            add_removed_members_after_team_entries(&mut state, &team_id, &agent_ids);
            let team = state.teams[index].clone();
            persist_state(&state)?;
            render_mutation("team_remove", serde_json::json!({ "team": team }))
        }
        TeamCommand::Split {
            target,
            name,
            agents,
        } => {
            let source_index = resolve_team_index(&state, &target)?;
            ensure_unique_team_name(&state, &name, None)?;
            let moved_ids = resolve_agent_ids(&agents)?;
            ensure_non_empty_team(&moved_ids)?;
            let source_id = state.teams[source_index].id.clone();
            for agent_id in &moved_ids {
                if !state.teams[source_index].agent_ids.contains(agent_id) {
                    return Err(CliError::backend(
                        ExitCode::NotFound,
                        "not_found",
                        format!("Agent {agent_id} is not a member of team {target}"),
                    ));
                }
            }
            let remaining = state.teams[source_index]
                .agent_ids
                .iter()
                .filter(|id| !moved_ids.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            ensure_non_empty_team(&remaining)?;
            state.teams[source_index].agent_ids = remaining;
            let new_team = TeamSummary {
                id: unique_id(
                    "team",
                    &name,
                    state.teams.iter().map(|team| team.id.as_str()),
                ),
                name,
                agent_ids: moved_ids,
            };
            insert_split_team_entries(&mut state, &source_id, &new_team.id);
            state.teams.push(new_team.clone());
            persist_state(&state)?;
            render_mutation("team_split", serde_json::json!({ "team": new_team }))
        }
        TeamCommand::Delete { target } => {
            let index = resolve_team_index(&state, &target)?;
            let team = state.teams.remove(index);
            for list in &mut state.watchlists {
                list.entries
                    .retain(|entry| entry.team_id() != Some(team.id.as_str()));
            }
            persist_state(&state)?;
            render_mutation("team_delete", serde_json::json!({ "team": team }))
        }
    }
}

pub fn handle_watchlist(args: WatchlistArgs) -> Result<String, CliError> {
    let mut state = load_state()?;
    match args.command {
        WatchlistCommand::List => render_json(serde_json::json!({
            "schema": 1,
            "watchlists": state.watchlists,
        })),
        WatchlistCommand::Show { target } => {
            let index = resolve_watchlist_index(&state, &target)?;
            render_json(serde_json::json!({
                "schema": 1,
                "watchlist": state.watchlists[index],
            }))
        }
        WatchlistCommand::Create { name } => {
            ensure_unique_watchlist_name(&state, &name, None)?;
            let watchlist = WatchlistSummary {
                id: unique_id(
                    "list",
                    &name,
                    state.watchlists.iter().map(|list| list.id.as_str()),
                ),
                name,
                entries: Vec::new(),
                agent_ids: Vec::new(),
            };
            state.watchlists.push(watchlist.clone());
            persist_state(&state)?;
            render_mutation(
                "watchlist_create",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
        WatchlistCommand::Rename { target, new_name } => {
            let index = resolve_watchlist_index(&state, &target)?;
            let current_id = state.watchlists[index].id.clone();
            ensure_unique_watchlist_name(&state, &new_name, Some(&current_id))?;
            state.watchlists[index].name = new_name;
            let watchlist = state.watchlists[index].clone();
            persist_state(&state)?;
            render_mutation(
                "watchlist_rename",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
        WatchlistCommand::AddTeam { target, team } => {
            let list_index = resolve_watchlist_index(&state, &target)?;
            let team_index = resolve_team_index(&state, &team)?;
            let team_id = state.teams[team_index].id.clone();
            if !state.watchlists[list_index]
                .entries
                .iter()
                .any(|entry| entry.team_id() == Some(team_id.as_str()))
            {
                state.watchlists[list_index]
                    .entries
                    .push(WatchlistEntry::Team { team_id });
            }
            refresh_watchlist_agent_ids(&mut state.watchlists[list_index]);
            let watchlist = state.watchlists[list_index].clone();
            persist_state(&state)?;
            render_mutation(
                "watchlist_add_team",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
        WatchlistCommand::RemoveTeam { target, team } => {
            let list_index = resolve_watchlist_index(&state, &target)?;
            let team_index = resolve_team_index(&state, &team)?;
            let team_id = state.teams[team_index].id.clone();
            state.watchlists[list_index]
                .entries
                .retain(|entry| entry.team_id() != Some(team_id.as_str()));
            refresh_watchlist_agent_ids(&mut state.watchlists[list_index]);
            let watchlist = state.watchlists[list_index].clone();
            persist_state(&state)?;
            render_mutation(
                "watchlist_remove_team",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
        WatchlistCommand::AddAgent { target, agent } => {
            let list_index = resolve_watchlist_index(&state, &target)?;
            let agent_id = resolve_agent_ids(&[agent])?.remove(0);
            if !state.watchlists[list_index]
                .entries
                .iter()
                .any(|entry| entry.agent_id() == Some(agent_id.as_str()))
            {
                state.watchlists[list_index]
                    .entries
                    .push(WatchlistEntry::Agent { agent_id });
            }
            refresh_watchlist_agent_ids(&mut state.watchlists[list_index]);
            let watchlist = state.watchlists[list_index].clone();
            persist_state(&state)?;
            render_mutation(
                "watchlist_add_agent",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
        WatchlistCommand::RemoveAgent { target, agent } => {
            let list_index = resolve_watchlist_index(&state, &target)?;
            let agent_id = resolve_agent_ids(&[agent])?.remove(0);
            state.watchlists[list_index]
                .entries
                .retain(|entry| entry.agent_id() != Some(agent_id.as_str()));
            refresh_watchlist_agent_ids(&mut state.watchlists[list_index]);
            let watchlist = state.watchlists[list_index].clone();
            persist_state(&state)?;
            render_mutation(
                "watchlist_remove_agent",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
        WatchlistCommand::Delete { target } => {
            let index = resolve_watchlist_index(&state, &target)?;
            let watchlist = state.watchlists.remove(index);
            persist_state(&state)?;
            render_mutation(
                "watchlist_delete",
                serde_json::json!({ "watchlist": watchlist }),
            )
        }
    }
}

fn load_state() -> Result<WatchlistState, CliError> {
    crate::disk::load_watchlist_state().map_err(|error| CliError::generic(error.to_string()))
}

fn persist_state(state: &WatchlistState) -> Result<(), CliError> {
    let home = wardian_home()?;
    let previous_teams = wardian_core::topology::load_team_memberships(&home);
    save_state(&home, state)?;
    seed_topology(&home, &previous_teams, state)?;
    let _ = live::notify_watchlists_changed();
    Ok(())
}

fn wardian_home() -> Result<PathBuf, CliError> {
    wardian_core::paths::wardian_home()
        .ok_or_else(|| CliError::generic("Could not determine Wardian home"))
}

fn save_state(home: &Path, state: &WatchlistState) -> Result<(), CliError> {
    let path = home.join("watchlists").join("index.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| CliError::generic(error.to_string()))?;
    }
    let json = serde_json::to_string_pretty(&canonical_state_value(state))
        .map_err(|error| CliError::generic(error.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|error| CliError::generic(error.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|error| CliError::generic(error.to_string()))?;
    Ok(())
}

fn seed_topology(
    home: &Path,
    previous_teams: &[wardian_core::topology::TeamMembership],
    state: &WatchlistState,
) -> Result<(), CliError> {
    let mut topology = wardian_core::topology::load_topology(home);
    let before = topology.edges.len();
    let before_suppressed = topology.suppressed_seed_pairs.len();
    let before_version = topology.version;
    if wardian_core::topology::needs_seed_suppression_migration(&topology) {
        wardian_core::topology::suppress_missing_team_seed_pairs(&mut topology, previous_teams);
        topology.version = wardian_core::topology::TOPOLOGY_SCHEMA_VERSION;
    }
    let now = chrono::Utc::now().to_rfc3339();
    for team in &state.teams {
        wardian_core::topology::seed_team_clique(&mut topology, &team.agent_ids, &now);
    }
    if topology.edges.len() != before
        || topology.suppressed_seed_pairs.len() != before_suppressed
        || topology.version != before_version
    {
        wardian_core::topology::save_topology(home, &topology)
            .map_err(|error| CliError::generic(error.to_string()))?;
    }
    Ok(())
}

fn canonical_state_value(state: &WatchlistState) -> serde_json::Value {
    serde_json::json!({
        "version": 2,
        "teams": state.teams.iter().map(|team| serde_json::json!({
            "id": team.id,
            "name": team.name,
            "agentIds": team.agent_ids,
        })).collect::<Vec<_>>(),
        "watchlists": state.watchlists.iter().map(|list| serde_json::json!({
            "id": list.id,
            "name": list.name,
            "agentIds": list.entries.iter().filter_map(|entry| entry.agent_id().map(str::to_string)).collect::<Vec<_>>(),
            "entries": list.entries.iter().map(canonical_entry_value).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn canonical_entry_value(entry: &WatchlistEntry) -> serde_json::Value {
    match entry {
        WatchlistEntry::Agent { agent_id } => {
            serde_json::json!({ "type": "agent", "agentId": agent_id })
        }
        WatchlistEntry::Team { team_id } => {
            serde_json::json!({ "type": "team", "teamId": team_id })
        }
    }
}

fn render_json(body: serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn render_mutation(action: &str, fields: serde_json::Value) -> Result<String, CliError> {
    let mut body = serde_json::Map::new();
    body.insert("schema".to_string(), serde_json::json!(1));
    body.insert("ok".to_string(), serde_json::json!(true));
    body.insert("action".to_string(), serde_json::json!(action));
    if let Some(extra) = fields.as_object() {
        for (key, value) in extra {
            body.insert(key.clone(), value.clone());
        }
    }
    render_json(serde_json::Value::Object(body))
}

fn resolve_agent_ids(targets: &[String]) -> Result<Vec<String>, CliError> {
    let agents = agent_snapshot()?;
    let mut ids = Vec::new();
    for target in targets {
        let agent = resolve_agent(&agents, target)?;
        if !ids.contains(&agent.uuid) {
            ids.push(agent.uuid.clone());
        }
    }
    Ok(ids)
}

fn agent_snapshot() -> Result<Vec<AgentIdentity>, CliError> {
    if let Ok(agents) = live::list_agents() {
        return Ok(agents);
    }
    let conn = open_db()?;
    identity::list_agents(
        &conn,
        &ListFilters {
            scope: Scope::All,
            caller_workspace: None,
            status: None,
            class: None,
            workspace: None,
        },
    )
    .map_err(identity_error)
}

fn resolve_agent<'a>(
    agents: &'a [AgentIdentity],
    target: &str,
) -> Result<&'a AgentIdentity, CliError> {
    if let Some(agent) = agents.iter().find(|agent| agent.uuid == target) {
        return Ok(agent);
    }
    let matches = agents
        .iter()
        .filter(|agent| agent.name == target)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(CliError::not_found(target)),
        [agent] => Ok(agent),
        _ => Err(CliError::backend(
            ExitCode::Ambiguous,
            "ambiguous_target",
            format!("Multiple agents are named {target}; pass a UUID instead"),
        )),
    }
}

fn resolve_team_index(state: &WatchlistState, target: &str) -> Result<usize, CliError> {
    if let Some(index) = state.teams.iter().position(|team| team.id == target) {
        return Ok(index);
    }
    let matches = state
        .teams
        .iter()
        .enumerate()
        .filter(|(_, team)| team.name == target)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(CliError::not_found_entity("Team", target)),
        [index] => Ok(*index),
        _ => Err(CliError::backend(
            ExitCode::Ambiguous,
            "ambiguous_target",
            format!("Multiple teams are named {target}; pass an id instead"),
        )),
    }
}

fn resolve_watchlist_index(state: &WatchlistState, target: &str) -> Result<usize, CliError> {
    if let Some(index) = state.watchlists.iter().position(|list| list.id == target) {
        return Ok(index);
    }
    let matches = state
        .watchlists
        .iter()
        .enumerate()
        .filter(|(_, list)| list.name == target)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(CliError::not_found_entity("Watchlist", target)),
        [index] => Ok(*index),
        _ => Err(CliError::backend(
            ExitCode::Ambiguous,
            "ambiguous_target",
            format!("Multiple watchlists are named {target}; pass an id instead"),
        )),
    }
}

fn ensure_unique_team_name(
    state: &WatchlistState,
    name: &str,
    excluding_id: Option<&str>,
) -> Result<(), CliError> {
    if state
        .teams
        .iter()
        .any(|team| team.name == name && excluding_id != Some(team.id.as_str()))
    {
        return Err(CliError::backend(
            ExitCode::Generic,
            "duplicate_name",
            format!("Team name already exists: {name}"),
        ));
    }
    Ok(())
}

fn ensure_unique_watchlist_name(
    state: &WatchlistState,
    name: &str,
    excluding_id: Option<&str>,
) -> Result<(), CliError> {
    if state
        .watchlists
        .iter()
        .any(|list| list.name == name && excluding_id != Some(list.id.as_str()))
    {
        return Err(CliError::backend(
            ExitCode::Generic,
            "duplicate_name",
            format!("Watchlist name already exists: {name}"),
        ));
    }
    Ok(())
}

fn ensure_non_empty_team(agent_ids: &[String]) -> Result<(), CliError> {
    if agent_ids.is_empty() {
        return Err(CliError::backend(
            ExitCode::Generic,
            "empty_team",
            "A team must contain at least one agent",
        ));
    }
    Ok(())
}

fn remove_agents_from_other_teams(
    state: &mut WatchlistState,
    agent_ids: &[String],
    keep_team_id: Option<&str>,
) {
    let moving = agent_ids.iter().collect::<BTreeSet<_>>();
    state.teams = state
        .teams
        .iter()
        .map(|team| {
            if keep_team_id == Some(team.id.as_str()) {
                return team.clone();
            }
            TeamSummary {
                id: team.id.clone(),
                name: team.name.clone(),
                agent_ids: team
                    .agent_ids
                    .iter()
                    .filter(|id| !moving.contains(id))
                    .cloned()
                    .collect(),
            }
        })
        .filter(|team| !team.agent_ids.is_empty())
        .collect();
}

fn add_removed_members_after_team_entries(
    state: &mut WatchlistState,
    team_id: &str,
    agent_ids: &[String],
) {
    for list in &mut state.watchlists {
        let mut next = Vec::new();
        for entry in &list.entries {
            next.push(entry.clone());
            if entry.team_id() == Some(team_id) {
                for agent_id in agent_ids {
                    next.push(WatchlistEntry::Agent {
                        agent_id: agent_id.clone(),
                    });
                }
            }
        }
        list.entries = dedupe_entries(next);
        refresh_watchlist_agent_ids(list);
    }
}

fn insert_split_team_entries(state: &mut WatchlistState, source_team_id: &str, new_team_id: &str) {
    for list in &mut state.watchlists {
        let mut next = Vec::new();
        for entry in &list.entries {
            next.push(entry.clone());
            if entry.team_id() == Some(source_team_id) {
                next.push(WatchlistEntry::Team {
                    team_id: new_team_id.to_string(),
                });
            }
        }
        list.entries = dedupe_entries(next);
        refresh_watchlist_agent_ids(list);
    }
}

fn prune_dangling_watchlist_entries(state: &mut WatchlistState) {
    let team_ids = state
        .teams
        .iter()
        .map(|team| team.id.as_str())
        .collect::<BTreeSet<_>>();
    for list in &mut state.watchlists {
        list.entries.retain(|entry| match entry {
            WatchlistEntry::Agent { .. } => true,
            WatchlistEntry::Team { team_id } => team_ids.contains(team_id.as_str()),
        });
        list.entries = dedupe_entries(list.entries.clone());
        refresh_watchlist_agent_ids(list);
    }
}

fn dedupe_entries(entries: Vec<WatchlistEntry>) -> Vec<WatchlistEntry> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for entry in entries {
        let key = match &entry {
            WatchlistEntry::Agent { agent_id } => format!("agent:{agent_id}"),
            WatchlistEntry::Team { team_id } => format!("team:{team_id}"),
        };
        if seen.insert(key) {
            deduped.push(entry);
        }
    }
    deduped
}

fn refresh_watchlist_agent_ids(list: &mut WatchlistSummary) {
    list.agent_ids = list
        .entries
        .iter()
        .filter_map(|entry| entry.agent_id().map(str::to_string))
        .collect();
}

fn unique_id<'a>(prefix: &str, name: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let existing = existing.collect::<BTreeSet<_>>();
    let slug = slugify(name);
    let base = format!("{prefix}-{slug}");
    if !existing.contains(base.as_str()) {
        return base;
    }
    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if !existing.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("unbounded id suffix search should find a free id")
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "item".to_string()
    } else {
        slug
    }
}
