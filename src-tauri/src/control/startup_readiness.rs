use super::{
    antigravity_output_has_ready_prompt, gemini_output_has_api_key_prompt,
    pi_output_has_startup_ready_prompt,
};
use crate::providers::claude::claude_output_has_bypass_permissions_consent_prompt;
use crate::state::terminal_session::{StartupScreenCommitResult, TerminalReadinessObservation};
use crate::state::AppState;
use crate::utils::strip_ansi_controls;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use wardian_core::control::{ProviderInputReadiness, ProviderReadyEvidence};

pub(super) async fn record_provider_ready_evidence(
    state: &AppState,
    session_id: &str,
    generation: u64,
    evidence: ProviderReadyEvidence,
) -> bool {
    let attachment_ready = state
        .agents
        .lock()
        .await
        .get(session_id)
        .map(crate::manager::codex_onboarding::codex_attachment_is_ready)
        .unwrap_or(true);
    if !attachment_ready {
        return false;
    }
    let recorded = state
        .interactions
        .record_provider_input_state(
            session_id,
            generation,
            ProviderInputReadiness::Ready,
            Some(evidence),
        )
        .await;
    recorded.generation == generation && recorded.state == ProviderInputReadiness::Ready
}

pub(super) async fn ensure_codex_attachment_ready(
    state: &AppState,
    provider: &str,
    session_id: &str,
) -> Result<(), String> {
    if provider == "codex"
        && !state
            .agents
            .lock()
            .await
            .get(session_id)
            .is_some_and(crate::manager::codex_onboarding::codex_attachment_is_ready)
    {
        return Err(format!(
            "Agent {session_id} Codex attachment is still starting; provider input is not ready"
        ));
    }
    Ok(())
}

/// Records startup readiness only after the provider has rendered its own
/// interactive prompt. This is deliberately separate from an `Idle` status:
/// a newly spawned process is not safe to receive mailbox input merely because
/// Wardian has not yet observed it doing work.
pub(crate) async fn record_provider_ready_prompt(
    state: &AppState,
    session_id: &str,
    generation: u64,
) -> bool {
    record_provider_ready_evidence(
        state,
        session_id,
        generation,
        ProviderReadyEvidence::PromptDetected,
    )
    .await
}

/// Retains title evidence for callers that also validate the current composer.
/// An OpenCode title alone cannot authorize startup publication or delivery.
pub(crate) async fn record_provider_ready_title(
    state: &AppState,
    session_id: &str,
    generation: u64,
) -> bool {
    record_provider_ready_evidence(
        state,
        session_id,
        generation,
        ProviderReadyEvidence::TitleDetected,
    )
    .await
}

/// The input generation and status identity belong to the runtime that
/// observed the screen, never to whichever runtime executes its queued task.
#[derive(Clone)]
pub(crate) struct ProviderStartupObservation {
    pub input_generation: u64,
    pub runtime_generation: u64,
    pub current_status: Arc<Mutex<String>>,
    pub startup_action_intent_revision: Option<u64>,
}

impl ProviderStartupObservation {
    async fn is_current_with_status(
        &self,
        state: &AppState,
        session_id: &str,
        expected_status: Option<&str>,
    ) -> bool {
        let Some((runtime_generation, current_status)) = ({
            let agents = state.agents.lock().await;
            agents
                .get(session_id)
                .map(|agent| (agent.runtime_generation, agent.current_status.clone()))
        }) else {
            return false;
        };
        runtime_generation == Some(self.runtime_generation)
            && Arc::ptr_eq(&current_status, &self.current_status)
            && expected_status.is_none_or(|expected| {
                current_status
                    .lock()
                    .is_ok_and(|status| status.eq_ignore_ascii_case(expected))
            })
    }

    async fn is_current(&self, state: &AppState, session_id: &str) -> bool {
        self.is_current_with_status(state, session_id, Some("idle"))
            .await
    }
}

/// Serializes publication with replacement. The drain is scheduled only while
/// this observation still owns the runtime; its ordinary readiness checks also
/// protect against a replacement after scheduling.
pub(crate) async fn publish_startup_readiness(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
    evidence: ProviderReadyEvidence,
) -> bool {
    let _lifecycle = state.lock_agent_lifecycle(session_id).await;
    publish_startup_readiness_locked(app, state, session_id, observation, evidence).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupReadinessRejection {
    RuntimeChanged,
    StatusOrOwnerChanged,
    BlockingAction,
    OpposingInput,
    InputOrStatusChanged,
    RepaintDidNotSettle,
    BrokerUnavailable,
}

impl StartupReadinessRejection {
    pub(crate) fn diagnostic_code(self) -> &'static str {
        match self {
            Self::RuntimeChanged => "runtime_changed",
            Self::StatusOrOwnerChanged => "status_or_owner_changed",
            Self::BlockingAction => "blocking_action",
            Self::OpposingInput => "opposing_input",
            Self::InputOrStatusChanged => "input_or_status_changed",
            Self::RepaintDidNotSettle => "repaint_did_not_settle",
            Self::BrokerUnavailable => "broker_unavailable",
        }
    }
}

const CLAUDE_READY_REOBSERVATION_LIMIT: usize = 3;
const CLAUDE_READY_REOBSERVATION_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(20);

/// Commits confirmed Claude readiness at the terminal actor's serialized
/// screen/input boundary. The provider generation fence is acquired before
/// asking the actor to commit, so an admitted input can finish without an
/// actor-to-interaction lock cycle. Idle becomes visible only after the input
/// proof has been durably recorded.
pub(crate) async fn publish_startup_readiness_from_action_needed<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
    initial: TerminalReadinessObservation,
    owner_can_commit: Arc<dyn Fn() -> bool + Send + Sync>,
    on_committed: Arc<dyn Fn() + Send + Sync>,
) -> Result<(), StartupReadinessRejection> {
    let _lifecycle = state.lock_agent_lifecycle(session_id).await;
    let Some(expected_status_intent_revision) = observation.startup_action_intent_revision else {
        return Err(StartupReadinessRejection::StatusOrOwnerChanged);
    };
    if state.status_intent_revision(session_id, &observation.current_status)
        != expected_status_intent_revision
        || !state
            .status_intent_status(session_id, &observation.current_status)
            .is_some_and(|intent| intent.eq_ignore_ascii_case("Action Needed"))
    {
        return Err(StartupReadinessRejection::StatusOrOwnerChanged);
    }
    let mut expected = initial;
    for attempt in 0..CLAUDE_READY_REOBSERVATION_LIMIT {
        if !observation
            .is_current_with_status(state, session_id, Some("Action Needed"))
            .await
        {
            return Err(StartupReadinessRejection::StatusOrOwnerChanged);
        }
        if expected.snapshot.runtime_generation != observation.runtime_generation {
            return Err(StartupReadinessRejection::RuntimeChanged);
        }

        let screen = &expected.snapshot.visible_grid;
        if !provider_output_has_startup_ready_prompt("claude", screen) {
            if provider_output_requires_startup_action("claude", screen)
                || claude_output_has_bypass_permissions_consent_prompt(screen)
            {
                return Err(StartupReadinessRejection::BlockingAction);
            }
            if attempt + 1 == CLAUDE_READY_REOBSERVATION_LIMIT {
                return Err(StartupReadinessRejection::RepaintDidNotSettle);
            }
            tokio::time::sleep(CLAUDE_READY_REOBSERVATION_INTERVAL).await;
            expected = state
                .terminal_sessions
                .readiness_observation(session_id)
                .await
                .map_err(|_| StartupReadinessRejection::BrokerUnavailable)?;
            continue;
        }

        let Some(input_commit) = state
            .interactions
            .prepare_provider_ready_after_startup_action(session_id, observation.input_generation)
            .await
        else {
            return Err(StartupReadinessRejection::InputOrStatusChanged);
        };
        let commit_app = app.clone();
        let commit_session_id = session_id.to_string();
        let commit_status = observation.current_status.clone();
        let commit_runtime_generation = observation.runtime_generation;
        let commit_expected_status_intent_revision = expected_status_intent_revision;
        let commit_owner_can_commit = owner_can_commit.clone();
        let commit_on_committed = on_committed.clone();
        let result = state
            .terminal_sessions
            .commit_if_readiness_observation_current(expected.clone(), move || {
                if !commit_owner_can_commit() {
                    return false;
                }
                let commit_state = commit_app.state::<AppState>();
                let current_agent = {
                    let Ok(agents) = commit_state.agents.try_lock() else {
                        return false;
                    };
                    agents.get(&commit_session_id).map(|agent| {
                        (
                            agent.runtime_generation,
                            agent.current_status.clone(),
                            agent.config.clone(),
                        )
                    })
                };
                let Some((current_generation, current_status, current_config)) = current_agent
                else {
                    return false;
                };
                let is_current = current_generation == Some(commit_runtime_generation)
                    && Arc::ptr_eq(&current_status, &commit_status)
                    && current_status
                        .lock()
                        .is_ok_and(|status| status.eq_ignore_ascii_case("Action Needed"))
                    && current_config
                        .lock()
                        .is_ok_and(|config| config.provider == "claude");
                if !is_current {
                    return false;
                }
                // The roster snapshot above proves this is Claude, which has no
                // Codex attachment admission gate.
                let Some((status_sequence, status_revision)) =
                    crate::manager::apply_admitted_status_transition_if_revision_after_commit(
                        commit_state.inner(),
                        &commit_session_id,
                        &commit_status,
                        "Action Needed",
                        commit_expected_status_intent_revision,
                        "Idle",
                        move || input_commit.commit(),
                    )
                else {
                    return false;
                };
                crate::manager::schedule_startup_readiness_status_observation(
                    &commit_app,
                    &commit_session_id,
                    &commit_status,
                    "Idle".to_string(),
                    status_sequence,
                    status_revision,
                );
                commit_on_committed();
                true
            })
            .await;
        match result {
            Ok(StartupScreenCommitResult::Committed) => return Ok(()),
            Ok(StartupScreenCommitResult::InputChanged) => {
                return Err(StartupReadinessRejection::OpposingInput)
            }
            Ok(StartupScreenCommitResult::CommitRejected) => {
                return Err(StartupReadinessRejection::InputOrStatusChanged)
            }
            Ok(StartupScreenCommitResult::ScreenChanged) => {
                if attempt + 1 == CLAUDE_READY_REOBSERVATION_LIMIT {
                    return Err(StartupReadinessRejection::RepaintDidNotSettle);
                }
                expected = state
                    .terminal_sessions
                    .readiness_observation(session_id)
                    .await
                    .map_err(|_| StartupReadinessRejection::BrokerUnavailable)?;
            }
            Err(crate::state::terminal_session::TerminalBrokerError::StaleRuntimeGeneration {
                ..
            }) => return Err(StartupReadinessRejection::RuntimeChanged),
            Err(_) => return Err(StartupReadinessRejection::BrokerUnavailable),
        }
    }
    Err(StartupReadinessRejection::RepaintDidNotSettle)
}

/// Publish a delayed OpenCode startup observation only while it still owns a
/// Starting runtime. Revalidate the current composer on both sides of the
/// status transition, restoring Starting if the runtime or screen changes.
pub(crate) async fn publish_startup_readiness_from_starting<Validate, Validation, SetStatus>(
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
    evidence: ProviderReadyEvidence,
    mut validate_ready: Validate,
    mut set_status: SetStatus,
) -> bool
where
    Validate: FnMut() -> Validation + Send,
    Validation: std::future::Future<Output = bool> + Send,
    SetStatus: FnMut(&str) + Send,
{
    let _lifecycle = state.lock_agent_lifecycle(session_id).await;
    if !observation
        .is_current_with_status(state, session_id, Some("Starting"))
        .await
        || !validate_ready().await
    {
        return false;
    }

    set_status("Idle");
    if !validate_ready().await {
        set_status("Starting");
        return false;
    }
    if publish_startup_readiness_locked(None, state, session_id, observation, evidence).await {
        true
    } else {
        set_status("Starting");
        false
    }
}

async fn publish_startup_readiness_locked(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
    observation: &ProviderStartupObservation,
    evidence: ProviderReadyEvidence,
) -> bool {
    if !observation.is_current(state, session_id).await {
        return false;
    }
    let config = {
        let agents = state.agents.lock().await;
        agents.get(session_id).map(|agent| agent.config.clone())
    };
    let Some(config) = config else {
        return false;
    };
    let opencode = match config.lock() {
        Ok(config) => config.provider == "opencode",
        Err(_) => return false,
    };
    if opencode
        && !opencode_current_screen_is_ready(state, session_id)
            .await
            .unwrap_or(false)
    {
        return false;
    }
    let ready = match evidence {
        ProviderReadyEvidence::PromptDetected => {
            record_provider_ready_prompt(state, session_id, observation.input_generation).await
        }
        ProviderReadyEvidence::TitleDetected => {
            record_provider_ready_title(state, session_id, observation.input_generation).await
        }
        _ => false,
    };
    if !ready || !observation.is_current(state, session_id).await {
        return false;
    }
    if let Some(app) = app {
        super::spawn_agent_messaging_if_idle(app, session_id, "Idle");
    }
    true
}

/// Recognizes initial compose readiness. Codex, Claude, OpenCode and Pi callers
/// must supply the canonical visible screen: raw chunks can omit startup blockers, while
/// accumulated output retains blockers that a later repaint already removed.
pub(crate) fn provider_output_has_startup_ready_prompt(provider: &str, output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    match provider {
        "opencode" => {
            let lines = cleaned
                .lines()
                .map(|line| {
                    line.trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '┃' | '│'))
                })
                .collect::<Vec<_>>();
            let has_placeholder_composer = lines
                .iter()
                .rposition(|line| line.starts_with("Ask anything"))
                .map(|composer| {
                    let footer = lines[composer + 1..].join(" ");
                    let footer = footer.split_whitespace().collect::<Vec<_>>().join(" ");
                    footer.contains("ctrl+p commands")
                })
                .unwrap_or(false);
            let has_restored_composer = opencode_has_restored_composer(&lines);
            !provider_output_requires_startup_action(provider, &cleaned)
                && !lines.iter().any(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower.starts_with("loading") || lower.starts_with("connecting")
                })
                && (has_placeholder_composer || has_restored_composer)
        }
        "codex" => {
            !provider_output_requires_startup_action("codex", &cleaned)
                && !crate::delivery::codex_composer::output_has_workspace_trust_prompt(&cleaned)
                && cleaned.contains('›')
                && !crate::delivery::codex_composer::active_screen_is_starting(&cleaned)
        }
        "claude" => {
            let Some((_, after_prompt)) = cleaned.rsplit_once('❯') else {
                return false;
            };
            // A selection menu or partial initial paint also contains ❯.
            // Require the composer footer below it, not text in the draft.
            let footer = after_prompt.lines().skip(1).collect::<Vec<_>>().join(" ");
            let compact = footer
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>();
            !provider_output_requires_startup_action("claude", &cleaned)
                && !claude_output_has_bypass_permissions_consent_prompt(&cleaned)
                && (footer.contains("shift+tab to cycle") || footer.contains("? for shortcuts"))
                && !compact.contains("rcconnecting")
        }
        "gemini" => {
            !gemini_output_has_api_key_prompt(&cleaned)
                && cleaned.contains("Type your message or @path/to/file")
        }
        "antigravity" => antigravity_output_has_ready_prompt(&cleaned),
        "pi" => pi_output_has_startup_ready_prompt(&cleaned),
        _ => false,
    }
}

/// OpenCode removes the placeholder text while a resumed conversation is on
/// screen. Its empty composer is still represented by the bottom border, but
/// the fixed-width footer can wrap `ctrl+p` and `commands` onto adjacent rows
/// when the workspace path occupies the left side of the terminal.
fn opencode_has_restored_composer(lines: &[&str]) -> bool {
    let Some(border_index) = lines.iter().rposition(|line| {
        let line = line.trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '┃' | '│'));
        line.starts_with('╹') && line.contains("▀▀")
    }) else {
        return false;
    };

    let footer = &lines[border_index + 1..];
    let Some(ctrl_row) = footer.iter().rposition(|line| line.contains("ctrl+p")) else {
        return false;
    };
    let provider_row_limit = (ctrl_row + 3).min(footer.len());
    let footer_rows = &footer[ctrl_row..provider_row_limit];
    let has_commands = footer_rows.iter().enumerate().any(|(offset, line)| {
        if offset == 0 {
            line.split_once("ctrl+p")
                .is_some_and(|(_, after)| after.contains("commands"))
        } else {
            line.contains("commands")
        }
    });
    let legacy_footer = footer_rows.iter().any(|line| line.contains("OpenCode"));
    // Current OpenCode places provider/model metadata inside the composer,
    // while the commands footer contains only navigation controls. Narrow
    // terminals can clip the agents label; require an empty padding row above
    // the metadata instead of depending on that optional navigation label.
    let model_row = border_index
        .checked_sub(1)
        .and_then(|index| lines.get(index));
    let model_footer = model_row.is_some_and(|line| {
        let fields = line.split('·').map(str::trim).collect::<Vec<_>>();
        fields.len() == 3 && fields.iter().all(|field| !field.is_empty())
    }) && border_index
        .checked_sub(2)
        .and_then(|index| lines.get(index))
        .is_some_and(|line| line.is_empty());
    has_commands && (legacy_footer || model_footer)
}

#[cfg(test)]
#[test]
fn restored_opencode_model_footer_is_ready_without_brand_text() {
    let ready = "Previous response\n┃\n┃\n┃ Build · GPT-5.6 Luna OpenAI · high\n╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n<workspace-root>/project  tab  ctrl+p\nagents commands";
    assert!(provider_output_has_startup_ready_prompt("opencode", ready));
    let narrow = ready.replace("tab  ctrl+p\nagents commands", "12.0K (3 ctrl+p\ncommands");
    assert!(provider_output_has_startup_ready_prompt(
        "opencode", &narrow
    ));
    assert!(!provider_output_has_startup_ready_prompt(
        "opencode",
        &narrow.replace("┃\n┃ Build", "┃ draft\n┃ Build")
    ));
    assert!(!provider_output_has_startup_ready_prompt(
        "opencode",
        &format!("Loading session...\n{ready}")
    ));
    assert!(!provider_output_has_startup_ready_prompt(
        "opencode",
        &ready.replace('╹', " ")
    ));
    assert!(!provider_output_has_startup_ready_prompt(
        "opencode",
        &ready.replace("Build · GPT-5.6 Luna OpenAI · high", "Quoted commands")
    ));
}

const CLAUDE_WORKSPACE_TRUST_QUESTION: &str =
    "Quick safety check: Is this a project you created or one you trust?";
const CLAUDE_WORKSPACE_TRUST_QUESTION_2_1_283: &str = "Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.";
const CLAUDE_WORKSPACE_TRUST_QUESTION_MAX_ROWS: usize = 3;

fn claude_workspace_trust_question_row_text(line: &str) -> &str {
    line.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '│' | '┃' | '|')
    })
}

fn claude_trust_option_text(line: &str) -> &str {
    let line = line.trim();
    line.strip_prefix('❯').unwrap_or(line).trim()
}

pub(crate) fn claude_workspace_trust_question_span(lines: &[&str], index: usize) -> Option<usize> {
    for row_count in 1..=CLAUDE_WORKSPACE_TRUST_QUESTION_MAX_ROWS {
        let Some(rows) = lines.get(index..index + row_count) else {
            break;
        };
        if rows
            .iter()
            .any(|line| claude_workspace_trust_question_row_text(line).is_empty())
        {
            break;
        }
        let normalized = rows
            .iter()
            .map(|line| {
                claude_workspace_trust_question_row_text(line)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" ");
        if (row_count == 1 && normalized.eq_ignore_ascii_case(CLAUDE_WORKSPACE_TRUST_QUESTION))
            || normalized.eq_ignore_ascii_case(CLAUDE_WORKSPACE_TRUST_QUESTION_2_1_283)
        {
            return Some(row_count);
        }
    }
    None
}

/// Recognizes Claude Code's folder-trust dialog on a current terminal screen.
/// The option labels and question must be present together so ordinary project
/// text cannot turn a managed startup into an approval state.
fn claude_workspace_trust_prompt_indices(lines: &[&str]) -> Option<(usize, usize, usize)> {
    let question = lines
        .iter()
        .enumerate()
        .find_map(|(index, _)| claude_workspace_trust_question_span(lines, index).map(|_| index));
    let no_exit = lines
        .iter()
        .position(|line| claude_trust_option_text(line).eq_ignore_ascii_case("No, exit"));
    let yes_trust = lines.iter().position(|line| {
        claude_trust_option_text(line).eq_ignore_ascii_case("Yes, I trust this folder")
    });
    match (question, no_exit, yes_trust) {
        (Some(question), Some(no_exit), Some(yes_trust))
            if question < no_exit && no_exit < yes_trust =>
        {
            Some((question, no_exit, yes_trust))
        }
        _ => None,
    }
}

pub(crate) fn claude_workspace_trust_prompt_is_current(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output);
    let lines = cleaned.lines().collect::<Vec<_>>();
    claude_workspace_trust_prompt_indices(&lines).is_some()
}

/// Auto-confirmation is safe only while Claude still has the observed default
/// selection. Any changed selection remains visible as Action Needed.
pub(crate) fn claude_workspace_trust_prompt_selects_no(output: &str) -> bool {
    claude_workspace_trust_prompt_selection(output) == Some((true, false))
}

/// Returns whether the No and Yes rows are selected in one exact current trust
/// prompt. Keeping both marker states lets callers fail closed during redraws.
pub(crate) fn claude_workspace_trust_prompt_selection(output: &str) -> Option<(bool, bool)> {
    let cleaned = strip_ansi_controls(output);
    let lines = cleaned.lines().collect::<Vec<_>>();
    let (_, no_exit, yes_trust) = claude_workspace_trust_prompt_indices(&lines)?;
    let selected = |index: usize| lines[index].trim().starts_with('❯');
    Some((selected(no_exit), selected(yes_trust)))
}

/// Provider startup can require an explicit account or workspace decision
/// before a compose prompt exists. Keep that state visible and prevent queued
/// delivery from being mistaken for a prompt the provider can receive.
pub(crate) fn provider_output_requires_startup_action(provider: &str, output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).to_ascii_lowercase();
    match provider {
        "claude" => {
            cleaned.contains("allow external claude.md file imports?")
                || claude_workspace_trust_prompt_is_current(output)
        }
        "codex" => crate::delivery::codex_menu::current_screen_requires_choice(
            &strip_ansi_controls(output),
        ),
        "antigravity" => cleaned.contains("do you trust the contents of this project?"),
        "opencode" => cleaned.contains("permission required") || cleaned.contains("do you trust"),
        _ => false,
    }
}

/// Validates the current OpenCode composer, never retained watch output or a
/// generic title. A missing/replaced terminal cannot authorize prompt bytes.
pub(crate) async fn opencode_current_screen_is_ready(
    state: &AppState,
    session_id: &str,
) -> Result<bool, String> {
    let generation = state
        .agents
        .lock()
        .await
        .get(session_id)
        .and_then(|agent| agent.runtime_generation)
        .ok_or_else(|| "OpenCode runtime identity unavailable before input".to_string())?;
    let snapshot = state
        .terminal_sessions
        .snapshot(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let current_generation = state
        .agents
        .lock()
        .await
        .get(session_id)
        .and_then(|agent| agent.runtime_generation);
    Ok(snapshot.runtime_generation == generation
        && current_generation == Some(generation)
        && provider_output_has_startup_ready_prompt("opencode", &snapshot.visible_grid))
}

/// Read the current runtime's canonical screen before trusting a cached Ready
/// observation. Missing or replacement-runtime evidence cannot authorize input.
pub(crate) async fn codex_current_screen_requires_choice(
    state: &AppState,
    session_id: &str,
) -> Result<bool, String> {
    let generation = state
        .agents
        .lock()
        .await
        .get(session_id)
        .and_then(|agent| agent.runtime_generation)
        .ok_or_else(|| "Codex runtime identity unavailable before input".to_string())?;
    let snapshot = state
        .terminal_sessions
        .snapshot(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let current_generation = state
        .agents
        .lock()
        .await
        .get(session_id)
        .and_then(|agent| agent.runtime_generation);
    if snapshot.runtime_generation != generation || current_generation != Some(generation) {
        return Err("Codex terminal runtime changed before input".to_string());
    }
    Ok(crate::delivery::codex_menu::current_screen_requires_choice(
        &snapshot.visible_grid,
    ))
}

/// A delayed completion/log event cannot dismiss a still-visible model menu.
/// The caller holds the lifecycle lock and rechecks status-Arc identity before
/// publishing. No provider selection is made and no historical output is read.
pub(crate) async fn constrain_codex_status_observation(
    state: &AppState,
    session_id: &str,
    status: &str,
) -> Option<&'static str> {
    if !matches!(
        wardian_core::identity::normalize_status(status).as_str(),
        "idle" | "processing"
    ) {
        return None;
    }
    let codex = state
        .agents
        .lock()
        .await
        .get(session_id)
        .is_some_and(|agent| {
            agent
                .config
                .lock()
                .is_ok_and(|config| config.provider == "codex")
        });
    if codex
        && codex_current_screen_requires_choice(state, session_id)
            .await
            // A failed or stale snapshot cannot prove that a choice was dismissed.
            .unwrap_or(true)
    {
        Some("Action Needed")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_startup_composer_rejects_partial_loading_and_consent_screens() {
        let ready = "Ask anything...\nBuild  mimo-v2.5-free\nctrl+p commands";
        assert!(provider_output_has_startup_ready_prompt("opencode", ready));

        // A resumed conversation can replace the placeholder with an empty
        // composer border. The fixed-width footer may wrap around the
        // workspace path, so `ctrl+p` and `commands` can occupy adjacent rows.
        let mut composer_border = "  ╹".to_string();
        composer_border.extend(std::iter::repeat_n(
            '▀',
            190 - composer_border.chars().count(),
        ));
        let footer_row = |suffix: &str| {
            let mut row = format!("{:<133}{}", "<workspace>/long-path", suffix);
            row.extend(std::iter::repeat_n(' ', 190 - row.chars().count()));
            row
        };
        let ctrl_row = footer_row("ctrl+p");
        let commands_row = footer_row("commands • OpenCode 1.18.30");
        assert_eq!(composer_border.chars().count(), 190);
        assert_eq!(ctrl_row.find("ctrl+p"), Some(133));
        assert_eq!(commands_row.find("commands"), Some(133));
        let mut restored_rows = vec![String::new(); 51];
        restored_rows[1] = "New session - resumed".to_string();
        restored_rows[2] = "USER_marker".to_string();
        restored_rows[10] = "Build · MiMo V2.5 Free".to_string();
        restored_rows[30] = "Getting started".to_string();
        restored_rows[47] = composer_border;
        restored_rows[48] = ctrl_row;
        restored_rows[49] = commands_row;
        let restored = restored_rows.join("\n");
        assert_eq!(restored_rows.len(), 51);
        assert!(provider_output_has_startup_ready_prompt(
            "opencode", &restored
        ));
        for blocked in [
            "OpenCode",
            "Ask anything...",
            "ctrl+p commands",
            "Ask anything...\nLoading session...\nctrl+p commands",
            "Permission required\nAsk anything...\nctrl+p commands",
            "Do you trust this directory?\nAsk anything...\nctrl+p commands",
            "Loading session...\n╹▀▀▀▀▀▀▀▀▀▀\nctrl+p commands • OpenCode 1.18.30",
            "╹▀▀▀▀▀▀▀▀▀▀\nctrl+p commands",
            "Restored transcript\nUSER_marker\ncommands • OpenCode 1.18.30",
        ] {
            assert!(
                !provider_output_has_startup_ready_prompt("opencode", blocked),
                "{blocked}"
            );
        }
    }

    #[tokio::test]
    async fn current_rate_limit_screen_blocks_payload_and_late_idle_until_repaint() {
        use super::super::{test_support::TestWardianHome, tests::insert_test_agent};
        use crate::delivery::{submit_live_surface_prompt, LiveSurfacePromptRequest};
        use crate::state::terminal_session::TerminalRuntimeHandles;
        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        insert_test_agent(&state, "menu-agent", "Menu", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                "menu-agent",
                TerminalRuntimeHandles::new(tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry {
                    cols: 190,
                    rows: 51,
                },
            )
            .await
            .unwrap();
        {
            let mut agents = state.agents.lock().await;
            let agent = agents.get_mut("menu-agent").unwrap();
            agent.runtime_generation = Some(generation);
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let retained = include_str!("../delivery/fixtures/codex-rate-limit-menu.txt");
        let terminal = state.terminal_sessions.clone();
        let bytes = format!("\x1b[2J\x1b[H{}", retained.replace('\n', "\r\n")).into_bytes();
        tokio::task::spawn_blocking(move || {
            terminal.process_output_blocking("menu-agent", generation, bytes)
        })
        .await
        .unwrap()
        .unwrap();
        record_provider_ready_evidence(
            &state,
            "menu-agent",
            0,
            ProviderReadyEvidence::ProviderEvent,
        )
        .await;
        let result = submit_live_surface_prompt(
            None,
            &state,
            LiveSurfacePromptRequest::message("menu-agent", "must not enter model menu"),
        )
        .await;
        assert!(result
            .unwrap_err()
            .message
            .contains("explicit Codex model choice"));
        assert!(
            rx.try_recv().is_err(),
            "No payload, Return, or model-choice keys may be written"
        );
        assert_eq!(
            constrain_codex_status_observation(&state, "menu-agent", "Idle").await,
            Some("Action Needed")
        );
        assert_eq!(
            constrain_codex_status_observation(&state, "menu-agent", "Processing...").await,
            Some("Action Needed")
        );
        let terminal = state.terminal_sessions.clone();
        tokio::task::spawn_blocking(move || {
            terminal.process_output_blocking(
                "menu-agent",
                generation,
                b"\x1b[2J\x1b[H\xe2\x80\xba Ask Codex to do anything".to_vec(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert!(!codex_current_screen_requires_choice(&state, "menu-agent")
            .await
            .unwrap());
        assert_eq!(
            constrain_codex_status_observation(&state, "menu-agent", "Idle").await,
            None
        );
    }

    #[tokio::test]
    async fn stale_startup_generation_cannot_ready_replacement() {
        let _home = super::super::test_support::TestWardianHome::new_async().await;
        let state = crate::state::AppState::new();
        let old = state
            .interactions
            .start_provider_input_generation(
                "startup-race",
                wardian_core::control::ProviderInputReadiness::Booting,
                None,
            )
            .await;
        let replacement = state
            .interactions
            .start_provider_input_generation(
                "startup-race",
                wardian_core::control::ProviderInputReadiness::Booting,
                None,
            )
            .await;
        assert_ne!(old.generation, replacement.generation);
        // The old reader's task resumes after a new Booting generation exists.
        super::record_provider_ready_prompt(&state, "startup-race", old.generation).await;
        assert_eq!(
            state
                .interactions
                .provider_input_state("startup-race")
                .await
                .unwrap(),
            replacement
        );
    }

    #[tokio::test]
    async fn claude_trust_watcher_publishes_ready_screen_after_reader_consumed_final_output() {
        use super::super::{test_support::TestWardianHome, tests::insert_test_agent};
        use crate::state::terminal_session::TerminalRuntimeHandles;
        use tauri::Manager;
        use tokio::sync::mpsc;
        use wardian_core::control::ProviderInputReadiness;

        let _home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        const SESSION_ID: &str = "claude-trust-handoff";
        insert_test_agent(&state, SESSION_ID, "ClaudeTrust", "Coder").await;
        let (tx, mut input_rx) = mpsc::channel(1);
        let runtime_generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                SESSION_ID,
                TerminalRuntimeHandles::new(tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry {
                    rows: 24,
                    cols: 120,
                },
            )
            .await
            .unwrap();
        let input_generation = state
            .interactions
            .start_provider_input_generation(SESSION_ID, ProviderInputReadiness::Booting, None)
            .await
            .generation;
        state
            .interactions
            .record_provider_input_state(
                SESSION_ID,
                input_generation,
                ProviderInputReadiness::ActionRequired,
                None,
            )
            .await;
        let current_status = {
            let mut agents = state.agents.lock().await;
            let agent = agents.get_mut(SESSION_ID).unwrap();
            agent.runtime_generation = Some(runtime_generation);
            agent.config.lock().unwrap().provider = "claude".to_string();
            *agent.current_status.lock().unwrap() = "Action Needed".to_string();
            agent.current_status.clone()
        };
        let startup_action_intent_revision = {
            let _status = current_status.lock().unwrap();
            state.reserve_status_intent(SESSION_ID, &current_status, "Action Needed")
        };
        let observation = ProviderStartupObservation {
            input_generation,
            runtime_generation,
            current_status: current_status.clone(),
            startup_action_intent_revision: Some(startup_action_intent_revision),
        };
        let output = "Claude Code v2.1.283\n❯ Try ask Claude\n────────\nHaiku 4.5 | workspace | /rc\n⏵⏵ bypass permissions on (shift+tab to cycle)";
        let terminal = state.terminal_sessions.clone();
        tokio::task::spawn_blocking(move || {
            terminal.process_output_blocking(
                SESSION_ID,
                runtime_generation,
                format!("\x1b[2J\x1b[H{}", output.replace('\n', "\r\n")).into_bytes(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        let final_output = state.terminal_sessions.snapshot(SESSION_ID).await.unwrap();
        assert!(provider_output_has_startup_ready_prompt(
            "claude",
            &final_output.visible_grid
        ));
        let initial_readiness = state
            .terminal_sessions
            .readiness_observation(SESSION_ID)
            .await
            .unwrap();
        let repaint_broker = state.terminal_sessions.clone();
        let repaint = output.replace("Try ask Claude", "draft stays here");
        tokio::task::spawn_blocking(move || {
            repaint_broker.process_output_blocking(
                SESSION_ID,
                runtime_generation,
                format!("\x1b[2J\x1b[H{}", repaint.replace('\n', "\r\n")).into_bytes(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert!(provider_output_has_startup_ready_prompt(
            "claude",
            &state
                .terminal_sessions
                .snapshot(SESSION_ID)
                .await
                .unwrap()
                .visible_grid
        ));
        let committed_hooks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let committed_hooks_for_callback = committed_hooks.clone();
        assert!(publish_startup_readiness_from_action_needed(
            &app.handle().clone(),
            &state,
            SESSION_ID,
            &observation,
            initial_readiness,
            std::sync::Arc::new(|| true),
            std::sync::Arc::new(move || {
                committed_hooks_for_callback.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            }),
        )
        .await
        .is_ok());
        assert_eq!(
            committed_hooks.load(std::sync::atomic::Ordering::Acquire),
            1
        );

        assert_eq!(*current_status.lock().unwrap(), "Idle");
        assert_eq!(
            state
                .interactions
                .provider_input_state(SESSION_ID)
                .await
                .unwrap()
                .state,
            ProviderInputReadiness::Ready,
            "the watcher publishes readiness without another PTY output"
        );
        assert!(
            state
                .terminal_sessions
                .snapshot(SESSION_ID)
                .await
                .unwrap()
                .sequence_barrier
                > final_output.sequence_barrier,
            "the benign repaint is processed before the readiness proof is refreshed"
        );
        assert!(input_rx.try_recv().is_err());

        let next_input_generation = state
            .interactions
            .start_provider_input_generation(SESSION_ID, ProviderInputReadiness::Booting, None)
            .await
            .generation;
        state
            .interactions
            .record_provider_input_state(
                SESSION_ID,
                next_input_generation,
                ProviderInputReadiness::ActionRequired,
                None,
            )
            .await;
        *current_status.lock().unwrap() = "Action Needed".to_string();
        let direct_action_intent_revision = {
            let _status = current_status.lock().unwrap();
            state.reserve_status_intent(SESSION_ID, &current_status, "Action Needed")
        };
        let direct_input_observation = ProviderStartupObservation {
            input_generation: next_input_generation,
            runtime_generation,
            current_status: current_status.clone(),
            startup_action_intent_revision: Some(direct_action_intent_revision),
        };
        let before_direct_input = state
            .terminal_sessions
            .readiness_observation(SESSION_ID)
            .await
            .unwrap();
        let decision = state
            .terminal_sessions
            .send_legacy_input(SESSION_ID, b"manual opposing input\r".to_vec())
            .await
            .unwrap();
        assert_eq!(
            decision.status,
            wardian_core::models::TerminalLeaseDecisionStatus::Accepted
        );
        assert_eq!(
            publish_startup_readiness_from_action_needed(
                &app.handle().clone(),
                &state,
                SESSION_ID,
                &direct_input_observation,
                before_direct_input.clone(),
                std::sync::Arc::new(|| true),
                std::sync::Arc::new(|| {}),
            )
            .await,
            Err(StartupReadinessRejection::OpposingInput),
            "actor-serialized input evidence rejects a direct input already in flight"
        );
        assert_eq!(*current_status.lock().unwrap(), "Action Needed");
        let still_action_required = state
            .interactions
            .provider_input_state(SESSION_ID)
            .await
            .unwrap();
        assert_eq!(still_action_required.generation, next_input_generation);
        assert_eq!(
            still_action_required.state,
            ProviderInputReadiness::ActionRequired
        );
        assert_eq!(
            input_rx.try_recv().unwrap(),
            b"manual opposing input\r".to_vec(),
            "the readiness publisher neither consumes nor resends direct input"
        );

        let (replacement_input_tx, _replacement_input_rx) = mpsc::channel(1);
        let replacement_runtime_generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                SESSION_ID,
                TerminalRuntimeHandles::new(replacement_input_tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry {
                    rows: 24,
                    cols: 120,
                },
            )
            .await
            .unwrap();
        let replacement_status = std::sync::Arc::new(std::sync::Mutex::new("Starting".to_string()));
        {
            let mut agents = state.agents.lock().await;
            let agent = agents.get_mut(SESSION_ID).unwrap();
            agent.runtime_generation = Some(replacement_runtime_generation);
            agent.current_status = replacement_status.clone();
        }
        assert!(publish_startup_readiness_from_action_needed(
            &app.handle().clone(),
            &state,
            SESSION_ID,
            &direct_input_observation,
            before_direct_input,
            std::sync::Arc::new(|| true),
            std::sync::Arc::new(|| {}),
        )
        .await
        .is_err());
        assert_eq!(*replacement_status.lock().unwrap(), "Starting");
        assert_eq!(
            state
                .interactions
                .provider_input_state(SESSION_ID)
                .await
                .unwrap(),
            still_action_required,
            "a stale runtime cannot change the replacement input generation"
        );
    }

    #[tokio::test]
    async fn claude_startup_handoff_preserves_blocking_action_busy_and_replacement() {
        use super::super::{test_support::TestWardianHome, tests::insert_test_agent};
        use crate::state::terminal_session::TerminalRuntimeHandles;
        use tauri::Manager;
        use tokio::sync::mpsc;
        use wardian_core::control::ProviderInputReadiness;

        let _home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();

        for case in [
            "blocking-action",
            "busy-input",
            "later-status",
            "pending-later-action",
            "pending-same-status-action",
            "replacement",
        ] {
            let session_id = format!("claude-{case}");
            insert_test_agent(&state, &session_id, case, "Coder").await;
            let (input_tx, mut input_rx) = mpsc::channel(2);
            let runtime_generation = state
                .terminal_sessions
                .start_or_replace_runtime(
                    &session_id,
                    TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
                    wardian_core::models::TerminalGeometry {
                        rows: 24,
                        cols: 120,
                    },
                )
                .await
                .unwrap();
            let input_generation = state
                .interactions
                .start_provider_input_generation(&session_id, ProviderInputReadiness::Booting, None)
                .await
                .generation;
            state
                .interactions
                .record_provider_input_state(
                    &session_id,
                    input_generation,
                    ProviderInputReadiness::ActionRequired,
                    None,
                )
                .await;
            let current_status = {
                let mut agents = state.agents.lock().await;
                let agent = agents.get_mut(&session_id).unwrap();
                agent.runtime_generation = Some(runtime_generation);
                agent.config.lock().unwrap().provider = "claude".to_string();
                *agent.current_status.lock().unwrap() = "Action Needed".to_string();
                agent.current_status.clone()
            };
            let startup_action_intent_revision = {
                let _status = current_status.lock().unwrap();
                state.reserve_status_intent(&session_id, &current_status, "Action Needed")
            };
            let observation = ProviderStartupObservation {
                input_generation,
                runtime_generation,
                current_status: current_status.clone(),
                startup_action_intent_revision: Some(startup_action_intent_revision),
            };
            let output_broker = state.terminal_sessions.clone();
            tokio::task::spawn_blocking({
                let session_id = session_id.clone();
                move || {
                    output_broker.process_output_blocking(
                        &session_id,
                        runtime_generation,
                        "\x1b[2J\x1b[HClaude Code\r\n❯ Try ask Claude\r\n────────\r\nHaiku 4.5 | workspace | /rc\r\n⏵⏵ bypass permissions on (shift+tab to cycle)".as_bytes().to_vec(),
                    )
                }
            })
            .await
            .unwrap()
            .unwrap();
            let initial = state
                .terminal_sessions
                .readiness_observation(&session_id)
                .await
                .unwrap();

            let expected_rejection = match case {
                "blocking-action" => {
                    let output_broker = state.terminal_sessions.clone();
                    let repaint_session = session_id.clone();
                    tokio::task::spawn_blocking(move || {
                        output_broker.process_output_blocking(
                            &repaint_session,
                            runtime_generation,
                            b"\x1b[2J\x1b[HAllow external CLAUDE.md file imports?\r\nNo, disable external imports\r\nYes, allow external imports".to_vec(),
                        )
                    })
                    .await
                    .unwrap()
                    .unwrap();
                    StartupReadinessRejection::BlockingAction
                }
                "busy-input" => {
                    state
                        .interactions
                        .record_provider_input_state(
                            &session_id,
                            input_generation,
                            ProviderInputReadiness::Busy,
                            None,
                        )
                        .await;
                    StartupReadinessRejection::InputOrStatusChanged
                }
                "later-status" => {
                    *current_status.lock().unwrap() = "Processing...".to_string();
                    StartupReadinessRejection::StatusOrOwnerChanged
                }
                "pending-later-action" => {
                    let _status = current_status.lock().unwrap();
                    state.reserve_status_intent(&session_id, &current_status, "Processing...");
                    StartupReadinessRejection::StatusOrOwnerChanged
                }
                "pending-same-status-action" => {
                    let _status = current_status.lock().unwrap();
                    state.reserve_status_intent(&session_id, &current_status, "Action Needed");
                    StartupReadinessRejection::StatusOrOwnerChanged
                }
                "replacement" => {
                    let (replacement_tx, _replacement_rx) = mpsc::channel(1);
                    let replacement_runtime = state
                        .terminal_sessions
                        .start_or_replace_runtime(
                            &session_id,
                            TerminalRuntimeHandles::new(replacement_tx, |_| Ok(())),
                            wardian_core::models::TerminalGeometry {
                                rows: 24,
                                cols: 120,
                            },
                        )
                        .await
                        .unwrap();
                    let replacement_input = state
                        .interactions
                        .start_provider_input_generation(
                            &session_id,
                            ProviderInputReadiness::Booting,
                            None,
                        )
                        .await;
                    let replacement_status =
                        std::sync::Arc::new(std::sync::Mutex::new("Starting".to_string()));
                    let mut agents = state.agents.lock().await;
                    let agent = agents.get_mut(&session_id).unwrap();
                    agent.runtime_generation = Some(replacement_runtime);
                    agent.current_status = replacement_status;
                    drop(agents);
                    assert_eq!(
                        state
                            .interactions
                            .provider_input_state(&session_id)
                            .await
                            .unwrap(),
                        replacement_input
                    );
                    StartupReadinessRejection::StatusOrOwnerChanged
                }
                _ => unreachable!(),
            };

            let rejected = publish_startup_readiness_from_action_needed(
                &app.handle().clone(),
                &state,
                &session_id,
                &observation,
                initial,
                std::sync::Arc::new(|| true),
                std::sync::Arc::new(|| {}),
            )
            .await;
            assert_eq!(rejected, Err(expected_rejection), "case={case}");
            assert!(
                input_rx.try_recv().is_err(),
                "case={case}: no trust keys replayed"
            );
            let current_input = state
                .interactions
                .provider_input_state(&session_id)
                .await
                .unwrap();
            if case == "busy-input" {
                assert_eq!(current_input.state, ProviderInputReadiness::Busy);
            } else if case != "replacement" {
                assert_eq!(current_input.generation, input_generation);
                assert_eq!(current_input.state, ProviderInputReadiness::ActionRequired);
            }
        }
    }

    #[tokio::test]
    async fn opencode_recheck_rejects_changed_screen_and_current_composer_can_publish() {
        use super::super::{test_support::TestWardianHome, tests::insert_test_agent};
        use crate::state::terminal_session::TerminalRuntimeHandles;
        use tokio::sync::mpsc;
        use wardian_core::control::ProviderInputReadiness;

        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        const SESSION_ID: &str = "opencode-recheck-race";
        insert_test_agent(&state, SESSION_ID, "OpenCodeRecheck", "Coder").await;
        let (tx, mut input_rx) = mpsc::channel(1);
        let runtime_generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                SESSION_ID,
                TerminalRuntimeHandles::new(tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry {
                    rows: 24,
                    cols: 120,
                },
            )
            .await
            .unwrap();
        let input_generation = state
            .interactions
            .start_provider_input_generation(SESSION_ID, ProviderInputReadiness::Booting, None)
            .await
            .generation;
        let current_status = {
            let mut agents = state.agents.lock().await;
            let agent = agents.get_mut(SESSION_ID).unwrap();
            agent.runtime_generation = Some(runtime_generation);
            agent.config.lock().unwrap().provider = "opencode".to_string();
            *agent.current_status.lock().unwrap() = "Starting".to_string();
            agent.current_status.clone()
        };
        let observation = ProviderStartupObservation {
            input_generation,
            runtime_generation,
            current_status: current_status.clone(),
            startup_action_intent_revision: None,
        };
        let broker = state.terminal_sessions.clone();
        let ready = b"\x1b[2J\x1b[HAsk anything...\r\nBuild  mimo-v2.5-free\r\nctrl+p commands";
        let output_broker = broker.clone();
        tokio::task::spawn_blocking(move || {
            output_broker.process_output_blocking(SESSION_ID, runtime_generation, ready.to_vec())
        })
        .await
        .unwrap()
        .unwrap();
        assert!(opencode_current_screen_is_ready(&state, SESSION_ID)
            .await
            .unwrap());

        let validation_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_for_validation = validation_count.clone();
        let state_ref = &state;
        let status_for_failure = current_status.clone();
        assert!(
            !publish_startup_readiness_from_starting(
                &state,
                SESSION_ID,
                &observation,
                ProviderReadyEvidence::PromptDetected,
                move || {
                    let count = count_for_validation.clone();
                    let broker = broker.clone();
                    async move {
                        if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                            tokio::task::spawn_blocking(move || {
                                broker.process_output_blocking(
                                    SESSION_ID,
                                    runtime_generation,
                                    b"\x1b[2J\x1b[HLoading session...".to_vec(),
                                )
                            })
                            .await
                            .unwrap()
                            .unwrap();
                        }
                        opencode_current_screen_is_ready(state_ref, SESSION_ID)
                            .await
                            .unwrap_or(false)
                    }
                },
                move |next_status| {
                    *status_for_failure.lock().unwrap() = next_status.to_string();
                },
            )
            .await
        );
        assert_eq!(
            validation_count.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(*current_status.lock().unwrap(), "Starting");
        assert_eq!(
            state
                .interactions
                .provider_input_state(SESSION_ID)
                .await
                .unwrap()
                .state,
            ProviderInputReadiness::Booting,
            "screen invalidation must not publish a readiness receipt"
        );

        let output_broker = state.terminal_sessions.clone();
        tokio::task::spawn_blocking(move || {
            output_broker.process_output_blocking(SESSION_ID, runtime_generation, ready.to_vec())
        })
        .await
        .unwrap()
        .unwrap();
        let status_for_success = current_status.clone();
        assert!(
            publish_startup_readiness_from_starting(
                &state,
                SESSION_ID,
                &observation,
                ProviderReadyEvidence::PromptDetected,
                || async {
                    opencode_current_screen_is_ready(&state, SESSION_ID)
                        .await
                        .unwrap_or(false)
                },
                move |next_status| {
                    *status_for_success.lock().unwrap() = next_status.to_string();
                },
            )
            .await
        );
        assert_eq!(*current_status.lock().unwrap(), "Idle");
        assert_eq!(
            state
                .interactions
                .provider_input_state(SESSION_ID)
                .await
                .unwrap()
                .state,
            ProviderInputReadiness::Ready
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn startup_ready_prompt_requires_provider_composer() {
        let model_choice = "GPT-5.4 Mini will be deprecated soon\nCodex now uses GPT-5.6 Luna in place of GPT-5.4 Mini.\nChoose how you'd like Codex to proceed.\n› 1. Try new model\n  2. Use existing model\nUse ↑/↓ to move, press enter to confirm";
        assert!(!provider_output_has_startup_ready_prompt(
            "codex",
            model_choice
        ));
        assert!(provider_output_requires_startup_action(
            "codex",
            model_choice
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "codex",
            "│ model: loading │\nResuming session…\n› Write tests for @filename",
        ));
        assert!(provider_output_has_startup_ready_prompt(
            "codex",
            "\u{1b}[1;1H\u{1b}[J\u{1b}[13;1H\u{1b}[1m›\u{1b}[22m Write tests for @filename\u{1b}[?25h",
        ));
        assert!(!provider_output_has_startup_ready_prompt("claude", "❯"));
        assert!(!provider_output_has_startup_ready_prompt(
            "claude",
            "Choose a theme\n❯ Dark mode\n  Light mode",
        ));
        let imports = "Allow external CLAUDE.md file imports?\nExternal imports:\n  <class>/AGENTS.md\n  <habitat>/AGENTS.md\n❯ No, disable external imports\n  Yes, allow external imports\nEnter to confirm · Esc to cancel";
        assert!(!provider_output_has_startup_ready_prompt("claude", imports));
        assert!(provider_output_requires_startup_action("claude", imports));
        assert!(provider_output_has_startup_ready_prompt(
            "claude",
            "Claude Code v2.1.263\n❯ Try fix typecheck errors\n────────\nHaiku 4.5 | workspace | /rc\n⏵⏵ bypass permissions on (shift+tab to cycle)",
        ));
    }

    #[test]
    fn claude_workspace_trust_screen_requires_the_exact_question_and_both_choices() {
        let prompt = "Quick safety check: Is this a project you created or one you trust?\n/workspace/project\n❯ No, exit\nYes, I trust this folder";

        assert!(claude_workspace_trust_prompt_is_current(prompt));
        assert!(claude_workspace_trust_prompt_selects_no(prompt));
        assert!(provider_output_requires_startup_action("claude", prompt));
        assert!(!provider_output_has_startup_ready_prompt("claude", prompt));
        assert!(!provider_output_requires_startup_action("codex", prompt));

        let changed_selection = prompt
            .replace("❯ No, exit", "No, exit")
            .replace("Yes, I trust this folder", "❯ Yes, I trust this folder");
        assert!(claude_workspace_trust_prompt_is_current(&changed_selection));
        assert!(!claude_workspace_trust_prompt_selects_no(
            &changed_selection
        ));
        assert_eq!(
            claude_workspace_trust_prompt_selection(&changed_selection),
            Some((false, true))
        );
        assert!(provider_output_requires_startup_action(
            "claude",
            &changed_selection,
        ));

        let transient_both_selected = prompt.replace(
            "❯ No, exit\nYes, I trust this folder",
            "❯ No, exit\n❯ Yes, I trust this folder",
        );
        assert_eq!(
            claude_workspace_trust_prompt_selection(&transient_both_selected),
            Some((true, true))
        );
        assert!(!claude_workspace_trust_prompt_selects_no(
            &transient_both_selected
        ));
        assert!(!claude_workspace_trust_prompt_is_current(
            "Quick safety check: Is this a project you created or one you trust?\n❯ No, exit"
        ));
        assert!(!claude_workspace_trust_prompt_is_current(
            "A project description mentions Quick safety check: Is this a project you created or one you trust?\n❯ No, exit\nYes, I trust this folder"
        ));

        let current_2_1_283 = "Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.\nClaude Code'll be able to read, edit, and execute files here.\n❯ No, exit\nYes, I trust this folder";
        assert!(claude_workspace_trust_prompt_is_current(current_2_1_283));
        assert!(claude_workspace_trust_prompt_selects_no(current_2_1_283));
        assert!(provider_output_requires_startup_action(
            "claude",
            current_2_1_283
        ));

        let current_2_1_283_wrapped = "  Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's\n  in this folder first.\n  Claude Code'll be able to read, edit, and execute files here.\n  ❯ No, exit\n    Yes, I trust this folder";
        assert!(claude_workspace_trust_prompt_is_current(
            current_2_1_283_wrapped
        ));
        assert!(claude_workspace_trust_prompt_selects_no(
            current_2_1_283_wrapped
        ));
        assert!(provider_output_requires_startup_action(
            "claude",
            current_2_1_283_wrapped
        ));

        let changed_wrap =
            current_2_1_283_wrapped.replace("  in this folder first.", "  this folder first.");
        assert!(!claude_workspace_trust_prompt_is_current(&changed_wrap));
        assert!(!provider_output_requires_startup_action(
            "claude",
            &changed_wrap
        ));
    }

    #[test]
    fn startup_ready_prompt_accepts_pi_regular_tui_footer() {
        assert!(provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\r\n────────────────\r\nC:\\workspace • Wardian-Pi\r\n0.0%/33k (auto) echo",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "No models available. Use /login to log into a provider.",
        ));
    }

    #[test]
    fn startup_ready_prompt_accepts_pi_truncated_long_workspace_footer() {
        assert!(provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace (test/provider-conformanc...\n$0.000 (sub) 0.0%/272k (auto) (openai-codex) gpt-5.4-mini • medium",
        ));
    }

    #[test]
    fn startup_ready_prompt_accepts_pi_wrapped_footer_and_rejects_non_editor_frames() {
        assert!(provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace (test/provider-conformanc...\n$0.000 (sub) 0.0%/272k (auto)\n(openai-codex) gpt-5.4-mini • medium",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace\n$0.000 (sub) 0.0%/272k (auto)\nmodel: loading",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace\nError: provider authentication failed",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace • old-session\nError: provider authentication failed\n0.0%/33k (auto) echo",
        ));
        assert!(provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/starting-project • active\n0.0%/33k (auto) echo",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace\nWrite a message here",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n0.0%/272k (auto) • arbitrary",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\nDraft text • arbitrary\n0.0%/272k (auto) echo",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\nDraft text / arbitrary\n0.0%/272k (auto) echo",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace • old-session\n0.0%/33k (auto) echo\npi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace\nDrafting a new prompt",
        ));
        assert!(!provider_output_has_startup_ready_prompt(
            "pi",
            "pi v0.84.2\n────────────────\n<workspace-root>/habitat/workspace • old-session\n0.0%/33k (auto) echo\nDrafting a new prompt",
        ));
    }

    #[test]
    fn antigravity_startup_trust_prompt_requires_action() {
        assert!(provider_output_requires_startup_action(
            "antigravity",
            "Do you trust the contents of this project?",
        ));
        assert!(!provider_output_requires_startup_action(
            "antigravity",
            "Welcome to the Antigravity CLI. You are currently not signed in.",
        ));
        assert!(!provider_output_requires_startup_action(
            "codex",
            "Do you trust the contents of this project?",
        ));
    }
}
