use super::diagnostics::{SafeStderrDiagnostic, StderrRedactionContext};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub(super) const MAX_CODEX_STDERR_CAPTURE_BYTES: usize = 16 * 1024;
const STDERR_READ_CHUNK_BYTES: usize = 4096;
const STDERR_READER_JOIN_GRACE: Duration = Duration::from_millis(500);

struct CaptureState {
    bytes: Box<[u8; MAX_CODEX_STDERR_CAPTURE_BYTES]>,
    len: usize,
    capturing: bool,
    truncated: bool,
    read_failed: bool,
    #[cfg(test)]
    peak_bytes_retained: usize,
}

impl Default for CaptureState {
    fn default() -> Self {
        Self {
            bytes: Box::new([0; MAX_CODEX_STDERR_CAPTURE_BYTES]),
            len: 0,
            capturing: false,
            truncated: false,
            read_failed: false,
            #[cfg(test)]
            peak_bytes_retained: 0,
        }
    }
}

/// Drains the child pipe for its lifetime while retaining only startup bytes.
pub(super) struct CodexStderrCapture {
    state: Arc<Mutex<CaptureState>>,
    reader: Option<JoinHandle<()>>,
    redaction: Option<StderrRedactionContext>,
}

impl CodexStderrCapture {
    pub(super) fn start<R>(mut stderr: R, redaction: StderrRedactionContext) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let state = Arc::new(Mutex::new(CaptureState {
            capturing: true,
            ..CaptureState::default()
        }));
        let reader_state = Arc::clone(&state);
        let reader = tokio::spawn(async move {
            let mut chunk = [0u8; STDERR_READ_CHUNK_BYTES];
            loop {
                let read = match stderr.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(_) => {
                        reader_state.lock().await.read_failed = true;
                        break;
                    }
                };

                let mut state = reader_state.lock().await;
                if !state.capturing {
                    continue;
                }
                let remaining = MAX_CODEX_STDERR_CAPTURE_BYTES.saturating_sub(state.len);
                let retained = read.min(remaining);
                let start = state.len;
                let end = start + retained;
                state.bytes[start..end].copy_from_slice(&chunk[..retained]);
                state.len = end;
                state.truncated |= retained < read;
                #[cfg(test)]
                {
                    state.peak_bytes_retained = state.peak_bytes_retained.max(state.len);
                }
            }
        });
        Self {
            state,
            reader: Some(reader),
            redaction: Some(redaction),
        }
    }

    /// Freeze startup capture while leaving the pipe drainer owned by the live process.
    pub(super) async fn seal_startup(&mut self) -> SafeStderrDiagnostic {
        self.diagnostic(false).await
    }

    /// Wait briefly for EOF after failed startup cleanup, then close an inherited writer.
    pub(super) async fn finish_startup(&mut self) -> SafeStderrDiagnostic {
        let read_failed = self.join_reader().await;
        self.diagnostic(read_failed).await
    }

    /// Join the drainer after shutdown, bounding cleanup if a descendant holds stderr open.
    pub(super) async fn join_after_owner_shutdown(&mut self) {
        let _ = self.join_reader().await;
        let mut state = self.state.lock().await;
        state.capturing = false;
        state.bytes.fill(0);
        state.len = 0;
    }

    async fn join_reader(&mut self) -> bool {
        let Some(reader) = self.reader.as_mut() else {
            return false;
        };
        match tokio::time::timeout(STDERR_READER_JOIN_GRACE, &mut *reader).await {
            Ok(result) => {
                self.reader.take();
                result.is_err()
            }
            Err(_) => {
                if let Some(reader) = self.reader.as_mut() {
                    reader.abort();
                    let _ = reader.await;
                }
                self.reader.take();
                true
            }
        }
    }

    async fn diagnostic(&mut self, join_failed: bool) -> SafeStderrDiagnostic {
        let redaction = self
            .redaction
            .take()
            .expect("startup stderr redaction is consumed once");
        let mut state = self.state.lock().await;
        state.capturing = false;
        let diagnostic = SafeStderrDiagnostic::from_bytes(
            &state.bytes[..state.len],
            state.truncated,
            state.read_failed || join_failed,
            &redaction,
        );
        state.bytes.fill(0);
        state.len = 0;
        diagnostic
    }
}

impl Drop for CodexStderrCapture {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            reader.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{duplex, AsyncWriteExt, ReadBuf};
    use tokio::sync::oneshot;

    struct NotifyingReader {
        inner: tokio::io::DuplexStream,
        first_read: Option<oneshot::Sender<()>>,
    }

    impl AsyncRead for NotifyingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let before = buffer.filled().len();
            let result = Pin::new(&mut self.inner).poll_read(context, buffer);
            if matches!(&result, Poll::Ready(Ok(()))) && buffer.filled().len() > before {
                if let Some(first_read) = self.first_read.take() {
                    let _ = first_read.send(());
                }
            }
            result
        }
    }

    fn empty_redaction() -> StderrRedactionContext {
        StderrRedactionContext::new(
            std::iter::empty::<std::path::PathBuf>(),
            std::iter::empty::<Vec<u8>>(),
        )
    }

    #[tokio::test]
    async fn high_volume_stderr_is_drained_with_a_strict_capture_bound() {
        let (reader, mut writer) = duplex(64);
        let mut capture = CodexStderrCapture::start(reader, empty_redaction());
        writer
            .write_all(&vec![b'x'; MAX_CODEX_STDERR_CAPTURE_BYTES + 32 * 1024])
            .await
            .unwrap();
        drop(writer);

        assert_eq!(
            capture.finish_startup().await.as_str(),
            Some("[provider stderr omitted: capture limit reached]")
        );
        assert!(capture.reader.is_none());
        let state = capture.state.lock().await;
        assert_eq!(state.peak_bytes_retained, MAX_CODEX_STDERR_CAPTURE_BYTES);
        assert_eq!(state.len, 0);
    }

    #[tokio::test]
    async fn successful_startup_stops_capture_and_shutdown_joins_the_drainer() {
        let (reader, mut writer) = duplex(64);
        let (first_read, read) = oneshot::channel();
        let reader = NotifyingReader {
            inner: reader,
            first_read: Some(first_read),
        };
        let mut capture = CodexStderrCapture::start(reader, empty_redaction());
        writer.write_all(b"socket ready\n").await.unwrap();
        read.await.unwrap();
        assert_eq!(capture.seal_startup().await.as_str(), Some("socket ready"));

        writer
            .write_all(&vec![b'y'; MAX_CODEX_STDERR_CAPTURE_BYTES + 32 * 1024])
            .await
            .unwrap();
        drop(writer);
        capture.join_after_owner_shutdown().await;

        assert!(capture.reader.is_none());
        let state = capture.state.lock().await;
        assert!(!state.capturing);
        assert_eq!(state.len, 0);
        assert_eq!(state.peak_bytes_retained, b"socket ready\n".len());
    }

    #[tokio::test]
    async fn failed_startup_joins_the_reader_before_returning_a_diagnostic() {
        let (reader, mut writer) = duplex(64);
        let mut capture = CodexStderrCapture::start(reader, empty_redaction());
        writer.write_all(b"startup rejected").await.unwrap();
        drop(writer);

        assert_eq!(
            capture.finish_startup().await.as_str(),
            Some("startup rejected")
        );
        assert!(capture.reader.is_none());
    }

    #[tokio::test]
    async fn shutdown_bounds_cleanup_when_an_inherited_writer_keeps_stderr_open() {
        let (reader, mut writer) = duplex(64);
        let mut capture = CodexStderrCapture::start(reader, empty_redaction());
        capture.seal_startup().await;

        tokio::time::timeout(Duration::from_secs(2), capture.join_after_owner_shutdown())
            .await
            .expect("stderr cleanup must remain bounded when a descendant holds the pipe");
        assert!(capture.reader.is_none());

        let write = tokio::time::timeout(
            Duration::from_secs(1),
            writer.write_all(&vec![b'z'; STDERR_READ_CHUNK_BYTES]),
        )
        .await
        .expect("aborted stderr reader must not leave the writer blocked");
        assert!(
            write.is_err(),
            "the reader side should be closed after cleanup"
        );
    }

    #[tokio::test]
    async fn cancelling_a_pending_join_leaves_the_reader_owned_for_drop() {
        let (reader, mut writer) = duplex(64);
        let mut capture = CodexStderrCapture::start(reader, empty_redaction());
        let mut join = Box::pin(capture.join_after_owner_shutdown());
        std::future::poll_fn(|context| match join.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(()) => panic!("join should wait while the stderr writer remains open"),
        })
        .await;
        drop(join);

        assert!(
            capture.reader.is_some(),
            "cancellation must retain the join handle"
        );
        drop(capture);
        let write = tokio::time::timeout(
            Duration::from_secs(1),
            writer.write_all(&vec![b'q'; STDERR_READ_CHUNK_BYTES]),
        )
        .await
        .expect("dropping the capture must abort its reader");
        assert!(
            write.is_err(),
            "the reader side should be closed after drop"
        );
    }
}
