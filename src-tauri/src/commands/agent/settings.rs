use super::{apply_agent_update_fields, set_agent_reasoning_effort};
use crate::commands::agent::config_persistence::persist_agent_config_with_roster_barrier;
use crate::manager;
use crate::providers::codex_model_selection::{
    resolve_live_selection_for_settings, CodexLiveModelSelection, CodexLiveSelectionError,
};
use crate::providers::models::ProviderModelCatalog;
use crate::state::AppState;
use tauri::{AppHandle, State};
use wardian_core::models::{AgentConfig, ProviderConfig};

/// Aggregate outcome of applying persisted model and effort settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentModelLiveApplication {
    Applied,
    Deferred,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSettingIntent {
    Unchanged,
    Set,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSettingLiveStatus {
    Unchanged,
    Applied,
    Deferred,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentSettingUpdateResult {
    pub intent: AgentSettingIntent,
    pub desired_value: Option<String>,
    pub live_status: AgentSettingLiveStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_value: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentRuntimeBinding {
    pub agent_id: String,
    pub generation: u64,
    pub thread_id: String,
    pub provider: String,
}

/// Persisted agent configuration together with the independent live outcome.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentModelSelectionUpdateResult {
    pub config: AgentConfig,
    pub live_application: AgentModelLiveApplication,
    pub live_error: Option<String>,
    pub model: AgentSettingUpdateResult,
    pub reasoning_effort: AgentSettingUpdateResult,
    pub restart_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_binding: Option<AgentRuntimeBinding>,
}

pub(crate) async fn update_agent_config_inner<R: tauri::Runtime>(
    mut new_config: AgentConfig,
    state: State<'_, AppState>,
    _app: AppHandle<R>,
) -> Result<AgentModelSelectionUpdateResult, String> {
    manager::log_debug(&format!(
        "[WARDIAN] update_agent_config called for session: {}",
        new_config.session_id
    ));
    normalize_complete_config_settings(&mut new_config)?;
    let session_id = new_config.session_id.clone();
    update_agent_settings(state.inner(), &session_id, move |current| {
        let model =
            setting_change_from_config(current.model.as_deref(), new_config.model.as_deref());
        let effort = setting_change_from_config(
            agent_reasoning_effort(current),
            agent_reasoning_effort(&new_config),
        );
        Ok((new_config.clone(), model, effort))
    })
    .await
}

struct AgentModelSelectionMutationGuards {
    roster: Option<wardian_core::agent_replacement::AgentRosterBarrier>,
    _lifecycle: tokio::sync::OwnedMutexGuard<()>,
    _delivery: tokio::sync::OwnedMutexGuard<()>,
}

async fn lock_agent_model_selection_mutation(
    state: &AppState,
    session_id: &str,
) -> Result<AgentModelSelectionMutationGuards, String> {
    // Acquire the roster barrier before either local gate. If a local gate is
    // busy, release the barrier and retry rather than waiting for the roster
    // while holding lifecycle or delivery ownership.
    let lifecycle = state.agent_lifecycle_lock_for(session_id).await;
    let delivery = state.delivery_lock_for(session_id).await;
    loop {
        let roster = match wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
            .map_err(|error| error.to_string())?
        {
            Some(roster) => roster,
            None => tokio::task::spawn_blocking(|| {
                wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Agent roster barrier is unavailable".to_string())?,
        };
        let Some(lifecycle_guard) = lifecycle.clone().try_lock_owned().ok() else {
            drop(roster);
            tokio::task::yield_now().await;
            continue;
        };
        let Some(delivery_guard) = delivery.clone().try_lock_owned().ok() else {
            drop(lifecycle_guard);
            drop(roster);
            tokio::task::yield_now().await;
            continue;
        };
        return Ok(AgentModelSelectionMutationGuards {
            roster: Some(roster),
            _lifecycle: lifecycle_guard,
            _delivery: delivery_guard,
        });
    }
}

impl AgentModelSelectionMutationGuards {
    fn roster(&self) -> &wardian_core::agent_replacement::AgentRosterBarrier {
        self.roster
            .as_ref()
            .expect("settings mutation roster barrier released too early")
    }

    fn release_roster(&mut self) {
        self.roster.take();
    }
}

pub(crate) async fn update_agent_model_selection_inner(
    session_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    state: State<'_, AppState>,
    _app: AppHandle,
) -> Result<AgentModelSelectionUpdateResult, String> {
    update_agent_settings(state.inner(), &session_id, move |current| {
        let model = setting_change_from_complete_input(model.clone(), "model")?;
        let effort =
            setting_change_from_complete_input(reasoning_effort.clone(), "reasoning_effort")?;
        let mut config = current.clone();
        config.model = model.desired_value.clone();
        set_agent_reasoning_effort(&mut config, effort.desired_value.clone())?;
        Ok((config, model, effort))
    })
    .await
}

#[derive(Debug, Clone)]
struct AgentSettingChange {
    intent: AgentSettingIntent,
    desired_value: Option<String>,
}

fn normalize_complete_config_settings(config: &mut AgentConfig) -> Result<(), String> {
    config.model = normalize_complete_setting(config.model.take(), "model")?;
    let effort = normalize_complete_setting(
        agent_reasoning_effort(config).map(str::to_owned),
        "reasoning_effort",
    )?;
    set_agent_reasoning_effort(config, effort)?;
    Ok(())
}

fn normalize_complete_setting(
    value: Option<String>,
    field: &str,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(format!("{field} cannot be empty"));
            }
            Ok(Some(value.to_string()))
        }
    }
}

fn setting_change_from_config(current: Option<&str>, desired: Option<&str>) -> AgentSettingChange {
    if current == desired {
        AgentSettingChange {
            intent: AgentSettingIntent::Unchanged,
            desired_value: desired.map(str::to_owned),
        }
    } else {
        AgentSettingChange {
            intent: if desired.is_some() {
                AgentSettingIntent::Set
            } else {
                AgentSettingIntent::Default
            },
            desired_value: desired.map(str::to_owned),
        }
    }
}

fn setting_change_from_complete_input(
    value: Option<String>,
    field: &str,
) -> Result<AgentSettingChange, String> {
    match value {
        None => Ok(AgentSettingChange {
            intent: AgentSettingIntent::Default,
            desired_value: None,
        }),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(format!("{field} cannot be empty"));
            }
            Ok(AgentSettingChange {
                intent: AgentSettingIntent::Set,
                desired_value: Some(value.to_string()),
            })
        }
    }
}

enum AgentLivePlan {
    NoLiveChange,
    Deferred {
        reason: String,
        restart_required: bool,
    },
    Apply(CodexLiveModelSelection),
}

struct AgentLiveOutcome {
    application: AgentModelLiveApplication,
    error: Option<String>,
    reason: String,
    model_status: AgentSettingLiveStatus,
    effort_status: AgentSettingLiveStatus,
    model_effective: Option<String>,
    effort_effective: Option<String>,
    runtime_binding: Option<AgentRuntimeBinding>,
    restart_required: bool,
}

async fn update_agent_settings<F>(
    state: &AppState,
    session_id: &str,
    build: F,
) -> Result<AgentModelSelectionUpdateResult, String>
where
    F: FnMut(&AgentConfig) -> Result<(AgentConfig, AgentSettingChange, AgentSettingChange), String>,
{
    let mut build = build;
    for attempt in 0..=1 {
        let initial = read_agent_settings_snapshot(state, session_id).await?;
        let (config, model_change, effort_change) = build(&initial.config)?;
        if config.session_id != session_id {
            return Err("Agent configuration session identity changed during update".to_string());
        }

        // Catalog discovery can invoke an external provider command. Keep it
        // outside the roster barrier, then revalidate the snapshot under all
        // mutation fences before persisting. A restore publication can advance
        // the runtime generation while this plan is waiting; release every
        // mutation guard and build a fresh plan once in that case.
        let live_plan = prepare_agent_live_plan(&config, &model_change, &effort_change).await?;
        let mut mutation_guards = lock_agent_model_selection_mutation(state, session_id).await?;
        let current = read_agent_settings_snapshot(state, session_id).await?;
        if !settings_snapshot_matches(&initial, &current)? {
            drop(mutation_guards);
            if attempt == 0 {
                continue;
            }
            return Err(
                "Agent configuration or runtime changed while validating model settings; retry"
                    .to_string(),
            );
        }
        let launch_only_changed = launch_only_config_changed(&current.config, &config);
        persist_agent_config_with_roster_barrier(config.clone(), state, mutation_guards.roster())
            .await?;
        mutation_guards.release_roster();

        let live = match live_plan {
            AgentLivePlan::NoLiveChange => AgentLiveOutcome {
                application: AgentModelLiveApplication::Deferred,
                error: None,
                reason: "no_live_change".to_string(),
                model_status: setting_status(&model_change, AgentSettingLiveStatus::Unchanged),
                effort_status: setting_status(&effort_change, AgentSettingLiveStatus::Unchanged),
                model_effective: None,
                effort_effective: None,
                runtime_binding: None,
                restart_required: launch_only_changed,
            },
            AgentLivePlan::Deferred {
                reason,
                restart_required,
            } => AgentLiveOutcome {
                application: AgentModelLiveApplication::Deferred,
                error: None,
                reason: reason.clone(),
                model_status: setting_status(&model_change, AgentSettingLiveStatus::Deferred),
                effort_status: setting_status(&effort_change, AgentSettingLiveStatus::Deferred),
                model_effective: None,
                effort_effective: None,
                runtime_binding: None,
                restart_required: launch_only_changed || restart_required,
            },
            AgentLivePlan::Apply(selection) => {
                apply_resolved_agent_model_selection_live(
                    state,
                    session_id,
                    &model_change,
                    &effort_change,
                    &selection,
                    launch_only_changed,
                )
                .await
            }
        };

        return Ok(AgentModelSelectionUpdateResult {
            config,
            live_application: live.application,
            live_error: live.error,
            model: setting_result(
                &model_change,
                live.model_status,
                live.model_effective,
                &live.reason,
            ),
            reasoning_effort: setting_result(
                &effort_change,
                live.effort_status,
                live.effort_effective,
                &live.reason,
            ),
            restart_required: live.restart_required,
            runtime_binding: live.runtime_binding,
        });
    }
    unreachable!("settings update retry loop always returns");
}

#[derive(Clone)]
struct AgentSettingsSnapshot {
    config: AgentConfig,
    runtime_generation: Option<u64>,
}

pub(crate) struct AgentControlUpdate<'a> {
    pub class: Option<&'a str>,
    pub workspace: Option<&'a str>,
    pub description: Option<&'a str>,
    pub model: Option<Option<String>>,
    pub reasoning_effort: Option<Option<String>>,
    pub classes: &'a [wardian_core::models::AgentClassDefinition],
}

async fn read_agent_settings_snapshot(
    state: &AppState,
    session_id: &str,
) -> Result<AgentSettingsSnapshot, String> {
    let agents = state.agents.lock().await;
    let agent = agents
        .get(session_id)
        .ok_or_else(|| format!("Agent {session_id} not found"))?;
    let config = agent.config.lock().unwrap().clone();
    Ok(AgentSettingsSnapshot {
        config,
        runtime_generation: agent.runtime_generation,
    })
}

fn settings_snapshot_matches(
    expected: &AgentSettingsSnapshot,
    actual: &AgentSettingsSnapshot,
) -> Result<bool, String> {
    Ok(expected.runtime_generation == actual.runtime_generation
        && serde_json::to_value(&expected.config).map_err(|error| error.to_string())?
            == serde_json::to_value(&actual.config).map_err(|error| error.to_string())?)
}

pub(crate) async fn update_agent_from_control(
    state: &AppState,
    session_id: &str,
    update: AgentControlUpdate<'_>,
) -> Result<(AgentModelSelectionUpdateResult, Vec<String>), String> {
    if update.class.is_none()
        && update.workspace.is_none()
        && update.description.is_none()
        && update.model.is_none()
        && update.reasoning_effort.is_none()
    {
        return Err("At least one agent update field is required".to_string());
    }
    validate_control_setting(update.model.as_ref(), "model")?;
    validate_control_setting(update.reasoning_effort.as_ref(), "reasoning_effort")?;
    let mut updated_fields = Vec::new();
    let result = update_agent_settings(state, session_id, |current| {
        let mut config = current.clone();
        let model_input = update
            .model
            .as_ref()
            .map(|value| value.as_deref().unwrap_or_default());
        let effort_input = update
            .reasoning_effort
            .as_ref()
            .map(|value| value.as_deref().unwrap_or_default());
        updated_fields = apply_agent_update_fields(
            &mut config,
            update.class,
            update.workspace,
            update.description,
            model_input,
            effort_input,
            update.classes,
        )?;
        if updated_fields.iter().any(|field| field == "class") {
            config.system_include_directories =
                Some(crate::utils::fs::resolve_system_include_directories(
                    &config.agent_class,
                    &config.session_id,
                ));
        }
        let model_change =
            setting_change_from_config(current.model.as_deref(), config.model.as_deref());
        let effort_change = setting_change_from_config(
            agent_reasoning_effort(current),
            agent_reasoning_effort(&config),
        );
        Ok((config, model_change, effort_change))
    })
    .await?;
    Ok((result, updated_fields))
}

fn validate_control_setting(value: Option<&Option<String>>, field: &str) -> Result<(), String> {
    if let Some(Some(value)) = value {
        if !value.is_empty() && value.trim().is_empty() {
            return Err(format!("{field} cannot be whitespace"));
        }
    }
    Ok(())
}

fn agent_reasoning_effort(config: &AgentConfig) -> Option<&str> {
    match &config.provider_config {
        ProviderConfig::Claude(value) => value.reasoning_effort.as_deref(),
        ProviderConfig::Codex(value) => value.reasoning_effort.as_deref(),
        ProviderConfig::Antigravity(value) => value.reasoning_effort.as_deref(),
        ProviderConfig::Pi(value) => value.reasoning_effort.as_deref(),
        _ => None,
    }
}

fn setting_status(
    change: &AgentSettingChange,
    changed_status: AgentSettingLiveStatus,
) -> AgentSettingLiveStatus {
    if change.intent == AgentSettingIntent::Unchanged {
        AgentSettingLiveStatus::Unchanged
    } else {
        changed_status
    }
}

fn setting_result(
    change: &AgentSettingChange,
    status: AgentSettingLiveStatus,
    effective_value: Option<String>,
    reason: &str,
) -> AgentSettingUpdateResult {
    AgentSettingUpdateResult {
        intent: change.intent,
        desired_value: change.desired_value.clone(),
        live_status: status,
        effective_value,
        reason: if change.intent == AgentSettingIntent::Unchanged && reason != "no_live_change" {
            "unchanged".to_string()
        } else {
            reason.to_string()
        },
    }
}

async fn prepare_agent_live_plan(
    config: &AgentConfig,
    model: &AgentSettingChange,
    effort: &AgentSettingChange,
) -> Result<AgentLivePlan, String> {
    if model.intent == AgentSettingIntent::Unchanged
        && effort.intent == AgentSettingIntent::Unchanged
    {
        return Ok(AgentLivePlan::NoLiveChange);
    }
    let provider = config.provider.trim().to_ascii_lowercase();
    if !provider.eq_ignore_ascii_case("codex") {
        let catalog = crate::providers::models::model_catalog(&provider, false).await;
        return match validate_provider_selection(
            &catalog,
            &provider,
            config.model.as_deref(),
            agent_reasoning_effort(config),
        ) {
            Ok(()) if config.is_off => Ok(AgentLivePlan::Deferred {
                reason: "agent_off".to_string(),
                restart_required: false,
            }),
            Ok(()) => Ok(AgentLivePlan::Deferred {
                reason: "provider_runtime_mismatch".to_string(),
                restart_required: true,
            }),
            Err(ProviderSelectionValidationError::DefaultUnavailable(reason)) => {
                Ok(AgentLivePlan::Deferred {
                    reason: format!("default_unresolved:{reason}"),
                    restart_required: true,
                })
            }
            Err(ProviderSelectionValidationError::Invalid(reason)) => Err(reason),
        };
    }

    if config.is_off {
        return Ok(AgentLivePlan::Deferred {
            reason: "agent_off".to_string(),
            restart_required: false,
        });
    }

    let catalog = crate::providers::models::model_catalog("codex", false).await;
    match resolve_live_selection_for_settings(
        &catalog,
        config.model.as_deref(),
        agent_reasoning_effort(config),
    ) {
        Ok(selection) => Ok(AgentLivePlan::Apply(selection)),
        Err(CodexLiveSelectionError::DefaultUnavailable(reason)) => Ok(AgentLivePlan::Deferred {
            reason: format!("default_unresolved:{reason}"),
            restart_required: false,
        }),
        Err(CodexLiveSelectionError::Invalid(reason)) => Err(reason),
    }
}

enum ProviderSelectionValidationError {
    DefaultUnavailable(String),
    Invalid(String),
}

fn validate_provider_selection(
    catalog: &ProviderModelCatalog,
    provider: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), ProviderSelectionValidationError> {
    let selected_model = match model.map(str::trim).filter(|value| !value.is_empty()) {
        Some(model) => catalog
            .models
            .iter()
            .find(|option| option.id == model)
            .ok_or_else(|| {
                ProviderSelectionValidationError::Invalid(format!(
                    "{provider} model {model} is not present in the provider catalog"
                ))
            })?,
        None => catalog
            .models
            .iter()
            .find(|option| option.is_default)
            .ok_or_else(|| {
                ProviderSelectionValidationError::DefaultUnavailable(
                    catalog.refresh_error.clone().unwrap_or_else(|| {
                        format!("{provider} catalog has no authoritative default model")
                    }),
                )
            })?,
    };

    if let Some(effort) = effort.map(str::trim).filter(|value| !value.is_empty()) {
        if !selected_model
            .effort_options
            .iter()
            .any(|option| option == effort)
        {
            return Err(ProviderSelectionValidationError::Invalid(format!(
                "{provider} model {} does not support reasoning effort {effort}",
                selected_model.id
            )));
        }
    }

    Ok(())
}

async fn apply_resolved_agent_model_selection_live(
    state: &AppState,
    session_id: &str,
    model: &AgentSettingChange,
    effort: &AgentSettingChange,
    selection: &CodexLiveModelSelection,
    launch_only_changed: bool,
) -> AgentLiveOutcome {
    let generation = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .and_then(|agent| agent.runtime_generation)
    };
    let Some(generation) = generation else {
        return AgentLiveOutcome {
            application: AgentModelLiveApplication::Deferred,
            error: None,
            reason: "runtime_unavailable".to_string(),
            model_status: setting_status(model, AgentSettingLiveStatus::Deferred),
            effort_status: setting_status(effort, AgentSettingLiveStatus::Deferred),
            model_effective: None,
            effort_effective: None,
            runtime_binding: None,
            restart_required: true,
        };
    };

    let binding = match state
        .native_delivery
        .codex_binding(session_id, generation)
        .await
    {
        Ok(binding) => AgentRuntimeBinding {
            agent_id: binding.wardian_agent_id,
            generation: binding.generation,
            thread_id: binding.provider_session_id,
            provider: "codex".to_string(),
        },
        Err(_) => {
            return AgentLiveOutcome {
                application: AgentModelLiveApplication::Deferred,
                error: None,
                reason: "runtime_provider_mismatch".to_string(),
                model_status: setting_status(model, AgentSettingLiveStatus::Deferred),
                effort_status: setting_status(effort, AgentSettingLiveStatus::Deferred),
                model_effective: None,
                effort_effective: None,
                runtime_binding: None,
                restart_required: true,
            };
        }
    };
    let update = state
        .native_delivery
        .codex_update_thread_settings(session_id, generation, &selection.model, &selection.effort)
        .await;
    match update {
        Ok(update) => AgentLiveOutcome {
            application: AgentModelLiveApplication::Applied,
            error: None,
            reason: "provider_acknowledged".to_string(),
            model_status: setting_status(model, AgentSettingLiveStatus::Applied),
            effort_status: setting_status(effort, AgentSettingLiveStatus::Applied),
            model_effective: Some(update.model),
            effort_effective: Some(update.effort),
            runtime_binding: Some(binding),
            restart_required: launch_only_changed,
        },
        Err(error) if error.code == "provider_rejected" => AgentLiveOutcome {
            application: AgentModelLiveApplication::Failed,
            error: Some(error.to_string()),
            reason: "provider_rejected".to_string(),
            model_status: setting_status(model, AgentSettingLiveStatus::Failed),
            effort_status: setting_status(effort, AgentSettingLiveStatus::Failed),
            model_effective: None,
            effort_effective: None,
            runtime_binding: Some(binding),
            restart_required: launch_only_changed,
        },
        Err(error) if error.code == "unsupported" => AgentLiveOutcome {
            application: AgentModelLiveApplication::Deferred,
            error: None,
            reason: "runtime_changed".to_string(),
            model_status: setting_status(model, AgentSettingLiveStatus::Deferred),
            effort_status: setting_status(effort, AgentSettingLiveStatus::Deferred),
            model_effective: None,
            effort_effective: None,
            runtime_binding: Some(binding),
            restart_required: true,
        },
        Err(error) => AgentLiveOutcome {
            application: AgentModelLiveApplication::Unknown,
            error: Some(error.to_string()),
            reason: "runtime_changed_after_submit".to_string(),
            model_status: setting_status(model, AgentSettingLiveStatus::Unknown),
            effort_status: setting_status(effort, AgentSettingLiveStatus::Unknown),
            model_effective: None,
            effort_effective: None,
            runtime_binding: Some(binding),
            restart_required: launch_only_changed,
        },
    }
}

fn launch_only_config_changed(current: &AgentConfig, desired: &AgentConfig) -> bool {
    let mut current = serde_json::to_value(current).unwrap_or_default();
    let mut desired = serde_json::to_value(desired).unwrap_or_default();
    for value in [&mut current, &mut desired] {
        if let Some(object) = value.as_object_mut() {
            object.remove("session_name");
            object.remove("description");
            object.remove("model");
            if let Some(provider_config) = object
                .get_mut("provider_config")
                .and_then(serde_json::Value::as_object_mut)
            {
                provider_config.remove("reasoning_effort");
            }
        }
    }
    current != desired
}

pub(crate) fn normalized_optional_agent_setting(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::models::ProviderModelOption;

    fn catalog(provider: &str) -> ProviderModelCatalog {
        ProviderModelCatalog {
            provider: provider.to_string(),
            version: Some("test-provider".to_string()),
            source: "provider_aliases".to_string(),
            models: vec![ProviderModelOption {
                id: "target".to_string(),
                display_name: "Target".to_string(),
                effort_options: vec!["low".to_string(), "high".to_string()],
                default_effort: Some("low".to_string()),
                is_default: true,
            }],
            refresh_error: None,
        }
    }

    #[test]
    fn complete_config_json_normalizes_sets_and_preserves_null_defaults() {
        let set: Option<String> = serde_json::from_value(serde_json::json!("  target  "))
            .expect("JSON string should deserialize");
        assert_eq!(
            normalize_complete_setting(set, "model").expect("trimmed set"),
            Some("target".to_string())
        );

        let default: Option<String> =
            serde_json::from_value(serde_json::Value::Null).expect("JSON null should deserialize");
        assert_eq!(
            normalize_complete_setting(default, "model").expect("default should remain valid"),
            None
        );

        let empty: Option<String> = serde_json::from_value(serde_json::json!(" \t "))
            .expect("JSON whitespace string should deserialize");
        assert_eq!(
            normalize_complete_setting(empty, "model").expect_err("empty Set must reject"),
            "model cannot be empty"
        );
    }

    #[test]
    fn complete_config_normalization_updates_the_actual_candidate() {
        let mut config = AgentConfig {
            provider: "codex".to_string(),
            provider_config: ProviderConfig::Codex(Default::default()),
            model: Some("  target  ".to_string()),
            ..Default::default()
        };
        if let ProviderConfig::Codex(provider_config) = &mut config.provider_config {
            provider_config.reasoning_effort = Some(" high ".to_string());
        }

        normalize_complete_config_settings(&mut config).expect("complete config should normalize");

        assert_eq!(config.model.as_deref(), Some("target"));
        assert_eq!(agent_reasoning_effort(&config), Some("high"));

        config.model = Some(" \t ".to_string());
        assert_eq!(
            normalize_complete_config_settings(&mut config).expect_err("empty model must reject"),
            "model cannot be empty"
        );
    }

    #[test]
    fn non_codex_selection_uses_the_provider_catalog_before_deferred_restart() {
        let catalog = catalog("claude");
        assert!(
            validate_provider_selection(&catalog, "claude", Some("target"), Some("high")).is_ok()
        );
        assert!(matches!(
            validate_provider_selection(&catalog, "claude", Some("missing"), None),
            Err(ProviderSelectionValidationError::Invalid(_))
        ));
        assert!(matches!(
            validate_provider_selection(&catalog, "claude", Some("target"), Some("xhigh")),
            Err(ProviderSelectionValidationError::Invalid(_))
        ));
    }

    #[test]
    fn settings_snapshot_revalidation_covers_config_and_runtime_generation() {
        let config = AgentConfig::default();
        let same = AgentSettingsSnapshot {
            config: config.clone(),
            runtime_generation: Some(4),
        };
        assert!(settings_snapshot_matches(&same, &same).expect("snapshot comparison"));

        let mut changed_config = config;
        changed_config.provider = "codex".to_string();
        let changed = AgentSettingsSnapshot {
            config: changed_config,
            runtime_generation: Some(4),
        };
        assert!(!settings_snapshot_matches(&same, &changed).expect("config comparison"));

        let changed_generation = AgentSettingsSnapshot {
            config: same.config.clone(),
            runtime_generation: Some(5),
        };
        assert!(
            !settings_snapshot_matches(&same, &changed_generation).expect("generation comparison")
        );
    }
}
