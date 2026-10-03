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
async fn identical_fallback_provider_rows_keep_distinct_archive_and_chat_identity() {
    let _env_lock = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("isolated Wardian home");
    struct RestoreWardianHome(Option<std::ffi::OsString>);
    impl Drop for RestoreWardianHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => std::env::set_var("WARDIAN_HOME", home),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }
    let _home = RestoreWardianHome(std::env::var_os("WARDIAN_HOME"));
    std::env::set_var("WARDIAN_HOME", temp.path());
    wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
        .expect("initialize isolated state database");
    crate::utils::save_shell_settings(&crate::utils::ShellSettings {
        conversation_logging: ConversationLoggingSetting::Enabled,
        ..Default::default()
    })
    .expect("enable conversation logging");

    let log_path = temp.path().join("mock-provider.jsonl");
    let prefix_template = r#"{"type":"ignored","padding":"{}"}"#;
    let prefix_base = prefix_template.replace("{}", "");
    let prefix_line = prefix_template.replace(
        "{}",
        &"x".repeat(310_usize.saturating_sub(prefix_base.len())),
    );
    let row_template = r#"{"type":"message","message":{"role":"user","content":"Repeat this request"},"padding":"{}"}"#;
    let row_base = row_template.replace("{}", "");
    let row = row_template.replace("{}", &"x".repeat(203_usize.saturating_sub(row_base.len())));
    assert_eq!(prefix_line.len(), 310);
    assert_eq!(row.len(), 203);
    std::fs::write(&log_path, format!("{prefix_line}\n{row}\n"))
        .expect("write first provider row at byte offset 311");
    let state = AppState::new();
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().expect("agent config");
        config.session_id = "agent-1".to_string();
        config.session_name = "Agent One".to_string();
        config.agent_class = "Coder".to_string();
        config.provider = "pi".to_string();
        config.reset_provider_config_for_provider();
        config.folder = temp.path().to_string_lossy().to_string();
        config.fresh_provider_session_id = Some("pi-session-1".to_string());
        config.conversation_logging = AgentConversationLoggingSetting::Enabled;
    }
    *agent.log_path.lock().expect("agent log path") = Some(log_path.clone());
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent);
    state.agent_order.lock().await.push("agent-1".to_string());

    let first = crate::commands::chat::archive_agent_chat_events_for_state(&state, "agent-1")
        .await
        .expect("capture first provider row");
    let first_event = first
        .events
        .iter()
        .find(|event| event.metadata["provider_log"] == true)
        .expect("first provider event")
        .clone();
    assert_eq!(
        first_event.metadata["provider_log_row_offset"].as_u64(),
        Some(311)
    );
    let context = first.context.clone();
    let conversation_id = state
        .conversation_archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let (_, records_before_second) = state
        .conversation_archive
        .show(&conversation_id)
        .expect("read first narrative");

    use std::io::Write as _;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .expect("open provider log for append"),
        "{row}"
    )
    .expect("append identical second row");
    let second = crate::commands::chat::archive_agent_chat_events_for_state(&state, "agent-1")
        .await
        .expect("capture second distinct provider row");
    let second_event = second
        .events
        .iter()
        .find(|event| event.metadata["provider_log"] == true)
        .expect("second provider event")
        .clone();
    assert_eq!(
        second_event.metadata["provider_log_row_offset"].as_u64(),
        Some(515)
    );
    assert_ne!(first_event.id, second_event.id);

    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, "agent-1")
        .await
        .expect("snapshot for direct provider-log projection");
    let tail_projection = crate::commands::chat::collect_agent_chat_events_for_archive(&snapshot)
        .expect("project complete provider log");
    let tail_events = tail_projection
        .events
        .iter()
        .filter(|event| event.metadata["provider_log"] == true)
        .collect::<Vec<_>>();
    let first_tail_event = tail_events
        .iter()
        .find(|event| event.metadata["provider_log_row_offset"].as_u64() == Some(311))
        .expect("first direct-tail observation");
    let second_tail_event = tail_events
        .iter()
        .find(|event| event.metadata["provider_log_row_offset"].as_u64() == Some(515))
        .expect("second direct-tail observation");
    assert_eq!(first_tail_event.id, first_event.id);
    assert_eq!(second_tail_event.id, second_event.id);

    let chat_events = state
        .conversation_archive
        .chat_events_for_capture(&context)
        .expect("read archived chat events");
    assert_eq!(
        chat_events
            .iter()
            .filter(|event| event.text.as_deref() == Some("Repeat this request"))
            .count(),
        2,
        "Chat must retain both identical provider rows"
    );
    let second_archived_event = chat_events
        .iter()
        .find(|event| event.metadata["provider_log_row_offset"].as_u64() == Some(515))
        .expect("persisted second provider observation")
        .clone();
    state
        .conversation_archive
        .append_chat_events_with_context(
            context.clone(),
            std::slice::from_ref(&second_archived_event),
        )
        .expect("replaying the same source row is idempotent");
    let conversation_id = state
        .conversation_archive
        .active_conversation_id_for_test("agent-1")
        .expect("active conversation");
    let (_, records) = state
        .conversation_archive
        .show(&conversation_id)
        .expect("read narratives");
    assert_eq!(records.len(), 2);
    assert_ne!(records[0].event_refs, records[1].event_refs);
    assert_eq!(records_before_second[0].event_refs, records[0].event_refs);
    assert_eq!(records_before_second[0].source_refs, records[0].source_refs);

    let chat_read =
        crate::commands::chat::load_agent_chat_transcript_for_state(&state, "agent-1".to_string())
            .await
            .expect("read the transcript through Chat");
    assert_eq!(
        chat_read
            .iter()
            .filter(|event| event.text.as_deref() == Some("Repeat this request"))
            .count(),
        2
    );
    let chat_ids = chat_read
        .iter()
        .filter(|event| event.text.as_deref() == Some("Repeat this request"))
        .map(|event| event.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(chat_ids.contains(first_event.id.as_str()));
    assert!(chat_ids.contains(second_event.id.as_str()));
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
