use super::{CodexSharedClient, CodexSharedError, SocketWriter};
use crate::commands::agent::LifecycleLeaseHeartbeat;
use crate::manager::RegistrationPublicationState;
use futures_util::{task::AtomicWaker, SinkExt, Stream, StreamExt};
use std::future::{poll_fn, Future};
use std::sync::Mutex;
use std::task::Poll;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy)]
pub(super) enum RecoveryRequestLease<'a> {
    Preparing(&'a LifecycleLeaseHeartbeat),
    Attaching(&'a RegistrationPublicationState),
}

impl RecoveryRequestLease<'_> {
    fn with_effect<T>(
        self,
        agent: &str,
        native: &str,
        effect: impl FnOnce() -> T,
    ) -> Result<T, String> {
        match self {
            Self::Preparing(heartbeat) => heartbeat.with_current_codex_recovery_lease(
                "write Codex startup checkpoint",
                agent,
                native,
                effect,
            ),
            Self::Attaching(publication) => {
                publication.with_recovery_attachment(agent, native, effect)
            }
        }
    }
}

/// WebSocket reads can flush queued writes while answering a ping or close.
/// Pause read polling just for a fenced write, including its interrupted tail.
#[derive(Default)]
pub(super) struct RecoveryReadGate {
    paused: Mutex<bool>,
    waker: AtomicWaker,
}

impl RecoveryReadGate {
    pub(super) async fn next<S: Stream + Unpin>(&self, reader: &mut S) -> Option<S::Item> {
        poll_fn(|cx| {
            let paused = self.paused.lock().unwrap();
            if *paused {
                self.waker.register(cx.waker());
                Poll::Pending
            } else {
                reader.poll_next_unpin(cx)
            }
        })
        .await
    }

    fn pause(&self) {
        *self.paused.lock().unwrap() = true;
    }

    fn resume(&self) {
        *self.paused.lock().unwrap() = false;
        self.waker.wake();
    }
}

impl CodexSharedClient {
    pub(super) async fn send_recovery_effect(
        &self,
        writer: &mut Option<SocketWriter>,
        lease: RecoveryRequestLease<'_>,
        native_id: &str,
        message: Message,
    ) -> Result<(), CodexSharedError> {
        if self
            .proxy
            .as_ref()
            .is_some_and(|proxy| !proxy.recovery_writer)
        {
            return Err(CodexSharedError::unsupported(
                "recovery proxy writer unavailable",
            ));
        }
        let mut transport = writer
            .take()
            .ok_or_else(|| CodexSharedError::unsupported("provider writer unavailable"))?;
        // If this send is interrupted, the writer is dropped and the reader
        // stays paused. Closing the owner then aborts without flushing it.
        self.recovery_reads.pause();
        let written = {
            let send = transport.send(message);
            tokio::pin!(send);
            let mut began = false;
            poll_fn(|cx| {
                match lease.with_effect(&self.agent_id, native_id, || {
                    began = true;
                    send.as_mut().poll(cx)
                }) {
                    Ok(progress) => progress.map(Ok),
                    Err(error) => Poll::Ready(Err(if began {
                        CodexSharedError::uncertain(error)
                    } else {
                        CodexSharedError::unsupported(error)
                    })),
                }
            })
            .await?
        };
        written.map_err(|_| {
            CodexSharedError::uncertain("provider request write failed; not replayed")
        })?;
        *writer = Some(transport);
        self.recovery_reads.resume();
        Ok(())
    }
}
