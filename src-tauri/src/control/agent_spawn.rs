use super::*;
use wardian_core::control::{AgentSpawnAdmissionResponse, AgentSpawnOperationResponse};

/// Deletion alone may resolve an exact durable ticket after the live target is
/// gone. Names never adopt a ticket, and storage errors never become absence.
pub(super) async fn resolve_removal_uuid(
    state: &AppState,
    target: &str,
) -> Result<Option<String>, ControlError> {
    if let Some(id) = resolve_target_uuid_in_state(state, target).await {
        return Ok(Some(id));
    }
    let Ok(uuid) = uuid::Uuid::parse_str(target) else {
        return Ok(None);
    };
    let id = uuid.to_string();
    let lookup = id.clone();
    let ticket = tokio::task::spawn_blocking(move || {
        wardian_core::agent_spawn::bound_removal_ticket(&lookup)
    })
    .await
    .map_err(|error| ControlError::request_failed(error.to_string()))?
    .map_err(ControlError::request_failed)?;
    Ok(ticket.map(|_| id))
}

pub(super) async fn dispatch(
    request: ControlRequest,
    app: &AppHandle,
) -> Result<String, ControlError> {
    match request {
        ControlRequest::AgentSpawn {
            provider,
            class,
            name,
            workspace,
            model,
            reasoning_effort,
        } => {
            use crate::commands::agent::spawn_agent;
            let provider = crate::commands::agent::canonical_agent_provider_name(&provider)
                .map_err(ControlError::bad_request)?;
            let class = crate::commands::agent::canonical_agent_class_name(&class)
                .map_err(ControlError::not_found)?;
            let req = build_spawn_agent_request(
                provider,
                class,
                name,
                workspace,
                model,
                reasoning_effort,
            )
            .map_err(ControlError::bad_request)?;
            let config = spawn_agent(req, app.state::<AppState>(), app.clone())
                .await
                .map_err(ControlError::request_failed)?;
            let identity = agent_config_to_identity(&config, app).await;
            ok_json(&AgentResponse::new(identity))
        }

        ControlRequest::AgentSpawnAsync {
            operation_id,
            mut request,
        } => {
            request.provider =
                crate::commands::agent::canonical_agent_provider_name(&request.provider)
                    .map_err(ControlError::bad_request)?;
            if request
                .workspace
                .as_deref()
                .is_none_or(|workspace| workspace.trim().is_empty())
            {
                return Err(ControlError::bad_request(
                    "workspace is required for agent spawn",
                ));
            }
            let planned_agent_id = uuid::Uuid::new_v4().to_string();
            let provider_session_id =
                crate::commands::agent::planned_provider_session_id(&request.provider);
            let request_for_admission = request.clone();
            let admission = tokio::task::spawn_blocking(move || {
                wardian_core::agent_spawn::admit(
                    &operation_id,
                    &request_for_admission,
                    &planned_agent_id,
                    provider_session_id.as_deref(),
                )
            })
            .await
            .map_err(|error| {
                ControlError::request_failed(format!("spawn admission worker failed: {error}"))
            })?
            .map_err(|error| {
                let code = if error.contains("queue is full") {
                    "queue_full"
                } else if error.contains("different spawn request") {
                    "idempotency_conflict"
                } else {
                    "bad_request"
                };
                ControlError::coded(code, error)
            })?;
            if admission.operation.state == wardian_core::agent_spawn::AgentSpawnState::Queued {
                crate::spawn_operations::schedule(
                    app.clone(),
                    admission.operation.operation_id.clone(),
                );
            }
            ok_json(&AgentSpawnAdmissionResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                operation: admission.operation,
            })
        }

        ControlRequest::AgentSpawnOperationGet { operation_id } => {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            let operation = crate::spawn_operations::observe_before_deadline(
                deadline,
                tokio::task::spawn_blocking(move || wardian_core::agent_spawn::get(&operation_id)),
            )
            .await?
            .map_err(|error| {
                ControlError::request_failed(format!("spawn lookup worker failed: {error}"))
            })?
            .map_err(ControlError::request_failed)?
            .ok_or_else(|| ControlError::not_found("spawn operation not found"))?;
            ok_json(&operation_response(operation, false, deadline, app).await?)
        }

        ControlRequest::AgentSpawnOperationWait {
            operation_id,
            timeout_ms,
        } => {
            let (operation, timed_out, deadline) =
                crate::spawn_operations::wait_for_operation(&operation_id, timeout_ms).await?;
            ok_json(&operation_response(operation, timed_out, deadline, app).await?)
        }

        _ => unreachable!("spawn dispatch receives only spawn requests"),
    }
}

async fn operation_response<R: tauri::Runtime>(
    operation: wardian_core::agent_spawn::AgentSpawnOperation,
    timed_out: bool,
    deadline: tokio::time::Instant,
    app: &AppHandle<R>,
) -> Result<AgentSpawnOperationResponse, ControlError> {
    let unavailable = AgentSpawnOperationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        operation: operation.clone(),
        timed_out: true,
        availability: wardian_core::control::AgentSpawnAvailability::Unavailable,
        current_agent: None,
    };
    match crate::spawn_operations::observe_before_deadline(
        deadline,
        operation_response_inner(operation, timed_out, deadline, app),
    )
    .await
    {
        Ok(Ok(response))
            if tokio::time::Instant::now() >= deadline
                && response.availability
                    == wardian_core::control::AgentSpawnAvailability::Current =>
        {
            Ok(unavailable)
        }
        Err(error) if error.code() == "wait_timeout" => Ok(unavailable),
        result => result?,
    }
}

async fn operation_response_inner<R: tauri::Runtime>(
    operation: wardian_core::agent_spawn::AgentSpawnOperation,
    timed_out: bool,
    deadline: tokio::time::Instant,
    app: &AppHandle<R>,
) -> Result<AgentSpawnOperationResponse, ControlError> {
    use wardian_core::control::AgentSpawnAvailability as Availability;
    let mut response = AgentSpawnOperationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        availability: Availability::NotRegistered,
        current_agent: None,
        operation,
        timed_out,
    };
    if response.operation.state != wardian_core::agent_spawn::AgentSpawnState::Registered {
        return Ok(response);
    }
    response.availability = Availability::Unavailable;
    let state = app.state::<AppState>();
    if !state
        .agent_roster_restored
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Ok(response);
    }
    let _lifecycle = state
        .lock_agent_lifecycle(&response.operation.agent_id)
        .await;
    let _roster = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .map_err(ControlError::request_failed)?;
    let operation_id = response.operation.operation_id.clone();
    let evidence = tokio::task::spawn_blocking(move || {
        let plan =
            wardian_core::agent_spawn::plan(&operation_id)?.ok_or("Spawn plan is unavailable")?;
        let roster = crate::spawn_operations::persisted_roster()?;
        Ok::<_, String>((plan, roster))
    })
    .await
    .map_err(|error| ControlError::request_failed(error.to_string()))?;
    let Ok((plan, roster)) = evidence else {
        return Ok(response);
    };
    let saved = match saved_registration(&plan, &roster) {
        Ok(saved) => saved,
        Err(availability) => {
            response.availability = availability;
            return Ok(response);
        }
    };
    let source = {
        let agents = state.agents.lock().await;
        agents
            .get(&response.operation.agent_id)
            .map(AgentSnapshotSource::from)
    };
    let Some(source) = source else {
        return Ok(response);
    };
    let (availability, current_agent) = observe_current_identity(
        source,
        saved.clone(),
        deadline,
        wardian_core::conversation_lease::load_leases,
    )
    .await?;
    response.availability = availability;
    response.current_agent = current_agent;
    Ok(response)
}

/// Lease I/O and snapshot mutexes share the observer budget and never occupy
/// the control executor while a filesystem read or another owner is stalled.
async fn observe_current_identity(
    source: AgentSnapshotSource,
    saved: wardian_core::models::AgentConfig,
    deadline: tokio::time::Instant,
    load_leases: impl FnOnce() -> Vec<wardian_core::conversation_lease::ConversationLease>
        + Send
        + 'static,
) -> Result<
    (
        wardian_core::control::AgentSpawnAvailability,
        Option<AgentIdentity>,
    ),
    ControlError,
> {
    use wardian_core::control::AgentSpawnAvailability as Availability;
    let observed = crate::spawn_operations::observe_before_deadline(
        deadline,
        tokio::task::spawn_blocking(move || {
            let config = source
                .config
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            if !same_registered_identity(&saved, &config) {
                return (Availability::Incompatible, None);
            }
            let leases = load_leases();
            let now = chrono::Utc::now().to_rfc3339();
            (
                Availability::Current,
                Some(snapshot_agent_source(&source, &leases, &now)),
            )
        }),
    )
    .await?
    .map_err(|error| ControlError::request_failed(error.to_string()))?;
    if tokio::time::Instant::now() >= deadline {
        return Err(ControlError::coded(
            "wait_timeout",
            "spawn identity observation deadline expired",
        ));
    }
    Ok(observed)
}

fn saved_registration<'a>(
    plan: &wardian_core::agent_spawn::AgentSpawnPlan,
    roster: &'a [wardian_core::models::AgentConfig],
) -> Result<&'a wardian_core::models::AgentConfig, wardian_core::control::AgentSpawnAvailability> {
    use wardian_core::control::AgentSpawnAvailability as Availability;
    if plan.first_native_session_id.is_some() && !plan.first_native_published {
        return Err(Availability::Unavailable);
    }
    let mut matches = roster
        .iter()
        .filter(|config| config.session_id == plan.operation.agent_id);
    let saved = matches.next().ok_or(Availability::Missing)?;
    let native_id = saved
        .resume_session
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .or(saved.fresh_provider_session_id.as_deref());
    if matches.next().is_some()
        || saved.session_name != plan.operation.agent_name
        || saved.provider != plan.operation.provider
        || saved.agent_class != plan.operation.class
        || plan.prepared_workspace.as_deref() != Some(saved.folder.as_str())
        || native_id
            != plan
                .provider_session_id
                .as_deref()
                .or(plan.first_native_session_id.as_deref())
    {
        return Err(Availability::Incompatible);
    }
    Ok(saved)
}

fn same_registered_identity(
    saved: &wardian_core::models::AgentConfig,
    live: &wardian_core::models::AgentConfig,
) -> bool {
    saved.session_id == live.session_id
        && saved.session_name == live.session_name
        && saved.provider == live.provider
        && saved.agent_class == live.agent_class
        && saved.folder == live.folder
        && saved.resume_session == live.resume_session
        && saved.fresh_provider_session_id == live.fresh_provider_session_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn removal_resolves_only_exact_ticket_uuid_after_live_target_is_gone() {
        use wardian_core::agent_spawn as ledger;
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let agent = uuid::Uuid::new_v4().to_string();
        let attempt = uuid::Uuid::new_v4().to_string();
        let config = wardian_core::models::AgentConfig {
            session_id: agent.clone(),
            session_name: "TicketOnly".into(),
            provider: "mock".into(),
            agent_class: "Coder".into(),
            folder: "workspace".into(),
            ..Default::default()
        };
        ledger::reserve_legacy_name(
            &attempt,
            &agent,
            ledger::LegacyNamePurpose::Spawn,
            None,
            &config.session_name,
        )
        .unwrap();
        ledger::begin_legacy_name_effects(&attempt, &agent).unwrap();
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: &agent,
            session_name: &config.session_name,
            description: "",
            agent_class: &config.agent_class,
            provider: &config.provider,
            workspace: Some(&config.folder),
            project: None,
            is_off: true,
            created_at: None,
        })
        .unwrap();
        ledger::complete_legacy_name(&attempt, &agent, std::slice::from_ref(&config)).unwrap();
        ledger::admit_bound_removal(
            &ledger::RemovalAgentIdentity::from(&config),
            None,
            &ledger::RemovalProfileIdentity {
                directory_id: None,
                ownership_records: Default::default(),
            },
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        let state = AppState::new();
        assert_eq!(
            resolve_removal_uuid(&state, &agent).await.unwrap(),
            Some(agent)
        );
        assert_eq!(
            resolve_removal_uuid(&state, "TicketOnly").await.unwrap(),
            None
        );
        assert_eq!(
            resolve_removal_uuid(&state, &uuid::Uuid::new_v4().to_string())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn current_snapshot_deadline_covers_the_final_blocking_lease_read() {
        let saved = wardian_core::models::AgentConfig::default();
        let source = AgentSnapshotSource {
            config: Arc::new(Mutex::new(saved.clone())),
            current_status: Arc::new(Mutex::new("Idle".into())),
            init_timestamp: Arc::new(Mutex::new(None)),
            last_status_at: Arc::new(Mutex::new(None)),
            process_id: None,
        };
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let observer = tokio::spawn(observe_current_identity(
            source,
            saved,
            tokio::time::Instant::now() + std::time::Duration::from_millis(20),
            move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Vec::new()
            },
        ));
        entered_rx.await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), observer).await;
        release_tx.send(()).unwrap();
        assert_eq!(result.unwrap().unwrap().unwrap_err().code(), "wait_timeout");
    }

    #[tokio::test]
    async fn spawn_observation_deadline_bounds_database_lifecycle_and_roster_contention() {
        use wardian_core::agent_spawn as spawn;
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let id = uuid::Uuid::new_v4().to_string();
        let agent_id = uuid::Uuid::new_v4().to_string();
        let request = spawn::AgentSpawnRequest {
            provider: "codex".into(),
            class: "Coder".into(),
            name: Some("wait-fixture".into()),
            workspace: Some("workspace".into()),
            model: None,
            reasoning_effort: None,
        };
        spawn::admit(&id, &request, &agent_id, None).unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            wardian_core::db::get_db_conn(|_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap()
        });
        entered_rx.await.unwrap();
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::spawn_operations::wait_for_operation(&id, 20),
        )
        .await
        .unwrap();
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(observed.unwrap_err().code(), "wait_timeout");
        assert_eq!(
            spawn::get(&id).unwrap().unwrap().state,
            spawn::AgentSpawnState::Queued
        );
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        state
            .agent_roster_restored
            .store(true, std::sync::atomic::Ordering::Release);
        let mut operation = spawn::get(&id).unwrap().unwrap();
        operation.state = spawn::AgentSpawnState::Registered;
        let lifecycle = state.lock_agent_lifecycle(&agent_id).await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(30);
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        let response = operation_response(operation.clone(), false, deadline, app.handle())
            .await
            .unwrap();
        assert!(response.timed_out);
        assert_eq!(
            response.availability,
            wardian_core::control::AgentSpawnAvailability::Unavailable
        );
        drop(lifecycle);
        let barrier = crate::manager::roster_io::acquire_roster_barrier()
            .await
            .unwrap();
        let response = operation_response(
            operation,
            false,
            tokio::time::Instant::now() + std::time::Duration::from_millis(20),
            app.handle(),
        )
        .await
        .unwrap();
        assert!(response.timed_out);
        assert!(response.current_agent.is_none());
        drop(barrier);
        assert_eq!(
            spawn::get(&id).unwrap().unwrap().state,
            spawn::AgentSpawnState::Queued
        );
    }

    #[test]
    fn spawn_exact_queued_retry_recovers_after_preclaim_storage_failure_without_replay() {
        use wardian_core::agent_spawn as spawn;
        let _home = crate::control::test_support::TestWardianHome::new();
        let id = uuid::Uuid::new_v4().to_string();
        let agent = uuid::Uuid::new_v4().to_string();
        let request = spawn::AgentSpawnRequest {
            provider: "codex".into(),
            class: "Coder".into(),
            name: Some("claim-fixture".into()),
            workspace: Some("workspace".into()),
            model: None,
            reasoning_effort: None,
        };
        spawn::admit(&id, &request, &agent, None).unwrap();
        wardian_core::db::get_db_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER preclaim_cut BEFORE UPDATE ON agent_spawn_operations
                WHEN NEW.state = 'preparing' BEGIN SELECT RAISE(ABORT, 'claim storage fault'); END;")?;
            Ok(())
        }).unwrap();
        assert!(spawn::claim_queued(&id).is_err());
        let retry = spawn::admit(&id, &request, &uuid::Uuid::new_v4().to_string(), None).unwrap();
        assert!(!retry.created);
        assert_eq!(retry.operation.state, spawn::AgentSpawnState::Queued);
        assert_eq!(retry.operation.agent_id, agent);
        wardian_core::db::get_db_conn(|conn| {
            conn.execute_batch("DROP TRIGGER preclaim_cut")?;
            Ok(())
        })
        .unwrap();
        assert!(spawn::claim_queued(&id).unwrap().is_some());
        assert!(spawn::claim_queued(&id).unwrap().is_none());
    }

    #[test]
    fn spawn_first_owned_native_binding_preserves_registration_and_rejects_replacement() {
        use wardian_core::agent_spawn as spawn;
        for provider in ["opencode", "antigravity"] {
            let _home = crate::control::test_support::TestWardianHome::new();
            let id = uuid::Uuid::new_v4().to_string();
            let agent = uuid::Uuid::new_v4().to_string();
            let request = spawn::AgentSpawnRequest {
                provider: provider.into(),
                class: "Coder".into(),
                name: Some("native-fixture".into()),
                workspace: Some("workspace".into()),
                model: None,
                reasoning_effort: None,
            };
            spawn::admit(&id, &request, &agent, None).unwrap();
            let attempt = spawn::claim_queued(&id)
                .unwrap()
                .unwrap()
                .attempt_id
                .unwrap();
            spawn::set_prepared_identity(&id, &attempt, "Coder", "workspace").unwrap();
            spawn::mark_launching(&id, &attempt).unwrap();
            wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
                session_id: &agent,
                session_name: "native-fixture",
                description: "",
                agent_class: "Coder",
                provider,
                workspace: Some("workspace"),
                project: None,
                is_off: true,
                created_at: None,
            })
            .unwrap();
            let mut config = wardian_core::models::AgentConfig {
                session_id: agent.clone(),
                session_name: "native-fixture".into(),
                provider: provider.into(),
                agent_class: "Coder".into(),
                folder: "workspace".into(),
                ..Default::default()
            };
            spawn::record_first_native_runtime(&config, 7, false).unwrap();
            assert!(spawn::mark_registered(&id, &attempt, None).unwrap());
            // A paused restore allocates no runtime. A later fresh resume can
            // reuse the old number, but cannot consume its old-boot permission.
            spawn::recover_after_owner_acquired().unwrap();
            spawn::record_first_native_runtime(&config, 7, false).unwrap();
            spawn::record_first_native_binding(&agent, provider, "ses_wrong_boot", 7).unwrap();
            assert!(!spawn::first_native_binding_matches(&agent, "ses_wrong_boot", 7).unwrap());
            // Only an eligible exact restore explicitly admits its new owner.
            spawn::record_first_native_runtime(&config, 7, true).unwrap();
            spawn::record_first_native_runtime(&config, 8, false).unwrap();
            spawn::record_first_native_binding(&agent, provider, "ses_replacement", 8).unwrap();
            assert!(!spawn::first_native_binding_matches(&agent, "ses_replacement", 8).unwrap());
            assert!(saved_registration(
                &spawn::plan(&id).unwrap().unwrap(),
                std::slice::from_ref(&config)
            )
            .is_ok());
            spawn::record_first_native_binding(&agent, provider, "ses_first", 7).unwrap();
            assert!(spawn::restore_is_excluded(&agent).unwrap());
            config.resume_session = Some("ses_wrong".into());
            crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config))
                .unwrap();
            spawn::recover_first_native_bindings(
                &crate::spawn_operations::persisted_roster().unwrap(),
            )
            .unwrap();
            assert!(!spawn::plan(&id).unwrap().unwrap().first_native_published);
            config.resume_session = Some("ses_first".into());
            crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config))
                .unwrap();
            spawn::recover_first_native_bindings(
                &crate::spawn_operations::persisted_roster().unwrap(),
            )
            .unwrap();
            assert!(spawn::plan(&id).unwrap().unwrap().first_native_published);
            spawn::record_first_native_runtime(&config, 99, true).unwrap();
            assert!(spawn::first_native_binding_matches(&agent, "ses_first", 7).unwrap());
            assert!(!spawn::first_native_binding_matches(&agent, "ses_first", 8).unwrap());
            config.resume_session = Some("ses_first".into());
            let plan = spawn::plan(&id).unwrap().unwrap();
            assert!(plan.provider_session_id.is_none());
            assert!(saved_registration(&plan, std::slice::from_ref(&config)).is_ok());
            spawn::record_first_native_binding(&agent, provider, "ses_replacement", 8).unwrap();
            config.resume_session = Some("ses_replacement".into());
            assert!(saved_registration(
                &spawn::plan(&id).unwrap().unwrap(),
                std::slice::from_ref(&config)
            )
            .is_err());
            let retry =
                spawn::admit(&id, &request, &uuid::Uuid::new_v4().to_string(), None).unwrap();
            assert!(!retry.created);
            assert_eq!(retry.operation.state, spawn::AgentSpawnState::Registered);
        }
    }

    #[test]
    fn registered_same_key_retains_history_but_requires_compatible_saved_config() {
        use wardian_core::agent_spawn as spawn;
        use wardian_core::control::AgentSpawnAvailability as Availability;
        let _home = crate::control::test_support::TestWardianHome::new();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let agent_id = uuid::Uuid::new_v4().to_string();
        let request = spawn::AgentSpawnRequest {
            provider: "codex".into(),
            class: "Coder".into(),
            name: Some("registered".into()),
            workspace: Some("workspace".into()),
            model: None,
            reasoning_effort: None,
        };
        spawn::admit(&operation_id, &request, &agent_id, None).unwrap();
        let plan = spawn::claim_queued(&operation_id).unwrap().unwrap();
        let attempt = plan.attempt_id.unwrap();
        assert!(
            spawn::set_prepared_identity(&operation_id, &attempt, "Coder", "workspace").unwrap()
        );
        assert!(spawn::mark_launching(&operation_id, &attempt).unwrap());
        assert!(spawn::record_provider_session_id(&operation_id, &attempt, "thread").unwrap());
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: &agent_id,
            session_name: "registered",
            description: "",
            agent_class: "Coder",
            provider: "codex",
            workspace: Some("workspace"),
            project: None,
            is_off: true,
            created_at: None,
        })
        .unwrap();
        assert!(spawn::mark_registered(&operation_id, &attempt, Some("thread")).unwrap());
        let retry = spawn::admit(
            &operation_id,
            &request,
            &uuid::Uuid::new_v4().to_string(),
            None,
        )
        .unwrap();
        assert!(!retry.created);
        assert_eq!(retry.operation.state, spawn::AgentSpawnState::Registered);
        let plan = spawn::plan(&operation_id).unwrap().unwrap();
        assert_eq!(
            saved_registration(&plan, &[]).unwrap_err(),
            Availability::Missing
        );
        let mut config = wardian_core::models::AgentConfig {
            session_id: agent_id,
            session_name: "registered".into(),
            provider: "codex".into(),
            agent_class: "Coder".into(),
            folder: "workspace".into(),
            resume_session: Some("thread".into()),
            ..Default::default()
        };
        assert!(saved_registration(&plan, std::slice::from_ref(&config)).is_ok());
        for field in ["name", "provider", "class", "workspace", "native_id"] {
            let saved = config.clone();
            match field {
                "name" => config.session_name = "other".into(),
                "provider" => config.provider = "claude".into(),
                "class" => config.agent_class = "Other".into(),
                "workspace" => config.folder = "elsewhere".into(),
                _ => config.resume_session = Some("other".into()),
            }
            assert_eq!(
                saved_registration(&plan, std::slice::from_ref(&config)).unwrap_err(),
                Availability::Incompatible
            );
            assert!(!same_registered_identity(&saved, &config));
            config = saved;
        }
        assert_eq!(
            spawn::get(&operation_id).unwrap().unwrap().state,
            spawn::AgentSpawnState::Registered
        );
    }
}
