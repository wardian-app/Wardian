mod args;
mod disk;
mod errors;
mod live;
mod output;

use std::{
    collections::HashMap,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use args::{
    AgentArgs, AgentCommand, AgentWorktreeCommand, ApprovalArg, AskArgs, Cli, Command,
    QueuePolicyArg, ReplyArgs, ReplyStatusArg, SendArgs, TeamArgs, TeamCommand, WatchlistArgs,
    WatchlistCommand, WorkflowArgs, WorkflowCommand, WorkflowScheduleCommand,
};
use clap::Parser;
use errors::{CliError, ExitCode};
use output::{render_list, render_show, RenderOptions};
use wardian_core::control::{ApprovalAction, MessageInputMode, QueuePolicy};
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
    serde_json::to_string_pretty(&response)
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
// wardian workflow
// ---------------------------------------------------------------------------

fn handle_workflow(args: WorkflowArgs) -> Result<String, CliError> {
    match args.command {
        WorkflowCommand::NodeTypes { json } => render_workflow_node_types(json),
        WorkflowCommand::Validate { path } => render_workflow_validate(&path),
        WorkflowCommand::Exec {
            path,
            executor,
            input,
            bind,
        } => render_workflow_exec(&path, &executor, input.as_deref(), &bind),
        WorkflowCommand::Runs => render_workflow_runs(),
        WorkflowCommand::RunShow {
            blueprint_id,
            run_id,
        } => render_workflow_run_show(&blueprint_id, &run_id),
        WorkflowCommand::Replay {
            blueprint_id,
            run_id,
        } => render_workflow_replay(&blueprint_id, &run_id),
        WorkflowCommand::Parse { path } => render_workflow_parse(&path),
        WorkflowCommand::Normalize { path, write } => render_workflow_normalize(&path, write),
        WorkflowCommand::GenSchema { out, check } => {
            render_workflow_gen(&out, GenKind::Schema, check)
        }
        WorkflowCommand::GenDocs { out, check } => render_workflow_gen(&out, GenKind::Docs, check),
        WorkflowCommand::Schedule(command) => render_workflow_schedule(command),
    }
}

fn render_workflow_node_types(json: bool) -> Result<String, CliError> {
    if json {
        return Ok(format!("{}\n", wardian_core::workflow::ts_schema_json()));
    }
    // Human summary: one line per node type.
    let mut lines = String::from("NODE TYPES\n");
    for def in wardian_core::workflow::node_types() {
        lines.push_str(&format!(
            "  {:<18} {:<8} {}\n",
            def.id,
            format!("{:?}", def.kind).to_lowercase(),
            def.description
        ));
    }
    Ok(lines)
}

fn render_workflow_validate(path: &str) -> Result<String, CliError> {
    let blueprint = wardian_core::workflow::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    let report = wardian_core::workflow::validate(&blueprint);
    let body = serde_json::json!({
        "schema": 1,
        "ok": report.is_valid(),
        "diagnostics": report.diagnostics,
    });
    serde_json::to_string_pretty(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn render_workflow_exec(
    path: &str,
    executor: &str,
    input: Option<&str>,
    bind: &[String],
) -> Result<String, CliError> {
    if executor != "mock" {
        return Err(CliError::backend(
            ExitCode::Generic,
            "unsupported_executor",
            "only 'mock' is available until the real executor lands",
        ));
    }
    let input = parse_workflow_exec_input(input)?;
    let _bindings = parse_workflow_bindings(bind)?;

    let blueprint = wardian_core::workflow::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    let report = wardian_core::workflow::validate(&blueprint);
    if !report.is_valid() {
        let body = serde_json::json!({
            "schema": 1,
            "ok": false,
            "diagnostics": report.diagnostics,
        });
        return serde_json::to_string_pretty(&body)
            .map(|json| format!("{json}\n"))
            .map_err(|e| CliError::generic(e.to_string()));
    }

    let run_id = wardian_core::engine::driver::new_run_id();
    let run_root = wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id)
        .ok_or_else(|| CliError::generic("could not resolve workflow run directory"))?;
    let runtime = build_workflow_runtime()?;
    let mock = wardian_core::engine::MockExecutor::new();
    let state = runtime
        .block_on(wardian_core::engine::Engine::start_with_id(
            &blueprint,
            &run_id,
            input,
            &run_root,
            &mock,
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
    serde_json::to_string_pretty(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn parse_workflow_exec_input(input: Option<&str>) -> Result<serde_json::Value, CliError> {
    let Some(raw) = input else {
        return Ok(serde_json::json!({}));
    };
    let value = serde_json::from_str::<serde_json::Value>(raw)
        .map_err(|error| CliError::generic(format!("invalid --input JSON: {error}")))?;
    if !value.is_object() {
        return Err(CliError::generic("--input must be a JSON object"));
    }
    Ok(value)
}

fn parse_workflow_bindings(bind: &[String]) -> Result<HashMap<String, String>, CliError> {
    let mut bindings = HashMap::new();
    for entry in bind {
        let Some((name, provider)) = entry.split_once('=') else {
            return Err(CliError::generic(format!(
                "invalid --bind `{entry}`; expected name=provider"
            )));
        };
        let name = name.trim();
        let provider = provider.trim();
        if name.is_empty() || provider.is_empty() {
            return Err(CliError::generic(format!(
                "invalid --bind `{entry}`; expected non-empty name=provider"
            )));
        }
        bindings.insert(name.to_string(), provider.to_string());
    }
    Ok(bindings)
}

fn render_workflow_runs() -> Result<String, CliError> {
    let mut runs = Vec::new();
    let Some(runs_root) = wardian_core::paths::workflow_runs_dir() else {
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

fn render_workflow_run_show(blueprint_id: &str, run_id: &str) -> Result<String, CliError> {
    let run_root = workflow_run_root(blueprint_id, run_id)?;
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

fn render_workflow_replay(blueprint_id: &str, run_id: &str) -> Result<String, CliError> {
    let blueprint = find_library_blueprint(blueprint_id)?.ok_or_else(|| {
        CliError::generic(format!(
            "blueprint {blueprint_id} not found in library/workflows"
        ))
    })?;
    let run_root = workflow_run_root(blueprint_id, run_id)?;
    let state = wardian_core::engine::Engine::replay(&blueprint, &run_root)
        .map_err(|e| CliError::generic(e.to_string()))?;
    render_json(serde_json::json!({
        "schema": 1,
        "state": state,
    }))
}

fn render_workflow_parse(path: &str) -> Result<String, CliError> {
    let blueprint = wardian_core::workflow::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    render_json(serde_json::json!({
        "schema": 1,
        "blueprint": blueprint,
    }))
}

fn render_workflow_normalize(path: &str, write: bool) -> Result<String, CliError> {
    let mut blueprint = wardian_core::workflow::parse_file(Path::new(path))
        .map_err(|e| CliError::generic(e.to_string()))?;
    wardian_core::workflow::normalize(&mut blueprint);
    let normalized = wardian_core::workflow::to_string(&blueprint)
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

fn render_workflow_schedule(command: WorkflowScheduleCommand) -> Result<String, CliError> {
    use WorkflowScheduleCommand as C;
    use wardian_core::schedule::{compute_next_run, load_schedules, save_schedules};

    match command {
        C::List => render_json(serde_json::json!({
            "schema": 1,
            "schedules": load_schedules(),
        })),
        C::Add {
            blueprint,
            name,
            every,
            daily,
            weekly,
            at,
            provider,
            input,
            bind,
        } => {
            let schedule = build_schedule_definition(every, daily, weekly, at)?;
            let input = parse_workflow_exec_input(input.as_deref())?;
            let bindings = parse_workflow_bindings(&bind)?;
            let now = current_epoch_ms();
            let record = wardian_core::models::WorkflowSchedule {
                id: wardian_core::engine::driver::new_run_id(),
                blueprint_id: blueprint,
                name,
                provider,
                workspace: None,
                input,
                bindings,
                next_run_epoch_ms: compute_next_run(&schedule, now),
                paused_remaining_ms: None,
                is_paused: false,
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

fn mutate_schedule(
    id: &str,
    mutate: impl FnOnce(&mut wardian_core::models::WorkflowSchedule),
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
    every: Option<u32>,
    daily: Option<String>,
    weekly: Option<String>,
    at: Option<String>,
) -> Result<wardian_core::models::ScheduleDefinition, CliError> {
    let mut definition = wardian_core::models::ScheduleDefinition {
        active: true,
        ..Default::default()
    };
    if let Some(minutes) = every {
        definition.schedule_type = "interval".into();
        definition.interval_minutes = Some(minutes);
    } else if let Some(time) = daily {
        definition.schedule_type = "daily".into();
        definition.time_of_day = Some(time);
    } else if let Some(spec) = weekly {
        let (days, time) = spec.split_once('@').ok_or_else(|| {
            CliError::generic("--weekly expects Days@HH:MM, e.g. Mon,Fri@09:30")
        })?;
        definition.schedule_type = "weekly".into();
        definition.days_of_week = Some(days.split(',').map(|day| day.trim().to_string()).collect());
        definition.time_of_day = Some(time.to_string());
    } else if let Some(when) = at {
        definition.schedule_type = "one_time".into();
        definition.run_at = Some(when);
    } else {
        return Err(CliError::generic(
            "specify one of --every / --daily / --weekly / --at",
        ));
    }
    Ok(definition)
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn render_json(body: serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(&body)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

fn workflow_run_root(blueprint_id: &str, run_id: &str) -> Result<PathBuf, CliError> {
    wardian_core::paths::workflow_run_dir(blueprint_id, run_id)
        .ok_or_else(|| CliError::generic("could not resolve workflow run directory"))
}

fn find_library_blueprint(
    blueprint_id: &str,
) -> Result<Option<wardian_core::workflow::Blueprint>, CliError> {
    let Some(home) = wardian_core::paths::wardian_home() else {
        return Ok(None);
    };
    let root = home.join("library").join("workflows");
    if !root.exists() {
        return Ok(None);
    }
    find_library_blueprint_in_dir(&root, blueprint_id)
}

fn find_library_blueprint_in_dir(
    dir: &Path,
    blueprint_id: &str,
) -> Result<Option<wardian_core::workflow::Blueprint>, CliError> {
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
        let blueprint = wardian_core::workflow::parse_file(&path)
            .map_err(|e| CliError::generic(e.to_string()))?;
        if blueprint.id == blueprint_id {
            return Ok(Some(blueprint));
        }
    }
    Ok(None)
}

fn build_workflow_runtime() -> Result<tokio::runtime::Runtime, CliError> {
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

fn render_workflow_gen(out: &str, kind: GenKind, check: bool) -> Result<String, CliError> {
    let generated = match kind {
        GenKind::Schema => format!("{}\n", wardian_core::workflow::ts_schema_json()),
        GenKind::Docs => wardian_core::workflow::reference_doc(),
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

fn handle_send(args: SendArgs) -> Result<String, CliError> {
    let approval_action = args.approval.map(approval_arg_to_control);
    let message = read_send_message_input(
        args.message.as_deref(),
        args.stdin,
        args.file.as_deref(),
        args.approval,
    )?;
    let queue_policy = queue_policy_arg_to_control(args.queue_policy);
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
            "delivery": response.delivery,
            "cursor": watch.cursor,
        })
    } else {
        let sent = if input_mode == MessageInputMode::Message
            && queue_policy == QueuePolicy::QueueIfBusy
            && approval_action.is_none()
        {
            live::send_message(&args.to, &message, args.thread.as_deref())
        } else {
            live::send_message_with_delivery_options(
                &args.to,
                &message,
                args.thread.as_deref(),
                input_mode,
                queue_policy,
                approval_action,
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
    serde_json::to_string_pretty(&response)
        .map(|json| format!("{json}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
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
    } else if e.kind() == std::io::ErrorKind::TimedOut {
        CliError::control_endpoint_timeout(e.to_string())
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
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
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
    fn workflow_node_types_json_lists_task_type() {
        let out = render_workflow_node_types(true).unwrap();
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["schema"], 2);
        assert!(json["node_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "task"));
    }

    #[test]
    fn workflow_validate_reports_unknown_node_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.md");
        std::fs::write(
            &path,
            "---\nschema: 2\nid: bad\nname: Bad\nnodes:\n  - id: x\n    type: frobnicate\nedges: []\n---\n",
        )
        .unwrap();
        let out = render_workflow_validate(path.to_str().unwrap()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["ok"], false);
        assert!(json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "unknown_node_type"));
    }

    #[test]
    fn gen_schema_check_detects_drift() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.json");
        std::fs::write(&path, "{}").unwrap();
        let err = render_workflow_gen(&path.to_string_lossy(), GenKind::Schema, true).unwrap_err();
        assert_eq!(err.code, "drift");
    }

    #[test]
    fn gen_schema_writes_file_when_not_checking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.json");
        let out = render_workflow_gen(&path.to_string_lossy(), GenKind::Schema, false).unwrap();
        assert!(out.contains("wrote"));
        assert!(path.exists());
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
}
