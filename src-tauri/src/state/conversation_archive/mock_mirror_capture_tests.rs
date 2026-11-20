use super::*;
use crate::commands::chat::{AgentArchiveCaptureSnapshot, ArchiveCaptureResult};
use crate::commands::provider_log_acquisition::{self, ProviderLogBatch};
use std::io::Write;

struct Fixture {
    home: crate::control::test_support::TestWardianHome,
    archive: ConversationArchiveState,
    context: ConversationArchiveContext,
    snapshot: AgentArchiveCaptureSnapshot,
    watch: ArchiveCaptureResult,
    raw: serde_json::Value,
}

impl Fixture {
    fn new() -> Self {
        let home = crate::control::test_support::TestWardianHome::new();
        let path = home.path().join("mock-transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"init\",\"session_id\":\"owned\"}\n").unwrap();
        let raw = json!({"type":"model", "turn_id":"actual-turn", "content":"Same actual body",
            "wardian_mirror":{"observation_id":"c60e52bf-71f2-43f1-a313-9ea96fe2f7bc:1",
                "incarnation":"c60e52bf-71f2-43f1-a313-9ea96fe2f7bc", "provider_session_id":"owned", "logged":true}});
        let mut message =
            crate::providers::transcript::extract_transcript_message("mock", &raw.to_string())
                .unwrap();
        crate::providers::transcript::bind_mock_watch_message(
            &mut message,
            "owned",
            &path.to_string_lossy(),
        );
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "mirror-capture".into(),
            32,
            4096,
        )));
        watch_state.lock().unwrap().push_transcript(message);
        let snapshot = AgentArchiveCaptureSnapshot {
            session_id: "mirror-capture".into(),
            provider: "mock".into(),
            resume_session: None,
            fresh_provider_session_id: Some("owned".into()),
            cleared_provider_sessions: Vec::new(),
            current_status: "Processing...".into(),
            last_status_at: None,
            log_path: Some(path),
            agent_name: "Mirror capture".into(),
            agent_class: "Coder".into(),
            workspace: home.path().to_string_lossy().into_owned(),
            agent_conversation_logging: AgentConversationLoggingSetting::Enabled,
            watch_state,
        };
        // This is the ordinary collector's empty-current-batch fallback. The
        // source claim remains unproved until normal acquisition sees its row.
        let watch =
            crate::commands::chat::collect_agent_chat_events_for_archive(&snapshot).unwrap();
        assert!(watch.watch_mirrors.is_some());
        let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
        Self {
            home,
            archive: ConversationArchiveState::default(),
            context,
            snapshot,
            watch,
            raw,
        }
    }

    fn append_watch(&self) {
        let watch_only = self
            .watch
            .events
            .iter()
            .filter(|event| event.metadata["provider_log"] != true)
            .cloned()
            .collect::<Vec<_>>();
        self.archive
            .append_chat_events_with_mock_watch(
                self.context.clone(),
                &watch_only,
                self.watch.watch_mirrors.as_ref(),
            )
            .unwrap();
    }

    fn append_raw(&self, raw: &serde_json::Value) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.snapshot.log_path.as_ref().unwrap())
            .unwrap();
        writeln!(file, "{raw}").unwrap();
    }

    fn acquire(&self) -> ProviderLogBatch {
        let path = self.snapshot.log_path.as_ref().unwrap();
        let key = self.context.provider_source_key.as_deref().unwrap();
        let previous = self
            .archive
            .provider_log_capture_state(&self.context.agent_id, key)
            .unwrap();
        if let Some(policy) =
            provider_log_acquisition::observe_provider_log_policy_with_initial_absence(
                path,
                key,
                previous.clone(),
                true,
                true,
            )
            .unwrap()
        {
            self.archive
                .append_provider_log_batch_with_context(
                    self.context.clone(),
                    &[],
                    previous.as_ref(),
                    &policy.next,
                )
                .unwrap();
        }
        let previous = self
            .archive
            .provider_log_capture_state(&self.context.agent_id, key)
            .unwrap();
        let mut batch = provider_log_acquisition::acquire_provider_log_batch(
            &self.context.agent_id,
            "mock",
            path,
            key,
            previous,
            true,
        )
        .unwrap();
        crate::commands::chat::decorate_forward_provider_log_events(
            &mut batch.events,
            "mock",
            path,
        );
        assert_eq!(
            batch.source_proof().unwrap().mock_provider_session(),
            Some("owned")
        );
        batch
    }

    fn commit(&self, batch: &ProviderLogBatch) -> io::Result<usize> {
        self.archive
            .append_verified_provider_log_batch_with_context(
                self.context.clone(),
                &batch.events,
                batch.previous.as_ref(),
                &batch.next,
                batch.source_proof(),
            )
    }

    fn settled_headers(&self) -> Vec<serde_json::Value> {
        for _ in 0..16 {
            if !self
                .archive
                .chat_projection
                .advance(&self.context.agent_id)
                .unwrap()
            {
                break;
            }
        }
        let reference = chat_read::head(&self.context.agent_id).unwrap().unwrap();
        let (_, objects) = chat_read::locations(&self.context.agent_id).unwrap();
        let mut store = chat_read_store::Store::new(&objects);
        let head: serde_json::Value = store.read(&reference).unwrap();
        assert_eq!(head["progress"], "ready");
        let root: Option<String> = serde_json::from_value(head["root"].clone()).unwrap();
        store
            .page(&root, None, 80)
            .unwrap()
            .into_iter()
            .map(|(_, reference)| store.read(&reference).unwrap())
            .filter(|row: &serde_json::Value| row["event"]["role"] == "assistant")
            .collect()
    }

    fn assert_single_native_owner(&self) {
        let headers = self.settled_headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers[0]["event"]["id"],
            headers[0]["event"]["metadata"]["chat_source_ref"]
        );
        let reference = chat_read::head(&self.context.agent_id).unwrap().unwrap();
        let (_, objects) = chat_read::locations(&self.context.agent_id).unwrap();
        let head: serde_json::Value = chat_read_store::Store::new(&objects)
            .read(&reference)
            .unwrap();
        let directory = conversation_dir(
            &self.context.agent_id,
            head["conversation_id"].as_str().unwrap(),
        )
        .unwrap();
        let events: Vec<AgentChatEvent> =
            read_jsonl_records(&directory.join("events.jsonl")).unwrap();
        let native = events
            .iter()
            .find(|event| event.metadata[NATIVE].is_object())
            .unwrap();
        let watch = events
            .iter()
            .find(|event| event.metadata[WATCH].is_object())
            .unwrap();
        assert!(
            !provenance::same_observation(watch, native),
            "mirror correspondence never relaxes physical source ownership"
        );
        let records: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
        let owners = records
            .iter()
            .filter(|record| record.role.as_deref() == Some("assistant"))
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), 1);
        assert!(
            owners[0].event_refs.contains(&native.id) && owners[0].event_refs.contains(&watch.id)
        );
        let restarted = ConversationArchiveState::default();
        assert!(!restarted.bootstrap_saved_chat(&self.context).unwrap());
        assert_eq!(
            chat_read::head(&self.context.agent_id).unwrap().as_deref(),
            Some(reference.as_str())
        );
        assert!(self.home.path().exists());
    }
}

#[test]
fn actual_acquisition_joins_each_publication_order_and_survives_restart() {
    for watch_first in [false, true] {
        let fixture = Fixture::new();
        if watch_first {
            fixture.append_watch();
        }
        fixture.append_raw(&fixture.raw);
        let batch = fixture.acquire();
        fixture.commit(&batch).unwrap();
        fixture.append_watch();
        fixture.append_watch(); // The later EOF/empty batch must be idempotent.
        fixture.assert_single_native_owner();
    }
}

#[test]
fn failed_cursor_commit_preserves_the_old_watch_head_until_retry() {
    let fixture = Fixture::new();
    fixture.append_watch();
    let before = chat_read::head(&fixture.context.agent_id).unwrap();
    fixture.append_raw(&fixture.raw);
    let batch = fixture.acquire();
    fixture
        .archive
        .fail_next_chat_cursor_commit
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(fixture.commit(&batch).is_err());
    assert_eq!(chat_read::head(&fixture.context.agent_id).unwrap(), before);
    let source = fixture
        .archive
        .provider_log_capture_state(
            &fixture.context.agent_id,
            fixture.context.provider_source_key.as_deref().unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(source.committed_offset < batch.next.committed_offset);
    fixture.commit(&batch).unwrap();
    fixture.append_watch();
    fixture.assert_single_native_owner();
}

#[test]
fn actual_duplicate_receipts_do_not_advance_the_native_cursor() {
    let fixture = Fixture::new();
    fixture.append_watch();
    fixture.append_raw(&fixture.raw);
    fixture.append_raw(&fixture.raw);
    let batch = fixture.acquire();
    let before = chat_read::head(&fixture.context.agent_id).unwrap();
    assert!(fixture
        .commit(&batch)
        .unwrap_err()
        .to_string()
        .contains("ambiguous Mock mirror"));
    assert_eq!(chat_read::head(&fixture.context.agent_id).unwrap(), before);
    let source = fixture
        .archive
        .provider_log_capture_state(
            &fixture.context.agent_id,
            fixture.context.provider_source_key.as_deref().unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(source.committed_offset < batch.next.committed_offset);
}

#[test]
fn actual_conflicting_payload_does_not_collapse_the_watch_observation() {
    let fixture = Fixture::new();
    fixture.append_watch();
    let mut conflicting = fixture.raw.clone();
    conflicting["content"] = json!("Conflicting actual body");
    fixture.append_raw(&conflicting);
    fixture.commit(&fixture.acquire()).unwrap();
    fixture.append_watch();
    assert_eq!(fixture.settled_headers().len(), 2);
}

#[test]
fn interrupted_narrative_reconciliation_recovers_on_append_and_restart() {
    for restart in [false, true] {
        let fixture = Fixture::new();
        fixture.append_raw(&fixture.raw);
        let batch = fixture.acquire();
        fixture
            .archive
            .fail_next_chat_cursor_commit
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(fixture.commit(&batch).is_err());
        // The observation remains independently capturable while the native
        // cursor is uncommitted, creating the two legitimate audit narratives.
        fixture.append_watch();
        let before = chat_read::head(&fixture.context.agent_id).unwrap();
        fixture
            .archive
            .fail_compatibility_stage
            .store(5, std::sync::atomic::Ordering::SeqCst);
        assert!(fixture
            .commit(&batch)
            .unwrap_err()
            .to_string()
            .contains("failure after events"));
        assert_eq!(chat_read::head(&fixture.context.agent_id).unwrap(), before);
        let restarted = restart.then(ConversationArchiveState::default);
        let archive = restarted.as_ref().unwrap_or(&fixture.archive);
        let watch_only = fixture
            .watch
            .events
            .iter()
            .filter(|event| event.metadata["provider_log"] != true)
            .cloned()
            .collect::<Vec<_>>();
        archive
            .append_chat_events_with_mock_watch(
                fixture.context.clone(),
                &watch_only,
                fixture.watch.watch_mirrors.as_ref(),
            )
            .unwrap();
        for _ in 0..16 {
            if !archive
                .chat_projection
                .advance(&fixture.context.agent_id)
                .unwrap()
            {
                break;
            }
        }
        fixture.assert_single_native_owner();
    }
}
