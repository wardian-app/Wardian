//! Public model types for agent-owned conversation archives.

use crate::atomic_file::{replace_file, tmp_path_for};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::Path,
};

pub const CONVERSATION_SCHEMA: u8 = 1;
pub const CONVERSATION_TURNS_SCHEMA: u8 = 3;
pub const CONVERSATION_INLINE_TEXT_LIMIT_BYTES: usize = 8 * 1024;

const JSONL_WRITE_BUFFER_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLoggingSetting {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentConversationLoggingSetting {
    Default,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStatus {
    Open,
    Closed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationBoundaryReason {
    Spawn,
    ProviderSourceChanged,
    Clear,
    WorktreeSwitch,
    LoggingEnabled,
    Shutdown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationRecordKind {
    Message,
    ToolCall,
    ToolResult,
    Approval,
    Error,
    Lifecycle,
    Status,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSpeakerType {
    User,
    Assistant,
    Agent,
    Tool,
    System,
    Unknown,
}

/// The normalized provenance of a message-shaped input record.
///
/// This is deliberately independent from the provider's display role. Some
/// providers serialize injected context as a `user` message, even though it
/// must not start a request-indexed turn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationInputOrigin {
    HumanInput,
    AgentInput,
    ContextInjection,
    ProviderInternal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationFormatVersions {
    pub manifest: u8,
    pub conversation: u8,
    pub events: u8,
    pub sources: u8,
    #[serde(default = "default_turns_format_version")]
    pub turns: u8,
}

impl Default for ConversationFormatVersions {
    fn default() -> Self {
        Self {
            manifest: 1,
            conversation: 1,
            events: 1,
            sources: 1,
            turns: CONVERSATION_TURNS_SCHEMA,
        }
    }
}

fn default_turns_format_version() -> u8 {
    1
}

fn default_status_source() -> String {
    "unknown".to_string()
}

fn deserialize_status_source<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_status_source))
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTurnStatus {
    InProgress,
    PendingResponse,
    Responded,
    Interrupted,
    Lifecycle,
    ContextOnly,
    Superseded,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationProviderNativeRef {
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub source_kind: String,
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnProviderRef {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationManifest {
    pub schema: u8,
    pub conversation_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub agent_class: String,
    pub workspace: String,
    pub provider: String,
    pub provider_session_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_source_key: Option<String>,
    #[serde(default)]
    pub provider_native_refs: Vec<ConversationProviderNativeRef>,
    pub effective_logging: ConversationLoggingSetting,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub status: ConversationStatus,
    pub boundary_reason: ConversationBoundaryReason,
    pub format_versions: ConversationFormatVersions,
    #[serde(default)]
    pub record_count: u64,
    #[serde(default)]
    pub turn_count: u64,
    #[serde(default)]
    pub has_turns: bool,
    #[serde(default)]
    pub lifecycle_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationNarrativeRecord {
    pub schema: u8,
    pub seq: u64,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub at: String,
    pub kind: ConversationRecordKind,
    pub role: Option<String>,
    pub speaker_type: Option<ConversationSpeakerType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_origin: Option<ConversationInputOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_root_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal_ref: Option<String>,
    pub text: Option<String>,
    pub tool: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub excerpt: Option<String>,
    pub event_refs: Vec<String>,
    pub source_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnFailureSignal {
    pub kind: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnRequest {
    pub seq: u64,
    pub kind: String,
    pub text: Option<String>,
    pub text_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_text_truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnAssistantResult {
    pub seq: u64,
    pub text: Option<String>,
    pub text_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnCounts {
    pub records: u64,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    pub nonzero_tool_results: u64,
    pub failed_tool_results: u64,
    pub timeouts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnFiles {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub mentioned: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnSideEffect {
    pub kind: String,
    pub evidence_seq: u64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnRecordRefs {
    #[serde(alias = "conversation_seq_start")]
    pub seq_start: u64,
    #[serde(alias = "conversation_seq_end")]
    pub seq_end: u64,
    #[serde(default)]
    pub event_count: u64,
    #[serde(default)]
    pub source_count: u64,
    #[serde(default)]
    pub artifact_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTurnRecord {
    pub schema: u8,
    pub conversation_id: String,
    pub turn_index: u64,
    pub turn_key: String,
    pub status: ConversationTurnStatus,
    #[serde(
        default = "default_status_source",
        deserialize_with = "deserialize_status_source"
    )]
    pub status_source: String,
    pub seq_start: u64,
    pub seq_end: u64,
    pub started_at: String,
    pub updated_at: String,
    pub request: ConversationTurnRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_result: Option<ConversationTurnAssistantResult>,
    pub counts: ConversationTurnCounts,
    pub tools_used: BTreeMap<String, u64>,
    pub files: ConversationTurnFiles,
    pub external_side_effects: Vec<ConversationTurnSideEffect>,
    pub failure_signals: Vec<ConversationTurnFailureSignal>,
    pub record_refs: ConversationTurnRecordRefs,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_refs: Vec<ConversationTurnProviderRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationSourceRecord {
    pub schema: u8,
    pub source_id: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub source_kind: String,
    pub source_path: Option<String>,
    pub cursor: Option<String>,
    pub offset: Option<u64>,
    pub row_id: Option<String>,
    pub provider_event_type: Option<String>,
    pub hash: Option<String>,
    pub artifact_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationIndexEntry {
    pub schema: u8,
    pub conversation_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub agent_class: String,
    pub workspace: String,
    pub provider: String,
    pub provider_session_ids: Vec<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: ConversationStatus,
    pub boundary_reason: ConversationBoundaryReason,
    pub first_prompt_excerpt: Option<String>,
    pub last_record_excerpt: Option<String>,
    pub record_count: u64,
    #[serde(default)]
    pub turn_count: u64,
    #[serde(default)]
    pub has_turns: bool,
    #[serde(default)]
    pub lifecycle_only: bool,
    pub artifact_count: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MaterializedTextPayload {
    pub text: Option<String>,
    pub excerpt: Option<String>,
    pub artifact_refs: Vec<String>,
}

/// Appends one complete encoded row, then flushes without adding an fsync.
/// The destination is opened before serialization so filesystem errors retain
/// precedence. Only this record is buffered, including its terminating newline.
pub fn append_jsonl_record<T: Serialize>(path: &Path, record: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    append_jsonl_to_writer(&mut file, record)
}

fn append_jsonl_to_writer<T: Serialize>(writer: &mut impl Write, record: &T) -> io::Result<()> {
    let mut row = serde_json::to_vec(record).map_err(io::Error::other)?;
    row.push(b'\n');
    writer.write_all(&row)?;
    writer.flush()
}

pub fn read_jsonl_records<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        records.push(serde_json::from_str(&line).map_err(io::Error::other)?);
    }
    Ok(records)
}

/// Reads JSONL records while skipping malformed records and returning their count.
///
/// This is intentionally opt-in. Callers that use shared archive files for
/// authoritative state should continue using `read_jsonl_records`, which fails
/// on the first malformed record.
pub fn read_jsonl_records_resilient<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> io::Result<(Vec<T>, usize)> {
    if !path.exists() {
        return Ok((Vec::new(), 0));
    }

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut skipped_records = 0;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(record) => records.push(record),
            Err(_) => skipped_records += 1,
        }
    }
    Ok((records, skipped_records))
}

pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    crate::atomic_file::write_json_atomic(path, value)
}

pub fn materialize_text_payload(
    artifacts_dir: &Path,
    stem: &str,
    text: &str,
) -> io::Result<MaterializedTextPayload> {
    if text.len() <= CONVERSATION_INLINE_TEXT_LIMIT_BYTES {
        return Ok(MaterializedTextPayload {
            text: Some(text.to_string()),
            excerpt: None,
            artifact_refs: Vec::new(),
        });
    }

    fs::create_dir_all(artifacts_dir)?;
    let artifact_stem = sanitize_artifact_stem(stem);
    let mut suffix = 1_u32;
    let artifact_ref = loop {
        let candidate = format!("{artifact_stem}-{suffix:04}.txt");
        if !artifacts_dir.join(&candidate).exists() {
            break candidate;
        }
        suffix = suffix.saturating_add(1);
    };
    fs::write(artifacts_dir.join(&artifact_ref), text)?;

    Ok(MaterializedTextPayload {
        text: None,
        excerpt: Some(bounded_text_excerpt(
            text,
            CONVERSATION_INLINE_TEXT_LIMIT_BYTES,
        )),
        artifact_refs: vec![artifact_ref],
    })
}

pub fn append_index_upsert(path: &Path, entry: &ConversationIndexEntry) -> io::Result<()> {
    append_jsonl_record(path, entry)
}

pub fn read_latest_index_entries(path: &Path) -> io::Result<Vec<ConversationIndexEntry>> {
    let records = read_jsonl_records::<ConversationIndexEntry>(path)?;
    let mut latest_by_id: HashMap<String, ConversationIndexEntry> = HashMap::new();
    for entry in records {
        latest_by_id.insert(entry.conversation_id.clone(), entry);
    }

    let mut entries: Vec<_> = latest_by_id.into_values().collect();
    entries.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then_with(|| left.conversation_id.cmp(&right.conversation_id))
    });
    Ok(entries)
}

/// Streams JSONL rows through a bounded buffer, flushing before fsync and
/// closing the temporary file before replacement. No archive-sized copy is made.
pub fn write_jsonl_atomic<T: Serialize>(path: &Path, records: &[T]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = tmp_path_for(path);
    {
        let mut file = fs::File::create(&tmp_path)?;
        write_jsonl_to_writer(&mut file, records)?;
        file.sync_all()?;
    }
    replace_file(&tmp_path, path)
}

fn write_jsonl_to_writer<T: Serialize>(writer: &mut impl Write, records: &[T]) -> io::Result<()> {
    let mut buffer = BufWriter::with_capacity(JSONL_WRITE_BUFFER_BYTES, writer);
    for record in records {
        serde_json::to_writer(&mut buffer, record).map_err(io::Error::other)?;
        buffer.write_all(b"\n")?;
    }
    // Drop cannot report a buffered write failure. Propagate it before the
    // caller can fsync or replace the authoritative destination.
    buffer.flush()
}

fn sanitize_artifact_stem(stem: &str) -> String {
    let mut sanitized = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | '.' | ' ' | '/' | '\\' | ':')
            && !sanitized.ends_with('-')
            && !sanitized.is_empty()
        {
            sanitized.push('-');
        }
    }

    while sanitized.ends_with('-') {
        sanitized.pop();
    }

    if sanitized.is_empty() {
        "artifact".to_string()
    } else {
        sanitized
    }
}

fn bounded_text_excerpt(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let mut end = 0;
    for (index, ch) in text.char_indices() {
        let next = index + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// Counts calls reaching a real File below the production encoding/buffer.
    /// These are Write calls, not a claim about kernel syscall counts.
    struct CountedFile {
        file: fs::File,
        write_calls: usize,
        flush_calls: usize,
        max_chunk: usize,
        fail_write: bool,
        fail_flush: bool,
    }

    impl CountedFile {
        fn create(path: &Path) -> Self {
            Self {
                file: fs::File::create(path).unwrap(),
                write_calls: 0,
                flush_calls: 0,
                max_chunk: usize::MAX,
                fail_write: false,
                fail_flush: false,
            }
        }
    }

    impl Write for CountedFile {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::other("injected write failure"));
            }
            self.write_calls += 1;
            self.file.write(&bytes[..bytes.len().min(self.max_chunk)])
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flush_calls += 1;
            if self.fail_flush {
                return Err(io::Error::other("injected flush failure"));
            }
            self.file.flush()
        }
    }

    struct FailingRecord<'a>(&'a std::cell::Cell<bool>);

    impl Serialize for FailingRecord<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq;
            self.0.set(true);
            let mut sequence = serializer.serialize_seq(Some(2))?;
            sequence.serialize_element("prefix before serialization failure")?;
            Err(serde::ser::Error::custom("injected serialization failure"))
        }
    }

    fn jsonl_write_records() -> Vec<serde_json::Value> {
        (0..32)
            .map(|seq| {
                serde_json::json!({"schema":1,"seq":seq,"text":"λ 雪 \"quoted\"\nnext",
                    "refs":(0..64).map(|n|format!("event-{seq}-{n}")).collect::<Vec<_>>()})
            })
            .collect()
    }

    fn unbuffered_jsonl(writer: &mut impl Write, rows: &[serde_json::Value]) {
        for row in rows {
            serde_json::to_writer(&mut *writer, row).unwrap();
            writer.write_all(b"\n").unwrap();
        }
        writer.flush().unwrap();
    }

    #[test]
    fn jsonl_buffering_matches_legacy_bytes_and_reduces_real_file_writes() {
        let temp = tempfile::tempdir().unwrap();
        let rows = jsonl_write_records();
        let legacy_path = temp.path().join("legacy.jsonl");
        let append_path = temp.path().join("append.jsonl");
        let rewrite_path = temp.path().join("rewrite.jsonl");
        let mut legacy = CountedFile::create(&legacy_path);
        unbuffered_jsonl(&mut legacy, &rows);
        let mut appended = CountedFile::create(&append_path);
        for row in &rows {
            append_jsonl_to_writer(&mut appended, row).unwrap();
        }
        let mut rewritten = CountedFile::create(&rewrite_path);
        write_jsonl_to_writer(&mut rewritten, &rows).unwrap();
        let expected = fs::read(&legacy_path).unwrap();
        assert_eq!(fs::read(&append_path).unwrap(), expected);
        assert_eq!(fs::read(&rewrite_path).unwrap(), expected);
        assert_eq!(
            read_jsonl_records::<serde_json::Value>(&rewrite_path).unwrap(),
            rows
        );
        assert!(appended.write_calls * 10 < legacy.write_calls);
        assert!(rewritten.write_calls * 10 < legacy.write_calls);
        assert_eq!(appended.flush_calls, rows.len());
        assert_eq!(rewritten.flush_calls, 1);
    }

    #[test]
    fn jsonl_public_helpers_preserve_order_unicode_newlines_and_empty_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/archive.jsonl");
        let rows = jsonl_write_records();
        for row in &rows {
            append_jsonl_record(&path, row).unwrap();
        }
        let appended = fs::read(&path).unwrap();
        write_jsonl_atomic(&path, &rows).unwrap();
        assert_eq!(fs::read(&path).unwrap(), appended);
        assert_eq!(
            appended.iter().filter(|byte| **byte == b'\n').count(),
            rows.len()
        );
        write_jsonl_atomic::<serde_json::Value>(&path, &[]).unwrap();
        assert!(fs::read(&path).unwrap().is_empty());
    }

    #[test]
    fn jsonl_oversized_row_streams_through_fixed_rewrite_buffer() {
        let temp = tempfile::tempdir().unwrap();
        let row = serde_json::json!({"seq":7,"text":"雪".repeat(1024 * 1024)});
        let rows = [row];
        let path = temp.path().join("large.jsonl");
        append_jsonl_record(&path, &rows[0]).unwrap();
        let expected = fs::read(&path).unwrap();
        assert!(expected.len() > 2 * 1024 * 1024);
        write_jsonl_atomic(&path, &rows).unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(
            read_jsonl_records::<serde_json::Value>(&path).unwrap(),
            rows
        );
        let counted_path = temp.path().join("counted-large.jsonl");
        let mut writer = CountedFile::create(&counted_path);
        write_jsonl_to_writer(&mut writer, &rows).unwrap();
        assert_eq!(fs::read(&counted_path).unwrap(), expected);
        assert!(
            writer.write_calls < 64,
            "the oversized string must not fragment per character"
        );
    }

    #[test]
    fn jsonl_open_error_precedes_serialization_and_serialization_does_not_append_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let called = std::cell::Cell::new(false);
        let record = FailingRecord(&called);
        append_jsonl_record(temp.path(), &record).unwrap_err();
        assert!(
            !called.get(),
            "opening the destination must precede encoding"
        );
        let path = temp.path().join("archive.jsonl");
        fs::write(&path, b"{\"seq\":1}\n").unwrap();
        let expected = fs::read(&path).unwrap();
        let error = append_jsonl_record(&path, &record).unwrap_err();
        assert!(called.get());
        assert!(error.to_string().contains("injected serialization failure"));
        assert_eq!(fs::read(&path).unwrap(), expected);
        write_jsonl_atomic(&path, &[record]).unwrap_err();
        assert_eq!(
            fs::read(&path).unwrap(),
            expected,
            "failed encoding cannot replace target"
        );
    }

    #[test]
    fn jsonl_writers_handle_partial_writes_and_propagate_write_and_flush_failures() {
        let temp = tempfile::tempdir().unwrap();
        let rows = jsonl_write_records();
        for atomic in [false, true] {
            let path = temp.path().join(format!("partial-{atomic}.jsonl"));
            let mut writer = CountedFile::create(&path);
            writer.max_chunk = 7;
            if atomic {
                write_jsonl_to_writer(&mut writer, &rows).unwrap();
                assert_eq!(
                    read_jsonl_records::<serde_json::Value>(&path).unwrap(),
                    rows
                );
            } else {
                append_jsonl_to_writer(&mut writer, &rows[0]).unwrap();
                assert_eq!(
                    read_jsonl_records::<serde_json::Value>(&path).unwrap(),
                    rows[..1]
                );
            }
            for flush_fault in [false, true] {
                let mut writer = CountedFile::create(&path);
                writer.fail_write = !flush_fault;
                writer.fail_flush = flush_fault;
                let error = if atomic {
                    write_jsonl_to_writer(&mut writer, &rows)
                } else {
                    append_jsonl_to_writer(&mut writer, &rows[0])
                }
                .unwrap_err();
                assert!(error
                    .to_string()
                    .contains(if flush_fault { "flush" } else { "write" }));
            }
        }
    }

    #[test]
    // Byte equivalence is deterministic; timing output is informational.
    fn jsonl_real_file_write_metrics() {
        let temp = tempfile::tempdir().unwrap();
        let rows = jsonl_write_records();
        let mut results = Vec::new();
        let mut reference = None;
        for mode in ["legacy", "append", "rewrite"] {
            let path = temp.path().join(format!("{mode}.jsonl"));
            let mut writer = CountedFile::create(&path);
            let started = std::time::Instant::now();
            for _ in 0..100 {
                match mode {
                    "legacy" => unbuffered_jsonl(&mut writer, &rows),
                    "append" => {
                        for row in &rows {
                            append_jsonl_to_writer(&mut writer, row).unwrap();
                        }
                    }
                    _ => write_jsonl_to_writer(&mut writer, &rows).unwrap(),
                }
            }
            let elapsed = started.elapsed();
            let bytes = fs::read(&path).unwrap();
            if let Some(expected) = &reference {
                assert_eq!(&bytes, expected);
            } else {
                reference = Some(bytes.clone());
            }
            results.push(serde_json::json!({"mode":mode,"rows":rows.len()*100,
                "bytes":bytes.len(),"file_write_calls":writer.write_calls,
                "flush_calls":writer.flush_calls,"elapsed_ms":elapsed.as_secs_f64()*1000.0}));
        }
        eprintln!("{}", serde_json::to_string(&results).unwrap());
    }

    #[test]
    fn manifest_serializes_archive_enums_as_snake_case_without_capture_quality() {
        let manifest = ConversationManifest {
            schema: CONVERSATION_SCHEMA,
            conversation_id: "conv_20260615_000000_agent_1".to_string(),
            agent_id: "agent-1".to_string(),
            agent_name: "Coder One".to_string(),
            agent_class: "coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["session-1".to_string()],
            provider_source_key: Some("codex:session:session-1".to_string()),
            provider_native_refs: Vec::new(),
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-06-15T00:00:00.000Z".to_string(),
            updated_at: "2026-06-15T00:01:00.000Z".to_string(),
            closed_at: None,
            status: ConversationStatus::Interrupted,
            boundary_reason: ConversationBoundaryReason::ProviderSourceChanged,
            format_versions: ConversationFormatVersions::default(),
            record_count: 2,
            turn_count: 1,
            has_turns: true,
            lifecycle_only: false,
        };

        let json = serde_json::to_value(&manifest).unwrap();

        assert_eq!(json["schema"], CONVERSATION_SCHEMA);
        assert_eq!(json["status"], "interrupted");
        assert_eq!(json["boundary_reason"], "provider_source_changed");
        assert_eq!(json["effective_logging"], "enabled");
        assert_eq!(json["format_versions"]["turns"], CONVERSATION_TURNS_SCHEMA);
        assert_eq!(json["record_count"], 2);
        assert_eq!(json["turn_count"], 1);
        assert!(json.get("capture_quality").is_none());
    }

    #[test]
    fn manifest_missing_turns_format_deserializes_as_v1() {
        let manifest: ConversationManifest = serde_json::from_str(
            r#"{
              "schema": 1,
              "conversation_id": "conv_legacy",
              "agent_id": "agent-1",
              "agent_name": "Coder One",
              "agent_class": "coder",
              "workspace": "<absolute-workspace-path>",
              "provider": "codex",
              "provider_session_ids": [],
              "effective_logging": "enabled",
              "created_at": "2026-06-15T00:00:00.000Z",
              "updated_at": "2026-06-15T00:01:00.000Z",
              "closed_at": null,
              "status": "open",
              "boundary_reason": "spawn",
              "format_versions": {
                "manifest": 1,
                "conversation": 1,
                "events": 1,
                "sources": 1
              }
            }"#,
        )
        .unwrap();

        assert_eq!(manifest.format_versions.turns, 1);
    }

    #[test]
    fn narrative_tool_result_can_reference_artifacts() {
        let record = ConversationNarrativeRecord {
            schema: CONVERSATION_SCHEMA,
            seq: 7,
            turn_id: Some("turn-1".to_string()),
            at: "2026-06-15T00:00:07.000Z".to_string(),
            kind: ConversationRecordKind::ToolResult,
            role: Some("tool".to_string()),
            speaker_type: Some(ConversationSpeakerType::Tool),
            input_origin: None,
            input_purpose: None,
            request_root_id: None,
            causal_ref: None,
            text: None,
            tool: Some("shell_command".to_string()),
            status: Some("success".to_string()),
            summary: Some("captured long command output".to_string()),
            excerpt: Some("first 8 KiB of output".to_string()),
            event_refs: vec!["events:7".to_string()],
            source_refs: vec!["sources:3".to_string()],
            artifact_refs: vec!["artifacts/tool-result-7.txt".to_string()],
        };

        let json = serde_json::to_value(&record).unwrap();

        assert_eq!(json["kind"], "tool_result");
        assert_eq!(json["turn_id"], "turn-1");
        assert_eq!(json["speaker_type"], "tool");
        assert_eq!(json["tool"], "shell_command");
        assert_eq!(json["artifact_refs"][0], "artifacts/tool-result-7.txt");
    }

    #[test]
    fn turn_record_serializes_only_factual_aggregations_without_capture_quality() {
        let mut tools_used = std::collections::BTreeMap::new();
        tools_used.insert("shell_command".to_string(), 2);
        let record = ConversationTurnRecord {
            schema: CONVERSATION_TURNS_SCHEMA,
            conversation_id: "conv-1".to_string(),
            turn_index: 1,
            turn_key: "conv-1:turn:000001".to_string(),
            status: ConversationTurnStatus::Responded,
            status_source: "mechanical_assistant_message".to_string(),
            seq_start: 1,
            seq_end: 4,
            started_at: "2026-06-15T00:00:01.000Z".to_string(),
            updated_at: "2026-06-15T00:00:04.000Z".to_string(),
            request: ConversationTurnRequest {
                seq: 1,
                kind: "user_request".to_string(),
                text: Some("Run the tests.".to_string()),
                text_truncated: false,
                objective_text: None,
                objective_text_truncated: None,
            },
            assistant_result: Some(ConversationTurnAssistantResult {
                seq: 4,
                text: Some("Tests failed.".to_string()),
                text_truncated: false,
            }),
            counts: ConversationTurnCounts {
                records: 4,
                assistant_messages: 1,
                tool_calls: 1,
                tool_results: 1,
                nonzero_tool_results: 1,
                failed_tool_results: 1,
                timeouts: 0,
            },
            tools_used,
            files: ConversationTurnFiles {
                read: vec!["src/lib.rs".to_string()],
                written: vec!["src/lib.rs".to_string()],
                mentioned: vec!["docs/specs/archive.md".to_string()],
            },
            external_side_effects: vec![ConversationTurnSideEffect {
                kind: "git_commit".to_string(),
                evidence_seq: 4,
                summary: "commit created".to_string(),
                paths: Vec::new(),
            }],
            failure_signals: vec![ConversationTurnFailureSignal {
                kind: "command_nonzero_exit".to_string(),
                seq: 3,
                tool: Some("shell_command".to_string()),
                summary: Some("Exit code: 1".to_string()),
            }],
            record_refs: ConversationTurnRecordRefs {
                seq_start: 1,
                seq_end: 4,
                event_count: 1,
                source_count: 1,
                artifact_count: 0,
            },
            provider_refs: vec![ConversationTurnProviderRef {
                provider: "codex".to_string(),
                provider_session_id: Some("session-1".to_string()),
                source_kind: "provider_log".to_string(),
                source_id: Some("source-1".to_string()),
            }],
        };

        let json = serde_json::to_value(&record).unwrap();

        assert_eq!(json["conversation_id"], "conv-1");
        assert_eq!(json["schema"], CONVERSATION_TURNS_SCHEMA);
        assert_eq!(json["turn_index"], 1);
        assert_eq!(json["turn_key"], "conv-1:turn:000001");
        assert_eq!(json["status"], "responded");
        assert_eq!(json["status_source"], "mechanical_assistant_message");
        assert_eq!(json["request"]["kind"], "user_request");
        assert_eq!(json["assistant_result"]["text"], "Tests failed.");
        assert_eq!(json["counts"]["failed_tool_results"], 1);
        assert_eq!(json["tools_used"]["shell_command"], 2);
        assert_eq!(json["files"]["written"][0], "src/lib.rs");
        assert_eq!(json["external_side_effects"][0]["kind"], "git_commit");
        assert_eq!(json["failure_signals"][0]["kind"], "command_nonzero_exit");
        assert_eq!(json["schema"], 3);
        assert_eq!(json["record_refs"]["seq_start"], 1);
        assert_eq!(json["record_refs"]["seq_end"], 4);
        assert_eq!(json["record_refs"]["event_count"], 1);
        assert_eq!(json["record_refs"]["source_count"], 1);
        assert_eq!(json["record_refs"]["artifact_count"], 0);
        assert!(json["record_refs"].get("event_refs").is_none());
        assert_eq!(json["provider_refs"][0]["source_id"], "source-1");
        assert!(json["provider_refs"][0].get("source_path").is_none());
        assert!(json.get("provider_native_refs").is_none());
        assert!(json.get("turn_id").is_none());
        assert!(json.get("capture_quality").is_none());
        assert!(json.get("notes_for_evolver").is_none());
        assert!(json.get("user_correction").is_none());
    }

    #[test]
    fn turn_record_deserializes_null_status_source_as_unknown() {
        let record: ConversationTurnRecord = serde_json::from_value(serde_json::json!({
            "schema": 3,
            "conversation_id": "conv-legacy",
            "turn_index": 1,
            "turn_key": "conv-legacy:turn:000001",
            "status": "unknown",
            "status_source": null,
            "seq_start": 1,
            "seq_end": 1,
            "started_at": "2026-06-23T00:00:00.000Z",
            "updated_at": "2026-06-23T00:00:00.000Z",
            "request": {
                "seq": 1,
                "kind": "user_request",
                "text": null,
                "text_truncated": false
            },
            "counts": {
                "records": 1,
                "assistant_messages": 0,
                "tool_calls": 0,
                "tool_results": 0,
                "nonzero_tool_results": 0,
                "failed_tool_results": 0,
                "timeouts": 0
            },
            "tools_used": {},
            "files": {
                "read": [],
                "written": [],
                "mentioned": []
            },
            "external_side_effects": [],
            "failure_signals": [],
            "record_refs": {
                "seq_start": 1,
                "seq_end": 1
            }
        }))
        .expect("legacy turn record with null status_source");

        assert_eq!(record.status_source, "unknown");
    }

    #[test]
    fn source_record_serializes_provider_cursor() {
        let record = ConversationSourceRecord {
            schema: CONVERSATION_SCHEMA,
            source_id: "src_42".to_string(),
            provider: "opencode".to_string(),
            provider_session_id: Some("ses_abc123".to_string()),
            source_kind: "opencode_db".to_string(),
            source_path: Some("<absolute-workspace-path>/state/opencode.db".to_string()),
            cursor: Some("provider-cursor-42".to_string()),
            offset: Some(128),
            row_id: Some("part_123".to_string()),
            provider_event_type: Some("text".to_string()),
            hash: Some("sha256:abc123".to_string()),
            artifact_ref: Some("artifacts/source-42.json".to_string()),
        };

        let json = serde_json::to_value(&record).unwrap();

        assert_eq!(json["schema"], CONVERSATION_SCHEMA);
        assert_eq!(json["source_id"], "src_42");
        assert_eq!(json["provider"], "opencode");
        assert_eq!(json["provider_session_id"], "ses_abc123");
        assert_eq!(json["source_kind"], "opencode_db");
        assert_eq!(
            json["source_path"],
            "<absolute-workspace-path>/state/opencode.db"
        );
        assert_eq!(json["cursor"], "provider-cursor-42");
        assert_eq!(json["offset"], 128);
        assert_eq!(json["row_id"], "part_123");
        assert_eq!(json["provider_event_type"], "text");
        assert_eq!(json["hash"], "sha256:abc123");
        assert_eq!(json["artifact_ref"], "artifacts/source-42.json");
    }

    #[test]
    fn jsonl_append_creates_parent_dirs_and_read_skips_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("agent-1")
            .join("conversations")
            .join("conversation.jsonl");
        let first = narrative_record(1, "first message");
        let second = narrative_record(2, "second message");

        append_jsonl_record(&path, &first).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        append_jsonl_record(&path, &second).unwrap();

        let records: Vec<ConversationNarrativeRecord> = read_jsonl_records(&path).unwrap();
        let missing: Vec<ConversationNarrativeRecord> =
            read_jsonl_records(&dir.path().join("missing.jsonl")).unwrap();

        assert_eq!(records, vec![first, second]);
        assert!(missing.is_empty());
    }

    #[test]
    fn jsonl_read_reports_malformed_json_as_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conversation.jsonl");
        fs::write(&path, "{not-json}\n").unwrap();

        let error = read_jsonl_records::<ConversationNarrativeRecord>(&path).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Other);
    }

    #[test]
    fn write_json_atomic_creates_parent_dirs_and_writes_pretty_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("agent-1")
            .join("conversations")
            .join("conv-a")
            .join("manifest.json");
        let manifest = manifest("conv-a", ConversationStatus::Open);

        write_json_atomic(&path, &manifest).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("{\n"));
        assert!(content.contains("  \"conversation_id\": \"conv-a\""));
        assert!(content.ends_with('\n'));

        let roundtrip: ConversationManifest = serde_json::from_str(&content).unwrap();
        assert_eq!(roundtrip, manifest);
    }

    #[test]
    fn materialize_text_payload_keeps_small_text_inline() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts_dir = dir.path().join("artifacts");

        let payload =
            materialize_text_payload(&artifacts_dir, "Tool Result: 7/unsafe", "small output")
                .unwrap();

        assert_eq!(payload.text.as_deref(), Some("small output"));
        assert_eq!(payload.excerpt, None);
        assert!(payload.artifact_refs.is_empty());
        assert!(!artifacts_dir.exists());
    }

    #[test]
    fn materialize_text_payload_writes_large_text_to_deterministic_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts_dir = dir.path().join("artifacts");
        let text = format!(
            "{}tail",
            "x".repeat(CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1)
        );

        let payload =
            materialize_text_payload(&artifacts_dir, "Tool Result: 7/unsafe", &text).unwrap();

        assert_eq!(payload.text, None);
        assert_eq!(payload.artifact_refs, vec!["tool-result-7-unsafe-0001.txt"]);
        assert_eq!(
            fs::read_to_string(artifacts_dir.join("tool-result-7-unsafe-0001.txt")).unwrap(),
            text
        );
        let excerpt = payload.excerpt.unwrap();
        assert!(excerpt.len() <= CONVERSATION_INLINE_TEXT_LIMIT_BYTES);
        assert!(text.starts_with(&excerpt));
    }

    #[test]
    fn materialize_text_payload_uses_next_suffix_when_artifact_exists() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts_dir = dir.path().join("artifacts");
        let first = "x".repeat(CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);
        let second = "y".repeat(CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);

        let first_payload =
            materialize_text_payload(&artifacts_dir, "tool result", &first).unwrap();
        let second_payload =
            materialize_text_payload(&artifacts_dir, "tool result", &second).unwrap();

        assert_eq!(first_payload.artifact_refs, vec!["tool-result-0001.txt"]);
        assert_eq!(second_payload.artifact_refs, vec!["tool-result-0002.txt"]);
        assert_eq!(
            fs::read_to_string(artifacts_dir.join("tool-result-0001.txt")).unwrap(),
            first
        );
        assert_eq!(
            fs::read_to_string(artifacts_dir.join("tool-result-0002.txt")).unwrap(),
            second
        );
    }

    #[test]
    fn index_upsert_reads_latest_entry_per_conversation_sorted_by_started_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conversations").join("index.jsonl");
        let older = index_entry("conv-a", "2026-06-15T00:00:00.000Z", "first");
        let newest = index_entry("conv-b", "2026-06-15T00:02:00.000Z", "second");
        let updated = ConversationIndexEntry {
            status: ConversationStatus::Closed,
            ended_at: Some("2026-06-15T00:03:00.000Z".to_string()),
            last_record_excerpt: Some("latest update".to_string()),
            record_count: 3,
            ..index_entry("conv-a", "2026-06-15T00:01:00.000Z", "updated")
        };
        let tie = index_entry("conv-c", "2026-06-15T00:02:00.000Z", "third");

        append_index_upsert(&path, &older).unwrap();
        append_index_upsert(&path, &newest).unwrap();
        append_index_upsert(&path, &updated).unwrap();
        append_index_upsert(&path, &tie).unwrap();

        let entries = read_latest_index_entries(&path).unwrap();

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.conversation_id.as_str())
                .collect::<Vec<_>>(),
            vec!["conv-b", "conv-c", "conv-a"]
        );
        let conv_a = entries
            .iter()
            .find(|entry| entry.conversation_id == "conv-a")
            .unwrap();
        assert_eq!(conv_a.status, ConversationStatus::Closed);
        assert_eq!(conv_a.record_count, 3);
        assert_eq!(conv_a.last_record_excerpt.as_deref(), Some("latest update"));
    }

    fn narrative_record(seq: u64, text: &str) -> ConversationNarrativeRecord {
        ConversationNarrativeRecord {
            schema: CONVERSATION_SCHEMA,
            seq,
            turn_id: None,
            at: format!("2026-06-15T00:00:0{seq}.000Z"),
            kind: ConversationRecordKind::Message,
            role: Some("assistant".to_string()),
            speaker_type: Some(ConversationSpeakerType::Assistant),
            input_origin: None,
            input_purpose: None,
            request_root_id: None,
            causal_ref: None,
            text: Some(text.to_string()),
            tool: None,
            status: None,
            summary: None,
            excerpt: None,
            event_refs: Vec::new(),
            source_refs: Vec::new(),
            artifact_refs: Vec::new(),
        }
    }

    fn manifest(conversation_id: &str, status: ConversationStatus) -> ConversationManifest {
        ConversationManifest {
            schema: CONVERSATION_SCHEMA,
            conversation_id: conversation_id.to_string(),
            agent_id: "agent-1".to_string(),
            agent_name: "Coder One".to_string(),
            agent_class: "coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["session-1".to_string()],
            provider_source_key: Some("codex:session:session-1".to_string()),
            provider_native_refs: Vec::new(),
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-06-15T00:00:00.000Z".to_string(),
            updated_at: "2026-06-15T00:01:00.000Z".to_string(),
            closed_at: None,
            status,
            boundary_reason: ConversationBoundaryReason::Spawn,
            format_versions: ConversationFormatVersions::default(),
            record_count: 0,
            turn_count: 0,
            has_turns: false,
            lifecycle_only: false,
        }
    }

    fn index_entry(
        conversation_id: &str,
        started_at: &str,
        last_record_excerpt: &str,
    ) -> ConversationIndexEntry {
        ConversationIndexEntry {
            schema: CONVERSATION_SCHEMA,
            conversation_id: conversation_id.to_string(),
            agent_id: "agent-1".to_string(),
            agent_name: "Coder One".to_string(),
            agent_class: "coder".to_string(),
            workspace: "<absolute-workspace-path>".to_string(),
            provider: "codex".to_string(),
            provider_session_ids: vec!["session-1".to_string()],
            started_at: started_at.to_string(),
            ended_at: None,
            status: ConversationStatus::Open,
            boundary_reason: ConversationBoundaryReason::Spawn,
            first_prompt_excerpt: Some("first prompt".to_string()),
            last_record_excerpt: Some(last_record_excerpt.to_string()),
            record_count: 1,
            turn_count: 0,
            has_turns: false,
            lifecycle_only: false,
            artifact_count: 0,
            path: format!("agent-1/conversations/{conversation_id}"),
        }
    }
}
