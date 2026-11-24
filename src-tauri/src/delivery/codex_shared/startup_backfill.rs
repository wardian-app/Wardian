//! A cold, private Codex state index can gate the listener before any RPC.

use std::fs::{self, File, OpenOptions};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::FutureExt;
use rusqlite::{Connection, OpenFlags};
use tokio::{task::JoinHandle, time::Instant};
use winapi::um::{
    fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    winbase::FILE_FLAG_OPEN_REPARSE_POINT,
    winnt::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
};

const DATABASE: &str = "state_5.sqlite";
const WAL: &str = "state_5.sqlite-wal";
const SHM: &str = "state_5.sqlite-shm";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const PROGRESS_WINDOW: Duration = Duration::from_secs(20);

/// Qualify before spawn, under the owner gate. Existing or inaccessible state
/// cannot be attributed to this launch, so it keeps the ordinary socket budget.
pub(super) fn cold_home(home: &Path) -> Option<PathBuf> {
    [DATABASE, WAL, SHM]
        .iter()
        .all(|leaf| {
            matches!(fs::symlink_metadata(home.join(leaf)), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        })
        .then(|| home.to_path_buf())
}

enum BackfillRow {
    Running(String),
    Complete(String),
}

#[derive(Default)]
struct Progress {
    baseline: Option<String>,
    last_advance: Option<Instant>,
    advances: u32,
}

impl Progress {
    fn observe(&mut self, row: Option<BackfillRow>, sampled_at: Instant) {
        match (row, self.baseline.as_ref()) {
            (Some(BackfillRow::Running(cursor)), None) => self.baseline = Some(cursor),
            (Some(BackfillRow::Running(cursor)), Some(previous)) if &cursor > previous => {
                self.baseline = Some(cursor);
                self.last_advance = Some(sampled_at);
                self.advances = self.advances.saturating_add(1);
            }
            (Some(BackfillRow::Running(cursor)), Some(previous)) if &cursor == previous => {}
            // Completion can precede the bind by a poll. It cannot renew the window.
            (Some(BackfillRow::Complete(cursor)), Some(previous)) if &cursor >= previous => {}
            _ => self.last_advance = None,
        }
    }

    fn recent(&self, now: Instant) -> bool {
        self.last_advance
            .is_some_and(|at| now.saturating_duration_since(at) < PROGRESS_WINDOW)
    }
}

/// Read at most one checkpoint at a time off the async runtime.
#[derive(Default)]
pub(super) struct BackfillProbe {
    home: Option<PathBuf>,
    pending: Option<JoinHandle<Option<BackfillRow>>>,
    sampled_at: Option<Instant>,
    progress: Progress,
}

impl BackfillProbe {
    pub(super) fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            ..Self::default()
        }
    }

    pub(super) fn poll(&mut self, now: Instant) -> bool {
        let Some(home) = &self.home else {
            return false;
        };
        if let Some(result) = self
            .pending
            .as_mut()
            .and_then(|pending| pending.now_or_never())
        {
            self.pending = None;
            self.progress
                .observe(result.ok().flatten(), self.sampled_at.unwrap_or(now));
        }
        if self
            .sampled_at
            .is_none_or(|at| now.saturating_duration_since(at) >= POLL_INTERVAL)
        {
            if self.pending.is_some() {
                return false;
            }
            let home = home.clone();
            self.sampled_at = Some(now);
            self.pending = Some(tokio::task::spawn_blocking(move || read_checkpoint(&home)));
        }
        self.progress.recent(now)
    }

    pub(super) fn advances(&self) -> u32 {
        self.progress.advances
    }
}

fn read_checkpoint(home: &Path) -> Option<BackfillRow> {
    // Hold the direct, single-link files through the read. In particular, never
    // follow a substituted file or let SQLite create missing WAL sidecars.
    let path = home.join(DATABASE);
    let _database = private_file(&path)?;
    let _wal = private_file(&home.join(WAL))?;
    let _shm = private_file(&home.join(SHM))?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .ok()?;
    connection.busy_timeout(Duration::ZERO).ok()?;
    let (status, cursor): (String, String) = connection
        .query_row(
            "SELECT status, last_watermark FROM backfill_state
             WHERE id = 1 AND length(last_watermark) BETWEEN 1 AND 32768",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    match status.as_str() {
        "running" => Some(BackfillRow::Running(cursor)),
        "complete" => Some(BackfillRow::Complete(cursor)),
        _ => None,
    }
}

fn private_file(path: &Path) -> Option<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the owned handle remains live and the output has the Win32 ABI.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) } == 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    (info.nNumberOfLinks == 1
        && info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) == 0)
        .then_some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_strictly_advancing_running_checkpoints_renew_grace() {
        let now = Instant::now();
        let mut progress = Progress::default();
        let running = |cursor: &str| Some(BackfillRow::Running(cursor.into()));
        progress.observe(running("a"), now);
        assert!(!progress.recent(now));
        progress.observe(running("b"), now);
        assert!(progress.recent(now));
        let expired = now + PROGRESS_WINDOW;
        progress.observe(running("b"), expired);
        progress.observe(Some(BackfillRow::Complete("c".into())), expired);
        assert!(!progress.recent(expired));
        progress.observe(running("c"), expired);
        assert!(progress.recent(expired));
        progress.observe(running("b"), expired);
        assert!(!progress.recent(expired));
        progress.observe(running("d"), expired);
        assert!(progress.recent(expired));
        progress.observe(None, expired);
        assert!(!progress.recent(expired));
        progress.observe(running("d"), expired);
        assert!(!progress.recent(expired));
        assert_eq!(progress.advances, 3);
    }

    #[test]
    fn cold_qualification_and_read_require_private_wal_files() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert_eq!(cold_home(home).as_deref(), Some(home));
        assert!(read_checkpoint(home).is_none());
        let database = home.join(DATABASE);
        let writer = Connection::open(&database).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE unrecognized (id INTEGER);")
            .unwrap();
        assert!(cold_home(home).is_none());
        assert!(read_checkpoint(home).is_none());
        writer.execute_batch(
            "CREATE TABLE backfill_state (id INTEGER PRIMARY KEY, status TEXT, last_watermark TEXT);
             INSERT INTO backfill_state VALUES (1, 'running', 'sessions/synthetic');",
        ).unwrap();
        assert!(matches!(
            read_checkpoint(home),
            Some(BackfillRow::Running(cursor)) if cursor == "sessions/synthetic"
        ));
        let other_link = home.join("other-link");
        fs::hard_link(&database, &other_link).unwrap();
        assert!(read_checkpoint(home).is_none());
        fs::remove_file(other_link).unwrap();
        drop(writer);
        assert!(read_checkpoint(home).is_none());
    }
}
