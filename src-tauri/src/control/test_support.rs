use super::*;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(crate) async fn deliver_prompt_to_agent(
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    prompt: &str,
    input_mode: MessageInputMode,
) -> Result<DeliveryDetail, ControlError> {
    let delivery = deliver_message_to_target_with_headless_timeout(
        app,
        state,
        target,
        prompt,
        None,
        input_mode,
        QueuePolicy::QueueIfBusy,
        None,
        None,
        false,
        crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
    )
    .await?;

    record_conversation_delivery(state, &delivery, prompt, None).await;
    delivery.into_iter().next().ok_or_else(|| {
        ControlError::request_failed(format!(
            "prompt delivery produced no result for target: {target}"
        ))
    })
}

pub(super) async fn record_conversation_delivery(
    state: &AppState,
    delivery: &[DeliveryDetail],
    message: &str,
    origin: Option<&MessageOrigin>,
) {
    if message.trim().is_empty() {
        return;
    }

    let global_conversation_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    let sender_agent_id =
        origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.as_str());
    let target_settings = {
        let agents = state.agents.lock().await;
        delivery
            .iter()
            .filter(|detail| conversation_delivery_state_is_recordable(&detail.delivery_state))
            .filter_map(|detail| {
                let agent = agents.get(&detail.uuid)?;
                let config = agent.config.lock().ok()?;
                let setting = config.conversation_logging;
                let workspace = config
                    .git_worktree_folder
                    .clone()
                    .unwrap_or_else(|| config.folder.clone());
                let provider_session_ids = [
                    config.resume_session.as_deref(),
                    config.fresh_provider_session_id.as_deref(),
                ]
                .into_iter()
                .flatten()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
                let log_path =
                    agent.log_path.lock().ok().and_then(|path| {
                        path.as_ref().map(|path| path.to_string_lossy().to_string())
                    });
                let provider_source_key = provider_session_ids
                    .first()
                    .map(|session| format!("{}:session:{session}", config.provider))
                    .or_else(|| log_path.map(|path| format!("{}:source:{path}", config.provider)));
                let context = ConversationArchiveContext {
                    agent_id: detail.uuid.clone(),
                    agent_name: if config.session_name.trim().is_empty() {
                        detail.uuid.clone()
                    } else {
                        config.session_name.clone()
                    },
                    agent_class: config.agent_class.clone(),
                    workspace,
                    provider: config.provider.clone(),
                    provider_session_ids,
                    provider_source_key,
                };
                Some((context, setting))
            })
            .collect::<Vec<_>>()
    };

    for (context, agent_conversation_logging) in target_settings {
        if effective_conversation_logging(global_conversation_logging, agent_conversation_logging)
            != ConversationLoggingSetting::Enabled
        {
            continue;
        }
        let agent_id = context.agent_id.clone();
        if let Err(error) = state
            .conversation_archive
            .append_delivered_input_with_context(context, message, sender_agent_id)
        {
            manager::log_debug(&format!(
                "[WARDIAN] conversation archive delivery append failed for {agent_id}: {error}"
            ));
        }
    }
}

pub(super) fn conversation_delivery_state_is_recordable(delivery_state: &str) -> bool {
    matches!(
        delivery_state,
        "submitted" | "submit_sent_unverified" | "provider_accepted" | "approval_submitted"
    )
}

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
