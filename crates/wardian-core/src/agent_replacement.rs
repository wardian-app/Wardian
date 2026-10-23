use crate::atomic_file::write_json_atomic;
use crate::conversations::{read_jsonl_records, ConversationIndexEntry, ConversationStatus};
use crate::db::{self, AgentUpsert};
use crate::models::AgentConfig;
use crate::paths::{agent_conversations_dir, state_db_path, wardian_home};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

const JOURNAL_SCHEMA: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementPhase {
    Prepared,
    StatePersisted,
    MetadataPersisted,
    BoundaryCommitting,
    BoundaryCommitted,
    LiveInstalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAgentReplacement {
    pub schema: u8,
    pub generation_id: String,
    pub operation: String,
    pub session_id: String,
    pub original_config: AgentConfig,
    pub replacement_config: AgentConfig,
    pub original_created_at: Option<String>,
    pub replacement_created_at: Option<String>,
    pub archive_conversation_id: Option<String>,
    pub archive_expected: bool,
    pub session_close_intent: Option<SessionCloseIntent>,
    pub phase: ReplacementPhase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCloseIntent {
    pub boundary_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub workspace: String,
    pub provider: String,
    pub boundary_reason: String,
    pub archive_available: bool,
    pub conversation_id: Option<String>,
    pub source_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredAgentReplacement {
    pub session_id: String,
    pub generation_id: String,
    pub session_close_intent: Option<SessionCloseIntent>,
}

impl PendingAgentReplacement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation: impl Into<String>,
        session_id: impl Into<String>,
        original_config: AgentConfig,
        replacement_config: AgentConfig,
        original_created_at: Option<String>,
        replacement_created_at: Option<String>,
        archive_conversation_id: Option<String>,
        archive_expected: bool,
    ) -> Self {
        Self {
            schema: JOURNAL_SCHEMA,
            generation_id: uuid::Uuid::new_v4().to_string(),
            operation: operation.into(),
            session_id: session_id.into(),
            original_config,
            replacement_config,
            original_created_at,
            replacement_created_at,
            archive_conversation_id,
            archive_expected,
            session_close_intent: None,
            phase: ReplacementPhase::Prepared,
        }
    }
}

pub struct ReplacementJournalGuard {
    _lock: AgentRosterBarrier,
    path: PathBuf,
    record: PendingAgentReplacement,
    completed: bool,
}

impl ReplacementJournalGuard {
    pub fn begin(record: PendingAgentReplacement, barrier: AgentRosterBarrier) -> io::Result<Self> {
        let path = journal_path(&record.session_id)?;
        write_json_atomic(&path, &record)?;
        Ok(Self {
            _lock: barrier,
            path,
            record,
            completed: false,
        })
    }

    pub fn advance(&mut self, phase: ReplacementPhase) -> io::Result<()> {
        self.record.phase = phase;
        write_json_atomic(&self.path, &self.record)
    }

    pub fn generation_id(&self) -> &str {
        &self.record.generation_id
    }

    pub fn session_close_intent(&self) -> Option<&SessionCloseIntent> {
        self.record.session_close_intent.as_ref()
    }

    pub fn set_session_close_intent(&mut self, intent: SessionCloseIntent) -> io::Result<()> {
        self.record.session_close_intent = Some(intent);
        write_json_atomic(&self.path, &self.record)
    }

    pub fn complete(mut self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => {
                self.completed = true;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.completed = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryStatus {
    Idle,
    Busy,
    Recovered(Vec<RecoveredAgentReplacement>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingReplacementStatus {
    None,
    Busy,
    Pending(Vec<String>),
}

pub struct AgentRosterBarrier(File);

impl Drop for AgentRosterBarrier {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub fn acquire_agent_roster_barrier(wait: bool) -> io::Result<Option<AgentRosterBarrier>> {
    acquire_lock(wait).map(|lock| lock.map(AgentRosterBarrier))
}

pub fn recover_pending_replacements(wait_for_lock: bool) -> io::Result<RecoveryStatus> {
    let Some(lock) = acquire_agent_roster_barrier(wait_for_lock)? else {
        return Ok(RecoveryStatus::Busy);
    };
    let directory = journal_dir()?;
    if !directory.exists() {
        drop(lock);
        return Ok(RecoveryStatus::Idle);
    }

    let paths = journal_paths(&directory)?;
    let mut recovered = Vec::new();
    for path in paths {
        let content = fs::read_to_string(&path)?;
        let record: PendingAgentReplacement =
            serde_json::from_str(&content).map_err(io::Error::other)?;
        if record.schema != JOURNAL_SCHEMA {
            return Err(io::Error::other(format!(
                "unsupported agent replacement journal schema {}",
                record.schema
            )));
        }
        let roll_forward = recover_record(&record)?;
        let session_close_intent = roll_forward
            .then(|| record.session_close_intent.clone())
            .flatten();
        if session_close_intent.is_none() {
            fs::remove_file(&path)?;
        }
        recovered.push(RecoveredAgentReplacement {
            session_id: record.session_id.clone(),
            generation_id: record.generation_id.clone(),
            session_close_intent,
        });
    }
    drop(lock);
    if recovered.is_empty() {
        Ok(RecoveryStatus::Idle)
    } else {
        Ok(RecoveryStatus::Recovered(recovered))
    }
}

pub fn complete_recovered_replacement(session_id: &str, generation_id: &str) -> io::Result<bool> {
    let Some(_lock) = acquire_agent_roster_barrier(true)? else {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "agent roster barrier is unavailable",
        ));
    };
    let path = journal_path(session_id)?;
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(&path)?;
    let current: PendingAgentReplacement =
        serde_json::from_str(&content).map_err(io::Error::other)?;
    if current.generation_id != generation_id {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn pending_replacement_status() -> io::Result<PendingReplacementStatus> {
    let Some(_lock) = acquire_agent_roster_barrier(false)? else {
        return Ok(PendingReplacementStatus::Busy);
    };
    let directory = journal_dir()?;
    if !directory.exists() {
        return Ok(PendingReplacementStatus::None);
    }
    let mut session_ids = Vec::new();
    for path in journal_paths(&directory)? {
        let content = fs::read_to_string(path)?;
        let record: PendingAgentReplacement =
            serde_json::from_str(&content).map_err(io::Error::other)?;
        session_ids.push(record.session_id);
    }
    if session_ids.is_empty() {
        Ok(PendingReplacementStatus::None)
    } else {
        Ok(PendingReplacementStatus::Pending(session_ids))
    }
}

fn recover_record(record: &PendingAgentReplacement) -> io::Result<bool> {
    let roll_forward = match record.phase {
        ReplacementPhase::BoundaryCommitted | ReplacementPhase::LiveInstalled => true,
        ReplacementPhase::BoundaryCommitting => {
            record.archive_expected && archived_boundary_is_closed(record)?
        }
        ReplacementPhase::Prepared
        | ReplacementPhase::StatePersisted
        | ReplacementPhase::MetadataPersisted => false,
    };
    let (config, created_at) = if roll_forward {
        (
            &record.replacement_config,
            record.replacement_created_at.as_deref(),
        )
    } else {
        (
            &record.original_config,
            record.original_created_at.as_deref(),
        )
    };

    let current = current_state_config(&record.session_id)?;
    if !configs_match(&current, &record.original_config)
        && !configs_match(&current, &record.replacement_config)
    {
        return Err(io::Error::other(format!(
            "agent {} changed after its replacement journal was written",
            record.session_id
        )));
    }

    replace_config_in_state(config)?;
    persist_config(config, created_at)?;
    Ok(roll_forward)
}

fn current_state_config(session_id: &str) -> io::Result<AgentConfig> {
    let home = wardian_home()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Wardian home is unavailable"))?;
    let content = fs::read_to_string(home.join("settings").join("state.json"))?;
    let configs: Vec<AgentConfig> = serde_json::from_str(&content).map_err(io::Error::other)?;
    configs
        .into_iter()
        .find(|config| config.session_id == session_id)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("agent {session_id} is missing from state.json"),
            )
        })
}

fn configs_match(left: &AgentConfig, right: &AgentConfig) -> bool {
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}

fn archived_boundary_is_closed(record: &PendingAgentReplacement) -> io::Result<bool> {
    let Some(conversation_id) = record.archive_conversation_id.as_deref() else {
        return Ok(false);
    };
    let Some(directory) = agent_conversations_dir(&record.session_id) else {
        return Ok(false);
    };
    let index_path = directory.join("index.jsonl");
    if !index_path.exists() {
        return Ok(false);
    }
    let entries: Vec<ConversationIndexEntry> = read_jsonl_records(&index_path)?;
    Ok(entries.iter().rev().any(|entry| {
        entry.conversation_id == conversation_id && entry.status != ConversationStatus::Open
    }))
}

fn replace_config_in_state(config: &AgentConfig) -> io::Result<()> {
    let home = wardian_home()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Wardian home is unavailable"))?;
    let path = home.join("settings").join("state.json");
    let content = fs::read_to_string(&path)?;
    let mut configs: Vec<AgentConfig> = serde_json::from_str(&content).map_err(io::Error::other)?;
    let Some(existing) = configs
        .iter_mut()
        .find(|existing| existing.session_id == config.session_id)
    else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("agent {} is missing from state.json", config.session_id),
        ));
    };
    *existing = config.clone();
    write_json_atomic(&path, &configs)
}

fn persist_config(config: &AgentConfig, created_at: Option<&str>) -> io::Result<()> {
    let db_path = state_db_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "state.db is unavailable"))?;
    let connection = rusqlite::Connection::open(db_path).map_err(io::Error::other)?;
    let workspace = (!config.folder.trim().is_empty()).then_some(config.folder.as_str());
    let project = workspace.and_then(db::project_name_from_workspace);
    db::upsert_agent_with_conn(
        &connection,
        &AgentUpsert {
            session_id: &config.session_id,
            session_name: &config.session_name,
            description: &config.description,
            agent_class: &config.agent_class,
            provider: &config.provider,
            workspace,
            project: project.as_deref(),
            is_off: config.is_off,
            created_at,
        },
    )
    .map_err(io::Error::other)
}

fn acquire_lock(wait: bool) -> io::Result<Option<File>> {
    let home = wardian_home()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Wardian home is unavailable"))?;
    let settings = home.join("settings");
    fs::create_dir_all(&settings)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(settings.join("agent-replacements.lock"))?;
    if wait {
        FileExt::lock_exclusive(&file)?;
        return Ok(Some(file));
    }
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                || matches!(error.raw_os_error(), Some(11 | 32 | 33 | 35)) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn journal_dir() -> io::Result<PathBuf> {
    let home = wardian_home()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Wardian home is unavailable"))?;
    Ok(home.join("settings").join("agent-replacements"))
}

fn journal_path(session_id: &str) -> io::Result<PathBuf> {
    let directory = journal_dir()?;
    fs::create_dir_all(&directory)?;
    let digest = Sha256::digest(session_id.as_bytes());
    Ok(directory.join(format!("{:x}.json", digest)))
}

fn journal_paths(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin_with_barrier(record: PendingAgentReplacement) -> io::Result<ReplacementJournalGuard> {
        let barrier = acquire_agent_roster_barrier(true)?
            .expect("blocking roster barrier acquisition returns a barrier");
        ReplacementJournalGuard::begin(record, barrier)
    }

    fn set_state(home: &Path, config: &AgentConfig) {
        fs::create_dir_all(home.join("settings")).unwrap();
        write_json_atomic(
            &home.join("settings").join("state.json"),
            std::slice::from_ref(config),
        )
        .unwrap();
    }

    #[test]
    fn prepared_journal_recovers_original_identity() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let original = AgentConfig {
            session_id: "agent-1".into(),
            session_name: "Original".into(),
            folder: home.path().to_string_lossy().to_string(),
            is_off: true,
            ..Default::default()
        };
        let replacement = AgentConfig {
            session_name: "Replacement".into(),
            is_off: false,
            ..original.clone()
        };
        set_state(home.path(), &replacement);
        let guard = begin_with_barrier(PendingAgentReplacement::new(
            "fresh_resume",
            "agent-1",
            original.clone(),
            replacement,
            None,
            None,
            None,
            false,
        ))
        .unwrap();
        let generation_id = guard.generation_id().to_string();
        drop(guard);

        assert_eq!(
            recover_pending_replacements(true).unwrap(),
            RecoveryStatus::Recovered(vec![RecoveredAgentReplacement {
                session_id: "agent-1".into(),
                generation_id,
                session_close_intent: None,
            }])
        );
        let state: Vec<AgentConfig> = serde_json::from_str(
            &fs::read_to_string(home.path().join("settings").join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state[0].session_name, "Original");
        assert!(state[0].is_off);
        let row = db::get_all_agents()
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == "agent-1")
            .unwrap();
        assert_eq!(row.session_name, "Original");
        assert!(row.is_off);
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }

    #[test]
    fn boundary_committed_journal_recovers_replacement_identity() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let original = AgentConfig {
            session_id: "agent-2".into(),
            session_name: "Original".into(),
            folder: home.path().to_string_lossy().to_string(),
            is_off: true,
            ..Default::default()
        };
        let replacement = AgentConfig {
            session_name: "Replacement".into(),
            is_off: false,
            ..original.clone()
        };
        set_state(home.path(), &original);
        let mut guard = begin_with_barrier(PendingAgentReplacement::new(
            "fresh_resume",
            "agent-2",
            original,
            replacement.clone(),
            None,
            None,
            None,
            false,
        ))
        .unwrap();
        guard.advance(ReplacementPhase::BoundaryCommitted).unwrap();
        drop(guard);

        recover_pending_replacements(true).unwrap();
        let state: Vec<AgentConfig> = serde_json::from_str(
            &fs::read_to_string(home.path().join("settings").join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state[0].session_name, "Replacement");
        assert!(!state[0].is_off);
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }

    #[test]
    fn every_pre_boundary_phase_recovers_original_identity() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        db::init_db_at_path(&home.path().join("state.db")).unwrap();

        for (index, phase) in [
            ReplacementPhase::Prepared,
            ReplacementPhase::StatePersisted,
            ReplacementPhase::MetadataPersisted,
        ]
        .into_iter()
        .enumerate()
        {
            let session_id = format!("agent-pre-boundary-{index}");
            let original = AgentConfig {
                session_id: session_id.clone(),
                session_name: format!("Original-{index}"),
                folder: home.path().to_string_lossy().to_string(),
                is_off: true,
                ..Default::default()
            };
            let replacement = AgentConfig {
                session_name: format!("Replacement-{index}"),
                is_off: false,
                ..original.clone()
            };
            set_state(home.path(), &replacement);
            let mut journal = begin_with_barrier(PendingAgentReplacement::new(
                "fresh_resume",
                &session_id,
                original,
                replacement,
                None,
                None,
                None,
                false,
            ))
            .unwrap();
            journal.advance(phase).unwrap();
            drop(journal);

            recover_pending_replacements(true).unwrap();
            let state: Vec<AgentConfig> = serde_json::from_str(
                &fs::read_to_string(home.path().join("settings").join("state.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                state[0].session_name,
                format!("Original-{index}"),
                "phase {phase:?}"
            );
            assert!(state[0].is_off, "phase {phase:?}");
        }
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }

    #[test]
    fn active_replacement_lock_prevents_concurrent_recovery() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let config = AgentConfig {
            session_id: "agent-busy".into(),
            session_name: "Busy".into(),
            folder: home.path().to_string_lossy().to_string(),
            ..Default::default()
        };
        set_state(home.path(), &config);
        let journal = begin_with_barrier(PendingAgentReplacement::new(
            "clear",
            "agent-busy",
            config.clone(),
            config,
            None,
            None,
            None,
            false,
        ))
        .unwrap();

        assert_eq!(
            recover_pending_replacements(false).unwrap(),
            RecoveryStatus::Busy
        );
        assert!(
            acquire_agent_roster_barrier(false).unwrap().is_none(),
            "journal guard must retain the exact admitted roster barrier"
        );
        journal.complete().unwrap();
        let released = acquire_agent_roster_barrier(false)
            .unwrap()
            .expect("journal completion releases its admitted barrier");
        drop(released);
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }

    #[test]
    fn boundary_committing_recovery_rolls_forward_and_preserves_close_intent() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let original = AgentConfig {
            session_id: "agent-boundary-crash".into(),
            session_name: "OriginalBoundary".into(),
            folder: home.path().to_string_lossy().to_string(),
            is_off: true,
            ..Default::default()
        };
        let replacement = AgentConfig {
            session_name: "ReplacementBoundary".into(),
            is_off: false,
            ..original.clone()
        };
        set_state(home.path(), &replacement);
        let conversations = agent_conversations_dir("agent-boundary-crash").unwrap();
        fs::create_dir_all(&conversations).unwrap();
        crate::conversations::append_jsonl_record(
            &conversations.join("index.jsonl"),
            &ConversationIndexEntry {
                schema: 1,
                conversation_id: "conversation-closed".into(),
                agent_id: "agent-boundary-crash".into(),
                agent_name: "OriginalBoundary".into(),
                agent_class: "Coder".into(),
                workspace: home.path().to_string_lossy().to_string(),
                provider: "codex".into(),
                provider_session_ids: vec!["provider-old".into()],
                started_at: "2026-08-24T00:00:00Z".into(),
                ended_at: Some("2026-08-24T00:01:00Z".into()),
                status: ConversationStatus::Closed,
                boundary_reason: crate::conversations::ConversationBoundaryReason::Clear,
                first_prompt_excerpt: None,
                last_record_excerpt: None,
                record_count: 3,
                turn_count: 1,
                has_turns: true,
                lifecycle_only: false,
                artifact_count: 0,
                path: "conversation-closed".into(),
            },
        )
        .unwrap();
        let mut journal = begin_with_barrier(PendingAgentReplacement::new(
            "clear",
            "agent-boundary-crash",
            original,
            replacement,
            None,
            None,
            Some("conversation-closed".into()),
            true,
        ))
        .unwrap();
        let intent = SessionCloseIntent {
            boundary_id: "boundary-stable".into(),
            agent_id: "agent-boundary-crash".into(),
            agent_name: "OriginalBoundary".into(),
            workspace: home.path().to_string_lossy().to_string(),
            provider: "codex".into(),
            boundary_reason: "clear".into(),
            archive_available: true,
            conversation_id: Some("conversation-closed".into()),
            source_sequence: None,
        };
        journal.set_session_close_intent(intent.clone()).unwrap();
        journal
            .advance(ReplacementPhase::BoundaryCommitting)
            .unwrap();
        let generation_id = journal.generation_id().to_string();
        drop(journal);

        assert_eq!(
            recover_pending_replacements(true).unwrap(),
            RecoveryStatus::Recovered(vec![RecoveredAgentReplacement {
                session_id: "agent-boundary-crash".into(),
                generation_id: generation_id.clone(),
                session_close_intent: Some(intent),
            }])
        );
        let state: Vec<AgentConfig> = serde_json::from_str(
            &fs::read_to_string(home.path().join("settings").join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state[0].session_name, "ReplacementBoundary");
        assert!(matches!(
            pending_replacement_status().unwrap(),
            PendingReplacementStatus::Pending(_)
        ));
        let next_config = AgentConfig {
            session_id: "agent-boundary-crash".into(),
            session_name: "NextReplacement".into(),
            folder: home.path().to_string_lossy().to_string(),
            ..Default::default()
        };
        let next_journal = begin_with_barrier(PendingAgentReplacement::new(
            "clear",
            "agent-boundary-crash",
            next_config.clone(),
            next_config,
            None,
            None,
            None,
            false,
        ))
        .unwrap();
        let next_generation_id = next_journal.generation_id().to_string();
        drop(next_journal);
        assert!(!complete_recovered_replacement("agent-boundary-crash", &generation_id).unwrap());
        assert!(matches!(
            pending_replacement_status().unwrap(),
            PendingReplacementStatus::Pending(_)
        ));
        assert!(
            complete_recovered_replacement("agent-boundary-crash", &next_generation_id).unwrap()
        );
        assert_eq!(
            pending_replacement_status().unwrap(),
            PendingReplacementStatus::None
        );
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }

    #[test]
    fn recovery_refuses_to_overwrite_a_later_roster_update() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let original = AgentConfig {
            session_id: "agent-cas".into(),
            session_name: "OriginalCas".into(),
            folder: home.path().to_string_lossy().to_string(),
            ..Default::default()
        };
        let replacement = AgentConfig {
            session_name: "ReplacementCas".into(),
            ..original.clone()
        };
        set_state(home.path(), &original);
        let journal = begin_with_barrier(PendingAgentReplacement::new(
            "clear",
            "agent-cas",
            original.clone(),
            replacement,
            None,
            None,
            None,
            false,
        ))
        .unwrap();
        drop(journal);
        let later = AgentConfig {
            session_name: "LaterLegitimateUpdate".into(),
            ..original
        };
        set_state(home.path(), &later);

        let error = recover_pending_replacements(true)
            .expect_err("recovery must reject an unexpected newer config");
        assert!(error
            .to_string()
            .contains("changed after its replacement journal"));
        let state: Vec<AgentConfig> = serde_json::from_str(
            &fs::read_to_string(home.path().join("settings").join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state[0].session_name, "LaterLegitimateUpdate");
        assert!(matches!(
            pending_replacement_status().unwrap(),
            PendingReplacementStatus::Pending(_)
        ));
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }
}
