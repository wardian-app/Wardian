//! A cold, private Codex state index can gate the listener before any RPC.

use super::{observe_socket_wait, ChildLiveness, SocketWaitDiagnostic, SocketWaitOutcome};
use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct DatabaseIdentity {
    volume: u32,
    file_index: u64,
}

struct Checkpoint {
    database: DatabaseIdentity,
    row: BackfillRow,
}

#[derive(Default)]
struct Progress {
    database: Option<DatabaseIdentity>,
    high_water: Option<String>,
    comparable_at: Option<Instant>,
    last_advance: Option<Instant>,
    advances: u32,
}

impl Progress {
    fn observe(&mut self, checkpoint: Option<Checkpoint>, sampled_at: Instant, now: Instant) {
        // A new file at the same path cannot continue this launch's evidence.
        let row = checkpoint.and_then(|checkpoint| {
            let database = self.database.get_or_insert(checkpoint.database);
            (*database == checkpoint.database).then_some(checkpoint.row)
        });
        let (cursor, running) = match row {
            Some(BackfillRow::Running(cursor)) => (cursor, true),
            Some(BackfillRow::Complete(cursor)) => (cursor, false),
            None => {
                self.invalidate_timing();
                return;
            }
        };
        let ordering = self.high_water.as_ref().map(|last| cursor.cmp(last));
        if ordering == Some(Ordering::Less) {
            self.invalidate_timing();
            return;
        }
        if ordering != Some(Ordering::Equal) {
            self.high_water = Some(cursor);
        }
        let fresh = sampled_at <= now && now - sampled_at < PROGRESS_WINDOW;
        let comparable = self
            .comparable_at
            .is_some_and(|last| last <= sampled_at && now - last < PROGRESS_WINDOW);
        if !fresh || !comparable {
            self.invalidate_timing();
        }
        // Completion can preserve previously earned grace during bind.
        if fresh && running {
            self.comparable_at = Some(sampled_at);
            if comparable && ordering == Some(Ordering::Greater) {
                self.last_advance = Some(sampled_at);
                self.advances = self.advances.saturating_add(1);
            }
        }
    }

    fn invalidate_timing(&mut self) {
        self.comparable_at = None;
        self.last_advance = None;
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
    pending: Option<JoinHandle<Option<Checkpoint>>>,
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

    pub(super) async fn wait_for_socket(
        mut self,
        socket: &Path,
        deadline: Instant,
        cancelled: impl Future<Output = ()>,
        mut child_liveness: impl FnMut() -> ChildLiveness,
    ) -> (SocketWaitDiagnostic, u32) {
        tokio::pin!(cancelled);
        let mut diagnostic = observe_socket_wait(
            socket,
            deadline,
            &mut cancelled,
            &mut child_liveness,
            |now| self.poll(now),
        )
        .await;
        // The retained owner task must drain this read before proxy or cleanup.
        if let Some(pending) = self.pending.take() {
            pending.abort();
            let _ = pending.await;
            if diagnostic.outcome == SocketWaitOutcome::Ready {
                diagnostic.child_liveness = child_liveness();
                diagnostic.outcome = if cancelled.now_or_never().is_some() {
                    SocketWaitOutcome::Cancelled
                } else {
                    diagnostic
                        .child_liveness
                        .failure()
                        .unwrap_or(diagnostic.outcome)
                };
            }
        }
        (diagnostic, self.progress.advances)
    }

    fn poll(&mut self, now: Instant) -> bool {
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
                .observe(result.ok().flatten(), self.sampled_at.unwrap_or(now), now);
        }
        if self
            .sampled_at
            .is_none_or(|at| now.saturating_duration_since(at) >= POLL_INTERVAL)
        {
            if self.pending.is_some() {
                self.progress.invalidate_timing();
                return false;
            }
            let home = home.clone();
            self.sampled_at = Some(now);
            self.pending = Some(tokio::task::spawn_blocking(move || read_checkpoint(&home)));
        }
        self.progress.recent(now)
    }
}

fn read_checkpoint(home: &Path) -> Option<Checkpoint> {
    // Hold the direct, single-link files through the read. In particular, never
    // follow a substituted file or let SQLite create missing WAL sidecars.
    let path = home.join(DATABASE);
    let (_database_file, database) = private_file(&path)?;
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
    let row = match status.as_str() {
        "running" => BackfillRow::Running(cursor),
        "complete" => BackfillRow::Complete(cursor),
        _ => return None,
    };
    Some(Checkpoint { database, row })
}

fn private_file(path: &Path) -> Option<(File, DatabaseIdentity)> {
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
    if info.nNumberOfLinks != 1
        || info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
    {
        return None;
    }
    let identity = DatabaseIdentity {
        volume: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    };
    Some((file, identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_checkpoint(cursor: &str) -> Option<Checkpoint> {
        Some(Checkpoint {
            database: DatabaseIdentity {
                volume: 0,
                file_index: 0,
            },
            row: BackfillRow::Running(cursor.into()),
        })
    }

    #[test]
    fn first_checkpoint_after_observation_gap_only_reestablishes_timing() {
        let now = Instant::now();
        let mut progress = Progress::default();
        progress.observe(fixture_checkpoint("a"), now, now);
        let resumed = now + PROGRESS_WINDOW + Duration::from_secs(1);
        progress.observe(fixture_checkpoint("b"), resumed, resumed);
        assert!(!progress.recent(resumed));
        let next = resumed + POLL_INTERVAL;
        progress.observe(fixture_checkpoint("c"), next, next);
        assert!(progress.recent(resumed + POLL_INTERVAL));
        assert_eq!(progress.advances, 1);
    }

    #[test]
    fn a_delayed_read_records_high_water_without_granting_grace() {
        let now = Instant::now();
        let received = now + PROGRESS_WINDOW + POLL_INTERVAL;
        let mut progress = Progress::default();
        progress.observe(fixture_checkpoint("a"), now, now);
        progress.observe(fixture_checkpoint("b"), now + POLL_INTERVAL, received);
        assert!(!progress.recent(received));
        progress.observe(fixture_checkpoint("a"), received, received);
        progress.observe(fixture_checkpoint("b"), received, received);
        assert!(!progress.recent(received));
        progress.observe(fixture_checkpoint("c"), received, received);
        assert!(progress.recent(received));
        assert_eq!(progress.advances, 1);
    }

    #[test]
    fn read_unavailability_breaks_comparability_without_resetting_high_water() {
        let now = Instant::now();
        let mut progress = Progress::default();
        progress.observe(fixture_checkpoint("a"), now, now);
        progress.observe(fixture_checkpoint("b"), now, now);
        progress.observe(None, now, now);
        progress.observe(fixture_checkpoint("c"), now, now);
        assert!(!progress.recent(now));
        progress.observe(fixture_checkpoint("a"), now, now);
        progress.observe(fixture_checkpoint("b"), now, now);
        assert!(!progress.recent(now));
        progress.observe(fixture_checkpoint("c"), now, now);
        progress.observe(fixture_checkpoint("d"), now, now);
        assert!(progress.recent(now));
        assert_eq!(progress.advances, 2);
    }

    #[tokio::test]
    async fn parked_probe_stays_in_owner_gate_until_joined_for_every_outcome() {
        use std::task::Poll;
        use tokio::sync::{oneshot, Mutex};

        for expected in [
            SocketWaitOutcome::Ready,
            SocketWaitOutcome::TimedOut,
            SocketWaitOutcome::Cancelled,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let socket = directory.path().join("control.sock");
            let database = directory.path().join(DATABASE);
            fs::write(&database, b"first owner").unwrap();
            let deadline = if expected == SocketWaitOutcome::TimedOut {
                Instant::now()
            } else {
                Instant::now() + Duration::from_secs(60)
            };
            if expected == SocketWaitOutcome::Ready {
                fs::write(&socket, b"presence fixture").unwrap();
            }
            let (started_tx, started) = oneshot::channel();
            let (release, parked) = std::sync::mpsc::channel();
            let (read_tx, read) = oneshot::channel();
            let probe_database = database.clone();
            let pending = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                if parked.recv().is_ok() {
                    read_tx.send(fs::read(probe_database).unwrap()).unwrap();
                }
                None
            });
            started.await.unwrap();
            let probe = BackfillProbe {
                pending: Some(pending),
                ..BackfillProbe::default()
            };
            let gate = Mutex::new(());
            let held = gate.lock().await;
            let startup = async {
                let _held = held;
                probe
                    .wait_for_socket(
                        &socket,
                        deadline,
                        async {
                            if expected != SocketWaitOutcome::Cancelled {
                                std::future::pending::<()>().await;
                            }
                        },
                        || ChildLiveness::Alive,
                    )
                    .await
            };
            tokio::pin!(startup);
            assert!(
                std::future::poll_fn(|cx| Poll::Ready(startup.as_mut().poll(cx)))
                    .await
                    .is_pending(),
                "owner left a probe running after {expected:?}"
            );
            assert!(gate.try_lock().is_err());
            release.send(()).unwrap();
            let (observed, _) = startup.await;
            assert_eq!(observed.outcome, expected);
            let _replacement = gate.try_lock().unwrap();
            fs::write(&database, b"replacement owner").unwrap();
            assert_eq!(read.await.unwrap(), b"first owner");
        }
    }

    #[test]
    fn queued_probe_is_cancelled_before_a_replacement_can_start() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::Poll;
        use tokio::sync::oneshot;

        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let (started, running) = oneshot::channel();
                let (release, parked) = std::sync::mpsc::channel::<()>();
                let blocker = tokio::task::spawn_blocking(move || {
                    started.send(()).unwrap();
                    let _ = parked.recv();
                });
                running.await.unwrap();

                let ran = std::sync::Arc::new(AtomicBool::new(false));
                let ran_in_probe = ran.clone();
                let pending = tokio::task::spawn_blocking(move || {
                    ran_in_probe.store(true, Ordering::Relaxed);
                    None
                });
                let probe = BackfillProbe {
                    pending: Some(pending),
                    ..BackfillProbe::default()
                };
                let home = tempfile::tempdir().unwrap();
                let socket = home.path().join("control.sock");
                let startup =
                    probe.wait_for_socket(&socket, Instant::now(), std::future::pending(), || {
                        ChildLiveness::Alive
                    });
                tokio::pin!(startup);
                let selected =
                    std::future::poll_fn(|cx| Poll::Ready(startup.as_mut().poll(cx))).await;
                release.send(()).unwrap();
                let (diagnostic, _) = match selected {
                    Poll::Ready(diagnostic) => diagnostic,
                    Poll::Pending => startup.await,
                };
                assert_eq!(diagnostic.outcome, SocketWaitOutcome::TimedOut);
                blocker.await.unwrap();
                tokio::task::spawn_blocking(|| ()).await.unwrap();
                assert!(!ran.load(Ordering::Relaxed));
            });
    }

    fn open_fixture(path: &Path, cursor: &str) -> Connection {
        let writer = Connection::open(path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE backfill_state
                   (id INTEGER PRIMARY KEY, status TEXT, last_watermark TEXT);",
            )
            .unwrap();
        writer
            .execute(
                "INSERT INTO backfill_state VALUES (1, 'running', ?1)",
                [cursor],
            )
            .unwrap();
        writer
    }

    #[test]
    fn only_strictly_advancing_running_checkpoints_renew_grace() {
        let now = Instant::now();
        let mut progress = Progress::default();
        let database = DatabaseIdentity {
            volume: 0,
            file_index: 0,
        };
        let sample = |row| Some(Checkpoint { database, row });
        let running = |cursor: &str| sample(BackfillRow::Running(cursor.into()));
        progress.observe(running("a"), now, now);
        assert!(!progress.recent(now));
        progress.observe(running("b"), now, now);
        assert!(progress.recent(now));
        let completed = now + POLL_INTERVAL;
        progress.observe(
            sample(BackfillRow::Complete("c".into())),
            completed,
            completed,
        );
        assert!(progress.recent(completed));
        let expired = now + PROGRESS_WINDOW;
        progress.observe(sample(BackfillRow::Complete("c".into())), expired, expired);
        assert!(!progress.recent(expired));
        progress.observe(running("c"), expired, expired);
        assert!(!progress.recent(expired));
        progress.observe(running("d"), expired, expired);
        assert!(progress.recent(expired));
        progress.observe(running("b"), expired, expired);
        assert!(!progress.recent(expired));
        progress.observe(running("d"), expired, expired);
        assert!(!progress.recent(expired));
        progress.observe(running("e"), expired, expired);
        assert!(progress.recent(expired));
        progress.observe(None, expired, expired);
        assert!(!progress.recent(expired));
        progress.observe(running("e"), expired, expired);
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
            Some(Checkpoint { row: BackfillRow::Running(cursor), .. }) if cursor == "sessions/synthetic"
        ));
        let other_link = home.join("other-link");
        fs::hard_link(&database, &other_link).unwrap();
        assert!(read_checkpoint(home).is_none());
        fs::remove_file(other_link).unwrap();
        drop(writer);
        assert!(read_checkpoint(home).is_none());
    }

    #[test]
    fn replacement_database_cannot_inherit_progress_from_the_original() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let database = home.join(DATABASE);
        let now = Instant::now();
        let mut progress = Progress::default();
        let writer = open_fixture(&database, "a");
        progress.observe(read_checkpoint(home), now, now);
        writer
            .execute("UPDATE backfill_state SET last_watermark = 'b'", [])
            .unwrap();
        progress.observe(read_checkpoint(home), now, now);
        assert!(progress.recent(now));
        drop(writer);
        fs::rename(&database, home.join("previous.sqlite")).unwrap();
        let _replacement = open_fixture(&database, "c");
        progress.observe(read_checkpoint(home), now, now);
        assert!(!progress.recent(now));
        assert_eq!(progress.advances, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_socket_requires_fresh_progress_in_the_private_index() {
        use super::super::{observe_socket_wait, ChildLiveness, SocketWaitOutcome};

        for (advance_at, expected) in [
            (None, SocketWaitOutcome::TimedOut),
            (Some(2), SocketWaitOutcome::TimedOut),
            (Some(20), SocketWaitOutcome::Ready),
        ] {
            let home = tempfile::tempdir().unwrap();
            let home = home.path();
            let writer = open_fixture(&home.join(DATABASE), "a");
            let socket = home.join("control.sock");
            let started = Instant::now();
            let deadline = started + Duration::from_secs(25);
            let mut next_sample = started;
            let mut progress = Progress::default();
            let (_, observed) = tokio::join!(
                async {
                    if let Some(at) = advance_at {
                        tokio::time::sleep_until(started + Duration::from_secs(at)).await;
                        writer
                            .execute("UPDATE backfill_state SET last_watermark = 'b'", [])
                            .unwrap();
                    }
                    tokio::time::sleep_until(deadline + Duration::from_secs(1)).await;
                    writer
                        .execute("UPDATE backfill_state SET status = 'complete'", [])
                        .unwrap();
                    tokio::time::sleep_until(deadline + Duration::from_secs(2)).await;
                    fs::write(&socket, b"presence fixture").unwrap();
                },
                observe_socket_wait(
                    &socket,
                    deadline,
                    std::future::pending(),
                    || ChildLiveness::Alive,
                    |now| {
                        if now >= next_sample {
                            progress.observe(read_checkpoint(home), now, now);
                            next_sample = now + Duration::from_secs(1);
                        }
                        progress.recent(now)
                    },
                )
            );
            assert_eq!(observed.outcome, expected, "advance at {advance_at:?}");
        }
    }
}
