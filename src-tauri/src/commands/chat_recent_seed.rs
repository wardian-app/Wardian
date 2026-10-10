//! Bounded, provisional observations from the independently owned recent source.
//! No archive cursor, complete-history or speculative alias claim is made here.
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use serde_json::json;
use sha2::{Digest, Sha256};
use wardian_core::models::chat::{AgentChatDetail, AgentChatEvent};

use super::chat::AgentArchiveCaptureSnapshot;
use super::provider_log_acquisition::{native_file_identity, ProviderLogNativeIdentity};

const HEADER_BYTES: u64 = 4096;
const TAIL_BYTES: u64 = 128 * 1024;
const RECORD_BYTES: usize = 16 * 1024;
const RECORDS: usize = 80;

#[derive(Default)]
pub(crate) struct Seed {
    pub(crate) revision: String,
    pub(crate) events: Vec<AgentChatEvent>,
    pub(crate) progress: String,
    pub(crate) bytes_read: usize,
    pub(crate) records_decoded: usize,
    pub(crate) unchanged: bool,
    pub(crate) checkpoint: Option<SeedCheckpoint>,
}

pub(crate) struct SeedCheckpoint {
    pub(crate) epoch: String,
    pub(crate) admission: String,
    pub(crate) extent: u64,
    pub(crate) before: Option<u64>,
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The exact native file identity, framed byte position, row digest and adapter
/// output ordinal prove a shared observation. Equal text never proves a bridge.
pub(crate) fn source_reference(
    agent_id: &str,
    identity: &ProviderLogNativeIdentity,
    offset: u64,
    raw: &str,
    ordinal: usize,
) -> String {
    format!(
        "source:{agent_id}:{}:{offset}:{}:{ordinal}",
        hash(&serde_json::to_vec(identity).unwrap_or_default()),
        hash(raw.as_bytes())
    )
}

fn ownership(snapshot: &AgentArchiveCaptureSnapshot, header: &[u8]) -> bool {
    let Some(expected) = snapshot
        .resume_session
        .as_deref()
        .or(snapshot.fresh_provider_session_id.as_deref())
    else {
        return false;
    };
    if snapshot
        .cleared_provider_sessions
        .iter()
        .any(|id| id == expected)
    {
        return false;
    }
    let Some(end) = header.iter().position(|b| *b == b'\n') else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&header[..end]) else {
        return false;
    };
    match snapshot.provider.as_str() {
        "codex" => {
            value["type"] == "session_meta" && value["payload"]["id"].as_str() == Some(expected)
        }
        "pi" => value["type"] == "session" && value["id"].as_str() == Some(expected),
        "claude" => value["sessionId"].as_str() == Some(expected),
        _ => false,
    }
}

fn observe(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> io::Result<(
    File,
    ProviderLogNativeIdentity,
    u64,
    String,
    usize,
    crate::state::conversation_archive::chat_read::SourcePolicy,
)> {
    let context = super::chat::conversation_archive_context_from_snapshot(snapshot);
    let policy = crate::state::conversation_archive::chat_read::source_policy(&context)?;
    let path = snapshot
        .log_path
        .as_deref()
        .ok_or_else(|| io::Error::other("source unavailable"))?;
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let extent = metadata.len();
    let identity = native_file_identity(&file)?;
    if identity != policy.identity {
        return Err(io::Error::other("source policy epoch changed"));
    }
    if !super::provider_log_acquisition::published_anchor_matches(&mut file, &policy.anchor)? {
        return Err(io::Error::other("source continuity changed"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut header = Vec::new();
    (&mut file)
        .take(extent.min(HEADER_BYTES))
        .read_to_end(&mut header)?;
    if !ownership(snapshot, &header) {
        return Err(io::Error::other("recent source ownership unavailable"));
    }
    let revision = hash(
        format!(
            "{identity:?}:{extent}:{:?}:{}:{}",
            metadata.modified().ok(),
            hash(&header),
            hash(&serde_json::to_vec(&policy).map_err(io::Error::other)?)
        )
        .as_bytes(),
    );
    Ok((
        file,
        identity,
        extent,
        revision,
        header.len()
            + HEADER_BYTES as usize
            + crate::state::conversation_archive::chat_read::POLICY_BYTES as usize,
        policy,
    ))
}

/// Validate the published privacy/ownership boundary without decoding history.
pub(crate) fn source_scope(
    snapshot: &AgentArchiveCaptureSnapshot,
) -> io::Result<(String, String, u64, String)> {
    let (file, identity, extent, _, _, policy) = observe(snapshot)?;
    Ok((
        hash(&serde_json::to_vec(&identity).map_err(io::Error::other)?),
        policy.admission_digest()?,
        extent,
        format!("{:?}", file.metadata()?.modified().ok()),
    ))
}

fn normalize(
    snapshot: &AgentArchiveCaptureSnapshot,
    identity: &ProviderLogNativeIdentity,
    offset: u64,
    raw: &str,
    state: &mut crate::providers::chat_transcript::TranscriptNormalizationState,
) -> Vec<AgentChatEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    if snapshot.provider == "claude"
        && value["sessionId"].as_str()
            != snapshot
                .resume_session
                .as_deref()
                .or(snapshot.fresh_provider_session_id.as_deref())
    {
        return Vec::new();
    }
    crate::providers::chat_transcript::normalize_chat_lines_with_state(
        &snapshot.session_id,
        &snapshot.provider,
        [raw],
        state,
        true,
        false,
    )
    .unwrap_or_default()
    .into_iter()
    .enumerate()
    .map(|(ordinal, mut event)| {
        let source_ref = source_reference(&snapshot.session_id, identity, offset, raw, ordinal);
        event.id = source_ref.clone();
        event.metadata["chat_source_ref"] = json!(source_ref);
        event.metadata["chat_source_start"] = json!(offset);
        event.metadata["chat_source_end"] = json!(offset + raw.len() as u64);
        event.metadata["chat_source_epoch"] =
            json!(hash(&serde_json::to_vec(identity).unwrap_or_default()));
        event.metadata["chat_provisional"] = json!(true);
        event.metadata["chat_identity_resolution"] = json!("unresolved");
        event.metadata["provider_log"] = json!(true);
        event.metadata["log_path"] = json!(snapshot
            .log_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()));
        event.metadata["provider_session_id"] = json!(snapshot
            .resume_session
            .as_ref()
            .or(snapshot.fresh_provider_session_id.as_ref()));
        // This independent row cannot infer a root from omitted history.
        if let Some(metadata) = event.metadata.as_object_mut() {
            metadata.remove("legacy_event_ids");
        }
        event
    })
    .collect()
}

pub(crate) fn read(
    snapshot: &AgentArchiveCaptureSnapshot,
    previous: Option<&str>,
) -> io::Result<Seed> {
    read_window(snapshot, previous, None)
}

/// A background checkpoint seeks backward inside one independently admitted
/// source interval. It never searches beyond the same recent-read budget.
pub(crate) fn read_window(
    snapshot: &AgentArchiveCaptureSnapshot,
    previous: Option<&str>,
    before: Option<u64>,
) -> io::Result<Seed> {
    if !matches!(snapshot.provider.as_str(), "codex" | "claude" | "pi") {
        return Ok(Seed {
            revision: "unsupported".into(),
            events: Vec::new(),
            progress: "ready".into(),
            bytes_read: 0,
            records_decoded: 0,
            unchanged: previous == Some("unsupported"),
            checkpoint: None,
        });
    }
    let (mut file, identity, extent, revision, header_bytes, policy) = match observe(snapshot) {
        Ok(observation) => observation,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Seed {
                revision: "absent".into(),
                events: Vec::new(),
                progress: "source_unavailable".into(),
                bytes_read: (2 * HEADER_BYTES
                    + crate::state::conversation_archive::chat_read::POLICY_BYTES)
                    as usize,
                records_decoded: 2,
                unchanged: previous == Some("absent"),
                checkpoint: None,
            })
        }
        Err(_) => {
            return Ok(Seed {
                revision: "unowned".into(),
                events: Vec::new(),
                progress: "ownership_pending".into(),
                bytes_read: (2 * HEADER_BYTES
                    + crate::state::conversation_archive::chat_read::POLICY_BYTES)
                    as usize,
                records_decoded: 2,
                unchanged: previous == Some("unowned"),
                checkpoint: None,
            })
        }
    };
    let mut checkpoint = SeedCheckpoint {
        epoch: hash(&serde_json::to_vec(&identity).map_err(io::Error::other)?),
        admission: policy.admission_digest()?,
        extent,
        before: None,
    };
    if previous == Some(revision.as_str()) {
        return Ok(Seed {
            revision,
            events: Vec::new(),
            progress: "provisional".into(),
            bytes_read: header_bytes,
            records_decoded: 1,
            unchanged: true,
            checkpoint: Some(checkpoint),
        });
    }
    let Some((lower, end)) = policy.recent_interval(before.unwrap_or(extent).min(extent)) else {
        return Ok(Seed {
            revision,
            events: Vec::new(),
            progress: "no_admitted_rows".into(),
            bytes_read: header_bytes,
            records_decoded: 2,
            unchanged: false,
            checkpoint: Some(checkpoint),
        });
    };
    let start = end.saturating_sub(TAIL_BYTES - 1).max(lower);
    let start = if start > lower { start - 1 } else { start };
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut file).take(end - start).read_to_end(&mut bytes)?;
    let mut verified = Vec::new();
    file.seek(SeekFrom::Start(start))?;
    (&mut file).take(end - start).read_to_end(&mut verified)?;
    let bytes_read = header_bytes + bytes.len() + verified.len();
    if bytes != verified
        || bytes.len() as u64 != end - start
        || file.metadata()?.len() < extent
        || native_file_identity(&file)? != identity
    {
        return Ok(Seed {
            revision,
            events: Vec::new(),
            progress: "source_changed".into(),
            bytes_read,
            records_decoded: 1,
            unchanged: false,
            checkpoint: None,
        });
    }
    let first = if start == lower && policy.framed_starts.contains(&start) {
        0
    } else {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|index| index + 1)
            .unwrap_or(bytes.len())
    };
    let last = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|index| index + 1)
        .unwrap_or(first);
    let mut rows = Vec::new();
    let mut position = first;
    for raw in bytes[first..last].split_inclusive(|b| *b == b'\n') {
        rows.push((start + position as u64, raw));
        position += raw.len();
    }
    let mut events = Vec::new();
    let mut decoded = 2;
    let mut oversized = first == last && extent > 0;
    let mut window = rows.into_iter().rev().take(RECORDS).collect::<Vec<_>>();
    window.reverse();
    let earliest = window.first().map(|(offset, _)| *offset).unwrap_or(start);
    let mut normalizer = crate::providers::chat_transcript::TranscriptNormalizationState::default();
    for (offset, raw) in window {
        if !policy.admits(offset, offset + raw.len() as u64) {
            normalizer = Default::default();
            continue;
        }
        if raw.len() > RECORD_BYTES {
            oversized = true;
            normalizer = Default::default();
            continue;
        }
        decoded += 1;
        let Ok(raw) = std::str::from_utf8(raw) else {
            normalizer = Default::default();
            continue;
        };
        let raw = raw.trim_end_matches(['\r', '\n']);
        let mut observations = normalize(snapshot, &identity, offset, raw, &mut normalizer);
        for (ordinal, event) in observations.iter_mut().enumerate() {
            let context = super::chat::conversation_archive_context_from_snapshot(snapshot);
            let reference = format!(
                "seed:{}:{}:{}:{offset}:{}:{}:{ordinal}",
                hash(&serde_json::to_vec(&identity).map_err(io::Error::other)?),
                policy.generation,
                hash(format!("{}:{:?}", context.agent_id, context.provider_source_key).as_bytes()),
                raw.len(),
                hash(raw.as_bytes())
            );
            let has_detail = crate::state::conversation_archive::chat_read::tool_input_body(event)
                .is_some()
                || event.text.as_ref().is_some_and(|text| {
                    text.len() > crate::state::conversation_archive::chat_read::PREVIEW_BYTES
                });
            *event = crate::state::conversation_archive::chat_read::header(event);
            event.metadata["chat_provisional"] = json!(true);
            event.metadata["chat_identity_resolution"] = json!("unresolved");
            if has_detail {
                event.metadata["chat_detail_ref"] = json!(reference);
            }
            event.metadata["chat_source_admission"] = json!(checkpoint.admission);
            event.sequence = Some(offset);
        }
        events.extend(observations);
        if events.len() >= RECORDS {
            events.truncate(RECORDS);
            break;
        }
    }
    let stable = observe(snapshot).is_ok_and(
        |(_, observed_identity, observed_extent, _, _, observed_policy)| {
            observed_identity == identity
                && observed_extent >= extent
                && policy.same_admission(&observed_policy)
        },
    );
    let bytes_read = bytes_read
        + (2 * HEADER_BYTES + crate::state::conversation_archive::chat_read::POLICY_BYTES) as usize;
    if !stable {
        events.clear();
    }
    checkpoint.before = policy.recent_interval(earliest).map(|_| earliest);
    Ok(Seed {
        revision,
        progress: if !stable {
            "source_changed"
        } else if oversized {
            "oversized_record"
        } else {
            "provisional"
        }
        .into(),
        events,
        bytes_read,
        records_decoded: decoded + 2,
        unchanged: false,
        checkpoint: stable.then_some(checkpoint),
    })
}

pub(crate) fn detail(
    snapshot: &AgentArchiveCaptureSnapshot,
    reference: &str,
) -> io::Result<AgentChatDetail> {
    let parts: Vec<_> = reference.split(':').collect();
    if !matches!(parts.len(), 8 | 9) || parts[0] != "seed" {
        return Err(io::Error::other("invalid recent detail"));
    }
    let offset: u64 = parts[4].parse().map_err(io::Error::other)?;
    let length: u64 = parts[5].parse().map_err(io::Error::other)?;
    let ordinal: usize = parts[7].parse().map_err(io::Error::other)?;
    let body_offset: usize = parts
        .get(8)
        .map_or(Ok(0), |offset| offset.parse().map_err(io::Error::other))?;
    let (mut file, identity, extent, _, _, policy) = observe(snapshot)?;
    let context = super::chat::conversation_archive_context_from_snapshot(snapshot);
    if length > RECORD_BYTES as u64
        || offset
            .checked_add(length)
            .is_none_or(|end| end > extent || !policy.admits(offset, end))
        || policy.generation.to_string() != parts[2]
        || hash(format!("{}:{:?}", context.agent_id, context.provider_source_key).as_bytes())
            != parts[3]
        || hash(&serde_json::to_vec(&identity).map_err(io::Error::other)?) != parts[1]
    {
        return Err(io::Error::other("stale recent detail"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    (&mut file).take(length).read_to_end(&mut bytes)?;
    if hash(&bytes) != parts[6] {
        return Err(io::Error::other("recent detail source changed"));
    }
    let raw = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let event = normalize(snapshot, &identity, offset, raw, &mut Default::default())
        .into_iter()
        .nth(ordinal)
        .ok_or_else(|| io::Error::other("recent detail row unavailable"))?;
    if !observe(snapshot).is_ok_and(
        |(_, observed_identity, observed_extent, _, _, observed_policy)| {
            observed_identity == identity
                && observed_extent >= extent
                && policy.same_admission(&observed_policy)
        },
    ) {
        return Err(io::Error::other("recent detail scope changed"));
    }
    let body =
        crate::state::conversation_archive::chat_read::inline_body_text(&event).unwrap_or_default();
    let remaining = body
        .get(body_offset..)
        .ok_or_else(|| io::Error::other("invalid recent detail offset"))?;
    let text = crate::state::conversation_archive::chat_read::clip(
        remaining,
        crate::state::conversation_archive::chat_read::BODY_BYTES,
    );
    let next_offset = body_offset + text.len();
    let complete = next_offset == body.len();
    Ok(AgentChatDetail {
        event_id: event.id,
        text,
        next: (!complete).then(|| format!("{}:{next_offset}", parts[..8].join(":"))),
        complete,
    })
}
