use clap::{Args, Parser, Subcommand};

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
    Send(SendArgs),
    Ask(AskArgs),
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
    List,
    Show { target: String },
    Run { target: String },
    Stop { run_instance_id: String },
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

    /// Wait until the target reaches this status after sending
    #[arg(long = "wait-until")]
    pub wait_until: Option<String>,

    /// Maximum time to wait, e.g. 30s, 10m, or 1000ms
    #[arg(long, default_value = "10m")]
    pub timeout: String,
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

    /// Completion condition: status:<status>, output:<substring>, event:<kind>, delivery:<state>, or a bare status
    #[arg(long, default_value = "status:idle")]
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
    Watch {
        target: String,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        include: Option<String>,
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
            Some(AgentCommand::Watch { ref target, ref since, ref until, ref include, tail, ref timeout, follow })
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
        assert_eq!(args.until.as_deref(), Some("status:idle"));
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
