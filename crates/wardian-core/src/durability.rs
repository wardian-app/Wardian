//! Flush-to-disk calls that a test process can skip.
//!
//! A test process cannot lose power, so a flush there proves nothing and costs
//! milliseconds per call on Windows. Skipping is opt-in at runtime: test
//! fixtures call [`skip_flushes_in_test_process`], which exists only with the
//! dev-dependency feature `test-skip-fsync`. A binary that never runs that
//! fixture code always flushes, even if feature unification compiled it in.
//! This crate's own unit tests skip by default. `WARDIAN_TEST_FSYNC=1` keeps
//! flushing in any test run.

use std::fs::File;
use std::io;

#[cfg(any(test, feature = "test-skip-fsync"))]
static SKIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(test));

#[cfg(any(test, feature = "test-skip-fsync"))]
fn flush_forced() -> bool {
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| std::env::var_os("WARDIAN_TEST_FSYNC").is_some_and(|value| value == "1"))
}

/// Skip flushes for the rest of this test process. Call only from test fixtures.
#[cfg(feature = "test-skip-fsync")]
pub fn skip_flushes_in_test_process() {
    SKIP.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn skip() -> bool {
    #[cfg(any(test, feature = "test-skip-fsync"))]
    {
        SKIP.load(std::sync::atomic::Ordering::Relaxed) && !flush_forced()
    }
    #[cfg(not(any(test, feature = "test-skip-fsync")))]
    {
        false
    }
}

/// `sync_all` and `sync_data` that an opted-in test process skips.
pub trait Durable {
    fn durable_sync_all(&self) -> io::Result<()>;
    fn durable_sync_data(&self) -> io::Result<()>;
}

impl Durable for File {
    fn durable_sync_all(&self) -> io::Result<()> {
        if skip() {
            return Ok(());
        }
        self.sync_all()
    }

    fn durable_sync_data(&self) -> io::Result<()> {
        if skip() {
            return Ok(());
        }
        self.sync_data()
    }
}
