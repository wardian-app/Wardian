use crate::args::{GraphArgs, GraphCommand};
use crate::errors::CliError;

pub fn handle_graph(args: GraphArgs) -> Result<String, CliError> {
    match args.command {
        GraphCommand::Show
        | GraphCommand::Neighbors { .. }
        | GraphCommand::Activity
        | GraphCommand::Link { .. }
        | GraphCommand::Unlink { .. }
        | GraphCommand::Ignore { .. }
        | GraphCommand::Unignore { .. } => {
            Err(CliError::generic("graph command not implemented yet"))
        }
    }
}
