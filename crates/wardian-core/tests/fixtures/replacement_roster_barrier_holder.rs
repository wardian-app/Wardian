use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

fn required_path(name: &str) -> PathBuf {
    let value = std::env::var_os(name)
        .unwrap_or_else(|| panic!("{name} must explicitly name the isolated native test home"));
    let path = PathBuf::from(value);
    assert!(path.is_absolute(), "{name} must be absolute");
    path.canonicalize()
        .unwrap_or_else(|error| panic!("{name} must resolve to an existing isolated home: {error}"))
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn hold_roster_barrier() {
    let home = required_path("WARDIAN_NATIVE_ROSTER_BARRIER_HOME");
    let wardian_home = required_path("WARDIAN_HOME");
    let native_home = required_path("WARDIAN_E2E_NATIVE_HOME");
    assert!(
        same_path(&home, &wardian_home),
        "WARDIAN_HOME differs from the explicit isolated-home intent"
    );
    assert!(
        same_path(&home, &native_home),
        "WARDIAN_E2E_NATIVE_HOME differs from the explicit isolated-home intent"
    );
    assert!(
        home.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("wardian-e2e-native-")),
        "the explicit home must use the native harness isolated-home prefix"
    );

    let run_id = std::env::var("WARDIAN_E2E_RUN_ID")
        .expect("WARDIAN_E2E_RUN_ID must bind the helper to the native harness run");
    assert!(
        !run_id.trim().is_empty(),
        "WARDIAN_E2E_RUN_ID must not be empty"
    );
    let owner_path = home.join(".native-e2e-lock").join("owner.json");
    let owner: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&owner_path).unwrap_or_else(|error| {
            panic!("native harness home owner record is required: {error}")
        }))
        .expect("native harness home owner record must be valid JSON");
    assert_eq!(
        owner.get("runId").and_then(serde_json::Value::as_str),
        Some(run_id.as_str()),
        "the home claim must belong to this native harness run"
    );
    assert!(
        owner
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|pid| pid > 0),
        "the home owner record must contain a positive process id"
    );

    let nonce = std::env::var("WARDIAN_NATIVE_ROSTER_BARRIER_NONCE")
        .expect("WARDIAN_NATIVE_ROSTER_BARRIER_NONCE is required");
    assert!(
        !nonce.is_empty()
            && nonce.len() <= 128
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "the barrier protocol nonce must be short ASCII"
    );

    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .expect("acquire native-test roster barrier")
        .expect("blocking roster-barrier acquisition returns a guard");
    println!("READY:{nonce}");
    io::stdout().flush().expect("flush holder readiness");
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut line = String::new();
    let bytes_read = stdin.read_line(&mut line).expect("read release command");
    if bytes_read != 0 {
        let release = line.strip_suffix('\n').unwrap_or(&line);
        let release = release.strip_suffix('\r').unwrap_or(release);
        assert_eq!(
            release,
            format!("RELEASE:{nonce}"),
            "the holder only accepts its matching explicit release or EOF"
        );
    }
    drop(barrier);
    println!("RELEASED:{nonce}");
    io::stdout().flush().expect("flush holder release");
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else {
        println!("roster barrier holder utility not activated");
        return;
    };
    if first != "--hold" {
        println!("roster barrier holder utility not activated");
        return;
    }
    assert!(
        args.next().is_none(),
        "--hold does not accept additional arguments"
    );
    hold_roster_barrier();
}
