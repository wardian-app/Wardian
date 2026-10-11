use super::*;
use std::io::Write;

const ID: &str = "019d331a-0500-7592-969f-8f437886f42b";
const OTHER_ID: &str = "019d331a-0500-7592-969f-8f437886f42c";

fn record(id: &str, text: &str) -> Vec<u8> {
    let value = serde_json::json!({"type":"user", "sessionId":id,
        "message":{"role":"user", "content":text}});
    format!("{value}\n").into_bytes()
}
fn seed(root: &Path, project: &str, bytes: &[u8]) -> PathBuf {
    let folder = root.join("projects").join(project);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{ID}.jsonl"));
    std::fs::write(&path, bytes).unwrap();
    path
}
fn config() -> AgentConfig {
    AgentConfig {
        provider: "claude".into(),
        resume_session: Some(ID.into()),
        fresh_provider_session_id: Some("stale-fresh-id".into()),
        ..Default::default()
    }
}

#[test]
fn configured_root_takes_precedence_and_relative_root_uses_child_cwd() {
    let root = tempfile::tempdir().unwrap();
    let default = root.path().join("user");
    let child = root.path().join("projected-child");
    let configured = root.path().join("isolated-claude");
    assert_eq!(
        config_root_with_environment(
            Some(configured.clone().into_os_string()),
            Some(default.clone()),
            &child
        )
        .unwrap(),
        configured
    );
    assert_eq!(
        config_root_with_environment(
            Some(OsString::from("relative-claude")),
            Some(default.clone()),
            &child
        )
        .unwrap(),
        child.join("relative-claude")
    );
    assert_eq!(
        config_root_with_environment(Some(OsString::new()), Some(default.clone()), &child).unwrap(),
        default.join(".claude")
    );
    assert!(config_root_with_environment(None, None, &child).is_err());
    assert!(config_root_with_environment(
        Some(OsString::from("relative-claude")),
        None,
        Path::new("relative-child")
    )
    .is_err());
}

#[test]
fn exact_global_uuid_lookup_is_independent_of_destination_project_name() {
    let root = tempfile::tempdir().unwrap();
    let path = seed(
        root.path(),
        "old-project",
        &record(ID, "retained source context"),
    );
    let before = std::fs::read(&path).unwrap();
    let binding = prepare_in(root.path(), ID, None).unwrap();
    assert_eq!(
        binding.canonical_path,
        std::fs::canonicalize(&path).unwrap()
    );
    assert_eq!(binding.session_id, ID);
    assert_eq!(binding.confirmed_bytes, before.len() as u64);
    assert_eq!(binding.prefix_sha256, hash(&before));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn missing_override_history_never_falls_back_to_default_history() {
    let root = tempfile::tempdir().unwrap();
    let default = root.path().join("default");
    seed(
        &default.join(".claude"),
        "old-project",
        &record(ID, "retained"),
    );
    let configured = root.path().join("isolated");
    std::fs::create_dir_all(&configured).unwrap();
    let selected = config_root_with_environment(
        Some(configured.clone().into_os_string()),
        Some(default),
        root.path(),
    )
    .unwrap();
    assert_eq!(selected, configured);
    assert!(prepare_in(&selected, ID, None)
        .unwrap_err()
        .contains("history directory"));
}

#[test]
fn missing_transcript_does_not_change_owner_or_create_fresh_identity() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("projects")).unwrap();
    let owner = config();
    let before = serde_json::to_value(&owner).unwrap();
    assert!(prepare_in(root.path(), exact_id(&owner).unwrap(), None)
        .unwrap_err()
        .contains("fresh fallback is forbidden"));
    assert_eq!(serde_json::to_value(&owner).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(root.path().join("projects"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn wrong_identity_missing_identity_and_unmaterialized_history_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = seed(root.path(), "source", &record(OTHER_ID, "wrong"));
    assert!(prepare_in(root.path(), ID, None)
        .unwrap_err()
        .contains("session identity"));
    std::fs::write(
        &path,
        b"{\"type\":\"user\",\"message\":{\"content\":\"unbound\"}}\n",
    )
    .unwrap();
    assert!(prepare_in(root.path(), ID, None)
        .unwrap_err()
        .contains("session identity"));
    for bytes in [
        b"".as_slice(),
        b"{\"type\":\"system\",\"subtype\":\"init\"}\n".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(prepare_in(root.path(), ID, None)
            .unwrap_err()
            .contains("no materialized"));
    }
}

#[test]
fn malformed_and_incomplete_records_fail_without_rewriting_the_transcript() {
    let root = tempfile::tempdir().unwrap();
    for tail in [b"{invalid}\n".as_slice(), b"{\"type\":\"user\"}".as_slice()] {
        let mut bytes = record(ID, "retained");
        bytes.extend_from_slice(tail);
        let path = seed(root.path(), "source", &bytes);
        assert!(prepare_in(root.path(), ID, None).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn duplicate_exact_uuid_across_projects_is_ambiguous() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), "a", &record(ID, "retained"));
    seed(root.path(), "b", &record(ID, "retained"));
    assert!(prepare_in(root.path(), ID, None)
        .unwrap_err()
        .contains("ambiguous"));
}

#[test]
fn materialized_append_extends_the_binding_without_changing_confirmed_history() {
    let root = tempfile::tempdir().unwrap();
    let path = seed(root.path(), "source", &record(ID, "retained"));
    let binding = prepare_in(root.path(), ID, None).unwrap();
    File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&record(ID, "closing turn"))
        .unwrap();
    let extended = prepare_in(root.path(), ID, Some(&binding)).unwrap();
    assert_eq!(extended.native_identity, binding.native_identity);
    assert!(extended.confirmed_bytes > binding.confirmed_bytes);
}

#[test]
fn changed_confirmed_prefix_and_truncation_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let path = seed(root.path(), "source", &record(ID, "A"));
    let binding = prepare_in(root.path(), ID, None).unwrap();
    std::fs::write(&path, record(ID, "B")).unwrap();
    assert!(prepare_in(root.path(), ID, Some(&binding))
        .unwrap_err()
        .contains("confirmed history changed"));
    std::fs::write(&path, b"").unwrap();
    assert!(prepare_in(root.path(), ID, Some(&binding))
        .unwrap_err()
        .contains("truncated"));
}

#[test]
fn replaced_file_is_rejected_even_when_its_bytes_are_identical() {
    let root = tempfile::tempdir().unwrap();
    let bytes = record(ID, "retained");
    let path = seed(root.path(), "source", &bytes);
    let binding = prepare_in(root.path(), ID, None).unwrap();
    std::fs::rename(&path, path.with_extension("preserved")).unwrap();
    std::fs::write(&path, bytes).unwrap();
    assert!(prepare_in(root.path(), ID, Some(&binding))
        .unwrap_err()
        .contains("replaced"));
}

#[test]
fn changed_configuration_root_cannot_rebind_to_an_identical_copy() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("a");
    let destination = root.path().join("b");
    seed(&source, "source", &record(ID, "retained"));
    seed(&destination, "source", &record(ID, "retained"));
    let binding = prepare_in(&source, ID, None).unwrap();
    assert!(prepare_in(&destination, ID, Some(&binding))
        .unwrap_err()
        .contains("configuration root changed"));
}

#[test]
fn inspection_bounds_reject_oversize_history_and_records() {
    let root = tempfile::tempdir().unwrap();
    let path = seed(root.path(), "source", &record(ID, "retained"));
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_TRANSCRIPT_BYTES + 1)
        .unwrap();
    assert!(prepare_in(root.path(), ID, None)
        .unwrap_err()
        .contains("inspection safety bound"));
    let mut line = vec![b' '; MAX_RECORD_BYTES + 1];
    line.push(b'\n');
    assert!(validate_records(&line, ID)
        .unwrap_err()
        .contains("record exceeds"));
}

#[test]
fn custom_identity_workspace_and_persistence_selectors_are_rejected() {
    for selector in [
        "--resume other",
        "--resume=other",
        "-rother",
        "--continue",
        "-c",
        "--fork-session",
        "--session-id other",
        "--worktree other",
        "-wother",
        "--no-session-persistence",
        "--resume-session-at message",
        "--rewind-files message",
        "--from-pr 12",
        "--remote-session-id other",
        "--teleport other",
        "--cwd other",
        "--",
        "'unclosed",
    ] {
        let owner = AgentConfig {
            custom_args: Some(selector.into()),
            ..config()
        };
        let before = serde_json::to_value(&owner).unwrap();
        assert!(validate_selectors(&owner).is_err(), "{selector}");
        assert_eq!(serde_json::to_value(&owner).unwrap(), before);
    }
    let owner = AgentConfig {
        custom_args: Some("--model haiku --effort low".into()),
        ..config()
    };
    validate_selectors(&owner).unwrap();
}

#[test]
fn exact_identity_rejects_path_picker_and_blank_selectors() {
    for id in [
        "",
        " ",
        "../outside",
        "C:/outside",
        "provider-picker",
        "019d331a-0500-7592-969f-8f437886f42b ",
    ] {
        let owner = AgentConfig {
            resume_session: Some(id.into()),
            ..config()
        };
        assert!(exact_id(&owner).is_err(), "{id}");
    }
}

#[cfg(windows)]
#[test]
fn unreadable_transcript_fails_before_any_config_mutation() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = seed(root.path(), "source", &record(ID, "retained"));
    let _exclusive = File::options()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let owner = config();
    let before = serde_json::to_value(&owner).unwrap();
    assert!(prepare_in(root.path(), ID, None)
        .unwrap_err()
        .contains("unreadable"));
    assert_eq!(serde_json::to_value(&owner).unwrap(), before);
}

#[test]
fn preparation_captures_inherited_root_and_revalidation_uses_captured_root() {
    let _lock = crate::utils::wardian_test_env_lock();
    struct RestoreRoot(Option<OsString>);
    impl Drop for RestoreRoot {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
                None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
            }
        }
    }
    let _restore = RestoreRoot(std::env::var_os("CLAUDE_CONFIG_DIR"));
    let root = tempfile::tempdir().unwrap();
    let configured = root.path().join("isolated-provider");
    let path = seed(&configured, "source", &record(ID, "retained"));
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &configured) };
    let owner = config();
    let before = serde_json::to_value(&owner).unwrap();
    let prepared = prepare_exact_continuation(&owner, root.path()).unwrap();
    assert_eq!(
        prepared.config_root,
        std::fs::canonicalize(&configured).unwrap()
    );
    File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&record(ID, "closing turn"))
        .unwrap();
    let revalidated =
        revalidate_exact_continuation_at_root(&owner, &prepared.config_root, &prepared).unwrap();
    assert!(revalidated.confirmed_bytes > prepared.confirmed_bytes);
    assert_eq!(serde_json::to_value(&owner).unwrap(), before);
    let changed = AgentConfig {
        resume_session: Some(OTHER_ID.into()),
        ..owner.clone()
    };
    assert!(
        revalidate_exact_continuation_at_root(&changed, &prepared.config_root, &prepared)
            .unwrap_err()
            .contains("identity changed")
    );
    let conflicting = AgentConfig {
        custom_args: Some("--fork-session".into()),
        ..owner
    };
    assert!(prepare_exact_continuation(&conflicting, root.path()).is_err());
}

#[test]
fn malformed_conversation_content_cannot_materialize_or_append_history() {
    let root = tempfile::tempdir().unwrap();
    for content in [
        serde_json::json!([{}]),
        serde_json::json!([{"type":"text"}]),
        serde_json::json!([{"type":"tool_use","id":"tool-1","name":"Bash"}]),
        serde_json::json!([{"type":"tool_result","tool_use_id":"tool-1","content":[{}]}]),
        serde_json::json!(null),
        serde_json::json!({"type":"text","text":"unframed"}),
    ] {
        let malformed = serde_json::json!({"type":"user", "sessionId":ID,
            "message":{"role":"user", "content":content}});
        let bytes = format!("{malformed}\n").into_bytes();
        let path = seed(root.path(), "source", &bytes);
        assert!(prepare_in(root.path(), ID, None)
            .unwrap_err()
            .contains("malformed"));
        std::fs::write(&path, record(ID, "retained")).unwrap();
        let binding = prepare_in(root.path(), ID, None).unwrap();
        File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(prepare_in(root.path(), ID, Some(&binding))
            .unwrap_err()
            .contains("malformed"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn legitimate_text_thinking_tools_and_attachment_blocks_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let assistant = serde_json::json!({"type":"assistant", "sessionId":ID,
    "message":{"role":"assistant", "content":[
        {"type":"thinking","thinking":"","signature":"signed"},
        {"type":"redacted_thinking","data":"opaque-signed-data"},
        {"type":"text","text":"retained"},
        {"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"pwd"}},
        {"type":"server_tool_use","id":"tool-2","name":"web_search","input":{"query":"fixture"}}
    ]}});
    let user = serde_json::json!({"type":"user", "sessionId":ID,
    "message":{"role":"user", "content":[
        {"type":"tool_result","tool_use_id":"tool-1","content":[{"type":"text","text":"source"}]},
        {"type":"tool_result","tool_use_id":"empty-tool"},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"fixture"}},
        {"type":"document","source":{"type":"text","data":"retained document"}},
        {"type":"search_result","source":"https://example.invalid","title":"fixture","content":[{"type":"text","text":"retained"}]}
    ]}});
    let bytes = format!("{assistant}\n{user}\n").into_bytes();
    let path = seed(root.path(), "source", &bytes);
    prepare_in(root.path(), ID, None).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn conversation_message_and_role_are_required_even_after_valid_history() {
    for message in [
        serde_json::json!([]),
        serde_json::json!({"content":"missing role"}),
        serde_json::json!({"role":"assistant","content":"wrong role"}),
        serde_json::json!({"role":"user"}),
    ] {
        let mut bytes = record(ID, "retained");
        let malformed = serde_json::json!({"type":"user", "sessionId":ID, "message":message});
        bytes.extend_from_slice(format!("{malformed}\n").as_bytes());
        assert!(validate_records(&bytes, ID).is_err());
    }
}
#[test]
fn explicit_root_receipt_survives_restore_and_accepts_only_append() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("claude-source");
    let other = root.path().join("claude-other");
    let path = seed(&source, "old-project", &record(ID, "retained"));
    seed(&other, "other-project", &record(ID, "retained"));
    let owner = config();
    let binding = prepare_exact_continuation_at_root(&owner, &source).unwrap();
    let restored: ClaudeTranscriptBinding =
        serde_json::from_value(serde_json::to_value(&binding).unwrap()).unwrap();
    File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&record(ID, "closing append"))
        .unwrap();
    let final_binding = revalidate_exact_continuation_at_root(&owner, &source, &restored).unwrap();
    assert_eq!(final_binding.canonical_path, restored.canonical_path);
    assert_eq!(final_binding.native_identity, restored.native_identity);
    assert!(final_binding.confirmed_bytes > restored.confirmed_bytes);
    assert!(
        revalidate_exact_continuation_at_root(&owner, &other, &restored)
            .unwrap_err()
            .contains("configuration root changed")
    );
    std::fs::write(&path, record(ID, "changed!")).unwrap();
    assert!(revalidate_exact_continuation_at_root(&owner, &source, &final_binding).is_err());
}

#[test]
fn malformed_persisted_receipts_fail_before_prefix_slicing() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), "source", &record(ID, "retained"));
    let owner = config();
    let binding = prepare_exact_continuation_at_root(&owner, root.path()).unwrap();
    let mut malformed = Vec::new();
    for length in [0, MAX_TRANSCRIPT_BYTES + 1, u64::MAX] {
        let mut candidate = binding.clone();
        candidate.confirmed_bytes = length;
        malformed.push(candidate);
    }
    for digest in ["", "no-digest", &"z".repeat(64)] {
        let mut candidate = binding.clone();
        candidate.prefix_sha256 = digest.into();
        malformed.push(candidate);
    }
    let mut candidate = binding.clone();
    candidate.canonical_path = PathBuf::from("relative-transcript");
    malformed.push(candidate);
    let mut candidate = binding.clone();
    candidate.config_root = PathBuf::from("relative-root");
    malformed.push(candidate);
    let mut candidate = binding.clone();
    candidate.session_id = "../another-session".into();
    malformed.push(candidate);
    for candidate in malformed {
        assert!(
            revalidate_exact_continuation_at_root(&owner, root.path(), &candidate)
                .unwrap_err()
                .contains("receipt is malformed")
        );
    }
}

#[test]
fn explicit_root_rejects_relative_root_instead_of_reading_global_environment() {
    let owner = config();
    assert!(
        prepare_exact_continuation_at_root(&owner, Path::new("relative-root"))
            .unwrap_err()
            .contains("absolute effective configuration root")
    );
}

#[test]
fn workspace_handover_refuses_relative_override_before_projection_rotation() {
    assert!(
        validate_workspace_handover_root_with_environment(Some(OsString::from("relative-claude")))
            .unwrap_err()
            .contains("could rebind its source history")
    );
    assert!(validate_workspace_handover_root_with_environment(None).is_ok());
    assert!(validate_workspace_handover_root_with_environment(Some(OsString::new())).is_ok());
    let root = tempfile::tempdir().unwrap();
    assert!(validate_workspace_handover_root_with_environment(Some(
        root.path().as_os_str().into()
    ))
    .is_ok());
}

#[test]
fn explicit_root_validation_does_not_accept_substituted_durable_identity() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), "source", &record(ID, "retained"));
    let mut owner = config();
    owner.require_exact_resume();
    owner.resume_session = Some(OTHER_ID.into());
    assert!(prepare_exact_continuation_at_root(&owner, root.path()).is_err());
    let mut owner = config();
    owner.provider = "codex".into();
    assert!(prepare_exact_continuation_at_root(&owner, root.path())
        .unwrap_err()
        .contains("another provider identity"));
}
