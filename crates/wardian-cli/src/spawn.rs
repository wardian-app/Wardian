//! CLI creation with explicit team membership and its truthful receipt.
use crate::args::{AgentArgs, AgentCommand};
use crate::errors::CliError;
use crate::output::render_show;
use crate::{control_error, live, render_options, watchlist};

pub(crate) fn handle_agent_spawn(
    provider: &str,
    class: &str,
    name: Option<&str>,
    workspace: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let team = match &args.command {
        Some(AgentCommand::Spawn { team, .. }) => team.as_deref(),
        _ => unreachable!("spawn handler only receives the spawn command"),
    };
    let (agent, membership) = watchlist::spawn_with_team(team, || {
        live::agent_spawn(provider, class, name, workspace, model, reasoning_effort)
            .map_err(control_error)
    })?;
    let rendered = render_show(&agent, &render_options(args))?;
    // Scalar projections deliberately retain their existing bare-value contract.
    if args.field.is_some() || team.is_none() {
        return Ok(rendered);
    }
    if args.pretty {
        let team = membership.expect("explicit spawn team was persisted");
        return Ok(format!("{rendered}Team: {} ({})\n", team.name, team.id));
    }
    let mut receipt: serde_json::Value =
        serde_json::from_str(&rendered).map_err(|error| CliError::generic(error.to_string()))?;
    receipt["team"] =
        serde_json::to_value(membership).map_err(|error| CliError::generic(error.to_string()))?;
    serde_json::to_string(&receipt)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}
