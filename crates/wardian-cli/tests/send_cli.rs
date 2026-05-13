use std::process::Command;
use tempfile::TempDir;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_wardian-cli") {
        return path.into();
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_wardian_cli") {
        return path.into();
    }
    let exe = if cfg!(windows) {
        "wardian-cli.exe"
    } else {
        "wardian-cli"
    };
    std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap()
        .join(exe)
}

#[test]
fn send_requires_to_flag() {
    let output = Command::new(bin())
        .args(["send", "hello world"])
        .output()
        .unwrap();
    // clap exits 1 when required flag is missing
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"generic""#));
}

#[test]
fn send_without_app_exits_six() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["send", "hello", "--to", "coder-a1"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"app_not_running""#));
}

#[test]
fn send_to_class_prefix_without_app_exits_six() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["send", "review this", "--to", "class:Coder"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"app_not_running""#));
}

#[test]
fn send_to_all_without_app_exits_six() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["send", "broadcast", "--to", "all"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
}

#[test]
fn send_no_message_no_stdin_exits_one() {
    let home = TempDir::new().unwrap();
    // --to is given but no message, no --stdin, no --file
    let output = Command::new(bin())
        .args(["send", "--to", "coder-a1"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    // Should fail: no message provided (either generic error or app_not_running,
    // since control pipe check happens before message validation)
    assert_ne!(output.status.code(), Some(0));
}

#[test]
fn send_wait_until_rejects_class_selector_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args([
            "send",
            "hello",
            "--to",
            "class:Coder",
            "--wait-until",
            "idle",
        ])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
}

#[test]
fn send_as_command_to_all_rejects_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["send", "--to", "all", "--as-command", "/goal test"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("--as-command requires a single agent name or uuid"));
}

#[test]
fn send_as_command_to_class_rejects_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["send", "--to", "class:Coder", "--as-command", "/status"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("--as-command requires a single agent name or uuid"));
}

#[test]
fn send_as_command_rejects_thread_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args([
            "send",
            "--to",
            "coder-a1",
            "--thread",
            "review",
            "--as-command",
            "/goal test",
        ])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("--as-command cannot be combined with --thread"));
}
