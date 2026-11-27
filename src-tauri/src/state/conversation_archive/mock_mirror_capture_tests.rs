use super::*;
use crate::commands::chat::{AgentArchiveCaptureSnapshot, ArchiveCaptureResult};
use crate::commands::provider_log_acquisition::{self, ProviderLogBatch};
use std::{io::Write, path::PathBuf};
use wardian_core::models::chat::AgentChatPage;

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

    fn page(&self, archive: &ConversationArchiveState) -> AgentChatPage {
        self.read_page(archive, 80, None)
    }

    fn read_page(
        &self,
        archive: &ConversationArchiveState,
        limit: usize,
        before: Option<String>,
    ) -> AgentChatPage {
        let read = archive
            .archive_chat_reader(
                self.context.clone(),
                super::super::archive_chat::Request {
                    before,
                    revision: None,
                    detail: None,
                    limit,
                    logging_enabled: true,
                },
            )
            .unwrap();
        read().unwrap()
    }

    fn archive_directory(&self) -> PathBuf {
        let conversation = self
            .archive
            .active_conversation_id(&self.context.agent_id)
            .unwrap()
            .unwrap();
        conversation_dir(&self.context.agent_id, &conversation).unwrap()
    }

    fn assert_single_native_owner(&self, archive: &ConversationArchiveState) {
        let page = self.page(archive);
        assert_eq!(page.progress, "ready");
        let conversation = page.conversation_id.as_deref().unwrap();
        let directory = conversation_dir(&self.context.agent_id, conversation).unwrap();
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
        let visible = page
            .events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(visible, [native.id.as_str()]);
        assert!(page
            .aliases
            .iter()
            .any(|alias| { alias.observation_id == watch.id && alias.canonical_id == native.id }));
        let restarted = ConversationArchiveState::default();
        let cold = self.page(&restarted);
        assert_eq!(
            cold.events
                .iter()
                .map(|event| &event.id)
                .collect::<Vec<_>>(),
            page.events
                .iter()
                .map(|event| &event.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            serde_json::to_value(&cold.aliases).unwrap(),
            serde_json::to_value(&page.aliases).unwrap()
        );
        let (head, objects) = chat_read::locations(&self.context.agent_id).unwrap();
        assert!(!head.exists() && !objects.exists());
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
        fixture.assert_single_native_owner(&fixture.archive);
    }
}

#[test]
fn actual_archive_older_advances_past_both_mirror_frames() {
    let fixture = Fixture::new();
    let mut older = fixture
        .watch
        .events
        .iter()
        .find(|event| event.role == Some(AgentChatRole::Assistant))
        .unwrap()
        .clone();
    older.id = "earlier-owned-assistant".into();
    older.text = Some("Earlier unrelated body".into());
    older.turn_id = Some("earlier-turn".into());
    older.metadata = json!({});
    fixture
        .archive
        .append_chat_events_with_context(fixture.context.clone(), &[older])
        .unwrap();
    fixture.append_watch();
    fixture.append_raw(&fixture.raw);
    fixture.commit(&fixture.acquire()).unwrap();

    let recent = fixture.read_page(&fixture.archive, 1, None);
    assert_eq!(recent.events.len(), 1);
    assert_eq!(recent.aliases.len(), 1);
    assert_eq!(recent.events[0].id, recent.aliases[0].canonical_id);
    let before = recent.next_before.expect("older archived page");
    let older = fixture.read_page(&ConversationArchiveState::default(), 1, Some(before));
    assert!(older.older_page && !older.reset);
    assert_eq!(older.events[0].id, "earlier-owned-assistant");
    assert!(older.next_before.is_none());
}

#[test]
fn failed_cursor_commit_keeps_the_watch_frame_until_native_retry() {
    let fixture = Fixture::new();
    fixture.append_watch();
    let journal = fixture.archive_directory().join("events.jsonl");
    let before: Vec<AgentChatEvent> = read_jsonl_records(&journal).unwrap();
    let watch = before
        .iter()
        .find(|event| event.metadata[WATCH].is_object())
        .unwrap();
    fixture.append_raw(&fixture.raw);
    let batch = fixture.acquire();
    fixture
        .archive
        .fail_next_chat_cursor_commit
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(fixture.commit(&batch).is_err());
    let interrupted: Vec<AgentChatEvent> = read_jsonl_records(&journal).unwrap();
    assert_eq!(
        interrupted.iter().find(|event| event.id == watch.id),
        Some(watch)
    );
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
    fixture.assert_single_native_owner(&fixture.archive);
}

#[test]
fn actual_duplicate_receipts_do_not_advance_the_native_cursor() {
    let fixture = Fixture::new();
    fixture.append_watch();
    fixture.append_raw(&fixture.raw);
    fixture.append_raw(&fixture.raw);
    let batch = fixture.acquire();
    let journals =
        ["events.jsonl", "conversation.jsonl"].map(|name| fixture.archive_directory().join(name));
    let before = journals.each_ref().map(|path| std::fs::read(path).unwrap());
    assert!(fixture
        .commit(&batch)
        .unwrap_err()
        .to_string()
        .contains("ambiguous Mock mirror"));
    assert_eq!(
        journals.each_ref().map(|path| std::fs::read(path).unwrap()),
        before
    );
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
    let page = fixture.page(&fixture.archive);
    assert_eq!(
        page.events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .count(),
        2
    );
    assert!(page.aliases.is_empty());
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
        let narrative = fixture.archive_directory().join("conversation.jsonl");
        let before = std::fs::read(&narrative).unwrap();
        fixture
            .archive
            .fail_compatibility_stage
            .store(5, std::sync::atomic::Ordering::SeqCst);
        assert!(fixture
            .commit(&batch)
            .unwrap_err()
            .to_string()
            .contains("failure after events"));
        assert_eq!(std::fs::read(&narrative).unwrap(), before);
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
        fixture.assert_single_native_owner(archive);
    }
}
