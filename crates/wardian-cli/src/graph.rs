use crate::args::{GraphArgs, GraphCommand};
use crate::errors::{CliError, ExitCode};
use crate::live;
use wardian_core::identity::{self, AgentIdentity, ListFilters, Scope};

/// Full agent roster: live control endpoint when the app runs, DB fallback otherwise.
/// Mirrors the fallback pattern in `handle_list`.
#[allow(dead_code)]
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

#[allow(dead_code)]
fn wardian_home() -> Result<std::path::PathBuf, CliError> {
    wardian_core::paths::wardian_home()
        .ok_or_else(|| CliError::generic("Could not determine Wardian home"))
}

/// UUID match wins; otherwise a unique name match. Duplicated names are ambiguous.
#[allow(dead_code)]
fn resolve_endpoint<'a>(
    agents: &'a [AgentIdentity],
    target: &str,
) -> Result<&'a AgentIdentity, CliError> {
    if let Some(agent) = agents.iter().find(|agent| agent.uuid == target) {
        return Ok(agent);
    }
    let matches: Vec<&AgentIdentity> =
        agents.iter().filter(|agent| agent.name == target).collect();
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
#[allow(dead_code)]
#[derive(Debug)]
struct CallerContext {
    self_uuid: Option<String>,
}

#[allow(dead_code)]
fn caller_context(agents: &[AgentIdentity]) -> Result<CallerContext, CliError> {
    caller_context_from(std::env::var("WARDIAN_SESSION_ID").ok().as_deref(), agents)
}

/// Fail closed on a stale session: an unknown WARDIAN_SESSION_ID must not
/// silently acquire operator (unrestricted) powers.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        GraphCommand::Show
        | GraphCommand::Neighbors { .. }
        | GraphCommand::Activity
        | GraphCommand::Link { .. }
        | GraphCommand::Unlink { .. }
        | GraphCommand::Ignore { .. }
        | GraphCommand::Unignore { .. } => {
            Err(CliError::generic("graph command not implemented yet"))
        }
    }
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
