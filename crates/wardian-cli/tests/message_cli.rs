//! Parser/auth boundaries for canonical messaging; no application or provider required.
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(home: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .args(args)
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap()
}

#[test]
fn retired_commands_and_delivery_flags_are_not_callable() {
    let home = TempDir::new().unwrap();
    for args in [
        vec!["send", "hello", "--to", "Peer"],
        vec!["ask", "Peer", "task"],
        vec!["reply", "id", "done", "--status", "done"],
    ] {
        assert!(!run(&home, &args).status.success(), "{args:?}");
    }
    for flag in [
        "--queue-policy",
        "--as-command",
        "--approval",
        "--wait-until",
        "--thread",
        "--scope",
        "--deadline",
        "--expires-in",
        "--expected-generation",
        "--invalidate-premise",
        "--targets",
        "--sender",
        "--from",
    ] {
        let output = run(&home, &["message", "send", "Peer", "hello", flag, "value"]);
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("unexpected argument"), "{flag}: {error}");
    }
    assert!(home.path().read_dir().unwrap().next().is_none());
}

#[test]
fn all_six_operations_require_managed_identity_before_storage_or_transport() {
    let home = TempDir::new().unwrap();
    for args in [
        vec!["message", "list"],
        vec!["message", "send", "Peer", "info"],
        vec!["message", "followup", "Peer", "task"],
        vec!["message", "receive"],
        vec![
            "message",
            "reply",
            "canonical-id",
            "done",
            "--status",
            "done",
        ],
        vec!["message", "interrupt", "Peer"],
    ] {
        let output = run(&home, &args);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(home.path().read_dir().unwrap().next().is_none());
}

#[test]
fn receive_bounds_and_exact_recipient_are_enforced_before_transport() {
    let home = TempDir::new().unwrap();
    for args in [
        vec!["message", "receive", "--limit", "0"],
        vec!["message", "receive", "--limit", "101"],
        vec!["message", "receive", "--timeout-ms", "60001"],
        vec!["message", "send", "class:Coder", "info"],
        vec!["message", "followup", "all", "task"],
    ] {
        let output = run(&home, &args);
        assert!(!output.status.success());
        assert_ne!(
            output.status.code(),
            Some(3),
            "invalid input reached authentication: {args:?}"
        );
    }
}
