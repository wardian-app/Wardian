fn wardian_test_env_mutex() -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;

    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| {
        // Every Wardian-home fixture takes this lock, so this is the one
        // place a test process opts out of flush-to-disk calls.
        wardian_core::durability::skip_flushes_in_test_process();
        tokio::sync::Mutex::new(())
    })
}

/// Serializes synchronous tests that share process environment or database state.
/// Keep the guard until fixture cleanup is complete.
///
/// # Panics
/// Panics inside an async runtime. Use [`wardian_test_env_lock_async`] there,
/// including in fixture constructors called by async tests.
pub(crate) fn wardian_test_env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    wardian_test_env_mutex().blocking_lock()
}

/// Serializes async tests with the same process-wide lock as synchronous tests.
/// Keep the guard across awaits and until fixture cleanup is complete.
pub(crate) async fn wardian_test_env_lock_async() -> tokio::sync::MutexGuard<'static, ()> {
    wardian_test_env_mutex().lock().await
}

mod test_env_tests {
    use super::{wardian_test_env_lock, wardian_test_env_lock_async};
    use std::future::Future;

    #[test]
    fn synchronous_and_async_fixtures_share_one_lock() {
        let sync_guard = wardian_test_env_lock();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        runtime.block_on(async {
            let mut async_lock = Box::pin(wardian_test_env_lock_async());
            let pending = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(async_lock.as_mut().poll(cx).is_pending())
            })
            .await;
            assert!(pending, "async fixture must wait for synchronous cleanup");

            drop(sync_guard);
            let async_guard = async_lock.await;
            drop(async_guard);
        });

        // Async fixture cleanup must also release the lock for synchronous tests.
        let _sync_guard = wardian_test_env_lock();
    }
}
