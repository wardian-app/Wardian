use std::time::{Duration, Instant};

use tokio::process::Child;

/// Samples only the owned daemon, using a retained query-only Windows handle.
/// A failed query is optional diagnostics, never a startup failure or retry.
pub(super) struct StartupResourceObserver {
    started: Instant,
    before: Option<ProcessCounters>,
    #[cfg(windows)]
    process: Option<std::os::windows::io::OwnedHandle>,
}

impl StartupResourceObserver {
    pub(super) fn start(child: &Child) -> Self {
        #[cfg(windows)]
        let process = child.raw_handle().and_then(retain_process);
        #[cfg(not(windows))]
        let _ = child;
        let started = Instant::now();
        #[cfg(windows)]
        let before = process.as_ref().and_then(read_counters);
        #[cfg(not(windows))]
        let before = None;
        Self {
            started,
            before,
            #[cfg(windows)]
            process,
        }
    }

    pub(super) fn finish(self) -> Option<StartupResourceDelta> {
        #[cfg(windows)]
        let after = self.process.as_ref().and_then(read_counters);
        #[cfg(not(windows))]
        let after = None;
        resource_delta(self.before, after, self.started.elapsed())
    }
}

#[derive(Clone, Copy)]
struct ProcessCounters {
    creation_time: u64,
    kernel_cpu: u64,
    user_cpu: u64,
    read_operations: u64,
    write_operations: u64,
    other_operations: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}

/// Numeric process accounting over the sampling window, not disk-only I/O or
/// a provider phase label. Descendants and the Wardian process are excluded.
#[derive(serde::Serialize)]
pub(super) struct StartupResourceDelta {
    sample_elapsed_us: u64,
    kernel_cpu_us: u64,
    user_cpu_us: u64,
    read_operations: u64,
    write_operations: u64,
    other_operations: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}

fn resource_delta(
    before: Option<ProcessCounters>,
    after: Option<ProcessCounters>,
    elapsed: Duration,
) -> Option<StartupResourceDelta> {
    let (before, after) = before.zip(after)?;
    if before.creation_time != after.creation_time {
        return None;
    }
    Some(StartupResourceDelta {
        sample_elapsed_us: elapsed.as_micros().try_into().ok()?,
        // Windows reports CPU time in 100 ns units; truncate only after subtracting.
        kernel_cpu_us: after.kernel_cpu.checked_sub(before.kernel_cpu)? / 10,
        user_cpu_us: after.user_cpu.checked_sub(before.user_cpu)? / 10,
        read_operations: after.read_operations.checked_sub(before.read_operations)?,
        write_operations: after
            .write_operations
            .checked_sub(before.write_operations)?,
        other_operations: after
            .other_operations
            .checked_sub(before.other_operations)?,
        read_bytes: after.read_bytes.checked_sub(before.read_bytes)?,
        write_bytes: after.write_bytes.checked_sub(before.write_bytes)?,
        other_bytes: after.other_bytes.checked_sub(before.other_bytes)?,
    })
}

#[cfg(windows)]
fn retain_process(
    raw: std::os::windows::io::RawHandle,
) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle;
    use winapi::um::{handleapi::DuplicateHandle, processthreadsapi::GetCurrentProcess};
    let mut retained = std::ptr::null_mut();
    // The source is the existing Child handle. Duplicate its identity once with
    // query rights only; never reopen a PID, enumerate processes, or inherit it.
    let duplicated = unsafe {
        let current = GetCurrentProcess();
        DuplicateHandle(
            current,
            raw.cast(),
            current,
            &mut retained,
            winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            0,
        )
    };
    if duplicated == 0 {
        return None;
    }
    // DuplicateHandle returned a new owned handle, closed by OwnedHandle on drop.
    Some(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(retained.cast()) })
}

#[cfg(windows)]
fn read_counters(process: &std::os::windows::io::OwnedHandle) -> Option<ProcessCounters> {
    use std::os::windows::io::AsRawHandle;
    use winapi::shared::minwindef::FILETIME;
    use winapi::um::{processthreadsapi::GetProcessTimes, winbase::GetProcessIoCounters};
    let empty_time = || FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) =
        (empty_time(), empty_time(), empty_time(), empty_time());
    let mut io = std::mem::MaybeUninit::<winapi::um::winnt::IO_COUNTERS>::uninit();
    // The retained process handle stays alive across both calls. All output
    // pointers name initialized FILETIMEs or an IO_COUNTERS initialized on success.
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle().cast(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
        || unsafe { GetProcessIoCounters(process.as_raw_handle().cast(), io.as_mut_ptr()) } == 0
    {
        return None;
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let io = unsafe { io.assume_init() };
    Some(ProcessCounters {
        creation_time: ticks(creation),
        kernel_cpu: ticks(kernel),
        user_cpu: ticks(user),
        read_operations: io.ReadOperationCount,
        write_operations: io.WriteOperationCount,
        other_operations: io.OtherOperationCount,
        read_bytes: io.ReadTransferCount,
        write_bytes: io.WriteTransferCount,
        other_bytes: io.OtherTransferCount,
    })
}

#[cfg(test)]
#[path = "startup_resources_tests.rs"]
mod tests;
