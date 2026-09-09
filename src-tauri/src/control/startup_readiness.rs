use super::{
    antigravity_output_has_ready_prompt, gemini_output_has_api_key_prompt,
    pi_output_has_startup_ready_prompt,
};
use crate::providers::claude::claude_output_has_bypass_permissions_consent_prompt;
use crate::state::AppState;
use crate::utils::strip_ansi_controls;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use wardian_core::control::{ProviderInputReadiness, ProviderReadyEvidence};

pub(super) async fn record_provider_ready_evidence(
    state: &AppState,
    session_id: &str,
    generation: u64,
    evidence: ProviderReadyEvidence,
) -> bool {
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

/// OpenCode exposes initial compose readiness through its provider-owned
/// terminal title rather than a stable prompt marker in the raw PTY stream.
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
}

impl ProviderStartupObservation {
    async fn is_current(&self, state: &AppState, session_id: &str) -> bool {
        let agents = state.agents.lock().await;
        agents.get(session_id).is_some_and(|agent| {
            agent.runtime_generation == Some(self.runtime_generation)
                && Arc::ptr_eq(&agent.current_status, &self.current_status)
                && agent
                    .current_status
                    .lock()
                    .is_ok_and(|status| status.eq_ignore_ascii_case("idle"))
        })
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
    if !observation.is_current(state, session_id).await {
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
        super::spawn_mailbox_drain_if_idle(app, session_id, "Idle");
    }
    true
}

/// Recognizes initial compose readiness. Codex and Claude callers must supply
/// the canonical visible screen: raw chunks can omit startup blockers, while
/// accumulated output retains blockers that a later repaint already removed.
pub(crate) fn provider_output_has_startup_ready_prompt(provider: &str, output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    match provider {
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

/// Provider startup can require an explicit account or workspace decision
/// before a compose prompt exists. Keep that state visible and prevent queued
/// delivery from being mistaken for a prompt the provider can receive.
pub(crate) fn provider_output_requires_startup_action(provider: &str, output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).to_ascii_lowercase();
    match provider {
        "claude" => cleaned.contains("allow external claude.md file imports?"),
        "codex" => crate::delivery::codex_menu::current_screen_requires_choice(
            &strip_ansi_controls(output),
        ),
        "antigravity" => cleaned.contains("do you trust the contents of this project?"),
        _ => false,
    }
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
    async fn deferred_startup_publication_rechecks_replaced_runtime_before_drain() {
        use super::super::{
            test_support::TestWardianHome,
            tests::{insert_test_agent, install_test_terminal_runtime},
        };
        use wardian_core::control::{MessageInputMode, ProviderInputReadiness};
        let _home = TestWardianHome::new_async().await;
        let state = Arc::new(AppState::new());
        insert_test_agent(&state, "agent-1", "Startup", "Coder").await;
        let (tx, mut old_rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;
        let generation = state
            .interactions
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Booting, None)
            .await
            .generation;
        let observed = {
            let agents = state.agents.lock().await;
            let agent = &agents["agent-1"];
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            ProviderStartupObservation {
                input_generation: generation,
                runtime_generation: agent.runtime_generation.unwrap(),
                current_status: agent.current_status.clone(),
            }
        };
        let queued = super::super::deliver_prompt_to_agent(
            None,
            &state,
            "agent-1",
            "queued work",
            MessageInputMode::Message,
        )
        .await
        .unwrap();
        assert_eq!(queued.delivery_state, "queued");

        let lifecycle = state.lock_agent_lifecycle("agent-1").await;
        let task_state = state.clone();
        let old_observation = observed.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let publication = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            publish_startup_readiness(
                None,
                &task_state,
                "agent-1",
                &old_observation,
                ProviderReadyEvidence::PromptDetected,
            )
            .await
        });
        started_rx.await.unwrap();
        let replacement = state
            .interactions
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Booting, None)
            .await;
        let (tx, mut replacement_rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;
        let current = {
            let mut agents = state.agents.lock().await;
            let agent = agents.get_mut("agent-1").unwrap();
            agent.current_status = Arc::new(Mutex::new("Starting".to_string()));
            ProviderStartupObservation {
                input_generation: replacement.generation,
                runtime_generation: agent.runtime_generation.unwrap(),
                current_status: agent.current_status.clone(),
            }
        };
        drop(lifecycle);
        assert!(!publication.await.unwrap());
        assert_eq!(
            state
                .interactions
                .provider_input_state("agent-1")
                .await
                .unwrap(),
            replacement
        );
        assert!(
            super::super::drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(old_rx.try_recv().is_err());
        assert!(replacement_rx.try_recv().is_err());

        *current.current_status.lock().unwrap() = "Idle".to_string();
        let wrong_identity = ProviderStartupObservation {
            current_status: observed.current_status,
            ..current.clone()
        };
        assert!(
            !publish_startup_readiness(
                None,
                &state,
                "agent-1",
                &wrong_identity,
                ProviderReadyEvidence::TitleDetected
            )
            .await
        );
        assert!(
            publish_startup_readiness(
                None,
                &state,
                "agent-1",
                &current,
                ProviderReadyEvidence::TitleDetected
            )
            .await
        );
        let delivered =
            super::super::drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
                .await
                .unwrap()
                .unwrap();
        assert_eq!(delivered.message_id, queued.message_id);
        assert!(replacement_rx.try_recv().is_ok());
        assert!(
            super::super::drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
                .await
                .unwrap()
                .is_none()
        );
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
