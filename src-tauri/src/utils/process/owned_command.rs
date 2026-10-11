//! Containment for commands whose entire descendant tree belongs to Wardian.
//! Explorer, external applications and updater handoffs do not use this path.

#[cfg(windows)]
static APP_OWNED_PROCESS_JOB: std::sync::OnceLock<Result<win32job::Job, String>> =
    std::sync::OnceLock::new();

/// Preserve Tokio's stdio, environment, arguments and cancellation policy while
/// containing an ordinary owned root before its first instruction can execute.
pub(crate) fn spawn_owned_command(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    #[cfg(windows)]
    {
        spawn_with_jobs(command, None, |_| {})
    }
    #[cfg(not(windows))]
    {
        command.spawn()
    }
}

/// Also install an existing per-runtime stop job before resuming the child.
/// The caller retains that job independently of the app-lifetime containment.
#[cfg(windows)]
pub(crate) fn spawn_owned_command_in_job(
    command: &mut tokio::process::Command,
    job: &win32job::Job,
) -> std::io::Result<tokio::process::Child> {
    spawn_with_jobs(command, Some(job), |_| {})
}

#[cfg(windows)]
fn spawn_with_jobs(
    command: &mut tokio::process::Command,
    runtime_job: Option<&win32job::Job>,
    before_assign: impl FnOnce(&tokio::process::Child),
) -> std::io::Result<tokio::process::Child> {
    use winapi::um::{
        jobapi::IsProcessInJob,
        processthreadsapi::TerminateProcess,
        synchapi::WaitForSingleObject,
        winbase::{CREATE_SUSPENDED, WAIT_OBJECT_0},
    };

    // The verified outer job covers even the spawn-to-assignment crash window.
    // Deliberately omit CREATE_BREAKAWAY_FROM_JOB on ordinary owned roots.
    super::init_app_process_supervisor().map_err(std::io::Error::other)?;
    let job = APP_OWNED_PROCESS_JOB
        .get_or_init(|| super::create_kill_on_close_job("app-owned descendants"))
        .as_ref()
        .map_err(|error| std::io::Error::other(error.clone()))?;
    command.creation_flags(super::windows_silent_process_creation_flags() | CREATE_SUSPENDED);
    let child = command.spawn()?;
    let process = child
        .raw_handle()
        .ok_or_else(|| std::io::Error::other("suspended child has no process handle"))?
        as _;
    let setup = (|| {
        let supervisor = super::APP_PROCESS_SUPERVISOR
            .get()
            .and_then(|result| result.as_ref().ok())
            .ok_or_else(|| std::io::Error::other("app supervisor is unavailable"))?;
        let mut contained = 0;
        if unsafe { IsProcessInJob(process, supervisor._job.handle() as _, &mut contained) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if contained == 0 {
            return Err(std::io::Error::other(
                "owned child did not inherit the app job",
            ));
        }
        before_assign(&child);
        job.assign_process(process as isize)
            .map_err(std::io::Error::other)?;
        if let Some(runtime_job) = runtime_job {
            runtime_job
                .assign_process(process as isize)
                .map_err(std::io::Error::other)?;
        }
        resume_initial_thread(process)
    })();
    if let Err(error) = setup {
        // The retained process handle is the sole termination authority. Never
        // resume an uncontained child, and never use a PID to clean up a failure.
        if unsafe { TerminateProcess(process, 1) } == 0
            && unsafe { WaitForSingleObject(process, 0) } != WAIT_OBJECT_0
        {
            return Err(std::io::Error::other(format!(
                "{error}; suspended-child termination failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        if unsafe { WaitForSingleObject(process, 5000) } != WAIT_OBJECT_0 {
            return Err(std::io::Error::other(format!(
                "{error}; suspended-child exit was not observed"
            )));
        }
        return Err(error);
    }
    Ok(child)
}

/// std::process closes the initial thread handle. Find the unique thread of the
/// still-suspended child, retain it, and compare its owner's kernel object with
/// the spawn handle before resuming it. Snapshot IDs alone confer no authority.
#[cfg(windows)]
fn resume_initial_thread(process: winapi::um::winnt::HANDLE) -> std::io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use winapi::um::{
        handleapi::{CompareObjectHandles, INVALID_HANDLE_VALUE},
        processthreadsapi::{
            GetProcessId, GetProcessIdOfThread, OpenProcess, OpenThread, ResumeThread,
        },
        tlhelp32::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        winnt::{
            PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
            THREAD_SUSPEND_RESUME,
        },
    };
    let pid = unsafe { GetProcessId(process) };
    if pid == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful API calls return handles owned by this scope.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of_val(&entry) as u32;
    if unsafe { Thread32First(snapshot.as_raw_handle() as _, &mut entry) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut initial = None;
    loop {
        if entry.th32OwnerProcessID == pid {
            let raw = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    entry.th32ThreadID,
                )
            };
            if raw.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let thread = unsafe { OwnedHandle::from_raw_handle(raw as _) };
            let owner_pid = unsafe { GetProcessIdOfThread(thread.as_raw_handle() as _) };
            if owner_pid == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, owner_pid) };
            if raw.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let owner = unsafe { OwnedHandle::from_raw_handle(raw as _) };
            if unsafe { CompareObjectHandles(process, owner.as_raw_handle() as _) } == 0 {
                return Err(std::io::Error::other("initial thread owner changed"));
            }
            if initial.replace(thread).is_some() {
                return Err(std::io::Error::other(
                    "suspended child has more than one initial thread",
                ));
            }
        }
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        if unsafe { Thread32Next(snapshot.as_raw_handle() as _, &mut entry) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(18) {
                return Err(error);
            }
            break;
        }
    }
    let initial = initial
        .ok_or_else(|| std::io::Error::other("suspended child's initial thread is missing"))?;
    let previous = unsafe { ResumeThread(initial.as_raw_handle() as _) };
    if previous != 1 {
        return Err(std::io::Error::other(format!(
            "unexpected initial thread suspend count: {previous}"
        )));
    }
    Ok(())
}

#[cfg(all(test, windows))]
#[path = "owned_command_tests.rs"]
pub(crate) mod tests;

#[cfg(all(test, windows))]
pub(crate) fn test_owned_job_contains(handle: winapi::um::winnt::HANDLE) -> bool {
    let job = APP_OWNED_PROCESS_JOB.get().unwrap().as_ref().unwrap();
    let mut contained = 0;
    assert_ne!(
        unsafe { winapi::um::jobapi::IsProcessInJob(handle, job.handle() as _, &mut contained) },
        0
    );
    contained != 0
}
