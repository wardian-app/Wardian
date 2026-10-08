use super::agent_lifecycle::fresh_provider_session_for_initial_capture;
use super::config_persistence::persist_agent_config_with_roster_barrier;
use super::tests::{make_test_agent, WardianHomeGuard};
use super::{lifecycle_config_for_session, promote_fresh_provider_session_after_resume};
use crate::state::AppState;
use wardian_core::conversations::{AgentConversationLoggingSetting, ConversationLoggingSetting};
use wardian_core::models::AgentConfig;

async fn persist_agent_config_for_test(
    new_config: AgentConfig,
    state: &AppState,
) -> Result<(), String> {
    let roster_barrier = tokio::task::spawn_blocking(|| {
        wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "Agent roster barrier is unavailable".to_string())?;
    persist_agent_config_with_roster_barrier(new_config, state, &roster_barrier).await
}

#[test]
fn mock_launch_identity_survives_promotion_and_registration_only_in_memory() {
    use super::agent_lifecycle::{
        promote_fresh_provider_session_fields, sync_registered_provider_session,
    };

    let mut config = AgentConfig {
        provider: "mock".to_string(),
        fresh_provider_session_id: Some("mock-owned-session".to_string()),
        ..AgentConfig::default()
    };
    assert!(promote_fresh_provider_session_fields("mock", &mut config));
    assert_eq!(
        config.fresh_provider_session_id.as_deref(),
        Some("mock-owned-session")
    );
    let mut active = config.clone();
    sync_registered_provider_session(
        &mut config,
        &mut active,
        Some("mock-owned-session".to_string()),
    );
    for registered in [&config, &active] {
        assert_eq!(
            fresh_provider_session_for_initial_capture(registered, Some("mock-owned-session")),
            Some("mock-owned-session".to_string())
        );
    }
    let restored: AgentConfig =
        serde_json::from_value(serde_json::to_value(&config).expect("serialize config"))
            .expect("restore config");
    assert_eq!(restored.fresh_provider_session_id, None);
    assert_eq!(
        fresh_provider_session_for_initial_capture(&restored, Some("mock-owned-session")),
        None
    );
}

#[test]
fn mock_initial_capture_rejects_missing_empty_and_foreign_identities() {
    use super::agent_lifecycle::sync_registered_provider_session;

    for (fresh, resume, actual) in [
        (None, Some("owned"), Some("owned")),
        (Some(""), None, Some("owned")),
        (Some("owned"), None, None),
        (Some("owned"), None, Some("")),
        (Some("owned"), None, Some("foreign")),
        (Some("owned"), Some("foreign"), Some("owned")),
        (Some("owned"), Some(""), Some("owned")),
    ] {
        let mut config = AgentConfig {
            provider: "mock".to_string(),
            fresh_provider_session_id: fresh.map(str::to_string),
            resume_session: resume.map(str::to_string),
            ..AgentConfig::default()
        };
        assert_eq!(
            fresh_provider_session_for_initial_capture(&config, actual),
            None
        );
        let mut active = config.clone();
        sync_registered_provider_session(&mut config, &mut active, actual.map(str::to_string));
        assert_eq!(config.fresh_provider_session_id, None);
        assert_eq!(active.fresh_provider_session_id, None);
    }
}

#[tokio::test]
async fn mock_initial_archive_capture_preserves_enabled_input_and_excludes_private_spans() {
    use crate::commands::chat::{
        agent_archive_capture_snapshot, archive_agent_chat_events_until_stable_for_state,
        conversation_archive_context_from_snapshot, record_provider_log_policy_for_snapshot,
    };
    use std::io::Write as _;

    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
        .expect("initialize isolated state database");

    // Observe the nightly failure's prefix boundary before appending the rest
    // of the two turns. No acquisition occurs until all policy changes finish.
    for (case, fresh, initially_enabled, disable_before_capture, expect_first) in [
        ("fresh-enabled", true, true, false, true),
        ("restored", false, true, false, false),
        ("initially-disabled", true, false, false, false),
        ("globally-disabled", true, false, false, false),
        ("disabled-backlog", true, true, true, true),
    ] {
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: if case == "globally-disabled" {
                ConversationLoggingSetting::Disabled
            } else {
                ConversationLoggingSetting::Enabled
            },
            ..Default::default()
        })
        .expect("save global logging policy");
        let log_path = temp.path().join(format!("{case}.jsonl"));
        std::fs::write(
            &log_path,
            concat!(
                "{\"type\":\"init\",\"session_id\":\"mock-owned-session\"}\n",
                "{\"type\":\"user\",\"content\":\"Lower the work-log grouping threshold and record a spec.\"}\n"
            ),
        )
        .expect("write initial Mock prefix");
        let state = AppState::new();
        let agent = make_test_agent();
        {
            let mut config = agent.config.lock().expect("agent config");
            config.session_id = case.to_string();
            config.session_name = case.to_string();
            config.provider = "mock".to_string();
            config.reset_provider_config_for_provider();
            config.folder = temp.path().to_string_lossy().to_string();
            config.resume_session = Some("mock-owned-session".to_string());
            config.fresh_provider_session_id = fresh.then(|| "mock-owned-session".to_string());
            config.conversation_logging = if initially_enabled || case == "globally-disabled" {
                AgentConversationLoggingSetting::Default
            } else {
                AgentConversationLoggingSetting::Disabled
            };
        }
        *agent.log_path.lock().expect("agent log path") = Some(log_path.clone());
        state.agents.lock().await.insert(case.to_string(), agent);
        state.agent_order.lock().await.push(case.to_string());
        let snapshot = agent_archive_capture_snapshot(&state, case)
            .await
            .expect("capture snapshot");
        record_provider_log_policy_for_snapshot(&state, &snapshot, initially_enabled)
            .expect("observe policy before first acquisition");

        if disable_before_capture {
            let mut disabled = lifecycle_config_for_session(&state, case).await.unwrap();
            disabled.conversation_logging = AgentConversationLoggingSetting::Disabled;
            persist_agent_config_for_test(disabled, &state)
                .await
                .expect("disable before acquisition");
        }
        if !initially_enabled || disable_before_capture {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .expect("open disabled log");
            writeln!(file, r#"{{"type":"user","content":"SECRET_DISABLED"}}"#)
                .expect("append disabled input");
            drop(file);
            if case == "globally-disabled" {
                crate::commands::settings::save_shell_settings_for_state(
                    &state,
                    crate::utils::ShellSettingsDocument {
                        schema_version: 2,
                        settings: crate::utils::ShellSettings {
                            conversation_logging: ConversationLoggingSetting::Enabled,
                            ..Default::default()
                        },
                        overrides: crate::utils::ShellSettingsOverrides {
                            conversation_logging: Some(ConversationLoggingSetting::Enabled),
                            ..Default::default()
                        },
                    },
                )
                .await
                .expect("enable global capture before first acquisition");
            } else {
                let mut enabled = lifecycle_config_for_session(&state, case).await.unwrap();
                enabled.conversation_logging = AgentConversationLoggingSetting::Enabled;
                persist_agent_config_for_test(enabled, &state)
                    .await
                    .expect("enable before first acquisition");
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("open enabled log");
        file.write_all(concat!(
            "{\"type\":\"tool_call\",\"call_id\":\"edit-first\",\"tool_name\":\"Edit\",\"input\":{\"file_path\":\"first.ts\"}}\n",
            "{\"type\":\"user\",\"content\":\"Now widen the change kinds.\"}\n",
            "{\"type\":\"tool_call\",\"call_id\":\"edit-second\",\"tool_name\":\"Edit\",\"input\":{\"file_path\":\"second.ts\"}}\n"
        ).as_bytes()).expect("append remaining turns");
        drop(file);
        archive_agent_chat_events_until_stable_for_state(&state, case)
            .await
            .expect("capture Mock log");
        let context = conversation_archive_context_from_snapshot(&snapshot);
        let events = state
            .conversation_archive
            .chat_events_for_capture(&context)
            .expect("read captured events");
        let users = events
            .iter()
            .filter(|event| event.role == Some(wardian_core::models::chat::AgentChatRole::User))
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>();
        let expected = if expect_first {
            vec![
                "Lower the work-log grouping threshold and record a spec.",
                "Now widen the change kinds.",
            ]
        } else {
            vec!["Now widen the change kinds."]
        };
        assert_eq!(users, expected, "{case}");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.metadata["tool_name"] == "Edit")
                .count(),
            2,
            "{case}"
        );
    }
}

pub(crate) async fn register_capture_test_agent(
    state: &AppState,
    config: &AgentConfig,
    active: crate::state::ActiveAgent,
) -> Result<(), String> {
    let pending = super::PendingRuntime::prepare(config, &state.terminal_sessions)?.attach(active);
    let mut completion = None;
    super::codex_onboarding::commit_registered_agent(
        state,
        &config.session_id,
        pending,
        &mut completion,
        super::AgentOrderPlacement::Top,
    )
    .await
    .map_err(|(_, error)| error)
}

#[test]
fn pi_fresh_provider_session_promotion_retains_launch_provenance() {
    let mut new_active = make_test_agent();
    {
        let mut config = new_active.config.lock().unwrap();
        config.fresh_provider_session_id = Some("pi-fresh-session".to_string());
        config.resume_session = None;
    }

    promote_fresh_provider_session_after_resume("pi", &mut new_active);

    let config = new_active.config.lock().unwrap();
    assert_eq!(config.resume_session.as_deref(), Some("pi-fresh-session"));
    assert_eq!(
        config.fresh_provider_session_id.as_deref(),
        Some("pi-fresh-session")
    );
}

#[test]
fn pi_initial_capture_provenance_requires_the_launch_owned_identity() {
    let fresh_config = AgentConfig {
        provider: "pi".to_string(),
        fresh_provider_session_id: Some("pi-fresh-session".to_string()),
        ..AgentConfig::default()
    };
    assert_eq!(
        fresh_provider_session_for_initial_capture(&fresh_config, Some("pi-fresh-session")),
        Some("pi-fresh-session".to_string())
    );
    assert_eq!(
        fresh_provider_session_for_initial_capture(&fresh_config, Some("different-session")),
        None
    );

    let resumed_config = AgentConfig {
        provider: "pi".to_string(),
        resume_session: Some("pi-resumed-session".to_string()),
        fresh_provider_session_id: Some("pi-fresh-session".to_string()),
        ..AgentConfig::default()
    };
    assert_eq!(
        fresh_provider_session_for_initial_capture(&resumed_config, Some("pi-resumed-session")),
        None
    );
}

#[test]
fn pi_provider_log_policy_baselines_resume_and_skips_disabled_span() {
    // Acquisition policy evidence only. The provider Init-to-capture
    // regression belongs at the actual spawn helper seam.
    let temp = tempfile::tempdir().expect("provider log temp dir");
    let resume_path = temp.path().join("resume-pi.jsonl");
    std::fs::write(
        &resume_path,
        concat!(
            r#"{"type":"session","id":"pi-historical-session"}"#,
            "\n",
            r#"{"type":"message_end","message":{"role":"assistant","content":"Historical prefix","stopReason":"stop"}}"#,
            "\n"
        ),
    )
    .expect("write historical Pi prefix");

    let resumed_config = AgentConfig {
        provider: "pi".to_string(),
        resume_session: Some("pi-resumed-session".to_string()),
        ..AgentConfig::default()
    };
    let resumed_identity =
        fresh_provider_session_for_initial_capture(&resumed_config, Some("pi-resumed-session"));
    assert!(resumed_identity.is_none());
    let resumed = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "pi",
        &resume_path,
        "pi:session:pi-resumed-session",
        None,
        false,
    )
    .expect("baseline ordinary Pi resume");
    assert!(resumed.events.is_empty());
    assert_eq!(
        resumed.next.unknown_before_offset,
        Some(std::fs::metadata(&resume_path).unwrap().len())
    );

    let disabled_path = temp.path().join("disabled-pi.jsonl");
    std::fs::write(
        &disabled_path,
        r#"{"type":"message_end","message":{"role":"assistant","content":"Before disabled","stopReason":"stop"}}
"#,
    )
    .expect("write disabled Pi prefix");
    let disabled = crate::commands::provider_log_acquisition::observe_provider_log_policy(
        &disabled_path,
        "pi:session:disabled",
        None,
        false,
        true,
    )
    .expect("open disabled Pi policy");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&disabled_path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(
                file,
                r#"{{"type":"message_end","message":{{"role":"assistant","content":"Hidden while disabled","stopReason":"stop"}}}}"#
            )
        })
        .expect("write disabled Pi answer");
    let enabled = crate::commands::provider_log_acquisition::observe_provider_log_policy(
        &disabled_path,
        "pi:session:disabled",
        Some(disabled.next),
        true,
        true,
    )
    .expect("close disabled Pi policy");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&disabled_path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(
                file,
                r#"{{"type":"message_end","message":{{"role":"assistant","content":"Visible after disabled","stopReason":"stop"}}}}"#
            )
        })
        .expect("write enabled Pi answer");

    let skipped = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "pi",
        &disabled_path,
        "pi:session:disabled",
        Some(enabled.next),
        true,
    )
    .expect("skip the disabled Pi span");
    assert!(skipped.events.is_empty());
    assert!(skipped.continue_immediately);
    let visible = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "pi",
        &disabled_path,
        "pi:session:disabled",
        Some(skipped.next),
        true,
    )
    .expect("capture only post-policy Pi bytes");
    assert_eq!(
        visible
            .events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        vec!["Visible after disabled"]
    );
}

#[test]
fn generated_claude_and_captured_codex_identities_survive_registration_shape() {
    for provider in ["claude", "codex"] {
        let mut fresh_config = AgentConfig {
            provider: provider.to_string(),
            fresh_provider_session_id: Some("fresh-provider-session".to_string()),
            ..AgentConfig::default()
        };
        assert_eq!(
            fresh_provider_session_for_initial_capture(
                &fresh_config,
                Some("fresh-provider-session")
            ),
            Some("fresh-provider-session".to_string())
        );
        fresh_config.resume_session = Some("fresh-provider-session".to_string());
        assert_eq!(
            fresh_provider_session_for_initial_capture(
                &fresh_config,
                Some("fresh-provider-session")
            ),
            Some("fresh-provider-session".to_string())
        );
    }
}

#[test]
fn empty_or_mismatched_runtime_identity_fails_closed() {
    let cases = [
        (Some("fresh-provider-session"), Some("different-session")),
        (Some(""), Some("fresh-provider-session")),
        (Some("fresh-provider-session"), Some("")),
    ];
    for (fresh, resume) in cases {
        let config = AgentConfig {
            provider: "claude".to_string(),
            fresh_provider_session_id: fresh.map(str::to_string),
            resume_session: resume.map(str::to_string),
            ..AgentConfig::default()
        };
        assert_eq!(
            fresh_provider_session_for_initial_capture(&config, Some("fresh-provider-session")),
            None
        );
    }
}

#[tokio::test]
async fn agent_logging_transition_excludes_provider_bytes_written_while_disabled() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
        .expect("initialize isolated state database");
    crate::utils::save_shell_settings(&crate::utils::ShellSettings {
        conversation_logging: ConversationLoggingSetting::Enabled,
        ..Default::default()
    })
    .expect("save enabled global logging");
    let log_path = temp.path().join("provider.jsonl");
    std::fs::write(
        &log_path,
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Before agent disable\"}}\n",
    )
    .expect("write initial provider event");
    let state = AppState::new();
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().expect("agent config");
        config.session_id = "agent-1".to_string();
        config.session_name = "Agent One".to_string();
        config.agent_class = "Coder".to_string();
        config.provider = "codex".to_string();
        config.reset_provider_config_for_provider();
        config.folder = temp.path().to_string_lossy().to_string();
        config.fresh_provider_session_id = Some("provider-session-1".to_string());
        config.conversation_logging = AgentConversationLoggingSetting::Default;
    }
    *agent.log_path.lock().expect("agent log path") = Some(log_path.clone());
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent);
    state.agent_order.lock().await.push("agent-1".to_string());
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
        .await
        .expect("capture enabled prefix");

    let mut disabled = lifecycle_config_for_session(&state, "agent-1")
        .await
        .expect("load agent config");
    disabled.conversation_logging = AgentConversationLoggingSetting::Disabled;
    persist_agent_config_for_test(disabled, &state)
        .await
        .expect("disable agent logging");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"SECRET_AGENT_DISABLED\"}}}}")
        })
        .expect("append disabled provider event");

    let mut enabled = lifecycle_config_for_session(&state, "agent-1")
        .await
        .expect("reload agent config");
    enabled.conversation_logging = AgentConversationLoggingSetting::Enabled;
    persist_agent_config_for_test(enabled, &state)
        .await
        .expect("re-enable agent logging");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"After agent re-enable\"}}}}")
        })
        .expect("append enabled provider event");
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
        .await
        .expect("capture after agent re-enable");

    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, "agent-1")
        .await
        .expect("capture snapshot");
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archived = state
        .conversation_archive
        .chat_events_for_capture(&context)
        .expect("read archive");
    let text = archived
        .iter()
        .filter_map(|event| event.text.as_deref())
        .collect::<Vec<_>>();
    assert!(text.contains(&"Before agent disable"));
    assert!(text.contains(&"After agent re-enable"));
    assert!(!text.contains(&"SECRET_AGENT_DISABLED"));
}

#[tokio::test]
async fn lifecycle_archive_drain_does_not_queue_behind_background_syncs() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
        .expect("initialize isolated state database");
    crate::utils::save_shell_settings(&crate::utils::ShellSettings {
        conversation_logging: ConversationLoggingSetting::Enabled,
        ..Default::default()
    })
    .expect("save enabled global logging");
    let log_path = temp.path().join("provider.jsonl");
    std::fs::write(
        &log_path,
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Closing turn\"}}\n",
    )
    .expect("write provider event");
    let state = Arc::new(AppState::new());
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().expect("agent config");
        config.session_id = "agent-1".to_string();
        config.session_name = "Agent One".to_string();
        config.agent_class = "Coder".to_string();
        config.provider = "codex".to_string();
        config.reset_provider_config_for_provider();
        config.folder = temp.path().to_string_lossy().to_string();
        config.fresh_provider_session_id = Some("provider-session-1".to_string());
    }
    *agent.log_path.lock().expect("agent log path") = Some(log_path);
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent);
    state.agent_order.lock().await.push("agent-1".to_string());

    // A capture pass is running, and several best-effort syncs (the restore and
    // status syncs a restart schedules for every agent) are already waiting.
    let running_pass = state.conversation_capture_policy_lock.lock().await;
    let finished = Arc::new(AtomicUsize::new(0));
    let mut background = Vec::new();
    for _ in 0..4 {
        let state = state.clone();
        let finished = finished.clone();
        background.push(tokio::spawn(async move {
            crate::commands::chat::archive_agent_chat_events_until_stable_for_state(
                &state, "agent-1",
            )
            .await
            .expect("background sync");
            finished.fetch_add(1, Ordering::SeqCst)
        }));
    }
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let lifecycle = {
        let state = state.clone();
        let finished = finished.clone();
        tokio::spawn(async move {
            crate::commands::chat::archive_agent_chat_events_until_stable_for_lifecycle(
                &state, "agent-1",
            )
            .await
            .expect("lifecycle drain");
            finished.fetch_add(1, Ordering::SeqCst)
        })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }

    drop(running_pass);
    let lifecycle_position = lifecycle.await.expect("lifecycle task");
    for task in background {
        task.await.expect("background task");
    }

    assert_eq!(
        lifecycle_position, 0,
        "New Session's closing drain must finish before the queued background syncs"
    );
}

#[tokio::test]
async fn background_capture_waiting_through_a_boundary_does_not_replay_the_old_session() {
    use std::sync::Arc;
    use wardian_core::conversations::{ConversationBoundaryReason, ConversationStatus};

    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
        .expect("initialize isolated state database");
    crate::utils::save_shell_settings(&crate::utils::ShellSettings {
        conversation_logging: ConversationLoggingSetting::Enabled,
        ..Default::default()
    })
    .expect("save enabled global logging");
    let user_message = |text: &str| {
        format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{text}\"}}}}\n")
    };
    let old_log = temp.path().join("old-provider.jsonl");
    std::fs::write(&old_log, user_message("Old session prompt")).expect("write old log");
    let new_log = temp.path().join("new-provider.jsonl");
    std::fs::write(&new_log, user_message("New session prompt")).expect("write new log");

    let state = Arc::new(AppState::new());
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().expect("agent config");
        config.session_id = "agent-1".to_string();
        config.session_name = "Agent One".to_string();
        config.agent_class = "Coder".to_string();
        config.provider = "codex".to_string();
        config.reset_provider_config_for_provider();
        config.folder = temp.path().to_string_lossy().to_string();
        config.fresh_provider_session_id = Some("provider-session-1".to_string());
    }
    *agent.log_path.lock().expect("agent log path") = Some(old_log.clone());
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent);
    state.agent_order.lock().await.push("agent-1".to_string());
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, "agent-1")
        .await
        .expect("capture the old session");

    // A best-effort pass is waiting for the gate while a boundary runs.
    let running_pass = state.conversation_capture_policy_lock.lock().await;
    let waiting_pass = {
        let state = state.clone();
        tokio::spawn(async move {
            crate::commands::chat::archive_agent_chat_events_until_stable_for_state(
                &state, "agent-1",
            )
            .await
            .expect("waiting background pass");
        })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }

    // The boundary rotates the archive and installs the new provider session;
    // the old provider then writes one last line.
    state
        .conversation_archive
        .rollover_agent("agent-1", ConversationBoundaryReason::Clear)
        .expect("roll over the archive")
        .expect("an active conversation to close");
    {
        let agents = state.agents.lock().await;
        let agent = agents.get("agent-1").expect("agent");
        agent
            .config
            .lock()
            .expect("agent config")
            .fresh_provider_session_id = Some("provider-session-2".to_string());
        *agent.log_path.lock().expect("agent log path") = Some(new_log);
    }
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&old_log)
            .expect("open old log");
        file.write_all(user_message("Late old-session line").as_bytes())
            .expect("append late line");
    }

    drop(running_pass);
    waiting_pass.await.expect("waiting pass");

    let entries = state
        .conversation_archive
        .list(Some("agent-1"), false)
        .expect("list conversations");
    let old_session = entries
        .iter()
        .filter(|entry| {
            entry
                .provider_session_ids
                .iter()
                .any(|id| id == "provider-session-1")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        old_session.len(),
        1,
        "a stale pass must not create a second conversation for the old session: {entries:?}"
    );
    assert!(matches!(old_session[0].status, ConversationStatus::Closed));
    assert!(matches!(
        old_session[0].boundary_reason,
        ConversationBoundaryReason::Clear
    ));
    for open in entries
        .iter()
        .filter(|entry| matches!(entry.status, ConversationStatus::Open))
    {
        assert!(
            open.provider_session_ids
                .iter()
                .any(|id| id == "provider-session-2"),
            "an open conversation must belong to the new session: {open:?}"
        );
    }
}
