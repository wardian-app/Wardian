//! Commands that must work before local storage migration.

use super::*;

/// Dispatches static and app-owned commands without touching local storage.
pub(super) fn dispatch(command: &Command) -> Option<i32> {
    if let Command::Mcp { command } = command {
        return Some(wardian_cli::mcp::run(command));
    }
    // Static discovery must work without a home, migrations, or running app.
    if let Command::Schema { path } = command {
        return Some(finish(schema::render(path)));
    }
    if let Command::Browser(args) = command {
        if let Some(help) = browser::target_help(args) {
            return Some(finish(Ok(help)));
        }
    }
    if let Command::Agent(args) = command {
        if let Err(error) = validate_agent_output(args) {
            return Some(finish(Err(error)));
        }
    }
    if let Command::Automation(AutomationArgs {
        command: AutomationCommand::NodeTypes { node, json },
        ..
    }) = command
    {
        return Some(finish(render_automation_node_types(node.as_deref(), *json)));
    }
    if let Command::Message(args) = command {
        return Some(finish(messaging::handle(args)));
    }
    if let Command::Remote(args) = command {
        return Some(finish(remote::handle(args)));
    }
    None
}
