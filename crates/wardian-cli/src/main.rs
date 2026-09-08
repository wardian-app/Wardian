mod args;
mod artifact;
mod automation_invoker;
mod automation_replay;
mod browser;
mod disk;
mod errors;
mod graph;
mod inbox;
mod json_input;
mod library;
mod listener;
mod live;
mod mcp;
mod memory;
mod output;
mod schema;
mod telemetry;
mod watchlist;
use args::{
    AgentArgs, AgentCommand, AgentWorktreeCommand, ApprovalArg, AskArgs, AutomationArgs,
    AutomationCommand, AutomationScheduleCommand, AutomationSessionCloseCommand, Cli, Command,
    ConversationArgs, ConversationCommand, DeliveryArgs, DeliveryCommand, NotifyArgs,
    NotifyCommand, QueuePolicyArg, ReplyArgs, ReplyStatusArg, ScheduleDefinitionArgs, SendArgs,
};
use clap::Parser;
use errors::{handle_parse_error, parse_error, CliError, ExitCode};
use output::{render_list, render_show, RenderOptions};
use std::{
    collections::HashMap,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use wardian_core::control::{
    ApprovalAction, AutomationRunResponse, InboxNotificationKind, InboxNotificationPayload,
    MessageInputMode, OrchestrationDeliveryOptions, QueuePolicy,
};
use wardian_core::identity::{self, ListFilters, Scope};
use wardian_core::models::{
    AutomationAssignments, LibraryEntry, LibraryIndexNode, ScheduleDefinition,
};
use wardian_core::native_transport::NativeMessageOperation;

fn main() {
    std::process::exit(run());
}

#[cfg(test)]
fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn run() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return handle_parse_error(error),
    };
    if let Command::Mcp { command } = &cli.command {
        return mcp::run(command);
    }
    // Static discovery must work without a home, migrations, or running app.
    if let Command::Schema { path } = &cli.command {
        return finish(schema::render(path));
    }
    if let Command::Browser(args) = &cli.command {
        if let Some(help) = browser::target_help(args) {
            return finish(Ok(help));
        }
    }
    if let Command::Agent(args) = &cli.command {
        if let Err(error) = validate_agent_output(args) {
            return finish(Err(error));
        }
    }
    if let Command::Automation(AutomationArgs {
        command: AutomationCommand::NodeTypes { node, json },
        ..
    }) = &cli.command
    {
        return finish(render_automation_node_types(node.as_deref(), *json));
    }
    if let Err(error) = wardian_core::automation_migration::migrate_current_home() {
        let error = CliError::generic(format!(
            "could not migrate legacy automation storage: {error}"
        ));
        error.emit();
        return error.code_i32();
    }
    let result = match cli.command {
        Command::Mcp { .. } => unreachable!("MCP runs before home migration"),
        Command::Schema { path } => schema::render(&path),
        Command::Agent(args) => handle_agent(args),
        Command::Artifact(args) => artifact::handle_artifact(args),
        Command::Browser(args) => browser::handle_browser(args),
        Command::Conversation(args) => handle_conversation(args),
        Command::Inbox(args) => inbox::handle_inbox(args),
        Command::Memory(args) => memory::handle_memory(args),
        Command::Library(args) => library::handle_library(args),
        Command::Automation(args) => handle_automation(args),
        Command::Team(args) => watchlist::handle_team(args),
        Command::Watchlist(args) => watchlist::handle_watchlist(args),
        Command::Telemetry(args) => telemetry::handle_telemetry(args),
        Command::Graph(args) => graph::handle_graph(args),
        Command::Send(args) => handle_send(args),
        Command::Delivery(args) => handle_delivery(args),
        Command::Notify(args) => handle_notify(args),
        Command::Ask(args) => handle_ask(args),
        Command::Reply(args) => handle_reply(args),
    };

    finish(result)
}

fn finish(result: Result<String, CliError>) -> i32 {
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

// ---------------------------------------------------------------------------
// wardian conversation
// ---------------------------------------------------------------------------

fn handle_conversation(args: ConversationArgs) -> Result<String, CliError> {
    match args.command {
        ConversationCommand::List { agent, scope } => render_conversation_list(agent, &scope),
        ConversationCommand::Show { conversation_id } => render_conversation_show(&conversation_id),
    }
}

fn render_conversation_list(agent: Option<String>, scope: &str) -> Result<String, CliError> {
    let scope_all = match scope {
        "current" => false,
        "all" => true,
        other => {
            return Err(CliError::generic(format!(
                "unsupported conversation scope `{other}`; expected current or all"
            )))
        }
    };
    let effective_agent = resolve_conversation_agent(agent, scope_all)?;
    let (response, source) = read_snapshot(
        live::conversation_list(effective_agent.as_deref(), scope_all),
        || {
            disk::load_conversation_list(effective_agent.as_deref(), scope_all)
                .map(wardian_core::control::ConversationListResponse::new)
        },
    )?;
    render_conversation_response(response, source)
}

fn render_conversation_show(conversation_id: &str) -> Result<String, CliError> {
    let (response, source) = read_snapshot(live::conversation_show(conversation_id), || {
        disk::load_conversation_show(conversation_id)
    })?;
    render_conversation_response(response, source)
}

fn render_conversation_response<T: serde::Serialize>(
    response: T,
    source: &str,
) -> Result<String, CliError> {
    let mut body =
        serde_json::to_value(response).map_err(|error| CliError::generic(error.to_string()))?;
    body["status_source"] = serde_json::json!(source);
    render_json(body)
}

/// Persisted snapshots are an offline fallback, never a substitute for a live rejection.
fn read_snapshot<T>(
    result: std::io::Result<T>,
    offline: impl FnOnce() -> std::io::Result<T>,
) -> Result<(T, &'static str), CliError> {
    match result {
        Ok(value) => Ok((value, "live")),
        Err(error) if is_control_endpoint_unavailable(&error) => offline()
            .map(|value| (value, "persisted"))
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CliError::backend(ExitCode::NotFound, "not_found", error.to_string())
                } else {
                    CliError::generic(error.to_string())
                }
            }),
        Err(error) => Err(control_error(error)),
    }
}

fn resolve_conversation_agent(
    agent: Option<String>,
    scope_all: bool,
) -> Result<Option<String>, CliError> {
    if let Some(agent) = agent {
        return resolve_conversation_agent_target(&agent).map(Some);
    }
    if scope_all {
        return Ok(None);
    }

    std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Some)
        .ok_or_else(|| CliError::generic("--agent, --scope all, or WARDIAN_SESSION_ID is required"))
}

fn resolve_conversation_agent_target(target: &str) -> Result<String, CliError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(CliError::not_found(target));
    }
    let agents = match live::list_agents() {
        Ok(agents) => agents,
        Err(error) if is_control_endpoint_unavailable(&error) => {
            if wardian_core::paths::state_db_path().is_some_and(|path| path.exists()) {
                identity::list_agents(&open_db()?, &ListFilters::default())
                    .map_err(identity_error)?
            } else {
                Vec::new()
            }
        }
        Err(error) => return Err(control_error(error)),
    };
    if let Some(agent) = agents
        .iter()
        .find(|agent| agent.uuid == trimmed)
        .or_else(|| agents.iter().find(|agent| agent.name == trimmed))
    {
        return Ok(agent.uuid.clone());
    }
    // Removed agents can still have archived conversations addressable by ID.
    if !disk::load_conversation_list(Some(trimmed), false)
        .map_err(|error| CliError::generic(error.to_string()))?
        .is_empty()
    {
        return Ok(trimmed.to_string());
    }
    Err(CliError::not_found(trimmed))
}

// ---------------------------------------------------------------------------
// wardian agent
// ---------------------------------------------------------------------------

fn validate_agent_output(args: &AgentArgs) -> Result<(), CliError> {
    output::validate_options(&render_options(args))?;
    if (args.field.is_some() || args.fields.is_some() || args.pretty || args.verbose)
        && !matches!(
            args.command,
            None | Some(
                AgentCommand::Show { .. }
                    | AgentCommand::List { .. }
                    | AgentCommand::Spawn { .. }
                    | AgentCommand::Clone { .. }
            )
        )
    {
        return Err(CliError::backend(
            ExitCode::Generic,
            "invalid_arguments",
            "identity output flags apply only to agent show, list, spawn, and clone",
        ));
    }
    Ok(())
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
        Some(AgentCommand::Models { provider, refresh }) => handle_agent_models(provider, *refresh),
        None => handle_show(args.target.as_deref(), &args),
        Some(AgentCommand::Delete {
            target,
            confirm,
            force,
        }) => handle_agent_delete(target, confirm, *force),
        Some(AgentCommand::Rename { target, new_name }) => handle_agent_rename(target, new_name),
        Some(AgentCommand::Restart { target }) => handle_agent_restart(target),
        Some(AgentCommand::Pause { target }) => handle_agent_pause(target),
        Some(AgentCommand::Resume { target }) => handle_agent_resume(target),
        Some(AgentCommand::Spawn {
            provider,
            class,
            name,
            workspace,
            model,
            reasoning_effort,
        }) => handle_agent_spawn(
            provider,
            class,
            name.as_deref(),
            workspace.as_deref(),
            model.as_deref(),
            reasoning_effort.as_deref(),
            &args,
        ),
        Some(AgentCommand::Update {
            target,
            class,
            workspace,
            description,
            model,
            reasoning_effort,
        }) => handle_agent_update(
            target,
            class.as_deref(),
            workspace.as_deref(),
            description.as_deref(),
            model.as_deref(),
            reasoning_effort.as_deref(),
        ),
        Some(AgentCommand::Doctor { target }) => handle_agent_doctor(target),
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
            raw,
            tail,
            timeout,
            follow,
        }) => handle_agent_watch(
            target,
            AgentWatchCliOptions {
                since: since.as_deref(),
                until: until.as_deref(),
                include: include.as_deref(),
                raw: *raw,
                tail: *tail,
                timeout,
                follow: *follow,
            },
        ),
        Some(AgentCommand::Wait {
            target,
            until,
            timeout,
            next,
        }) => handle_agent_wait(target, until, timeout, *next, &args),
    }
}

fn handle_agent_delete(target: &str, confirm_name: &str, force: bool) -> Result<String, CliError> {
    live::agent_delete(target, confirm_name, force).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({
            "schema": 1,
            "ok": true,
            "target": target,
            "deleted": true,
            "removed": ["agent", "habitat", "session_history"]
        }))
        .unwrap()
    ))
}

fn handle_agent_rename(target: &str, new_name: &str) -> Result<String, CliError> {
    let response = live::agent_rename(target, new_name).map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn handle_agent_restart(target: &str) -> Result<String, CliError> {
    live::agent_restart(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target,"preserved":["agent","habitat","session_history"]}))
            .unwrap()
    ))
}

fn handle_agent_pause(target: &str) -> Result<String, CliError> {
    live::agent_pause(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target})).unwrap()
    ))
}

fn handle_agent_resume(target: &str) -> Result<String, CliError> {
    live::agent_resume(target).map_err(control_error)?;
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({"schema":1,"ok":true,"target":target})).unwrap()
    ))
}

fn handle_agent_spawn(
    provider: &str,
    class: &str,
    name: Option<&str>,
    workspace: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    args: &AgentArgs,
) -> Result<String, CliError> {
    let agent = live::agent_spawn(provider, class, name, workspace, model, reasoning_effort)
        .map_err(control_error)?;
    render_show(&agent, &render_options(args))
}

fn handle_agent_models(provider: &str, force_refresh: bool) -> Result<String, CliError> {
    let catalog = live::agent_models(provider, force_refresh).map_err(control_error)?;
    serde_json::to_string(&catalog)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn handle_agent_update(
    target: &str,
    class: Option<&str>,
    workspace: Option<&str>,
    description: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> Result<String, CliError> {
    let response = live::agent_update(
        target,
        class,
        workspace,
        description,
        model,
        reasoning_effort,
    )
    .map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn handle_agent_doctor(target: &str) -> Result<String, CliError> {
    let response = live::agent_doctor(target).map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
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
    serde_json::to_string(&serde_json::json!({
        "schema": 1,
        "worktrees": worktrees,
    }))
    .map(|json| format!("{json}\n"))
    .map_err(|e| CliError::generic(e.to_string()))
}

fn render_worktree_mutation_response(
    response: &wardian_core::control::AgentWorktreeMutationResponse,
) -> Result<String, CliError> {
    serde_json::to_string(response)
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
        return serde_json::to_string(&response)
            .map(|json| format!("{json}\n"))
            .map_err(|e| CliError::generic(e.to_string()));
    }
    let agent = live::wait_agent_until(target, until, timeout).map_err(control_error)?;
    render_show(&agent, &render_options(args))
}

fn handle_agent_watch(target: &str, options: AgentWatchCliOptions<'_>) -> Result<String, CliError> {
    if options.follow {
        return Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            "agent watch --follow is reserved for a future streaming implementation",
        ));
    }
    let timeout = parse_timeout(options.timeout)?;
    let include = parse_watch_include(options.include, options.raw);
    let response = live::agent_watch(
        target,
        options.since,
        options.until,
        include,
        options.tail,
        options.follow,
        timeout,
    )
    .map_err(control_error)?;
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

struct AgentWatchCliOptions<'a> {
    since: Option<&'a str>,
    until: Option<&'a str>,
    include: Option<&'a str>,
    raw: bool,
    tail: Option<usize>,
    timeout: &'a str,
    follow: bool,
}

// ---------------------------------------------------------------------------
// wardian automation
// ---------------------------------------------------------------------------

fn handle_automation(args: AutomationArgs) -> Result<String, CliError> {
    match args.command {
        AutomationCommand::NodeTypes { node, json } => {
            render_automation_node_types(node.as_deref(), json)
        }
        AutomationCommand::List => render_automation_list(args.pretty),
        AutomationCommand::Validate { path } => render_automation_validate(&path),
        AutomationCommand::Exec {
            path,
            executor,
            input,
            provider,
            workspace,
            bind,
        } => render_automation_exec(
            &path,
            &executor,
            input.as_deref(),
            provider.as_deref(),
            workspace.as_deref(),
            &bind,
        ),
        AutomationCommand::Runs => render_automation_runs(),
        AutomationCommand::RunShow {
            blueprint_id,
            run_id,
        } => render_automation_run_show(&blueprint_id, &run_id),
        AutomationCommand::Replay {
            blueprint_id,
            run_id,
        } => automation_replay::render(&blueprint_id, &run_id),
        AutomationCommand::Parse { path } => render_automation_parse(&path),
        AutomationCommand::Normalize { path, write } => render_automation_normalize(&path, write),
        AutomationCommand::GenSchema { out, check } => {
            render_automation_gen(&out, GenKind::Schema, check)
        }
        AutomationCommand::GenDocs { out, check } => {
            render_automation_gen(&out, GenKind::Docs, check)
        }
        AutomationCommand::Schedule(command) => render_automation_schedule(*command),
        AutomationCommand::SessionClose(command) => render_automation_session_close(*command),
        AutomationCommand::Listener(command) => listener::render(*command),
    }
}

fn render_automation_node_types(node: Option<&str>, json: bool) -> Result<String, CliError> {
    let definitions = if let Some(id) = node {
        let definition = wardian_core::automation::find_node_type(id).ok_or_else(|| {
            let mut error = CliError::backend(
                ExitCode::NotFound,
                "unknown_node_type",
                format!("unknown automation node type `{id}`"),
            );
            error.hint =
                Some("Run `wardian automation node-types` to list node types.".to_string());
            error
        })?;
        std::slice::from_ref(definition)
    } else {
        wardian_core::automation::node_types()
    };
    if json {
        return render_json(serde_json::json!({"schema": 2, "node_types": definitions}));
    }
    if node.is_some() {
        // A selected contract is useful without needing a second --json call.
        return render_json(serde_json::json!({"schema": 2, "node_types": definitions}));
    }
    // Human summary: one line per node type.
    let mut lines = String::from("NODE TYPES\n");
    for def in definitions {
        let status = if def.supported { "" } else { " [unsupported]" };
        lines.push_str(&format!(
            "  {:<18} {:<8} {}{}\n",
            def.id,
            format!("{:?}", def.kind).to_lowercase(),
            def.description,
            status
        ));
    }
    Ok(lines)
}

fn render_automation_list(pretty: bool) -> Result<String, CliError> {
    let home = wardian_core::paths::wardian_home()
        .ok_or_else(|| CliError::generic("could not resolve Wardian home"))?;
    let index = wardian_core::library::build_library_index(&home)
        .map_err(|error| CliError::generic(error.to_string()))?;
    let mut entries = Vec::new();
    if let Some(section) = index.sections.get("automations") {
        flatten_library_entries(&section.tree.children, &mut entries);
    }

    let automations_root =
        wardian_core::library::LibrarySectionId::Automations.root_for_home(&home);
    let mut automations = Vec::with_capacity(entries.len());
    for entry in entries {
        let automation_path = absolute_path(&automations_root.join(&entry.path))?;
        let mut row = serde_json::json!({
            "blueprint_id": serde_json::Value::Null,
            "name": entry.name,
            "entry_ref": entry.entry_ref,
            "automation_path": automation_path.to_string_lossy(),
            "error": serde_json::Value::Null,
        });

        match wardian_core::automation::parse_file(&automation_path) {
            Ok(blueprint) => {
                row["blueprint_id"] = serde_json::json!(blueprint.id);
                row["name"] = serde_json::json!(blueprint.name);
            }
            Err(error) => {
                row["error"] = serde_json::json!(error.to_string());
            }
        }
        automations.push(row);
    }

    if pretty {
        return Ok(render_automation_list_pretty(&automations));
    }

    render_json(serde_json::json!({
        "schema": 1,
        "automations": automations,
    }))
}

fn flatten_library_entries(nodes: &[LibraryIndexNode], entries: &mut Vec<LibraryEntry>) {
    for node in nodes {
        match node {
            LibraryIndexNode::Entry(entry) => entries.push(entry.clone()),
            LibraryIndexNode::Folder(folder) => {
                flatten_library_entries(&folder.children, entries);
            }
        }
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn render_automation_list_pretty(automations: &[serde_json::Value]) -> String {
    if automations.is_empty() {
        return "(no automations)\n".to_string();
    }

    let mut output = String::new();
    for automation in automations {
        let blueprint_id = automation["blueprint_id"]
            .as_str()
            .unwrap_or("<unparseable>");
        let name = automation["name"].as_str().unwrap_or_default();
        let entry_ref = automation["entry_ref"].as_str().unwrap_or_default();
        let path = automation["automation_path"].as_str().unwrap_or_default();
        output.push_str(&format!("{blueprint_id}  {name}  {entry_ref}  {path}\n"));
        if let Some(error) = automation["error"].as_str() {
            output.push_str(&format!("  error: {error}\n"));
        }
    }
    output
}

fn render_automation_validate(path: &str) -> Result<String, CliError> {
    let blueprint = wardian_core::automation::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    let report = wardian_core::automation::validate(&blueprint);
    let body = serde_json::json!({
        "schema": 1,
        "ok": report.is_valid(),
        "diagnostics": report.diagnostics,
    });
    if !report.is_valid() {
        return Err(CliError::backend_with_details(
            ExitCode::Generic,
            "validation_failed",
            "automation blueprint failed validation",
            body,
        ));
    }
    serde_json::to_string(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn render_automation_exec(
    path: &str,
    executor: &str,
    input: Option<&str>,
    provider: Option<&str>,
    workspace: Option<&str>,
    bind: &[String],
) -> Result<String, CliError> {
    render_automation_exec_with_live_launcher(
        path,
        executor,
        input,
        provider,
        workspace,
        bind,
        live::automation_run,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomationExecMode {
    Live,
    Mock,
}

impl AutomationExecMode {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "live" | "real" | "full" => Ok(Self::Live),
            "mock" => Ok(Self::Mock),
            other => Err(CliError::backend(
                ExitCode::Generic,
                "unsupported_executor",
                format!(
                    "unsupported automation executor `{other}`; expected live, real, full, or mock"
                ),
            )),
        }
    }
}

fn render_automation_exec_with_live_launcher(
    path: &str,
    executor: &str,
    input: Option<&str>,
    provider: Option<&str>,
    workspace: Option<&str>,
    bind: &[String],
    live_launcher: impl FnOnce(live::AutomationRunRequest) -> std::io::Result<AutomationRunResponse>,
) -> Result<String, CliError> {
    let input = automation_invoker::parse_automation_exec_input(input)?;
    let bindings = automation_invoker::parse_automation_bindings(bind)?;
    match AutomationExecMode::parse(executor)? {
        AutomationExecMode::Live => {
            let response = live_launcher(live::AutomationRunRequest {
                path: path.to_string(),
                provider: provider.map(str::to_string),
                workspace: workspace.map(str::to_string),
                input,
                bindings,
            })
            .map_err(control_error)?;
            render_live_automation_exec_response(response)
        }
        AutomationExecMode::Mock => render_automation_exec_mock(path, input),
    }
}

fn render_live_automation_exec_response(
    response: AutomationRunResponse,
) -> Result<String, CliError> {
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn render_automation_exec_mock(path: &str, input: serde_json::Value) -> Result<String, CliError> {
    let blueprint = wardian_core::automation::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    let report = wardian_core::automation::validate(&blueprint);
    if !report.is_valid() {
        let body = serde_json::json!({
            "schema": 1,
            "ok": false,
            "diagnostics": report.diagnostics,
        });
        return serde_json::to_string(&body)
            .map(|json| format!("{json}\n"))
            .map_err(|e| CliError::generic(e.to_string()));
    }

    let run_id = wardian_core::engine::driver::new_run_id();
    let run_root = wardian_core::paths::automation_run_dir(&blueprint.id, &run_id)
        .ok_or_else(|| CliError::generic("could not resolve automation run directory"))?;
    let runtime = build_automation_runtime()?;
    let mock = wardian_core::engine::MockExecutor::new();
    let state = runtime
        .block_on(wardian_core::engine::Engine::start_with_id(
            &blueprint, &run_id, input, &run_root, &mock,
        ))
        .map_err(|e| CliError::generic(e.to_string()))?;

    let body = serde_json::json!({
        "schema": 1,
        "ok": true,
        "run_id": run_id,
        "blueprint_id": blueprint.id,
        "status": state.status,
        "run_dir": run_root,
        "executor": "mock",
    });
    serde_json::to_string(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn render_automation_runs() -> Result<String, CliError> {
    let mut runs = Vec::new();
    let Some(runs_root) = wardian_core::paths::automation_runs_dir() else {
        return render_json(serde_json::json!({ "schema": 1, "runs": runs }));
    };
    if !runs_root.exists() {
        return render_json(serde_json::json!({ "schema": 1, "runs": runs }));
    }

    for blueprint_entry in fs::read_dir(&runs_root).map_err(|e| CliError::generic(e.to_string()))? {
        let blueprint_entry = blueprint_entry.map_err(|e| CliError::generic(e.to_string()))?;
        let blueprint_path = blueprint_entry.path();
        if !blueprint_path.is_dir() {
            continue;
        }
        for run_entry in
            fs::read_dir(&blueprint_path).map_err(|e| CliError::generic(e.to_string()))?
        {
            let run_entry = run_entry.map_err(|e| CliError::generic(e.to_string()))?;
            let run_path = run_entry.path();
            if !run_path.is_dir() {
                continue;
            }
            if let Ok(Some(state)) = wardian_core::engine::store::read_checkpoint(&run_path) {
                runs.push(serde_json::json!({
                    "run_id": state.run_id,
                    "blueprint_id": state.blueprint_id,
                    "status": state.status,
                    "node_count": state.nodes.len(),
                    "failure": state.failure,
                    "path": run_path,
                }));
            }
        }
    }

    render_json(serde_json::json!({
        "schema": 1,
        "runs": runs,
    }))
}

fn render_automation_run_show(blueprint_id: &str, run_id: &str) -> Result<String, CliError> {
    let run_root = automation_run_root(blueprint_id, run_id)?;
    let state = wardian_core::engine::store::read_checkpoint(&run_root)
        .map_err(|e| CliError::generic(e.to_string()))?
        .ok_or_else(|| CliError::generic(format!("state.json not found for run {run_id}")))?;
    let events = wardian_core::engine::store::read_events(&run_root)
        .map_err(|e| CliError::generic(e.to_string()))?;
    render_json(serde_json::json!({
        "schema": 1,
        "state": state,
        "events": events,
    }))
}

fn render_automation_parse(path: &str) -> Result<String, CliError> {
    let blueprint = wardian_core::automation::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    render_json(serde_json::json!({
        "schema": 1,
        "blueprint": blueprint,
    }))
}

fn render_automation_normalize(path: &str, write: bool) -> Result<String, CliError> {
    let mut blueprint = wardian_core::automation::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    wardian_core::automation::normalize(&mut blueprint);
    let normalized = wardian_core::automation::to_string(&blueprint)
        .map_err(|e| CliError::generic(e.to_string()))?;
    if write {
        fs::write(path, &normalized).map_err(|e| CliError::generic(e.to_string()))?;
        return render_json(serde_json::json!({
            "schema": 1,
            "written": true,
            "path": path,
        }));
    }
    Ok(normalized)
}

fn render_automation_schedule(command: AutomationScheduleCommand) -> Result<String, CliError> {
    use wardian_core::models::AutomationSchedule;
    use wardian_core::schedule::{compute_next_run, load_schedules, save_schedules};
    use AutomationScheduleCommand as C;

    match command {
        C::List => render_json(serde_json::json!({
            "schema": 1,
            "schedules": load_schedules(),
        })),
        C::Add {
            blueprint,
            name,
            cadence,
            provider,
            workspace,
            input,
            bind,
            assignments,
            paused,
        } => {
            if name.trim().is_empty() {
                return Err(CliError::generic("schedule name must not be empty"));
            }
            automation_invoker::validate_schedule_provider(provider.as_deref())?;
            let _blueprint = automation_invoker::validate_schedule_blueprint(&blueprint)?;
            let schedule = build_schedule_definition(&cadence, None)?;
            let workspace = resolve_schedule_workspace(&workspace)?;
            let input = automation_invoker::parse_automation_exec_input(input.as_deref())?;
            let (bindings, assignments) =
                parse_schedule_assignments(&bind, assignments.as_deref(), None)?;
            let now = current_epoch_ms();
            let record = AutomationSchedule {
                id: wardian_core::engine::driver::new_run_id(),
                blueprint_id: blueprint,
                name,
                provider,
                workspace: Some(workspace),
                input,
                bindings,
                assignments,
                next_run_epoch_ms: if paused {
                    None
                } else {
                    compute_next_run(&schedule, now)
                },
                paused_remaining_ms: None,
                is_paused: paused,
                last_run_status: None,
                last_run_error: None,
                last_run_epoch_ms: None,
                schedule,
            };
            let mut all = load_schedules();
            all.push(record.clone());
            save_schedules(&all).map_err(|error| CliError::generic(error.to_string()))?;
            render_json(serde_json::json!({
                "schema": 1,
                "ok": true,
                "schedule": record,
            }))
        }
        C::Update {
            id,
            blueprint,
            name,
            cadence,
            provider,
            workspace,
            input,
            bind,
            assignments,
            paused,
            active,
        } => {
            let mut all = load_schedules();
            let schedule = all
                .iter_mut()
                .find(|schedule| schedule.id == id)
                .ok_or_else(|| CliError::generic(format!("schedule not found: {id}")))?;

            let blueprint_id = blueprint.unwrap_or_else(|| schedule.blueprint_id.clone());
            let _blueprint = automation_invoker::validate_schedule_blueprint(&blueprint_id)?;
            let schedule_changed =
                schedule_definition_args_set(&cadence) || cadence.repeat_every.is_some();
            let end_changed = cadence.end.is_some()
                || cadence.end_date.is_some()
                || cadence.max_occurrences.is_some();
            if !schedule_changed
                && cadence.end.is_none()
                && (cadence.end_date.is_some() || cadence.max_occurrences.is_some())
            {
                return Err(CliError::generic(
                    "--end-date and --max-occurrences require --end",
                ));
            }
            if !schedule_changed && !end_changed {
                if schedule.schedule.end_condition.trim().is_empty() {
                    schedule.schedule.end_condition = "never".into();
                }
                wardian_core::schedule::validate_schedule_definition(&schedule.schedule)
                    .map_err(CliError::generic)?;
            }
            let next_definition = if schedule_changed || end_changed {
                Some(build_schedule_definition(
                    &cadence,
                    Some(&schedule.schedule),
                )?)
            } else {
                None
            };
            automation_invoker::validate_schedule_provider(provider.as_deref())?;
            let next_workspace = match workspace {
                Some(workspace) => Some(resolve_schedule_workspace(&workspace)?),
                None => schedule
                    .workspace
                    .as_deref()
                    .map(resolve_schedule_workspace)
                    .transpose()?,
            };
            let (next_bindings, next_assignments) = parse_schedule_assignments(
                &bind,
                assignments.as_deref(),
                Some((schedule.bindings.clone(), schedule.assignments.clone())),
            )?;
            let next_input = input
                .as_deref()
                .map(|value| automation_invoker::parse_automation_exec_input(Some(value)))
                .transpose()?;
            let now = current_epoch_ms();
            let was_paused = schedule.is_paused;
            let cadence_changed = next_definition.is_some();
            schedule.blueprint_id = blueprint_id;
            if let Some(name) = name {
                if name.trim().is_empty() {
                    return Err(CliError::generic("schedule name must not be empty"));
                }
                schedule.name = name;
            }
            if let Some(provider) = provider {
                schedule.provider = Some(provider);
            }
            schedule.workspace = next_workspace;
            if let Some(input) = next_input {
                schedule.input = input;
            }
            schedule.bindings = next_bindings;
            schedule.assignments = next_assignments;
            if let Some(definition) = next_definition {
                schedule.schedule = definition;
            }

            if paused {
                schedule.is_paused = true;
                schedule.paused_remaining_ms = schedule
                    .next_run_epoch_ms
                    .map(|next_run| next_run.saturating_sub(now));
                schedule.next_run_epoch_ms = None;
            } else if active {
                schedule.is_paused = false;
                schedule.paused_remaining_ms = None;
                schedule.next_run_epoch_ms = compute_next_run(&schedule.schedule, now);
            } else if cadence_changed {
                schedule.next_run_epoch_ms = if was_paused {
                    None
                } else {
                    compute_next_run(&schedule.schedule, now)
                };
            }
            if schedule.workspace.is_none() {
                return Err(CliError::generic(
                    "schedule has no workspace; provide --workspace",
                ));
            }
            let updated = schedule.clone();
            save_schedules(&all).map_err(|error| CliError::generic(error.to_string()))?;
            render_json(serde_json::json!({
                "schema": 1,
                "ok": true,
                "schedule": updated,
            }))
        }
        C::Pause { id } => mutate_schedule(&id, |schedule| {
            let now = current_epoch_ms();
            schedule.is_paused = true;
            schedule.paused_remaining_ms = schedule
                .next_run_epoch_ms
                .map(|next_run| next_run.saturating_sub(now));
            schedule.next_run_epoch_ms = None;
        }),
        C::Resume { id } => mutate_schedule(&id, |schedule| {
            let now = current_epoch_ms();
            schedule.is_paused = false;
            schedule.next_run_epoch_ms = match schedule.paused_remaining_ms.take() {
                Some(remaining) => Some(now.saturating_add(remaining)),
                None => compute_next_run(&schedule.schedule, now),
            };
        }),
        C::RunNow { id } => mutate_schedule(&id, |schedule| {
            schedule.is_paused = false;
            schedule.next_run_epoch_ms = Some(current_epoch_ms());
        }),
        C::Remove { id } => {
            let mut all = load_schedules();
            let before = all.len();
            all.retain(|schedule| schedule.id != id);
            save_schedules(&all).map_err(|error| CliError::generic(error.to_string()))?;
            render_json(serde_json::json!({
                "schema": 1,
                "ok": true,
                "removed": before - all.len(),
            }))
        }
    }
}

fn render_automation_session_close(
    command: AutomationSessionCloseCommand,
) -> Result<String, CliError> {
    use wardian_core::session_close::{
        load_invokers, mutate_invokers, AutomationSessionCloseInvoker,
    };
    use AutomationSessionCloseCommand as C;

    match command {
        C::List => render_json(serde_json::json!({
            "schema": 1,
            "session_close_invokers": load_invokers(),
        })),
        C::Add {
            blueprint,
            name,
            agent,
            boundary,
            provider,
            workspace,
            input,
            assignments,
            enable,
            require_archive,
        } => {
            automation_invoker::validate_schedule_blueprint(&blueprint)?;
            automation_invoker::validate_schedule_provider(provider.as_deref())?;
            let input = automation_invoker::parse_automation_exec_input(input.as_deref())?;
            let assignments: AutomationAssignments = assignments
                .as_deref()
                .map(|raw| json_input::parse(raw, "--assignments"))
                .transpose()?
                .unwrap_or_default();
            wardian_core::automation::assignment::validate_assignments(&assignments)
                .map_err(CliError::generic)?;
            let record = AutomationSessionCloseInvoker {
                id: wardian_core::engine::driver::new_run_id(),
                blueprint_id: blueprint,
                name,
                enabled: enable,
                require_archive,
                source_agent_id: agent
                    .map(|target| resolve_conversation_agent_target(&target))
                    .transpose()?,
                boundary_reasons: boundary,
                provider,
                workspace,
                input,
                bindings: wardian_core::automation::assignment::legacy_bindings(&assignments),
                assignments,
            };
            mutate_invokers(|invokers| {
                invokers.push(record.clone());
                Ok(())
            })
            .map_err(|error| CliError::generic(error.to_string()))?;
            render_json(serde_json::json!({ "schema": 1, "session_close_invoker": record }))
        }
        C::Enable { id } => mutate_session_close_invoker(&id, |invoker| invoker.enabled = true),
        C::Disable { id } => mutate_session_close_invoker(&id, |invoker| invoker.enabled = false),
        C::Remove { id } => {
            mutate_invokers(|invokers| {
                let before = invokers.len();
                invokers.retain(|invoker| invoker.id != id);
                if invokers.len() == before {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("session-close invoker not found: {id}"),
                    ));
                }
                Ok(())
            })
            .map_err(|error| CliError::generic(error.to_string()))?;
            render_json(serde_json::json!({ "schema": 1, "removed": id }))
        }
    }
}

fn mutate_session_close_invoker(
    id: &str,
    mutate: impl FnOnce(&mut wardian_core::session_close::AutomationSessionCloseInvoker),
) -> Result<String, CliError> {
    let result = wardian_core::session_close::mutate_invokers(|invokers| {
        let invoker = invokers
            .iter_mut()
            .find(|invoker| invoker.id == id)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("session-close invoker not found: {id}"),
                )
            })?;
        mutate(invoker);
        Ok(invoker.clone())
    })
    .map_err(|error| CliError::generic(error.to_string()))?;
    render_json(serde_json::json!({ "schema": 1, "session_close_invoker": result }))
}

fn mutate_schedule(
    id: &str,
    mutate: impl FnOnce(&mut wardian_core::models::AutomationSchedule),
) -> Result<String, CliError> {
    use wardian_core::schedule::{load_schedules, save_schedules};
    let mut all = load_schedules();
    let found = all.iter_mut().find(|schedule| schedule.id == id);
    let ok = found.is_some();
    if let Some(schedule) = found {
        mutate(schedule);
    }
    save_schedules(&all).map_err(|error| CliError::generic(error.to_string()))?;
    render_json(serde_json::json!({ "schema": 1, "ok": ok }))
}

fn build_schedule_definition(
    args: &ScheduleDefinitionArgs,
    base: Option<&ScheduleDefinition>,
) -> Result<ScheduleDefinition, CliError> {
    let mut definition = base.cloned().unwrap_or_else(|| ScheduleDefinition {
        active: true,
        ..Default::default()
    });
    let cadence_set = schedule_definition_args_set(args);
    if !cadence_set && base.is_none() {
        return Err(CliError::generic(
            "specify one of --every / --daily / --weekly / --monthly / --specific-dates / --at",
        ));
    }
    if let Some(minutes) = args.every {
        definition.schedule_type = "interval".into();
        definition.interval_minutes = Some(minutes);
        definition.time_of_day = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
        definition.run_at = None;
    } else if let Some(time) = args.daily.clone() {
        definition.schedule_type = "daily".into();
        definition.time_of_day = Some(time);
        definition.interval_minutes = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
        definition.run_at = None;
    } else if let Some(spec) = args.weekly.as_deref() {
        let (days, time) = spec
            .split_once('@')
            .ok_or_else(|| CliError::generic("--weekly expects Days@HH:MM, e.g. Mon,Fri@09:30"))?;
        definition.schedule_type = "weekly".into();
        definition.days_of_week = Some(days.split(',').map(|day| day.trim().to_string()).collect());
        definition.time_of_day = Some(time.to_string());
        definition.interval_minutes = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
        definition.run_at = None;
        if base.is_none_or(|base| base.schedule_type != "weekly") {
            definition.repeat_every = 1;
        }
    } else if let Some(spec) = args.monthly.as_deref() {
        let (days, time) = spec.split_once('@').ok_or_else(|| {
            CliError::generic("--monthly expects day numbers@HH:MM, e.g. 1,15@09:30")
        })?;
        let days = days
            .split(',')
            .map(|day| {
                day.trim()
                    .parse::<u32>()
                    .map_err(|_| CliError::generic(format!("invalid monthly day `{}`", day.trim())))
            })
            .collect::<Result<Vec<_>, _>>()?;
        definition.schedule_type = "monthly".into();
        definition.days_of_month = Some(days);
        definition.time_of_day = Some(time.to_string());
        definition.interval_minutes = None;
        definition.days_of_week = None;
        definition.specific_dates = None;
        definition.run_at = None;
    } else if let Some(spec) = args.specific_dates.as_deref() {
        let (dates, time) = spec.split_once('@').ok_or_else(|| {
            CliError::generic(
                "--specific-dates expects YYYY-MM-DD dates@HH:MM, e.g. 2026-09-01,2026-09-15@09:30",
            )
        })?;
        let dates = dates
            .split(',')
            .map(|date| date.trim().to_string())
            .collect::<Vec<_>>();
        definition.schedule_type = "specific_dates".into();
        definition.specific_dates = Some(dates);
        definition.time_of_day = Some(time.to_string());
        definition.interval_minutes = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.run_at = None;
    } else if let Some(when) = args.at.clone() {
        definition.schedule_type = "one_time".into();
        definition.run_at = Some(when);
        definition.interval_minutes = None;
        definition.time_of_day = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
    }
    if let Some(repeat_every) = args.repeat_every {
        if repeat_every == 0 {
            return Err(CliError::generic(
                "--repeat-every must be greater than zero",
            ));
        }
        if definition.schedule_type != "weekly" {
            return Err(CliError::generic(
                "--repeat-every is only supported for weekly schedules",
            ));
        }
        definition.repeat_every = repeat_every;
    }
    apply_end_condition(args, &mut definition)?;
    if definition.end_condition.trim().is_empty() {
        definition.end_condition = "never".into();
    }
    wardian_core::schedule::validate_schedule_definition(&definition).map_err(CliError::generic)?;
    Ok(definition)
}

fn schedule_definition_args_set(args: &ScheduleDefinitionArgs) -> bool {
    args.every.is_some()
        || args.daily.is_some()
        || args.weekly.is_some()
        || args.monthly.is_some()
        || args.specific_dates.is_some()
        || args.at.is_some()
}

fn apply_end_condition(
    args: &ScheduleDefinitionArgs,
    definition: &mut ScheduleDefinition,
) -> Result<(), CliError> {
    if args.end.is_none() && (args.end_date.is_some() || args.max_occurrences.is_some()) {
        return Err(CliError::generic(
            "--end-date and --max-occurrences require --end",
        ));
    }
    let Some(end) = args.end.as_deref() else {
        return Ok(());
    };
    match end {
        "never" => {
            definition.end_condition = "never".into();
            definition.end_date = None;
            definition.max_occurrences = None;
        }
        "on_date" => {
            let end_date = args
                .end_date
                .clone()
                .ok_or_else(|| CliError::generic("--end on_date requires --end-date"))?;
            definition.end_condition = "on_date".into();
            definition.end_date = Some(end_date);
            definition.max_occurrences = None;
        }
        "after_occurrences" => {
            let max_occurrences = args.max_occurrences.ok_or_else(|| {
                CliError::generic("--end after_occurrences requires --max-occurrences")
            })?;
            definition.end_condition = "after_occurrences".into();
            definition.end_date = None;
            definition.max_occurrences = Some(max_occurrences);
        }
        other => {
            return Err(CliError::generic(format!(
                "invalid --end `{other}`; expected never, on_date, or after_occurrences"
            )));
        }
    }
    Ok(())
}

fn resolve_schedule_workspace(value: &str) -> Result<String, CliError> {
    wardian_core::schedule::resolve_workspace_path(value)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(CliError::generic)
}

fn parse_schedule_assignments(
    bind: &[String],
    typed_json: Option<&str>,
    existing: Option<(HashMap<String, String>, AutomationAssignments)>,
) -> Result<(HashMap<String, String>, AutomationAssignments), CliError> {
    if bind.is_empty() && typed_json.is_none() {
        let Some((bindings, assignments)) = existing else {
            return Ok((HashMap::new(), AutomationAssignments::new()));
        };
        let assignments = canonicalize_schedule_assignments(assignments)?;
        return Ok((bindings, assignments));
    }

    let explicit_bindings = automation_invoker::parse_automation_bindings(bind)?;
    let typed = typed_json
        .map(|raw| json_input::parse::<AutomationAssignments>(raw, "--assignments"))
        .transpose()?;
    let assignments = if let Some(typed) = typed {
        wardian_core::automation::assignment::normalize_assignments(
            Some(typed),
            &explicit_bindings,
            wardian_core::models::InvocationKind::Scheduled,
        )
    } else {
        wardian_core::automation::assignment::normalize_assignments(
            None,
            &explicit_bindings,
            wardian_core::models::InvocationKind::Scheduled,
        )
    };
    let assignments = canonicalize_schedule_assignments(assignments)?;
    let mut bindings = wardian_core::automation::assignment::legacy_bindings(&assignments);
    bindings.extend(explicit_bindings);
    Ok((bindings, assignments))
}

fn canonicalize_schedule_assignments(
    mut assignments: AutomationAssignments,
) -> Result<AutomationAssignments, CliError> {
    for assignment in assignments.values_mut() {
        if let wardian_core::models::AutomationRoleAssignment::TemporaryProvider {
            workspace: Some(workspace),
            ..
        } = assignment
        {
            *workspace = resolve_schedule_workspace(workspace)?;
        }
    }
    wardian_core::automation::assignment::validate_assignments(&assignments)
        .map_err(CliError::generic)?;
    Ok(assignments)
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub(crate) fn render_json(body: serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn automation_run_root(blueprint_id: &str, run_id: &str) -> Result<PathBuf, CliError> {
    wardian_core::paths::automation_run_dir(blueprint_id, run_id)
        .ok_or_else(|| CliError::generic("could not resolve automation run directory"))
}

fn find_library_blueprint(
    blueprint_id: &str,
) -> Result<Option<wardian_core::automation::Blueprint>, CliError> {
    let Some(home) = wardian_core::paths::wardian_home() else {
        return Ok(None);
    };
    let root = home.join("library").join("automations");
    if !root.exists() {
        return Ok(None);
    }
    find_library_blueprint_in_dir(&root, blueprint_id)
}

fn find_library_blueprint_in_dir(
    dir: &Path,
    blueprint_id: &str,
) -> Result<Option<wardian_core::automation::Blueprint>, CliError> {
    for entry in fs::read_dir(dir).map_err(|e| CliError::generic(e.to_string()))? {
        let entry = entry.map_err(|e| CliError::generic(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(blueprint) = find_library_blueprint_in_dir(&path, blueprint_id)? {
                return Ok(Some(blueprint));
            }
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let blueprint = wardian_core::automation::parse_file(&path)
            .map_err(|e| CliError::generic(e.to_string()))?;
        if blueprint.id == blueprint_id {
            return Ok(Some(blueprint));
        }
    }
    Ok(None)
}

fn build_automation_runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::generic(e.to_string()))
}

#[derive(Clone, Copy)]
enum GenKind {
    Schema,
    Docs,
}

fn render_automation_gen(out: &str, kind: GenKind, check: bool) -> Result<String, CliError> {
    let generated = match kind {
        GenKind::Schema => format!("{}\n", wardian_core::automation::ts_schema_json()),
        GenKind::Docs => wardian_core::automation::reference_doc(),
    };
    let path = std::path::Path::new(out);
    if check {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        if current != generated {
            return Err(CliError::backend(
                ExitCode::Generic,
                "drift",
                format!("{out} is out of date; run the matching gen command and commit"),
            ));
        }
        return Ok(format!("{out} is up to date\n"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CliError::generic(e.to_string()))?;
    }
    std::fs::write(path, &generated).map_err(|e| CliError::generic(e.to_string()))?;
    Ok(format!("wrote {out}\n"))
}

// ---------------------------------------------------------------------------
// wardian send
// ---------------------------------------------------------------------------

fn handle_notify(args: NotifyArgs) -> Result<String, CliError> {
    let origin = live::require_current_message_origin().map_err(|_| CliError::not_in_session())?;
    let (notification, wait_timeout) = match args.command {
        NotifyCommand::Update {
            message,
            title,
            stdin,
            file,
        } => (
            InboxNotificationPayload {
                kind: InboxNotificationKind::Update,
                title,
                body: read_message_input(message.as_deref(), stdin, file.as_deref())?,
                proposed_action: None,
                risk: None,
                choices: Vec::new(),
                expires_at: None,
            },
            None,
        ),
        NotifyCommand::Approval {
            message,
            title,
            action,
            risk,
            choices,
            expires_in,
            wait,
            timeout,
            stdin,
            file,
        } => {
            let expires_in = parse_timeout(&expires_in)?;
            if expires_in.is_zero() || expires_in > Duration::from_secs(24 * 60 * 60) {
                return Err(CliError::backend(
                    ExitCode::Generic,
                    "invalid_expiry",
                    "--expires-in must be greater than zero and no longer than 24h",
                ));
            }
            let expires_at = chrono::Utc::now()
                + chrono::Duration::from_std(expires_in)
                    .map_err(|error| CliError::generic(error.to_string()))?;
            (
                InboxNotificationPayload {
                    kind: InboxNotificationKind::Approval,
                    title,
                    body: read_message_input(message.as_deref(), stdin, file.as_deref())?,
                    proposed_action: Some(action),
                    risk: Some(risk),
                    choices,
                    expires_at: Some(
                        expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    ),
                },
                wait.then(|| parse_timeout(&timeout)).transpose()?,
            )
        }
    };
    let created = live::create_notification(notification, origin).map_err(control_error)?;
    let response = if let Some(timeout) = wait_timeout {
        live::wait_for_notification(&created.notification_id, timeout).map_err(control_error)?
    } else {
        created
    };
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn handle_send(args: SendArgs) -> Result<String, CliError> {
    let approval_action = args.approval.map(approval_arg_to_control);
    let message = read_send_message_input(
        args.message.as_deref(),
        args.stdin,
        args.file.as_deref(),
        args.approval,
    )?;
    let queue_policy = queue_policy_arg_to_control(args.queue_policy);
    let orchestration = orchestration_options(
        args.idempotency_key.clone(),
        args.deadline.as_deref(),
        args.expires_in.as_deref(),
        args.expected_generation,
        args.invalidate_premise,
    )?;
    let input_mode = if approval_action.is_some() {
        MessageInputMode::ApprovalAction
    } else if args.as_command {
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
            live::SendMessageAndWatchOptions {
                thread: args.thread.as_deref(),
                input_mode,
                queue_policy,
                approval_action,
                until,
                timeout,
                target_scope: Some(args.scope.as_str()),
                orchestration: orchestration.clone(),
            },
        )
        .map_err(control_error)?;
        let watch = response.watch;
        serde_json::json!({
            "schema": 1,
            "ok": true,
            "target": args.to,
            "input_mode": input_mode,
            "status": watch.agent.status,
            "delivery": response.delivery, "watch_error": response.watch_error,
            "cursor": watch.cursor,
        })
    } else {
        let timeout = parse_timeout(&args.timeout)?;
        let sent = live::send_message_with_delivery_and_scope_options(
            &args.to,
            &message,
            live::SendMessageDeliveryOptions {
                thread: args.thread.as_deref(),
                input_mode,
                queue_policy,
                approval_action,
                target_scope: Some(args.scope.as_str()),
                timeout,
                orchestration,
            },
        )
        .map_err(control_error)?;
        serde_json::json!({
            "schema": 1,
            "ok": true,
            "target": args.to,
            "input_mode": input_mode,
            "delivery": sent.delivery,
        })
    };

    Ok(format!("{}\n", serde_json::to_string(&response).unwrap()))
}

fn handle_delivery(args: DeliveryArgs) -> Result<String, CliError> {
    let value = match args.command {
        DeliveryCommand::Show {
            interaction_id,
            evidence_limit,
        } => live::delivery_get(&interaction_id, evidence_limit)
            .and_then(|response| serde_json::to_value(response).map_err(std::io::Error::other)),
        DeliveryCommand::Cancel { interaction_id } => live::delivery_cancel(&interaction_id)
            .and_then(|response| serde_json::to_value(response).map_err(std::io::Error::other)),
        DeliveryCommand::Withdraw { interaction_id } => live::delivery_withdraw(&interaction_id)
            .and_then(|response| serde_json::to_value(response).map_err(std::io::Error::other)),
        DeliveryCommand::Replace {
            interaction_id,
            message,
            stdin,
            file,
            idempotency_key,
            deadline,
            expires_in,
        } => {
            let message = read_message_input(message.as_deref(), stdin, file.as_deref())?;
            let deadline_at = delivery_deadline(deadline.as_deref(), expires_in.as_deref())?;
            live::delivery_replace(&interaction_id, &message, &idempotency_key, deadline_at)
                .and_then(|response| serde_json::to_value(response).map_err(std::io::Error::other))
        }
        DeliveryCommand::Capabilities { target } => live::delivery_capabilities(&target)
            .and_then(|response| serde_json::to_value(response).map_err(std::io::Error::other)),
    }
    .map_err(control_error)?;
    serde_json::to_string(&value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn handle_ask(args: AskArgs) -> Result<String, CliError> {
    validate_single_agent_target(&args.target, "ask")?;
    for target in &args.targets {
        validate_single_agent_target(target, "ask")?;
    }
    validate_ask_thread(args.thread.as_deref())?;
    let message = read_message_input(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    let timeout = parse_timeout(&args.timeout)?;
    let condition = normalize_ask_condition(args.until.as_deref().unwrap_or("status:idle"))?;
    let orchestration = orchestration_options(
        args.idempotency_key,
        args.deadline.as_deref(),
        args.expires_in.as_deref(),
        args.expected_generation,
        args.invalidate_premise,
    )?;
    let mut targets = vec![args.target.clone()];
    targets.extend(args.targets);
    targets.sort();
    targets.dedup();

    if targets.len() > 1 && condition == "reply" {
        let response = live::ask_agents(
            &targets,
            &message,
            args.thread.as_deref(),
            Some(args.tail),
            timeout,
            orchestration,
        )
        .map_err(control_error)?;
        return render_ask_many_response(&response);
    }
    if targets.len() > 1 {
        return Err(CliError::backend(
            ExitCode::Generic,
            "not_supported",
            "multi-target wardian ask requires --until reply",
        ));
    }
    let response = live::ask_agent(
        &targets[0],
        &message,
        args.thread.as_deref(),
        &condition,
        Some(args.tail),
        timeout,
        orchestration,
    )
    .map_err(control_error)?;
    render_ask_response(&targets[0], &condition, response)
}

fn orchestration_options(
    idempotency_key: Option<String>,
    deadline: Option<&str>,
    expires_in: Option<&str>,
    expected_generation: Option<u64>,
    invalidate_premise: bool,
) -> Result<Option<OrchestrationDeliveryOptions>, CliError> {
    let deadline_at = delivery_deadline(deadline, expires_in)?;
    if idempotency_key.is_none()
        && deadline_at.is_none()
        && expected_generation.is_none()
        && !invalidate_premise
    {
        return Ok(None);
    }
    Ok(Some(OrchestrationDeliveryOptions {
        idempotency_key,
        deadline_at,
        expected_generation,
        operation: if invalidate_premise {
            NativeMessageOperation::InvalidatePremise
        } else {
            NativeMessageOperation::StartTurn
        },
    }))
}

fn delivery_deadline(
    deadline: Option<&str>,
    expires_in: Option<&str>,
) -> Result<Option<String>, CliError> {
    if let Some(deadline) = deadline {
        let parsed = chrono::DateTime::parse_from_rfc3339(deadline).map_err(|error| {
            CliError::generic(format!("invalid --deadline RFC3339 value: {error}"))
        })?;
        return Ok(Some(
            parsed
                .with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ));
    }
    expires_in.map(parse_timeout).transpose().map(|duration| {
        duration.map(|duration| {
            (chrono::Utc::now()
                + chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
    })
}

fn handle_reply(args: ReplyArgs) -> Result<String, CliError> {
    let body = read_message_input(args.message.as_deref(), args.stdin, args.file.as_deref())?;
    let response = live::submit_reply(
        &args.request_id,
        reply_status_arg_to_control(args.status),
        &body,
    )
    .map_err(control_error)?;
    serde_json::to_string(&serde_json::json!({
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

fn queue_policy_arg_to_control(policy: QueuePolicyArg) -> QueuePolicy {
    match policy {
        QueuePolicyArg::QueueIfBusy => QueuePolicy::QueueIfBusy,
        QueuePolicyArg::LiveOnly => QueuePolicy::LiveOnly,
        QueuePolicyArg::MailboxOnly => QueuePolicy::MailboxOnly,
    }
}

fn approval_arg_to_control(approval: ApprovalArg) -> ApprovalAction {
    match approval {
        ApprovalArg::Accept => ApprovalAction::Accept,
        ApprovalArg::Reject => ApprovalAction::Reject,
    }
}

fn approval_arg_default_message(approval: ApprovalArg) -> &'static str {
    match approval {
        ApprovalArg::Accept => "accept",
        ApprovalArg::Reject => "reject",
    }
}

fn read_send_message_input(
    message: Option<&str>,
    stdin: bool,
    file: Option<&str>,
    approval: Option<ApprovalArg>,
) -> Result<String, CliError> {
    match read_message_input(message, stdin, file) {
        Ok(message) => Ok(message),
        Err(_) if approval.is_some() && message.is_none() && !stdin && file.is_none() => {
            Ok(approval_arg_default_message(approval.unwrap()).to_string())
        }
        Err(error) => Err(error),
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
        "watch_error": ask.watch_error,
        "agent": watch.agent,
        "cursor": watch.cursor,
        "delivery": ask.delivery,
        "output": watch.output,
        "transcript": watch.transcript,
        "events": watch.events,
    });
    serde_json::to_string(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn render_ask_many_response(
    ask: &wardian_core::control::AskManyResponse,
) -> Result<String, CliError> {
    serde_json::to_string(ask)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn parse_include(include: Option<&str>) -> Vec<String> {
    include
        .unwrap_or("status,transcript,output,delivery")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_watch_include(include: Option<&str>, raw: bool) -> Vec<String> {
    let mut values = parse_include(include);
    if raw && !values.iter().any(|value| value == "raw_output") {
        values.push("raw_output".to_string());
    }
    values
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

pub(crate) fn control_error(e: std::io::Error) -> CliError {
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
    } else if e.kind() == std::io::ErrorKind::TimedOut {
        CliError::control_endpoint_timeout(e.to_string())
    } else if is_control_endpoint_unavailable(&e) {
        CliError::app_not_running()
    } else if let Some(endpoint_error) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<live::ControlEndpointError>())
    {
        let exit_code = match endpoint_error.code() {
            "not_found" | "artifact_not_found" | "review_not_found" | "browser_not_found" => {
                ExitCode::NotFound
            }
            "ambiguous" | "browser_ambiguous" | "ref_ambiguous" => ExitCode::Ambiguous,
            "invalid_origin" => ExitCode::NotInSession,
            _ => ExitCode::Generic,
        };
        let mut error =
            CliError::backend(exit_code, endpoint_error.code(), endpoint_error.to_string());
        error.details = endpoint_error.details().cloned().map(Box::new);
        error
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
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeChoice {
    Neighbors,
    Workspace,
    All,
}

fn decide_scope(scope: &str, in_session: bool) -> Result<ScopeChoice, CliError> {
    match scope {
        "auto" => Ok(if in_session {
            ScopeChoice::Neighbors
        } else {
            ScopeChoice::Workspace
        }),
        "neighbors" => Ok(ScopeChoice::Neighbors),
        "workspace" => Ok(ScopeChoice::Workspace),
        "all" => Ok(ScopeChoice::All),
        other => Err(CliError::generic(format!("unknown scope: {other}"))),
    }
}

/// Keep self + neighbors; annotate members with their visibility reasons.
fn filter_to_neighbors(
    agents: Vec<wardian_core::identity::AgentIdentity>,
    self_uuid: &str,
    home: &std::path::Path,
) -> Vec<wardian_core::identity::AgentIdentity> {
    use wardian_core::topology::{load_topology, resolve_neighbors, AgentRef};

    let topology = load_topology(home);
    let refs: Vec<AgentRef> = agents
        .iter()
        .map(|agent| AgentRef {
            uuid: agent.uuid.clone(),
            workspace: agent.workspace.clone(),
        })
        .collect();
    let view = resolve_neighbors(self_uuid, &topology, &refs);
    let reasons: std::collections::HashMap<String, String> = view
        .members
        .iter()
        .map(|member| (member.uuid.clone(), member.reasons.join(",")))
        .collect();

    agents
        .into_iter()
        .filter(|agent| agent.uuid == self_uuid || reasons.contains_key(&agent.uuid))
        .map(|mut agent| {
            agent.visibility = reasons.get(&agent.uuid).cloned();
            agent
        })
        .collect()
}

fn handle_show(target: Option<&str>, args: &AgentArgs) -> Result<String, CliError> {
    let live_agents = match live::list_agents() {
        Ok(agents) => Some(agents),
        Err(error) if is_control_endpoint_unavailable(&error) => None,
        Err(error) => return Err(control_error(error)),
    };
    if let Some(agents) = live_agents {
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
    let session = live::current_session_id();
    let scope_choice = if workspace.is_some() {
        ScopeChoice::All
    } else {
        decide_scope(scope, session.is_some())?
    };
    let mut agents = match live::list_agents() {
        Ok(agents) => agents,
        Err(error) if is_control_endpoint_unavailable(&error) => identity::list_agents(
            &open_db()?,
            &ListFilters {
                scope: Scope::All,
                ..Default::default()
            },
        )
        .map_err(identity_error)?,
        Err(error) => return Err(control_error(error)),
    };
    let caller_workspace = if scope_choice == ScopeChoice::Workspace {
        Some(match session.as_deref() {
            Some(id) => agents
                .iter()
                .find(|agent| agent.uuid == id)
                .ok_or_else(|| CliError::not_found(id))?
                .workspace
                .clone()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| {
                    CliError::generic("agent has no workspace; pass --workspace or --scope all")
                })?,
            None => std::env::current_dir()
                .map_err(|error| CliError::generic(error.to_string()))?
                .to_string_lossy()
                .into_owned(),
        })
    } else {
        None
    };
    // Resolve topology against the whole roster before applying display filters.
    if scope_choice == ScopeChoice::Neighbors {
        let session_id = session.as_deref().ok_or_else(CliError::not_in_session)?;
        if !agents.iter().any(|agent| agent.uuid == session_id) {
            return Err(CliError::not_found(session_id));
        }
        let home = wardian_core::paths::wardian_home()
            .ok_or_else(|| CliError::generic("could not determine Wardian home"))?;
        agents = filter_to_neighbors(agents, session_id, &home);
    }
    if let Some(expected) = workspace.as_deref().or(caller_workspace.as_deref()) {
        // Resolve existing directory identities consistently with agent creation,
        // including Windows casing and junctions. Keep unavailable historical
        // paths comparable without widening scope or changing returned paths.
        let expected_path = Path::new(expected);
        let expected_canonical = expected_path.canonicalize().ok();
        agents.retain(|agent| {
            agent.workspace.as_deref().is_some_and(|actual| {
                let actual_path = Path::new(actual);
                match (expected_canonical.as_ref(), actual_path.canonicalize().ok()) {
                    (Some(expected), Some(actual)) => *expected == actual,
                    _ => actual_path == expected_path,
                }
            })
        });
    }
    let agents = identity::filter_agents(
        agents,
        &ListFilters {
            scope: Scope::All,
            status,
            class: class_name,
            ..Default::default()
        },
    );
    render_list(&agents, &render_options(args))
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

fn render_options(args: &AgentArgs) -> RenderOptions {
    RenderOptions {
        fields: args.fields.as_deref().map(|fields| {
            fields
                .split(',')
                .map(str::trim)
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
    include!("tests/automation_snapshot_tests.rs");

    struct TestWardianHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
        previous_session_id: Option<std::ffi::OsString>,
    }

    impl TestWardianHome {
        fn new(path: &std::path::Path) -> Self {
            let guard = Self {
                _lock: crate::test_env_lock(),
                previous_home: std::env::var_os("WARDIAN_HOME"),
                previous_session_id: std::env::var_os("WARDIAN_SESSION_ID"),
            };
            std::env::set_var("WARDIAN_HOME", path);
            std::env::remove_var("WARDIAN_SESSION_ID");
            guard
        }
    }

    impl Drop for TestWardianHome {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
            match self.previous_session_id.take() {
                Some(value) => std::env::set_var("WARDIAN_SESSION_ID", value),
                None => std::env::remove_var("WARDIAN_SESSION_ID"),
            }
        }
    }

    #[test]
    fn conversation_list_rejects_unknown_scope() {
        let error = handle_conversation(args::ConversationArgs {
            command: args::ConversationCommand::List {
                agent: Some("agent-1".to_string()),
                scope: "workspace".to_string(),
            },
        })
        .unwrap_err();

        assert_eq!(error.code, "generic");
        assert!(error.message.contains("unsupported conversation scope"));
        assert!(error.message.contains("current"));
        assert!(error.message.contains("all"));
    }

    #[test]
    fn conversation_list_current_requires_agent_or_session() {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestWardianHome::new(temp.path());

        let error = handle_conversation(args::ConversationArgs {
            command: args::ConversationCommand::List {
                agent: None,
                scope: "current".to_string(),
            },
        })
        .unwrap_err();

        assert_eq!(error.code, "generic");
        assert!(error.message.contains("--agent"));
        assert!(error.message.contains("--scope all"));
        assert!(error.message.contains("WARDIAN_SESSION_ID"));
    }

    #[test]
    fn conversation_list_agent_accepts_persisted_agent_name() {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestWardianHome::new(temp.path());
        wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: "agent-uuid-1",
            session_name: "AgentOne",
            description: "",
            agent_class: "Coder",
            provider: "codex",
            workspace: Some("<absolute-workspace-path>"),
            project: None,
            is_off: false,
            created_at: Some("2026-06-15T00:00:00.000Z"),
        })
        .unwrap();

        let resolved = resolve_conversation_agent(Some("AgentOne".to_string()), false)
            .expect("resolve agent name");

        assert_eq!(resolved.as_deref(), Some("agent-uuid-1"));
    }

    #[test]
    fn automation_node_types_json_lists_task_type() {
        let out = render_automation_node_types(None, true).unwrap();
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["schema"], 2);
        assert!(json["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "task"));
    }

    #[test]
    fn automation_validate_reports_unknown_node_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.md");
        std::fs::write(
            &path,
            "---\nschema: 2\nid: bad\nname: Bad\nnodes:\n  - id: x\n    type: frobnicate\nedges: []\n---\n",
        )
        .unwrap();
        let error = render_automation_validate(path.to_str().unwrap()).unwrap_err();
        assert_eq!(error.code, "validation_failed");
        let json = error.details.unwrap();
        assert_eq!(json["ok"], false);
        assert!(json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "unknown_node_type"));
    }

    #[test]
    fn automation_exec_real_dispatches_to_live_launcher() {
        let out = render_automation_exec_with_live_launcher(
            "wf.md",
            "real",
            Some(r#"{"target":"HEAD"}"#),
            Some("codex"),
            Some("<absolute-workspace-path>"),
            &["reviewer=codex".to_string()],
            |request| {
                assert_eq!(request.path, "wf.md");
                assert_eq!(request.provider.as_deref(), Some("codex"));
                assert_eq!(
                    request.workspace.as_deref(),
                    Some("<absolute-workspace-path>")
                );
                assert_eq!(request.input, serde_json::json!({ "target": "HEAD" }));
                assert_eq!(
                    request.bindings,
                    HashMap::from([("reviewer".to_string(), "codex".to_string())])
                );
                Ok(AutomationRunResponse::started(
                    "live",
                    "run-1",
                    "autoreview",
                    "<absolute-workspace-path>/logs/automations/autoreview/run-1",
                ))
            },
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(json["ok"], true);
        assert_eq!(json["executor"], "live");
        assert_eq!(json["run_id"], "run-1");
    }

    #[test]
    fn automation_exec_mock_stays_local_and_does_not_call_live_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _env = TestWardianHome::new(home.path());
        let path = dir.path().join("wf.md");
        std::fs::write(
            &path,
            "---\nschema: 2\nid: wf\nname: Automation\nnodes:\n  - id: trigger\n    type: manual_trigger\n    fields: {}\nedges: []\n---\n",
        )
        .unwrap();

        let out = render_automation_exec_with_live_launcher(
            path.to_str().unwrap(),
            "mock",
            None,
            Some("codex"),
            Some("<absolute-workspace-path>"),
            &[],
            |_| panic!("mock executor must not call live launcher"),
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(json["ok"], true);
        assert_eq!(json["executor"], "mock");
        assert_eq!(json["blueprint_id"], "wf");
    }

    #[test]
    fn gen_schema_check_detects_drift() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.json");
        std::fs::write(&path, "{}").unwrap();
        let err =
            render_automation_gen(&path.to_string_lossy(), GenKind::Schema, true).unwrap_err();
        assert_eq!(err.code, "drift");
    }

    #[test]
    fn gen_schema_writes_file_when_not_checking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.json");
        let out = render_automation_gen(&path.to_string_lossy(), GenKind::Schema, false).unwrap();
        assert!(out.contains("wrote"));
        assert!(path.exists());
    }

    #[test]
    fn backend_codes_and_details_survive_without_a_cli_whitelist() {
        for code in [
            "idempotency_conflict",
            "stale_generation",
            "native_submitted_unconfirmed",
            "native_capability_unavailable",
            "future_error",
        ] {
            let details = serde_json::json!({"delivery_id": "delivery-1"});
            let error = control_error(std::io::Error::other(
                live::ControlEndpointError::with_details(
                    code,
                    "requires reconciliation",
                    details.clone(),
                ),
            ));
            assert_eq!(error.code, code);
            assert_eq!(error.details.as_deref(), Some(&details));
            assert_eq!(error.code_i32(), 1);
        }
    }

    #[test]
    fn snapshot_fallback_occurs_only_when_the_endpoint_is_unavailable() {
        for kind in [
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::NotFound,
        ] {
            assert_eq!(
                read_snapshot(Err(std::io::Error::new(kind, "offline")), || Ok(42)).unwrap(),
                (42, "persisted")
            );
        }
        for kind in [
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidData,
        ] {
            let result = read_snapshot::<u8>(Err(std::io::Error::new(kind, "failed")), || {
                panic!("must not hide live errors")
            });
            assert!(result.is_err());
        }
        let rejection = std::io::Error::other(live::ControlEndpointError::new(
            "bad_request",
            "invalid query",
        ));
        let error =
            read_snapshot::<u8>(Err(rejection), || panic!("must not hide rejection")).unwrap_err();
        assert_eq!(error.code, "bad_request");
        assert_eq!(
            read_snapshot(Ok(42), || panic!("live succeeded")).unwrap(),
            (42, "live")
        );
    }

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
    fn control_error_preserves_backend_bad_request_code() {
        let error = std::io::Error::other(live::ControlEndpointError::new(
            "bad_request",
            "workspace must be an absolute directory",
        ));

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "bad_request");
        assert_eq!(cli_error.code_i32(), 1);
        assert!(cli_error.message.contains("absolute directory"));
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
    fn control_error_reports_endpoint_timeout_separately_from_app_not_running() {
        let error = std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Wardian control endpoint timed out",
        );

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "control_endpoint_timeout");
        assert_ne!(cli_error.code_i32(), ExitCode::AppNotRunning as i32);
        assert!(cli_error.message.contains("timed out"));
        assert!(cli_error
            .hint
            .as_deref()
            .is_some_and(|hint| hint.contains("overloaded")));
    }

    #[test]
    fn control_error_still_maps_refused_endpoint_to_app_not_running() {
        let error = std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "could not connect to Wardian control endpoint",
        );

        let cli_error = control_error(error);

        assert_eq!(cli_error.code, "app_not_running");
        assert_eq!(cli_error.code_i32(), ExitCode::AppNotRunning as i32);
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
                queue_policy: wardian_core::control::QueuePolicy::QueueIfBusy,
                message_id: None,
                delivery_phase: None,
                observed_state: None,
                reason: None,
                profile: None,
                error: None,
            }],
            watch_error: None,
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
                transcript: None,
                raw_output: None,
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
            watch_error: None,
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
                transcript: None,
                raw_output: None,
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
    fn render_ask_response_includes_watch_error_when_present() {
        let ask = live::AskAgentResponse {
            request_id: None,
            reply: None,
            delivery: Vec::new(),
            watch_error: Some(wardian_core::control::WatchEvidenceError {
                code: "gap_detected".to_string(),
                message: "watch cursor expired while waiting".to_string(),
            }),
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
                transcript: None,
                raw_output: None,
                delivery: wardian_core::control::WatchDeliverySnapshot {
                    delivery: Vec::new(),
                },
            },
        };

        let rendered = render_ask_response("reviewer-a1", "reply", ask).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(json["watch_error"]["code"], "gap_detected");
        assert_eq!(
            json["watch_error"]["message"],
            "watch cursor expired while waiting"
        );
    }

    #[test]
    fn render_ask_response_includes_transcript_when_watch_has_it() {
        let ask = live::AskAgentResponse {
            request_id: None,
            reply: None,
            delivery: Vec::new(),
            watch_error: None,
            watch: wardian_core::control::AgentWatchResponse {
                schema: 1,
                agent: wardian_core::control::WatchAgentSnapshot {
                    uuid: "agent-1".to_string(),
                    name: "reviewer-a1".to_string(),
                    provider: "gemini".to_string(),
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
                transcript: Some(wardian_core::control::WatchTranscript {
                    cursor: "agent-1:2".to_string(),
                    messages: vec![wardian_core::control::WatchTranscriptMessage {
                        role: "assistant".to_string(),
                        text: "Gemini answer".to_string(),
                        provider: "gemini".to_string(),
                        turn_id: Some("m2".to_string()),
                        source: Some("gemini_log".to_string()),
                    }],
                    latest_text: "Gemini answer".to_string(),
                    truncated: false,
                    omitted_bytes: 0,
                }),
                raw_output: None,
                delivery: wardian_core::control::WatchDeliverySnapshot {
                    delivery: Vec::new(),
                },
            },
        };

        let rendered = render_ask_response("reviewer-a1", "output:Gemini answer", ask).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(json["transcript"]["latest_text"], "Gemini answer");
    }

    #[test]
    fn normalize_ask_condition_keeps_structured_reply_mode() {
        assert_eq!(normalize_ask_condition("reply").unwrap(), "reply");
    }

    #[test]
    fn parse_include_defaults_to_readable_watch_surfaces() {
        assert_eq!(
            parse_include(None),
            vec![
                "status".to_string(),
                "transcript".to_string(),
                "output".to_string(),
                "delivery".to_string()
            ]
        );
    }

    #[test]
    fn parse_include_adds_raw_output_when_raw_flag_is_set() {
        assert_eq!(
            parse_watch_include(Some("output"), true),
            vec!["output".to_string(), "raw_output".to_string()]
        );
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
                description: String::new(),
                class: "Coder".to_string(),
                provider: "codex".to_string(),
                status: "processing".to_string(),
                pid: None,
                started_at: None,
                workspace: Some("D:/repo/worktrees/review".to_string()),
                last_status_at: None,
                status_source: wardian_core::identity::StatusSource::Live,
                visibility: None,
            },
            worktree: Some(wardian_core::control::AgentWorktreeSummary {
                id: "D:/repo/worktrees/review".to_string(),
                name: "review".to_string(),
                source_folder: "D:/repo".to_string(),
                worktree_folder: "D:/repo/worktrees/review".to_string(),
                member_agent_ids: vec!["agent-1".to_string(), "agent-2".to_string()],
                can_delete: false,
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
        assert_eq!(json["worktree"]["can_delete"], false);
        assert_eq!(json["cleared_session"], true);
    }

    #[test]
    fn render_worktree_list_includes_delete_capability() {
        let worktrees = vec![wardian_core::control::AgentWorktreeSummary {
            id: "D:/repo/worktrees/review".to_string(),
            name: "review".to_string(),
            source_folder: "D:/repo".to_string(),
            worktree_folder: "D:/repo/worktrees/review".to_string(),
            member_agent_ids: Vec::new(),
            can_delete: true,
        }];

        let rendered = render_worktree_list(&worktrees).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(json["schema"], 1);
        assert_eq!(json["worktrees"][0]["name"], "review");
        assert_eq!(json["worktrees"][0]["can_delete"], true);
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

    #[test]
    fn effective_default_scope_prefers_neighbors_in_session() {
        assert_eq!(decide_scope("auto", true).unwrap(), ScopeChoice::Neighbors);
        assert_eq!(decide_scope("auto", false).unwrap(), ScopeChoice::Workspace);
        assert_eq!(
            decide_scope("neighbors", false).unwrap(),
            ScopeChoice::Neighbors
        );
        assert_eq!(
            decide_scope("workspace", true).unwrap(),
            ScopeChoice::Workspace
        );
        assert_eq!(decide_scope("all", true).unwrap(), ScopeChoice::All);
        assert!(decide_scope("bogus", true).is_err());
    }

    #[test]
    fn neighbors_filter_returns_neighbors_plus_self() {
        let temp = tempfile::tempdir().unwrap();
        let mut topology = wardian_core::topology::Topology::default();
        topology.add_edge("me", "friend", "2026-07-02T00:00:00Z");
        wardian_core::topology::save_topology(temp.path(), &topology).unwrap();

        let agents = vec![
            wardian_core::identity::AgentIdentity {
                name: "me-agent".to_string(),
                uuid: "me".to_string(),
                description: String::new(),
                class: "Coder".to_string(),
                provider: "claude".to_string(),
                status: "idle".to_string(),
                pid: None,
                started_at: None,
                workspace: None,
                last_status_at: None,
                status_source: wardian_core::identity::StatusSource::Persisted,
                visibility: None,
            },
            wardian_core::identity::AgentIdentity {
                name: "friend-agent".to_string(),
                uuid: "friend".to_string(),
                description: String::new(),
                class: "Architect".to_string(),
                provider: "claude".to_string(),
                status: "idle".to_string(),
                pid: None,
                started_at: None,
                workspace: None,
                last_status_at: None,
                status_source: wardian_core::identity::StatusSource::Persisted,
                visibility: None,
            },
            wardian_core::identity::AgentIdentity {
                name: "stranger-agent".to_string(),
                uuid: "stranger".to_string(),
                description: String::new(),
                class: "Reviewer".to_string(),
                provider: "claude".to_string(),
                status: "idle".to_string(),
                pid: None,
                started_at: None,
                workspace: None,
                last_status_at: None,
                status_source: wardian_core::identity::StatusSource::Persisted,
                visibility: None,
            },
        ];

        let filtered = filter_to_neighbors(agents, "me", temp.path());

        assert_eq!(filtered.len(), 2);
        let me = filtered.iter().find(|a| a.uuid == "me").unwrap();
        assert_eq!(me.visibility, None);
        let friend = filtered.iter().find(|a| a.uuid == "friend").unwrap();
        assert!(friend.visibility.is_some());
        assert_eq!(friend.visibility.as_deref(), Some("manual"));
    }

    #[test]
    fn neighbors_scope_without_session_id_yields_not_in_session_error() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = TestWardianHome::new(temp.path());
        std::env::remove_var("WARDIAN_SESSION_ID");

        // Simulate what handle_list does when scope is Neighbors
        let result: Result<String, CliError> =
            std::env::var("WARDIAN_SESSION_ID").map_err(|_| CliError::not_in_session());

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.exit_code, ExitCode::NotInSession);
    }
}
