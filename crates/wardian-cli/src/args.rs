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
        let Command::Agent(args) = cli.command else { panic!("expected Agent command") };
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
        let Command::Agent(args) = cli.command else { panic!("expected Agent command") };
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
        let Command::Agent(args) = cli.command else { panic!("expected Agent command") };
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert_eq!(args.field.as_deref(), Some("status"));
        assert!(args.verbose);
        assert!(args.pretty);
    }

    #[test]
    fn parses_agent_kill() {
        let cli = Cli::try_parse_from(["wardian", "agent", "kill", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else { panic!() };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Kill { target }) if target == "coder-a1"
        ));
    }

    #[test]
    fn parses_agent_pause() {
        let cli = Cli::try_parse_from(["wardian", "agent", "pause", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else { panic!() };
        assert!(matches!(args.command, Some(AgentCommand::Pause { .. })));
    }

    #[test]
    fn parses_agent_resume() {
        let cli = Cli::try_parse_from(["wardian", "agent", "resume", "coder-a1"]).unwrap();
        let Command::Agent(args) = cli.command else { panic!() };
        assert!(matches!(args.command, Some(AgentCommand::Resume { .. })));
    }

    #[test]
    fn parses_agent_spawn_with_class() {
        let cli = Cli::try_parse_from([
            "wardian", "agent", "spawn", "--class", "Coder",
            "--name", "coder-b1", "--workspace", "D:/Projects/foo",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command else { panic!() };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Spawn { ref class, ref name, ref workspace })
            if class == "Coder"
                && name.as_deref() == Some("coder-b1")
                && workspace.as_deref() == Some("D:/Projects/foo")
        ));
    }

    #[test]
    fn parses_agent_clone() {
        let cli =
            Cli::try_parse_from(["wardian", "agent", "clone", "coder-a1", "--name", "coder-a2"])
                .unwrap();
        let Command::Agent(args) = cli.command else { panic!() };
        assert!(matches!(
            args.command,
            Some(AgentCommand::Clone { ref target, ref name })
            if target == "coder-a1" && name.as_deref() == Some("coder-a2")
        ));
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
        let Command::Agent(args) = cli.command else { panic!("expected Agent command") };
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert!(args.pretty);
    }
}
