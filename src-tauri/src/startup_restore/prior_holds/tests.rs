use super::*;
use wardian_core::control::{InteractionBodyRef, InteractionStatus, InteractionTriggerPolicy};

fn config() -> AgentConfig {
    AgentConfig {
        session_id: "agent-1".into(),
        session_name: "Held Agent".into(),
        provider: "codex".into(),
        resume_session: Some("saved-session".into()),
        is_off: true,
        ..Default::default()
    }
}

fn hold(session: &str, acquisition: &str) -> ConversationLease {
    ConversationLease {
        agent_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: session.into(),
        owner_kind: "prior_provider_hold".into(),
        owner_id: format!("owner-{acquisition}"),
        acquisition_id: acquisition.into(),
        owner_node_id: None,
        mode: "uncertain_previous_provider".into(),
        started_at: "2000-01-01T00:00:00Z".into(),
        heartbeat_at: "2000-01-01T00:00:00Z".into(),
        expires_at: "9999-12-31T23:59:59Z".into(),
    }
}

#[test]
fn codex_retained_hold_diagnostic_explains_out_of_job_writer_evidence() {
    let reports = plan_retained_holds(&[config()], &[hold("saved-session", "writer-evidence")]);
    assert_eq!(reports.len(), 1);
    assert!(reports[0].blocks_saved_session);
    assert!(reports[0].reason.contains("out-of-job Codex daemon"));
    assert!(reports[0].reason.contains("supported unload evidence"));
    assert!(reports[0]
        .recovery_command
        .as_deref()
        .unwrap()
        .contains("--operator-verified-exit"));
}

#[test]
fn provider_label_and_hold_age_never_substitute_for_creator_and_launch_evidence() {
    for provider in ["codex", "claude", "pi", "mock"] {
        let mut config = config();
        config.provider = provider.into();
        for started_at in ["2000-01-01T00:00:00Z", "2099-01-01T00:00:00Z"] {
            let mut hold = hold("saved-session", "exact-acquisition");
            hold.provider = provider.into();
            hold.started_at = started_at.into();
            let reports = plan_retained_holds(std::slice::from_ref(&config), &[hold]);
            assert_eq!(reports.len(), 1);
            assert!(
                reports[0].blocks_saved_session,
                "Off agents must be reported too"
            );
            assert!(reports[0].reason.contains("creating-instance"));
            assert!(reports[0].reason.contains("cannot prove"));
            assert_eq!(reports[0].recovery_command.as_deref(), Some(
                "wardian agent release-hold agent-1 --acquisition-id exact-acquisition --operator-verified-exit"
            ));
            if provider == "codex" {
                assert!(reports[0].reason.contains("breakaway-enabled"));
                assert!(reports[0].reason.contains("followed process spawn"));
            }
        }
    }
}

#[test]
fn historical_holds_are_reported_without_claiming_the_saved_session_is_blocked() {
    let config = config();
    let older = hold("older-session", "older-acquisition");
    let mut unrelated = hold("saved-session", "execution-acquisition");
    unrelated.owner_kind = "automation_run".into();
    unrelated.mode = "background_resume".into();
    let reports = plan_retained_holds(&[config], &[older, unrelated]);
    assert_eq!(reports.len(), 1);
    assert!(!reports[0].blocks_saved_session);
    assert_eq!(reports[0].acquisition_id, "older-acquisition");
    assert!(reports[0].recovery_command.is_some());
}

#[test]
fn malformed_or_orphaned_identity_never_produces_an_executable_recovery_command() {
    let mut config = config();
    config.session_id = "unsafe;agent".into();
    let mut unsafe_hold = hold("saved-session", "unsafe;acquisition");
    unsafe_hold.agent_id = config.session_id.clone();
    let report = plan_retained_holds(&[config], &[unsafe_hold]);
    assert!(report[0].blocks_saved_session);
    assert!(report[0].recovery_command.is_none());
    let orphan = plan_retained_holds(&[], &[hold("saved-session", "exact")]);
    assert!(orphan[0].recovery_command.is_none());
    assert!(!orphan[0].blocks_saved_session);
}

struct RestoreHome(Option<std::ffi::OsString>);

impl Drop for RestoreHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

#[tokio::test]
async fn startup_persists_informational_diagnostic_without_releasing_holds_or_editing_history() {
    use wardian_core::conversation_lease::{load_leases_checked, save_leases};
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let home = tempfile::tempdir().unwrap();
    let _restore_home = RestoreHome(std::env::var_os("WARDIAN_HOME"));
    std::env::set_var("WARDIAN_HOME", home.path());
    wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
    let config = config();
    let leases = vec![
        hold("saved-session", "current"),
        hold("older-session", "older"),
    ];
    save_leases(&leases).unwrap();
    let history = home
        .path()
        .join("agents/agent-1/conversations/retained.jsonl");
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    std::fs::write(&history, "retained historical evidence\n").unwrap();
    let interactions = InteractionState::default();
    let reports = plan_retained_holds(&[config], &load_leases_checked().unwrap());
    assert_eq!(persist_reports(&interactions, reports).await.unwrap(), 1);
    assert_eq!(load_leases_checked().unwrap(), leases);
    assert_eq!(
        std::fs::read_to_string(history).unwrap(),
        "retained historical evidence\n"
    );
    let (notifications, truncated) = interactions.inbox_notifications_page(0, 10).await;
    assert!(!truncated);
    assert_eq!(notifications.len(), 1);
    let notification = &notifications[0];
    assert_eq!(notification.status, InteractionStatus::Completed);
    assert_eq!(
        notification.trigger_policy,
        InteractionTriggerPolicy::NotifyOnly
    );
    let InteractionBodyRef::Inline { body } = &notification.body_ref else {
        panic!("inline diagnostic")
    };
    let payload: InboxNotificationPayload = serde_json::from_str(body).unwrap();
    assert_eq!(payload.kind, InboxNotificationKind::Update);
    assert!(
        payload.choices.is_empty(),
        "startup must not invent an approval prompt"
    );
    assert!(payload.body.contains("Restart did not release"));
    assert!(payload
        .proposed_action
        .as_deref()
        .unwrap()
        .contains("--acquisition-id current"));
}
