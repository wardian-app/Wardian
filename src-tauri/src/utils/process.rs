#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub use_no_window: bool,
}

impl HeadlessCommandSpec {
    fn from_program_and_args(program: &str, args: Vec<String>) -> Self {
        Self {
            program: program.to_string(),
            args,
            use_no_window: cfg!(target_os = "windows"),
        }
    }
}

pub fn headless_command_spec(program: &str) -> HeadlessCommandSpec {
    if cfg!(target_os = "windows") && program.ends_with(".cmd") {
        HeadlessCommandSpec::from_program_and_args(
            "cmd",
            vec!["/c".to_string(), program.to_string()],
        )
    } else {
        HeadlessCommandSpec::from_program_and_args(program, Vec::new())
    }
}

pub fn new_headless_command(program: &str) -> tokio::process::Command {
    use tokio::process::Command;

    let spec = headless_command_spec(program);
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);

    #[cfg(target_os = "windows")]
    {
        if spec.use_no_window {
            cmd.creation_flags(0x08000000);
        }
    }

    cmd
}

pub fn new_headless_std_command(program: &str) -> std::process::Command {
    let spec = headless_command_spec(program);
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.args);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        if spec.use_no_window {
            cmd.creation_flags(0x08000000);
        }
    }

    cmd
}

#[cfg(windows)]
fn is_supported_terminal_wrapper_process(process_name: &str) -> bool {
    matches!(
        process_name,
        "cmd.exe"
            | "pwsh.exe"
            | "powershell.exe"
            | "codex.exe"
            | "claude.exe"
            | "gemini.exe"
            | "node.exe"
    )
}

#[cfg(windows)]
pub fn is_wardian_session_process_candidate(
    process_name: &str,
    command_line: &str,
    session_id: &str,
) -> bool {
    let process_name = process_name.trim().to_ascii_lowercase();
    let command_line = command_line.trim().to_ascii_lowercase();
    let session_id = session_id.trim().to_ascii_lowercase();

    if session_id.is_empty()
        || !is_supported_terminal_wrapper_process(&process_name)
        || !command_line.contains(&session_id)
    {
        return false;
    }

    let direct_session_markers = [
        format!("resume {}", session_id),
        format!("--resume {}", session_id),
        format!("agents\\{}", session_id),
        format!("agents/{}", session_id),
        format!("{}.jsonl", session_id),
    ];

    direct_session_markers
        .iter()
        .any(|marker| command_line.contains(marker))
        || command_line.contains(".wardian\\")
        || command_line.contains(".wardian/")
}

#[cfg(windows)]
pub fn find_wardian_session_process_roots(session_id: &str, exclude_pid: Option<u32>) -> Vec<u32> {
    let mut sys = sysinfo::System::new_all();
    sys.refresh_all();

    let mut matches = Vec::new();
    for (pid, process) in sys.processes() {
        let pid_u32 = pid.as_u32();
        if exclude_pid == Some(pid_u32) {
            continue;
        }

        let process_name = process.name().to_string_lossy().to_string();
        let command_line = process
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        if is_wardian_session_process_candidate(&process_name, &command_line, session_id) {
            matches.push(pid_u32);
        }
    }

    matches.sort_unstable();
    matches.dedup();
    matches
}

#[cfg(windows)]
pub fn force_kill_process_tree(pid: u32) -> Result<(), String> {
    let output = new_headless_std_command("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .map_err(|err| format!("taskkill failed to launch for {}: {}", pid, err))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    let combined = format!("{} {}", stdout, stderr);

    if combined.contains("not found")
        || combined.contains("no running instance")
        || combined.contains("process") && combined.contains("not running")
    {
        return Ok(());
    }

    Err(format!(
        "taskkill /PID {} /T /F failed: {}",
        pid,
        combined.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::{headless_command_spec, new_headless_std_command};

    #[test]
    fn wraps_cmd_shims_for_headless_windows_execution() {
        let spec = headless_command_spec("example.cmd");

        if cfg!(target_os = "windows") {
            assert_eq!(spec.program, "cmd");
            assert_eq!(spec.args, vec!["/c".to_string(), "example.cmd".to_string()]);
            assert!(spec.use_no_window);
        } else {
            assert_eq!(spec.program, "example.cmd");
            assert!(spec.args.is_empty());
            assert!(!spec.use_no_window);
        }
    }

    #[test]
    fn uses_binary_directly_when_no_cmd_wrapper_is_needed() {
        let spec = headless_command_spec("codex");

        assert_eq!(spec.program, "codex");
        assert!(spec.args.is_empty());
        assert_eq!(spec.use_no_window, cfg!(target_os = "windows"));
    }

    #[test]
    fn std_command_reuses_headless_wrapper_logic() {
        let cmd = new_headless_std_command("example.cmd");

        if cfg!(target_os = "windows") {
            assert_eq!(cmd.get_program().to_string_lossy(), "cmd");
        } else {
            assert_eq!(cmd.get_program().to_string_lossy(), "example.cmd");
        }
    }

    #[cfg(windows)]
    #[test]
    fn identifies_wardian_session_wrapper_processes() {
        assert!(super::is_wardian_session_process_candidate(
            "cmd.exe",
            "\"C:\\Windows\\system32\\cmd.exe\" /d /c \"call codex.cmd --cd D:\\Trading resume 019d331a-0500-7592-969f-8f437886f42b --no-alt-screen\"",
            "019d331a-0500-7592-969f-8f437886f42b",
        ));

        assert!(super::is_wardian_session_process_candidate(
            "cmd.exe",
            "\"C:\\Windows\\system32\\cmd.exe\" /d /c \"call gemini.cmd --include-directories C:\\Users\\tgemi\\.wardian\\common --session 019d331a-0500-7592-969f-8f437886f42b\"",
            "019d331a-0500-7592-969f-8f437886f42b",
        ));
    }

    #[cfg(windows)]
    #[test]
    fn ignores_unrelated_processes_without_wardian_session_markers() {
        assert!(!super::is_wardian_session_process_candidate(
            "pwsh.exe",
            "pwsh.exe -NoLogo -Command \"Write-Host 019d331a-0500-7592-969f-8f437886f42b\"",
            "019d331a-0500-7592-969f-8f437886f42b",
        ));
    }
}
