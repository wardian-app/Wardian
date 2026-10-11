//! Production preparation handler exercised with an isolated home and Git fixtures.
use super::{test_support::TestWardianHome, worktree::handle_agent_worktree_prepare};
use crate::commands::git::run_git;
use std::fs;
use wardian_core::control::{AgentWorktreePrepareResponse, ControlRequest};

#[tokio::test]
async fn preparation_handler_keeps_agent_records_history_and_leases_unchanged() {
    let home = TestWardianHome::new_async().await;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let cwd = source.to_str().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec!["config", "user.name", "Wardian Test"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "core.autocrlf", "false"],
    ] {
        run_git(cwd, &args).unwrap();
    }
    fs::write(source.join("README.md"), "fixture\n").unwrap();
    run_git(cwd, &["add", "README.md"]).unwrap();
    run_git(cwd, &["commit", "-m", "fixture"]).unwrap();
    let commit = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
    // Representative durable records of an unrelated existing provider. The
    // handler has no runtime/state argument and must not inspect or rewrite them.
    let records = [
        ("settings/state.json", br#"{"agents":[{"session_id":"owner","resume_session":"existing-thread","folder":"existing-workspace"}]}"#.as_slice()),
        ("agents/owner/conversations/thread/conversation.jsonl", b"retained transcript\n".as_slice()),
        ("agents/owner/provider-lease.json", b"exact-existing-owner\n".as_slice()),
    ];
    for (relative, bytes) in records {
        let path = home.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let request = ControlRequest::AgentWorktreePrepare {
        source: cwd.into(),
        name: "review".into(),
        reference: commit.trim().into(),
    };
    let ControlRequest::AgentWorktreePrepare {
        source,
        name,
        reference,
    } = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap()
    else {
        panic!("wrong preparation request")
    };
    let rendered = handle_agent_worktree_prepare(source, name, reference)
        .await
        .unwrap();
    let response: AgentWorktreePrepareResponse = serde_json::from_str(&rendered).unwrap();
    assert!(response.ok);
    assert!(!response.cleared_session);
    assert_eq!(response.commit, commit.trim());
    assert!(std::path::Path::new(&response.worktree_folder)
        .join(".git")
        .is_file());
    for (relative, bytes) in records {
        assert_eq!(fs::read(home.path().join(relative)).unwrap(), bytes);
    }
    let error = handle_agent_worktree_prepare(cwd.into(), "review".into(), "HEAD".into())
        .await
        .unwrap_err();
    assert!(error.message.contains("occupied"));
    for (relative, bytes) in records {
        assert_eq!(fs::read(home.path().join(relative)).unwrap(), bytes);
    }
}
