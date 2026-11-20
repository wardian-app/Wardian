//! Self-contained GUI-subsystem entry point for the maintained stdio MCP server.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

use clap::{Parser, Subcommand};
use wardian_cli::mcp::{self, McpCommand};

#[derive(Debug, Parser)]
#[command(name = "wardian-mcp", version, about = "Wardian MCP server")]
struct Cli {
    #[command(subcommand)]
    command: RootCommand,
}

#[derive(Debug, Subcommand)]
enum RootCommand {
    /// Serve Wardian's explicit MCP tools over stdio.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    match Cli::try_parse() {
        Ok(Cli {
            command: RootCommand::Mcp { command },
        }) => mcp::run(&command),
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            code
        }
    }
}
