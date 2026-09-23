use super::session_identity::ProviderIdentityOutcome;
use super::spawn::{codex_shared_tui_args, handle_provider_init_event};
use std::path::Path;
use std::sync::{Arc, Mutex};
use wardian_core::models::{AgentConfig, AgentEvent};

fn config(
    provider: &str,
    wardian_session_id: &str,
    resume_session: Option<&str>,
    fresh_provider_session_id: Option<&str>,
) -> Arc<Mutex<AgentConfig>> {
    Arc::new(Mutex::new(AgentConfig {
        provider: provider.to_string(),
        session_id: wardian_session_id.to_string(),
        resume_session: resume_session.map(str::to_string),
        fresh_provider_session_id: fresh_provider_session_id.map(str::to_string),
        ..Default::default()
    }))
}

fn init(session_id: &str) -> AgentEvent {
    AgentEvent::Init {
        session_id: session_id.to_string(),
        timestamp: Some("2026-07-16T12:00:00Z".to_string()),
    }
}

#[test]
fn shared_codex_tui_args_preserve_identity_without_overriding_screen_mode() {
    let prefix = vec!["codex-wrapper".to_string(), "--wrapper-option".to_string()];
    let workspace = Path::new("workspace");
    let model = "gpt-5.6-luna";
    let thread_id = "019db2f3-22de-7861-8bc6-1b86db1686db";

    let fresh = codex_shared_tui_args(prefix.clone(), Some(model), None, workspace);
    assert_eq!(
        fresh,
        vec![
            "codex-wrapper",
            "--wrapper-option",
            "--model",
            model,
            "--cd",
            "workspace",
        ]
    );

    let resumed = codex_shared_tui_args(prefix, Some(model), Some(thread_id), workspace);
    assert_eq!(
        resumed,
        vec![
            "codex-wrapper",
            "--wrapper-option",
            "--model",
            model,
            "resume",
            thread_id,
            "--cd",
            "workspace",
        ]
    );
    assert!(!fresh.iter().any(|argument| argument == "-c"));
    assert!(!resumed.iter().any(|argument| argument == "-c"));
    assert!(!fresh.iter().any(|argument| argument == "--no-alt-screen"));
    assert!(!resumed.iter().any(|argument| argument == "--no-alt-screen"));
}

#[test]
fn matching_claude_init_confirms_identity_then_captures_timestamp() {
    let config = config("claude", "wardian-id", Some("claude-id"), None);
    let timestamp = Arc::new(Mutex::new(None));
    assert_eq!(
        handle_provider_init_event("claude", &init("claude-id"), &config, &timestamp),
        Ok(ProviderIdentityOutcome::Confirmed)
    );
    assert_eq!(
        config.lock().unwrap().resume_session.as_deref(),
        Some("claude-id")
    );
    assert_eq!(
        timestamp.lock().unwrap().as_deref(),
        Some("2026-07-16T12:00:00Z")
    );
}

#[test]
fn conflicting_caller_owned_init_leaves_identity_and_timestamp_unchanged() {
    for provider in ["claude", "gemini", "pi", "mock"] {
        let config = config(provider, "wardian-id", Some("expected-id"), None);
        let timestamp = Arc::new(Mutex::new(None));
        assert!(
            handle_provider_init_event(provider, &init("conflicting-id"), &config, &timestamp,)
                .is_err()
        );
        assert_eq!(
            config.lock().unwrap().resume_session.as_deref(),
            Some("expected-id")
        );
        assert_eq!(*timestamp.lock().unwrap(), None);
    }
}

#[test]
fn matching_fresh_pi_init_confirms_exact_project_session() {
    let config = config("pi", "wardian-id", None, Some("pi-session-id"));
    let timestamp = Arc::new(Mutex::new(None));

    assert_eq!(
        handle_provider_init_event("pi", &init("pi-session-id"), &config, &timestamp),
        Ok(ProviderIdentityOutcome::Confirmed)
    );
}

#[test]
fn matching_resumed_codex_init_confirms_exact_thread() {
    let thread_id = "019db2f3-22de-7861-8bc6-1b86db1686db";
    let config = config("codex", "wardian-id", Some(thread_id), None);
    let timestamp = Arc::new(Mutex::new(None));
    assert_eq!(
        handle_provider_init_event("codex", &init(thread_id), &config, &timestamp),
        Ok(ProviderIdentityOutcome::Confirmed)
    );
}

#[test]
fn malformed_fresh_codex_init_does_not_capture_identity_or_timestamp() {
    let config = config("codex", "wardian-id", None, None);
    let timestamp = Arc::new(Mutex::new(None));
    assert!(
        handle_provider_init_event("codex", &init("not-a-thread-id"), &config, &timestamp,)
            .is_err()
    );
    assert_eq!(config.lock().unwrap().resume_session, None);
    assert_eq!(*timestamp.lock().unwrap(), None);
}

#[test]
fn valid_unbound_codex_init_cannot_become_persisted_identity() {
    let config = config("codex", "wardian-id", None, None);
    let timestamp = Arc::new(Mutex::new(None));
    let thread_id = "019db2f3-22de-7861-8bc6-1b86db1686db";

    assert!(handle_provider_init_event("codex", &init(thread_id), &config, &timestamp).is_err());
    assert_eq!(config.lock().unwrap().resume_session, None);
    assert_eq!(*timestamp.lock().unwrap(), None);
}
