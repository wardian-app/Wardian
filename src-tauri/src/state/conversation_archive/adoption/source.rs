//! Read-only, bounded Codex archive input. A selection is evidence, not authority:
//! the adoption owner must persist and authorize it separately from live capture.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use wardian_core::models::chat::{AgentChatEvent, AgentChatRole};

use crate::commands::provider_log_acquisition::{
    native_file_identity, published_anchor_matches, ProviderLogContinuityAnchor,
    ProviderLogNativeIdentity, PROVIDER_LOG_BATCH_BYTES, PROVIDER_LOG_MAX_RECORD_BYTES,
};
use crate::providers::chat_transcript::{
    normalize_chat_lines_with_state, TranscriptNormalizationState,
};

// Bump when changing this adapter's normalization/coordinate contract. IDs must
// never depend on a grant ID, selection boundary, path spelling, or pass number.
const NORMALIZER_VERSION: u32 = 1;
const PROVENANCE_KEY: &str = "archive_adoption";

/// Immutable witnesses for the operator's numeric interval. Bodies stay on disk.
/// `observed_eof` is an extent witness; it never widens the supplied end.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Selection {
    pub(super) path: String,
    pub(super) native_identity: ProviderLogNativeIdentity,
    pub(super) anchor: ProviderLogContinuityAnchor,
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) observed_eof: u64,
    header: RecordWitness,
    chunks: Vec<ChunkWitness>,
    next_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct RecordWitness {
    start: u64,
    end: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ChunkWitness {
    start: u64,
    end: u64,
    sha256: String,
    first_sequence: u64,
}

/// Only physical provenance. These fields confer no live-source admission,
/// source renewal, native proof, or compatibility alias.
#[derive(Debug, Serialize, Deserialize)]
struct Provenance {
    agent_id: String,
    session_id: String,
    native_identity: ProviderLogNativeIdentity,
    record_start: u64,
    record_end: u64,
    record_sha256: String,
    chunk_start: u64,
    chunk_sha256: String,
    normalizer_version: u32,
    output_ordinal: u64,
    provider_record: ProviderRecord,
}

/// Fixed fields and bounded scalar values only, never provider content or raw
/// JSON. Distinct native turn fields survive conflicting provider bindings.
#[derive(Debug, Serialize, Deserialize)]
struct ProviderRecord {
    kind: String,
    payload_kind: Option<Value>,
    turn_id: Option<Value>,
    payload_turn_id: Option<Value>,
    metadata_turn_id: Option<Value>,
    message_id: Option<Value>,
    payload_message_id: Option<Value>,
    timestamp: Option<Value>,
}

pub(super) struct Batch {
    pub events: Vec<AgentChatEvent>,
    pub next_cursor: u64,
    pub normalizer: TranscriptNormalizationState,
}

/// Pin an owned Codex file and scan only `[start, end)` with bounded buffers.
/// Complete JSONL records are validated before issuing witnesses. A partial last
/// record is excluded even if the file has already grown past the requested end.
pub(super) fn prepare(
    path: &Path,
    session: &str,
    start: u64,
    end: u64,
    anchor: &ProviderLogContinuityAnchor,
) -> io::Result<Selection> {
    let path = path.canonicalize()?;
    let mut file = File::open(&path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("adoption source is not a regular file"));
    }
    let observed_eof = file.metadata()?.len();
    if start > end || end > observed_eof {
        return Err(invalid(
            "adoption interval exceeds the observed file extent",
        ));
    }
    let native_identity = native_file_identity(&file)?;
    let header_bytes = read_window(&mut file, 0, observed_eof, false)?;
    let header_end = header_bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| invalid("adoption source has no complete session header"))?
        + 1;
    let header_bytes = &header_bytes[..header_end];
    check_header(header_bytes, session)?;
    check_anchor(&mut file, anchor, start)?;
    check_boundary(&mut file, start)?;

    let mut selection = Selection {
        path: path
            .to_str()
            .ok_or_else(|| invalid("adoption path is not UTF-8"))?
            .to_owned(),
        native_identity,
        anchor: anchor.clone(),
        start,
        end: start,
        observed_eof,
        header: witness(0, header_bytes),
        chunks: Vec::new(),
        next_sequence: 1,
    };
    let mut sequence = 1_u64;
    while selection.end < end {
        let bytes = read_window(&mut file, selection.end, end, true)?;
        if bytes.is_empty() {
            break;
        }
        let mut offset = selection.end;
        let mut record_count = 0;
        for record in bytes.split_inclusive(|byte| *byte == b'\n') {
            decode_record(record)?;
            record_count += 1;
            offset += record.len() as u64;
        }
        selection.chunks.push(ChunkWitness {
            start: selection.end,
            end: offset,
            sha256: digest(&bytes),
            first_sequence: sequence,
        });
        sequence = sequence
            .checked_add(record_count)
            .ok_or_else(|| invalid("adoption record count overflow"))?;
        selection.end = offset;
    }
    selection.next_sequence = sequence;
    // Recheck the path as well as the original handle after a potentially long
    // preparation scan. Later passes check the saved byte witnesses themselves.
    validate(&selection, session)?;
    Ok(selection)
}

/// Bounded identity/header/anchor/extent validation. Interval integrity is
/// checked chunk by chunk by `advance`, not by rereading the whole transcript.
pub(super) fn validate(selection: &Selection, session: &str) -> io::Result<()> {
    open_validated(selection, session).map(|_| ())
}

/// Consume exactly one prepared chunk, verifying its digest before normalizing
/// any bytes. The caller owns the full returned bodies and continuation state.
/// Only returned cursors are resumable; no prior live normalizer is imported.
pub(super) fn advance(
    selection: &Selection,
    agent: &str,
    session: &str,
    cursor: u64,
    normalizer: &TranscriptNormalizationState,
) -> io::Result<Batch> {
    let mut file = open_validated(selection, session)?;
    let index = selection
        .chunks
        .binary_search_by_key(&cursor, |chunk| chunk.start);
    if cursor < selection.start || cursor > selection.end {
        return Err(invalid("adoption cursor is outside the selected interval"));
    }
    let expected_sequence = if cursor == selection.end {
        selection.next_sequence
    } else {
        selection.chunks[index.map_err(|_| invalid("adoption cursor is not a batch boundary"))?]
            .first_sequence
    };
    if normalizer.next_sequence() != expected_sequence
        || (cursor == selection.start && *normalizer != TranscriptNormalizationState::default())
    {
        return Err(invalid(
            "adoption continuation does not match the selected cursor",
        ));
    }
    normalizer.validate_serialized_size().map_err(invalid)?;
    for event in normalizer.pending_events() {
        check_pending(selection, agent, session, cursor, event)?;
    }
    let mut next = normalizer.clone();
    if cursor == selection.end {
        // Every pending row was checked above and the saved state is bounded to
        // 1 MiB. Empty input drains only those physical observations; it cannot
        // import pre-start context, read a later request, or invent completion.
        let mut events = normalize_chat_lines_with_state(
            session,
            "codex",
            std::iter::empty::<&str>(),
            &mut next,
            true,
            true,
        )
        .map_err(invalid)?;
        for event in &mut events {
            if event.metadata["input_origin"] == "context_injection"
                && event.metadata["request_root_id"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                // Match the shared normalizer's release of unbound context at
                // a native turn boundary. Preserve its physical ID/provenance
                // and leave the unknown request root absent.
                event.role = Some(AgentChatRole::System);
            }
        }
        return Ok(Batch {
            events,
            next_cursor: cursor,
            normalizer: next,
        });
    }
    let chunk = &selection.chunks[index.expect("checked batch boundary")];
    let len = chunk
        .end
        .checked_sub(chunk.start)
        .filter(|len| *len > 0 && *len <= PROVIDER_LOG_MAX_RECORD_BYTES)
        .ok_or_else(|| invalid("invalid adoption chunk extent"))?;
    if chunk.end > selection.end {
        return Err(invalid("adoption chunk exceeds the selected end"));
    }
    let bytes = read_exact_range(&mut file, cursor, len)?;
    if digest(&bytes) != chunk.sha256 {
        return Err(invalid("adoption source chunk was rewritten"));
    }
    // Decode/check every record before letting normalization mutate a clone.
    let mut offset = cursor;
    let mut records = Vec::new();
    for raw in bytes.split_inclusive(|byte| *byte == b'\n') {
        let parsed = decode_record(raw)?;
        let actual = witness(offset, raw);
        offset = actual.end;
        records.push((raw, parsed, actual));
    }
    let after_sequence = selection
        .chunks
        .get(index.expect("checked boundary") + 1)
        .map_or(selection.next_sequence, |next| next.first_sequence);
    if chunk.first_sequence.checked_add(records.len() as u64) != Some(after_sequence)
        || offset != chunk.end
    {
        return Err(invalid("adoption chunk framing mismatch"));
    }
    let mut events = Vec::new();
    for (raw, parsed, record) in records {
        let sequence = next.next_sequence();
        let line = std::str::from_utf8(raw).map_err(invalid)?;
        let mut released =
            normalize_chat_lines_with_state(session, "codex", [line], &mut next, false, true)
                .map_err(invalid)?;
        let mut output_ordinal = 0;
        for event in released.iter_mut().chain(next.pending_events_mut()) {
            if event.sequence == Some(sequence) {
                attach_provenance(
                    selection,
                    agent,
                    session,
                    chunk,
                    &record,
                    &parsed,
                    event,
                    output_ordinal,
                )?;
                output_ordinal += 1;
            } else {
                // Deferred events already carry their ORIGINAL coordinate/ID.
                check_pending(selection, agent, session, record.start, event)?;
            }
        }
        // Version 1 of the shared Codex normalizer produces at most one event
        // per physical record. Reject a changed contract until IDs are versioned.
        if output_ordinal > 1 {
            return Err(invalid("adoption normalizer output contract changed"));
        }
        next.validate_serialized_size().map_err(invalid)?;
        events.append(&mut released);
    }
    Ok(Batch {
        events,
        next_cursor: chunk.end,
        normalizer: next,
    })
}

fn open_validated(selection: &Selection, session: &str) -> io::Result<File> {
    if selection.start > selection.end
        || selection.end > selection.observed_eof
        || selection.header.start != 0
        || selection.header.end == 0
        || selection.header.end > PROVIDER_LOG_MAX_RECORD_BYTES
        || selection.header.end > selection.observed_eof
        || selection.chunks.first().map(|chunk| chunk.start)
            != (selection.start < selection.end).then_some(selection.start)
        || selection.chunks.last().map(|chunk| chunk.end)
            != (selection.start < selection.end).then_some(selection.end)
    {
        return Err(invalid("invalid adoption selection witnesses"));
    }
    let mut file = File::open(&selection.path)?;
    if !file.metadata()?.is_file()
        || native_file_identity(&file)? != selection.native_identity
        || file.metadata()?.len() < selection.observed_eof
    {
        return Err(invalid("adoption source was replaced or truncated"));
    }
    let header = read_exact_range(&mut file, 0, selection.header.end)?;
    if witness(0, &header) != selection.header {
        return Err(invalid("adoption session header was rewritten"));
    }
    check_header(&header, session)?;
    check_anchor(&mut file, &selection.anchor, selection.start)?;
    check_boundary(&mut file, selection.start)?;
    Ok(file)
}

fn check_header(bytes: &[u8], session: &str) -> io::Result<()> {
    let header = decode_record(bytes)?;
    if session.is_empty()
        || header["type"] != "session_meta"
        || header["payload"]["id"].as_str() != Some(session)
    {
        return Err(invalid("adoption source is not owned by the Codex session"));
    }
    Ok(())
}

fn check_anchor(
    file: &mut File,
    anchor: &ProviderLogContinuityAnchor,
    start: u64,
) -> io::Result<()> {
    if anchor.start.checked_add(anchor.len) != Some(start)
        || !published_anchor_matches(file, anchor)?
    {
        return Err(invalid("adoption continuity anchor does not match"));
    }
    Ok(())
}

fn check_boundary(file: &mut File, start: u64) -> io::Result<()> {
    if start > 0 && read_exact_range(file, start - 1, 1)? != b"\n" {
        return Err(invalid("adoption start is not a record boundary"));
    }
    Ok(())
}

/// Normal work is 256 KiB. Only a first record with no newline in that window
/// can extend the buffer, up to 16 MiB INCLUDING its newline. During preparation
/// an oversized suffix is scanned with fixed scratch space: an unterminated
/// suffix is excluded, whereas an oversized complete record is rejected.
fn read_window(file: &mut File, start: u64, end: u64, preparing: bool) -> io::Result<Vec<u8>> {
    let remaining = end
        .checked_sub(start)
        .ok_or_else(|| invalid("invalid adoption read extent"))?;
    let first_len = remaining.min(PROVIDER_LOG_BATCH_BYTES);
    let mut bytes = read_exact_range(file, start, first_len)?;
    if let Some(index) = bytes.iter().rposition(|byte| *byte == b'\n') {
        bytes.truncate(index + 1);
        return Ok(bytes);
    }
    while (bytes.len() as u64) < remaining.min(PROVIDER_LOG_MAX_RECORD_BYTES) {
        let previous_len = bytes.len();
        let len = (remaining - previous_len as u64)
            .min(PROVIDER_LOG_BATCH_BYTES)
            .min(PROVIDER_LOG_MAX_RECORD_BYTES - previous_len as u64);
        bytes.resize(previous_len + len as usize, 0);
        file.read_exact(&mut bytes[previous_len..])?;
        if let Some(index) = bytes[previous_len..].iter().position(|byte| *byte == b'\n') {
            bytes.truncate(previous_len + index + 1);
            return Ok(bytes);
        }
    }
    if bytes.len() as u64 == remaining {
        return Ok(Vec::new());
    }
    if !preparing {
        return Err(invalid("adoption record exceeds 16 MiB"));
    }
    let mut scanned = bytes.len() as u64;
    // Reuse the existing allocation without retaining an unbounded suffix.
    bytes.resize(PROVIDER_LOG_BATCH_BYTES as usize, 0);
    while scanned < remaining {
        let len = (remaining - scanned).min(PROVIDER_LOG_BATCH_BYTES) as usize;
        file.read_exact(&mut bytes[..len])?;
        if bytes[..len].contains(&b'\n') {
            return Err(invalid("adoption record exceeds 16 MiB"));
        }
        scanned += len as u64;
    }
    Ok(Vec::new())
}

fn read_exact_range(file: &mut File, start: u64, len: u64) -> io::Result<Vec<u8>> {
    if len > PROVIDER_LOG_MAX_RECORD_BYTES {
        return Err(invalid("adoption read exceeds the bounded buffer"));
    }
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; len as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn decode_record(bytes: &[u8]) -> io::Result<Value> {
    if bytes.len() as u64 > PROVIDER_LOG_MAX_RECORD_BYTES || bytes.last() != Some(&b'\n') {
        return Err(invalid("invalid adoption JSONL framing"));
    }
    let text = std::str::from_utf8(bytes).map_err(invalid)?;
    let parsed: Value = serde_json::from_str(text).map_err(invalid)?;
    if !parsed.is_object() || parsed["type"].as_str().is_none_or(str::is_empty) {
        return Err(invalid(
            "adoption record is not a typed provider JSON object",
        ));
    }
    Ok(parsed)
}

fn witness(start: u64, bytes: &[u8]) -> RecordWitness {
    RecordWitness {
        start,
        end: start + bytes.len() as u64,
        sha256: digest(bytes),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[expect(
    clippy::too_many_arguments,
    reason = "bind one normalized output to its physical record"
)]
fn attach_provenance(
    selection: &Selection,
    agent: &str,
    session: &str,
    chunk: &ChunkWitness,
    record: &RecordWitness,
    parsed: &Value,
    event: &mut AgentChatEvent,
    output_ordinal: u64,
) -> io::Result<()> {
    let provenance = Provenance {
        agent_id: agent.to_owned(),
        session_id: session.to_owned(),
        native_identity: selection.native_identity.clone(),
        record_start: record.start,
        record_end: record.end,
        record_sha256: record.sha256.clone(),
        chunk_start: chunk.start,
        chunk_sha256: chunk.sha256.clone(),
        normalizer_version: NORMALIZER_VERSION,
        output_ordinal,
        provider_record: physical_provider_fields(parsed)?,
    };
    event.id = event_id(&provenance)?;
    // Archive envelopes belong to the Wardian agent; the provider session stays
    // explicit record provenance and must not become the saved chat session.
    event.session_id = agent.to_owned();
    event.metadata["provider_log"] = Value::Bool(true);
    event.metadata["log_path"] = Value::String(selection.path.clone());
    event.metadata["provider_session_id"] = Value::String(session.to_owned());
    event.metadata[PROVENANCE_KEY] = serde_json::to_value(provenance).map_err(invalid)?;
    Ok(())
}

/// Retain exact provider kind, message/turn identifiers and clock fields before
/// normalization adds inferred context. Exclude content and arbitrary metadata.
fn physical_provider_fields(parsed: &Value) -> io::Result<ProviderRecord> {
    let payload = parsed
        .get("payload")
        .or_else(|| parsed.get("item"))
        .unwrap_or(&Value::Null);
    let kind = bounded_scalar(parsed.get("type"))?
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| invalid("adoption record kind is missing"))?;
    Ok(ProviderRecord {
        kind,
        payload_kind: bounded_scalar(payload.get("type"))?,
        turn_id: bounded_scalar(parsed.get("turn_id"))?,
        payload_turn_id: bounded_scalar(payload.get("turn_id"))?,
        metadata_turn_id: bounded_scalar(
            payload["internal_chat_message_metadata_passthrough"].get("turn_id"),
        )?,
        message_id: bounded_scalar(parsed.get("id"))?,
        payload_message_id: bounded_scalar(payload.get("id").or_else(|| payload.get("call_id")))?,
        timestamp: bounded_scalar([parsed, payload].into_iter().find_map(|value| {
            ["timestamp", "created_at", "time", "ts"]
                .into_iter()
                .find_map(|key| value.get(key))
        }))?,
    })
}

fn bounded_scalar(value: Option<&Value>) -> io::Result<Option<Value>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= 256 => Ok(Some(Value::String(value.clone()))),
        Some(Value::Number(value)) => Ok(Some(Value::Number(value.clone()))),
        _ => Err(invalid(
            "adoption provider identity exceeds bounded scalar fields",
        )),
    }
}

fn event_id(provenance: &Provenance) -> io::Result<String> {
    // JSON tuple encoding makes string boundaries unambiguous. Neither selection
    // nor authorization identity participates, so retries/adjoining grants agree.
    let bytes = serde_json::to_vec(&(
        "wardian-archive-adoption",
        &provenance.agent_id,
        &provenance.session_id,
        &provenance.native_identity,
        provenance.record_start,
        provenance.record_end,
        &provenance.record_sha256,
        provenance.normalizer_version,
        provenance.output_ordinal,
    ))
    .map_err(invalid)?;
    Ok(format!("archive-adoption:{}", digest(&bytes)))
}

fn check_pending(
    selection: &Selection,
    agent: &str,
    session: &str,
    before: u64,
    event: &AgentChatEvent,
) -> io::Result<()> {
    let provenance: Provenance = serde_json::from_value(event.metadata[PROVENANCE_KEY].clone())
        .map_err(|_| invalid("adoption pending event has unknown provenance"))?;
    let chunk_index = selection
        .chunks
        .binary_search_by_key(&provenance.chunk_start, |chunk| chunk.start)
        .map_err(|_| invalid("adoption pending event has no prepared chunk"))?;
    let chunk = selection
        .chunks
        .get(chunk_index)
        .ok_or_else(|| invalid("adoption pending event is outside the selection"))?;
    let after_sequence = selection
        .chunks
        .get(chunk_index + 1)
        .map_or(selection.next_sequence, |next| next.first_sequence);
    if provenance.agent_id != agent
        || provenance.session_id != session
        || provenance.native_identity != selection.native_identity
        || provenance.normalizer_version != NORMALIZER_VERSION
        || provenance.output_ordinal != 0
        || provenance.record_start < selection.start
        || provenance.record_start < chunk.start
        || provenance.record_start >= provenance.record_end
        || provenance.record_end > before
        || provenance.record_end > chunk.end
        || provenance.record_sha256.len() != 64
        || !provenance
            .record_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || provenance.chunk_sha256 != chunk.sha256
        || event.provider != "codex"
        || event.session_id != agent
        || event.metadata["provider_log"] != true
        || event.metadata["log_path"].as_str() != Some(selection.path.as_str())
        || event.metadata["provider_session_id"].as_str() != Some(session)
        || event
            .sequence
            .is_none_or(|sequence| sequence < chunk.first_sequence || sequence >= after_sequence)
        || event.id != event_id(&provenance)?
    {
        return Err(invalid(
            "adoption pending event provenance does not match its prepared record",
        ));
    }
    Ok(())
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;
