use super::*;

pub(crate) struct NewAgentIdentityPlan {
    pub(crate) wardian_session_id: String,
    pub(crate) fresh_provider_session_id: Option<String>,
    pub(crate) bootstrap_provider_session: bool,
}

pub(super) fn new_agent_identity_plan(provider_name: &str) -> NewAgentIdentityPlan {
    NewAgentIdentityPlan {
        wardian_session_id: uuid::Uuid::new_v4().to_string(),
        fresh_provider_session_id: planned_provider_session_id(provider_name),
        bootstrap_provider_session: provider_name == "codex",
    }
}

pub(crate) fn planned_provider_session_id(provider_name: &str) -> Option<String> {
    provider_uses_manual_session_id(provider_name).then(|| uuid::Uuid::new_v4().to_string())
}

pub(crate) fn persisted_identity_plan(
    provider_name: &str,
    wardian_session_id: String,
    provider_session_id: Option<String>,
) -> NewAgentIdentityPlan {
    NewAgentIdentityPlan {
        wardian_session_id,
        fresh_provider_session_id: provider_session_id,
        bootstrap_provider_session: provider_name == "codex",
    }
}

pub(crate) struct PreparedAgentSpawn {
    pub(crate) config: AgentConfig,
    actual_resume: Option<String>,
    bootstrap_provider_session: bool,
    codex_launch_context: Option<std::sync::Arc<crate::utils::codex_home::CodexLaunchContext>>,
}

pub(super) fn prepare_agent_spawn(
    req: SpawnAgentRequest,
    identity: NewAgentIdentityPlan,
    session_name: String,
) -> Result<PreparedAgentSpawn, String> {
    let agent_class = canonical_agent_class_name(&req.agent_class)?;
    let folder = normalize_spawn_folder(&req.folder)?;
    let provider_name = req
        .config_override
        .as_ref()
        .map(|config| config.provider.clone())
        .unwrap_or_else(|| "claude".to_string());
    let provider_name = canonical_agent_provider_name(&provider_name)?;
    ensure_provider_available_before_session_bootstrap(&provider_name)?;

    let mut config = req.config_override.unwrap_or_default();
    config.provider = provider_name.clone();
    config.session_id = identity.wardian_session_id;
    config.session_name = session_name;
    config.agent_class = agent_class;
    config.folder = folder;
    config.resume_session = None;
    config.fresh_provider_session_id = None;
    config.is_off = req.is_off.unwrap_or(false);
    config.normalize_provider_config_for_provider();
    config.mark_provider_config_nested_for_save();

    let requested_resume = req.resume_session.filter(|value| !value.trim().is_empty());
    let bootstrap_provider_session =
        requested_resume.is_none() && identity.bootstrap_provider_session;
    let actual_resume = if let Some(resume_session) = requested_resume {
        config.resume_session = Some(resume_session.clone());
        manager::apply_provider_identity(&provider_name, &mut config, &resume_session)?;
        Some(resume_session)
    } else if let Some(provider_session_id) = identity.fresh_provider_session_id {
        config.fresh_provider_session_id = Some(provider_session_id.clone());
        Some(provider_session_id)
    } else {
        None
    };

    Ok(PreparedAgentSpawn {
        config,
        actual_resume,
        bootstrap_provider_session,
        codex_launch_context: None,
    })
}

pub(crate) fn prepare_durable_agent_spawn(
    request: wardian_core::agent_spawn::AgentSpawnRequest,
    plan: wardian_core::agent_spawn::AgentSpawnPlan,
) -> Result<PreparedAgentSpawn, String> {
    let mut prepared = prepare_durable_config(request, &plan)?;
    let attempt_id = plan
        .attempt_id
        .as_deref()
        .ok_or_else(|| "durable spawn operation has no active attempt".to_string())?;
    if !wardian_core::agent_spawn::set_prepared_identity(
        &plan.operation.operation_id,
        attempt_id,
        &prepared.config.agent_class,
        &prepared.config.folder,
    )? {
        return Err("durable spawn operation lost its preparation claim".into());
    }
    if prepared.config.provider == "codex" {
        let descriptor = capture_codex_context(&mut prepared)?;
        if !wardian_core::agent_spawn::record_launch_context(
            &plan.operation.operation_id,
            attempt_id,
            &descriptor,
        )? {
            return Err("durable spawn operation lost its policy preparation claim".into());
        }
    }
    Ok(prepared)
}

fn prepare_durable_config(
    request: wardian_core::agent_spawn::AgentSpawnRequest,
    plan: &wardian_core::agent_spawn::AgentSpawnPlan,
) -> Result<PreparedAgentSpawn, String> {
    let mut config_override = AgentConfig {
        provider: request.provider.clone(),
        ..Default::default()
    };
    config_override.reset_provider_config_for_provider();
    apply_agent_model_selection_update(
        &mut config_override,
        request.model.as_deref(),
        request.reasoning_effort.as_deref(),
    )?;
    let spawn_request = SpawnAgentRequest {
        session_name: plan.operation.agent_name.clone(),
        agent_class: request.class,
        folder: request.workspace.unwrap_or_default(),
        resume_session: None,
        is_off: None,
        config_override: Some(config_override),
    };
    let identity = persisted_identity_plan(
        &request.provider,
        plan.operation.agent_id.clone(),
        plan.provider_session_id.clone(),
    );
    prepare_agent_spawn(spawn_request, identity, plan.operation.agent_name.clone())
}

fn capture_codex_context(prepared: &mut PreparedAgentSpawn) -> Result<String, String> {
    let context = crate::utils::codex_home::CodexLaunchContext::capture(&prepared.config)
        .map_err(|_| "Codex launch policy or source could not be captured".to_string())?;
    let descriptor = context.descriptor_json()?;
    prepared.codex_launch_context = Some(std::sync::Arc::new(context));
    Ok(descriptor)
}

pub(crate) fn prepare_recovery_launch(
    witness: &wardian_core::agent_spawn::recovery::WitnessedLaunch,
    model: &str,
) -> Result<PreparedAgentSpawn, String> {
    let plan = &witness.plan;
    let mut request = plan.request.clone();
    request.model = Some(model.to_owned());
    let mut prepared = prepare_durable_config(request, plan)?;
    if prepared.config.session_id != plan.operation.agent_id
        || prepared.config.session_name != plan.operation.agent_name
        || prepared.config.agent_class != plan.operation.class
        || Some(prepared.config.folder.as_str()) != plan.prepared_workspace.as_deref()
        || prepared.config.provider != "codex"
        || prepared.config.model.as_deref() != Some(model)
        || !prepared.bootstrap_provider_session
        || prepared.actual_resume.is_some()
        || capture_codex_context(&mut prepared)? != witness.launch_context_json
    {
        return Err("The prepared Codex launch no longer matches the witnessed inputs.".into());
    }
    Ok(prepared)
}

pub(crate) struct RecoveryBootstrap {
    pub lease: wardian_core::conversation_lease::PersistedConversationLeaseGuard,
    pub roster: wardian_core::agent_replacement::AgentRosterBarrier,
    pub lifecycle: tokio::sync::OwnedMutexGuard<()>,
}

pub(crate) async fn launch_prepared_agent_spawn(
    mut prepared: PreparedAgentSpawn,
    state: &AppState,
    app: &AppHandle,
    legacy_name_claim_owner: Option<String>,
    durable_operation: Option<(String, String)>,
    recovery: Option<RecoveryBootstrap>,
) -> Result<super::codex_onboarding::RegisteredAgentSpawn, String> {
    if let Some(owner_id) = legacy_name_claim_owner.as_deref() {
        wardian_core::agent_spawn::begin_legacy_name_effects(
            owner_id,
            &prepared.config.session_id,
        )?;
    }
    let inherited_lease =
        bootstrap_prepared_agent_spawn(&mut prepared, durable_operation.as_ref(), recovery).await?;
    let session_name = prepared.config.session_name.clone();
    register_new_agent(
        prepared.config,
        prepared.actual_resume,
        state,
        app,
        AgentRegistrationOptions {
            clone_name_base: None,
            reserved_session_name: Some(&session_name),
            name_claim_owner_id: legacy_name_claim_owner.as_deref(),
            placement: configured_new_agent_order_placement(),
            emit_roster_update: true,
        },
        prepared.codex_launch_context,
        inherited_lease,
    )
    .await
}

/// Materialize and record the native identity before handing the acquisition
/// to provisional registration. The recovery branch cannot run Codex exec.
pub(crate) async fn bootstrap_prepared_agent_spawn(
    prepared: &mut PreparedAgentSpawn,
    durable_operation: Option<&(String, String)>,
    recovery: Option<RecoveryBootstrap>,
) -> Result<Option<wardian_core::conversation_lease::PersistedConversationLeaseGuard>, String> {
    let recovery_only = recovery.is_some();
    let recovery = if let Some(RecoveryBootstrap {
        lease,
        roster,
        lifecycle,
    }) = recovery
    {
        let context = prepared
            .codex_launch_context
            .as_ref()
            .ok_or("The recovery launch context is unavailable")?;
        context.revalidate()?;
        let mut lease =
            manager::spawn::provider_spawn_lease_for_startup(&prepared.config, Some(lease))
                .map_err(|error| error.to_string())?;
        lease.retain_on_drop();
        Some(RecoveryBootstrap {
            lease,
            roster,
            lifecycle,
        })
    } else {
        None
    };
    if prepared.bootstrap_provider_session {
        let cwd = if recovery_only {
            crate::utils::fs::validate_exact_spawn_workspace(&prepared.config.folder)?
        } else {
            crate::utils::fs::resolve_cwd(&prepared.config.folder, "")
        };
        let provider_session_id = if let Some(context) = prepared.codex_launch_context.as_ref() {
            manager::headless::obtain_session_id_with_codex_context(
                &cwd,
                Some(&prepared.config.agent_class),
                Some(&prepared.config),
                context,
                durable_operation
                    .map(|(operation_id, attempt_id)| (operation_id.as_str(), attempt_id.as_str())),
                recovery_only,
            )
            .await
        } else {
            manager::obtain_session_id(
                &cwd,
                Some(&prepared.config.agent_class),
                Some(&prepared.config),
            )
            .await
        }
        .map_err(|error| format!("Failed to initialize the provider session: {error}"))?;
        if let Some((operation_id, attempt_id)) = durable_operation {
            if !wardian_core::agent_spawn::record_provider_session_id(
                operation_id,
                attempt_id,
                &provider_session_id,
            )? {
                return Err(
                    "spawn operation lost ownership before provider identity was recorded".into(),
                );
            }
        }
        let provider = prepared.config.provider.clone();
        manager::apply_provider_identity(&provider, &mut prepared.config, &provider_session_id)?;
        prepared.actual_resume = Some(provider_session_id);
    }

    // Provisional registration acquires both barriers again. Persisted
    // exclusion survives that handoff and is retargeted to the native ID.
    Ok(recovery.map(
        |RecoveryBootstrap {
             lease,
             roster,
             lifecycle,
         }| {
            drop(roster);
            drop(lifecycle);
            lease
        },
    ))
}
