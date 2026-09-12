use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(super) struct OpenCodeReceiptFixture {
    pub(super) db_path: PathBuf,
    _xdg_data_home: ScopedEnvVar,
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

pub(super) fn opencode_receipt_fixture(
    home: &Path,
    provider_session_id: &str,
) -> OpenCodeReceiptFixture {
    let xdg_data_home = home.join("xdg-data");
    let opencode_dir = xdg_data_home.join("opencode");
    std::fs::create_dir_all(&opencode_dir).expect("create OpenCode fixture directory");
    let db_path = opencode_dir.join("opencode.db");
    let connection =
        rusqlite::Connection::open(&db_path).expect("create OpenCode fixture database");
    connection
        .execute_batch(
            r#"
            CREATE TABLE message (
                id text PRIMARY KEY,
                session_id text NOT NULL,
                time_created integer,
                time_updated integer,
                data text NOT NULL
            );
            CREATE TABLE part (
                id text PRIMARY KEY,
                message_id text NOT NULL,
                session_id text NOT NULL,
                time_created integer,
                time_updated integer,
                data text NOT NULL
            );
            "#,
        )
        .expect("create OpenCode fixture schema");
    connection
        .execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, 1, 1, ?3)",
            rusqlite::params!["baseline-message", provider_session_id, r#"{"role":"user"}"#],
        )
        .expect("insert OpenCode receipt baseline message");
    connection
        .execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, 2, 2, ?4)",
            rusqlite::params![
                "baseline-part",
                "baseline-message",
                provider_session_id,
                r#"{"type":"text","text":"previous prompt"}"#,
            ],
        )
        .expect("insert OpenCode receipt baseline part");
    drop(connection);

    OpenCodeReceiptFixture {
        db_path,
        _xdg_data_home: ScopedEnvVar::set("XDG_DATA_HOME", &xdg_data_home),
    }
}

pub(super) fn insert_opencode_user_receipt(
    db_path: &Path,
    provider_session_id: &str,
    message_id: &str,
    part_id: &str,
    prompt: &str,
) {
    let connection = rusqlite::Connection::open(db_path).expect("open OpenCode receipt database");
    connection
        .execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, 3, 3, ?3)",
            rusqlite::params![message_id, provider_session_id, r#"{"role":"user"}"#],
        )
        .expect("insert OpenCode user message");
    connection
        .execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, 4, 4, ?4)",
            rusqlite::params![
                part_id,
                message_id,
                provider_session_id,
                serde_json::json!({"type": "text", "text": prompt}).to_string(),
            ],
        )
        .expect("insert OpenCode user part");
}

pub(super) struct TestWardianHome {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    previous_home: Option<OsString>,
    _temp: tempfile::TempDir,
}

impl TestWardianHome {
    pub(super) fn new() -> Self {
        Self::from_guard(crate::utils::wardian_test_env_lock())
    }

    pub(super) async fn new_async() -> Self {
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

    pub(super) fn path(&self) -> &std::path::Path {
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
