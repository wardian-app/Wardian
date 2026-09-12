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
        super::spawn_mailbox_drain_if_idle(app, session_id, "Idle");
    }
    true
}

/// Recognizes initial compose readiness. Codex, Claude and OpenCode callers must supply
/// the canonical visible screen: raw chunks can omit startup blockers, while
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
            let Some(composer) = lines
                .iter()
                .rposition(|line| line.starts_with("Ask anything"))
            else {
                return false;
            };
            let footer = lines[composer + 1..].join(" ");
            let footer = footer.split_whitespace().collect::<Vec<_>>().join(" ");
            !provider_output_requires_startup_action(provider, &cleaned)
                && !lines.iter().any(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower.starts_with("loading") || lower.starts_with("connecting")
                })
                && footer.contains("ctrl+p commands")
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

    async fn assert_opencode_resume_log_append_keeps_unready_work_queued(title: &str) {
        use super::super::{test_support::TestWardianHome, tests::insert_test_agent};
        use crate::manager::telemetry::tests::apply_opencode_startup_log_pass;
        use crate::state::terminal_session::TerminalRuntimeHandles;
        use std::io::Write;
        use wardian_core::control::MessageInputMode;

        let fixture = TestWardianHome::new_async().await;
        let state = AppState::new();
        let session_id = "opencode-resume-startup";
        insert_test_agent(&state, session_id, "OpenCodeStartup", "Coder").await;
        let config = {
            let mut agents = state.agents.lock().await;
            let agent = agents.get_mut(session_id).unwrap();
            // The shared fixture has a sentinel PID; this test owns no process.
            agent.process_id = None;
            agent.config.clone()
        };
        {
            let mut config = config.lock().unwrap();
            config.provider = "opencode".to_string();
            config.folder = fixture.path().to_string_lossy().into_owned();
            config.resume_session = Some("ses_fresh".to_string());
        }

        let mut inputs = Vec::new();
        for expected_generation in 1..=3 {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            let runtime_generation = state
                .terminal_sessions
                .start_or_replace_runtime(
                    session_id,
                    TerminalRuntimeHandles::new_with_write_ack(tx, |_| Ok(())),
                    wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
                )
                .await
                .unwrap();
            state
                .agents
                .lock()
                .await
                .get_mut(session_id)
                .unwrap()
                .runtime_generation = Some(runtime_generation);
            inputs.push(rx);
            let input = state
                .interactions
                .start_provider_input_generation(session_id, ProviderInputReadiness::Booting, None)
                .await;
            assert_eq!(input.generation, expected_generation);
            if expected_generation < 3 {
                record_provider_ready_prompt(&state, session_id, input.generation).await;
            }
        }
        let (generation, current_status, terminal_title, watch_state) = {
            let agents = state.agents.lock().await;
            let agent = agents.get(session_id).unwrap();
            (
                agent.runtime_generation.unwrap(),
                agent.current_status.clone(),
                agent.terminal_title.clone(),
                agent.watch_state.clone(),
            )
        };
        *current_status.lock().unwrap() = "Starting".to_string();
        *terminal_title.lock().unwrap() = title.to_string();
        // Retained output is not the current runtime's canonical screen.
        watch_state
            .lock()
            .unwrap()
            .push_output(b"OpenCode\r\nUSER_previous_answer\r\nBuild  mimo-v2.5-free\r\n");
        let terminal = state.terminal_sessions.clone();
        tokio::task::spawn_blocking(move || {
            terminal.process_output_blocking(
                session_id,
                generation,
                b"\x1b[2J\x1b[HLoading session...".to_vec(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        let screen = state.terminal_sessions.snapshot(session_id).await.unwrap();
        assert_eq!(screen.runtime_generation, generation);
        assert!(screen.visible_grid.contains("Loading session..."));
        assert!(!screen.visible_grid.contains("USER_previous_answer"));

        let queued = super::super::deliver_prompt_to_agent(
            None,
            &state,
            session_id,
            "Recall the previous user marker",
            MessageInputMode::Message,
        )
        .await
        .unwrap();
        assert_eq!(queued.delivery_state, "queued");
        let before = state.mailbox.lock().await.list_for_target(session_id);
        assert_eq!(before.len(), 1);
        let receipts_before =
            wardian_core::db::list_interaction_delivery_attempts(&before[0].interaction_id)
                .unwrap();

        // A completed turn belongs to generation 2. Generation 3 appends only
        // startup activity, so re-reading the log cannot prove input readiness.
        let log_path = fixture.path().join("opencode.log");
        std::fs::write(
            &log_path,
            concat!(
                "timestamp=2026-09-09T19:38:54.782Z level=INFO run=old message=loop session.id=ses_fresh step=0\n",
                "timestamp=2026-09-09T19:38:58.512Z level=INFO run=old message=\"exiting loop\" session.id=ses_fresh\n",
            ),
        )
        .unwrap();
        apply_opencode_startup_log_pass(&state, session_id, &log_path, true).await;
        assert!(inputs.iter_mut().all(|input| input.try_recv().is_err()));
        assert_ne!(
            state
                .interactions
                .provider_input_state(session_id)
                .await
                .unwrap()
                .state,
            ProviderInputReadiness::Ready,
        );
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        writeln!(
            log,
            "timestamp=2026-09-09T19:39:08.402Z level=INFO run=resumed message=init"
        )
        .unwrap();
        drop(log);
        let pass = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            apply_opencode_startup_log_pass(&state, session_id, &log_path, false),
        )
        .await;
        let wrote_input = inputs.iter_mut().any(|input| input.try_recv().is_ok());
        let after = state.mailbox.lock().await.list_for_target(session_id);
        let receipts_after =
            wardian_core::db::list_interaction_delivery_attempts(&before[0].interaction_id)
                .unwrap();
        assert!(
            !wrote_input,
            "#1177: retained generation-2 Idle plus a generation-3 startup append must not write payload or Return while the current screen is loading (title={title:?})",
        );
        assert!(
            pass.is_ok(),
            "unready startup must preserve the queue without waiting for a provider receipt"
        );
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        assert_eq!(
            serde_json::to_value(receipts_after).unwrap(),
            serde_json::to_value(receipts_before).unwrap()
        );
        assert_eq!(*current_status.lock().unwrap(), "Starting");

        let observation = ProviderStartupObservation {
            input_generation: 3,
            runtime_generation: generation,
            current_status: current_status.clone(),
        };
        // Even an Idle observation or title cannot publish the loading screen.
        *current_status.lock().unwrap() = "Idle".to_string();
        assert!(
            !publish_startup_readiness(
                None,
                &state,
                session_id,
                &observation,
                ProviderReadyEvidence::TitleDetected,
            )
            .await
        );

        // Repaint the same generation to its composer, without a new title.
        // This is a synthetic unit frame, not real-provider acceptance evidence.
        let terminal = state.terminal_sessions.clone();
        tokio::task::spawn_blocking(move || {
            terminal.process_output_blocking(
                session_id,
                generation,
                b"\x1b[2J\x1b[HAsk anything...\r\nBuild  mimo-v2.5-free\r\nctrl+p commands"
                    .to_vec(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert!(
            publish_startup_readiness(
                None,
                &state,
                session_id,
                &observation,
                ProviderReadyEvidence::PromptDetected,
            )
            .await
        );
        // A retained provider event must not satisfy the new submit's receipt.
        crate::manager::record_agent_turn_started_for_watch(&state, session_id).await;
        let drain =
            super::super::drain_next_mailbox_message_for_idle_agent(None, &state, session_id);
        tokio::pin!(drain);
        let input = inputs.last_mut().unwrap();
        let payload = tokio::select! {
            request = input.recv() => request.unwrap(),
            result = &mut drain => panic!("drain completed before payload: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => panic!("ready composer did not release queued payload"),
        };
        assert_eq!(payload.bytes, b"Recall the previous user marker");
        payload.completion.send(Ok(())).unwrap();
        let submit = tokio::select! {
            request = input.recv() => request.unwrap(),
            result = &mut drain => panic!("drain completed before Return: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => panic!("Return missing after payload acknowledgement"),
        };
        assert_eq!(submit.bytes, b"\x1b[13u");
        submit.completion.send(Ok(())).unwrap();
        tokio::select! {
            result = &mut drain => panic!("write acknowledgements and an old event are not provider acceptance: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
        assert!(
            !wardian_core::db::list_interaction_delivery_attempts(&before[0].interaction_id)
                .unwrap()
                .iter()
                .any(|receipt| receipt.delivery_state == "provider_accepted")
        );
        crate::manager::record_agent_turn_started_for_watch(&state, session_id).await;
        let delivered = tokio::time::timeout(std::time::Duration::from_secs(2), &mut drain)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(delivered.message_id.as_deref(), Some(before[0].id.as_str()));
        assert_eq!(delivered.delivery_state, "provider_accepted");
        assert_eq!(delivered.delivery_phase.as_deref(), Some("turn_started"));
        *current_status.lock().unwrap() = "Idle".to_string();
        assert!(
            super::super::drain_next_mailbox_message_for_idle_agent(None, &state, session_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(inputs.iter_mut().all(|input| input.try_recv().is_err()));
        assert!(!wardian_core::db::list_mailbox_messages()
            .unwrap()
            .iter()
            .any(|record| record.id == before[0].id));
        state
            .terminal_sessions
            .terminate_and_remove_runtime(session_id, generation)
            .await
            .unwrap();
    }

    #[test]
    fn opencode_startup_composer_rejects_partial_loading_and_consent_screens() {
        let ready = "Ask anything...\nBuild  mimo-v2.5-free\nctrl+p commands";
        assert!(provider_output_has_startup_ready_prompt("opencode", ready));
        for blocked in [
            "OpenCode",
            "Ask anything...",
            "ctrl+p commands",
            "Ask anything...\nLoading session...\nctrl+p commands",
            "Permission required\nAsk anything...\nctrl+p commands",
            "Do you trust this directory?\nAsk anything...\nctrl+p commands",
        ] {
            assert!(
                !provider_output_has_startup_ready_prompt("opencode", blocked),
                "{blocked}"
            );
        }
    }

    #[tokio::test]
    async fn opencode_resume_startup_log_append_does_not_drain_before_current_screen() {
        assert_opencode_resume_log_append_keeps_unready_work_queued("").await;
    }

    #[tokio::test]
    async fn opencode_resume_startup_log_append_title_does_not_override_loading_screen() {
        assert_opencode_resume_log_append_keeps_unready_work_queued("OpenCode").await;
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
