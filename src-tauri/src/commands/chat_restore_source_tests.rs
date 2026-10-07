use super::*;
use crate::commands::provider_log_acquisition::{
    acquire_provider_log_batch, observe_provider_log_policy_with_identity,
};
use crate::state::conversation_archive::ConversationArchiveContext;
use std::ffi::OsString;
use std::io::Write;
use wardian_core::conversations::AgentConversationLoggingSetting;

struct TestHome(Option<OsString>);

impl TestHome {
    fn set(home: &Path) -> Self {
        let old = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", home);
        Self(old)
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        if let Some(home) = &self.0 {
            std::env::set_var("WARDIAN_HOME", home);
        } else {
            std::env::remove_var("WARDIAN_HOME");
        }
    }
}

struct Fixture {
    state: AppState,
    config: AgentConfig,
    user_home: PathBuf,
    path: PathBuf,
    captured: ProviderLogCaptureState,
    capture_bytes: Vec<u8>,
}

impl Fixture {
    async fn new(root: &Path, provider: &str, outside_root: bool) -> Self {
        let agent_id = uuid::Uuid::new_v4().to_string();
        let native_id = uuid::Uuid::new_v4().to_string();
        let config = AgentConfig {
            session_id: agent_id.clone(),
            session_name: "Captured restore fixture".into(),
            agent_class: "Coder".into(),
            provider: provider.into(),
            resume_session: Some(native_id.clone()),
            is_off: true,
            folder: root.join("workspace").to_string_lossy().into_owned(),
            conversation_logging: AgentConversationLoggingSetting::Enabled,
            ..Default::default()
        };
        let user_home = root.join("provider-home");
        let directory = if outside_root {
            root.join("not-provider-storage")
        } else if provider == "codex" {
            wardian_core::paths::agent_conversations_dir(&agent_id)
                .unwrap()
                .parent()
                .unwrap()
                .join("habitat/.codex/sessions/2026/01/01")
        } else {
            user_home.join(".claude/projects").join(
                crate::manager::claude::claude_project_dir_name(&config.folder),
            )
        };
        std::fs::create_dir_all(&directory).unwrap();
        let filename = if provider == "codex" {
            format!("rollout-2026-01-01T00-00-00-{native_id}.jsonl")
        } else {
            format!("{native_id}.jsonl")
        };
        let path = directory.join(filename);
        let header = if provider == "codex" {
            serde_json::json!({"type":"session_meta","payload":{"id":native_id,"cwd":config.folder}})
        } else {
            serde_json::json!({"type":"user","sessionId":native_id,"cwd":config.folder,
                "message":{"role":"user","content":"Owned fixture request"}})
        };
        std::fs::write(&path, format!("{header}\n")).unwrap();
        let state = AppState::new();
        // Use the normal inert restore constructor, not an injected log_path.
        let agent =
            crate::restored_agent_without_process(config.clone(), "Off", String::new(), None, None);
        state.agents.lock().await.insert(agent_id.clone(), agent);
        let snapshot = super::super::agent_archive_capture_snapshot(&state, &agent_id)
            .await
            .unwrap();
        let context = super::super::conversation_archive_context_from_snapshot(&snapshot);
        let key = context.provider_source_key.as_deref().unwrap();
        // Synthetic unit input represents a previously captured owned fresh
        // source. It is not native provider/startup acceptance or Chat prewarm.
        let batch =
            acquire_provider_log_batch(&agent_id, provider, &path, key, None, true).unwrap();
        state
            .conversation_archive
            .append_provider_log_batch_with_context(
                context.clone(),
                &batch.events,
                batch.previous.as_ref(),
                &batch.next,
            )
            .unwrap();
        let disabled = observe_provider_log_policy_with_identity(
            &path,
            key,
            Some(batch.next),
            false,
            false,
            None,
        )
        .unwrap()
        .unwrap();
        publish_policy(&state, &context, &disabled);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"ignored\",\"disabled\":true}\n")
            .unwrap();
        let enabled = observe_provider_log_policy_with_identity(
            &path,
            key,
            Some(disabled.next),
            true,
            false,
            None,
        )
        .unwrap()
        .unwrap();
        publish_policy(&state, &context, &enabled);
        assert_eq!(enabled.next.disabled_spans.len(), 1);
        let capture_path = wardian_core::paths::agent_conversations_dir(&agent_id)
            .unwrap()
            .parent()
            .unwrap()
            .join("conversation-capture.json");
        let capture_bytes = std::fs::read(capture_path).unwrap();
        Self {
            state,
            config,
            user_home,
            path,
            captured: enabled.next,
            capture_bytes,
        }
    }

    async fn bind(&self) {
        bind_for_background_in(
            &self.state,
            &self.config.session_id,
            None,
            Some(self.user_home.clone()),
        )
        .await
        .unwrap();
    }

    async fn cached(&self) -> Option<PathBuf> {
        self.state
            .agents
            .lock()
            .await
            .get(&self.config.session_id)
            .unwrap()
            .log_path
            .lock()
            .unwrap()
            .clone()
    }

    fn assert_capture_unchanged(&self) {
        let path = wardian_core::paths::agent_conversations_dir(&self.config.session_id)
            .unwrap()
            .parent()
            .unwrap()
            .join("conversation-capture.json");
        assert_eq!(std::fs::read(path).unwrap(), self.capture_bytes);
    }
}

fn publish_policy(
    state: &AppState,
    context: &ConversationArchiveContext,
    batch: &crate::commands::provider_log_acquisition::ProviderLogBatch,
) {
    state
        .conversation_archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &[],
            batch.previous.as_ref(),
            &batch.next,
        )
        .unwrap();
}

#[tokio::test]
async fn normal_background_request_binds_off_codex_without_foreground_discovery() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    let fixture = Fixture::new(root.path(), "codex", false).await;
    let source_before = std::fs::read(&fixture.path).unwrap();
    let (snapshot, _) =
        super::super::chat_read_snapshot_for_state(&fixture.state, &fixture.config.session_id)
            .unwrap();
    assert!(snapshot.log_path.is_none());
    assert!(fixture.cached().await.is_none());
    let request =
        super::super::background_capture_request(&fixture.state, &fixture.config.session_id)
            .await
            .unwrap()
            .unwrap();
    let canonical = fixture.path.canonicalize().unwrap();
    assert_eq!(request.source_path.as_ref(), Some(&canonical));
    assert_eq!(
        request.source.as_ref().unwrap().native_identity,
        fixture.captured.native_identity
    );
    assert_eq!(fixture.cached().await, Some(canonical));
    assert_eq!(std::fs::read(&fixture.path).unwrap(), source_before);
    fixture.assert_capture_unchanged();
    let agents = fixture.state.agents.lock().await;
    let agent = agents.get(&fixture.config.session_id).unwrap();
    assert!(agent.child_process.is_none());
    assert!(agent.process_id.is_none());
}

#[tokio::test]
async fn background_binding_binds_off_claude_with_exact_project_and_persisted_policy() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    let fixture = Fixture::new(root.path(), "claude", false).await;
    fixture.bind().await;
    assert_eq!(
        fixture.cached().await,
        Some(fixture.path.canonicalize().unwrap())
    );
    fixture.assert_capture_unchanged();
}

#[tokio::test]
async fn restored_binding_rejects_wrong_agent_session_and_storage_root() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    for provider in ["codex", "claude"] {
        let fixture = Fixture::new(root.path(), provider, false).await;
        let mut other = fixture.config.clone();
        other.session_id = uuid::Uuid::new_v4().to_string();
        fixture.state.agents.lock().await.insert(
            other.session_id.clone(),
            crate::restored_agent_without_process(other.clone(), "Off", String::new(), None, None),
        );
        bind_for_background_in(
            &fixture.state,
            &other.session_id,
            None,
            Some(fixture.user_home.clone()),
        )
        .await
        .unwrap();
        assert!(fixture
            .state
            .agents
            .lock()
            .await
            .get(&other.session_id)
            .unwrap()
            .log_path
            .lock()
            .unwrap()
            .is_none());
        fixture
            .state
            .agents
            .lock()
            .await
            .get(&fixture.config.session_id)
            .unwrap()
            .config
            .lock()
            .unwrap()
            .resume_session = Some(uuid::Uuid::new_v4().to_string());
        fixture.bind().await;
        assert!(fixture.cached().await.is_none());
        fixture.assert_capture_unchanged();
        let outside = Fixture::new(root.path(), provider, true).await;
        outside.bind().await;
        assert!(outside.cached().await.is_none());
        outside.assert_capture_unchanged();
    }
}

#[tokio::test]
async fn restored_binding_rejects_missing_replaced_and_rewritten_sources() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    for provider in ["codex", "claude"] {
        for mode in ["missing", "replacement", "continuity"] {
            let fixture = Fixture::new(root.path(), provider, false).await;
            let bytes = std::fs::read(&fixture.path).unwrap();
            if mode == "continuity" {
                let mut rewritten = bytes.clone();
                let anchor = &fixture.captured.continuity_anchor;
                assert!(anchor.len > 0);
                // Mutate the committed proof, not the later disabled interval.
                let end = usize::try_from(anchor.start + anchor.len).unwrap();
                rewritten[end - 1] ^= 1;
                std::fs::write(&fixture.path, rewritten).unwrap();
                let file = std::fs::File::open(&fixture.path).unwrap();
                assert_eq!(
                    native_file_identity(&file).unwrap(),
                    fixture.captured.native_identity
                );
            } else {
                let retained = fixture.path.with_extension("retained");
                std::fs::rename(&fixture.path, retained).unwrap();
                if mode == "replacement" {
                    std::fs::write(&fixture.path, bytes).unwrap();
                    let file = std::fs::File::open(&fixture.path).unwrap();
                    assert_ne!(
                        native_file_identity(&file).unwrap(),
                        fixture.captured.native_identity
                    );
                }
            }
            fixture.bind().await;
            assert!(fixture.cached().await.is_none(), "{provider}/{mode}");
            fixture.assert_capture_unchanged();
        }
    }
}

#[tokio::test]
async fn retired_background_incarnation_cannot_cache_the_restored_source() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let root = tempfile::tempdir().unwrap();
    let _home = TestHome::set(root.path());
    let fixture = Fixture::new(root.path(), "codex", false).await;
    let retired = Arc::new(Mutex::new("Off".to_string()));
    bind_for_background_in(
        &fixture.state,
        &fixture.config.session_id,
        Some(&retired),
        Some(fixture.user_home.clone()),
    )
    .await
    .unwrap();
    assert!(fixture.cached().await.is_none());
    fixture.assert_capture_unchanged();
}
