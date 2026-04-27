#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
static APP_PROCESS_SUPERVISOR: OnceLock<AppProcessSupervisor> = OnceLock::new();
#[cfg(windows)]
static APP_PROCESS_SUPERVISOR_ERROR: OnceLock<String> = OnceLock::new();

#[cfg(windows)]
#[derive(Debug)]
struct AppProcessSupervisor {
    _job: win32job::Job,
}

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
    use std::process::Stdio;
    use tokio::process::Command;

    let spec = headless_command_spec(program);
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        if spec.use_no_window {
            cmd.creation_flags(0x08000000);
        }
    }

    cmd
}

pub fn new_headless_std_command(program: &str) -> std::process::Command {
    use std::process::Stdio;

    let spec = headless_command_spec(program);
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.args);

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

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
pub fn init_app_process_supervisor() -> Result<(), String> {
    if APP_PROCESS_SUPERVISOR.get().is_some() {
        return Ok(());
    }

    if let Some(err) = APP_PROCESS_SUPERVISOR_ERROR.get() {
        return Err(err.clone());
    }

    let supervisor = match create_app_process_supervisor() {
        Ok(supervisor) => supervisor,
        Err(err) => {
            let _ = APP_PROCESS_SUPERVISOR_ERROR.set(err.clone());
            return Err(err);
        }
    };

    APP_PROCESS_SUPERVISOR
        .set(supervisor)
        .map_err(|_| "app process supervisor was initialized concurrently".to_string())
}

#[cfg(windows)]
pub fn app_process_supervisor_active() -> bool {
    APP_PROCESS_SUPERVISOR.get().is_some()
}

#[cfg(windows)]
fn create_app_process_supervisor() -> Result<AppProcessSupervisor, String> {
    let job = create_kill_on_close_job("app process supervisor")?;
    job.assign_current_process().map_err(|err| {
        format!(
            "failed to assign Wardian process to supervisor job: {}",
            err
        )
    })?;
    Ok(AppProcessSupervisor { _job: job })
}

#[cfg(windows)]
pub fn create_kill_on_close_job(context: &str) -> Result<win32job::Job, String> {
    let job = win32job::Job::create()
        .map_err(|err| format!("failed to create {} job object: {}", context, err))?;
    let mut info = job
        .query_extended_limit_info()
        .map_err(|err| format!("failed to query {} job limits: {}", context, err))?;
    info.limit_kill_on_job_close();
    job.set_extended_limit_info(&info)
        .map_err(|err| format!("failed to set {} kill-on-close limit: {}", context, err))?;
    Ok(job)
}

#[cfg(windows)]
pub fn assign_pid_to_job(job: &win32job::Job, pid: u32, context: &str) -> Result<(), String> {
    unsafe {
        use winapi::um::processthreadsapi::OpenProcess;
        use winapi::um::winnt::{PROCESS_SET_QUOTA, PROCESS_TERMINATE};

        let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(format!(
                "failed to open process {} for {} job assignment",
                pid, context
            ));
        }

        let assign_result = job.assign_process(handle as isize).map_err(|err| {
            format!(
                "failed to assign process {} to {} job: {}",
                pid, context, err
            )
        });
        winapi::um::handleapi::CloseHandle(handle);
        assign_result
    }
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
pub fn is_wardian_session_environment_candidate(environ: &[String], session_id: &str) -> bool {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return false;
    }

    environ.iter().any(|entry| {
        entry.split_once('=').is_some_and(|(key, value)| {
            key.eq_ignore_ascii_case("WARDIAN_SESSION_ID")
                && value.trim().eq_ignore_ascii_case(session_id)
        })
    })
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
        let environ = process
            .environ()
            .iter()
            .map(|entry| entry.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        if is_wardian_session_environment_candidate(&environ, session_id)
            || is_wardian_session_process_candidate(&process_name, &command_line, session_id)
        {
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
            "\"C:\\Windows\\system32\\cmd.exe\" /d /c \"call gemini.cmd --include-directories C:\\Users\\testuser\\.wardian\\common --session 019d331a-0500-7592-969f-8f437886f42b\"",
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

    #[cfg(windows)]
    #[test]
    fn identifies_wardian_session_environment_markers() {
        assert!(super::is_wardian_session_environment_candidate(
            &[
                "PATH=C:\\Windows\\System32".to_string(),
                "WARDIAN_SESSION_ID=019d331a-0500-7592-969f-8f437886f42b".to_string(),
            ],
            "019d331a-0500-7592-969f-8f437886f42b",
        ));

        assert!(!super::is_wardian_session_environment_candidate(
            &["WARDIAN_SESSION_ID=other-session".to_string()],
            "019d331a-0500-7592-969f-8f437886f42b",
        ));
    }
}
