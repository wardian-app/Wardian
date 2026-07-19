use std::process::Command;

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
        .expect("current exe")
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target dir")
        .join(exe)
}

#[test]
fn present_requires_managed_session_identity() {
    let home = tempfile::tempdir().expect("home");
    let file = home.path().join("report.md");
    std::fs::write(&file, "report").expect("file");
    let output = Command::new(bin())
        .args(["artifact", "present", file.to_string_lossy().as_ref()])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .expect("run");

    assert_eq!(output.status.code(), Some(3));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).expect("error json");
    assert_eq!(error["error"]["code"], "invalid_origin");
}

#[test]
fn present_does_not_fall_back_to_disk_when_app_is_absent() {
    let home = tempfile::tempdir().expect("home");
    let file = home.path().join("report.md");
    std::fs::write(&file, "report").expect("file");
    let output = Command::new(bin())
        .args(["artifact", "present", file.to_string_lossy().as_ref()])
        .env("WARDIAN_HOME", home.path())
        .env("WARDIAN_SESSION_ID", "session-1")
        .output()
        .expect("run");

    assert_eq!(output.status.code(), Some(6));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).expect("error json");
    assert_eq!(error["error"]["code"], "app_not_running");
    assert!(!home.path().join("artifacts").exists());
}
