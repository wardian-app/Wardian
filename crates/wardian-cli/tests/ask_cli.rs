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
fn ask_without_app_exits_six() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["ask", "reviewer-a1", "review this"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"app_not_running""#));
}

#[test]
fn ask_rejects_class_selector_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["ask", "class:Reviewer", "review this"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("single agent name or uuid"));
}

#[test]
fn ask_rejects_all_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["ask", "all", "review this"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("single agent name or uuid"));
}

#[test]
fn ask_rejects_class_selector_with_thread_before_app_lookup() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args([
            "ask",
            "class:Reviewer",
            "review this",
            "--thread",
            "plan-review",
        ])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("single agent name or uuid"));
}

#[test]
fn ask_rejects_thread_before_app_lookup_for_single_target() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args([
            "ask",
            "reviewer-a1",
            "review this",
            "--thread",
            "plan-review",
        ])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_supported""#));
    assert!(stderr.contains("--thread is not supported"));
}

#[test]
fn ask_no_message_no_stdin_exits_one() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["ask", "reviewer-a1"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Provide a message, --stdin, or --file"));
}
