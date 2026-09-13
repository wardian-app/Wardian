//! Bounded forward acquisition for append-only provider JSONL logs.
//!
//! Continuity combines the opened file's native identity with a fixed overlap
//! hash at the committed cursor. This detects replacement, truncation, and
//! rewrites near the cursor. It deliberately assumes provider logs are
//! append-only; a small overlap cannot prove that an arbitrary older prefix was
//! never rewritten in place.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wardian_core::models::chat::AgentChatEvent;

use crate::providers::chat_transcript::{
    normalize_chat_lines_with_state, TranscriptNormalizationState,
};

pub(crate) const PROVIDER_LOG_BATCH_BYTES: u64 = 256 * 1024;
const PROVIDER_LOG_ANCHOR_BYTES: u64 = 4 * 1024;
const MAX_PROVIDER_LOG_POLICY_SPANS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProviderLogNativeIdentity {
    pub(crate) platform: String,
    pub(crate) primary: u64,
    pub(crate) secondary: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProviderLogContinuityAnchor {
    pub(crate) start: u64,
    pub(crate) len: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProviderLogDisabledSpan {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProviderLogCaptureState {
    pub(crate) provider_source_key: String,
    pub(crate) path: String,
    pub(crate) native_identity: ProviderLogNativeIdentity,
    pub(crate) committed_offset: u64,
    pub(crate) continuity_anchor: ProviderLogContinuityAnchor,
    pub(crate) normalizer: TranscriptNormalizationState,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unknown_before_offset: Option<u64>,
    #[serde(default)]
    pub(crate) policy_generation: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) disabled_spans: Vec<ProviderLogDisabledSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) open_disabled_from: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct ProviderLogBatch {
    pub(crate) events: Vec<AgentChatEvent>,
    pub(crate) previous: Option<ProviderLogCaptureState>,
    pub(crate) next: ProviderLogCaptureState,
    pub(crate) consumed_bytes: u64,
    pub(crate) continue_immediately: bool,
}

pub(crate) fn acquire_provider_log_batch(
    session_id: &str,
    provider: &str,
    path: &Path,
    provider_source_key: &str,
    previous: Option<ProviderLogCaptureState>,
    trust_source_from_start: bool,
) -> io::Result<ProviderLogBatch> {
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let file_len = metadata.len();
    let canonical_path = std::fs::canonicalize(path)?.to_string_lossy().to_string();
    let native_identity = native_file_identity(&file)?;

    let mut state = match previous.clone() {
        Some(state) => {
            if state.provider_source_key != provider_source_key
                || state.path != canonical_path
                || state.native_identity != native_identity
            {
                return Ok(incomplete_batch(
                    previous,
                    state,
                    "provider_log_source_replaced",
                ));
            }
            if file_len < state.committed_offset {
                return Ok(incomplete_batch(previous, state, "provider_log_truncated"));
            }
            if !anchor_matches(&mut file, &state.continuity_anchor)? {
                return Ok(incomplete_batch(
                    previous,
                    state,
                    "provider_log_continuity_mismatch",
                ));
            }
            state
        }
        None => {
            let committed_offset = if trust_source_from_start { 0 } else { file_len };
            ProviderLogCaptureState {
                provider_source_key: provider_source_key.to_string(),
                path: canonical_path,
                native_identity,
                committed_offset,
                continuity_anchor: read_anchor(&mut file, committed_offset)?,
                normalizer: TranscriptNormalizationState::default(),
                status: if trust_source_from_start {
                    "pending".to_string()
                } else {
                    "complete".to_string()
                },
                reason: (!trust_source_from_start)
                    .then(|| "provider_log_prefix_policy_unknown".to_string()),
                unknown_before_offset: (!trust_source_from_start).then_some(file_len),
                policy_generation: 0,
                disabled_spans: Vec::new(),
                open_disabled_from: None,
            }
        }
    };

    if zero_length_policy_boundary_at_cursor(&state) {
        state.normalizer = TranscriptNormalizationState::default();
    }

    if let Some(disabled_end) = disabled_end_at_cursor(&state, file_len) {
        let skipped = disabled_end.saturating_sub(state.committed_offset);
        state.committed_offset = disabled_end;
        state.continuity_anchor = read_anchor(&mut file, disabled_end)?;
        state.normalizer = TranscriptNormalizationState::default();
        state.status = if state.committed_offset < file_len {
            "pending".to_string()
        } else {
            "complete".to_string()
        };
        state.reason = Some("provider_log_disabled_span_skipped".to_string());
        return Ok(ProviderLogBatch {
            events: Vec::new(),
            previous,
            next: state,
            consumed_bytes: skipped,
            continue_immediately: disabled_end < file_len,
        });
    }

    if state.committed_offset == file_len {
        state.status = if state.normalizer_has_pending_events() {
            "pending".to_string()
        } else {
            "complete".to_string()
        };
        if state.normalizer_has_pending_events() {
            state.reason = Some("provider_log_waiting_for_request_root".to_string());
        } else if state.unknown_before_offset.is_none() {
            state.reason = None;
        }
        return Ok(ProviderLogBatch {
            events: Vec::new(),
            previous,
            next: state,
            consumed_bytes: 0,
            continue_immediately: false,
        });
    }

    file.seek(SeekFrom::Start(state.committed_offset))?;
    let readable_end = next_disabled_start(&state, file_len)
        .unwrap_or(file_len)
        .min(file_len);
    let remaining = readable_end.saturating_sub(state.committed_offset);
    let read_len = remaining.min(PROVIDER_LOG_BATCH_BYTES);
    let mut bytes = Vec::with_capacity(read_len as usize);
    Read::by_ref(&mut file)
        .take(read_len)
        .read_to_end(&mut bytes)?;
    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        state.status = if remaining > PROVIDER_LOG_BATCH_BYTES {
            "incomplete".to_string()
        } else {
            "pending".to_string()
        };
        state.reason = Some(
            if remaining > PROVIDER_LOG_BATCH_BYTES {
                "provider_log_record_exceeds_batch_limit"
            } else {
                "provider_log_partial_record"
            }
            .to_string(),
        );
        return Ok(ProviderLogBatch {
            events: Vec::new(),
            previous,
            next: state,
            consumed_bytes: 0,
            continue_immediately: false,
        });
    };
    let complete_len = last_newline + 1;
    let complete = match std::str::from_utf8(&bytes[..complete_len]) {
        Ok(complete) => complete,
        Err(error) => {
            state.status = "incomplete".to_string();
            state.reason = Some(format!("provider_log_invalid_utf8: {error}"));
            return Ok(ProviderLogBatch {
                events: Vec::new(),
                previous,
                next: state,
                consumed_bytes: 0,
                continue_immediately: false,
            });
        }
    };
    if let Some((line_index, error)) = complete
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .find_map(|(index, line)| {
            serde_json::from_str::<serde_json::Value>(line)
                .err()
                .map(|error| (index, error))
        })
    {
        state.status = "incomplete".to_string();
        state.reason = Some(format!(
            "provider_log_invalid_json_line_{}: {error}",
            line_index + 1
        ));
        return Ok(ProviderLogBatch {
            events: Vec::new(),
            previous,
            next: state,
            consumed_bytes: 0,
            continue_immediately: false,
        });
    }

    let mut next_normalizer = state.normalizer.clone();
    let events = match normalize_chat_lines_with_state(
        session_id,
        provider,
        complete.lines(),
        &mut next_normalizer,
        false,
        true,
    ) {
        Ok(events) => events,
        Err(error) => {
            state.status = "incomplete".to_string();
            state.reason = Some(format!("provider_log_normalization_state_limit: {error}"));
            return Ok(ProviderLogBatch {
                events: Vec::new(),
                previous,
                next: state,
                consumed_bytes: 0,
                continue_immediately: false,
            });
        }
    };
    let consumed_bytes = complete_len as u64;
    state.committed_offset = state.committed_offset.saturating_add(consumed_bytes);
    state.continuity_anchor = read_anchor(&mut file, state.committed_offset)?;
    state.normalizer = next_normalizer;
    let unread_complete_bytes = state.committed_offset < file_len
        && bytes
            .get(complete_len..)
            .is_some_and(|tail| tail.contains(&b'\n'));
    let more_file_bytes = state.committed_offset < file_len;
    state.status = if more_file_bytes || state.normalizer_has_pending_events() {
        "pending".to_string()
    } else {
        "complete".to_string()
    };
    state.reason = if unread_complete_bytes || remaining > PROVIDER_LOG_BATCH_BYTES {
        Some("provider_log_batch_limit".to_string())
    } else if more_file_bytes {
        Some("provider_log_partial_record".to_string())
    } else if state.normalizer_has_pending_events() {
        Some("provider_log_waiting_for_request_root".to_string())
    } else {
        None
    };

    let policy_boundary_pending =
        next_disabled_start(&state, file_len).is_some_and(|start| start == state.committed_offset);
    Ok(ProviderLogBatch {
        events,
        previous,
        next: state,
        consumed_bytes,
        continue_immediately: unread_complete_bytes
            || remaining > PROVIDER_LOG_BATCH_BYTES
            || policy_boundary_pending,
    })
}

pub(crate) fn observe_provider_log_policy(
    path: &Path,
    provider_source_key: &str,
    previous: Option<ProviderLogCaptureState>,
    logging_enabled: bool,
    trust_source_from_start: bool,
) -> io::Result<ProviderLogBatch> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    let canonical_path = std::fs::canonicalize(path)?.to_string_lossy().to_string();
    let native_identity = native_file_identity(&file)?;
    let mut state = match previous.clone() {
        Some(state) => {
            let reason = if state.provider_source_key != provider_source_key
                || state.path != canonical_path
                || state.native_identity != native_identity
            {
                Some("provider_log_source_replaced")
            } else if file_len < state.committed_offset {
                Some("provider_log_truncated")
            } else if !anchor_matches(&mut file, &state.continuity_anchor)? {
                Some("provider_log_continuity_mismatch")
            } else {
                None
            };
            if let Some(reason) = reason {
                return Ok(incomplete_batch(previous, state, reason));
            }
            state
        }
        None => {
            // A source first observed while logging is disabled has no earlier
            // enabled interval to recover. Baseline it at EOF so its existing
            // prefix cannot be imported before the disabled span begins.
            // Existing enabled cursors still retain their backlog when a
            // later transition opens a disabled span.
            let prefix_policy_unknown = !trust_source_from_start || !logging_enabled;
            let committed_offset = if prefix_policy_unknown { file_len } else { 0 };
            ProviderLogCaptureState {
                provider_source_key: provider_source_key.to_string(),
                path: canonical_path,
                native_identity,
                committed_offset,
                continuity_anchor: read_anchor(&mut file, committed_offset)?,
                normalizer: TranscriptNormalizationState::default(),
                status: if committed_offset < file_len {
                    "pending".to_string()
                } else {
                    "complete".to_string()
                },
                reason: prefix_policy_unknown
                    .then(|| "provider_log_prefix_policy_unknown".to_string()),
                unknown_before_offset: prefix_policy_unknown.then_some(file_len),
                policy_generation: 0,
                disabled_spans: Vec::new(),
                open_disabled_from: None,
            }
        }
    };
    let changed = if logging_enabled {
        if let Some(start) = state.open_disabled_from.take() {
            if state.disabled_spans.len() >= MAX_PROVIDER_LOG_POLICY_SPANS {
                state.open_disabled_from = Some(start);
                return Ok(incomplete_batch(
                    previous,
                    state,
                    "provider_log_policy_span_limit",
                ));
            }
            state.disabled_spans.push(ProviderLogDisabledSpan {
                start,
                end: file_len,
            });
            true
        } else {
            false
        }
    } else if state.open_disabled_from.is_none() {
        state.open_disabled_from = Some(file_len);
        true
    } else {
        false
    };
    if changed {
        state.policy_generation = state.policy_generation.saturating_add(1);
    }
    state.status = if state.committed_offset < file_len {
        "pending".to_string()
    } else {
        "complete".to_string()
    };
    state.reason = if logging_enabled {
        (!state.disabled_spans.is_empty())
            .then(|| "provider_log_policy_boundary_recorded".to_string())
    } else {
        Some("provider_log_logging_disabled".to_string())
    };
    Ok(ProviderLogBatch {
        events: Vec::new(),
        previous,
        next: state,
        consumed_bytes: 0,
        continue_immediately: false,
    })
}

/// An optional provider log may not exist before its first observation, such
/// as for a mock runtime that only exposes terminal events. Once a source has
/// produced capture state, its disappearance remains an error so replacement
/// and deletion cannot be mistaken for an empty source.
pub(crate) fn observe_provider_log_policy_with_initial_absence(
    path: &Path,
    provider_source_key: &str,
    previous: Option<ProviderLogCaptureState>,
    logging_enabled: bool,
    trust_source_from_start: bool,
) -> io::Result<Option<ProviderLogBatch>> {
    match observe_provider_log_policy(
        path,
        provider_source_key,
        previous.clone(),
        logging_enabled,
        trust_source_from_start,
    ) {
        Ok(batch) => Ok(Some(batch)),
        Err(error) if previous.is_none() && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

impl ProviderLogCaptureState {
    fn normalizer_has_pending_events(&self) -> bool {
        self.normalizer.has_pending_events()
    }
}

fn disabled_end_at_cursor(state: &ProviderLogCaptureState, file_len: u64) -> Option<u64> {
    state
        .disabled_spans
        .iter()
        .find(|span| span.start <= state.committed_offset && state.committed_offset < span.end)
        .map(|span| span.end)
        .or_else(|| {
            state
                .open_disabled_from
                .filter(|start| *start <= state.committed_offset)
                .map(|_| file_len)
        })
}

fn zero_length_policy_boundary_at_cursor(state: &ProviderLogCaptureState) -> bool {
    state
        .disabled_spans
        .iter()
        .any(|span| span.start == state.committed_offset && span.end == state.committed_offset)
}

fn next_disabled_start(state: &ProviderLogCaptureState, file_len: u64) -> Option<u64> {
    state
        .disabled_spans
        .iter()
        .filter(|span| span.end > state.committed_offset)
        .map(|span| span.start)
        .chain(state.open_disabled_from)
        .filter(|start| *start >= state.committed_offset && *start <= file_len)
        .min()
}

fn incomplete_batch(
    previous: Option<ProviderLogCaptureState>,
    mut state: ProviderLogCaptureState,
    reason: &str,
) -> ProviderLogBatch {
    state.status = "incomplete".to_string();
    state.reason = Some(reason.to_string());
    ProviderLogBatch {
        events: Vec::new(),
        previous,
        next: state,
        consumed_bytes: 0,
        continue_immediately: false,
    }
}

fn read_anchor(file: &mut std::fs::File, offset: u64) -> io::Result<ProviderLogContinuityAnchor> {
    let start = offset.saturating_sub(PROVIDER_LOG_ANCHOR_BYTES);
    let len = offset.saturating_sub(start);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(len as usize);
    Read::by_ref(file).take(len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "provider log ended while reading continuity anchor",
        ));
    }
    Ok(ProviderLogContinuityAnchor {
        start,
        len,
        sha256: hex_sha256(&bytes),
    })
}

fn anchor_matches(
    file: &mut std::fs::File,
    expected: &ProviderLogContinuityAnchor,
) -> io::Result<bool> {
    file.seek(SeekFrom::Start(expected.start))?;
    let mut bytes = Vec::with_capacity(expected.len as usize);
    Read::by_ref(file)
        .take(expected.len)
        .read_to_end(&mut bytes)?;
    Ok(bytes.len() as u64 == expected.len && hex_sha256(&bytes) == expected.sha256)
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn native_file_identity(file: &std::fs::File) -> io::Result<ProviderLogNativeIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        return Ok(ProviderLogNativeIdentity {
            platform: "unix".to_string(),
            primary: metadata.dev(),
            secondary: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle as _;

        #[repr(C)]
        #[allow(non_snake_case)]
        struct FileTime {
            dwLowDateTime: u32,
            dwHighDateTime: u32,
        }
        #[repr(C)]
        #[allow(non_snake_case)]
        struct ByHandleFileInformation {
            dwFileAttributes: u32,
            ftCreationTime: FileTime,
            ftLastAccessTime: FileTime,
            ftLastWriteTime: FileTime,
            dwVolumeSerialNumber: u32,
            nFileSizeHigh: u32,
            nFileSizeLow: u32,
            nNumberOfLinks: u32,
            nFileIndexHigh: u32,
            nFileIndexLow: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                file: *mut c_void,
                information: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
        // SAFETY: `file` is an open OS handle and Windows initializes the full
        // output structure when the call reports success.
        let succeeded =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful call above initialized `information`.
        let information = unsafe { information.assume_init() };
        return Ok(ProviderLogNativeIdentity {
            platform: "windows".to_string(),
            primary: u64::from(information.dwVolumeSerialNumber),
            secondary: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        });
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "provider log native identity is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};

    #[test]
    fn forward_batches_keep_tool_relationships_across_a_two_mib_burst() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        let tool_call = r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"shell_command","call_id":"call-leading","input":{"command":"npm test"}}}"#;
        let filler = format!(r#"{{"type":"ignored","padding":"{}"}}"#, "x".repeat(1024));
        let tool_result = r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-leading","output":"tests passed"}}"#;
        let answer = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Finished after the burst"}]}}"#;
        let mut content = format!("{tool_call}\n");
        while content.len() <= 2 * 1024 * 1024 + filler.len() {
            content.push_str(&filler);
            content.push('\n');
        }
        content.push_str(tool_result);
        content.push('\n');
        content.push_str(answer);
        content.push('\n');
        std::fs::write(&path, content).expect("write oversized provider log");

        let mut previous = None;
        let mut events = Vec::new();
        loop {
            let batch = acquire_provider_log_batch(
                "agent-1",
                "codex",
                &path,
                "codex:session:one",
                previous,
                true,
            )
            .expect("acquire provider batch");
            let should_continue = batch.continue_immediately;
            events.extend(batch.events);
            previous = Some(batch.next);
            if !should_continue {
                break;
            }
        }

        assert!(events.iter().any(|event| {
            event.kind == AgentChatEventKind::ToolCall
                && event.turn_id.as_deref() == Some("call-leading")
        }));
        assert!(events.iter().any(|event| {
            event.kind == AgentChatEventKind::ToolResult
                && event.turn_id.as_deref() == Some("call-leading")
        }));
        assert!(events.iter().any(|event| {
            event.role == Some(AgentChatRole::Assistant)
                && event.text.as_deref() == Some("Finished after the burst")
        }));
        let state = previous.expect("final capture state");
        assert_eq!(state.status, "complete");
        assert_eq!(
            state.committed_offset,
            std::fs::metadata(path).unwrap().len()
        );
    }

    #[test]
    fn disabled_span_is_not_normalized_and_cannot_carry_pending_context_forward() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        let pending_context = r#"{"type":"response_item","payload":{"type":"message","id":"context-before-disable","role":"user","content":[{"type":"input_text","text":"Unresolved host context."}],"internal_chat_message_metadata_passthrough":{"turn_id":"codex-turn-old"}}}"#;
        std::fs::write(&path, format!("{pending_context}\n")).expect("write pending context");
        let pending =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("capture pending context");
        assert!(pending.events.is_empty());
        assert!(pending.next.normalizer_has_pending_events());

        let disabled = observe_provider_log_policy(
            &path,
            "codex:session:one",
            Some(pending.next),
            false,
            true,
        )
        .expect("open disabled span");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"SECRET_DISABLED\"}}}}")
            })
            .expect("append disabled content");
        let enabled = observe_provider_log_policy(
            &path,
            "codex:session:one",
            Some(disabled.next),
            true,
            true,
        )
        .expect("close disabled span");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"Visible request\"}}}}")?;
                writeln!(file, "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"Visible answer\"}}}}")
            })
            .expect("append re-enabled content");

        let skipped = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(enabled.next),
            true,
        )
        .expect("skip disabled span");
        assert!(skipped.events.is_empty());
        assert!(skipped.continue_immediately);
        assert!(!skipped.next.normalizer_has_pending_events());
        let visible = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(skipped.next),
            true,
        )
        .expect("capture re-enabled content");

        let text = visible
            .events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(text, vec!["Visible request", "Visible answer"]);
        assert!(visible.events.iter().all(|event| {
            event.metadata["request_root_id"] != "context-before-disable"
                && event.metadata["provider_turn_id"] != "codex-turn-old"
        }));
    }

    #[test]
    fn an_initially_absent_provider_log_is_optional_without_capture_state() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");

        let observed = observe_provider_log_policy_with_initial_absence(
            &path,
            "mock:session:one",
            None,
            true,
            true,
        )
        .expect("initially absent source is optional");

        assert!(observed.is_none());
    }

    #[test]
    fn a_missing_provider_log_after_observation_remains_an_error() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(&path, "{}\n").expect("write provider log");

        let observed = observe_provider_log_policy_with_initial_absence(
            &path,
            "codex:session:one",
            None,
            true,
            true,
        )
        .expect("observe existing source")
        .expect("existing source returns capture state");
        std::fs::remove_file(&path).expect("remove provider log");

        let error = observe_provider_log_policy_with_initial_absence(
            &path,
            "codex:session:one",
            Some(observed.next),
            true,
            true,
        )
        .expect_err("observed source disappearance must remain an error");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn initially_disabled_fresh_source_skips_its_existing_prefix() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"SECRET_BEFORE_FIRST_CAPTURE\"}}\n",
        )
        .expect("write pre-existing provider event");
        let existing_len = std::fs::metadata(&path).expect("provider metadata").len();

        let disabled = observe_provider_log_policy(&path, "codex:session:one", None, false, true)
            .expect("record initial disabled policy");
        let batch = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(disabled.next),
            true,
        )
        .expect("acquire while initially disabled");

        assert!(batch.events.is_empty());
        assert_eq!(batch.next.committed_offset, existing_len);
    }

    #[test]
    fn disable_then_enable_before_first_acquisition_skips_only_the_disabled_prefix() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"SECRET_BEFORE_FIRST_CAPTURE\"}}\n",
        )
        .expect("write pre-existing provider event");
        let disabled = observe_provider_log_policy(&path, "codex:session:one", None, false, true)
            .expect("record initial disabled policy");
        let enabled = observe_provider_log_policy(
            &path,
            "codex:session:one",
            Some(disabled.next),
            true,
            true,
        )
        .expect("re-enable before acquisition");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"Visible after enable\"}}}}")
            })
            .expect("append enabled provider event");

        let batch = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(enabled.next),
            true,
        )
        .expect("acquire after re-enable");

        let text = batch
            .events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(text, vec!["Visible after enable"]);
    }

    #[test]
    fn enabled_backlog_before_a_later_disable_remains_acquirable() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Enabled backlog\"}}\n",
        )
        .expect("write enabled backlog");
        let enabled = observe_provider_log_policy(&path, "codex:session:one", None, true, true)
            .expect("observe enabled fresh source");
        let disabled = observe_provider_log_policy(
            &path,
            "codex:session:one",
            Some(enabled.next),
            false,
            true,
        )
        .expect("disable after enabled backlog");

        let batch = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(disabled.next),
            true,
        )
        .expect("acquire enabled backlog");

        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].text.as_deref(), Some("Enabled backlog"));
    }

    #[test]
    fn zero_byte_disabled_interval_still_resets_pending_context() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"id\":\"old-context\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Old pending context\"}],\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"old-turn\"}}}\n",
        )
        .expect("write pending context");
        let pending =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("capture pending context");
        let disabled = observe_provider_log_policy(
            &path,
            "codex:session:one",
            Some(pending.next),
            false,
            true,
        )
        .expect("disable logging");
        let enabled = observe_provider_log_policy(
            &path,
            "codex:session:one",
            Some(disabled.next),
            true,
            true,
        )
        .expect("re-enable without intervening bytes");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"New request\"}}}}")
            })
            .expect("append new request");
        let batch = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(enabled.next),
            true,
        )
        .expect("capture after zero-byte disabled interval");

        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].text.as_deref(), Some("New request"));
        assert_ne!(batch.events[0].metadata["provider_turn_id"], "old-turn");
    }

    #[test]
    fn unknown_existing_prefix_is_skipped_without_guessing_context() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"id\":\"old-context\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Unknown old context\"}],\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"old-turn\"}}}\n",
        )
        .expect("write unknown prefix");
        let initial = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:resumed",
            None,
            false,
        )
        .expect("initialize unknown source");
        assert!(initial.events.is_empty());
        assert_eq!(
            initial.next.unknown_before_offset,
            Some(initial.next.committed_offset)
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"New request\"}}}}")
            })
            .expect("append new request");
        let next = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:resumed",
            Some(initial.next),
            false,
        )
        .expect("capture after unknown prefix");

        assert_eq!(next.events.len(), 1);
        assert_eq!(next.events[0].text.as_deref(), Some("New request"));
        assert_ne!(next.events[0].metadata["provider_turn_id"], "old-turn");
    }

    #[test]
    fn partial_line_waits_for_newline_without_advancing_past_it() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        let complete = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Complete request\"}}\n";
        let partial = "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"Partial";
        std::fs::write(&path, format!("{complete}{partial}"))
            .expect("write complete and partial records");
        let first =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("capture complete prefix");
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.next.status, "pending");
        assert_eq!(
            first.next.reason.as_deref(),
            Some("provider_log_partial_record")
        );
        assert_eq!(first.next.committed_offset, complete.len() as u64);

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, " answer\"}}}}")
            })
            .expect("finish partial record");
        let second = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(first.next),
            true,
        )
        .expect("capture completed record");
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].text.as_deref(), Some("Partial answer"));
        assert_eq!(second.next.status, "complete");
    }

    #[test]
    fn serialized_restart_state_keeps_explicit_tool_identity() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"name\":\"shell_command\",\"call_id\":\"call-before-restart\",\"input\":{\"command\":\"npm test\"}}}\n",
        )
        .expect("write tool call");
        let first =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("capture tool call");
        let persisted = serde_json::to_string(&first.next).expect("serialize capture state");
        let restored: ProviderLogCaptureState =
            serde_json::from_str(&persisted).expect("restore capture state");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"custom_tool_call_output\",\"call_id\":\"call-before-restart\",\"output\":\"passed\"}}}}")
            })
            .expect("append tool result");
        let second = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(restored),
            true,
        )
        .expect("resume after restart");

        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].kind, AgentChatEventKind::ToolResult);
        assert_eq!(
            second.events[0].turn_id.as_deref(),
            Some("call-before-restart")
        );
    }

    #[test]
    fn pending_normalization_state_limit_fails_without_cursor_progress() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        let content = (0..=256)
            .map(|index| {
                format!("{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"id\":\"context-{index}\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"context\"}}],\"internal_chat_message_metadata_passthrough\":{{\"turn_id\":\"turn-{index}\"}}}}}}")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&path, content).expect("write excessive pending context");
        let batch =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("state limit is represented as capture status");

        assert!(batch.events.is_empty());
        assert_eq!(batch.next.status, "incomplete");
        assert!(batch
            .next
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("pending-event limit exceeded")));
        assert_eq!(batch.next.committed_offset, 0);
    }

    #[test]
    fn tool_root_state_limit_fails_without_eviction_or_cursor_progress() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        let content = (0..=crate::providers::chat_transcript::MAX_TOOL_REQUEST_ROOTS)
            .map(|index| {
                format!(
                    "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"custom_tool_call\",\"name\":\"shell_command\",\"call_id\":\"call-{index}\",\"input\":{{\"command\":\"true\"}}}}}}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert!(content.len() < PROVIDER_LOG_BATCH_BYTES as usize);
        std::fs::write(&path, content).expect("write excessive tool roots");

        let batch =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("state limit is represented as capture status");

        assert!(batch.events.is_empty());
        assert_eq!(batch.next.status, "incomplete");
        assert!(batch
            .next
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("tool-root limit exceeded")));
        assert_eq!(batch.next.committed_offset, 0);
    }

    #[test]
    fn same_path_replacement_fails_closed_on_native_identity() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(&path, "{\"type\":\"ignored\"}\n").expect("write original");
        let first =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("first batch");

        let replacement = temp.path().join("replacement.jsonl");
        std::fs::write(&replacement, "{\"type\":\"ignored\"}\n").expect("write replacement");
        std::fs::remove_file(&path).expect("remove original");
        std::fs::rename(&replacement, &path).expect("replace at same path");
        let second = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(first.next),
            true,
        )
        .expect("replacement observation");

        assert_eq!(second.next.status, "incomplete");
        assert_eq!(
            second.next.reason.as_deref(),
            Some("provider_log_source_replaced")
        );
        assert_eq!(second.consumed_bytes, 0);
    }

    #[test]
    fn truncation_and_cursor_anchor_rewrite_fail_closed() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("provider.jsonl");
        std::fs::write(&path, "{\"type\":\"ignored\",\"value\":\"original\"}\n")
            .expect("write original");
        let first =
            acquire_provider_log_batch("agent-1", "codex", &path, "codex:session:one", None, true)
                .expect("first batch");
        let committed = first.next.clone();

        std::fs::write(&path, "{}\n").expect("truncate source");
        let truncated = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(committed.clone()),
            true,
        )
        .expect("truncation observation");
        assert_eq!(
            truncated.next.reason.as_deref(),
            Some("provider_log_truncated")
        );

        std::fs::write(&path, "{\"type\":\"ignored\",\"value\":\"rewritten\"}\n")
            .expect("rewrite source");
        let rewritten = acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            "codex:session:one",
            Some(committed),
            true,
        )
        .expect("rewrite observation");
        assert_eq!(
            rewritten.next.reason.as_deref(),
            Some("provider_log_continuity_mismatch")
        );
    }
}
