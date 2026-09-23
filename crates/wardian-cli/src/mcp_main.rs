//! Launch the console-backed CLI MCP server without allocating a Windows console.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let executable = match std::env::current_exe() {
        Ok(path) => path.with_file_name(if cfg!(windows) {
            "wardian-cli.exe"
        } else {
            "wardian-cli"
        }),
        Err(error) => {
            eprintln!("Could not locate the Wardian CLI beside the MCP launcher: {error}");
            return 1;
        }
    };

    #[cfg(windows)]
    let job = match kill_child_on_launcher_exit_job() {
        Ok(job) => job,
        Err(error) => {
            eprintln!("Could not supervise the Wardian MCP server process: {error}");
            return 1;
        }
    };

    let mut command = Command::new(executable);
    command.args(std::env::args_os().skip(1));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("Could not start the Wardian MCP server: {error}");
            #[cfg(windows)]
            if let Err(error) = release_launcher_job(job) {
                eprintln!("Could not release the Wardian MCP process job: {error}");
            }
            return 1;
        }
    };

    match child.wait() {
        Ok(status) => {
            #[cfg(windows)]
            if let Err(error) = release_launcher_job(job) {
                eprintln!("Could not release the Wardian MCP process job: {error}");
                return 1;
            }
            status.code().unwrap_or(1)
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("Could not wait for the Wardian MCP server: {error}");
            #[cfg(windows)]
            std::mem::forget(job);
            1
        }
    }
}

#[cfg(windows)]
fn kill_child_on_launcher_exit_job() -> Result<win32job::Job, String> {
    let job = win32job::Job::create().map_err(|error| error.to_string())?;
    let mut limits = job
        .query_extended_limit_info()
        .map_err(|error| error.to_string())?;
    limits.limit_kill_on_job_close();
    job.set_extended_limit_info(&limits)
        .map_err(|error| error.to_string())?;
    // Children inherit membership, so even a launcher exit during CreateProcess
    // cannot leave an unowned MCP child behind.
    job.assign_current_process()
        .map_err(|error| error.to_string())?;
    Ok(job)
}

#[cfg(windows)]
fn release_launcher_job(job: win32job::Job) -> Result<(), String> {
    let result = job
        .query_extended_limit_info()
        .map_err(|error| error.to_string())
        .and_then(|mut limits| {
            limits.clear_limits();
            job.set_extended_limit_info(&limits)
                .map_err(|error| error.to_string())
        });
    if result.is_ok() {
        drop(job);
    } else {
        // Keep kill-on-close in force until process exit so dropping this handle
        // cannot terminate the launcher before it reports the failure.
        std::mem::forget(job);
    }
    result
}
