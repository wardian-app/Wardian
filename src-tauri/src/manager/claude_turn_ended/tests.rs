use super::*;

fn fixture() -> (Launch, Receipt) {
    let launch = Launch {
        schema: 1,
        wardian_session_id: "owned-agent".into(),
        launch_id: "owned-launch".into(),
        accepted_sessions: vec!["claude-session".into()],
    };
    let receipt = Receipt {
        schema: 1,
        wardian_session_id: launch.wardian_session_id.clone(),
        launch_id: launch.launch_id.clone(),
        provider_session_id: "claude-session".into(),
        turn_id: "turn-A".into(),
        agent_id: None,
        prompt_origin: "composer".into(),
        prompt_text: "Explain the change".into(),
        start_text: "Explain the change".into(),
        answer: "Exact provider text.\n\n".into(),
        is_aborted: false,
        reason: Some("answer".into()),
        started_at_ms: 1000,
        captured_at_ms: 2000,
    };
    (launch, receipt)
}

#[test]
fn native_answer_reason_never_claims_success_and_keeps_exact_text() {
    let (launch, receipt) = fixture();
    assert!(receipt.valid_for(&launch));
    let item = receipt.item();
    assert_eq!(item["type"], "agent_turn_ended");
    assert_eq!(item["turn_outcome"], "unknown");
    assert_eq!(item["provider_reason"], "answer");
    assert_eq!(item["response_text"], "Exact provider text.\n\n");
    assert!(item.get("status").is_none());
    assert_eq!(item, receipt.item());
}

#[test]
fn interruption_with_no_answer_is_a_notice_without_stale_text() {
    let (_, mut receipt) = fixture();
    receipt.answer.clear();
    receipt.is_aborted = true;
    let item = receipt.item();
    assert_eq!(item["turn_outcome"], "aborted");
    assert_eq!(item["response_text"], "");
    assert_eq!(item["summary"], "Turn interrupted");
}

#[test]
fn missing_foreign_subagent_automatic_and_ambiguous_evidence_is_rejected() {
    let (launch, receipt) = fixture();
    type Mutation = (&'static str, Box<dyn Fn(&mut Receipt)>);
    let mut mutations: Vec<Mutation> = vec![
        (
            "foreign launch",
            Box::new(|r| r.launch_id = "old-launch".into()),
        ),
        (
            "foreign owner",
            Box::new(|r| r.wardian_session_id = "other-agent".into()),
        ),
        (
            "foreign provider",
            Box::new(|r| r.provider_session_id = "other-session".into()),
        ),
        (
            "subagent",
            Box::new(|r| r.agent_id = Some("subagent".into())),
        ),
        (
            "automatic",
            Box::new(|r| r.prompt_origin = "automatic".into()),
        ),
        (
            "wrong prompt",
            Box::new(|r| r.start_text = "next prompt".into()),
        ),
        (
            "no prompt",
            Box::new(|r| {
                r.prompt_text.clear();
                r.start_text.clear();
            }),
        ),
        ("invalid clock", Box::new(|r| r.captured_at_ms = 999)),
        ("no start", Box::new(|r| r.started_at_ms = 0)),
        (
            "path traversal",
            Box::new(|r| r.turn_id = "../turn-A".into()),
        ),
        ("unsupported schema", Box::new(|r| r.schema = 2)),
    ];
    for (name, mutate) in mutations.drain(..) {
        let mut invalid = receipt.clone();
        mutate(&mut invalid);
        assert!(!invalid.valid_for(&launch), "{name}");
    }
}

#[test]
fn launch_provider_and_turn_all_participate_in_dedup_identity() {
    let (_, receipt) = fixture();
    for field in ["launch_id", "provider_session_id", "turn_id"] {
        let mut json = serde_json::to_value(&receipt).unwrap();
        json[field] = "another-identity".into();
        let other: Receipt = serde_json::from_value(json).unwrap();
        assert_ne!(receipt.item()["id"], other.item()["id"]);
    }
}

#[test]
fn raw_or_torn_producer_files_cannot_replay_as_admitted_evidence() {
    let (launch, receipt) = fixture();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("receipts")).unwrap();
    write_json(&temp.path().join("receipts/notice-turn-A.json"), &receipt).unwrap();
    assert_eq!(receipts(temp.path(), &launch, "notice-").len(), 1);
    assert!(receipts(temp.path(), &launch, "admitted-").is_empty());
    let admitted = temp.path().join("admitted-turn-A.json");
    std::fs::write(&admitted, "{\"schema\":1,").unwrap();
    assert!(receipts(temp.path(), &launch, "admitted-").is_empty());
    write_json(&admitted, &receipt).unwrap();
    assert_eq!(receipts(temp.path(), &launch, "admitted-").len(), 1);
    std::fs::rename(admitted, temp.path().join("admitted-wrong-turn.json")).unwrap();
    assert!(receipts(temp.path(), &launch, "admitted-").is_empty());
}

#[test]
fn dismissal_tombstone_stays_suppressed_for_ended_notice() {
    let (_, receipt) = fixture();
    let mut item = receipt.item();
    let id = item["id"].as_str().unwrap().to_string();
    assert!(turn_completion::active_completion(&[item.clone()], &id).is_some());
    item["dismissed"] = true.into();
    assert!(turn_completion::active_completion(&[item], &id).is_none());
}

#[test]
fn generated_plugin_preserves_launch_binding_and_ownership() {
    let home = crate::control::test_support::TestWardianHome::new();
    let sentinel = home.path().join("unrelated.txt");
    std::fs::write(&sentinel, "untouched").unwrap();
    let config = AgentConfig {
        session_id: "qa-native-mod-probe".into(),
        provider: "claude".into(),
        resume_session: Some("provider-resume".into()),
        ..Default::default()
    };
    let launch = TurnEndedLaunch::prepare(&config).unwrap();
    let owner = home
        .path()
        .join("agents")
        .join(&config.session_id)
        .join("claude/turn-ended");
    assert_eq!(launch.dir.parent(), Some(owner.as_path()));
    assert!(safe_id(&launch.launch.launch_id));
    let persisted: Launch = read_json(&launch.dir.join("launch.json")).unwrap();
    assert_eq!(persisted.schema, 1);
    assert_eq!(persisted.wardian_session_id, config.session_id);
    assert_eq!(persisted.launch_id, launch.launch.launch_id);
    assert_eq!(
        persisted.accepted_sessions,
        vec![config.session_id.clone(), "provider-resume".into()]
    );
    let plugin = launch.plugin_dir();
    let manifest: serde_json::Value =
        read_json(&plugin.join(".claude-plugin/plugin.json")).unwrap();
    assert_eq!(manifest["name"], "wardian-turn-ended");
    assert_eq!(manifest["version"], "1.0.0");
    assert_eq!(manifest["author"]["name"], "Wardian");
    let hooks: serde_json::Value = read_json(&plugin.join("hooks/hooks.json")).unwrap();
    assert_eq!(hooks["modules"], serde_json::json!(["./register.js"]));
    let module_path = plugin.join("hooks/register.js");
    let module = std::fs::read_to_string(&module_path).unwrap();
    let (binding_line, body) = module.split_once('\n').unwrap();
    let binding: serde_json::Value = serde_json::from_str(
        binding_line
            .strip_prefix("const wardianBridge = ")
            .unwrap()
            .strip_suffix(';')
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body, MOD_SOURCE);
    assert_eq!(binding["wardian_session_id"], config.session_id);
    assert_eq!(binding["launch_id"], launch.launch.launch_id);
    assert_eq!(
        binding["directory"],
        launch.dir.join("receipts").to_string_lossy().as_ref()
    );
    let coverage: serde_json::Value = read_json(&launch.dir.join("coverage.json")).unwrap();
    assert_eq!(coverage["available"], false);
    assert_eq!(coverage["reason"], "awaiting_provider_mod_handshake");
    assert_eq!(
        std::fs::read_dir(launch.dir.join("receipts"))
            .unwrap()
            .count(),
        0
    );

    // A second preparation must not overwrite the first launch's binding.
    let next = TurnEndedLaunch::prepare(&config).unwrap();
    assert_ne!(next.dir, launch.dir);
    assert_eq!(next.dir.parent(), Some(owner.as_path()));
    assert_eq!(std::fs::read_to_string(&module_path).unwrap(), module);
    let foreign = AgentConfig {
        session_id: "other-agent".into(),
        ..config.clone()
    };
    let other = TurnEndedLaunch::prepare(&foreign).unwrap();
    assert!(!other.dir.starts_with(&owner));
    assert!(other
        .dir
        .starts_with(home.path().join("agents/other-agent")));
    let invalid = AgentConfig {
        session_id: "../outside-owner".into(),
        ..config
    };
    assert!(TurnEndedLaunch::prepare(&invalid).is_err());
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "untouched");

    // The pure assertions above always execute. This extension is a separate
    // installed-provider claim, never inferred from the unit test's PASS.
    if let Some(validator) = std::env::var_os("WARDIAN_CLAUDE_PLUGIN_VALIDATOR") {
        let validator = PathBuf::from(validator);
        assert!(validator.is_absolute() && validator.is_file());
        validate_generated_plugin_with_installed_provider(&validator, &plugin, home.path());
        eprintln!("WARDIAN_CLAUDE_PLUGIN_VALIDATION={{\"status\":\"PASS\",\"claim\":\"installed_provider_static_validation\"}}");
    } else {
        eprintln!("WARDIAN_CLAUDE_PLUGIN_VALIDATION={{\"status\":\"NOT_REQUESTED\",\"claim\":\"installed_provider_static_validation\"}}");
    }
}

fn validate_generated_plugin_with_installed_provider(
    validator: &Path,
    plugin: &Path,
    test_home: &Path,
) {
    let output = std::process::Command::new(validator)
        .args(["plugin", "validate", "--json", "--strict"])
        .arg(plugin)
        .env(
            "CLAUDE_CONFIG_DIR",
            test_home.join("isolated-claude-config"),
        )
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .current_dir(test_home)
        .output()
        .expect("run the installed provider's nonpaid static validator");
    assert!(
        output.status.success(),
        "provider rejected the actual generated plugin: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["success"], true);
}
