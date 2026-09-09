//! OpenCode's headless prompt input boundary.

use std::time::Duration;
use tokio::io::AsyncWriteExt;
use wardian_core::conversation_lease::ConversationLeaseOwner;

/// Pipe raw prompt bytes; OpenCode's positional message reconstruction adds
/// literal quotes. This configuration is used only by ordinary headless runs.
pub(super) fn configure(command: &mut tokio::process::Command) {
    command.stdin(std::process::Stdio::piped());
}

/// Deliver the complete UTF-8 prompt and EOF while the existing execution
/// deadline and conversation-lease heartbeat remain active. Never retry input:
/// a failed write may have delivered a prefix. Errors omit prompt and stderr.
/// The caller retains the process-tree guard during cancellation; on errors
/// this function terminates that tree and reaps the child before returning.
pub(super) async fn wait(
    child: &mut tokio::process::Child,
    prompt: &str,
    timeout: Duration,
    lease_owner: Option<&ConversationLeaseOwner>,
    process_tree: &mut super::HeadlessProcessTreeGuard,
) -> Result<std::process::ExitStatus, String> {
    wait_with_intervals(
        child,
        prompt,
        timeout,
        lease_owner,
        process_tree,
        (
            super::HEADLESS_PROCESS_POLL_INTERVAL,
            super::HEADLESS_LEASE_HEARTBEAT_INTERVAL,
        ),
    )
    .await
}

async fn wait_with_intervals(
    child: &mut tokio::process::Child,
    prompt: &str,
    timeout: Duration,
    lease_owner: Option<&ConversationLeaseOwner>,
    process_tree: &mut super::HeadlessProcessTreeGuard,
    intervals: (Duration, Duration),
) -> Result<std::process::ExitStatus, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let stdin = child.stdin.take();
    let outcome = {
        let delivery = async {
            let mut stdin =
                stdin.ok_or_else(|| "OpenCode prompt input pipe is unavailable".to_owned())?;
            async {
                stdin.write_all(prompt.as_bytes()).await?;
                // Windows queues pipe writes on Tokio's blocking pool. Await
                // the final write's result before closing the pipe for EOF.
                stdin.flush().await
            }.await.map_err(|error| {
                format!("OpenCode prompt input failed ({:?}); delivery may be partial and was not retried", error.kind())
            })?;
            // Dropping the pipe sends EOF without adding a newline or encoding.
            drop(stdin);
            Ok::<(), String>(())
        };
        let completion = super::wait_for_headless_child_with_intervals(
            child,
            "opencode",
            timeout,
            lease_owner,
            intervals.0,
            intervals.1,
        );
        tokio::pin!(delivery, completion);
        tokio::select! {
            // Start/poll the existing deadline and lease monitor before writes.
            biased;
            status = &mut completion => match status {
                Err(error) => Err(error),
                Ok(_) => Err("OpenCode exited before prompt input completed; delivery may be partial and was not retried".to_owned()),
            },
            delivered = &mut delivery => match delivered {
                Ok(()) => completion.await,
                Err(error) => {
                    // The monitor can close stdin while awaiting the killed
                    // child. Preserve its timeout instead of that broken pipe.
                    if tokio::time::Instant::now() >= deadline {
                        match completion.await {
                            Err(monitor_error) => Err(monitor_error),
                            Ok(_) => Err(error),
                        }
                    } else {
                        Err(error)
                    }
                },
            },
        }
    };
    let outcome = outcome.and_then(|status| {
        if status.success() {
            Ok(status)
        } else {
            // A provider can echo private input in stderr; retain its exit code
            // rather than forwarding that text through the error channel.
            Err(format!(
                "Headless provider opencode exited with status {}",
                status.code().unwrap_or(-1)
            ))
        }
    });
    if outcome.is_err() {
        // Keep the original PID even if the direct child already exited: a
        // wrapper's descendants may still own the inherited pipes.
        if let Some(pid) = process_tree.pid.take() {
            super::terminate_headless_process_tree(pid);
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    outcome
}

#[cfg(test)]
mod tests;
