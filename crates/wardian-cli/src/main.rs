mod args;
mod disk;
mod errors;
mod live;
mod output;

use std::{io::Read as _, time::Duration};

use args::{
    AgentArgs, AgentCommand, AgentWorktreeCommand, AskArgs, Cli, Command, ReplyArgs,
    ReplyStatusArg, SendArgs, TeamArgs, TeamCommand, WatchlistArgs, WatchlistCommand, WorkflowArgs,
    WorkflowCommand,
};
use clap::Parser;
use errors::{CliError, ExitCode};
use output::{render_list, render_show, RenderOptions};
use wardian_core::control::MessageInputMode;
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
        Command::Team(args) => handle_team(args),
        Command::Watchlist(args) => handle_watchlist(args),
        Command::Send(args) => handle_send(args),
        Command::Ask(args) => handle_ask(args),
        Command::Reply(args) => handle_reply(args),
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
// wardian team / watchlist
// ---------------------------------------------------------------------------

fn handle_team(args: TeamArgs) -> Result<String, CliError> {
    let state = disk::load_watchlist_state().map_err(|e| CliError::generic(e.to_string()))?;
    match args.command {
        TeamCommand::List => serde_json::to_string_pretty(&serde_json::json!({
            "schema": 1,
            "teams": state.teams,
        }))
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string())),
        TeamCommand::Show { target } => {
            let team = state
                .teams
                .into_iter()
                .find(|team| team.id == target || team.name == target)
                .ok_or_else(|| CliError::not_found_entity("Team", &target))?;
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": 1,
                "team": team,
            }))
            .map(|json| format!("{json}\n"))
            .map_err(|e| CliError::generic(e.to_string()))
        }
    }
}

fn handle_watchlist(args: WatchlistArgs) -> Result<String, CliError> {
    let state = disk::load_watchlist_state().map_err(|e| CliError::generic(e.to_string()))?;
    match args.command {
        WatchlistCommand::List => serde_json::to_string_pretty(&serde_json::json!({
            "schema": 1,
            "watchlists": state.watchlists,
        }))
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string())),
        WatchlistCommand::Show { target } => {
            let watchlist = state
                .watchlists
                .into_iter()
                .find(|watchlist| watchlist.id == target || watchlist.name == target)
                .ok_or_else(|| CliError::not_found_entity("Watchlist", &target))?;
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": 1,
                "watchlist": watchlist,
            }))
            .map(|json| format!("{json}\n"))
            .map_err(|e| CliError::generic(e.to_string()))
        }
    }
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
        Some(AgentCommand::Worktree { command }) => handle_agent_worktree(command),
        Some(AgentCommand::Watch { follow, .. }) if *follow => Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            "agent watch --follow is reserved for a future streaming implementation",
        )),
        Some(AgentCommand::Watch {
            target,
            since,
            until,
            include,
            tail,
            timeout,
            follow,
        }) => handle_agent_watch(
            target,
            since.as_deref(),
            until.as_deref(),
            include.as_deref(),
            *tail,
            timeout,
            *follow,
        ),
        Some(AgentCommand::Wait {
            target,
            until,
            timeout,
            next,
        }) => handle_agent_wait(target, until, timeout, *next, &args),
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

fn handle_agent_worktree(command: &AgentWorktreeCommand) -> Result<String, CliError> {
    match command {
        AgentWorktreeCommand::List => {
            let worktrees = live::agent_worktree_list().map_err(control_error)?;
            render_worktree_list(&worktrees)
        }
        AgentWorktreeCommand::Enable { target, name } => {
            let response =
                live::agent_worktree_enable(target, name.as_deref()).map_err(control_error)?;
            render_worktree_mutation_response(&response)
        }
        AgentWorktreeCommand::Join { target, worktree } => {
            let response = live::agent_worktree_join(target, worktree).map_err(control_error)?;
            render_worktree_mutation_response(&response)
        }
        AgentWorktreeCommand::Disable { target } => {
            let response = live::agent_worktree_disable(target).map_err(control_error)?;
            render_worktree_mutation_response(&response)
        }
    }
}

fn render_worktree_list(
    worktrees: &[wardian_core::control::AgentWorktreeSummary],
) -> Result<String, CliError> {
    serde_json::to_string_pretty(&serde_json::json!({
        "schema": 1,
        "worktrees": worktrees,
    }))
    .map(|json| format!("{json}\n"))
    .map_err(|e| CliError::generic(e.to_string()))
}

fn render_worktree_mutation_response(
    response: &wardian_core::control::AgentWorktreeMutationResponse,
) -> Result<String, CliError> {
    serde_json::to_string_pretty(response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn handle_agent_wait(
    target: &str,
    until: &str,
    timeout: &str,
    next: bool,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let timeout = parse_timeout(timeout)?;
    if next {
        let response =
            live::wait_agent_until_next(target, until, timeout).map_err(control_error)?;
        return serde_json::to_string_pretty(&response)
            .map(|json| format!("{json}\n"))
            .map_err(|e| CliError::generic(e.to_string()));
    }
    let agent = live::wait_agent_until(target, until, timeout).map_err(control_error)?;
    render_show(&agent, &render_options(args))
}

fn handle_agent_watch(
    target: &str,
    since: Option<&str>,
    until: Option<&str>,
    include: Option<&str>,
    tail: Option<usize>,
    timeout: &str,
    follow: bool,
) -> Result<String, CliError> {
    if follow {
        return Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            "agent watch --follow is reserved for a future streaming implementation",
        ));
    }
    let timeout = parse_timeout(timeout)?;
    let include = parse_include(include);
    let response = live::agent_watch(target, since, until, include, tail, follow, timeout)
        .map_err(control_error)?;
    serde_json::to_string_pretty(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
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
    let message = read_message_input(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    let input_mode = if args.as_command {
        validate_single_agent_target(&args.to, "send --as-command")?;
        validate_send_command_thread(args.thread.as_deref())?;
        MessageInputMode::Command
    } else {
        MessageInputMode::Message
    };

    let response = if let Some(until) = args.wait_until.as_deref() {
        validate_single_agent_target(&args.to, "send --wait-until")?;
        let timeout = parse_timeout(&args.timeout)?;
        let response = live::send_message_and_watch(
            &args.to,
            &message,
            args.thread.as_deref(),
            input_mode,
            until,
            timeout,
        )
        .map_err(control_error)?;
        let watch = response.watch;
        serde_json::json!({
            "schema": 1,
            "ok": true,
            "target": args.to,
            "input_mode": input_mode,
            "status": watch.agent.status,
            "delivery": response.delivery,
            "cursor": watch.cursor,
        })
    } else {
        let sent = if input_mode == MessageInputMode::Message {
            live::send_message(&args.to, &message, args.thread.as_deref())
        } else {
            live::send_message_with_input_mode(
                &args.to,
                &message,
                args.thread.as_deref(),
                input_mode,
            )
        }
        .map_err(control_error)?;
        serde_json::json!({
            "schema": 1,
            "ok": true,
            "target": args.to,
            "input_mode": input_mode,
            "delivery": sent.delivery,
        })
    };

    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&response).unwrap()
    ))
}

fn handle_ask(args: AskArgs) -> Result<String, CliError> {
    validate_single_agent_target(&args.target, "ask")?;
    validate_ask_thread(args.thread.as_deref())?;
    let message = read_message_input(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    let timeout = parse_timeout(&args.timeout)?;
    let condition = normalize_ask_condition(args.until.as_deref().unwrap_or("status:idle"))?;
    let response = live::ask_agent(
        &args.target,
        &message,
        args.thread.as_deref(),
        &condition,
        Some(args.tail),
        timeout,
    )
    .map_err(control_error)?;
    render_ask_response(&args.target, &condition, response)
}

fn handle_reply(args: ReplyArgs) -> Result<String, CliError> {
    let body = read_message_input(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    let response = live::submit_reply(
        &args.request_id,
        reply_status_arg_to_control(args.status),
        &body,
    )
    .map_err(control_error)?;
    serde_json::to_string_pretty(&serde_json::json!({
        "schema": 1,
        "ok": true,
        "request_id": response.request_id,
        "reply": response.reply,
    }))
    .map(|json| format!("{json}\n"))
    .map_err(|e| CliError::generic(e.to_string()))
}

fn reply_status_arg_to_control(status: ReplyStatusArg) -> wardian_core::control::ReplyStatus {
    match status {
        ReplyStatusArg::Done => wardian_core::control::ReplyStatus::Done,
        ReplyStatusArg::Blocked => wardian_core::control::ReplyStatus::Blocked,
        ReplyStatusArg::Failed => wardian_core::control::ReplyStatus::Failed,
    }
}

fn read_message_input(
    message: Option<&str>,
    stdin: bool,
    file: Option<&str>,
) -> Result<String, CliError> {
    if stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::generic(e.to_string()))?;
        Ok(buf)
    } else if let Some(path) = file {
        std::fs::read_to_string(path).map_err(|e| CliError::generic(e.to_string()))
    } else {
        message
            .map(ToOwned::to_owned)
            .ok_or_else(|| CliError::generic("Provide a message, --stdin, or --file".to_string()))
    }
}

fn validate_single_agent_target(target: &str, command_name: &str) -> Result<(), CliError> {
    if target == "all" || target.starts_with("class:") {
        return Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            format!("{command_name} requires a single agent name or uuid"),
        ));
    }
    Ok(())
}

fn validate_ask_thread(thread: Option<&str>) -> Result<(), CliError> {
    if thread.is_some() {
        return Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            "--thread is not supported by wardian ask yet",
        ));
    }
    Ok(())
}

fn validate_send_command_thread(thread: Option<&str>) -> Result<(), CliError> {
    if thread.is_some() {
        return Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            "--as-command cannot be combined with --thread",
        ));
    }
    Ok(())
}

fn normalize_ask_condition(until: &str) -> Result<String, CliError> {
    if until == "reply"
        || until.starts_with("status:")
        || until.starts_with("output:")
        || until.starts_with("event:")
        || until.starts_with("delivery:")
    {
        Ok(until.to_string())
    } else if until.contains(':') {
        Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            format!("unsupported watch condition: {until}"),
        ))
    } else {
        Ok(format!("status:{until}"))
    }
}

fn render_ask_response(
    target: &str,
    condition: &str,
    ask: live::AskAgentResponse,
) -> Result<String, CliError> {
    let watch = ask.watch;
    let response = serde_json::json!({
        "schema": 1,
        "ok": true,
        "target": target,
        "condition": condition,
        "request_id": ask.request_id,
        "reply": ask.reply,
        "agent": watch.agent,
        "cursor": watch.cursor,
        "delivery": ask.delivery,
        "output": watch.output,
        "events": watch.events,
    });
    serde_json::to_string_pretty(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn parse_include(include: Option<&str>) -> Vec<String> {
    include
        .unwrap_or("status,output,delivery")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
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
    } else if let Some(watch_timeout) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<live::WatchTimeoutError>())
    {
        CliError::backend(
            ExitCode::Generic,
            "watch_timeout",
            watch_timeout.to_string(),
        )
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
        let backend_error = |exit_code, code: &'static str| {
            endpoint_error.details().cloned().map_or_else(
                || CliError::backend(exit_code, code, endpoint_error.to_string()),
                |details| {
                    CliError::backend_with_details(
                        exit_code,
                        code,
                        endpoint_error.to_string(),
                        details,
                    )
                },
            )
        };
        match endpoint_error.code() {
            "not_supported" => backend_error(ExitCode::Generic, "not_supported"),
            "not_found" => backend_error(ExitCode::NotFound, "not_found"),
            "request_failed" => backend_error(ExitCode::Generic, "request_failed"),
            "not_managed_worktree" => backend_error(ExitCode::Generic, "not_managed_worktree"),
            "watch_timeout" => backend_error(ExitCode::Generic, "watch_timeout"),
            "cursor_expired" => backend_error(ExitCode::Generic, "cursor_expired"),
            "gap_detected" => backend_error(ExitCode::Generic, "gap_detected"),
            "invalid_cursor" => backend_error(ExitCode::Generic, "invalid_cursor"),
            _ => endpoint_error.details().cloned().map_or_else(
                || CliError::generic(endpoint_error.to_string()),
                |details| {
                    CliError::backend_with_details(
                        ExitCode::Generic,
                        "generic",
                        endpoint_error.to_string(),
                        details,
                    )
                },
            ),
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
    fn control_error_does_not_map_watch_timeout_to_app_not_running() {
        let error = std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            live::WatchTimeoutError::new("Wardian-Codex", "output:OK", "idle"),
        );

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "watch_timeout");
        assert_eq!(cli_error.code_i32(), 1);
        assert!(cli_error.message.contains("Wardian-Codex"));
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
    fn render_ask_response_uses_send_delivery() {
        let ask = live::AskAgentResponse {
            request_id: None,
            reply: None,
            delivery: vec![wardian_core::control::DeliveryDetail {
                uuid: "agent-1".to_string(),
                name: "reviewer-a1".to_string(),
                provider: "mock".to_string(),
                runtime_state: "live_pty_available".to_string(),
                delivery_state: "submitted".to_string(),
                input_mode: MessageInputMode::Message,
                error: None,
            }],
            watch: wardian_core::control::AgentWatchResponse {
                schema: 1,
                agent: wardian_core::control::WatchAgentSnapshot {
                    uuid: "agent-1".to_string(),
                    name: "reviewer-a1".to_string(),
                    provider: "mock".to_string(),
                    status: "idle".to_string(),
                    last_status_at: None,
                },
                cursor: "agent-1:2".to_string(),
                events: Vec::new(),
                output: wardian_core::control::WatchOutput {
                    cursor: "agent-1:2".to_string(),
                    text: "done".to_string(),
                    truncated: false,
                    omitted_bytes: 0,
                },
                delivery: wardian_core::control::WatchDeliverySnapshot {
                    delivery: Vec::new(),
                },
            },
        };

        let rendered = render_ask_response("reviewer-a1", "status:idle", ask).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(json["delivery"][0]["delivery_state"], "submitted");
        assert_eq!(json["output"]["text"], "done");
    }

    #[test]
    fn render_ask_response_includes_structured_reply() {
        let ask = live::AskAgentResponse {
            request_id: Some("ask_0123456789abcdef".to_string()),
            reply: Some(wardian_core::control::StructuredReply {
                request_id: "ask_0123456789abcdef".to_string(),
                status: wardian_core::control::ReplyStatus::Done,
                body: "finished".to_string(),
                target_session_id: "agent-1".to_string(),
                source_session_id: Some("agent-1".to_string()),
                replied_at: "2026-05-13T00:00:00.000Z".to_string(),
            }),
            delivery: Vec::new(),
            watch: wardian_core::control::AgentWatchResponse {
                schema: 1,
                agent: wardian_core::control::WatchAgentSnapshot {
                    uuid: "agent-1".to_string(),
                    name: "reviewer-a1".to_string(),
                    provider: "mock".to_string(),
                    status: "idle".to_string(),
                    last_status_at: None,
                },
                cursor: "agent-1:2".to_string(),
                events: Vec::new(),
                output: wardian_core::control::WatchOutput {
                    cursor: "agent-1:2".to_string(),
                    text: String::new(),
                    truncated: false,
                    omitted_bytes: 0,
                },
                delivery: wardian_core::control::WatchDeliverySnapshot {
                    delivery: Vec::new(),
                },
            },
        };

        let rendered = render_ask_response("reviewer-a1", "reply", ask).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(json["request_id"], "ask_0123456789abcdef");
        assert_eq!(json["reply"]["status"], "done");
        assert_eq!(json["reply"]["body"], "finished");
    }

    #[test]
    fn normalize_ask_condition_keeps_structured_reply_mode() {
        assert_eq!(normalize_ask_condition("reply").unwrap(), "reply");
    }

    #[test]
    fn render_worktree_mutation_response_keeps_schema_and_worktree_details() {
        let response = wardian_core::control::AgentWorktreeMutationResponse {
            schema: 1,
            ok: true,
            action: "join".to_string(),
            agent: wardian_core::identity::AgentIdentity {
                name: "coder-a1".to_string(),
                uuid: "agent-1".to_string(),
                class: "Coder".to_string(),
                provider: "codex".to_string(),
                status: "processing".to_string(),
                pid: None,
                started_at: None,
                workspace: Some("D:/repo/worktrees/review".to_string()),
                last_status_at: None,
                status_source: wardian_core::identity::StatusSource::Live,
            },
            worktree: Some(wardian_core::control::AgentWorktreeSummary {
                id: "D:/repo/worktrees/review".to_string(),
                name: "review".to_string(),
                source_folder: "D:/repo".to_string(),
                worktree_folder: "D:/repo/worktrees/review".to_string(),
                member_agent_ids: vec!["agent-1".to_string(), "agent-2".to_string()],
            }),
            previous_worktree: None,
            previous_workspace: Some("D:/repo".to_string()),
            current_workspace: Some("D:/repo/worktrees/review".to_string()),
            branch_name: None,
            cleared_session: true,
        };

        let rendered = render_worktree_mutation_response(&response).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(json["schema"], 1);
        assert_eq!(json["action"], "join");
        assert_eq!(json["agent"]["uuid"], "agent-1");
        assert_eq!(json["worktree"]["source_folder"], "D:/repo");
        assert_eq!(json["worktree"]["member_agent_ids"][1], "agent-2");
        assert_eq!(json["cleared_session"], true);
    }

    #[test]
    fn normalize_ask_condition_accepts_known_kinds_and_bare_status() {
        assert_eq!(normalize_ask_condition("idle").unwrap(), "status:idle");
        assert_eq!(
            normalize_ask_condition("output:REVIEW_DONE").unwrap(),
            "output:REVIEW_DONE"
        );
        assert_eq!(
            normalize_ask_condition("delivery:submitted").unwrap(),
            "delivery:submitted"
        );
    }

    #[test]
    fn normalize_ask_condition_rejects_unknown_colon_kind() {
        let error = normalize_ask_condition("ouptut:REVIEW_DONE").unwrap_err();

        assert_eq!(error.code, "not_supported");
        assert!(error.message.contains("unsupported watch condition"));
        assert!(error.message.contains("ouptut:REVIEW_DONE"));
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
