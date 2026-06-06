#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
static APP_PROCESS_SUPERVISOR: OnceLock<AppProcessSupervisor> = OnceLock::new();
#[cfg(windows)]
static APP_PROCESS_SUPERVISOR_ERROR: OnceLock<String> = OnceLock::new();

pub(crate) fn windows_create_no_window_flag() -> u32 {
    0x0800_0000
}

pub(crate) fn windows_silent_process_creation_flags() -> u32 {
    windows_create_no_window_flag()
}

pub(crate) fn apply_silent_tokio_command_policy(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(windows_silent_process_creation_flags());
    }
}

pub(crate) fn apply_silent_std_command_policy(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        cmd.creation_flags(windows_silent_process_creation_flags());
    }
}

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

    let mut cmd = new_silent_command(program);

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    cmd
}

pub fn new_silent_command(program: &str) -> tokio::process::Command {
    use tokio::process::Command;

    let spec = headless_command_spec(program);
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);

    #[cfg(target_os = "windows")]
    {
        if spec.use_no_window {
            apply_silent_tokio_command_policy(&mut cmd);
        }
    }

    cmd
}

pub fn new_headless_std_command(program: &str) -> std::process::Command {
    use std::process::Stdio;

    let mut cmd = new_silent_std_command(program);

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    cmd
}

pub fn new_silent_std_command(program: &str) -> std::process::Command {
    let spec = headless_command_spec(program);
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.args);

    #[cfg(target_os = "windows")]
    {
        if spec.use_no_window {
            apply_silent_std_command_policy(&mut cmd);
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
    let job = create_app_process_supervisor_job()?;
    job.assign_current_process().map_err(|err| {
        format!(
            "failed to assign Wardian process to supervisor job: {}",
            err
        )
    })?;
    Ok(AppProcessSupervisor { _job: job })
}

#[cfg(windows)]
fn create_app_process_supervisor_job() -> Result<win32job::Job, String> {
    let info = app_process_supervisor_limit_info();
    win32job::Job::create_with_limit_info(&info).map_err(|err| {
        format!(
            "failed to create app process supervisor job object: {}",
            err
        )
    })
}

#[cfg(windows)]
fn app_process_supervisor_limit_info() -> win32job::ExtendedLimitInfo {
    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    info.limit_breakaway_ok();
    info
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
pub fn process_exists(pid: u32) -> bool {
    unsafe {
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::processthreadsapi::OpenProcess;
        use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        CloseHandle(handle);
        true
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
fn taskkill_failure_indicates_process_gone(
    stdout: &str,
    stderr: &str,
    process_exists_after_taskkill: bool,
) -> bool {
    if !process_exists_after_taskkill {
        return true;
    }

    let combined = format!("{} {}", stdout, stderr).to_ascii_lowercase();
    combined.contains("not found")
        || combined.contains("no running instance")
        || (combined.contains("process") && combined.contains("not running"))
}

#[cfg(windows)]
pub fn force_kill_process_tree(pid: u32) -> Result<(), String> {
    if !process_exists(pid) {
        return Ok(());
    }

    let output = new_silent_std_command("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .map_err(|err| format!("taskkill failed to launch for {}: {}", pid, err))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if taskkill_failure_indicates_process_gone(&stdout, &stderr, process_exists(pid)) {
        return Ok(());
    }

    let combined = format!("{} {}", stdout, stderr).to_ascii_lowercase();
    Err(format!(
        "taskkill /PID {} /T /F failed: {}",
        pid,
        combined.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        headless_command_spec, new_headless_std_command, new_silent_command, new_silent_std_command,
    };

    fn captured_output_command() -> (String, Vec<String>) {
        if cfg!(target_os = "windows") {
            (
                "cmd".to_string(),
                vec![
                    "/C".to_string(),
                    "echo wardian_stdout&& echo wardian_stderr 1>&2".to_string(),
                ],
            )
        } else {
            (
                "sh".to_string(),
                vec![
                    "-c".to_string(),
                    "printf '%s\\n' wardian_stdout; printf '%s\\n' wardian_stderr >&2".to_string(),
                ],
            )
        }
    }

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

    #[test]
    fn silent_std_command_preserves_captured_stdout_and_stderr() {
        let (program, args) = captured_output_command();
        let output = new_silent_std_command(&program)
            .args(args)
            .output()
            .expect("silent std command should run");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("wardian_stdout"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("wardian_stderr"));
    }

    #[tokio::test]
    async fn silent_tokio_command_preserves_captured_stdout_and_stderr() {
        let (program, args) = captured_output_command();
        let output = new_silent_command(&program)
            .args(args)
            .output()
            .await
            .expect("silent tokio command should run");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("wardian_stdout"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("wardian_stderr"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_silent_process_creation_flags_include_no_window() {
        assert_eq!(
            super::windows_silent_process_creation_flags() & super::windows_create_no_window_flag(),
            super::windows_create_no_window_flag()
        );
    }

    #[cfg(windows)]
    #[test]
    fn app_process_supervisor_allows_explicit_child_breakaway() {
        use std::{mem, ptr};
        use winapi::um::jobapi2::QueryInformationJobObject;
        use winapi::um::winnt::{
            JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let info = super::app_process_supervisor_limit_info();
        let job = win32job::Job::create_with_limit_info(&info).expect("job");
        let mut queried: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        let ok = unsafe {
            QueryInformationJobObject(
                job.handle() as _,
                JobObjectExtendedLimitInformation,
                &mut queried as *mut _ as _,
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ptr::null_mut(),
            )
        };

        assert_ne!(ok, 0);
        assert_eq!(
            queried.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        );
        assert_eq!(
            queried.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            JOB_OBJECT_LIMIT_BREAKAWAY_OK
        );
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

    #[cfg(windows)]
    #[test]
    fn taskkill_empty_failure_is_success_only_when_process_is_already_gone() {
        assert!(super::taskkill_failure_indicates_process_gone(
            "", "", false
        ));
        assert!(!super::taskkill_failure_indicates_process_gone(
            "", "", true
        ));
    }

    #[cfg(windows)]
    #[test]
    fn taskkill_process_not_found_text_is_success_even_if_exit_status_failed() {
        assert!(super::taskkill_failure_indicates_process_gone(
            "ERROR: The process \"123456\" not found.",
            "",
            true,
        ));
    }
}
