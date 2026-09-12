use super::*;
use crate::manager::headless::{headless_provider_args, HeadlessProcessTreeGuard};
use crate::providers::ProviderFactory;
use std::process::Stdio;
use tokio::io::AsyncReadExt;

const CAPTURE: &str = r#"
    const chunks = [];
    process.stdin.on('data', chunk => chunks.push(chunk));
    process.stdin.on('end', () => process.stdout.write(JSON.stringify({
        argv: process.argv.slice(1), input: [...Buffer.concat(chunks)]
    })));
"#;

fn node_command(script: &str) -> tokio::process::Command {
    let mut command = crate::utils::process::new_headless_command(if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    });
    command.arg("-e").arg(script).arg("--");
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

#[tokio::test]
async fn real_child_receives_exact_prompt_on_stdin_without_positional_message() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let prompt = "  quotes \"double\" 'single', slash \\path\\file\r\nnext\n\t\u{1b}\u{7} Unicode 日本語 🐙 $x; | & < > ` end  \r\n";
    let provider = ProviderFactory::resolve("opencode").unwrap();
    let args = headless_provider_args(
        "opencode",
        provider.as_ref(),
        workspace.path(),
        prompt,
        "json",
        Some("ses_fixture"),
        None,
    );
    let mut command = node_command(CAPTURE);
    command.args(&args);
    configure(&mut command);
    let mut child = command
        .spawn()
        .expect("spawn inert Node argv/stdin fixture");
    let mut guard = HeadlessProcessTreeGuard::new(child.id());
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let output = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let status = wait(
        &mut child,
        prompt,
        Duration::from_secs(10),
        None,
        &mut guard,
    )
    .await
    .unwrap();
    guard.disarm();
    assert!(status.success());
    let captured: serde_json::Value = serde_json::from_slice(&output.await.unwrap()).unwrap();
    assert_eq!(
        captured["input"],
        serde_json::json!(prompt.as_bytes()),
        "complete prompt bytes must reach stdin"
    );
    assert_eq!(
        captured["argv"],
        serde_json::json!([
            "run",
            "--session",
            "ses_fixture",
            "--format",
            "json",
            "--dir",
            workspace.path().to_str().unwrap()
        ])
    );
    assert!(
        errors.await.unwrap().is_empty(),
        "prompt must not be sent to stderr"
    );
}

async fn collect(
    mut command: tokio::process::Command,
    prompt: &str,
) -> (Result<std::process::ExitStatus, String>, Vec<u8>, Vec<u8>) {
    configure(&mut command);
    let mut child = command.spawn().expect("spawn inert child");
    let mut guard = HeadlessProcessTreeGuard::new(child.id());
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let output = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let status = wait(
        &mut child,
        prompt,
        Duration::from_secs(10),
        None,
        &mut guard,
    )
    .await;
    if status.is_ok() {
        guard.disarm();
    }
    (status, output.await.unwrap(), errors.await.unwrap())
}

#[tokio::test]
async fn real_child_receives_large_prompt_including_nul_and_eof() {
    let prompt = "\0\u{1}\u{1b}\r\n\t\\\"日本語🦀  ".repeat(32_768);
    let (status, output, errors) = collect(node_command(CAPTURE), &prompt).await;
    assert!(status.unwrap().success());
    let captured: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(captured["input"], serde_json::json!(prompt.as_bytes()));
    assert_eq!(captured["argv"], serde_json::json!([]));
    assert!(errors.is_empty());
}

#[tokio::test]
async fn failed_provider_does_not_expose_echoed_prompt_or_stderr() {
    let prompt = "PRIVATE_PROMPT_1205\r\nwith \\\"quoted\\\" Unicode 🦀";
    let command = node_command(
        r#"
        const chunks = [];
        process.stdin.on('data', chunk => chunks.push(chunk));
        process.stdin.on('end', () => {
            process.stderr.write(Buffer.concat(chunks));
            process.stderr.write('PRIVATE_STDERR_1205');
            process.exitCode = 23;
        });
    "#,
    );
    let (status, _, stderr) = collect(command, prompt).await;
    let error = status.unwrap_err();
    assert!(error.contains("23"));
    assert!(!error.contains("PRIVATE_"));
    assert!(
        stderr.starts_with(prompt.as_bytes()),
        "fixture actually echoed private input"
    );
}

async fn ready_wrapper(
    close_input: bool,
) -> (tokio::process::Child, HeadlessProcessTreeGuard, u32) {
    use tokio::io::AsyncBufReadExt;
    let mut command = node_command(
        r#"
        const fs = require('node:fs');
        const {spawn} = require('node:child_process');
        const descendant = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {stdio: 'ignore'});
        if (process.argv[1] === 'close') fs.closeSync(0);
        process.stdout.write(JSON.stringify({pid: descendant.pid}) + '\n');
        setInterval(() => {}, 1000);
    "#,
    );
    command.arg(if close_input { "close" } else { "block" });
    if close_input {
        command = crate::utils::process::new_headless_command(
            std::env::current_exe().unwrap().to_str().unwrap(),
        );
        command.args([
            "--exact",
            "manager::headless::opencode_stdin::tests::closed_input_fixture",
            "--nocapture",
        ]);
        command.env("WARDIAN_CLOSED_INPUT_FIXTURE", "1");
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
    }
    configure(&mut command);
    let mut child = command.spawn().unwrap();
    let guard = HeadlessProcessTreeGuard::new(child.id());
    let mut ready = String::new();
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            ready.clear();
            assert_ne!(
                stdout.read_line(&mut ready).await.unwrap(),
                0,
                "fixture exited before readiness"
            );
            if ready.starts_with("{\"pid\":") {
                break;
            }
        }
    })
    .await
    .unwrap();
    let pid = serde_json::from_str::<serde_json::Value>(&ready).unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    (child, guard, pid)
}

async fn assert_stopped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_running(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned fixture process must terminate");
}

fn process_running(pid: u32) -> bool {
    #[cfg(windows)]
    return crate::utils::process::process_exists(pid);
    #[cfg(unix)]
    {
        let pid = sysinfo::Pid::from_u32(pid);
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).is_some_and(|process| {
            // Orphan zombies have exited but await their new parent's reap.
            !matches!(
                process.status(),
                sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
            )
        })
    }
}

#[tokio::test]
async fn failed_write_terminates_wrapper_and_descendant_without_retry() {
    let (mut child, mut guard, descendant) = ready_wrapper(true).await;
    let root = child.id().unwrap();
    let prompt = "PRIVATE_FAILED_WRITE".repeat(1_000_000);
    let error = wait(
        &mut child,
        &prompt,
        Duration::from_secs(5),
        None,
        &mut guard,
    )
    .await
    .unwrap_err();
    assert!(error.contains("prompt input failed"), "{error}");
    assert!(error.contains("not retried"));
    assert!(!error.contains("PRIVATE_"));
    assert_stopped(root).await;
    assert_stopped(descendant).await;
}

#[tokio::test]
async fn small_prompt_observes_the_final_buffered_write_failure() {
    let (mut child, mut guard, descendant) = ready_wrapper(true).await;
    let root = child.id().unwrap();
    let error = wait(
        &mut child,
        "PRIVATE_SMALL_WRITE",
        Duration::from_secs(5),
        None,
        &mut guard,
    )
    .await
    .unwrap_err();
    assert!(error.contains("prompt input failed"), "{error}");
    assert!(error.contains("not retried"));
    assert!(!error.contains("PRIVATE_"));
    assert_stopped(root).await;
    assert_stopped(descendant).await;
}

#[tokio::test]
async fn blocked_write_obeys_execution_timeout_and_terminates_descendant() {
    let (mut child, mut guard, descendant) = ready_wrapper(false).await;
    let root = child.id().unwrap();
    let prompt = "PRIVATE_BLOCKED_WRITE".repeat(1_000_000);
    let error = wait(
        &mut child,
        &prompt,
        Duration::from_millis(100),
        None,
        &mut guard,
    )
    .await
    .unwrap_err();
    assert!(error.contains("exceeded"), "{error}");
    assert!(!error.contains("PRIVATE_"));
    assert_stopped(root).await;
    assert_stopped(descendant).await;
}

#[tokio::test]
async fn cancelling_blocked_input_terminates_owned_process_tree() {
    let (mut child, mut guard, descendant) = ready_wrapper(false).await;
    let root = child.id().unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let prompt = "blocked".repeat(1_000_000);
        let mut operation = std::pin::pin!(wait(
            &mut child,
            &prompt,
            Duration::from_secs(30),
            None,
            &mut guard
        ));
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        assert!(poll_fn(|cx| Poll::Ready(operation.as_mut().poll(cx)))
            .await
            .is_pending());
        started.send(()).unwrap();
        operation.await
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_stopped(root).await;
    assert_stopped(descendant).await;
}

#[test]
fn other_providers_keep_their_positional_prompt_contract() {
    for name in ["claude", "codex", "antigravity", "pi", "gemini", "mock"] {
        let provider = ProviderFactory::resolve(name).unwrap();
        let prompt = "unchanged 'quoted' \\ prompt";
        let args = headless_provider_args(
            name,
            provider.as_ref(),
            std::path::Path::new("."),
            prompt,
            "json",
            None,
            None,
        );
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == prompt).count(),
            1,
            "{name}"
        );
    }
}

struct IsolatedHome {
    previous: Option<std::ffi::OsString>,
    directory: tempfile::TempDir,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl IsolatedHome {
    async fn new() -> Self {
        let lock = crate::utils::wardian_test_env_lock_async().await;
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", directory.path());
        Self {
            previous,
            directory,
            _lock: lock,
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

#[cfg(windows)]
#[tokio::test]
async fn configured_powershell_shim_preserves_stdin_and_provider_flags() {
    use wardian_core::models::{AgentConfig, OpenCodeProviderConfig, ProviderConfig};
    let isolated = IsolatedHome::new().await;
    let root = isolated.directory.path();
    std::fs::create_dir_all(root.join("settings")).unwrap();
    std::fs::write(
        root.join("settings/shell.json"),
        r#"{
        "shell_id":"custom", "custom_executable":"pwsh.exe",
        "custom_args":"-NoProfile -Command", "agent_session_persistence":"resume"
    }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("capture.cjs"),
        CAPTURE.replace("slice(1)", "slice(2)"),
    )
    .unwrap();
    let shim = root.join("provider shim.ps1");
    std::fs::write(
        &shim,
        "& node.exe \"$PSScriptRoot/capture.cjs\" @args\nexit $LASTEXITCODE\n",
    )
    .unwrap();
    let config = AgentConfig {
        provider: "opencode".into(),
        model: Some("fixture/model".into()),
        debug: Some(true),
        provider_config: ProviderConfig::OpenCode(OpenCodeProviderConfig {
            agent: Some("fixture-agent".into()),
            auto: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = ProviderFactory::resolve("opencode").unwrap();
    let prompt = "  \"quotes\" \\slashes\r\n\t\u{1b}\0Unicode 日本語 🐙 $env:PRIVATE ; | & `  ";
    let args = headless_provider_args(
        "opencode",
        provider.as_ref(),
        root,
        prompt,
        "json",
        Some("ses_fixture"),
        Some(&config),
    );
    let launch = crate::manager::headless::headless_provider_launch(
        "opencode",
        shim.to_str().unwrap(),
        &args,
    )
    .unwrap();
    let mut command = crate::utils::process::new_headless_command(&launch.executable);
    command
        .args(&launch.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let (status, output, errors) = collect(command, prompt).await;
    assert!(status.unwrap().success());
    let captured: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(captured["input"], serde_json::json!(prompt.as_bytes()));
    assert_eq!(
        captured["argv"],
        serde_json::json!([
            "run",
            "--print-logs",
            "--model",
            "fixture/model",
            "--agent",
            "fixture-agent",
            "--auto",
            "--session",
            "ses_fixture",
            "--format",
            "json",
            "--dir",
            root.to_str().unwrap()
        ])
    );
    assert!(errors.is_empty());
}

#[tokio::test]
async fn blocked_input_keeps_the_conversation_lease_alive() {
    let isolated = IsolatedHome::new().await;
    assert!(isolated.directory.path().exists());
    let now = chrono::Utc::now();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: "stdin-fixture".into(),
        provider: "opencode".into(),
        resume_session: "ses_fixture".into(),
        owner_kind: "automation".into(),
        owner_id: "stdin-run".into(),
        acquisition_id: "stdin-acquisition".into(),
        owner_node_id: None,
        mode: "background_resume".into(),
        started_at: now.to_rfc3339(),
        heartbeat_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::minutes(2)).to_rfc3339(),
    };
    wardian_core::conversation_lease::acquire_lease(lease.clone(), &now.to_rfc3339()).unwrap();
    let (mut child, mut guard, descendant) = ready_wrapper(false).await;
    let root = child.id().unwrap();
    let error = wait_with_intervals(
        &mut child,
        &"blocked".repeat(1_000_000),
        Duration::from_millis(250),
        Some(&lease.owner()),
        &mut guard,
        (Duration::from_millis(5), Duration::from_millis(10)),
    )
    .await
    .unwrap_err();
    assert!(error.contains("exceeded"), "{error}");
    let renewed = wardian_core::conversation_lease::load_leases()
        .into_iter()
        .find(|entry| entry.agent_id == lease.agent_id)
        .unwrap();
    assert_ne!(renewed.heartbeat_at, lease.heartbeat_at);
    assert_ne!(renewed.expires_at, lease.expires_at);
    assert_stopped(root).await;
    assert_stopped(descendant).await;
}

/// Inert subprocess entry point; only its parent test opts in. Rust avoids the
/// extra inherited stdin handles retained by Node/PowerShell on Windows.
#[test]
fn closed_input_fixture() {
    if std::env::var_os("WARDIAN_CLOSED_INPUT_FIXTURE").is_none() {
        return;
    }
    let mut descendant =
        std::process::Command::new(if cfg!(windows) { "node.exe" } else { "node" });
    descendant
        .args(["-e", "setInterval(() => {}, 1000)"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        descendant.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(windows)]
    unsafe {
        unsafe extern "C" {
            fn _close(fd: i32) -> i32;
        }
        let handle = winapi::um::processenv::GetStdHandle(winapi::um::winbase::STD_INPUT_HANDLE);
        // The CRT can own a separate duplicate of the inherited Win32 handle.
        // If both refer to one handle, the second close simply reports closed.
        _close(0);
        winapi::um::handleapi::CloseHandle(handle);
    }
    #[cfg(unix)]
    unsafe {
        assert_eq!(libc::close(libc::STDIN_FILENO), 0);
    }
    let mut descendant = descendant.spawn().unwrap();
    println!("{{\"pid\":{}}}", descendant.id());
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    let _ = descendant.wait();
}
