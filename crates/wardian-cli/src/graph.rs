use std::collections::{BTreeSet, HashMap};

use crate::args::{GraphArgs, GraphCommand};
use crate::errors::{CliError, ExitCode};
use crate::live;
use wardian_core::identity::{self, AgentIdentity, ListFilters, Scope};
use wardian_core::topology::{
    load_topology, pair_activity_from_records, resolve_neighbors, save_topology, AgentRef,
    PairActivity, Topology,
};

/// Full agent roster: live control endpoint when the app runs, DB fallback otherwise.
/// Mirrors the fallback pattern in `handle_list`.
fn agent_snapshot() -> Result<Vec<AgentIdentity>, CliError> {
    if let Ok(agents) = live::list_agents() {
        return Ok(agents);
    }
    let conn = crate::open_db()?;
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
    .map_err(crate::identity_error)
}

fn wardian_home() -> Result<std::path::PathBuf, CliError> {
    wardian_core::paths::wardian_home()
        .ok_or_else(|| CliError::generic("Could not determine Wardian home"))
}

/// UUID match wins; otherwise a unique name match. Duplicated names are ambiguous.
fn resolve_endpoint<'a>(
    agents: &'a [AgentIdentity],
    target: &str,
) -> Result<&'a AgentIdentity, CliError> {
    if let Some(agent) = agents.iter().find(|agent| agent.uuid == target) {
        return Ok(agent);
    }
    let matches: Vec<&AgentIdentity> = agents.iter().filter(|agent| agent.name == target).collect();
    match matches.as_slice() {
        [] => Err(CliError::not_found(target)),
        [single] => Ok(*single),
        _ => Err(CliError::backend(
            ExitCode::Ambiguous,
            "ambiguous_target",
            format!("Multiple agents are named {target}; pass a UUID instead"),
        )),
    }
}

/// Some(uuid) inside a session (mutations restricted to self); None = operator.
#[derive(Debug)]
struct CallerContext {
    self_uuid: Option<String>,
}

fn caller_context(agents: &[AgentIdentity]) -> Result<CallerContext, CliError> {
    caller_context_from(std::env::var("WARDIAN_SESSION_ID").ok().as_deref(), agents)
}

/// Fail closed on a stale session: an unknown WARDIAN_SESSION_ID must not
/// silently acquire operator (unrestricted) powers.
fn caller_context_from(
    session: Option<&str>,
    agents: &[AgentIdentity],
) -> Result<CallerContext, CliError> {
    match session {
        Some(session_id) => {
            if agents.iter().any(|agent| agent.uuid == session_id) {
                Ok(CallerContext {
                    self_uuid: Some(session_id.to_string()),
                })
            } else {
                Err(CliError::not_found(session_id))
            }
        }
        None => Ok(CallerContext { self_uuid: None }),
    }
}

/// Resolve a mutation's endpoints and enforce the self-serve rule.
fn resolve_pair(
    agents: &[AgentIdentity],
    ctx: &CallerContext,
    a: &str,
    b: Option<&str>,
) -> Result<(String, String), CliError> {
    let first = resolve_endpoint(agents, a)?.uuid.clone();
    let (x, y) = match (b, ctx.self_uuid.as_deref()) {
        (Some(b), _) => (first, resolve_endpoint(agents, b)?.uuid.clone()),
        (None, Some(self_uuid)) => (self_uuid.to_string(), first),
        (None, None) => {
            return Err(CliError::generic(
                "Two agents are required outside a session: wardian graph <verb> <a> <b>",
            ))
        }
    };
    if x == y {
        return Err(CliError::generic("Cannot connect an agent to itself"));
    }
    if let Some(self_uuid) = ctx.self_uuid.as_deref() {
        if x != self_uuid && y != self_uuid {
            return Err(CliError::backend(
                ExitCode::Generic,
                "self_serve_required",
                "Inside a session, graph edits must involve the calling agent",
            ));
        }
    }
    Ok((x, y))
}

pub fn handle_graph(args: GraphArgs) -> Result<String, CliError> {
    match args.command {
        GraphCommand::Show => render_graph_show(args.pretty),
        GraphCommand::Neighbors { agent } => render_graph_neighbors(agent.as_deref(), args.pretty),
        GraphCommand::Activity => render_graph_activity(args.pretty),
        GraphCommand::Link { a, b } => {
            handle_mutation(Mutation::Link, &a, b.as_deref(), args.pretty)
        }
        GraphCommand::Unlink { a, b } => {
            handle_mutation(Mutation::Unlink, &a, b.as_deref(), args.pretty)
        }
        GraphCommand::Ignore { a, b } => {
            handle_mutation(Mutation::Ignore, &a, b.as_deref(), args.pretty)
        }
        GraphCommand::Unignore { a, b } => {
            handle_mutation(Mutation::Unignore, &a, b.as_deref(), args.pretty)
        }
    }
}

enum Mutation {
    Link,
    Unlink,
    Ignore,
    Unignore,
}

impl Mutation {
    fn action(&self) -> &'static str {
        match self {
            Mutation::Link => "link",
            Mutation::Unlink => "unlink",
            Mutation::Ignore => "ignore",
            Mutation::Unignore => "unignore",
        }
    }
}

/// Idempotent read-modify-write on topology.json. Saves only when something
/// changed; the file write is atomic (temp + rename) inside save_topology.
fn handle_mutation(
    kind: Mutation,
    a: &str,
    b: Option<&str>,
    pretty: bool,
) -> Result<String, CliError> {
    let agents = agent_snapshot()?;
    let ctx = caller_context(&agents)?;
    let (x, y) = resolve_pair(&agents, &ctx, a, b)?;

    let home = wardian_home()?;
    let mut topology = load_topology(&home);
    let created_at = chrono::Utc::now().to_rfc3339();
    let changed = match kind {
        Mutation::Link => topology.add_edge(&x, &y, &created_at),
        Mutation::Unlink => topology.remove_edge(&x, &y),
        Mutation::Ignore => topology.ignore_pair(&x, &y),
        Mutation::Unignore => topology.unignore_pair(&x, &y),
    };
    if changed {
        save_topology(&home, &topology).map_err(|error| CliError::generic(error.to_string()))?;
    }

    // Report the canonical (sorted) pair, matching what topology.json stores.
    let (a_out, b_out) = wardian_core::topology::canonical_pair(&x, &y)
        .expect("resolve_pair rejects self/empty pairs");
    if pretty {
        let names = name_index(&agents);
        return Ok(format!(
            "{} {} <-> {}  (changed: {changed})\n",
            kind.action(),
            display_name(&names, &a_out),
            display_name(&names, &b_out),
        ));
    }
    render_json(serde_json::json!({
        "schema": 1,
        "action": kind.action(),
        "a": a_out,
        "b": b_out,
        "changed": changed,
    }))
}

fn render_graph_show(pretty: bool) -> Result<String, CliError> {
    let agents = agent_snapshot()?;
    let topology = load_topology(&wardian_home()?);
    // Activity is best-effort here: structure must render even if interactions
    // can't be read (e.g. table missing in an older DB).
    let activity = load_pair_activity().unwrap_or_default();
    let unmapped = unmapped_pairs(&topology, &activity, &agents);

    if pretty {
        return Ok(render_show_pretty(&agents, &topology, &unmapped));
    }
    let body = serde_json::json!({
        "schema": 1,
        "agents": agents.iter().map(|agent| serde_json::json!({
            "uuid": agent.uuid,
            "name": agent.name,
            "status": agent.status,
            "workspace": agent.workspace,
        })).collect::<Vec<_>>(),
        "edges": topology.edges,
        "unmapped_pairs": unmapped,
        "ignored_pairs": topology.ignored_pairs,
    });
    render_json(body)
}

fn render_graph_neighbors(target: Option<&str>, pretty: bool) -> Result<String, CliError> {
    let agents = agent_snapshot()?;
    let uuid = match target {
        Some(target) => resolve_endpoint(&agents, target)?.uuid.clone(),
        None => {
            let session_id =
                std::env::var("WARDIAN_SESSION_ID").map_err(|_| CliError::not_in_session())?;
            // Fail closed on a stale session id.
            resolve_endpoint(&agents, &session_id)?.uuid.clone()
        }
    };
    let topology = load_topology(&wardian_home()?);
    let refs: Vec<AgentRef> = agents
        .iter()
        .map(|agent| AgentRef {
            uuid: agent.uuid.clone(),
            workspace: agent.workspace.clone(),
        })
        .collect();
    let view = resolve_neighbors(&uuid, &topology, &refs);
    let names = name_index(&agents);

    if pretty {
        let mut out = format!("neighbors of {}:\n", display_name(&names, &view.agent_uuid));
        if view.members.is_empty() {
            out.push_str("  (none)\n");
        }
        for member in &view.members {
            out.push_str(&format!(
                "  {}  [{}]\n",
                display_name(&names, &member.uuid),
                member.reasons.join(", ")
            ));
        }
        return Ok(out);
    }
    let body = serde_json::json!({
        "schema": 1,
        "agent_uuid": view.agent_uuid,
        "members": view.members.iter().map(|member| serde_json::json!({
            "uuid": member.uuid,
            "name": names.get(member.uuid.as_str()),
            "reasons": member.reasons,
        })).collect::<Vec<_>>(),
    });
    render_json(body)
}

fn render_graph_activity(pretty: bool) -> Result<String, CliError> {
    let agents = agent_snapshot()?;
    let topology = load_topology(&wardian_home()?);
    let activity = load_pair_activity().map_err(CliError::generic)?;
    let unmapped: BTreeSet<(String, String)> = unmapped_key_set(&topology, &activity, &agents);
    let names = name_index(&agents);

    if pretty {
        let mut out = String::new();
        if activity.is_empty() {
            out.push_str("(no recorded communication)\n");
        }
        for pair in &activity {
            out.push_str(&format!(
                "{} <-> {}  last={}  ask={}{}\n",
                display_name(&names, &pair.a),
                display_name(&names, &pair.b),
                pair.last_message_at,
                pair.active_ask,
                if unmapped.contains(&(pair.a.clone(), pair.b.clone())) {
                    "  [unmapped]"
                } else {
                    ""
                },
            ));
        }
        return Ok(out);
    }
    let body = serde_json::json!({
        "schema": 1,
        "pairs": activity.iter().map(|pair| serde_json::json!({
            "a": pair.a,
            "b": pair.b,
            "last_message_at": pair.last_message_at,
            "active_ask": pair.active_ask,
            "awaiting_reply_from": pair.awaiting_reply_from,
            "unmapped": unmapped.contains(&(pair.a.clone(), pair.b.clone())),
        })).collect::<Vec<_>>(),
    });
    render_json(body)
}

/// Same aggregation the app's get_pair_activity command performs.
fn load_pair_activity() -> Result<Vec<PairActivity>, String> {
    let conn = crate::open_db().map_err(|error| error.message)?;
    let records =
        wardian_core::db::list_interaction_records_with_conn(&conn).map_err(|e| e.to_string())?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    Ok(pair_activity_from_records(&records, now_ms))
}

/// Activity pairs between known agents that have no manual edge and are not ignored -
/// the same derivation the GraphView performs for ghost edges.
fn unmapped_key_set(
    topology: &Topology,
    activity: &[PairActivity],
    agents: &[AgentIdentity],
) -> BTreeSet<(String, String)> {
    let known: BTreeSet<&str> = agents.iter().map(|agent| agent.uuid.as_str()).collect();
    let edges: BTreeSet<(&str, &str)> = topology
        .edges
        .iter()
        .map(|edge| (edge.a.as_str(), edge.b.as_str()))
        .collect();
    activity
        .iter()
        .filter(|pair| known.contains(pair.a.as_str()) && known.contains(pair.b.as_str()))
        .filter(|pair| !edges.contains(&(pair.a.as_str(), pair.b.as_str())))
        .filter(|pair| !topology.is_ignored(&pair.a, &pair.b))
        .map(|pair| (pair.a.clone(), pair.b.clone()))
        .collect()
}

fn unmapped_pairs(
    topology: &Topology,
    activity: &[PairActivity],
    agents: &[AgentIdentity],
) -> Vec<serde_json::Value> {
    let keys = unmapped_key_set(topology, activity, agents);
    activity
        .iter()
        .filter(|pair| keys.contains(&(pair.a.clone(), pair.b.clone())))
        .map(|pair| {
            serde_json::json!({
                "a": pair.a,
                "b": pair.b,
                "last_message_at": pair.last_message_at,
            })
        })
        .collect()
}

fn name_index(agents: &[AgentIdentity]) -> HashMap<&str, &str> {
    agents
        .iter()
        .map(|agent| (agent.uuid.as_str(), agent.name.as_str()))
        .collect()
}

fn display_name<'a>(names: &'a HashMap<&str, &str>, uuid: &'a str) -> &'a str {
    names.get(uuid).copied().unwrap_or(uuid)
}

fn render_show_pretty(
    agents: &[AgentIdentity],
    topology: &Topology,
    unmapped: &[serde_json::Value],
) -> String {
    let names = name_index(agents);
    let mut out = format!("agents: {}\nedges:\n", agents.len());
    if topology.edges.is_empty() {
        out.push_str("  (none)\n");
    }
    for edge in &topology.edges {
        out.push_str(&format!(
            "  {} <-> {}  ({})\n",
            display_name(&names, &edge.a),
            display_name(&names, &edge.b),
            edge.created_at
        ));
    }
    out.push_str("unmapped:\n");
    if unmapped.is_empty() {
        out.push_str("  (none)\n");
    }
    for pair in unmapped {
        out.push_str(&format!(
            "  {} <-> {}  (last {})\n",
            display_name(&names, pair["a"].as_str().unwrap_or("?")),
            display_name(&names, pair["b"].as_str().unwrap_or("?")),
            pair["last_message_at"].as_str().unwrap_or("?")
        ));
    }
    out.push_str("ignored:\n");
    if topology.ignored_pairs.is_empty() {
        out.push_str("  (none)\n");
    }
    for pair in &topology.ignored_pairs {
        out.push_str(&format!(
            "  {} <-> {}\n",
            display_name(&names, &pair.a),
            display_name(&names, &pair.b)
        ));
    }
    out
}

fn render_json(body: serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::identity::StatusSource;

    fn agent(uuid: &str, name: &str, workspace: Option<&str>) -> AgentIdentity {
        AgentIdentity {
            name: name.to_string(),
            uuid: uuid.to_string(),
            class: "Coder".to_string(),
            provider: "codex".to_string(),
            status: "Idle".to_string(),
            pid: None,
            started_at: None,
            workspace: workspace.map(str::to_string),
            last_status_at: None,
            status_source: StatusSource::Persisted,
            visibility: None,
        }
    }

    fn roster() -> Vec<AgentIdentity> {
        vec![
            agent("uuid-1", "coder-a1", Some("D:/ws")),
            agent("uuid-2", "architect-a1", Some("D:/ws")),
            agent("uuid-3", "coder-a1", Some("D:/other")), // duplicate name
        ]
    }

    #[test]
    fn resolve_endpoint_prefers_uuid_then_unique_name() {
        let agents = roster();
        assert_eq!(resolve_endpoint(&agents, "uuid-2").unwrap().uuid, "uuid-2");
        assert_eq!(
            resolve_endpoint(&agents, "architect-a1").unwrap().uuid,
            "uuid-2"
        );
    }

    #[test]
    fn resolve_endpoint_rejects_unknown_and_ambiguous() {
        let agents = roster();
        let err = resolve_endpoint(&agents, "ghost").unwrap_err();
        assert_eq!(err.code, "not_found");
        let err = resolve_endpoint(&agents, "coder-a1").unwrap_err();
        assert_eq!(err.code, "ambiguous_target");
    }

    #[test]
    fn caller_context_outside_session_is_unrestricted() {
        let ctx = caller_context_from(None, &roster()).unwrap();
        assert_eq!(ctx.self_uuid, None);
    }

    #[test]
    fn caller_context_stale_session_fails_closed() {
        let err = caller_context_from(Some("uuid-gone"), &roster()).unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[test]
    fn resolve_pair_defaults_to_self_in_session() {
        let agents = roster();
        let ctx = caller_context_from(Some("uuid-1"), &agents).unwrap();
        let (a, b) = resolve_pair(&agents, &ctx, "architect-a1", None).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("uuid-1", "uuid-2"));
    }

    #[test]
    fn resolve_pair_in_session_requires_self_endpoint() {
        let agents = roster();
        let ctx = caller_context_from(Some("uuid-1"), &agents).unwrap();
        let err = resolve_pair(&agents, &ctx, "uuid-2", Some("uuid-3")).unwrap_err();
        assert_eq!(err.code, "self_serve_required");
        // Explicit two-arg form including self is allowed.
        let (a, b) = resolve_pair(&agents, &ctx, "uuid-2", Some("uuid-1")).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("uuid-2", "uuid-1"));
    }

    #[test]
    fn resolve_pair_outside_session_requires_two_args_allows_any_pair() {
        let agents = roster();
        let ctx = caller_context_from(None, &agents).unwrap();
        assert!(resolve_pair(&agents, &ctx, "uuid-2", None).is_err());
        let (a, b) = resolve_pair(&agents, &ctx, "uuid-2", Some("uuid-3")).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("uuid-2", "uuid-3"));
    }

    #[test]
    fn resolve_pair_rejects_self_link() {
        let agents = roster();
        let ctx = caller_context_from(None, &agents).unwrap();
        let err = resolve_pair(&agents, &ctx, "uuid-2", Some("uuid-2")).unwrap_err();
        assert_eq!(err.code, "generic");
    }
}
