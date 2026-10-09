#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
static APP_PROCESS_SUPERVISOR: OnceLock<Result<AppProcessSupervisor, String>> = OnceLock::new();

#[path = "process/owned_command.rs"]
mod owned_command;
pub(crate) use owned_command::spawn_owned_command;
#[cfg(windows)]
pub(crate) use owned_command::spawn_owned_command_in_job;
#[cfg(all(test, windows))]
pub(crate) use owned_command::{test_owned_job_contains, tests as lifetime_test_support};

/// A retained job distinguishes containment established before the provider
/// runs from the older best-effort, post-launch fallback assignment.
#[cfg(windows)]
pub struct RuntimeProcessJob {
    job: win32job::Job,
    contained_from_launch: bool,
}

#[cfg(all(test, windows))]
pub(crate) struct TestPty {
    pub(crate) listening_addr: std::net::SocketAddr,
    pair: Option<portable_pty::PtyPair>,
    reader: Option<std::thread::JoinHandle<()>>,
}

#[cfg(all(test, windows))]
impl Drop for TestPty {
    fn drop(&mut self) {
        self.pair.take();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

#[cfg(all(test, windows))]
pub(crate) async fn test_contained_pty_tree(
    temp: &std::path::Path,
) -> (
    RuntimeProcessJob,
    Box<dyn portable_pty::Child + Send + Sync>,
    TestPty,
    std::os::windows::io::OwnedHandle,
) {
    use std::os::windows::io::FromRawHandle;
    let marker = temp.join("descendant-pid");
    let port_marker = temp.join("descendant-port");
    let pair = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize::default())
        .unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = output.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut pending = Vec::new();
        while let Ok(count) = reader.read(&mut buf) {
            if count == 0 {
                break;
            }
            let mut captured = captured.lock().unwrap();
            captured.extend_from_slice(&buf[..count]);
            if captured.len() > 4096 {
                let excess = captured.len() - 4096;
                captured.drain(..excess);
            }
            drop(captured);
            pending.extend_from_slice(&buf[..count]);
            if pending.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                use std::io::Write;
                let _ = writer.write_all(b"\x1b[1;1R");
                pending.clear();
            } else if pending.len() > 8 {
                let excess = pending.len() - 8;
                pending.drain(..excess);
            }
        }
    });
    let mut pty = TestPty {
        listening_addr: "127.0.0.1:0".parse().unwrap(),
        pair: Some(pair),
        reader: Some(reader),
    };
    let mut cmd = portable_pty::CommandBuilder::new("powershell.exe");
    use base64::Engine;
    let descendant_program = "$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0); $listener.Start(); [IO.File]::WriteAllText($env:WARDIAN_TEST_CHILD_PORT, [string]$listener.LocalEndpoint.Port); Start-Sleep -Seconds 120";
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        descendant_program
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    cmd.args(["-NoProfile", "-Command", "$child = Start-Process -WindowStyle Hidden -FilePath powershell.exe -ArgumentList '-NoProfile', '-EncodedCommand', $env:WARDIAN_TEST_CHILD_PROGRAM -PassThru; [IO.File]::WriteAllText($env:WARDIAN_TEST_CHILD_PID, [string]$child.Id); Start-Sleep -Seconds 120"]);
    cmd.env("WARDIAN_TEST_CHILD_PID", &marker);
    cmd.env("WARDIAN_TEST_CHILD_PORT", &port_marker);
    cmd.env("WARDIAN_TEST_CHILD_PROGRAM", encoded);
    let job = RuntimeProcessJob::prepare(&mut cmd).unwrap();
    let mut child = pty.pair.as_mut().unwrap().slave.spawn_command(cmd).unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let pid: u32 = loop {
        if let Some(pid) = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|value| value.parse().ok())
        {
            break pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "descendant did not start: child={:?}, output={:?}",
            child.try_wait(),
            String::from_utf8_lossy(&output.lock().unwrap())
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let handle = unsafe {
        winapi::um::processthreadsapi::OpenProcess(
            winapi::um::winnt::SYNCHRONIZE | winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    assert!(!handle.is_null());
    let descendant = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle as _) };
    let mut contained = 0;
    assert_ne!(
        unsafe {
            winapi::um::jobapi::IsProcessInJob(handle, job.job.handle() as _, &mut contained)
        },
        0
    );
    assert_ne!(contained, 0);
    assert!(job.active_processes().unwrap() >= 2);
    loop {
        if let Some(port) = std::fs::read_to_string(&port_marker)
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
        {
            pty.listening_addr.set_port(port);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "descendant listener did not start"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    (job, child, pty, descendant)
}

#[cfg(windows)]
impl RuntimeProcessJob {
    pub(crate) fn fallback(job: win32job::Job) -> Self {
        Self {
            job,
            contained_from_launch: false,
        }
    }

    pub(crate) fn contained_from_launch(&self) -> bool {
        self.contained_from_launch
    }

    /// Windows creates the PTY child suspended, assigns this non-breakaway job,
    /// then resumes its thread. Assignment/resume failure fails the spawn.
    pub(crate) fn prepare(cmd: &mut portable_pty::CommandBuilder) -> Result<Self, String> {
        // The outer job also owns a suspended child if Wardian dies before the
        // nested assignment. Ordinary roots must never request breakaway.
        init_app_process_supervisor()?;
        use std::os::windows::io::BorrowedHandle;
        let job = create_kill_on_close_job("provider runtime")?;
        // SAFETY: job owns a valid handle throughout this call. The builder
        // duplicates it, so no borrowed handle survives beyond this call.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(job.handle() as _) };
        cmd.set_windows_job(borrowed)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            job,
            contained_from_launch: true,
        })
    }

    /// Seal process creation, retain member handles, and wait for their kernel
    /// termination as well as job emptiness. Cancellation drops the job, but produces
    /// no receipt and therefore cannot release a persisted conversation hold.
    pub(crate) async fn stop_and_join(
        &self,
        child: &mut (dyn portable_pty::Child + Send),
    ) -> Result<(), String> {
        if !self.contained_from_launch {
            return Err("Provider tree was not contained before launch".into());
        }
        self.seal_process_creation()?;
        let members = self.member_handles()?;
        self.terminate()?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let child_exited = child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some();
            use std::os::windows::io::AsRawHandle;
            let mut members_exited = true;
            for member in &members {
                match unsafe {
                    winapi::um::synchapi::WaitForSingleObject(member.as_raw_handle() as _, 0)
                } {
                    winapi::um::winbase::WAIT_OBJECT_0 => {}
                    winapi::shared::winerror::WAIT_TIMEOUT => members_exited = false,
                    _ => {
                        return Err(format!(
                            "Provider member wait failed: {}",
                            std::io::Error::last_os_error()
                        ))
                    }
                }
            }
            if child_exited && members_exited && self.active_processes()? == 0 {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Provider tree exit was not verified before the deadline".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    fn seal_process_creation(&self) -> Result<(), String> {
        use winapi::um::{
            jobapi2::{QueryInformationJobObject, SetInformationJobObject},
            winnt::{
                JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            },
        };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&limits) as u32;
        if unsafe {
            QueryInformationJobObject(
                self.job.handle() as _,
                JobObjectExtendedLimitInformation,
                &mut limits as *mut _ as _,
                size,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 0;
        if unsafe {
            SetInformationJobObject(
                self.job.handle() as _,
                JobObjectExtendedLimitInformation,
                &mut limits as *mut _ as _,
                size,
            )
        } == 0
        {
            return Err(format!(
                "Provider creation fence failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe {
            QueryInformationJobObject(
                self.job.handle() as _,
                JobObjectExtendedLimitInformation,
                &mut limits as *mut _ as _,
                size,
                std::ptr::null_mut(),
            )
        } == 0
            || limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS == 0
            || limits.BasicLimitInformation.ActiveProcessLimit != 0
        {
            return Err("Provider creation fence could not be verified".into());
        }
        Ok(())
    }

    fn member_handles(&self) -> Result<Vec<std::os::windows::io::OwnedHandle>, String> {
        use std::os::windows::io::FromRawHandle;
        use winapi::um::{
            jobapi2::QueryInformationJobObject,
            winnt::{JobObjectBasicProcessIdList, JOBOBJECT_BASIC_PROCESS_ID_LIST},
        };
        let mut capacity = 16usize;
        loop {
            // Pointer-sized storage provides alignment for the variable PID array.
            let bytes = 8 + capacity * std::mem::size_of::<usize>();
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            let list = storage.as_mut_ptr() as *mut JOBOBJECT_BASIC_PROCESS_ID_LIST;
            let result = unsafe {
                QueryInformationJobObject(
                    self.job.handle() as _,
                    JobObjectBasicProcessIdList,
                    list as _,
                    bytes as u32,
                    std::ptr::null_mut(),
                )
            };
            if result == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(234) && capacity < 65536 {
                    capacity = (capacity * 2)
                        .max(unsafe { (*list).NumberOfAssignedProcesses } as usize)
                        .min(65536);
                    continue;
                }
                return Err(format!("Provider member enumeration failed: {error}"));
            }
            let count = unsafe { (*list).NumberOfProcessIdsInList } as usize;
            if count > capacity || count != unsafe { (*list).NumberOfAssignedProcesses } as usize {
                return Err("Provider member enumeration was incomplete".into());
            }
            let ids = unsafe { std::slice::from_raw_parts((*list).ProcessIdList.as_ptr(), count) };
            let mut members = Vec::with_capacity(count);
            for &pid in ids {
                let pid = u32::try_from(pid).map_err(|_| "Invalid provider member PID")?;
                let raw = unsafe {
                    winapi::um::processthreadsapi::OpenProcess(
                        winapi::um::winnt::SYNCHRONIZE
                            | winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        pid,
                    )
                };
                if raw.is_null() {
                    return Err(format!(
                        "Provider member handle failed: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                let member =
                    unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw as _) };
                let mut contained = 0;
                if unsafe {
                    winapi::um::jobapi::IsProcessInJob(raw, self.job.handle() as _, &mut contained)
                } == 0
                    || contained == 0
                {
                    return Err("Provider member identity could not be verified".into());
                }
                members.push(member);
            }
            return Ok(members);
        }
    }

    pub(crate) fn terminate(&self) -> Result<(), String> {
        if unsafe { winapi::um::jobapi2::TerminateJobObject(self.job.handle() as _, 1) } == 0 {
            return Err(format!(
                "Provider job termination failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn active_processes(&self) -> Result<u32, String> {
        use winapi::um::{
            jobapi2::QueryInformationJobObject,
            winnt::{JobObjectBasicAccountingInformation, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION},
        };
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let result = unsafe {
            QueryInformationJobObject(
                self.job.handle() as _,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as _,
                std::mem::size_of_val(&info) as u32,
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(info.ActiveProcesses)
    }
}

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
    APP_PROCESS_SUPERVISOR
        .get_or_init(create_app_process_supervisor)
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

#[cfg(windows)]
pub fn app_process_supervisor_active() -> bool {
    matches!(APP_PROCESS_SUPERVISOR.get(), Some(Ok(_)))
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
        use winapi::um::processthreadsapi::{GetExitCodeProcess, OpenProcess};
        use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        // OpenProcess can still succeed for a terminated process while another
        // handle keeps its kernel object alive. Treat only STILL_ACTIVE (259)
        // as a live descendant, otherwise timeout cleanup reports a false leak.
        let mut exit_code = 0u32;
        let active = GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == 259;
        CloseHandle(handle);
        active
    }
}

fn is_supported_terminal_wrapper_process(process_name: &str) -> bool {
    matches!(
        process_name,
        "cmd.exe"
            | "pwsh.exe"
            | "powershell.exe"
            | "codex.exe"
            | "codex"
            | "claude.exe"
            | "claude"
            | "gemini.exe"
            | "gemini"
            | "opencode.exe"
            | "opencode"
            | "antigravity.exe"
            | "antigravity"
            | "agy.exe"
            | "agy"
            | "pi.exe"
            | "pi"
            | "node.exe"
            | "node"
    )
}

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

fn provider_cli_tokens(provider: &str) -> &'static [&'static str] {
    match provider.trim().to_ascii_lowercase().as_str() {
        "codex" => &[
            "codex",
            "codex.exe",
            "codex.cmd",
            "codex.ps1",
            "codex.js",
            "codex-cli",
        ],
        "claude" => &[
            "claude",
            "claude.exe",
            "claude.cmd",
            "claude.ps1",
            "claude.js",
        ],
        "gemini" => &[
            "gemini",
            "gemini.exe",
            "gemini.cmd",
            "gemini.ps1",
            "gemini.js",
        ],
        "opencode" => &[
            "opencode",
            "opencode.exe",
            "opencode.cmd",
            "opencode.ps1",
            "opencode.js",
        ],
        "antigravity" => &[
            "antigravity",
            "antigravity.exe",
            "agy",
            "agy.exe",
            "agy.cmd",
            "agy.js",
        ],
        "pi" => &["pi", "pi.exe", "pi.cmd", "pi.js"],
        "mock" => &[
            "mock",
            "mock.exe",
            "wardian-mock-provider",
            "wardian-mock-provider.exe",
        ],
        _ => &[],
    }
}

fn argument_has_provider_name(argument: &str, provider: &str) -> bool {
    let base_name = argument
        .trim_matches(['"', '\''])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(argument)
        .to_ascii_lowercase();
    provider_cli_tokens(provider).contains(&base_name.as_str())
}

fn split_process_command_line(command_line: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for character in command_line.chars() {
        match quote {
            Some(delimiter) if character == delimiter => quote = None,
            Some(_) => current.push(character),
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                }
            }
            None => current.push(character),
        }
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    arguments
}

fn shell_command_target(arguments: &[String], wrappers: &[&str]) -> Option<String> {
    let first = arguments.first()?;
    let mut command = split_process_command_line(first);
    command.extend(arguments.iter().skip(1).cloned());
    command.into_iter().find(|argument| {
        !wrappers
            .iter()
            .any(|wrapper| argument.eq_ignore_ascii_case(wrapper))
    })
}

/// Matches a process executable or Node/shell command that directly launches
/// the configured provider CLI. Environment markers alone are insufficient:
/// child tools inherit them without owning the provider conversation.
pub fn is_wardian_provider_process_candidate(
    provider: &str,
    process_name: &str,
    command_line: &str,
) -> bool {
    is_wardian_provider_process_candidate_args(
        provider,
        process_name,
        &split_process_command_line(command_line),
    )
}

fn is_wardian_provider_process_candidate_args(
    provider: &str,
    process_name: &str,
    arguments: &[String],
) -> bool {
    let process_name = process_name
        .trim_matches(['"', '\''])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(process_name)
        .to_ascii_lowercase();
    if argument_has_provider_name(&process_name, provider) {
        return true;
    }

    match process_name.as_str() {
        "node" | "node.exe" => arguments
            .get(1)
            .is_some_and(|script| argument_has_provider_name(script, provider)),
        "cmd" | "cmd.exe" => {
            let Some(command_start) = arguments
                .iter()
                .position(|argument| matches!(argument.to_ascii_lowercase().as_str(), "/c" | "/k"))
                .map(|index| index + 1)
            else {
                return false;
            };
            let command = arguments.get(command_start..).unwrap_or_default();
            shell_command_target(command, &["call"])
                .is_some_and(|argument| argument_has_provider_name(&argument, provider))
        }
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe" => {
            let Some(command_start) = arguments
                .iter()
                .position(|argument| {
                    matches!(
                        argument.to_ascii_lowercase().as_str(),
                        "-command" | "-c" | "-file"
                    )
                })
                .map(|index| index + 1)
            else {
                return false;
            };
            let command = arguments.get(command_start..).unwrap_or_default();
            shell_command_target(command, &["&"])
                .is_some_and(|argument| argument_has_provider_name(&argument, provider))
        }
        _ => false,
    }
}

/// Requires both a session association and a command that invokes the configured
/// provider CLI. This is only a candidate signal; descendants may invoke the
/// same CLI, so it must never authorize termination by itself.
pub fn is_wardian_provider_session_process_candidate(
    provider: &str,
    process_name: &str,
    command_line: &str,
    environ: &[String],
    session_id: &str,
) -> bool {
    is_wardian_provider_session_process_candidate_args(
        provider,
        process_name,
        command_line,
        &split_process_command_line(command_line),
        environ,
        session_id,
    )
}

fn is_wardian_provider_session_process_candidate_args(
    provider: &str,
    process_name: &str,
    command_line: &str,
    arguments: &[String],
    environ: &[String],
    session_id: &str,
) -> bool {
    if !is_wardian_provider_process_candidate_args(provider, process_name, arguments) {
        return false;
    }

    // A Wardian-owned provider can mention other agent IDs in projected
    // instructions or memory. Its explicit marker takes precedence over those
    // incidental command-line matches; unmarked external processes still use
    // the conservative command-line fallback.
    if let Some(marked_session_id) = environ.iter().find_map(|entry| {
        entry
            .split_once('=')
            .filter(|(key, _)| key.eq_ignore_ascii_case("WARDIAN_SESSION_ID"))
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    }) {
        if marked_session_id.eq_ignore_ascii_case(session_id.trim()) {
            return true;
        }

        if provider.eq_ignore_ascii_case("codex")
            && uuid::Uuid::parse_str(marked_session_id).is_ok()
            && uuid::Uuid::parse_str(session_id.trim()).is_ok()
        {
            // A child may inherit agent A's valid marker while explicitly
            // resuming B. Keep direct writer evidence, but ignore B mentioned
            // only in projected instructions or memory on A's command line.
            let command_line = command_line.to_ascii_lowercase();
            let session_id = session_id.trim().to_ascii_lowercase();
            return ["resume", "--resume", "--session"]
                .iter()
                .any(|flag| command_line.contains(&format!("{flag} {session_id}")));
        }
    }

    is_wardian_session_process_candidate(process_name, command_line, session_id)
}

/// Finds possible provider process candidates for one session. Results are
/// suitable for conservative restore checks only; they do not prove ownership.
pub fn find_wardian_provider_process_candidates(
    session_id: &str,
    provider: &str,
    exclude_pid: Option<u32>,
) -> Vec<u32> {
    find_wardian_provider_process_candidates_for_sessions(
        &[(session_id.to_string(), provider.to_string())],
        exclude_pid,
    )
    .remove(session_id)
    .unwrap_or_default()
}

/// Scans process metadata once to find possible provider invocations for
/// multiple sessions. The original argv boundaries are retained for paths
/// with spaces. A result is ambiguous and must not be killed automatically.
pub fn find_wardian_provider_process_candidates_for_sessions(
    sessions: &[(String, String)],
    exclude_pid: Option<u32>,
) -> std::collections::HashMap<String, Vec<u32>> {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing()
            .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
            .with_environ(sysinfo::UpdateKind::OnlyIfNotSet),
    );

    let mut matches: std::collections::HashMap<String, Vec<u32>> = sessions
        .iter()
        .map(|(session_id, _)| (session_id.clone(), Vec::new()))
        .collect();
    for (pid, process) in sys.processes() {
        let pid_u32 = pid.as_u32();
        if exclude_pid == Some(pid_u32) {
            continue;
        }

        let process_name = process.name().to_string_lossy().to_string();
        let command_arguments = process
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy())
            .map(|part| part.into_owned())
            .collect::<Vec<_>>();
        let command_line = command_arguments.join(" ");
        let environ = process
            .environ()
            .iter()
            .map(|entry| entry.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        for (session_id, provider) in sessions {
            if is_wardian_provider_session_process_candidate_args(
                provider,
                &process_name,
                &command_line,
                &command_arguments,
                &environ,
                session_id,
            ) {
                matches.entry(session_id.clone()).or_default().push(pid_u32);
            }
        }
    }

    for pids in matches.values_mut() {
        pids.sort_unstable();
        pids.dedup();
    }
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

    const SESSION_ID: &str = "019d331a-0500-7592-969f-8f437886f42b";

    #[cfg(windows)]
    #[tokio::test]
    async fn contained_pty_stop_joins_descendants_and_preserves_foreign_process() {
        use std::os::windows::io::AsRawHandle;
        let temp = tempfile::tempdir().unwrap();
        let mut outside = new_silent_std_command("powershell.exe");
        outside.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 120"]);
        let mut outside = TestChildCleanup(outside.spawn().unwrap());
        let (job, mut child, _pair, descendant) = super::test_contained_pty_tree(temp.path()).await;
        job.stop_and_join(child.as_mut()).await.unwrap();
        assert_eq!(job.active_processes().unwrap(), 0);
        assert_eq!(
            unsafe {
                winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0)
            },
            winapi::um::winbase::WAIT_OBJECT_0
        );
        assert!(outside.0.try_wait().unwrap().is_none());
        let _rebound = std::net::TcpListener::bind(_pair.listening_addr).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn sealed_live_job_rejects_new_pty_members_before_execution() {
        use std::os::windows::io::BorrowedHandle;
        let temp = tempfile::tempdir().unwrap();
        let (job, mut child, _pair, _descendant) =
            super::test_contained_pty_tree(temp.path()).await;
        job.seal_process_creation().unwrap();
        assert!(job.active_processes().unwrap() >= 2);
        let marker = temp.path().join("unexpected-new-member");
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .unwrap();
        let mut cmd = portable_pty::CommandBuilder::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-Command",
            "[IO.File]::WriteAllText($env:WARDIAN_TEST_MARKER, 'unexpected')",
        ]);
        cmd.env("WARDIAN_TEST_MARKER", &marker);
        // SAFETY: the retained job lives through spawn; the builder duplicates it.
        cmd.set_windows_job(unsafe { BorrowedHandle::borrow_raw(job.job.handle() as _) })
            .unwrap();
        assert!(pair.slave.spawn_command(cmd).is_err());
        assert!(!marker.exists());
        job.stop_and_join(child.as_mut()).await.unwrap();
    }

    #[cfg(windows)]
    struct TestChildCleanup(std::process::Child);
    #[cfg(windows)]
    impl Drop for TestChildCleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(windows)]
    #[test]
    fn failed_job_assignment_never_runs_the_pty_child() {
        use std::os::windows::io::AsHandle;
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("unexpected-provider-start");
        let invalid_job = std::fs::File::create(temp.path().join("file-not-job")).unwrap();
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .unwrap();
        let mut cmd = portable_pty::CommandBuilder::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-Command",
            "[IO.File]::WriteAllText($env:WARDIAN_TEST_START_MARKER, 'started')",
        ]);
        cmd.env("WARDIAN_TEST_START_MARKER", &marker);
        cmd.set_windows_job(invalid_job.as_handle()).unwrap();
        let error = match pair.slave.spawn_command(cmd) {
            Ok(_) => panic!("file cannot be a job"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("PTY job assignment failed"));
        assert!(!marker.exists());
    }

    #[test]
    fn identifies_provider_in_nested_windows_cmd_payload() {
        let command_line = format!(
            "\"C:\\Windows\\system32\\cmd.exe\" /d /c \"call codex.cmd --cd D:\\Trading resume {SESSION_ID} --no-alt-screen\""
        );

        assert!(super::is_wardian_provider_process_candidate(
            "codex",
            "cmd.exe",
            &command_line,
        ));
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "cmd.exe",
            &command_line,
            &[],
            SESSION_ID,
        ));
    }

    #[test]
    fn identifies_direct_unix_provider_invocation_with_session_identity() {
        let command_line = format!("codex resume {SESSION_ID}");
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex",
            &command_line,
            &[],
            SESSION_ID,
        ));
    }

    #[test]
    fn marked_provider_is_not_a_candidate_for_an_agent_named_in_its_instructions() {
        let other_session = "11111111-2222-4333-8444-555555555555";
        let command_line = format!(
            "codex.exe app-server -c developer_instructions=\"inspect .wardian/agents/{SESSION_ID}\""
        );
        let environment = [format!("WARDIAN_SESSION_ID={other_session}")];

        assert!(!super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex.exe",
            &command_line,
            &environment,
            SESSION_ID,
        ));
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex.exe",
            &command_line,
            &environment,
            other_session,
        ));
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex.exe",
            &command_line,
            &[],
            SESSION_ID,
        ));
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex.exe",
            &command_line,
            &["WARDIAN_SESSION_ID= ".to_string()],
            SESSION_ID,
        ));
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex.exe",
            &command_line,
            &["WARDIAN_SESSION_ID=unknown".to_string()],
            SESSION_ID,
        ));

        let direct_resume = format!("codex.exe resume {SESSION_ID}");
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "codex.exe",
            &direct_resume,
            &environment,
            SESSION_ID,
        ));
    }

    #[test]
    fn ignores_provider_name_in_unrelated_command_arguments() {
        assert!(!super::is_wardian_provider_process_candidate(
            "codex",
            "cmd.exe",
            "cmd.exe /d /c echo codex",
        ));
        assert!(!super::is_wardian_provider_process_candidate(
            "codex",
            "node.exe",
            "node.exe script.js codex",
        ));
        assert!(!super::is_wardian_provider_session_process_candidate(
            "codex",
            "python.exe",
            "python.exe -m http.server 8000",
            &[format!("WARDIAN_SESSION_ID={SESSION_ID}")],
            SESSION_ID,
        ));
    }

    #[test]
    fn identifies_provider_node_script_as_the_invoked_executable() {
        let command_line = format!(
            "node.exe \"C:\\Users\\testuser\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\bin\\codex.js\" resume {SESSION_ID}"
        );

        assert!(super::is_wardian_provider_process_candidate(
            "codex",
            "node.exe",
            &command_line,
        ));
        assert!(super::is_wardian_provider_session_process_candidate(
            "codex",
            "node.exe",
            &command_line,
            &[format!("WARDIAN_SESSION_ID={SESSION_ID}")],
            SESSION_ID,
        ));

        assert!(super::is_wardian_provider_process_candidate_args(
            "codex",
            "node.exe",
            &[
                "node.exe".into(),
                r"C:\Program Files\Wardian CLI\codex.js".into(),
                "resume".into(),
                SESSION_ID.into(),
            ],
        ));
    }

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
