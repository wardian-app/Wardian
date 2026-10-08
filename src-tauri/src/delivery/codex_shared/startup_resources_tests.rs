use super::*;

fn counters() -> ProcessCounters {
    ProcessCounters {
        creation_time: 1,
        kernel_cpu: 100,
        user_cpu: 100,
        read_operations: 10,
        write_operations: 10,
        other_operations: 10,
        read_bytes: 100,
        write_bytes: 100,
        other_bytes: 100,
    }
}

#[test]
fn unavailable_or_changed_process_identity_has_no_resource_delta() {
    let before = counters();
    assert!(resource_delta(None, Some(before), Duration::ZERO).is_none());
    assert!(resource_delta(Some(before), None, Duration::ZERO).is_none());
    let after = ProcessCounters {
        creation_time: 2,
        ..before
    };
    assert!(resource_delta(Some(before), Some(after), Duration::ZERO).is_none());
}

#[test]
fn any_counter_rollback_rejects_the_whole_sample() {
    let before = counters();
    let after = ProcessCounters {
        user_cpu: before.user_cpu + 100,
        read_bytes: before.read_bytes - 1,
        ..before
    };
    assert!(resource_delta(Some(before), Some(after), Duration::from_secs(1)).is_none());
}

#[test]
fn resource_log_contains_only_numeric_deltas_and_preserves_socket_outcomes() {
    use super::super::SocketWaitOutcome;
    use super::super::{ChildLiveness, OwnerStartTimings, SocketPresence, SocketWaitDiagnostic};

    for outcome in [
        SocketWaitOutcome::Ready,
        SocketWaitOutcome::TimedOut,
        SocketWaitOutcome::ChildExited,
        SocketWaitOutcome::ChildStatusUnavailable,
        SocketWaitOutcome::Cancelled,
    ] {
        let before = counters();
        let after = ProcessCounters {
            user_cpu: before.user_cpu + 25,
            read_operations: before.read_operations + 1,
            read_bytes: before.read_bytes + 32,
            ..before
        };
        let diagnostic = SocketWaitDiagnostic {
            elapsed: Duration::from_millis(7),
            outcome,
            socket_presence: SocketPresence::Missing,
            child_liveness: ChildLiveness::Alive,
        };
        let expected = diagnostic.as_log_value();
        let mut timings = OwnerStartTimings::default();
        timings.record_socket_wait(diagnostic);
        timings.record_socket_wait_resources(resource_delta(
            Some(before),
            Some(after),
            Duration::from_micros(9),
        ));
        assert_eq!(timings.socket_wait_diagnostic_value(), expected);
        assert_eq!(timings.socket_wait, Duration::from_millis(7));
        let fields = timings.socket_wait_resources.as_object().unwrap();
        assert_eq!(fields.len(), 9);
        assert!(fields.values().all(serde_json::Value::is_u64));
        assert_eq!(fields["user_cpu_us"], 2);
        assert_eq!(fields["read_bytes"], 32);
        timings.record_socket_wait_resources(None);
        assert!(timings.socket_wait_resources.is_null());
        assert_eq!(timings.socket_wait_diagnostic_value(), expected);
    }
}

#[cfg(windows)]
#[test]
fn invalid_handle_cannot_enable_resource_sampling() {
    assert!(retain_process(std::ptr::null_mut()).is_none());
}

#[cfg(windows)]
#[test]
fn startup_resources_test_child() {
    if std::env::var_os("WARDIAN_STARTUP_RESOURCES_TEST_CHILD").is_none() {
        return;
    }
    use std::io::{Read, Write};
    println!("WARDIAN_RESOURCE_READY");
    std::io::stdout().flush().unwrap();
    let mut input = vec![0; 64 * 1024];
    std::io::stdin().read_exact(&mut input).unwrap();
    std::hint::black_box(input.iter().map(|byte| u64::from(*byte)).sum::<u64>());
    println!("WARDIAN_RESOURCE_DONE");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0; 1]).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn owned_observer_accounts_for_real_io_and_retains_identity_after_wait() {
    use std::os::windows::io::AsRawHandle;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use winapi::um::handleapi::GetHandleInformation;

    let work = async {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "delivery::codex_shared::owner::startup_resources::tests::startup_resources_test_child",
                "--nocapture",
            ])
            .env("WARDIAN_STARTUP_RESOURCES_TEST_CHILD", "1")
            .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        while !output
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .ends_with("WARDIAN_RESOURCE_READY")
        {}
        let observer = StartupResourceObserver::start(&child);
        let retained = observer.process.as_ref().expect("query-only owned handle");
        let mut flags = 0;
        assert_ne!(
            unsafe { GetHandleInformation(retained.as_raw_handle().cast(), &mut flags) },
            0
        );
        assert_eq!(flags & winapi::um::winbase::HANDLE_FLAG_INHERIT, 0);
        let expected_creation = observer.before.unwrap().creation_time;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&vec![0xa5; 64 * 1024])
            .await
            .unwrap();
        while !output
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .ends_with("WARDIAN_RESOURCE_DONE")
        {}
        let during = read_counters(retained).unwrap();
        let delta =
            resource_delta(observer.before, Some(during), observer.started.elapsed()).unwrap();
        assert!(delta.read_bytes >= 64 * 1024);
        assert!(delta.read_operations > 0);
        child.stdin.as_mut().unwrap().write_all(&[1]).await.unwrap();
        assert!(child.wait().await.unwrap().success());
        assert!(child.raw_handle().is_none());
        // The original Child handle is gone; the retained handle still names
        // the same process object, with no PID lookup or identity substitution.
        let after = read_counters(observer.process.as_ref().unwrap()).unwrap();
        assert_eq!(after.creation_time, expected_creation);
        assert!(observer.finish().is_some());
        assert!(StartupResourceObserver::start(&child).finish().is_none());
    };
    tokio::time::timeout(Duration::from_secs(15), work)
        .await
        .expect("owned test-child watchdog");
}
