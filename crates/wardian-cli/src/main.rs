mod args;
mod errors;
mod live;
mod output;

use args::{AgentArgs, AgentCommand, Cli, Command};
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
    }
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
