use super::spawn::capture_codex_init_resume_session;
use wardian_core::models::AgentConfig;

#[test]
fn codex_init_session_id_sets_empty_resume_session() {
    let mut config = AgentConfig {
        provider: "codex".to_string(),
        resume_session: None,
        ..Default::default()
    };

    let changed = capture_codex_init_resume_session(
        "codex",
        "019db2f3-22de-7861-8bc6-1b86db1686db",
        &mut config,
    );

    assert!(changed);
    assert_eq!(
        config.resume_session.as_deref(),
        Some("019db2f3-22de-7861-8bc6-1b86db1686db")
    );
}

#[test]
fn codex_init_session_id_does_not_replace_existing_resume_session() {
    let mut config = AgentConfig {
        provider: "codex".to_string(),
        resume_session: Some("existing-codex-thread".to_string()),
        ..Default::default()
    };

    let changed = capture_codex_init_resume_session("codex", "new-codex-thread", &mut config);

    assert!(!changed);
    assert_eq!(
        config.resume_session.as_deref(),
        Some("existing-codex-thread")
    );
}

#[test]
fn non_codex_init_session_id_does_not_use_codex_capture_path() {
    let mut config = AgentConfig {
        provider: "claude".to_string(),
        resume_session: None,
        ..Default::default()
    };

    let changed = capture_codex_init_resume_session("claude", "claude-thread", &mut config);

    assert!(!changed);
    assert_eq!(config.resume_session, None);
}
