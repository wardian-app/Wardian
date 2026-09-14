use super::agent_lifecycle::fresh_provider_session_for_initial_capture;
use super::tests::{make_test_agent, WardianHomeGuard};
use super::{
    lifecycle_config_for_session, persist_agent_config_while_lifecycle_locked,
    promote_fresh_provider_session_after_resume,
};
use crate::state::AppState;
use wardian_core::conversations::{AgentConversationLoggingSetting, ConversationLoggingSetting};
use wardian_core::models::AgentConfig;

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
    persist_agent_config_while_lifecycle_locked(disabled, &state)
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
    persist_agent_config_while_lifecycle_locked(enabled, &state)
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
