use super::*;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;

const SESSION: &str = "owned-codex-session";
const AGENT: &str = "archive-test-agent";

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
    header_end: u64,
}

impl Fixture {
    fn new(suffix: &[u8]) -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("rollout.jsonl");
        let header = format!(
            "{}\n",
            json!({"type": "session_meta", "payload": {"id": SESSION, "cwd": "fixture"}})
        );
        let mut file = File::create(&path).expect("fixture source");
        file.write_all(header.as_bytes()).expect("header");
        file.write_all(suffix).expect("records");
        Self {
            _directory: directory,
            path,
            header_end: header.len() as u64,
        }
    }

    fn eof(&self) -> u64 {
        fs::metadata(&self.path).expect("fixture extent").len()
    }

    fn select(&self) -> Selection {
        self.select_to(self.eof())
    }

    fn select_to(&self, end: u64) -> Selection {
        prepare(
            &self.path,
            SESSION,
            self.header_end,
            end,
            &self.anchor(self.header_end),
        )
        .expect("prepare fixture")
    }

    fn anchor(&self, start: u64) -> ProviderLogContinuityAnchor {
        let anchor_start = start.saturating_sub(4096);
        let bytes = read_exact_range(
            &mut File::open(&self.path).expect("fixture file"),
            anchor_start,
            start - anchor_start,
        )
        .expect("anchor bytes");
        ProviderLogContinuityAnchor {
            start: anchor_start,
            len: start - anchor_start,
            sha256: digest(&bytes),
        }
    }

    fn append(&self, bytes: &[u8]) {
        OpenOptions::new()
            .append(true)
            .open(&self.path)
            .expect("append fixture")
            .write_all(bytes)
            .expect("appended records");
    }

    fn rewrite(&self, offset: u64, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .expect("rewrite fixture");
        file.seek(SeekFrom::Start(offset)).expect("rewrite offset");
        file.write_all(bytes).expect("rewrite bytes");
    }
}

fn assistant(text: &str) -> String {
    format!(
        "{}\n",
        json!({"type":"event_msg", "payload":{"type":"agent_message", "message":text}})
    )
}

fn ignored_record(len: usize) -> String {
    let prefix = "{\"type\":\"ignored\",\"padding\":\"";
    let suffix = "\"}\n";
    assert!(len >= prefix.len() + suffix.len());
    format!(
        "{prefix}{}{suffix}",
        "x".repeat(len - prefix.len() - suffix.len())
    )
}

fn first(selection: &Selection) -> io::Result<Batch> {
    let batch = advance(
        selection,
        AGENT,
        SESSION,
        selection.start,
        &TranscriptNormalizationState::default(),
    )?;
    assert_batch_source_scope(selection, AGENT, &batch);
    Ok(batch)
}

fn assert_batch_source_scope(selection: &Selection, agent: &str, batch: &Batch) {
    for event in batch.events.iter().chain(batch.normalizer.pending_events()) {
        assert_eq!(event.session_id, agent);
        assert_eq!(event.provider, "codex");
        assert_eq!(event.metadata["provider_log"], true);
        assert_eq!(
            event.metadata["log_path"].as_str(),
            Some(selection.path.as_str())
        );
        assert_eq!(event.metadata["provider_session_id"], SESSION);
        let provenance = &event.metadata[PROVENANCE_KEY];
        assert_eq!(provenance["agent_id"], agent);
        assert_eq!(provenance["session_id"], SESSION);
        assert_eq!(
            provenance["native_identity"],
            serde_json::to_value(&selection.native_identity).expect("native identity")
        );
    }
}

fn collect(selection: &Selection) -> Vec<AgentChatEvent> {
    let mut cursor = selection.start;
    let mut normalizer = TranscriptNormalizationState::default();
    let mut events = Vec::new();
    while cursor < selection.end {
        let batch = advance(selection, AGENT, SESSION, cursor, &normalizer).expect("advance");
        assert_batch_source_scope(selection, AGENT, &batch);
        assert!(batch.next_cursor > cursor);
        assert!(batch.next_cursor <= selection.end);
        cursor = batch.next_cursor;
        normalizer = batch.normalizer;
        events.extend(batch.events);
    }
    events
}

#[test]
fn pins_canonical_path_actual_identity_and_round_trips_selection() {
    let fixture = Fixture::new(assistant("body").as_bytes());
    let selection = fixture.select();
    assert_eq!(
        Path::new(&selection.path),
        fixture.path.canonicalize().expect("canonical path")
    );
    assert_eq!(
        selection.native_identity,
        native_file_identity(&File::open(&fixture.path).expect("file")).expect("identity")
    );
    assert_eq!(selection.end, fixture.eof());
    let restored: Selection =
        serde_json::from_slice(&serde_json::to_vec(&selection).expect("serialize"))
            .expect("deserialize");
    assert_eq!(restored, selection);
    validate(&restored, SESSION).expect("validate restored selection");
}

#[test]
fn numeric_end_captured_before_append_never_expands_at_issuance_or_replay() {
    let fixture = Fixture::new(assistant("authorized").as_bytes());
    let numeric_end = fixture.eof();
    fixture.append(assistant("outside operator preparation").as_bytes());
    let selection = fixture.select_to(numeric_end);
    assert_eq!(selection.end, numeric_end);
    assert_eq!(selection.observed_eof, fixture.eof());
    fixture.append(b"malformed complete record outside grant\n");
    validate(&selection, SESSION).expect("append is allowed");
    let events = collect(&selection);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some("authorized"));
}

#[test]
fn partial_json_and_valid_json_without_newline_are_excluded_at_numeric_end() {
    for suffix in [
        b"{\"type\":\"event_msg\"".as_slice(),
        b"{\"type\":\"ignored\"}",
        &[0xff, 0xfe],
    ] {
        let fixture = Fixture::new(assistant("complete").as_bytes());
        let complete_end = fixture.eof();
        fixture.append(suffix);
        let numeric_end = fixture.eof();
        fixture.append(b"\n");
        let selection = fixture.select_to(numeric_end);
        assert_eq!(selection.end, complete_end);
        assert_eq!(collect(&selection).len(), 1);
    }
}

#[test]
fn rejects_malformed_invalid_utf8_blank_and_untyped_complete_records() {
    for suffix in [
        b"{bad}\n".as_slice(),
        &[0xff, b'\n'],
        b"\n",
        b"[]\n",
        b"{}\n",
    ] {
        let fixture = Fixture::new(suffix);
        assert!(prepare(
            &fixture.path,
            SESSION,
            fixture.header_end,
            fixture.eof(),
            &fixture.anchor(fixture.header_end)
        )
        .is_err());
    }
}

#[test]
fn accepts_crlf_and_excludes_an_end_inside_a_record() {
    let record = assistant("complete").replace('\n', "\r\n");
    let fixture = Fixture::new(record.as_bytes());
    let selection = fixture.select();
    assert_eq!(
        first(&selection).expect("CRLF record").events[0]
            .text
            .as_deref(),
        Some("complete")
    );
    assert_eq!(fixture.select_to(fixture.eof() - 1).end, fixture.header_end);
}

#[test]
fn rejects_wrong_session_header_anchor_and_unframed_start() {
    let fixture = Fixture::new(assistant("body").as_bytes());
    assert!(prepare(
        &fixture.path,
        "different-session",
        fixture.header_end,
        fixture.eof(),
        &fixture.anchor(fixture.header_end)
    )
    .is_err());
    assert!(validate(&fixture.select(), "different-session").is_err());
    let mut anchor = fixture.anchor(fixture.header_end);
    anchor.sha256 = "0".repeat(64);
    assert!(prepare(
        &fixture.path,
        SESSION,
        fixture.header_end,
        fixture.eof(),
        &anchor
    )
    .is_err());
    anchor.len = u64::MAX;
    assert!(prepare(
        &fixture.path,
        SESSION,
        fixture.header_end,
        fixture.eof(),
        &anchor
    )
    .is_err());
    let start = fixture.header_end + 1;
    assert!(prepare(
        &fixture.path,
        SESSION,
        start,
        fixture.eof(),
        &fixture.anchor(start)
    )
    .is_err());
    fixture.rewrite(0, b"{\"type\":\"not_session!");
    assert!(prepare(&fixture.path, SESSION, 0, fixture.eof(), &fixture.anchor(0)).is_err());
}

#[test]
fn rejects_requested_end_past_eof_and_backwards_interval() {
    let fixture = Fixture::new(b"");
    assert!(prepare(
        &fixture.path,
        SESSION,
        fixture.header_end,
        fixture.eof() + 1,
        &fixture.anchor(fixture.header_end)
    )
    .is_err());
    assert!(prepare(
        &fixture.path,
        SESSION,
        fixture.header_end,
        fixture.header_end - 1,
        &fixture.anchor(fixture.header_end)
    )
    .is_err());
}

#[test]
fn empty_interval_at_record_boundary_is_a_no_op() {
    let fixture = Fixture::new(b"");
    let selection = fixture.select();
    assert_eq!(selection.start, selection.end);
    let batch = first(&selection).expect("empty batch");
    assert!(batch.events.is_empty());
    assert_eq!(batch.next_cursor, selection.start);
    assert_eq!(batch.normalizer, TranscriptNormalizationState::default());
}

#[test]
fn rejects_same_path_replacement_even_with_identical_bytes() {
    let fixture = Fixture::new(assistant("body").as_bytes());
    let selection = fixture.select();
    let retained = fixture.path.with_extension("retained");
    fs::rename(&fixture.path, &retained).expect("retain original inode");
    fs::copy(&retained, &fixture.path).expect("replacement inode");
    assert!(validate(&selection, SESSION).is_err());
    assert!(first(&selection).is_err());
}

#[test]
fn rejects_truncation_even_when_only_unselected_observed_suffix_is_lost() {
    let fixture = Fixture::new(assistant("body").as_bytes());
    let numeric_end = fixture.eof();
    fixture.append(b"partial");
    let selection = fixture.select_to(numeric_end);
    OpenOptions::new()
        .write(true)
        .open(&fixture.path)
        .expect("file")
        .set_len(numeric_end)
        .expect("truncate observed suffix");
    assert!(validate(&selection, SESSION).is_err());
}

#[test]
fn detects_header_and_old_anchor_rewrites_without_relying_on_file_timestamps() {
    let fixture = Fixture::new(assistant("body").as_bytes());
    let selection = fixture.select();
    let identity = selection.native_identity.clone();
    fixture.rewrite(fixture.header_end - 4, b" ");
    assert_eq!(
        native_file_identity(&File::open(&fixture.path).expect("file")).expect("identity"),
        identity
    );
    assert!(validate(&selection, SESSION).is_err());

    let fixture = Fixture::new(ignored_record(5000).as_bytes());
    let start = fixture.eof();
    fixture.append(assistant("body").as_bytes());
    let selection = prepare(
        &fixture.path,
        SESSION,
        start,
        fixture.eof(),
        &fixture.anchor(start),
    )
    .expect("selection");
    fixture.rewrite(start - 5, b"y");
    assert!(validate(&selection, SESSION).is_err());
}

#[test]
fn verifies_each_chunk_before_normalization_and_rejects_same_identity_rewrites() {
    let fixture = Fixture::new(ignored_record(PROVIDER_LOG_BATCH_BYTES as usize).as_bytes());
    fixture.append(assistant("original").as_bytes());
    let selection = fixture.select();
    assert_eq!(selection.chunks.len(), 2);
    let batch = first(&selection).expect("unchanged first chunk");
    let second_start = batch.next_cursor;
    let original = assistant("original");
    let text_offset = original.find("original").expect("body offset") as u64;
    fixture.rewrite(second_start + text_offset, b"rewritten");
    assert_eq!(
        native_file_identity(&File::open(&fixture.path).expect("file")).expect("identity"),
        selection.native_identity
    );
    validate(&selection, SESSION).expect("bounded validation does not reread distant chunks");
    let before = batch.normalizer.clone();
    assert!(advance(&selection, AGENT, SESSION, second_start, &batch.normalizer).is_err());
    assert_eq!(batch.normalizer, before);
}

#[test]
fn equal_text_at_distinct_physical_records_has_distinct_retry_stable_ids() {
    let record = assistant("equal text");
    let fixture = Fixture::new(format!("{record}{record}").as_bytes());
    let selection = fixture.select();
    let batch = first(&selection).expect("first pass");
    let retry = first(&selection).expect("retry");
    assert_eq!(batch.events, retry.events);
    assert_eq!(batch.normalizer, retry.normalizer);
    assert_eq!(batch.events.len(), 2);
    assert_ne!(batch.events[0].id, batch.events[1].id);
    assert_eq!(batch.events[0].text, batch.events[1].text);
    let other_agent = advance(
        &selection,
        "other-agent",
        SESSION,
        selection.start,
        &TranscriptNormalizationState::default(),
    )
    .expect("other agent");
    assert_batch_source_scope(&selection, "other-agent", &other_agent);
    assert_ne!(batch.events[0].id, other_agent.events[0].id);
    // Different selection boundaries/pass numbers do not change the row ID.
    let second_start = fixture.header_end + record.len() as u64;
    let second = prepare(
        &fixture.path,
        SESSION,
        second_start,
        fixture.eof(),
        &fixture.anchor(second_start),
    )
    .expect("second selection");
    assert_eq!(
        first(&second).expect("second row").events[0].id,
        batch.events[1].id
    );
}

#[test]
fn normal_iteration_is_bounded_and_returns_full_bodies_owned_by_the_caller() {
    let text = "long body ".repeat(6000);
    let record = assistant(&text);
    let fixture = Fixture::new(record.repeat(12).as_bytes());
    let selection = fixture.select();
    assert!(selection.chunks.len() >= 3);
    assert!(selection
        .chunks
        .iter()
        .all(|chunk| chunk.end - chunk.start <= PROVIDER_LOG_BATCH_BYTES));
    let events = collect(&selection);
    assert_eq!(events.len(), 12);
    assert!(events
        .iter()
        .all(|event| event.text.as_deref() == Some(text.trim())));
    assert_eq!(
        events
            .iter()
            .map(|event| &event.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        12
    );
}

#[test]
fn exceptional_record_pass_preserves_a_body_larger_than_the_normal_budget() {
    let text = "a".repeat(PROVIDER_LOG_BATCH_BYTES as usize + 200);
    let fixture = Fixture::new(assistant(&text).as_bytes());
    let selection = fixture.select();
    let batch = first(&selection).expect("exceptional pass");
    assert_eq!(batch.next_cursor, selection.end);
    assert_eq!(batch.events[0].text.as_deref(), Some(text.as_str()));
}

#[test]
fn sixteen_mib_record_bound_includes_newline_and_oversized_partial_is_excluded() {
    let fixture = Fixture::new(ignored_record(PROVIDER_LOG_MAX_RECORD_BYTES as usize).as_bytes());
    let selection = fixture.select();
    assert_eq!(
        selection.end - selection.start,
        PROVIDER_LOG_MAX_RECORD_BYTES
    );
    first(&selection).expect("exact exceptional bound");

    let fixture =
        Fixture::new(ignored_record(PROVIDER_LOG_MAX_RECORD_BYTES as usize + 1).as_bytes());
    assert!(prepare(
        &fixture.path,
        SESSION,
        fixture.header_end,
        fixture.eof(),
        &fixture.anchor(fixture.header_end)
    )
    .is_err());

    let partial = "x".repeat(PROVIDER_LOG_MAX_RECORD_BYTES as usize + 1);
    let fixture = Fixture::new(partial.as_bytes());
    assert_eq!(fixture.select().end, fixture.header_end);
}

fn delayed_fixture() -> Fixture {
    let turn = "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"turn-a\"}}\n";
    let context = format!(
        "{}\n",
        json!({"type":"response_item", "payload":{
            "type":"message", "id":"context-a", "role":"user",
            "content":[{"type":"input_text", "text":"host context"}],
            "internal_chat_message_metadata_passthrough":{"turn_id":"turn-a", "content_item_kinds":["agents_md.instructions"]}
        }})
    );
    let answer = assistant("deferred answer");
    let mut suffix = format!("{turn}{context}{answer}");
    suffix.push_str(&ignored_record(
        PROVIDER_LOG_BATCH_BYTES as usize - suffix.len(),
    ));
    let fixture = Fixture::new(suffix.as_bytes());
    fixture.append(format!("{}\n", json!({"type":"response_item", "payload":{
        "type":"message", "id":"request-a", "role":"user",
        "content":[{"type":"input_text", "text":"user request"}],
        "internal_chat_message_metadata_passthrough":{"turn_id":"turn-a", "content_item_kinds":["user.text"]}
    }})).as_bytes());
    fixture
}

#[test]
fn deferred_outputs_keep_original_ids_coordinates_and_bodies_after_serialized_retry() {
    let fixture = delayed_fixture();
    let selection = fixture.select();
    let pending = first(&selection).expect("pending pass");
    assert!(pending.events.is_empty());
    assert_eq!(pending.normalizer.pending_events().len(), 2);
    let original = pending.normalizer.pending_events().to_vec();
    let restored: TranscriptNormalizationState =
        serde_json::from_slice(&serde_json::to_vec(&pending.normalizer).expect("serialize"))
            .expect("restore");
    let batch =
        advance(&selection, AGENT, SESSION, pending.next_cursor, &restored).expect("release");
    let retry =
        advance(&selection, AGENT, SESSION, pending.next_cursor, &restored).expect("retry release");
    assert_batch_source_scope(&selection, AGENT, &batch);
    assert_batch_source_scope(&selection, AGENT, &retry);
    assert_eq!(batch.events, retry.events);
    assert_eq!(batch.normalizer, retry.normalizer);
    assert_eq!(batch.events.len(), 3);
    for (released, prior) in batch.events.iter().zip(&original) {
        assert_eq!(released.id, prior.id);
        assert_eq!(released.text, prior.text);
        assert_eq!(
            released.metadata[PROVENANCE_KEY],
            prior.metadata[PROVENANCE_KEY]
        );
        assert!(
            released.metadata[PROVENANCE_KEY]["record_end"]
                .as_u64()
                .expect("end")
                <= pending.next_cursor
        );
    }
}

#[test]
fn rejects_unknown_foreign_or_corrupted_pending_provenance() {
    let fixture = delayed_fixture();
    let selection = fixture.select();
    let pending = first(&selection).expect("pending");
    for mutation in [
        "missing",
        "agent",
        "wardian_session",
        "provider_log",
        "log_path",
        "provider_session",
        "chunk",
        "coordinate",
        "digest",
        "id",
        "version",
    ] {
        let mut state = pending.normalizer.clone();
        let event = &mut state.pending_events_mut()[0];
        match mutation {
            "missing" => event
                .metadata
                .as_object_mut()
                .expect("metadata")
                .remove(PROVENANCE_KEY)
                .map(|_| ())
                .unwrap_or(()),
            "agent" => event.metadata[PROVENANCE_KEY]["agent_id"] = json!("foreign"),
            "wardian_session" => event.session_id = SESSION.to_owned(),
            "provider_log" => event.metadata["provider_log"] = json!(false),
            "log_path" => event.metadata["log_path"] = json!("foreign-source.jsonl"),
            "provider_session" => event.metadata["provider_session_id"] = json!("foreign-session"),
            "chunk" => event.metadata[PROVENANCE_KEY]["chunk_sha256"] = json!("0".repeat(64)),
            "coordinate" => event.metadata[PROVENANCE_KEY]["record_start"] = json!(selection.end),
            "digest" => event.metadata[PROVENANCE_KEY]["record_sha256"] = json!("0".repeat(64)),
            "id" => event.id = "unknown".into(),
            "version" => event.metadata[PROVENANCE_KEY]["normalizer_version"] = json!(999),
            _ => unreachable!(),
        }
        assert!(
            advance(&selection, AGENT, SESSION, pending.next_cursor, &state).is_err(),
            "accepted {mutation}"
        );
    }
}

#[test]
fn drains_pending_physical_events_at_pinned_end_without_a_later_request_or_completion() {
    let fixture = delayed_fixture();
    let numeric_end = fixture.header_end + PROVIDER_LOG_BATCH_BYTES;
    OpenOptions::new()
        .write(true)
        .open(&fixture.path)
        .expect("fixture source")
        .set_len(numeric_end)
        .expect("remove the later request from the fixture");
    let selection = fixture.select_to(numeric_end);
    let pending = first(&selection).expect("pending at end");
    assert!(pending.events.is_empty());
    assert!(pending.normalizer.has_pending_events());
    let original = pending.normalizer.pending_events().to_vec();
    let restored: TranscriptNormalizationState = serde_json::from_slice(
        &serde_json::to_vec(&pending.normalizer).expect("serialize pending"),
    )
    .expect("restore pending at fixed end");
    let end = advance(&selection, AGENT, SESSION, pending.next_cursor, &restored)
        .expect("drain verified physical pending rows at end");
    assert_batch_source_scope(&selection, AGENT, &end);
    assert_eq!(end.events.len(), 2);
    assert!(!end.normalizer.has_pending_events());
    assert_eq!(end.normalizer.next_sequence(), restored.next_sequence());
    assert_eq!(end.next_cursor, numeric_end);
    assert_eq!(end.events[0].role, Some(AgentChatRole::System));
    assert_eq!(end.events[0].metadata["input_origin"], "context_injection");
    assert_eq!(end.events[1].role, Some(AgentChatRole::Assistant));
    for (released, prior) in end.events.iter().zip(&original) {
        assert_eq!(released.id, prior.id);
        assert_eq!(released.text, prior.text);
        assert_eq!(released.kind, prior.kind);
        assert_eq!(released.sequence, prior.sequence);
        assert_eq!(released.metadata, prior.metadata);
        assert!(released.metadata.get("request_root_id").is_none());
        assert!(
            released.metadata[PROVENANCE_KEY]["record_end"]
                .as_u64()
                .expect("physical end")
                <= numeric_end
        );
    }
    let retry = advance(&selection, AGENT, SESSION, numeric_end, &restored).expect("retry drain");
    assert_batch_source_scope(&selection, AGENT, &retry);
    assert_eq!(retry.events, end.events);
    assert_eq!(retry.normalizer, end.normalizer);
    assert_eq!(restored, pending.normalizer);

    let settled = advance(&selection, AGENT, SESSION, numeric_end, &end.normalizer)
        .expect("already drained end is a no-op");
    assert!(settled.events.is_empty());
    assert_eq!(settled.normalizer, end.normalizer);
    assert_eq!(settled.next_cursor, numeric_end);
}

#[test]
fn rejects_unknown_pending_provenance_before_the_eof_drain() {
    let fixture = delayed_fixture();
    let selection = fixture.select_to(fixture.header_end + PROVIDER_LOG_BATCH_BYTES);
    let pending = first(&selection).expect("pending physical rows");
    let mut unknown = pending.normalizer.clone();
    unknown.pending_events_mut()[0]
        .metadata
        .as_object_mut()
        .expect("metadata")
        .remove(PROVENANCE_KEY);
    let before = unknown.clone();
    assert!(advance(&selection, AGENT, SESSION, selection.end, &unknown).is_err());
    assert_eq!(unknown, before);
}

#[test]
fn rejects_cursor_skips_and_prior_normalizer_context() {
    let fixture = delayed_fixture();
    let selection = fixture.select();
    let state = TranscriptNormalizationState::default();
    assert!(advance(&selection, AGENT, SESSION, selection.start + 1, &state).is_err());
    assert!(advance(&selection, AGENT, SESSION, selection.end, &state).is_err());
    let prior = first(&selection).expect("prior context");
    assert!(advance(
        &selection,
        AGENT,
        SESSION,
        selection.start,
        &prior.normalizer
    )
    .is_err());
}

#[test]
fn physical_metadata_preserves_kind_turn_message_time_without_raw_json_or_authority() {
    let body = "owned body ".repeat(5000);
    let raw = format!(
        "{}\n",
        json!({
            "type":"response_item", "turn_id":"outer-turn", "timestamp":"2026-11-14T01:02:03Z",
            "payload":{"type":"message", "id":"native-message", "role":"assistant", "content":[{"type":"output_text", "text":body}],
                "internal_chat_message_metadata_passthrough":{"turn_id":"inner-turn"}},
            "chat_source_ref":"untrusted", "metadata":{"chat_source_epoch":"untrusted"}
        })
    );
    let fixture = Fixture::new(raw.as_bytes());
    let event = first(&fixture.select())
        .expect("normalized")
        .events
        .remove(0);
    let provenance = &event.metadata[PROVENANCE_KEY];
    assert_eq!(provenance["provider_record"]["kind"], "response_item");
    assert_eq!(provenance["provider_record"]["payload_kind"], "message");
    assert_eq!(provenance["provider_record"]["turn_id"], "outer-turn");
    assert_eq!(
        provenance["provider_record"]["metadata_turn_id"],
        "inner-turn"
    );
    assert_eq!(
        provenance["provider_record"]["payload_message_id"],
        "native-message"
    );
    assert_eq!(
        provenance["provider_record"]["timestamp"],
        "2026-11-14T01:02:03Z"
    );
    assert!(serde_json::to_vec(provenance).expect("provenance").len() < 2048);
    assert_eq!(event.text.as_deref(), Some(body.trim()));
    for key in [
        "chat_source_ref",
        "chat_source_epoch",
        "chat_source_start",
        "chat_source_end",
        "recent_admission",
        "chat_native_observation",
        "event_aliases",
        "_wardian_provider_raw_line",
    ] {
        assert!(event.metadata.get(key).is_none(), "fabricated {key}");
    }
}

#[test]
fn receipts_scale_with_chunks_and_fit_the_audit_budget_for_eighty_four_mib() {
    let fixture = Fixture::new(assistant("body").as_bytes());
    let mut selection = fixture.select();
    selection.chunks = (0..340)
        .map(|index| ChunkWitness {
            start: fixture.header_end + index * PROVIDER_LOG_BATCH_BYTES,
            end: fixture.header_end + (index + 1) * PROVIDER_LOG_BATCH_BYTES,
            sha256: "a".repeat(64),
            first_sequence: 1 + index * 142,
        })
        .collect();
    let encoded = serde_json::to_vec(&selection).expect("compact selection");
    // Leave room for the surrounding immutable grant (request, actor, policy).
    assert!(encoded.len() < 60 * 1024, "{} bytes", encoded.len());
    let value: Value = serde_json::from_slice(&encoded).expect("selection JSON");
    assert!(value["chunks"]
        .as_array()
        .expect("chunks")
        .iter()
        .all(|chunk| chunk.as_object().expect("receipt").len() == 4));
}
