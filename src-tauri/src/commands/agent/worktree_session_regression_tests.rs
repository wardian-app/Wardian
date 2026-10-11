//! #1665 identity, capability, and deletion preflight regressions.
//! Pure config/lifecycle boundaries only: no provider is launched or moved.
use super::*;

fn configuration(provider: &str, source: &std::path::Path) -> AgentConfig {
    let mut config = AgentConfig {
        session_id: "wardian-owner".into(),
        session_name: "FixtureOwner".into(),
        provider: provider.into(),
        folder: source.to_string_lossy().into_owned(),
        resume_session: Some("provider-thread".into()),
        fresh_provider_session_id: Some("existing-fresh-identity".into()),
        session_persistence: AgentSessionPersistenceOverride::Resume,
        ..Default::default()
    };
    config.reset_provider_config_for_provider();
    config
}

#[test]
fn assignment_helpers_preserve_exact_provider_and_wardian_identity() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("repo");
    let first = root.path().join("repo.wt/first");
    let second = root.path().join("repo.wt/second");
    for provider in ["codex", "claude", "opencode", "pi", "mock"] {
        let mut config = configuration(provider, &source);
        let original = config.clone();
        enable_worktree_config(&mut config, &first);
        assign_worktree_config(&mut config, second.to_str().unwrap()).unwrap();
        assert_eq!(config.session_id, original.session_id);
        assert_eq!(config.resume_session, original.resume_session);
        assert_eq!(
            config.fresh_provider_session_id,
            original.fresh_provider_session_id
        );
        assert_eq!(config.provider_config, original.provider_config);
        disable_worktree_config(&mut config).unwrap();
        assert_eq!(config.folder, original.folder);
        assert_eq!(config.resume_session, original.resume_session);
    }
}

#[test]
fn explicit_user_clear_still_removes_provider_identity() {
    let root = tempfile::tempdir().unwrap();
    for provider in ["codex", "claude", "opencode", "pi", "mock"] {
        let mut config = configuration(provider, &root.path().join("repo"));
        assign_worktree_config(
            &mut config,
            root.path().join("repo.wt/review").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(config.resume_session.as_deref(), Some("provider-thread"));
        // Explicit user Clear remains an intentional conversation reset.
        prepare_clear_config(&mut config).unwrap();
        assert!(config.resume_session.is_none(), "{provider}");
        assert_ne!(
            config.fresh_provider_session_id.as_deref(),
            Some("existing-fresh-identity")
        );
        assert_eq!(config.session_id, "wardian-owner");
    }
}

#[test]
fn resume_policy_override_must_be_explicit_for_preserving_assignment() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    let mut config = configuration("mock", &root.path().join("repo"));
    config.session_persistence = AgentSessionPersistenceOverride::Fresh;
    assert_eq!(
        resolved_session_persistence(&config),
        AgentSessionPersistence::Fresh
    );
    config.session_persistence = AgentSessionPersistenceOverride::Resume;
    assert_eq!(
        resolved_session_persistence(&config),
        AgentSessionPersistence::Resume
    );
}

#[test]
fn codex_resume_args_pair_exact_thread_with_new_workspace() {
    use wardian_core::models::provider::AgentProvider;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("repo");
    let target = root.path().join("repo.wt/review");
    let mut config = configuration("codex", &source);
    assign_worktree_config(&mut config, target.to_str().unwrap()).unwrap();
    let provider_args = crate::providers::CodexProvider::new().get_spawn_args(&config, true);
    let args = manager::interactive_provider_args("codex", &target, &target, provider_args);
    assert_eq!(&args[..2], &["resume", "provider-thread"]);
    let cwd = args.iter().position(|arg| arg == "--cd").unwrap();
    assert_eq!(args[cwd + 1], target.to_string_lossy());
    assert!(!args.iter().any(|arg| arg == "--last"));
    assert_eq!(config.session_id, "wardian-owner");
}

#[test]
fn desired_assignment_transition_retains_provider_thread() {
    let root = tempfile::tempdir().unwrap();
    let mut config = configuration("codex", &root.path().join("repo"));
    assign_worktree_config(
        &mut config,
        root.path().join("repo.wt/review").to_str().unwrap(),
    )
    .unwrap();
    WorkspaceMutation::Enable(root.path().join("repo.wt/next"))
        .apply(&mut config)
        .unwrap();
    assert_eq!(
        config.resume_session.as_deref(),
        Some("provider-thread"),
        "#1665: assigning a workspace must not silently clear the provider thread"
    );
}

#[test]
fn worktree_preflight_preserves_identity_and_policy_without_publishing_folder() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("repo");
    let target = root.path().join("repo.wt/review");
    let mut config = configuration("mock", &source);
    config.session_persistence = AgentSessionPersistenceOverride::Fresh;
    let before = config.clone();
    let prepared = workspace_resume::prepare_workspace_resume_config(
        &config,
        &WorkspaceMutation::Enable(target.clone()),
    )
    .unwrap();
    assert_eq!(prepared.resume_session, before.resume_session);
    assert!(prepared.exact_resume.is_some());
    prepared.validate_exact_resume_identity().unwrap();
    assert_eq!(prepared.session_id, before.session_id);
    assert_eq!(
        prepared.session_persistence,
        AgentSessionPersistenceOverride::Fresh
    );
    assert_eq!(prepared.folder, normalize_workspace_record_path(&target));
    assert_eq!(
        serde_json::to_value(&config).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}

#[test]
fn worktree_preflight_rejects_malformed_identity_without_mutating_owner() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    for (provider, identity) in [
        ("codex", "malformed"),
        ("opencode", "malformed"),
        ("mock", "wardian-owner"),
    ] {
        let mut config = configuration(provider, &root.path().join("repo"));
        config.resume_session = Some(identity.into());
        let before = config.clone();
        let error = workspace_resume::prepare_workspace_resume_config(
            &config,
            &WorkspaceMutation::Enable(root.path().join("repo.wt/review")),
        )
        .unwrap_err();
        assert!(error.contains("identity"), "{error}");
        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }
}

#[test]
fn worktree_preflight_refuses_unqualified_provider_without_mutating_owner() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    for (provider, identity) in [
        ("claude", "provider-thread"),
        ("opencode", "ses_fixture"),
        ("pi", "provider-thread"),
        ("antigravity", "conversation-fixture"),
    ] {
        let mut config = configuration(provider, &root.path().join("repo"));
        config.resume_session = Some(identity.into());
        let before = serde_json::to_value(&config).unwrap();
        let error = workspace_resume::prepare_workspace_resume_config(
            &config,
            &WorkspaceMutation::Enable(root.path().join("repo.wt/review")),
        )
        .unwrap_err();
        assert!(
            error.contains("not qualified for provider"),
            "{provider}: {error}"
        );
        assert_eq!(serde_json::to_value(&config).unwrap(), before);
    }
}

#[test]
fn pending_handover_excludes_worktree_deletion_until_guard_release() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("WARDIAN_HOME");
    std::env::set_var("WARDIAN_HOME", root.path());
    let guard = workspace_resume::protect_worktree_assignment().unwrap();
    assert!(
        wardian_core::automation_execution_lock::try_acquire_worktree_mutation_guard()
            .unwrap()
            .is_none()
    );
    drop(guard);
    let deletion = wardian_core::automation_execution_lock::try_acquire_worktree_mutation_guard()
        .unwrap()
        .unwrap();
    assert!(workspace_resume::protect_worktree_assignment().is_err());
    drop(deletion);
    match previous {
        Some(value) => std::env::set_var("WARDIAN_HOME", value),
        None => std::env::remove_var("WARDIAN_HOME"),
    }
}

#[test]
fn off_assignment_retains_exact_requirement_for_later_resume() {
    let root = tempfile::tempdir().unwrap();
    let mut config = configuration("claude", &root.path().join("repo"));
    config.is_off = true;
    WorkspaceMutation::Enable(root.path().join("repo.wt/destination"))
        .apply(&mut config)
        .unwrap();
    assert!(config.is_off);
    let restored: AgentConfig =
        serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
    assert_eq!(restored.exact_resume, config.exact_resume);
    let before = serde_json::to_value(&restored).unwrap();
    // An identity-only assignment cannot bypass the adapter source preflight.
    let failure = ProviderFactory::resolve("claude")
        .unwrap()
        .validate_exact_resume(&restored)
        .unwrap_err();
    assert!(failure.contains("requires a provider UUID"), "{failure}");
    assert_eq!(serde_json::to_value(restored).unwrap(), before);
}

#[test]
fn fresh_restore_clears_the_previous_exact_requirement() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    let mut config = configuration("mock", root.path());
    config.is_off = false;
    config.session_persistence = AgentSessionPersistenceOverride::Fresh;
    config.require_exact_resume();
    prepare_restored_config_for_spawn(&mut config).unwrap();
    assert!(config.exact_resume.is_none());
    assert!(config.resume_session.is_none());
}

#[test]
fn restored_exact_resume_preflight_does_not_publish_identity_substitution() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    let mut config = configuration("mock", root.path());
    config.is_off = false;
    config.session_persistence = AgentSessionPersistenceOverride::Resume;
    config.require_exact_resume();
    config.resume_session = Some("substituted-provider-id".into());
    let before = serde_json::to_value(&config).unwrap();
    let error = prepare_restored_config_for_spawn(&mut config).unwrap_err();
    assert!(error.contains("identity"), "{error}");
    assert_eq!(serde_json::to_value(config).unwrap(), before);
}

#[test]
fn clone_and_new_session_do_not_inherit_exact_source_requirement() {
    let root = tempfile::tempdir().unwrap();
    let mut config = configuration("mock", root.path());
    config.require_exact_resume();
    let cloned = clone_sanitize_config(&config, "clone".into(), None, None, None, false);
    assert!(cloned.exact_resume.is_none());
    assert!(cloned.resume_session.is_none());
    prepare_clear_config(&mut config).unwrap();
    assert!(config.exact_resume.is_none());
}

#[test]
fn unmarked_pi_resume_rejects_sdk_options_before_owner_disposal() {
    let _lock = crate::utils::wardian_test_env_lock();
    let root = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("WARDIAN_HOME");
    std::env::set_var("WARDIAN_HOME", root.path());
    let id = uuid::Uuid::new_v4().to_string();
    for pending_fresh in [false, true] {
        let mut config = configuration("pi", root.path());
        config.resume_session = Some(id.clone());
        config.fresh_provider_session_id = None;
        config.custom_args = Some("'unclosed".into());
        if pending_fresh {
            config.resume_session = None;
            config.fresh_provider_session_id = Some(id.clone());
            config.session_persistence = AgentSessionPersistenceOverride::Fresh;
            let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("materialized.jsonl"), format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"source\"}}\n{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\"}}}}\n"
            )).unwrap();
            assert!(config.pending_pi_session_id().is_some());
        }
        assert!(config.exact_resume.is_none());
        let before = serde_json::to_value(&config).unwrap();
        let error = workspace_resume::prepare_resume_launch_before_stop(&config).unwrap_err();
        assert!(
            error.contains("Pi exact resume custom arguments have invalid quoting"),
            "{error}"
        );
        // This shared read-only boundary runs before stop/holds/reservations and
        // cannot publish the staged configuration or touch a runtime owner.
        assert_eq!(serde_json::to_value(&config).unwrap(), before);
    }
    match previous {
        Some(value) => std::env::set_var("WARDIAN_HOME", value),
        None => std::env::remove_var("WARDIAN_HOME"),
    }
}

#[test]
fn pi_pending_initialization_does_not_require_materialized_sdk_history() {
    let mut config = AgentConfig {
        provider: "pi".into(),
        resume_session: None,
        ..Default::default()
    };
    config.reset_provider_config_for_provider();
    ProviderFactory::resolve("pi")
        .unwrap()
        .validate_resume_launch(&config)
        .unwrap();
}
