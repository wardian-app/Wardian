use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "wardian", version, about = "Wardian command-line interface")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Agent(AgentArgs),
    Workflow(WorkflowArgs),
    Team(TeamArgs),
    Watchlist(WatchlistArgs),
    Send(SendArgs),
    Ask(AskArgs),
    Reply(ReplyArgs),
}

// ---------------------------------------------------------------------------
// wardian team / watchlist
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct TeamArgs {
    #[command(subcommand)]
    pub command: TeamCommand,
}

#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    List,
    Show { target: String },
}

#[derive(Debug, Args)]
pub struct WatchlistArgs {
    #[command(subcommand)]
    pub command: WatchlistCommand,
}

#[derive(Debug, Subcommand)]
pub enum WatchlistCommand {
    List,
    Show { target: String },
}

// ---------------------------------------------------------------------------
// wardian workflow
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct WorkflowArgs {
    #[command(subcommand)]
    pub command: WorkflowCommand,

    #[arg(long, global = true)]
    pub pretty: bool,
}

#[derive(Debug, Subcommand)]
pub enum WorkflowCommand {
    /// Print the node type registry (the contract agents author against).
    NodeTypes {
        /// Emit the machine-readable JSON schema instead of a summary table.
        #[arg(long)]
        json: bool,
    },
    /// Validate a blueprint `.md` file and report diagnostics.
    Validate { path: String },
    /// Execute a v2 blueprint headlessly and write a durable run.
    Exec {
        path: String,
        /// Execution backend. Only `mock` is available until the real executor lands.
        #[arg(long, default_value = "mock")]
        executor: String,
        /// JSON object of run input (entry input_schema values).
        #[arg(long)]
        input: Option<String>,
        /// Role/class -> provider binding, repeatable: --bind role=provider
        #[arg(long)]
        bind: Vec<String>,
    },
    /// List v2 runs under <home>/logs/workflows.
    Runs,
    /// Show one v2 run's state + event trace.
    RunShow {
        blueprint_id: String,
        run_id: String,
    },
    /// Replay a v2 run's event log into its final state (no execution).
    Replay {
        blueprint_id: String,
        run_id: String,
    },
    /// Parse a blueprint `.md` and print the structured graph.
    Parse { path: String },
    /// Normalize a blueprint `.md` (print, or --write back in place).
    Normalize {
        path: String,
        #[arg(long)]
        write: bool,
    },
    /// Write the node-type JSON schema artifact for the builder.
    GenSchema {
        #[arg(
            long,
            default_value = "src/features/workflows/nodeRegistry.schema.json"
        )]
        out: String,
        /// Exit non-zero if the file on disk differs (CI drift guard).
        #[arg(long)]
        check: bool,
    },
    /// Write the generated node-type reference doc.
    GenDocs {
        #[arg(long, default_value = "docs/workflows/node-reference-v2.md")]
        out: String,
        #[arg(long)]
        check: bool,
    },
    /// Manage v2 schedules (schedules.json). UI lives in the app; these edit the file.
    #[command(subcommand)]
    Schedule(WorkflowScheduleCommand),
}

#[derive(Debug, Subcommand)]
pub enum WorkflowScheduleCommand {
    /// Add a schedule for a blueprint id (resolves to library/workflows/<id>.md).
    Add {
        #[arg(long)]
        blueprint: String,
        #[arg(long)]
        name: String,
        /// Interval cadence in minutes.
        #[arg(long, conflicts_with_all = ["daily", "weekly", "at"])]
        every: Option<u32>,
        /// Daily at HH:MM local time.
        #[arg(long, conflicts_with_all = ["every", "weekly", "at"])]
        daily: Option<String>,
        /// Weekly comma-separated days and time, e.g. Mon,Wed,Fri@09:30.
        #[arg(long, conflicts_with_all = ["every", "daily", "at"])]
        weekly: Option<String>,
        /// One-time run at RFC3339 / YYYY-MM-DDTHH:MM local time.
        #[arg(long, conflicts_with_all = ["every", "daily", "weekly"])]
        at: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        /// JSON object of run input.
        #[arg(long)]
        input: Option<String>,
        /// Role/class -> provider binding, repeatable: --bind role=provider.
        #[arg(long)]
        bind: Vec<String>,
    },
    List,
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Remove {
        id: String,
    },
    RunNow {
        id: String,
    },
}

// ---------------------------------------------------------------------------
// wardian send
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct SendArgs {
    /// Message text (omit when using --stdin or --file)
    pub message: Option<String>,

    /// Target: agent name, UUID, "class:<ClassName>", or "all"
    #[arg(long)]
    pub to: String,

    /// Read message from stdin
    #[arg(long, conflicts_with = "message")]
    pub stdin: bool,

    /// Read message from a file
    #[arg(long, conflicts_with_all = ["message", "stdin"])]
    pub file: Option<String>,

    /// Thread name for grouped conversations
    #[arg(long)]
    pub thread: Option<String>,

    /// Send the message body as a provider slash command without sender attribution
    #[arg(long = "as-command")]
    pub as_command: bool,

    /// Queue policy to use when the target is not safe for live delivery
    #[arg(long = "queue-policy", value_enum, default_value = "queue-if-busy")]
    pub queue_policy: QueuePolicyArg,

    /// Send an explicit approval action instead of a normal message
    #[arg(long, value_enum, conflicts_with = "as_command")]
    pub approval: Option<ApprovalArg>,

    /// Wait until the target reaches this status after sending
    #[arg(long = "wait-until")]
    pub wait_until: Option<String>,

    /// Maximum time to wait, e.g. 30s, 10m, or 1000ms
    #[arg(long, default_value = "10m")]
    pub timeout: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QueuePolicyArg {
    QueueIfBusy,
    LiveOnly,
    MailboxOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ApprovalArg {
    Accept,
    Reject,
}

// ---------------------------------------------------------------------------
// wardian ask
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct AskArgs {
    /// Target agent name or UUID. Broadcast and class targets are not supported.
    pub target: String,

    /// Message text (omit when using --stdin or --file)
    pub message: Option<String>,

    /// Read message from stdin
    #[arg(long, conflicts_with = "message")]
    pub stdin: bool,

    /// Read message from a file
    #[arg(long, conflicts_with_all = ["message", "stdin"])]
    pub file: Option<String>,

    /// Completion condition: reply, status:<status>, output:<substring>, event:<kind>, delivery:<state>, or a bare status
    #[arg(long, default_value = "reply")]
    pub until: Option<String>,

    /// Maximum time to wait, e.g. 30s, 10m, or 1000ms
    #[arg(long, default_value = "10m")]
    pub timeout: String,

    /// Maximum output bytes to return from the response snapshot
    #[arg(long, default_value_t = 65536)]
    pub tail: usize,

    /// Thread name for grouped conversations
    #[arg(long)]
    pub thread: Option<String>,
}

// ---------------------------------------------------------------------------
// wardian reply
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ReplyArgs {
    /// Structured ask request id.
    pub request_id: String,

    /// Reply status.
    #[arg(long, value_enum)]
    pub status: ReplyStatusArg,

    /// Reply body text (omit when using --stdin or --file)
    pub message: Option<String>,

    /// Read reply body from stdin
    #[arg(long, conflicts_with = "message")]
    pub stdin: bool,

    /// Read reply body from a file
    #[arg(long, conflicts_with_all = ["message", "stdin"])]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ReplyStatusArg {
    Done,
    Blocked,
    Failed,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    pub target: Option<String>,

    #[command(subcommand)]
    pub command: Option<AgentCommand>,

    #[arg(long, global = true)]
    pub fields: Option<String>,

    #[arg(long, global = true)]
    pub field: Option<String>,

    #[arg(long, global = true)]
    pub verbose: bool,

    #[arg(long, global = true)]
    pub pretty: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    Show {
        target: Option<String>,
    },
    List {
        #[arg(long, default_value = "workspace")]
        scope: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long = "class")]
        class_name: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
    },
    Kill {
        target: String,
    },
    Pause {
        target: String,
    },
    Resume {
        target: String,
    },
    Spawn {
        #[arg(long)]
        provider: String,
        #[arg(long = "class")]
        class: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
    },
    Clone {
        target: String,
        #[arg(long)]
        name: Option<String>,
    },
    Worktree {
        #[command(subcommand)]
        command: AgentWorktreeCommand,
    },
    Watch {
        target: String,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        include: Option<String>,
        #[arg(long)]
        raw: bool,
        #[arg(long = "tail")]
        tail: Option<usize>,
        #[arg(long, default_value = "10m")]
        timeout: String,
        #[arg(long)]
        follow: bool,
    },
    Wait {
        target: String,
        #[arg(long)]
        until: String,
        #[arg(long, default_value = "10m")]
        timeout: String,
        #[arg(long)]
        next: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentWorktreeCommand {
    List,
    Enable {
        target: String,
        #[arg(long)]
        name: Option<String>,
    },
    Join {
        target: String,
        #[arg(long)]
        worktree: String,
    },
    Disable {
        target: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_agent_target_shorthand() {
        let cli = Cli::try_parse_from(["wardian", "agent", "coder-a1"]).unwrap();
        assert!(matches!(cli.command, Command::Agent(_)));
    }

    #[test]
    fn parses_workflow_node_types_json() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "node-types", "--json"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::NodeTypes { json: true }
        ));
    }

    #[test]
    fn parses_workflow_validate_path() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "validate", "wf.md"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Validate { ref path } if path == "wf.md"
        ));
    }

    #[test]
    fn parses_workflow_exec_path_with_default_executor() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "exec", "wf.md"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Exec { ref path, ref executor, .. }
                if path == "wf.md" && executor == "mock"
        ));
    }

    #[test]
    fn parses_workflow_exec_executor() {
        let cli =
            Cli::try_parse_from(["wardian", "workflow", "exec", "wf.md", "--executor", "real"])
                .unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Exec { ref path, ref executor, .. }
                if path == "wf.md" && executor == "real"
        ));
    }

    #[test]
    fn parses_workflow_exec_with_input_and_bind() {
        let cli = Cli::try_parse_from([
            "wardian",
            "workflow",
            "exec",
            "wf.md",
            "--input",
            "{\"x\":1}",
            "--bind",
            "role=claude",
        ])
        .unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Exec { ref input, ref bind, .. }
                if input.as_deref() == Some("{\"x\":1}") && bind == &vec!["role=claude".to_string()]
        ));
    }

    #[test]
    fn parses_workflow_runs() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "runs"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(args.command, WorkflowCommand::Runs));
    }

    #[test]
    fn parses_schedule_add() {
        let cli = Cli::try_parse_from([
            "wardian",
            "workflow",
            "schedule",
            "add",
            "--blueprint",
            "heartbeat",
            "--name",
            "HB",
            "--every",
            "60",
        ])
        .unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Schedule(WorkflowScheduleCommand::Add { .. })
        ));
    }

    #[test]
    fn parses_workflow_run_show() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "run-show", "wf", "r1"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::RunShow { ref blueprint_id, ref run_id }
                if blueprint_id == "wf" && run_id == "r1"
        ));
    }

    #[test]
    fn parses_workflow_replay() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "replay", "wf", "r1"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Replay { ref blueprint_id, ref run_id }
                if blueprint_id == "wf" && run_id == "r1"
        ));
    }

    #[test]
    fn parses_workflow_parse() {
        let cli = Cli::try_parse_from(["wardian", "workflow", "parse", "wf.md"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Parse { ref path } if path == "wf.md"
        ));
    }

    #[test]
    fn parses_workflow_normalize_write() {
        let cli =
            Cli::try_parse_from(["wardian", "workflow", "normalize", "wf.md", "--write"]).unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected Workflow")
        };
        assert!(matches!(
            args.command,
            WorkflowCommand::Normalize { ref path, write: true } if path == "wf.md"
        ));
    }

    #[test]
    fn parses_send_as_command() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "--to",
            "Wardian-Codex",
            "--as-command",
            "/goal test",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };

        assert!(args.as_command);
        assert_eq!(args.to, "Wardian-Codex");
        assert_eq!(args.message.as_deref(), Some("/goal test"));
    }

    #[test]
    fn parses_send_queue_policy() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "hello",
            "--to",
            "agent-1",
            "--queue-policy",
            "live-only",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };

        assert_eq!(args.queue_policy, QueuePolicyArg::LiveOnly);
    }

    #[test]
    fn parses_send_approval_action() {
        let cli =
            Cli::try_parse_from(["wardian", "send", "--approval", "accept", "--to", "agent-1"])
                .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected Send command")
        };

        assert_eq!(args.approval, Some(ApprovalArg::Accept));
        assert_eq!(args.message, None);
    }

    #[test]
    fn send_approval_conflicts_with_as_command() {
        let err = Cli::try_parse_from([
            "wardian",
            "send",
            "--approval",
            "accept",
            "--to",
            "agent-1",
            "--as-command",
            "/status",
        ])
        .unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_agent_show_explicit_target() {
        let cli = Cli::try_parse_from(["wardian", "agent", "show", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Show { target }) if target.as_deref() == Some("coder-a1")
        ));
    }

    #[test]
    fn parses_agent_list_filters() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "list",
            "--scope",
            "all",
            "--status",
            "idle",
            "--class",
            "Coder",
            "--workspace",
            "D:/Development/Wardian",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::List {
                scope,
                status,
                class_name,
                workspace
            }) if scope == "all"
                && status.as_deref() == Some("idle")
                && class_name.as_deref() == Some("Coder")
                && workspace.as_deref() == Some("D:/Development/Wardian")
        ));
    }

    #[test]
    fn parses_output_modifiers() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "coder-a1",
            "--fields",
            "name,status",
            "--field",
            "status",
            "--verbose",
            "--pretty",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert_eq!(args.field.as_deref(), Some("status"));
        assert!(args.verbose);
        assert!(args.pretty);
    }

    #[test]
    fn parses_agent_kill() {
        let cli = Cli::try_parse_from(["wardian", "agent", "kill", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Kill { target }) if target == "coder-a1"
        ));
    }

    #[test]
    fn parses_agent_pause() {
        let cli = Cli::try_parse_from(["wardian", "agent", "pause", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, Some(AgentCommand::Pause { .. })));
    }

    #[test]
    fn parses_agent_resume() {
        let cli = Cli::try_parse_from(["wardian", "agent", "resume", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.command, Some(AgentCommand::Resume { .. })));
    }

    #[test]
    fn parses_agent_spawn_with_class() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "spawn",
            "--provider",
            "codex",
            "--class",
            "Coder",
            "--name",
            "coder-b1",
            "--workspace",
            "D:/Projects/foo",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Spawn { ref provider, ref class, ref name, ref workspace })
            if provider == "codex"
                && class == "Coder"
                && name.as_deref() == Some("coder-b1")
                && workspace.as_deref() == Some("D:/Projects/foo")
        ));
    }

    #[test]
    fn parses_agent_clone() {
        let cli = Cli::try_parse_from([
            "wardian", "agent", "clone", "coder-a1", "--name", "coder-a2",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Clone { ref target, ref name })
            if target == "coder-a1" && name.as_deref() == Some("coder-a2")
        ));
    }

    #[test]
    fn parses_agent_wait_until_status() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "wait",
            "reviewer-a1",
            "--until",
            "idle",
            "--timeout",
            "30s",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!()
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Wait { ref target, ref until, ref timeout, next: false })
            if target == "reviewer-a1" && until == "idle" && timeout == "30s"
        ));
    }

    #[test]
    fn parses_agent_watch_options() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "watch",
            "Wardian-Codex",
            "--since",
            "agent-1:0001",
            "--until",
            "output:OK",
            "--include",
            "status,output",
            "--tail",
            "4096",
            "--timeout",
            "30s",
        ])
        .unwrap();

        let Command::Agent(args) = cli.command else {
            panic!("agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Watch { ref target, ref since, ref until, ref include, raw: false, tail, ref timeout, follow })
                if target == "Wardian-Codex"
                    && since.as_deref() == Some("agent-1:0001")
                    && until.as_deref() == Some("output:OK")
                    && include.as_deref() == Some("status,output")
                    && tail == Some(4096)
                    && timeout == "30s"
                    && !follow
        ));
    }

    #[test]
    fn parses_agent_watch_readable_and_raw_options() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "watch",
            "Wardian-Codex",
            "--include",
            "transcript,output,raw_output",
            "--raw",
        ])
        .unwrap();

        let Command::Agent(args) = cli.command else {
            panic!("agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Watch { ref include, raw: true, .. })
                if include.as_deref() == Some("transcript,output,raw_output")
        ));
    }

    #[test]
    fn parses_agent_wait_next() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "wait",
            "Wardian-Codex",
            "--until",
            "idle",
            "--next",
        ])
        .unwrap();

        let Command::Agent(args) = cli.command else {
            panic!("agent")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Wait { next: true, .. })
        ));
    }

    #[test]
    fn parses_agent_worktree_list() {
        let cli = Cli::try_parse_from(["wardian", "agent", "worktree", "list"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::List
            })
        ));
    }

    #[test]
    fn parses_agent_worktree_enable_with_name() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "worktree",
            "enable",
            "coder-a1",
            "--name",
            "review fixes",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::Enable { ref target, ref name }
            }) if target == "coder-a1" && name.as_deref() == Some("review fixes")
        ));
    }

    #[test]
    fn parses_agent_worktree_join() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "worktree",
            "join",
            "coder-a1",
            "--worktree",
            "D:/Development/Wardian/.worktrees/review",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::Join { ref target, ref worktree }
            }) if target == "coder-a1" && worktree == "D:/Development/Wardian/.worktrees/review"
        ));
    }

    #[test]
    fn parses_agent_worktree_disable() {
        let cli =
            Cli::try_parse_from(["wardian", "agent", "worktree", "disable", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Worktree {
                command: AgentWorktreeCommand::Disable { ref target }
            }) if target == "coder-a1"
        ));
    }

    #[test]
    fn parses_team_list_and_show() {
        let cli = Cli::try_parse_from(["wardian", "team", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Team(TeamArgs {
                command: TeamCommand::List
            })
        ));

        let cli = Cli::try_parse_from(["wardian", "team", "show", "review"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Team(TeamArgs {
                command: TeamCommand::Show { ref target }
            }) if target == "review"
        ));
    }

    #[test]
    fn parses_watchlist_list_and_show() {
        let cli = Cli::try_parse_from(["wardian", "watchlist", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Watchlist(WatchlistArgs {
                command: WatchlistCommand::List
            })
        ));

        let cli = Cli::try_parse_from(["wardian", "watchlist", "show", "main"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Watchlist(WatchlistArgs {
                command: WatchlistCommand::Show { ref target }
            }) if target == "main"
        ));
    }

    #[test]
    fn parses_send_wait_until_status() {
        let cli = Cli::try_parse_from([
            "wardian",
            "send",
            "review this",
            "--to",
            "reviewer-a1",
            "--wait-until",
            "idle",
            "--timeout",
            "10m",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.message.as_deref(), Some("review this"));
        assert_eq!(args.wait_until.as_deref(), Some("idle"));
        assert_eq!(args.timeout, "10m");
    }

    #[test]
    fn parses_ask_with_inline_message_and_defaults() {
        let cli = Cli::try_parse_from(["wardian", "ask", "reviewer-a1", "review this"]).unwrap();
        let Command::Ask(args) = cli.command else {
            panic!("expected Ask command")
        };
        assert_eq!(args.target, "reviewer-a1");
        assert_eq!(args.message.as_deref(), Some("review this"));
        assert!(!args.stdin);
        assert_eq!(args.file, None);
        assert_eq!(args.until.as_deref(), Some("reply"));
        assert_eq!(args.timeout, "10m");
        assert_eq!(args.tail, 65536);
    }

    #[test]
    fn parses_ask_with_output_condition_and_stdin() {
        let cli = Cli::try_parse_from([
            "wardian",
            "ask",
            "reviewer-a1",
            "--stdin",
            "--until",
            "output:REVIEW_DONE",
            "--tail",
            "131072",
            "--timeout",
            "30s",
        ])
        .unwrap();
        let Command::Ask(args) = cli.command else {
            panic!("expected Ask command")
        };
        assert_eq!(args.target, "reviewer-a1");
        assert!(args.stdin);
        assert_eq!(args.until.as_deref(), Some("output:REVIEW_DONE"));
        assert_eq!(args.tail, 131072);
        assert_eq!(args.timeout, "30s");
    }

    #[test]
    fn parses_reply_with_done_status_and_stdin() {
        let cli = Cli::try_parse_from([
            "wardian",
            "reply",
            "ask_0123456789abcdef",
            "--status",
            "done",
            "--stdin",
        ])
        .unwrap();
        let Command::Reply(args) = cli.command else {
            panic!("expected Reply command")
        };
        assert_eq!(args.request_id, "ask_0123456789abcdef");
        assert_eq!(args.status, ReplyStatusArg::Done);
        assert!(args.stdin);
    }

    #[test]
    fn reply_rejects_unknown_status() {
        let err = Cli::try_parse_from([
            "wardian",
            "reply",
            "ask_0123456789abcdef",
            "--status",
            "waiting",
            "--stdin",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn ask_rejects_stdin_and_file_together() {
        let err = Cli::try_parse_from([
            "wardian",
            "ask",
            "reviewer-a1",
            "--stdin",
            "--file",
            "prompt.md",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_output_modifiers_after_list_subcommand() {
        let cli = Cli::try_parse_from([
            "wardian",
            "agent",
            "list",
            "--scope",
            "all",
            "--fields",
            "name,status",
            "--pretty",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else {
            panic!("expected Agent command")
        };
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert!(args.pretty);
    }
}
