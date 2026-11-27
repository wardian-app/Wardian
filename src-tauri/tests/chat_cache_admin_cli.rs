use std::process::Command;

#[test]
fn maintenance_command_requires_every_scope_and_rejects_standalone_writes() {
    let home = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_wardian-chat-cache-admin");
    let output = Command::new(binary)
        .args([
            "verify-archive",
            "--agent-id",
            "synthetic-agent",
            "--dry-run",
        ])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--home is required"));
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);

    let output = Command::new(binary)
        .args([
            "verify-archive",
            "--home",
            home.path().to_str().unwrap(),
            "--agent-id",
            "synthetic-agent",
            "--dry-run",
            "--apply",
        ])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(64));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("legacy Chat maintenance is read-only")
    );
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn archive_verification_refuses_the_configured_home_and_reports_an_incomplete_copy() {
    let live = tempfile::tempdir().unwrap();
    let copy = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_wardian-chat-cache-admin");
    let run = |home: &std::path::Path| {
        Command::new(binary)
            .arg("verify-archive")
            .arg("--home")
            .arg(home)
            .args(["--agent-id", "synthetic-agent", "--dry-run"])
            .env("WARDIAN_HOME", live.path())
            .output()
            .unwrap()
    };
    let refused = run(live.path());
    assert_eq!(refused.status.code(), Some(64));
    assert!(refused.stdout.is_empty());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("separate absolute offline copy"));

    let unavailable = run(copy.path());
    assert_eq!(unavailable.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&unavailable.stdout).unwrap();
    assert_eq!(report["scan_complete"], false);
    assert_eq!(report["deletion_blocked"], true);
    assert_eq!(report["incomplete_scans"], 1);
    assert_eq!(std::fs::read_dir(copy.path()).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(live.path()).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn archive_verification_refuses_physical_overlap_with_home_junctions() {
    let fixture = tempfile::tempdir().unwrap();
    let parent = fixture.path().join("physical");
    let physical = parent.join("wardian");
    let child = physical.join("descendant");
    let profile = fixture.path().join("synthetic-profile");
    let configured = profile.join(".wardian");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir(&profile).unwrap();
    junction::create(&physical, &configured).unwrap();

    let run = |home: &std::path::Path, override_home: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-chat-cache-admin"));
        command
            .arg("verify-archive")
            .arg("--home")
            .arg(home)
            .args(["--agent-id", "synthetic-agent", "--dry-run"])
            .env("USERPROFILE", &profile)
            .env("HOME", &profile);
        if override_home {
            command.env("WARDIAN_HOME", &configured);
        } else {
            command.env_remove("WARDIAN_HOME");
        }
        command.output().unwrap()
    };
    let attempted = [
        ("configured home", run(&physical, true)),
        ("configured descendant", run(&child, true)),
        ("default ancestor", run(&parent, false)),
    ];
    junction::delete(&configured).unwrap();

    for (case, output) in attempted {
        assert_eq!(output.status.code(), Some(64), "{case}");
        assert!(output.stdout.is_empty(), "{case}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("separate absolute offline copy"),
            "{case}"
        );
    }
    assert_eq!(std::fs::read_dir(&child).unwrap().count(), 0);
}
