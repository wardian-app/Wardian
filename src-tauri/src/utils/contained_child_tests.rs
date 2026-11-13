//! Real Windows children exercise the same launcher used by the shared owner.
use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

fn command(directory: &std::path::Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command",
        "[IO.File]::WriteAllText($env:WARDIAN_CHILD_STARTED, 'started'); $child = Start-Process -WindowStyle Hidden -FilePath powershell.exe -ArgumentList '-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 120' -PassThru; [IO.File]::WriteAllText($env:WARDIAN_DESCENDANT_PID, [string]$child.Id); Start-Sleep -Seconds 120"]);
    command.env("WARDIAN_CHILD_STARTED", directory.join("started"));
    command.env("WARDIAN_DESCENDANT_PID", directory.join("descendant"));
    command
}

fn process_handle(pid: u32) -> OwnedHandle {
    let handle = unsafe {
        winapi::um::processthreadsapi::OpenProcess(
            winapi::um::winnt::SYNCHRONIZE | winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    assert!(!handle.is_null());
    unsafe { OwnedHandle::from_raw_handle(handle as _) }
}

async fn exited(handle: &OwnedHandle) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if unsafe { winapi::um::synchapi::WaitForSingleObject(handle.as_raw_handle() as _, 0) }
                == winapi::um::winbase::WAIT_OBJECT_0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("retained process did not exit");
}

async fn descendant(directory: &std::path::Path) -> OwnedHandle {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(directory.join("descendant"))
                .ok()
                .and_then(|value| value.parse().ok())
            {
                break process_handle(pid);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("immediate descendant did not start")
}

#[tokio::test]
async fn assignment_precedes_first_instruction_and_shutdown_joins_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let (child, job) = OwnedBootstrapChild::spawn_with_setup(
        &mut command(directory.path()),
        || {},
        |job, pid| {
            // Deliberately widen the old spawn/assignment race.
            std::thread::sleep(Duration::from_millis(100));
            assert!(!directory.path().join("started").exists());
            assign_pid_to_job(job, pid, "test child")
        },
        |pid| {
            assert!(!directory.path().join("started").exists());
            resume_suspended_process(pid)
        },
    )
    .unwrap();
    let mut owned = OwnedBootstrapChild {
        child,
        complete: false,
        job: Some(job),
    };
    let descendant = descendant(directory.path()).await;
    let mut contained = 0;
    assert_ne!(
        unsafe {
            winapi::um::jobapi::IsProcessInJob(
                descendant.as_raw_handle() as _,
                owned.job.as_ref().unwrap().handle() as _,
                &mut contained,
            )
        },
        0
    );
    assert_ne!(contained, 0);
    owned
        .terminate_and_reap(tokio::time::Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    exited(&descendant).await;
    owned.release_after_verified_exit();
}

#[tokio::test]
async fn assignment_and_resume_failures_reap_without_executing() {
    for fail_assignment in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let captured = std::sync::Mutex::new(None);
        let result = OwnedBootstrapChild::spawn_with_setup(
            &mut command(directory.path()),
            || {},
            |job, pid| {
                *captured.lock().unwrap() = Some(process_handle(pid));
                if fail_assignment {
                    Err("assignment rejected".into())
                } else {
                    assign_pid_to_job(job, pid, "test child")
                }
            },
            |_| Err("resume rejected".into()),
        );
        assert_eq!(
            result.err().unwrap(),
            if fail_assignment {
                "assignment rejected"
            } else {
                "resume rejected"
            }
        );
        exited(&captured.into_inner().unwrap().unwrap()).await;
        assert!(!directory.path().join("started").exists());
        assert!(!directory.path().join("descendant").exists());
    }
}

#[tokio::test]
async fn cancellation_closes_job_and_leaves_no_live_descendant() {
    let directory = tempfile::tempdir().unwrap();
    let mut launch = command(directory.path());
    let (ready, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (child, job) = OwnedBootstrapChild::spawn_contained(&mut launch).unwrap();
        ready.send(process_handle(child.id().unwrap())).unwrap();
        let _child = child;
        let _job = job;
        std::future::pending::<()>().await;
    });
    let child = received.await.unwrap();
    let descendant = descendant(directory.path()).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    exited(&child).await;
    exited(&descendant).await;
}
