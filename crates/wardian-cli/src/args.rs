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
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    pub target: Option<String>,

    #[command(subcommand)]
    pub command: Option<AgentCommand>,

    #[arg(long)]
    pub fields: Option<String>,

    #[arg(long)]
    pub field: Option<String>,

    #[arg(long)]
    pub verbose: bool,

    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    Show {
        target: Option<String>,
    },
    List {
        #[arg(long, default_value = "project")]
        scope: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long = "class")]
        class_name: Option<String>,
        #[arg(long)]
        project: Option<String>,
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
        let Command::Agent(args) = cli.command;
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
            "--project",
            "Wardian",
        ])
        .unwrap();
        let Command::Agent(args) = cli.command;
        assert!(matches!(
            args.command,
            Some(AgentCommand::List {
                scope,
                status,
                class_name,
                project
            }) if scope == "all"
                && status.as_deref() == Some("idle")
                && class_name.as_deref() == Some("Coder")
                && project.as_deref() == Some("Wardian")
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
        let Command::Agent(args) = cli.command;
        assert_eq!(args.fields.as_deref(), Some("name,status"));
        assert_eq!(args.field.as_deref(), Some("status"));
        assert!(args.verbose);
        assert!(args.pretty);
    }
}
