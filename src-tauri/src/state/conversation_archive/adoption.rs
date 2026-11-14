//! A fixed operator grant imports owned archive observations without granting
//! native capture, recent-source admission, or source renewal.
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use wardian_core::{
    control::{ArchiveAdoptionRequest, ArchiveAdoptionResponse, MessageOrigin, CONTROL_SCHEMA},
    conversations::{write_json_atomic, ConversationLoggingSetting, ConversationStatus},
};

use super::{
    storage, ConversationArchiveContext, ConversationArchiveState, ConversationCaptureState,
};
use crate::commands::provider_log_acquisition::ProviderLogCaptureState;
use crate::providers::chat_transcript::TranscriptNormalizationState;

#[cfg(test)]
mod replay_tests;
mod source;
#[cfg(test)]
mod tests;

const AUDIT_RECORD_BYTES: u64 = 64 * 1024;
const JOB_BYTES: u64 = 2 * 1024 * 1024;

/// Captured only while holding the global capture-policy gate. Source and
/// destination checks additionally run under the archive's per-agent gate.
#[derive(Clone)]
pub(crate) struct Admission {
    pub context: ConversationArchiveContext,
    pub enabled: bool,
    pub provider_session: Option<String>,
    pub cleared_sessions: Vec<String>,
    pub configuration: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Grant {
    schema: u8,
    request: ArchiveAdoptionRequest,
    agent_id: String,
    authorized_at: String,
    operator_os_identity: String,
    managed_actor: Option<MessageOrigin>,
    selection: source::Selection,
    restriction_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
struct AdoptionWork {
    schema: u8,
    grant: Grant,
    cursor: u64,
    normalizer: TranscriptNormalizationState,
    status: String,
    reason: Option<String>,
    completed_at: Option<String>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn directory(agent: &str) -> io::Result<PathBuf> {
    wardian_core::paths::agent_conversations_dir(agent)
        .and_then(|path| path.parent().map(|path| path.join("archive-adoptions")))
        .ok_or_else(|| invalid("unsafe archive adoption agent"))
}

fn job_path(agent: &str, id: &str) -> io::Result<PathBuf> {
    uuid::Uuid::parse_str(id).map_err(|_| invalid("authorization_id must be a UUID"))?;
    Ok(directory(agent)?.join(format!("{id}.json")))
}

fn bounded_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> io::Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("archive adoption document exceeds limit"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn current_source<'a>(
    capture: &'a ConversationCaptureState,
    admission: &Admission,
) -> io::Result<&'a ProviderLogCaptureState> {
    let mut sources = capture.provider_log_sources.iter().filter(|source| {
        Some(&source.provider_source_key) == admission.context.provider_source_key.as_ref()
    });
    let source = sources
        .next()
        .ok_or_else(|| invalid("original capture receipt missing"))?;
    if sources.next().is_some() {
        return Err(invalid("ambiguous original source receipt"));
    }
    Ok(source)
}

fn restrictions(
    capture: &ConversationCaptureState,
    admission: &Admission,
    request: &ArchiveAdoptionRequest,
) -> io::Result<String> {
    if !admission.enabled
        || admission.context.provider != "codex"
        || admission.provider_session.as_deref() != Some(request.provider_session.as_str())
        || !admission
            .context
            .provider_session_ids
            .contains(&request.provider_session)
        || admission
            .cleared_sessions
            .contains(&request.provider_session)
        || request.start_offset >= request.end_offset
    {
        return Err(invalid(
            "archive adoption logging/session/interval is not permitted",
        ));
    }
    // There is no timestamp at the old byte anchor. Opaque event exclusions
    // likewise cannot be translated to positions in a replacement file.
    if capture.skip_events_at_or_before.is_some()
        || !capture.skip_event_ids.is_empty()
        || capture.skip_event_scopes.iter().any(|scope| {
            (scope.provider_source_key.is_none()
                || scope.provider_source_key == admission.context.provider_source_key)
                && (scope.skip_events_at_or_before.is_some()
                    || !scope.event_ids.is_empty()
                    || scope.disabled_from.is_some()
                    || scope.disabled_until.is_some())
        })
    {
        return Err(invalid(
            "archive adoption cannot locate time/identity restrictions before the start",
        ));
    }
    let source = current_source(capture, admission)?;
    let same_path = source.path == request.source_path
        || std::fs::canonicalize(&source.path)
            .ok()
            .zip(std::fs::canonicalize(&request.source_path).ok())
            .is_some_and(|(old, selected)| old == selected);
    if !same_path
        || source.committed_offset != request.start_offset
        || source
            .continuity_anchor
            .start
            .checked_add(source.continuity_anchor.len)
            != Some(request.start_offset)
        || source.reason.as_deref() != Some("provider_log_source_replaced")
        || source.unknown_before_offset.unwrap_or(0) > request.start_offset
        || source.open_disabled_from.is_some()
        || source.disabled_spans.iter().any(|span| {
            span.start >= request.start_offset
                || span.end > request.start_offset
                || span.end < span.start
        })
    {
        return Err(invalid(
            "archive adoption has a changed boundary or later byte restriction",
        ));
    }
    let bytes = serde_json::to_vec(&serde_json::json!({
        "configuration": admission.configuration,
        "agent": admission.context.agent_id, "provider": admission.context.provider,
        "sessions": admission.context.provider_session_ids, "source_key": admission.context.provider_source_key,
        "cutoff": capture.skip_events_at_or_before, "ids": capture.skip_event_ids,
        "scopes": capture.skip_event_scopes,
        "native_identity": source.native_identity, "anchor": source.continuity_anchor,
        "cursor": source.committed_offset, "generation": source.policy_generation,
        "unknown_before": source.unknown_before_offset,
        "disabled": source.disabled_spans, "open_disabled": source.open_disabled_from,
    })).map_err(io::Error::other)?;
    Ok(super::chat_read_store::digest(&bytes))
}

fn current_fingerprint(
    capture: &ConversationCaptureState,
    admission: &Admission,
    request: &ArchiveAdoptionRequest,
) -> io::Result<String> {
    let restrictions = restrictions(capture, admission, request)?;
    let source = current_source(capture, admission)?;
    let path = directory(&admission.context.agent_id)?.with_file_name("chat-source-policy.json");
    let mut bytes = Vec::new();
    File::open(path)?
        .take(super::chat_read::POLICY_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > super::chat_read::POLICY_BYTES {
        return Err(invalid("archive adoption source policy exceeds limit"));
    }
    let policy: Option<super::chat_read::SourcePolicy> =
        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let policy = policy.ok_or_else(|| invalid("archive adoption source policy is fenced"))?;
    if policy.agent_id != admission.context.agent_id
        || policy.source_key != source.provider_source_key
        || policy.identity != source.native_identity
        || policy.anchor != source.continuity_anchor
        || policy.generation != source.policy_generation
        || policy.unknown_before != source.unknown_before_offset.unwrap_or(0)
        || policy.disabled != source.disabled_spans
        || policy.open_disabled != source.open_disabled_from
        || policy
            .blocked_reason
            .as_deref()
            .is_some_and(|reason| reason != "provider_log_source_replaced")
    {
        return Err(invalid(
            "archive adoption source policy disagrees with original privacy receipt",
        ));
    }
    Ok(super::chat_read_store::digest(
        &serde_json::to_vec(&(restrictions, super::chat_read_store::digest(&bytes)))
            .map_err(io::Error::other)?,
    ))
}

#[derive(Serialize, Deserialize)]
struct Audit {
    event: String,
    outcome: String,
    grant: Grant,
}

#[derive(Serialize, Deserialize)]
struct Scheduling {
    authorization_id: String,
    pending: bool,
}

fn history_path(agent: &str) -> io::Result<PathBuf> {
    Ok(directory(agent)?.with_file_name("archive-adoption-history.jsonl"))
}

fn latest_grant(agent: &str) -> io::Result<Option<Grant>> {
    let mut file = match File::open(history_path(agent)?) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let length = file.metadata()?.len();
    if length == 0 {
        return Ok(None);
    }
    let offset = length.saturating_sub(AUDIT_RECORD_BYTES + 1);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(AUDIT_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if !bytes.ends_with(b"\n") {
        return Err(invalid("incomplete archive adoption audit tail"));
    }
    bytes.pop();
    let start = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => index + 1,
        None if offset == 0 => 0,
        None => return Err(invalid("archive adoption audit record exceeds limit")),
    };
    let record: Audit = serde_json::from_slice(&bytes[start..]).map_err(io::Error::other)?;
    if record.event != "archive_adoption_authorized"
        || record.outcome != "authorization_only"
        || record.grant.schema != 1
        || record.grant.agent_id != agent
        || record.grant.selection.start != record.grant.request.start_offset
        || record.grant.selection.end > record.grant.request.end_offset
        || record.grant.request.end_offset > record.grant.selection.observed_eof
        || record.grant.selection.native_identity.platform
            != record.grant.request.native_identity.platform
        || record.grant.selection.native_identity.primary
            != record.grant.request.native_identity.primary
        || record.grant.selection.native_identity.secondary
            != record.grant.request.native_identity.secondary
    {
        return Err(invalid("unrecognized archive adoption audit"));
    }
    Ok(Some(record.grant))
}

fn pending_grant(agent: &str) -> io::Result<Option<Grant>> {
    let Some(grant) = latest_grant(agent)? else {
        return Ok(None);
    };
    let schedule = bounded_json::<Scheduling>(&directory(agent)?.join("schedule.json"), 1024);
    match schedule {
        Ok(schedule)
            if schedule.authorization_id == grant.request.authorization_id && !schedule.pending =>
        {
            Ok(None)
        }
        Ok(_) => Ok(Some(grant)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Some(grant)),
        Err(error) => Err(error),
    }
}

fn schedule(grant: &Grant, pending: bool) -> io::Result<()> {
    write_json_atomic(
        &directory(&grant.agent_id)?.join("schedule.json"),
        &Scheduling {
            authorization_id: grant.request.authorization_id.clone(),
            pending,
        },
    )
}

fn sync_authorization(grant: &Grant) -> io::Result<()> {
    // A previous attempt may have written a complete audit record but failed
    // its sync. Recovery must establish durability before creating/executing a
    // job, rather than treating readable bytes as a successful earlier sync.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(history_path(&grant.agent_id)?)?
        .sync_all()
}

fn append_audit(grant: &Grant) -> io::Result<()> {
    // The bounded final record must be complete before appending. A torn audit
    // fails closed; it is never silently truncated to manufacture authority.
    latest_grant(&grant.agent_id)?;
    let mut bytes = serde_json::to_vec(&Audit {
        event: "archive_adoption_authorized".into(),
        outcome: "authorization_only".into(),
        grant: grant.clone(),
    })
    .map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > AUDIT_RECORD_BYTES {
        return Err(invalid("archive adoption grant exceeds audit record limit"));
    }
    let path = history_path(&grant.agent_id)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| invalid("audit parent missing"))?,
    )?
    .sync_all()?;
    Ok(())
}

fn destination(admission: &Admission, request: &ArchiveAdoptionRequest) -> io::Result<()> {
    let manifest = storage::read_manifest(
        &storage::conversation_dir(&admission.context.agent_id, &request.conversation_id)?
            .join("manifest.json"),
    )?
    .ok_or_else(|| invalid("archive adoption destination missing"))?;
    if manifest.agent_id != admission.context.agent_id
        || manifest.conversation_id != request.conversation_id
        || manifest.provider != "codex"
        || manifest.provider_source_key != admission.context.provider_source_key
        || manifest.provider_session_ids != admission.context.provider_session_ids
        || manifest.status != ConversationStatus::Open
        || manifest.effective_logging != ConversationLoggingSetting::Enabled
    {
        return Err(invalid("archive adoption destination scope changed"));
    }
    Ok(())
}

/// Only the grant owner can construct this capability, after validating the
/// audit, current restrictions and exact source bytes under archive exclusion.
pub(super) struct ArchiveAdoptionAuthority {
    conversation_id: String,
    rows: Vec<wardian_core::models::chat::AgentChatEvent>,
}

impl ArchiveAdoptionAuthority {
    pub(super) fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
    pub(super) fn proves(&self, row: &wardian_core::models::chat::AgentChatEvent) -> bool {
        self.rows.iter().any(|expected| expected == row)
    }
}

impl ConversationArchiveState {
    pub(crate) fn retire_pending_adoption(&self, agent: &str, reason: &str) -> io::Result<()> {
        let lock = storage::agent_lock_for(&self.agent_locks, agent)?;
        let _guard = storage::lock_agent_archive(&lock)?;
        self.retire_adoption_locked(agent, reason)
    }

    pub(super) fn retire_adoption_locked(&self, agent: &str, reason: &str) -> io::Result<()> {
        let Some(grant) = pending_grant(agent)? else {
            return Ok(());
        };
        let path = job_path(agent, &grant.request.authorization_id)?;
        let mut work = match bounded_json::<AdoptionWork>(&path, JOB_BYTES) {
            Ok(work) if work.grant == grant => work,
            Ok(_) => return Err(invalid("job grant conflicts with operator audit")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => AdoptionWork {
                schema: 1,
                cursor: grant.selection.start,
                grant: grant.clone(),
                normalizer: TranscriptNormalizationState::default(),
                status: "pending".into(),
                reason: None,
                completed_at: None,
            },
            Err(error) => return Err(error),
        };
        if work.status == "pending" {
            work.status = "retired".into();
            work.reason = Some(reason.into());
            write_json_atomic(&path, &work)?;
        }
        schedule(&grant, false)
    }
    /// Read scheduling demand before acquiring live roster/config locks. The
    /// audit tail and scheduling receipt are bounded, including after restart.
    pub(crate) fn has_pending_adoption(&self, agent: &str) -> io::Result<bool> {
        Ok(pending_grant(agent)?.is_some())
    }

    pub(crate) fn authorize_adoption(
        &self,
        admission: &Admission,
        request: ArchiveAdoptionRequest,
        origin: Option<MessageOrigin>,
        issuer: String,
    ) -> io::Result<ArchiveAdoptionResponse> {
        let lock = storage::agent_lock_for(&self.agent_locks, &admission.context.agent_id)?;
        let _guard = storage::lock_agent_archive(&lock)?;
        let path = job_path(&admission.context.agent_id, &request.authorization_id)?;
        if issuer.trim().is_empty()
            || request.approval_reference.trim().is_empty()
            || request.approval_text.trim().is_empty()
        {
            return Err(invalid(
                "operator identity and recorded user approval are required",
            ));
        }
        let capture = storage::read_capture_state(&admission.context.agent_id)?;
        let fingerprint = current_fingerprint(&capture, admission, &request)?;
        destination(admission, &request)?;
        // Repeating the current authorization resumes creation after an audit
        // sync succeeded but the following job write did not.
        let latest = latest_grant(&admission.context.agent_id)?;
        let grant = if let Some(grant) =
            latest.filter(|grant| grant.request.authorization_id == request.authorization_id)
        {
            if grant.request != request || grant.agent_id != admission.context.agent_id {
                return Err(invalid("authorization ID reused for another scope"));
            }
            grant
        } else {
            if pending_grant(&admission.context.agent_id)?.is_some() {
                return Err(invalid("another archive adoption is pending"));
            }
            // An older authorization ID may not be reused after a subsequent
            // job. Existing immutable job evidence makes that conflict visible.
            if path.exists() {
                return Err(invalid("authorization ID already used"));
            }
            let previous = current_source(&capture, admission)?;
            let selection = source::prepare(
                Path::new(&request.source_path),
                &request.provider_session,
                request.start_offset,
                request.end_offset,
                &previous.continuity_anchor,
            )?;
            if selection.native_identity.platform != request.native_identity.platform
                || selection.native_identity.primary != request.native_identity.primary
                || selection.native_identity.secondary != request.native_identity.secondary
                || selection.native_identity == previous.native_identity
            {
                return Err(invalid(
                    "replacement native identity does not match authorization",
                ));
            }
            let grant = Grant {
                schema: 1,
                request,
                agent_id: admission.context.agent_id.clone(),
                authorized_at: chrono::Utc::now().to_rfc3339(),
                operator_os_identity: issuer,
                managed_actor: origin,
                selection,
                restriction_fingerprint: fingerprint,
            };
            append_audit(&grant)?;
            #[cfg(test)]
            self.adoption_fault(1)?;
            grant
        };
        sync_authorization(&grant)?;
        std::fs::create_dir_all(directory(&grant.agent_id)?)?;
        let work = match bounded_json::<AdoptionWork>(&path, JOB_BYTES) {
            Ok(work) => {
                if work.grant != grant {
                    return Err(invalid("job grant conflicts with operator audit"));
                }
                work
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let work = AdoptionWork {
                    schema: 1,
                    cursor: grant.selection.start,
                    grant: grant.clone(),
                    normalizer: TranscriptNormalizationState::default(),
                    status: "pending".into(),
                    reason: None,
                    completed_at: None,
                };
                write_json_atomic(&path, &work)?;
                work
            }
            Err(error) => return Err(error),
        };
        schedule(&grant, work.status == "pending")?;
        Ok(ArchiveAdoptionResponse {
            schema: CONTROL_SCHEMA,
            ok: true,
            agent_id: grant.agent_id,
            authorization_id: grant.request.authorization_id,
            conversation_id: grant.request.conversation_id,
            start_offset: grant.selection.start,
            end_offset: grant.selection.end,
            status: work.status,
        })
    }

    /// One bounded step under policy -> per-agent archive -> projection order.
    /// An old native replacement stop never blocks this independent work kind.
    pub(crate) fn advance_adoption(&self, admission: &Admission) -> io::Result<bool> {
        let lock = storage::agent_lock_for(&self.agent_locks, &admission.context.agent_id)?;
        let _guard = storage::lock_agent_archive(&lock)?;
        let Some(grant) = pending_grant(&admission.context.agent_id)? else {
            return Ok(false);
        };
        sync_authorization(&grant)?;
        let path = job_path(&grant.agent_id, &grant.request.authorization_id)?;
        let mut work = match bounded_json::<AdoptionWork>(&path, JOB_BYTES) {
            Ok(work) => work,
            Err(error) if error.kind() == io::ErrorKind::NotFound => AdoptionWork {
                schema: 1,
                cursor: grant.selection.start,
                grant: grant.clone(),
                normalizer: TranscriptNormalizationState::default(),
                status: "pending".into(),
                reason: None,
                completed_at: None,
            },
            Err(error) => return Err(error),
        };
        if work.schema != 1
            || work.grant != grant
            || work.cursor < grant.selection.start
            || work.cursor > grant.selection.end
        {
            return Err(invalid("invalid archive adoption checkpoint"));
        }
        if work.status != "pending" {
            schedule(&grant, false)?;
            return Ok(false);
        }
        let check = || -> io::Result<()> {
            let capture = storage::read_capture_state(&grant.agent_id)?;
            if current_fingerprint(&capture, admission, &grant.request)?
                != grant.restriction_fingerprint
            {
                return Err(invalid(
                    "archive adoption restriction/configuration changed",
                ));
            }
            destination(admission, &grant.request)?;
            source::validate(&grant.selection, &grant.request.provider_session)?;
            Ok(())
        };
        if let Err(error) = check() {
            return retire_work(&path, &mut work, error);
        }
        let mut batch = match source::advance(
            &grant.selection,
            &grant.agent_id,
            &grant.request.provider_session,
            work.cursor,
            &work.normalizer,
        ) {
            Ok(batch) => batch,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                return retire_work(&path, &mut work, error);
            }
            Err(error) => return Err(error),
        };
        #[cfg(test)]
        self.replace_adoption_source_for_test(&grant.selection.path)?;
        if let Err(error) = check() {
            return retire_work(&path, &mut work, error);
        }
        for event in &mut batch.events {
            event.metadata["archive_adoption"]["authorization_id"] =
                serde_json::json!(grant.request.authorization_id);
        }
        let authority = ArchiveAdoptionAuthority {
            conversation_id: grant.request.conversation_id.clone(),
            rows: batch.events.clone(),
        };
        let (_, candidate) = self.append_chat_events_prepared_locked(
            admission.context.clone(),
            &batch.events,
            None,
            None,
            None,
            Some(&authority),
        )?;
        if let Some(candidate) = candidate {
            self.publish_chat_candidate(candidate)?;
        }
        // append_chat_events_prepared_locked owns artifacts and repairs a torn
        // event/narrative pair before this independent cursor can advance.
        sync_archive(&grant)?;
        #[cfg(test)]
        self.adoption_fault(4)?;
        work.cursor = batch.next_cursor;
        work.normalizer = batch.normalizer;
        if work.cursor == grant.selection.end && !work.normalizer.has_pending_events() {
            work.status = "complete".into();
            work.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
        write_json_atomic(&path, &work)?;
        schedule(&grant, work.status == "pending")?;
        // Completion still schedules the existing derived archive handoff.
        Ok(true)
    }
}

fn retire_work(path: &Path, work: &mut AdoptionWork, error: io::Error) -> io::Result<bool> {
    work.status = "retired".into();
    work.reason = Some(error.to_string());
    write_json_atomic(path, work)?;
    schedule(&work.grant, false)?;
    Ok(false)
}

pub(super) fn sync_record_bodies(
    directory: &Path,
    record: &wardian_core::conversations::ConversationNarrativeRecord,
) -> io::Result<()> {
    let artifacts = directory.join("artifacts");
    for name in &record.artifact_refs {
        // Owned artifacts are single filenames, never live provider paths.
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(invalid("unsafe adopted body artifact"));
        }
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(artifacts.join(name))?
            .sync_all()?;
    }
    #[cfg(unix)]
    if artifacts.exists() {
        File::open(artifacts)?.sync_all()?;
    }
    Ok(())
}

fn sync_archive(grant: &Grant) -> io::Result<()> {
    let directory = storage::conversation_dir(&grant.agent_id, &grant.request.conversation_id)?;
    for name in [
        "events.jsonl",
        "conversation.jsonl",
        "sources.jsonl",
        "turns.jsonl",
        "manifest.json",
    ] {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.join(name))
        {
            Ok(file) => file.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(storage::index_path(&grant.agent_id)?)?
        .sync_all()?;
    #[cfg(unix)]
    File::open(&directory)?.sync_all()?;
    Ok(())
}
