//! Explicit local-owner grant management; never issues credentials during startup.
use crate::errors::CliError;
use clap::{Args, Subcommand};
use wardian_core::controllers::{ControllerScope, ControllerStore};

#[derive(Debug, Args)]
pub struct ControllerArgs {
    #[command(subcommand)]
    pub command: ControllerCommand,
}
#[derive(Debug, Subcommand)]
pub enum ControllerCommand {
    /// Issue a named capability grant and print its bearer token exactly once.
    Create {
        #[arg(long)]
        name: String,
        /// Exact canonical HTTPS protected resource URI, e.g. https://bridge.example/mcp.
        #[arg(long)]
        resource: String,
        /// Exact selected agent UUID; repeat to select multiple agents.
        #[arg(long = "agent", required = true)]
        agent_ids: Vec<String>,
        /// Explicit permission, e.g. agents:read or tasks:submit; repeat as needed.
        #[arg(long = "scope", required = true)]
        scopes: Vec<ControllerScope>,
        /// Absolute RFC3339 expiry. Omitting it creates a grant valid until revoked.
        #[arg(long)]
        expires_at: Option<String>,
    },
    /// List public grant identities and policy, never credentials or digests.
    List,
    /// Immediately deny future operations with this controller's credential.
    Revoke { id: String },
    /// Verify WARDIAN_CONTROLLER_TOKEN for a resource and emit secret-free policy.
    Verify {
        #[arg(long)]
        resource: String,
    },
}

pub fn handle(args: &ControllerArgs) -> Result<String, CliError> {
    // This is an owner terminal workflow, not a managed-agent capability.
    if !matches!(args.command, ControllerCommand::Verify { .. })
        && std::env::var("WARDIAN_SESSION_ID").is_ok_and(|value| !value.trim().is_empty())
    {
        return Err(CliError::generic("controller grants must be managed from a local owner terminal outside an agent session"));
    }
    let store = ControllerStore::current().map_err(|e| CliError::generic(e.to_string()))?;
    handle_store(args, &store)
}
fn handle_store(args: &ControllerArgs, store: &ControllerStore) -> Result<String, CliError> {
    let value = match &args.command {
        ControllerCommand::Create {
            name,
            resource,
            agent_ids,
            scopes,
            expires_at,
        } => {
            let expiry = expires_at
                .as_deref()
                .map(chrono::DateTime::parse_from_rfc3339)
                .transpose()
                .map_err(|_| CliError::generic("--expires-at must be RFC3339"))?
                .map(|time| time.timestamp());
            let issued = store
                .create(name, resource, agent_ids.clone(), scopes.clone(), expiry)
                .map_err(|e| CliError::generic(e.to_string()))?;
            serde_json::json!({"controller": issued.principal, "token": issued.token})
        }
        ControllerCommand::List => {
            serde_json::json!({"controllers": store.list().map_err(|e| CliError::generic(e.to_string()))?})
        }
        ControllerCommand::Revoke { id } => {
            serde_json::json!({"controller": store.revoke(id).map_err(|e| CliError::generic(e.to_string()))?})
        }
        ControllerCommand::Verify { resource } => {
            let token = std::env::var("WARDIAN_CONTROLLER_TOKEN")
                .map_err(|_| CliError::generic("WARDIAN_CONTROLLER_TOKEN is required"))?;
            serde_json::to_value(
                store
                    .authenticate_token_for_resource(&token, resource)
                    .map_err(|e| CliError::generic(e.to_string()))?,
            )
            .map_err(|e| CliError::generic(e.to_string()))?
        }
    };
    serde_json::to_string_pretty(&value)
        .map(|body| format!("{body}\n"))
        .map_err(|e| CliError::generic(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn explicit_cli_requires_targets_and_known_scopes() {
        assert!(crate::args::Cli::try_parse_from([
            "wardian",
            "controller",
            "create",
            "--name",
            "demo"
        ])
        .is_err());
        assert!(crate::args::Cli::try_parse_from([
            "wardian",
            "controller",
            "create",
            "--name",
            "demo",
            "--resource",
            "https://bridge.example/mcp",
            "--agent",
            "00000000-0000-0000-0000-000000000001",
            "--scope",
            "*"
        ])
        .is_err());
        assert!(crate::args::Cli::try_parse_from([
            "wardian",
            "controller",
            "create",
            "--name",
            "demo",
            "--resource",
            "https://bridge.example/mcp",
            "--agent",
            "00000000-0000-0000-0000-000000000001",
            "--scope",
            "agents:read"
        ])
        .is_ok());
    }
    #[test]
    fn local_fixture_issuance_list_and_revoke() {
        let temp = tempfile::tempdir().unwrap();
        let store = ControllerStore::at(temp.path());
        let args = ControllerArgs {
            command: ControllerCommand::Create {
                name: "test-only".into(),
                resource: "https://bridge.example/mcp".into(),
                agent_ids: vec!["00000000-0000-0000-0000-000000000001".into()],
                scopes: vec![ControllerScope::AgentsRead],
                expires_at: None,
            },
        };
        let issued: serde_json::Value =
            serde_json::from_str(&handle_store(&args, &store).unwrap()).unwrap();
        let token = issued["token"].as_str().unwrap();
        let id = issued["controller"]["id"].as_str().unwrap();
        assert!(store.authenticate_token(token).is_ok());
        let listed = handle_store(
            &ControllerArgs {
                command: ControllerCommand::List,
            },
            &store,
        )
        .unwrap();
        assert!(!listed.contains(token));
        assert!(!listed.contains("token_sha256"));
        handle_store(
            &ControllerArgs {
                command: ControllerCommand::Revoke { id: id.into() },
            },
            &store,
        )
        .unwrap();
        assert!(store.authenticate_token(token).is_err());
    }
    #[test]
    fn verify_reads_environment_and_never_returns_bearer() {
        let _guard = crate::test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let store = ControllerStore::at(temp.path());
        let issued = store
            .create(
                "test-only",
                "https://bridge.example/mcp",
                vec!["00000000-0000-0000-0000-000000000001".into()],
                vec![ControllerScope::AgentsRead],
                None,
            )
            .unwrap();
        let previous = std::env::var_os("WARDIAN_CONTROLLER_TOKEN");
        std::env::set_var("WARDIAN_CONTROLLER_TOKEN", &issued.token);
        let verified = handle_store(
            &ControllerArgs {
                command: ControllerCommand::Verify {
                    resource: "https://bridge.example/mcp".into(),
                },
            },
            &store,
        );
        let rejected = handle_store(
            &ControllerArgs {
                command: ControllerCommand::Verify {
                    resource: "https://other.example/mcp".into(),
                },
            },
            &store,
        );
        match previous {
            Some(value) => std::env::set_var("WARDIAN_CONTROLLER_TOKEN", value),
            None => std::env::remove_var("WARDIAN_CONTROLLER_TOKEN"),
        }
        let verified = verified.unwrap();
        assert!(verified.contains(&issued.principal.id));
        assert!(!verified.contains(&issued.token));
        assert!(!verified.contains("token_sha256"));
        assert!(rejected.is_err());
    }
}
