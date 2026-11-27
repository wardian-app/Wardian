use std::future::Future;
use std::io::{self, PipeWriter, Write};
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::time::Duration;
use tokio::io::AsyncWrite;

/// The caller polls this writer inside the exact recovery lease check. A full
/// pipe yields before returning to the runtime, with no queued native write.
pub(super) struct GuardedProxyInput {
    pipe: Option<PipeWriter>,
    ready_again: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl GuardedProxyInput {
    pub(super) fn new(pipe: PipeWriter) -> io::Result<Self> {
        set_nonblocking(&pipe)?;
        Ok(Self {
            pipe: Some(pipe),
            ready_again: None,
        })
    }
}

#[cfg(windows)]
fn set_nonblocking(pipe: &PipeWriter) -> io::Result<()> {
    use std::{os::windows::io::AsRawHandle, ptr::null_mut};
    use winapi::um::{namedpipeapi::SetNamedPipeHandleState, winbase::PIPE_NOWAIT};
    let mut mode = PIPE_NOWAIT;
    // A private anonymous byte pipe supports the same wait mode as a named
    // pipe. Change only the parent's writer end before handing stdin to Codex.
    let changed = unsafe {
        SetNamedPipeHandleState(pipe.as_raw_handle() as _, &mut mode, null_mut(), null_mut())
    };
    if changed == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn set_nonblocking(pipe: &PipeWriter) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl AsyncWrite for GuardedProxyInput {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if let Some(backpressure) = &mut self.ready_again {
            ready!(backpressure.as_mut().poll(cx));
            self.ready_again = None;
        }
        let Some(pipe) = self.pipe.as_mut() else {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        };
        match pipe.write(bytes) {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            result => return Poll::Ready(result),
        }
        // Windows byte-mode WriteFile may report a successful zero-byte
        // write. No data crossed in that case. Poll later under a fresh lease.
        let mut backpressure = Box::pin(tokio::time::sleep(Duration::from_millis(5)));
        if backpressure.as_mut().poll(cx).is_ready() {
            cx.waker().wake_by_ref();
        }
        self.ready_again = Some(backpressure);
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        drop(self.pipe.take());
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::agent::acquire_agent_lifecycle_transition_lease_for_identity;
    use crate::delivery::codex_shared::{CodexSharedClient, RecoveryRequestLease};
    use crate::manager::RegistrationPublicationState;
    use std::io::Read;
    use wardian_core::conversation_lease::{self, CodexRecoveryLeaseScope};

    #[tokio::test]
    async fn full_recovery_proxy_pipe_yields_lease_and_refuses_late_write() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let id = uuid::Uuid::new_v4().to_string();
        let native = uuid::Uuid::new_v4().to_string();
        let lease = acquire_agent_lifecycle_transition_lease_for_identity(
            &id,
            "proxy-write",
            "codex",
            "",
            "fixture",
        )
        .unwrap();
        assert!(conversation_lease::refresh_codex_recovery_lease_persisted(
            lease.owner(),
            chrono::Duration::minutes(20),
            CodexRecoveryLeaseScope::Attach {
                agent_id: &id,
                native_id: &native
            },
        )
        .unwrap());
        let publication = RegistrationPublicationState::for_recovery(
            lease.owner().clone(),
            id.clone(),
            native.clone(),
        );
        let (mut output, pipe) = std::io::pipe().unwrap();
        let mut input = GuardedProxyInput::new(pipe).unwrap();
        let mut filled = 0;
        let fill = [b'x'; 4096];
        for chunk in [&fill[..], &fill[..1]] {
            loop {
                assert!(filled <= 1024 * 1024, "fixture pipe never filled");
                match input.pipe.as_mut().unwrap().write(chunk) {
                    Ok(0) => break,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    result => filled += result.unwrap(),
                }
            }
        }
        assert!(filled > 0);

        let (reader, _peer) = tokio::io::duplex(64);
        let socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
            tokio::io::join(reader, input),
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let client = CodexSharedClient::from_connected(id.clone(), 1, socket, None);
        let write = client.attachment_request(
            "thread/inject_items",
            serde_json::json!({"threadId": native}),
            &native,
            Some(RecoveryRequestLease::Attaching(&publication)),
            None,
        );
        tokio::pin!(write);
        assert!(futures_util::poll!(&mut write).is_pending());

        let mut leases = conversation_lease::load_leases_checked().unwrap();
        leases
            .iter_mut()
            .find(|entry| entry.owner() == *lease.owner())
            .unwrap()
            .expires_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        conversation_lease::save_leases(&leases).unwrap();
        let pause = acquire_agent_lifecycle_transition_lease_for_identity(
            &id,
            "proxy-write",
            "codex",
            &native,
            "pause",
        )
        .unwrap();
        assert_ne!(lease.owner(), pause.owner());
        assert!(write.await.unwrap_err().provider_boundary_crossed);
        assert!(publication.recovery_lease_lost());
        client.close().await;
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, vec![b'x'; filled]);
    }
}
