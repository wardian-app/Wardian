use crate::args::{MemoryArgs, MemoryCommand, MemoryKindArg, MemoryScopeArg};
use crate::errors::{CliError, ExitCode};
use wardian_core::identity::{self, AgentIdentity, ListFilters, Scope};
use wardian_core::memory::{
    MemoryActor, MemoryKind, MemorySource, MemoryStore, SaveMemoryRequest, UpdateMemoryRequest,
};

pub fn handle_memory(args: MemoryArgs) -> Result<String, CliError> {
    let store = MemoryStore::from_default_home().map_err(memory_error)?;
    let actor = memory_actor(&store)?;
    let value = match args.command {
        MemoryCommand::Save {
            text,
            evidence,
            kind,
            scope,
            agent,
            workspace,
            source,
            idempotency_key,
        } => {
            let context = resolve_context(&actor, agent, workspace)?;
            let workspace = match scope {
                MemoryScopeArg::Agent => None,
                MemoryScopeArg::Workspace => Some(context.workspace.ok_or_else(|| {
                    CliError::generic(
                        "workspace scope requires --workspace or a persisted workspace for the agent",
                    )
                })?),
            };
            let record = store
                .save(
                    &actor,
                    SaveMemoryRequest {
                        agent_id: context.agent_id,
                        workspace,
                        kind: match kind {
                            MemoryKindArg::Stable => MemoryKind::Stable,
                            MemoryKindArg::Current => MemoryKind::Current,
                        },
                        text,
                        evidence_excerpt: evidence,
                        sources: source.into_iter().map(source_from_locator).collect(),
                        idempotency_key,
                    },
                )
                .map_err(memory_error)?;
            serde_json::json!({ "schema": 1, "memory": record })
        }
        MemoryCommand::List { agent, workspace } => {
            let context = resolve_context(&actor, agent, workspace)?;
            let records = store
                .list_active(&actor, &context.agent_id, context.workspace.as_deref())
                .map_err(memory_error)?;
            serde_json::json!({ "schema": 1, "memories": records })
        }
        MemoryCommand::Show { memory_id } => {
            let record = store
                .get(&actor, &memory_id)
                .map_err(|error| actor_memory_error(&actor, error))?;
            serde_json::json!({ "schema": 1, "memory": record })
        }
        MemoryCommand::Update {
            memory_id,
            text,
            evidence,
            source,
            idempotency_key,
        } => {
            let record = store
                .update(
                    &actor,
                    UpdateMemoryRequest {
                        memory_id,
                        text,
                        evidence_excerpt: evidence,
                        sources: source.into_iter().map(source_from_locator).collect(),
                        idempotency_key,
                    },
                )
                .map_err(|error| actor_memory_error(&actor, error))?;
            serde_json::json!({ "schema": 1, "memory": record })
        }
        MemoryCommand::Remove { memory_id } => {
            let record = store
                .remove(&actor, &memory_id)
                .map_err(|error| actor_memory_error(&actor, error))?;
            serde_json::json!({ "schema": 1, "memory": record })
        }
        MemoryCommand::Recall { agent, workspace } => {
            let context = resolve_context(&actor, agent, workspace)?;
            let recall = store
                .recall(&actor, &context.agent_id, context.workspace.as_deref())
                .map_err(memory_error)?;
            serde_json::to_value(recall).map_err(|error| CliError::generic(error.to_string()))?
        }
        MemoryCommand::History { memory_id } => {
            let history = store
                .history(&actor, &memory_id)
                .map_err(|error| actor_memory_error(&actor, error))?;
            serde_json::json!({ "schema": 1, "history": history })
        }
    };

    serde_json::to_string(&value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

struct MemoryContext {
    agent_id: String,
    workspace: Option<String>,
}

fn resolve_context(
    actor: &MemoryActor,
    requested_agent: Option<String>,
    requested_workspace: Option<String>,
) -> Result<MemoryContext, CliError> {
    let caller = match actor {
        MemoryActor::Agent(agent_id) => Some(agent_id.clone()),
        MemoryActor::Operator => None,
    };
    let target = requested_agent
        .or_else(|| caller.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliError::generic("--agent or WARDIAN_SESSION_ID is required"))?;

    let agents = agent_snapshot()?;
    let resolved = resolve_agent(&agents, &target)?;
    if let Some(caller) = caller {
        let caller = resolve_agent(&agents, &caller)?;
        if caller.uuid != resolved.uuid {
            return Err(memory_access_denied(&caller.uuid, &resolved.uuid));
        }
    }
    let agent_id = resolved.uuid.clone();
    let workspace = requested_workspace.or_else(|| resolved.workspace.clone());
    Ok(MemoryContext {
        agent_id,
        workspace,
    })
}

fn memory_actor(store: &MemoryStore) -> Result<MemoryActor, CliError> {
    managed_caller(store).map(MemoryActor::agent)
}

fn managed_caller(store: &MemoryStore) -> Result<String, CliError> {
    let agent_id = std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(memory_identity_required)?;
    let token = std::env::var(wardian_core::memory::MEMORY_CAPABILITY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_memory_capability(&agent_id))?;
    if !store
        .validate_capability(&agent_id, &token)
        .map_err(memory_error)?
    {
        return Err(invalid_memory_capability(&agent_id));
    }
    Ok(agent_id)
}

/// Full roster with the persisted database as the offline authority.
fn agent_snapshot() -> Result<Vec<AgentIdentity>, CliError> {
    let pending =
        wardian_core::agent_replacement::pending_replacement_status().map_err(|error| {
            CliError::generic(format!("failed to inspect agent replacement: {error}"))
        })?;
    match pending {
        wardian_core::agent_replacement::PendingReplacementStatus::Busy => {
            return Err(CliError::generic(
                "agent replacement is still in progress; retry when the desktop operation finishes",
            ));
        }
        wardian_core::agent_replacement::PendingReplacementStatus::Pending(agent_ids) => {
            if crate::live::list_agents().is_ok() {
                return Err(CliError::generic(format!(
                    "agent replacement recovery is pending for {}; restart Wardian before reading memory",
                    agent_ids.join(", ")
                )));
            }
            let recovered = wardian_core::agent_replacement::recover_pending_replacements(false)
                .map_err(|error| {
                    CliError::generic(format!("failed to recover agent replacement: {error}"))
                })?;
            if recovered == wardian_core::agent_replacement::RecoveryStatus::Busy {
                return Err(CliError::generic(
                    "agent replacement started during recovery; retry when it finishes",
                ));
            }
        }
        wardian_core::agent_replacement::PendingReplacementStatus::None => {
            if let Ok(agents) = crate::live::list_agents() {
                return Ok(agents);
            }
        }
    }
    let connection = crate::open_db()?;
    identity::list_agents(
        &connection,
        &ListFilters {
            scope: Scope::All,
            caller_workspace: None,
            status: None,
            class: None,
            workspace: None,
        },
    )
    .map_err(crate::identity_error)
}

fn resolve_agent<'a>(
    agents: &'a [AgentIdentity],
    target: &str,
) -> Result<&'a AgentIdentity, CliError> {
    if let Some(agent) = agents.iter().find(|agent| agent.uuid == target) {
        return Ok(agent);
    }
    let matches = agents
        .iter()
        .filter(|agent| agent.name == target)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(CliError::not_found(target)),
        [agent] => Ok(agent),
        _ => Err(CliError::backend(
            ExitCode::Ambiguous,
            "ambiguous_target",
            format!("Multiple agents are named {target}; pass a UUID instead"),
        )),
    }
}

fn invalid_memory_capability(agent_id: &str) -> CliError {
    CliError::backend_with_details(
        ExitCode::Generic,
        "invalid_memory_capability",
        "managed memory commands require the capability issued to this provider process",
        serde_json::json!({ "claimed_agent_id": agent_id }),
    )
}

fn memory_identity_required() -> CliError {
    CliError::backend(
        ExitCode::Generic,
        "memory_identity_required",
        "wardian memory commands require a managed agent identity and capability",
    )
}

fn memory_access_denied(caller: &str, target: &str) -> CliError {
    CliError::backend_with_details(
        ExitCode::Generic,
        "memory_access_denied",
        "managed agents may access only their own memory",
        serde_json::json!({ "caller_agent_id": caller, "target_agent_id": target }),
    )
}

fn memory_id_ambiguous(prefix: &str) -> CliError {
    CliError::backend(
        ExitCode::Ambiguous,
        "memory_id_ambiguous",
        format!("memory id prefix {prefix} matches multiple memories; use a longer prefix"),
    )
}

fn actor_memory_error(actor: &MemoryActor, error: wardian_core::memory::MemoryError) -> CliError {
    if let wardian_core::memory::MemoryError::MemoryIdAmbiguous(prefix) = &error {
        return memory_id_ambiguous(prefix);
    }
    if let MemoryActor::Agent(agent_id) = actor {
        if matches!(
            error,
            wardian_core::memory::MemoryError::NotFound(_)
                | wardian_core::memory::MemoryError::AccessDenied { .. }
        ) {
            return memory_access_denied(agent_id, "redacted");
        }
    }
    memory_error(error)
}

fn source_from_locator(locator: String) -> MemorySource {
    let source_type = locator
        .split_once(':')
        .map(|(source_type, _)| source_type)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("manual")
        .to_string();
    MemorySource {
        source_type,
        locator: Some(locator),
        source_hash: None,
        primary: false,
    }
}

fn memory_error(error: impl std::fmt::Display) -> CliError {
    CliError::generic(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::db::AgentUpsert;

    struct TestHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _home: tempfile::TempDir,
        previous_home: Option<std::ffi::OsString>,
        previous_session: Option<std::ffi::OsString>,
        previous_capability: Option<std::ffi::OsString>,
    }

    impl TestHome {
        fn new() -> Self {
            let guard = crate::test_env_lock();
            let home = tempfile::tempdir().expect("temp home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            let previous_session = std::env::var_os("WARDIAN_SESSION_ID");
            let previous_capability = std::env::var_os(wardian_core::memory::MEMORY_CAPABILITY_ENV);
            unsafe {
                std::env::set_var("WARDIAN_HOME", home.path());
                std::env::remove_var("WARDIAN_SESSION_ID");
                std::env::remove_var(wardian_core::memory::MEMORY_CAPABILITY_ENV);
            }
            let connection = rusqlite::Connection::open(home.path().join("state.db")).unwrap();
            wardian_core::db::run_migrations(&connection).unwrap();
            for (id, name, workspace) in [
                ("agent-a", "Alpha", "C:/work/alpha"),
                ("agent-b", "Beta", "C:/work/beta"),
            ] {
                wardian_core::db::upsert_agent_with_conn(
                    &connection,
                    &AgentUpsert {
                        session_id: id,
                        session_name: name,
                        description: "",
                        agent_class: "Coder",
                        provider: "codex",
                        workspace: Some(workspace),
                        project: None,
                        is_off: true,
                        created_at: None,
                    },
                )
                .unwrap();
            }
            Self {
                _guard: guard,
                _home: home,
                previous_home,
                previous_session,
                previous_capability,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            unsafe {
                match self.previous_home.take() {
                    Some(value) => std::env::set_var("WARDIAN_HOME", value),
                    None => std::env::remove_var("WARDIAN_HOME"),
                }
                match self.previous_session.take() {
                    Some(value) => std::env::set_var("WARDIAN_SESSION_ID", value),
                    None => std::env::remove_var("WARDIAN_SESSION_ID"),
                }
                match self.previous_capability.take() {
                    Some(value) => {
                        std::env::set_var(wardian_core::memory::MEMORY_CAPABILITY_ENV, value)
                    }
                    None => std::env::remove_var(wardian_core::memory::MEMORY_CAPABILITY_ENV),
                }
            }
        }
    }

    fn seed(agent_id: &str, workspace: &str, text: &str) -> wardian_core::memory::MemoryRecord {
        MemoryStore::from_default_home()
            .unwrap()
            .save(
                &MemoryActor::Operator,
                SaveMemoryRequest {
                    agent_id: agent_id.into(),
                    workspace: Some(workspace.into()),
                    kind: MemoryKind::Stable,
                    text: text.into(),
                    evidence_excerpt: "durable test evidence".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap()
    }

    #[test]
    fn managed_offline_name_resolution_uses_persisted_agent_uuid_and_workspace() {
        let _home = TestHome::new();
        seed("agent-a", "C:/work/alpha", "Alpha preference");
        let capability = MemoryStore::from_default_home()
            .unwrap()
            .issue_capability("agent-a")
            .unwrap();
        unsafe {
            std::env::set_var("WARDIAN_SESSION_ID", "agent-a");
            std::env::set_var(wardian_core::memory::MEMORY_CAPABILITY_ENV, capability);
        }

        let output = handle_memory(MemoryArgs {
            command: MemoryCommand::List {
                agent: Some("Alpha".into()),
                workspace: None,
            },
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["memories"][0]["agent_id"], "agent-a");
        assert_eq!(value["memories"][0]["text"], "Alpha preference");
    }

    #[test]
    fn managed_agent_can_read_and_mutate_self_memory_by_short_id() {
        let _home = TestHome::new();
        let record = seed("agent-a", "C:/work/alpha", "Alpha preference");
        let short_id = record.memory_id[..8].to_string();
        let capability = MemoryStore::from_default_home()
            .unwrap()
            .issue_capability("agent-a")
            .unwrap();
        unsafe {
            std::env::set_var("WARDIAN_SESSION_ID", "agent-a");
            std::env::set_var(wardian_core::memory::MEMORY_CAPABILITY_ENV, capability);
        }

        let shown = handle_memory(MemoryArgs {
            command: MemoryCommand::Show {
                memory_id: short_id.clone(),
            },
        })
        .unwrap();
        let shown: serde_json::Value = serde_json::from_str(&shown).unwrap();
        assert_eq!(shown["memory"]["memory_id"], record.memory_id);

        let updated = handle_memory(MemoryArgs {
            command: MemoryCommand::Update {
                memory_id: short_id.clone(),
                text: "Alpha preference refined".into(),
                evidence: "The user clarified the preference.".into(),
                source: vec![],
                idempotency_key: None,
            },
        })
        .unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(updated["memory"]["memory_id"], record.memory_id);
        assert_eq!(updated["memory"]["revision"], 2);

        let history = handle_memory(MemoryArgs {
            command: MemoryCommand::History {
                memory_id: short_id.clone(),
            },
        })
        .unwrap();
        let history: serde_json::Value = serde_json::from_str(&history).unwrap();
        assert_eq!(history["history"].as_array().unwrap().len(), 2);

        let removed = handle_memory(MemoryArgs {
            command: MemoryCommand::Remove {
                memory_id: short_id,
            },
        })
        .unwrap();
        let removed: serde_json::Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(removed["memory"]["memory_id"], record.memory_id);
        assert_eq!(removed["memory"]["status"], "removed");
    }

    #[test]
    fn ambiguous_memory_prefix_has_a_distinct_cli_error() {
        let error = memory_id_ambiguous("deadbeef");
        assert_eq!(error.code, "memory_id_ambiguous");
        assert_eq!(error.code_i32(), 5);
        assert!(error.message.contains("use a longer prefix"));
    }

    #[test]
    fn clearing_managed_identity_does_not_grant_operator_memory_access() {
        let _home = TestHome::new();
        seed("agent-b", "C:/work/beta", "Beta preference");
        unsafe {
            std::env::remove_var("WARDIAN_SESSION_ID");
            std::env::remove_var(wardian_core::memory::MEMORY_CAPABILITY_ENV);
        }

        let error = handle_memory(MemoryArgs {
            command: MemoryCommand::List {
                agent: Some("Beta".into()),
                workspace: None,
            },
        })
        .unwrap_err();
        assert_eq!(error.code, "memory_identity_required");
    }

    #[test]
    fn offline_memory_resolution_refuses_an_active_replacement() {
        let _home = TestHome::new();
        let config = wardian_core::models::AgentConfig {
            session_id: "agent-a".into(),
            session_name: "Alpha".into(),
            folder: "C:/work/alpha".into(),
            ..Default::default()
        };
        let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
            .expect("roster barrier")
            .expect("roster barrier acquired");
        let journal = wardian_core::agent_replacement::ReplacementJournalGuard::begin(
            wardian_core::agent_replacement::PendingAgentReplacement::new(
                "clear",
                "agent-a",
                config.clone(),
                config,
                None,
                None,
                None,
                false,
            ),
            barrier,
        )
        .expect("begin active replacement");

        let error = agent_snapshot().expect_err("active replacement must gate offline memory");
        assert!(error.message.contains("replacement is still in progress"));
        journal.complete().expect("complete replacement journal");
    }

    #[test]
    fn managed_agent_can_save_self_but_cannot_target_or_open_peer_memory() {
        let _home = TestHome::new();
        let peer = seed("agent-b", "C:/work/beta", "Beta preference");
        let capability = MemoryStore::from_default_home()
            .unwrap()
            .issue_capability("agent-a")
            .unwrap();
        unsafe {
            std::env::set_var("WARDIAN_SESSION_ID", "agent-a");
            std::env::set_var(wardian_core::memory::MEMORY_CAPABILITY_ENV, &capability);
        }

        let own = handle_memory(MemoryArgs {
            command: MemoryCommand::Save {
                text: "Alpha preference".into(),
                evidence: "The user chose this convention.".into(),
                kind: MemoryKindArg::Stable,
                scope: MemoryScopeArg::Workspace,
                agent: None,
                workspace: None,
                source: vec![],
                idempotency_key: None,
            },
        })
        .unwrap();
        assert!(own.contains("agent-a"));

        let target_error = handle_memory(MemoryArgs {
            command: MemoryCommand::List {
                agent: Some("Beta".into()),
                workspace: None,
            },
        })
        .unwrap_err();
        assert_eq!(target_error.code, "memory_access_denied");

        let save_error = handle_memory(MemoryArgs {
            command: MemoryCommand::Save {
                text: "Unauthorized beta preference".into(),
                evidence: "No authority".into(),
                kind: MemoryKindArg::Stable,
                scope: MemoryScopeArg::Workspace,
                agent: Some("Beta".into()),
                workspace: None,
                source: vec![],
                idempotency_key: None,
            },
        })
        .unwrap_err();
        assert_eq!(save_error.code, "memory_access_denied");

        let recall_error = handle_memory(MemoryArgs {
            command: MemoryCommand::Recall {
                agent: Some("Beta".into()),
                workspace: None,
            },
        })
        .unwrap_err();
        assert_eq!(recall_error.code, "memory_access_denied");

        let record_error = handle_memory(MemoryArgs {
            command: MemoryCommand::Show {
                memory_id: peer.memory_id.clone(),
            },
        })
        .unwrap_err();
        assert_eq!(record_error.code, "memory_access_denied");

        let short_peer_id = peer.memory_id[..8].to_string();
        let short_record_error = handle_memory(MemoryArgs {
            command: MemoryCommand::Show {
                memory_id: short_peer_id,
            },
        })
        .unwrap_err();
        assert_eq!(short_record_error.code, "memory_access_denied");

        let update_error = handle_memory(MemoryArgs {
            command: MemoryCommand::Update {
                memory_id: peer.memory_id.clone(),
                text: "Unauthorized update".into(),
                evidence: "No authority".into(),
                source: vec![],
                idempotency_key: None,
            },
        })
        .unwrap_err();
        assert_eq!(update_error.code, "memory_access_denied");

        let history_error = handle_memory(MemoryArgs {
            command: MemoryCommand::History {
                memory_id: peer.memory_id.clone(),
            },
        })
        .unwrap_err();
        assert_eq!(history_error.code, "memory_access_denied");

        let remove_error = handle_memory(MemoryArgs {
            command: MemoryCommand::Remove {
                memory_id: peer.memory_id.clone(),
            },
        })
        .unwrap_err();
        assert_eq!(remove_error.code, "memory_access_denied");
        assert_eq!(
            MemoryStore::from_default_home()
                .unwrap()
                .get(&MemoryActor::Operator, &peer.memory_id)
                .unwrap()
                .status,
            wardian_core::memory::MemoryStatus::Active
        );

        unsafe { std::env::set_var("WARDIAN_SESSION_ID", "agent-b") };
        let spoofed = handle_memory(MemoryArgs {
            command: MemoryCommand::List {
                agent: None,
                workspace: None,
            },
        })
        .unwrap_err();
        assert_eq!(spoofed.code, "invalid_memory_capability");
    }
}
