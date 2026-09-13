use std::ffi::OsString;

pub(crate) struct TestWardianHome {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    previous_home: Option<OsString>,
    _temp: tempfile::TempDir,
}

impl TestWardianHome {
    pub(crate) fn new() -> Self {
        Self::from_guard(crate::utils::wardian_test_env_lock())
    }

    pub(crate) async fn new_async() -> Self {
        Self::from_guard(crate::utils::wardian_test_env_lock_async().await)
    }

    fn from_guard(lock: tokio::sync::MutexGuard<'static, ()>) -> Self {
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());
        let fixture = Self {
            _lock: lock,
            previous_home,
            _temp: temp,
        };
        wardian_core::db::init_db_at_path(&fixture.path().join("state.db"))
            .expect("init test database");
        fixture
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self._temp.path()
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
