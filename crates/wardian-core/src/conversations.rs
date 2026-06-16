//! Public model types for agent-owned conversation archives.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};
#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

pub const CONVERSATION_SCHEMA: u8 = 1;
pub const CONVERSATION_INLINE_TEXT_LIMIT_BYTES: usize = 8 * 1024;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationFormatVersions {
    pub manifest: u8,
    pub conversation: u8,
    pub events: u8,
    pub sources: u8,
}

impl Default for ConversationFormatVersions {
    fn default() -> Self {
        Self {
            manifest: 1,
            conversation: 1,
            events: 1,
            sources: 1,
        }
    }
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
    pub effective_logging: ConversationLoggingSetting,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub status: ConversationStatus,
    pub boundary_reason: ConversationBoundaryReason,
    pub format_versions: ConversationFormatVersions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationNarrativeRecord {
    pub schema: u8,
    pub seq: u64,
    pub at: String,
    pub kind: ConversationRecordKind,
    pub role: Option<String>,
    pub speaker_type: Option<ConversationSpeakerType>,
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
    pub artifact_count: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MaterializedTextPayload {
    pub text: Option<String>,
    pub excerpt: Option<String>,
    pub artifact_refs: Vec<String>,
}

pub fn append_jsonl_record<T: Serialize>(path: &Path, record: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, record).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()
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

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = tmp_path_for(path);
    let mut file = fs::File::create(&tmp_path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()?;
    drop(file);

    replace_file(&tmp_path, path)
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

fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("wardian");
    path.with_file_name(format!(".{file_name}.tmp"))
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    let from = wide_null(from.as_os_str());
    let to = wide_null(to.as_os_str());
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    // Windows std::fs::rename does not replace an existing destination.
    // MoveFileExW avoids a pre-delete gap while keeping the operation same-volume.
    let replaced = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
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

    #[test]
    fn manifest_serializes_archive_enums_as_snake_case() {
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
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-06-15T00:00:00.000Z".to_string(),
            updated_at: "2026-06-15T00:01:00.000Z".to_string(),
            closed_at: None,
            status: ConversationStatus::Interrupted,
            boundary_reason: ConversationBoundaryReason::ProviderSourceChanged,
            format_versions: ConversationFormatVersions::default(),
        };

        let json = serde_json::to_value(&manifest).unwrap();

        assert_eq!(json["schema"], CONVERSATION_SCHEMA);
        assert_eq!(json["status"], "interrupted");
        assert_eq!(json["boundary_reason"], "provider_source_changed");
        assert_eq!(json["effective_logging"], "enabled");
    }

    #[test]
    fn narrative_tool_result_can_reference_artifacts() {
        let record = ConversationNarrativeRecord {
            schema: CONVERSATION_SCHEMA,
            seq: 7,
            at: "2026-06-15T00:00:07.000Z".to_string(),
            kind: ConversationRecordKind::ToolResult,
            role: Some("tool".to_string()),
            speaker_type: Some(ConversationSpeakerType::Tool),
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
        assert_eq!(json["speaker_type"], "tool");
        assert_eq!(json["tool"], "shell_command");
        assert_eq!(json["artifact_refs"][0], "artifacts/tool-result-7.txt");
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

        let first_payload = materialize_text_payload(&artifacts_dir, "tool result", &first).unwrap();
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
            at: format!("2026-06-15T00:00:0{seq}.000Z"),
            kind: ConversationRecordKind::Message,
            role: Some("assistant".to_string()),
            speaker_type: Some(ConversationSpeakerType::Assistant),
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
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-06-15T00:00:00.000Z".to_string(),
            updated_at: "2026-06-15T00:01:00.000Z".to_string(),
            closed_at: None,
            status,
            boundary_reason: ConversationBoundaryReason::Spawn,
            format_versions: ConversationFormatVersions::default(),
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
            artifact_count: 0,
            path: format!("agent-1/conversations/{conversation_id}"),
        }
    }
}
