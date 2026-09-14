//! Retain launch files through exact child exit and Pi watcher join.
use super::Receipt;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct OwnedChild {
    child: Option<Box<dyn portable_pty::Child + Send>>,
    receipt: Arc<Receipt>,
}
impl std::fmt::Debug for OwnedChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PiReceiptChild").finish_non_exhaustive()
    }
}
impl OwnedChild {
    pub fn new(child: Box<dyn portable_pty::Child + Send>, receipt: Arc<Receipt>) -> Self {
        Self {
            child: Some(child),
            receipt,
        }
    }
}
impl portable_pty::ChildKiller for OwnedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.receipt.stop();
        self.child.as_mut().expect("owned child").kill()
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        self.child.as_ref().expect("owned child").clone_killer()
    }
}
impl portable_pty::Child for OwnedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.child.as_mut().expect("owned child").try_wait()
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        self.child.as_mut().expect("owned child").wait()
    }
    fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.process_id())
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        self.child.as_ref().and_then(|child| child.as_raw_handle())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let Some(child) = self.child.take() else {
            return;
        };
        self.receipt.stop();
        let receipt = self.receipt.clone();
        // A cancelled async caller cannot abandon the child/file cleanup worker.
        std::thread::spawn(move || finish(child, receipt, Duration::from_secs(5)));
    }
}

pub(super) fn finish(
    mut child: Box<dyn portable_pty::Child + Send>,
    receipt: Arc<Receipt>,
    timeout: Duration,
) {
    receipt.stop();
    let _ = child.kill();
    let deadline = Instant::now() + timeout;
    let exited = loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if let Some(watcher) = receipt
        .watcher
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        if watcher.join().is_err() {
            return;
        }
    }
    if exited {
        receipt.cleanup();
    }
    // Uncertain exit deliberately retains this launch's files; newer launches
    // use different directories. Never remove a possible live appender's file.
}
