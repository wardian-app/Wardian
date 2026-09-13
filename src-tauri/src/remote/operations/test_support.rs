use std::ffi::OsString;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

pub(super) struct TestWardianHome {
    _temp: tempfile::TempDir,
    _lock: tokio::sync::MutexGuard<'static, ()>,
    previous_home: Option<OsString>,
}

impl TestWardianHome {
    pub(super) async fn new_async() -> Self {
        let lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());
        let fixture = Self {
            _lock: lock,
            previous_home,
            _temp: temp,
        };
        wardian_core::db::init_db_at_path(&fixture._temp.path().join("state.db"))
            .expect("init test database");
        fixture
    }
}

impl Drop for TestWardianHome {
    fn drop(&mut self) {
        match self.previous_home.take() {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

/// Owns deliberate contention on a separate thread, never on the async executor.
/// Construction waits until the mutex is held; dropping releases and joins the
/// holder even when the test panics. Do not acquire the same mutex before drop.
pub(super) struct HeldMutex {
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl HeldMutex {
    pub(super) fn new<T: Send + 'static>(mutex: Arc<Mutex<T>>) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = mutex.lock().expect("contention mutex");
            if ready_tx.send(()).is_ok() {
                let _ = release_rx.recv();
            }
        });
        let holder = Self {
            release: Some(release_tx),
            thread: Some(thread),
        };
        ready_rx.recv().expect("contention thread acquired mutex");
        holder
    }
}

impl Drop for HeldMutex {
    fn drop(&mut self) {
        self.release.take();
        if let Some(thread) = self.thread.take() {
            // Do not double-panic during test unwinding.
            let _ = thread.join();
        }
    }
}

#[test]
fn contention_is_ready_before_return_and_released_on_drop() {
    let mutex = Arc::new(Mutex::new(()));
    let holder = HeldMutex::new(mutex.clone());
    assert!(mutex.try_lock().is_err());
    drop(holder);
    assert!(mutex.try_lock().is_ok());
}

#[test]
fn panic_cleanup_releases_the_contended_mutex_without_poisoning() {
    let mutex = Arc::new(Mutex::new(()));
    let result = std::panic::catch_unwind(|| {
        let _holder = HeldMutex::new(mutex.clone());
        panic!("simulated assertion failure");
    });
    assert!(result.is_err());
    assert!(mutex.try_lock().is_ok());
}
