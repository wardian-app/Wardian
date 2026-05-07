mod args;
mod disk;
mod errors;
mod live;
mod output;

use std::{io::Read as _, time::Duration};

use args::{AgentArgs, AgentCommand, Cli, Command, SendArgs, WorkflowArgs, WorkflowCommand};
use clap::Parser;
use errors::{CliError, ExitCode};
use output::{render_list, render_show, RenderOptions};
use wardian_core::identity::{self, ListFilters, Scope};

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error),
    };
    let result = match cli.command {
        Command::Agent(args) => handle_agent(args),
        Command::Workflow(args) => handle_workflow(args),
        Command::Send(args) => handle_send(args),
    };

    match result {
        Ok(stdout) => {
            print!("{stdout}");
            ExitCode::Success as i32
        }
        Err(error) => {
            error.emit();
            error.code_i32()
        }
    }
}

fn handle_parse_error(error: clap::Error) -> i32 {
    if matches!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    ) {
        print!("{error}");
        return ExitCode::Success as i32;
    }

    CliError::generic(error.to_string()).emit();
    ExitCode::Generic as i32
}

// ---------------------------------------------------------------------------
// wardian agent
// ---------------------------------------------------------------------------

fn handle_agent(args: AgentArgs) -> Result<String, CliError> {
    match &args.command {
        Some(AgentCommand::Show { target }) => handle_show(target.as_deref(), &args),
        Some(AgentCommand::List {
            scope,
            status,
            class_name,
            workspace,
        }) => handle_list(
            scope,
            status.clone(),
            class_name.clone(),
            workspace.clone(),
            &args,
        ),
        None => handle_show(args.target.as_deref(), &args),
        Some(AgentCommand::Kill { target }) => handle_agent_kill(target),
        Some(AgentCommand::Pause { target }) => handle_agent_pause(target),
        Some(AgentCommand::Resume { target }) => handle_agent_resume(target),
        Some(AgentCommand::Spawn {
            provider,
            class,
            name,
            workspace,
        }) => handle_agent_spawn(
            provider,
            class,
            name.as_deref(),
            workspace.as_deref(),
            &args,
        ),
        Some(AgentCommand::Clone { target, name }) => {
            handle_agent_clone(target, name.as_deref(), &args)
        }
        Some(AgentCommand::Wait {
            target,
            until,
            timeout,
        }) => handle_agent_wait(target, until, timeout, &args),
    }
}

fn handle_agent_kill(target: &str) -> Result<String, CliError> {
    live::agent_kill(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({"schema":1,"ok":true,"target":target}))
            .unwrap()
    ))
}

fn handle_agent_pause(target: &str) -> Result<String, CliError> {
    live::agent_pause(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({"schema":1,"ok":true,"target":target}))
            .unwrap()
    ))
}

fn handle_agent_resume(target: &str) -> Result<String, CliError> {
    live::agent_resume(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({"schema":1,"ok":true,"target":target}))
            .unwrap()
    ))
}

fn handle_agent_spawn(
    provider: &str,
    class: &str,
    name: Option<&str>,
    workspace: Option<&str>,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let agent = live::agent_spawn(provider, class, name, workspace).map_err(control_error)?;
    render_show(&agent, &render_options(args))
}

fn handle_agent_clone(
    target: &str,
    name: Option<&str>,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let agent = live::agent_clone(target, name).map_err(control_error)?;
    render_show(&agent, &render_options(args))
}

fn handle_agent_wait(
    target: &str,
    until: &str,
    timeout: &str,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let timeout = parse_timeout(timeout)?;
    let agent = live::wait_agent_until(target, until, timeout).map_err(control_error)?;
    render_show(&agent, &render_options(args))
}

// ---------------------------------------------------------------------------
// wardian workflow
// ---------------------------------------------------------------------------

fn handle_workflow(args: WorkflowArgs) -> Result<String, CliError> {
    match args.command {
        WorkflowCommand::List => {
            let workflows = match live::workflow_list() {
                Ok(workflows) => workflows,
                Err(error) if is_control_endpoint_unavailable(&error) => {
                    let workflows = disk::list_workflows_from_disk()
                        .map_err(|e| CliError::generic(e.to_string()))?;
                    disk::workflow_summaries(&workflows)
                }
                Err(error) => return Err(control_error(error)),
            };
            output::render_workflow_list(&workflows, args.pretty)
        }
        WorkflowCommand::Show { target } => {
            let wf = match live::workflow_show(&target) {
                Ok(workflow) => workflow,
                Err(error) if is_control_endpoint_unavailable(&error) => {
                    let workflows = disk::list_workflows_from_disk()
                        .map_err(|e| CliError::generic(e.to_string()))?;
                    workflows
                        .into_iter()
                        .find(|w| w.id == target || w.name == target)
                        .ok_or_else(|| CliError::not_found(&target))?
                }
                Err(error) => return Err(control_error(error)),
            };
            output::render_workflow_show(&wf, args.pretty)
        }
        WorkflowCommand::Run { target } => {
            live::workflow_run(&target).map_err(control_error)?;
            Ok(format!(
                "{}\n",
                serde_json::to_string_pretty(
                    &serde_json::json!({"schema":1,"ok":true,"workflow":target})
                )
                .unwrap()
            ))
        }
        WorkflowCommand::Stop { run_instance_id } => {
            live::workflow_stop(&run_instance_id).map_err(control_error)?;
            Ok(format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({"schema":1,"ok":true})).unwrap()
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// wardian send
// ---------------------------------------------------------------------------

fn handle_send(args: SendArgs) -> Result<String, CliError> {
    let message = if args.stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::generic(e.to_string()))?;
        buf
    } else if let Some(path) = &args.file {
        std::fs::read_to_string(path).map_err(|e| CliError::generic(e.to_string()))?
    } else {
        args.message
            .clone()
            .ok_or_else(|| CliError::generic("Provide a message, --stdin, or --file".to_string()))?
    };

    let waited_agent = if let Some(until) = args.wait_until.as_deref() {
        let timeout = parse_timeout(&args.timeout)?;
        Some(
            live::send_message_and_wait(&args.to, &message, args.thread.as_deref(), until, timeout)
                .map_err(control_error)?,
        )
    } else {
        live::send_message(&args.to, &message, args.thread.as_deref()).map_err(control_error)?;
        None
    };

    let mut response = serde_json::json!({"schema":1,"ok":true,"target":args.to});
    if let Some(agent) = waited_agent {
        response["status"] = serde_json::Value::String(agent.status);
    }

    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&response).unwrap()
    ))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn control_error(e: std::io::Error) -> CliError {
    if let Some(wait_timeout) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<live::WaitTimeoutError>())
    {
        CliError::backend(ExitCode::Generic, "wait_timeout", wait_timeout.to_string())
    } else if let Some(wait_not_found) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<live::WaitTargetNotFoundError>())
    {
        CliError::backend(ExitCode::NotFound, "not_found", wait_not_found.to_string())
    } else if is_control_endpoint_unavailable(&e) {
        CliError::app_not_running()
    } else if let Some(endpoint_error) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<live::ControlEndpointError>())
    {
        match endpoint_error.code() {
            "not_supported" => CliError::backend(
                ExitCode::Generic,
                "not_supported",
                endpoint_error.to_string(),
            ),
            "not_found" => {
                CliError::backend(ExitCode::NotFound, "not_found", endpoint_error.to_string())
            }
            _ => CliError::generic(endpoint_error.to_string()),
        }
    } else {
        CliError::generic(e.to_string())
    }
}

fn parse_timeout(value: &str) -> Result<Duration, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::generic("timeout must not be empty"));
    }

    let (number, multiplier) = if let Some(number) = trimmed.strip_suffix("ms") {
        (number, Duration::from_millis(1))
    } else if let Some(number) = trimmed.strip_suffix('s') {
        (number, Duration::from_secs(1))
    } else if let Some(number) = trimmed.strip_suffix('m') {
        (number, Duration::from_secs(60))
    } else {
        (trimmed, Duration::from_secs(1))
    };

    let count = number
        .trim()
        .parse::<u64>()
        .map_err(|_| CliError::generic(format!("invalid timeout: {value}")))?;
    let count = u32::try_from(count)
        .map_err(|_| CliError::generic(format!("timeout is too large: {value}")))?;
    multiplier
        .checked_mul(count)
        .ok_or_else(|| CliError::generic(format!("timeout is too large: {value}")))
}

fn is_control_endpoint_unavailable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::TimedOut
    )
}

fn handle_show(target: Option<&str>, args: &AgentArgs) -> Result<String, CliError> {
    if let Ok(agents) = live::list_agents() {
        let agent = match target {
            Some(target) => agents
                .into_iter()
                .find(|agent| agent.uuid == target || agent.name == target)
                .ok_or_else(|| CliError::not_found(target))?,
            None => resolve_live_self_for_show(&agents)?.clone(),
        };
        return render_show(&agent, &render_options(args));
    }

    let conn = open_db()?;
    let agent = match target {
        Some(target) => identity::resolve_by_name_or_uuid(&conn, target).map_err(identity_error)?,
        None => identity::resolve_self(&conn).map_err(identity_error)?,
    };
    render_show(&agent, &render_options(args))
}

fn handle_list(
    scope: &str,
    status: Option<String>,
    class_name: Option<String>,
    workspace: Option<String>,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let requested_scope = if workspace.is_some() {
        Scope::All
    } else {
        match scope {
            "workspace" => Scope::Workspace,
            "all" => Scope::All,
            other => return Err(CliError::generic(format!("unknown scope: {other}"))),
        }
    };

    if let Ok(live_agents) = live::list_agents() {
        let caller_workspace = if requested_scope == Scope::Workspace {
            resolve_live_self(&live_agents)
                .and_then(|agent| agent.workspace.clone())
                .filter(|workspace| !workspace.is_empty())
        } else {
            None
        };
        let effective_scope = if requested_scope == Scope::Workspace && caller_workspace.is_none() {
            Scope::All
        } else {
            requested_scope
        };
        let agents = identity::filter_agents(
            live_agents,
            &ListFilters {
                scope: effective_scope,
                caller_workspace,
                status,
                class: class_name,
                workspace,
            },
        );
        return render_list(&agents, &render_options(args));
    }

    let conn = open_db()?;
    let caller_workspace = caller_workspace_from_db(&conn, requested_scope);
    let effective_scope = if requested_scope == Scope::Workspace && caller_workspace.is_none() {
        Scope::All
    } else {
        requested_scope
    };
    let agents = identity::list_agents(
        &conn,
        &ListFilters {
            scope: effective_scope,
            caller_workspace,
            status,
            class: class_name,
            workspace,
        },
    )
    .map_err(identity_error)?;
    render_list(&agents, &render_options(args))
}

fn resolve_live_self(
    agents: &[wardian_core::identity::AgentIdentity],
) -> Option<&wardian_core::identity::AgentIdentity> {
    let session_id = std::env::var("WARDIAN_SESSION_ID").ok()?;
    agents.iter().find(|agent| agent.uuid == session_id)
}

fn resolve_live_self_for_show(
    agents: &[wardian_core::identity::AgentIdentity],
) -> Result<&wardian_core::identity::AgentIdentity, CliError> {
    let session_id = std::env::var("WARDIAN_SESSION_ID").map_err(|_| CliError::not_in_session())?;
    agents
        .iter()
        .find(|agent| agent.uuid == session_id)
        .ok_or_else(|| CliError::not_found(&session_id))
}

fn caller_workspace_from_db(conn: &rusqlite::Connection, scope: Scope) -> Option<String> {
    if scope != Scope::Workspace {
        return None;
    }
    identity::resolve_self(conn)
        .ok()
        .and_then(|agent| agent.workspace)
        .filter(|workspace| !workspace.is_empty())
}

fn render_options(args: &AgentArgs) -> RenderOptions {
    RenderOptions {
        fields: args.fields.as_deref().map(|fields| {
            fields
                .split(',')
                .map(str::trim)
                .filter(|field| !field.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        }),
        field: args.field.clone(),
        verbose: args.verbose,
        pretty: args.pretty,
    }
}

fn open_db() -> Result<rusqlite::Connection, CliError> {
    let path = wardian_core::paths::state_db_path()
        .ok_or_else(|| CliError::db_unavailable("Could not resolve Wardian state.db path"))?;
    if !path.exists() {
        return Err(CliError::db_unavailable(format!(
            "state.db was not found at {}",
            path.display()
        )));
    }
    let conn = rusqlite::Connection::open(path)
        .map_err(|error| CliError::db_unavailable(error.to_string()))?;
    wardian_core::db::run_migrations(&conn)
        .map_err(|error| CliError::db_unavailable(error.to_string()))?;
    Ok(conn)
}

fn identity_error(error: identity::IdentityError) -> CliError {
    match error {
        identity::IdentityError::NotInSession => CliError::not_in_session(),
        identity::IdentityError::NotFound(requested) => CliError::not_found(&requested),
        identity::IdentityError::Db(error) => CliError::db_unavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_error_preserves_backend_not_supported_code() {
        let error = std::io::Error::other(live::ControlEndpointError::new(
            "not_supported",
            "--thread is not supported",
        ));

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "not_supported");
        assert_eq!(cli_error.code_i32(), 1);
        assert!(cli_error.message.contains("--thread is not supported"));
    }

    #[test]
    fn control_error_preserves_backend_not_found_exit_code() {
        let error = std::io::Error::other(live::ControlEndpointError::new(
            "not_found",
            "agent not found: ghost",
        ));

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "not_found");
        assert_eq!(cli_error.code_i32(), 2);
        assert!(cli_error.message.contains("ghost"));
    }

    #[test]
    fn control_error_does_not_treat_wait_timeout_as_app_not_running() {
        let error = std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            live::WaitTimeoutError::new("reviewer-a1", "idle", "processing"),
        );

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "wait_timeout");
        assert_eq!(cli_error.code_i32(), 1);
        assert!(cli_error.message.contains("reviewer-a1"));
    }

    #[test]
    fn control_error_does_not_treat_wait_target_miss_as_app_not_running() {
        let error = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            live::WaitTargetNotFoundError::new("ghost"),
        );

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "not_found");
        assert_eq!(cli_error.code_i32(), 2);
        assert!(cli_error.message.contains("ghost"));
    }

    #[test]
    fn parses_timeout_units() {
        assert_eq!(
            parse_timeout("250ms").unwrap(),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            parse_timeout("30s").unwrap(),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            parse_timeout("10m").unwrap(),
            std::time::Duration::from_secs(600)
        );
    }
}
