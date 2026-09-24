//! Provider-neutral, agent-owned memory persisted independently from conversation archives.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 4;
pub const DEFAULT_STALE_DAYS: i64 = 30;
pub const MEMORY_BUDGET_POLICY_VERSION: u32 = 2;
pub const MEMORY_CAPABILITY_ENV: &str = "WARDIAN_MEMORY_CAPABILITY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Stable,
    Current,
}

impl MemoryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Current => "current",
        }
    }

    fn parse(value: &str) -> Result<Self, MemoryError> {
        match value {
            "stable" => Ok(Self::Stable),
            "current" => Ok(Self::Current),
            other => Err(MemoryError::Corrupt(format!("unknown memory kind {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Superseded,
    Removed,
}

impl MemoryStatus {
    fn parse(value: &str) -> Result<Self, MemoryError> {
        match value {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "removed" => Ok(Self::Removed),
            other => Err(MemoryError::Corrupt(format!(
                "unknown memory status {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySource {
    pub source_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub revision_id: String,
    pub memory_id: String,
    pub revision: u32,
    pub agent_id: String,
    pub workspace: Option<String>,
    pub kind: MemoryKind,
    pub text: String,
    pub evidence_excerpt: String,
    pub evidence_hash: String,
    pub status: MemoryStatus,
    pub supersedes_revision_id: Option<String>,
    pub replaced_by_revision_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_verified_at: String,
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveMemoryRequest {
    pub agent_id: String,
    #[serde(default)]
    pub workspace: Option<String>,
    pub kind: MemoryKind,
    pub text: String,
    pub evidence_excerpt: String,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMemoryRequest {
    pub memory_id: String,
    pub text: String,
    pub evidence_excerpt: String,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallEntry {
    #[serde(flatten)]
    pub record: MemoryRecord,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallResult {
    pub agent_id: String,
    pub workspace: Option<String>,
    pub stable: Vec<RecallEntry>,
    pub current: Vec<RecallEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBriefKind {
    Fresh,
    ResumeDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledMemoryBrief {
    pub kind: MemoryBriefKind,
    pub context_text: String,
    pub fingerprint: String,
    pub revision_ids: Vec<String>,
    pub omitted_count: usize,
    pub is_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub event_id: String,
    pub agent_id: String,
    pub memory_id: Option<String>,
    pub revision_id: Option<String>,
    pub action: String,
    pub payload: serde_json::Value,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryMutation {
    Save {
        kind: MemoryKind,
        text: String,
        evidence_excerpt: String,
        #[serde(default)]
        sources: Vec<MemorySource>,
    },
    Update {
        memory_id: String,
        text: String,
        evidence_excerpt: String,
        #[serde(default)]
        sources: Vec<MemorySource>,
    },
    Remove {
        memory_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCommitBatch {
    pub agent_id: String,
    #[serde(default)]
    pub workspace: Option<String>,
    pub idempotency_key: String,
    #[serde(default)]
    pub operations: Vec<MemoryMutation>,
    #[serde(default)]
    pub cursor: Option<MemoryCursorUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCursorUpdate {
    pub cursor_key: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCommitResult {
    pub idempotency_key: String,
    pub memory_ids: Vec<String>,
    pub replayed: bool,
}

/// Authority presented to the memory store for one operation.
///
/// Managed provider processes must use `Agent`; the desktop host may use the
/// explicit `Operator` variant for user-directed cross-agent administration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryActor {
    Agent(String),
    Operator,
}

impl MemoryActor {
    pub fn agent(agent_id: impl Into<String>) -> Self {
        Self::Agent(agent_id.into())
    }

    fn authorize_subject(&self, subject_agent_id: &str) -> Result<String, MemoryError> {
        let subject_agent_id = required("agent_id", subject_agent_id)?;
        if let Self::Agent(actor_agent_id) = self {
            let actor_agent_id = required("actor agent_id", actor_agent_id)?;
            if actor_agent_id != subject_agent_id {
                return Err(MemoryError::AccessDenied {
                    actor_agent_id,
                    subject_agent_id,
                });
            }
        }
        Ok(subject_agent_id)
    }

    fn agent_id(&self) -> Result<Option<String>, MemoryError> {
        match self {
            Self::Agent(agent_id) => Ok(Some(required("actor agent_id", agent_id)?)),
            Self::Operator => Ok(None),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory validation failed: {0}")]
    Validation(String),
    #[error("memory record not found: {0}")]
    NotFound(String),
    #[error("memory id prefix is ambiguous: {0}")]
    MemoryIdAmbiguous(String),
    #[error("memory access denied for agent {actor_agent_id} on subject {subject_agent_id}")]
    AccessDenied {
        actor_agent_id: String,
        subject_agent_id: String,
    },
    #[error("memory database is unavailable")]
    HomeUnavailable,
    #[error("memory database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("memory database is corrupt: {0}")]
    Corrupt(String),
    #[error("memory serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("memory filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct MemoryStore {
    path: PathBuf,
}

pub struct MemoryCapabilityLease {
    store_path: PathBuf,
    agent_id: String,
    token: String,
}

impl MemoryCapabilityLease {
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for MemoryCapabilityLease {
    fn drop(&mut self) {
        if let Ok(store) = MemoryStore::open(&self.store_path) {
            let _ = store.revoke_capability(&self.agent_id, &self.token);
        }
    }
}

struct StoredInjection {
    revision_ids: Vec<String>,
}

impl MemoryStore {
    pub fn from_default_home() -> Result<Self, MemoryError> {
        let path = crate::paths::memory_db_path().ok_or(MemoryError::HomeUnavailable)?;
        Self::open(path)
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MemoryError> {
        let store = Self { path: path.into() };
        if let Some(parent) = store.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = store.connection()?;
        migrate(&connection)?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Mint an agent-bound memory capability for one provider process.
    /// Only the digest is persisted, so changing `WARDIAN_SESSION_ID` cannot
    /// turn a caller's inherited capability into another agent's authority.
    /// Multiple provider processes for the same agent remain valid concurrently.
    pub fn issue_capability(&self, agent_id: &str) -> Result<String, MemoryError> {
        let agent_id = required("agent_id", agent_id)?;
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let token_hash = hash_text(&token);
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO memory_process_capabilities(token_hash, agent_id, created_at) VALUES (?1, ?2, ?3)",
            params![token_hash, agent_id, Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(token)
    }

    pub fn issue_process_capability(
        &self,
        agent_id: &str,
    ) -> Result<MemoryCapabilityLease, MemoryError> {
        let agent_id = required("agent_id", agent_id)?;
        let token = self.issue_capability(&agent_id)?;
        Ok(MemoryCapabilityLease {
            store_path: self.path.clone(),
            agent_id,
            token,
        })
    }

    pub fn revoke_capability(&self, agent_id: &str, token: &str) -> Result<bool, MemoryError> {
        let agent_id = required("agent_id", agent_id)?;
        let token = required("memory capability", token)?;
        let token_hash = hash_text(&token);
        let connection = self.connection()?;
        Ok(connection.execute(
            "DELETE FROM memory_process_capabilities WHERE agent_id=?1 AND token_hash=?2",
            params![agent_id, token_hash],
        )? > 0)
    }

    pub fn validate_capability(&self, agent_id: &str, token: &str) -> Result<bool, MemoryError> {
        let agent_id = required("agent_id", agent_id)?;
        let token = required("memory capability", token)?;
        let token_hash = hash_text(&token);
        let connection = self.connection()?;
        Ok(connection
            .query_row(
                "SELECT 1 FROM memory_process_capabilities WHERE agent_id=?1 AND token_hash=?2",
                params![agent_id, token_hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn connection(&self) -> Result<Connection, MemoryError> {
        let connection = Connection::open(&self.path)?;
        connection.busy_timeout(StdDuration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(connection)
    }

    pub fn save(
        &self,
        actor: &MemoryActor,
        request: SaveMemoryRequest,
    ) -> Result<MemoryRecord, MemoryError> {
        let agent_id = actor.authorize_subject(&request.agent_id)?;
        let text = normalize_text(&request.text)?;
        let evidence = normalize_evidence(&request.evidence_excerpt)?;
        let workspace = normalize_workspace(request.workspace.as_deref());
        let idempotency_key = normalize_optional(request.idempotency_key.as_deref());
        let sources = normalize_sources(&request.sources)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        if let Some(mut existing) =
            idempotent_record(&transaction, actor, idempotency_key.as_deref())?
        {
            existing.sources = sources_for_revision(&transaction, &existing.revision_id)?;
            if existing.agent_id != agent_id
                || existing.workspace != workspace
                || existing.kind != request.kind
                || existing.text != text
                || existing.evidence_excerpt != evidence
                || existing.sources != sources
            {
                return Err(MemoryError::Validation(
                    "idempotency key was already used for a different save request".into(),
                ));
            }
            transaction.commit()?;
            return self.attach_sources(existing);
        }
        let memory_id = Uuid::new_v4().to_string();
        let revision_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let evidence_hash = hash_text(&evidence);
        transaction.execute(
            "INSERT INTO memory_records
             (revision_id, memory_id, revision, agent_id, workspace, kind, text,
              evidence_excerpt, evidence_hash, status, created_at, updated_at,
              last_verified_at, idempotency_key)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, ?9, ?9, ?10)",
            params![
                revision_id,
                memory_id,
                agent_id,
                workspace,
                request.kind.as_str(),
                text,
                evidence,
                evidence_hash,
                now,
                idempotency_key
            ],
        )?;
        insert_sources(&transaction, &revision_id, &sources)?;
        insert_event(
            &transaction,
            &agent_id,
            Some(&memory_id),
            Some(&revision_id),
            "saved",
            Some(&serde_json::json!({
                "text": text,
                "evidence_excerpt": evidence,
                "evidence_hash": evidence_hash,
                "kind": request.kind,
                "workspace": workspace,
                "sources": sources
            })),
        )?;
        transaction.commit()?;
        self.get_revision(&revision_id)
    }

    pub fn update(
        &self,
        actor: &MemoryActor,
        request: UpdateMemoryRequest,
    ) -> Result<MemoryRecord, MemoryError> {
        let memory_id = required("memory_id", &request.memory_id)?;
        let text = normalize_text(&request.text)?;
        let evidence = normalize_evidence(&request.evidence_excerpt)?;
        let idempotency_key = normalize_optional(request.idempotency_key.as_deref());
        let sources = normalize_sources(&request.sources)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        if let Some(mut existing) =
            idempotent_record(&transaction, actor, idempotency_key.as_deref())?
        {
            existing.sources = sources_for_revision(&transaction, &existing.revision_id)?;
            if (existing.memory_id != memory_id && !existing.memory_id.starts_with(&memory_id))
                || existing.text != text
                || existing.evidence_excerpt != evidence
                || existing.sources != sources
            {
                return Err(MemoryError::Validation(
                    "idempotency key was already used for a different update request".into(),
                ));
            }
            transaction.commit()?;
            return self.attach_sources(existing);
        }
        let memory_id = resolve_memory_id(&transaction, actor, &memory_id, true)?;
        let previous = query_active_for_actor(&transaction, actor, &memory_id)?
            .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;
        let revision_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let evidence_hash = hash_text(&evidence);
        transaction.execute(
            "UPDATE memory_records SET status='superseded', replaced_by_revision_id=?1, updated_at=?2
             WHERE revision_id=?3 AND agent_id=?4",
            params![revision_id, now, previous.revision_id, previous.agent_id],
        )?;
        transaction.execute(
            "INSERT INTO memory_records
             (revision_id, memory_id, revision, agent_id, workspace, kind, text,
              evidence_excerpt, evidence_hash, status, supersedes_revision_id,
              created_at, updated_at, last_verified_at, idempotency_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10, ?11, ?11, ?11, ?12)",
            params![
                revision_id,
                &memory_id,
                previous.revision + 1,
                previous.agent_id,
                previous.workspace,
                previous.kind.as_str(),
                text,
                evidence,
                evidence_hash,
                previous.revision_id,
                now,
                idempotency_key
            ],
        )?;
        insert_sources(&transaction, &revision_id, &sources)?;
        insert_event(
            &transaction,
            &previous.agent_id,
            Some(&memory_id),
            Some(&revision_id),
            "updated",
            Some(&serde_json::json!({
                "text": text,
                "evidence_excerpt": evidence,
                "evidence_hash": evidence_hash,
                "kind": previous.kind,
                "workspace": previous.workspace,
                "sources": sources
            })),
        )?;
        transaction.commit()?;
        self.get_revision(&revision_id)
    }

    pub fn remove(
        &self,
        actor: &MemoryActor,
        memory_id: &str,
    ) -> Result<MemoryRecord, MemoryError> {
        let memory_id = required("memory_id", memory_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let mut record = query_active_for_actor(&transaction, actor, &memory_id)?
            .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;
        let resolved_memory_id = record.memory_id.clone();
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE memory_records SET status='removed', updated_at=?1 WHERE revision_id=?2 AND agent_id=?3",
            params![now, record.revision_id, record.agent_id],
        )?;
        insert_event(
            &transaction,
            &record.agent_id,
            Some(&resolved_memory_id),
            Some(&record.revision_id),
            "removed",
            None,
        )?;
        transaction.commit()?;
        record.status = MemoryStatus::Removed;
        record.updated_at = now;
        self.attach_sources(record)
    }

    pub fn get(&self, actor: &MemoryActor, memory_id: &str) -> Result<MemoryRecord, MemoryError> {
        let connection = self.connection()?;
        let memory_id = resolve_memory_id(&connection, actor, memory_id, false)?;
        let record = match actor.agent_id()? {
            Some(agent_id) => connection
                .query_row(
                    "SELECT * FROM memory_records WHERE memory_id=?1 AND agent_id=?2 ORDER BY revision DESC LIMIT 1",
                    params![memory_id, agent_id],
                    row_to_record,
                )
                .optional()?,
            None => connection
                .query_row(
                    "SELECT * FROM memory_records WHERE memory_id=?1 ORDER BY revision DESC LIMIT 1",
                    [&memory_id],
                    row_to_record,
                )
                .optional()?,
        }
        .ok_or_else(|| MemoryError::NotFound(memory_id.to_string()))?;
        self.attach_sources(record)
    }

    pub fn history(
        &self,
        actor: &MemoryActor,
        memory_id: &str,
    ) -> Result<Vec<MemoryRecord>, MemoryError> {
        let connection = self.connection()?;
        let memory_id = resolve_memory_id(&connection, actor, memory_id, false)?;
        let records = match actor.agent_id()? {
            Some(agent_id) => {
                let mut statement = connection.prepare(
                    "SELECT * FROM memory_records WHERE memory_id=?1 AND agent_id=?2 ORDER BY revision ASC",
                )?;
                let records = statement
                    .query_map(params![memory_id, agent_id], row_to_record)?
                    .collect::<Result<Vec<_>, _>>()?;
                records
            }
            None => {
                let mut statement = connection.prepare(
                    "SELECT * FROM memory_records WHERE memory_id=?1 ORDER BY revision ASC",
                )?;
                let records = statement
                    .query_map([&memory_id], row_to_record)?
                    .collect::<Result<Vec<_>, _>>()?;
                records
            }
        };
        if records.is_empty() {
            return Err(MemoryError::NotFound(memory_id.to_string()));
        }
        records
            .into_iter()
            .map(|record| self.attach_sources(record))
            .collect()
    }

    pub fn list_active(
        &self,
        actor: &MemoryActor,
        agent_id: &str,
        workspace: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, MemoryError> {
        let agent_id = actor.authorize_subject(agent_id)?;
        let workspace = normalize_workspace(workspace);
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT * FROM memory_records
             WHERE agent_id=?1 AND status='active' AND (workspace IS NULL OR workspace=?2)
             ORDER BY CASE kind WHEN 'stable' THEN 0 ELSE 1 END,
               CASE WHEN workspace IS NULL THEN 0 ELSE 1 END,
               last_verified_at DESC, memory_id ASC",
        )?;
        let records = statement
            .query_map(params![agent_id, workspace], row_to_record)?
            .collect::<Result<Vec<_>, _>>()?;
        records
            .into_iter()
            .map(|record| self.attach_sources(record))
            .collect()
    }

    pub fn recall(
        &self,
        actor: &MemoryActor,
        agent_id: &str,
        workspace: Option<&str>,
    ) -> Result<RecallResult, MemoryError> {
        let workspace = normalize_workspace(workspace);
        let stale_before = Utc::now() - Duration::days(DEFAULT_STALE_DAYS);
        let mut stable = Vec::new();
        let mut current = Vec::new();
        actor.authorize_subject(agent_id)?;
        for record in self.list_active(actor, agent_id, workspace.as_deref())? {
            if hash_text(&record.evidence_excerpt) != record.evidence_hash {
                continue;
            }
            let stale = record.kind == MemoryKind::Current
                && DateTime::parse_from_rfc3339(&record.last_verified_at)
                    .map(|value| value.with_timezone(&Utc) < stale_before)
                    .unwrap_or(true);
            let entry = RecallEntry { record, stale };
            match entry.record.kind {
                MemoryKind::Stable => stable.push(entry),
                MemoryKind::Current => current.push(entry),
            }
        }
        Ok(RecallResult {
            agent_id: agent_id.to_string(),
            workspace,
            stable,
            current,
        })
    }

    /// Compile a deterministic startup brief. A resumed provider process gets
    /// only revisions that changed since its latest recorded fingerprint.
    // Keep the authority argument explicit alongside the complete provider
    // checkpoint identity; collapsing either into ambient state weakens recall
    // isolation and retry semantics.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_brief(
        &self,
        actor: &MemoryActor,
        agent_id: &str,
        workspace: Option<&str>,
        provider: &str,
        provider_process_key: &str,
        resumed: bool,
        max_chars: usize,
    ) -> Result<CompiledMemoryBrief, MemoryError> {
        actor.authorize_subject(agent_id)?;
        let recall = self.recall(actor, agent_id, workspace)?;
        let all_active = recall
            .stable
            .iter()
            .chain(recall.current.iter())
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        let revision_ids = all_active
            .iter()
            .map(|record| record.revision_id.clone())
            .collect::<Vec<_>>();
        let fingerprint = memory_fingerprint(&all_active);
        let previous = if resumed {
            self.latest_injection(agent_id, provider, provider_process_key)?
        } else {
            None
        };
        let (kind, changed, removed) = if let Some(previous) = previous {
            let previous_records = self.records_by_revision_ids(&previous.revision_ids)?;
            let previous_by_memory = previous_records
                .iter()
                .map(|record| (record.memory_id.as_str(), record))
                .collect::<std::collections::HashMap<_, _>>();
            let active_ids = all_active
                .iter()
                .map(|record| record.memory_id.as_str())
                .collect::<std::collections::HashSet<_>>();
            let removed = previous_records
                .iter()
                .filter(|record| !active_ids.contains(record.memory_id.as_str()))
                .map(|record| record.memory_id.clone())
                .collect::<Vec<_>>();
            let mut changed = all_active
                .iter()
                .filter(|record| {
                    previous_by_memory
                        .get(record.memory_id.as_str())
                        .is_none_or(|prior| prior.revision_id != record.revision_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            // Every provider process must receive an authoritative checkpoint.
            // A prior process can exit before it submits a turn, so an audit
            // record alone does not prove that its model consumed the brief.
            // Re-send the active set when there is no content delta; changed
            // resumes still receive only changed and removed revisions.
            if changed.is_empty() && removed.is_empty() {
                changed = all_active.clone();
            }
            (MemoryBriefKind::ResumeDelta, changed, removed)
        } else {
            (MemoryBriefKind::Fresh, all_active, Vec::new())
        };
        let (context_text, omitted_count) = render_brief(kind, &changed, &removed, max_chars);
        Ok(CompiledMemoryBrief {
            kind,
            is_empty: context_text.is_empty(),
            context_text,
            fingerprint,
            revision_ids,
            omitted_count,
        })
    }

    pub fn record_injection(
        &self,
        actor: &MemoryActor,
        agent_id: &str,
        workspace: Option<&str>,
        provider: &str,
        provider_process_key: &str,
        brief: &CompiledMemoryBrief,
    ) -> Result<Option<String>, MemoryError> {
        let agent_id = actor.authorize_subject(agent_id)?;
        if brief.is_empty {
            return Ok(None);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let injection_id = Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO memory_injections
             (injection_id, agent_id, workspace, provider, provider_process_key, fingerprint,
              revision_ids_json, context_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                injection_id,
                agent_id,
                normalize_workspace(workspace),
                provider,
                provider_process_key,
                brief.fingerprint,
                serde_json::to_string(&brief.revision_ids)?,
                brief.context_text,
                Utc::now().to_rfc3339()
            ],
        )?;
        insert_event(
            &transaction,
            &agent_id,
            None,
            None,
            "loaded",
            Some(&serde_json::json!({
                "injection_id": injection_id,
                "kind": brief.kind,
                "fingerprint": brief.fingerprint,
                "revision_ids": brief.revision_ids,
                "omitted_count": brief.omitted_count,
                "budget_policy_version": MEMORY_BUDGET_POLICY_VERSION,
                "injected_context": brief.context_text,
                "provider": provider,
                "provider_process_key": provider_process_key
            })),
        )?;
        transaction.commit()?;
        Ok(Some(injection_id))
    }

    pub fn list_events(
        &self,
        actor: &MemoryActor,
        agent_id: &str,
    ) -> Result<Vec<MemoryEvent>, MemoryError> {
        let agent_id = actor.authorize_subject(agent_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_id, agent_id, memory_id, revision_id, action, payload_json, occurred_at
             FROM (
               SELECT event_id, agent_id, memory_id, revision_id, action, payload_json, occurred_at
               FROM memory_events WHERE agent_id=?1
               ORDER BY occurred_at DESC, event_id DESC LIMIT 100
             ) ORDER BY occurred_at ASC, event_id ASC",
        )?;
        let events = statement
            .query_map([agent_id], |row| {
                let payload: Option<String> = row.get(5)?;
                Ok(MemoryEvent {
                    event_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    memory_id: row.get(2)?,
                    revision_id: row.get(3)?,
                    action: row.get(4)?,
                    payload: payload
                        .and_then(|value| serde_json::from_str(&value).ok())
                        .unwrap_or(serde_json::Value::Null),
                    occurred_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    /// Validate and commit a consolidator batch, its cursor, and its durable
    /// idempotency receipt in one transaction.
    pub fn commit_batch(
        &self,
        actor: &MemoryActor,
        batch: MemoryCommitBatch,
    ) -> Result<MemoryCommitResult, MemoryError> {
        let agent_id = actor.authorize_subject(&batch.agent_id)?;
        let key = required("idempotency_key", &batch.idempotency_key)?;
        let workspace = normalize_workspace(batch.workspace.as_deref());
        let mut connection = self.connection()?;
        // Acquire the writer lock before reading the idempotency receipt or
        // cursor so overlapping consolidators observe one serialized order.
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut canonical_batch = batch;
        for mutation in &mut canonical_batch.operations {
            let memory_id = match mutation {
                MemoryMutation::Update { memory_id, .. } | MemoryMutation::Remove { memory_id } => {
                    memory_id
                }
                MemoryMutation::Save { .. } => continue,
            };
            *memory_id = resolve_memory_id(
                &transaction,
                &MemoryActor::agent(&agent_id),
                memory_id,
                false,
            )?;
        }
        let request_hash = hash_text(&serde_json::to_string(&canonical_batch)?);
        if let Some((stored_hash, stored_result)) = transaction
            .query_row(
                "SELECT request_hash, result_json FROM memory_commits WHERE idempotency_key=?1",
                [&key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_hash != request_hash {
                return Err(MemoryError::Validation(
                    "idempotency key was already used for a different memory batch".into(),
                ));
            }
            let mut result: MemoryCommitResult = serde_json::from_str(&stored_result)?;
            result.replayed = true;
            transaction.commit()?;
            return Ok(result);
        }

        if let Some(cursor) = &canonical_batch.cursor {
            advance_consolidation_cursor(&transaction, &agent_id, workspace.as_deref(), cursor)?;
        }

        let mut memory_ids = Vec::new();
        for mutation in &canonical_batch.operations {
            match mutation {
                MemoryMutation::Save {
                    kind,
                    text,
                    evidence_excerpt,
                    sources,
                } => {
                    let text = normalize_text(text)?;
                    let evidence = normalize_evidence(evidence_excerpt)?;
                    let memory_id = Uuid::new_v4().to_string();
                    let revision_id = Uuid::new_v4().to_string();
                    let now = Utc::now().to_rfc3339();
                    transaction.execute(
                        "INSERT INTO memory_records
                         (revision_id, memory_id, revision, agent_id, workspace, kind, text,
                          evidence_excerpt, evidence_hash, status, created_at, updated_at, last_verified_at)
                         VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, ?9, ?9)",
                        params![revision_id, memory_id, agent_id, workspace, kind.as_str(), text,
                            evidence, hash_text(&evidence), now],
                    )?;
                    insert_sources(&transaction, &revision_id, sources)?;
                    insert_event(
                        &transaction,
                        &agent_id,
                        Some(&memory_id),
                        Some(&revision_id),
                        "saved",
                        Some(&serde_json::json!({
                            "text": text,
                            "evidence_excerpt": evidence,
                            "evidence_hash": hash_text(&evidence),
                            "kind": kind,
                            "workspace": workspace,
                            "sources": sources
                        })),
                    )?;
                    memory_ids.push(memory_id);
                }
                MemoryMutation::Update {
                    memory_id,
                    text,
                    evidence_excerpt,
                    sources,
                } => {
                    let resolved_memory_id = resolve_memory_id(
                        &transaction,
                        &MemoryActor::agent(&agent_id),
                        memory_id,
                        true,
                    )?;
                    let prior = query_active_for_agent_resolved(
                        &transaction,
                        &agent_id,
                        &resolved_memory_id,
                    )?
                    .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;
                    if prior.agent_id != agent_id {
                        return Err(MemoryError::Validation(format!(
                            "memory {memory_id} belongs to another agent"
                        )));
                    }
                    let text = normalize_text(text)?;
                    let evidence = normalize_evidence(evidence_excerpt)?;
                    let revision_id = Uuid::new_v4().to_string();
                    let now = Utc::now().to_rfc3339();
                    transaction.execute(
                        "UPDATE memory_records SET status='superseded', replaced_by_revision_id=?1, updated_at=?2 WHERE revision_id=?3 AND agent_id=?4",
                        params![revision_id, now, prior.revision_id, agent_id],
                    )?;
                    transaction.execute(
                        "INSERT INTO memory_records
                         (revision_id, memory_id, revision, agent_id, workspace, kind, text,
                          evidence_excerpt, evidence_hash, status, supersedes_revision_id,
                          created_at, updated_at, last_verified_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10, ?11, ?11, ?11)",
                        params![
                            revision_id,
                            &resolved_memory_id,
                            prior.revision + 1,
                            agent_id,
                            prior.workspace,
                            prior.kind.as_str(),
                            text,
                            evidence,
                            hash_text(&evidence),
                            prior.revision_id,
                            now
                        ],
                    )?;
                    insert_sources(&transaction, &revision_id, sources)?;
                    insert_event(
                        &transaction,
                        &agent_id,
                        Some(&resolved_memory_id),
                        Some(&revision_id),
                        "updated",
                        Some(&serde_json::json!({
                            "text": text,
                            "evidence_excerpt": evidence,
                            "evidence_hash": hash_text(&evidence),
                            "kind": prior.kind,
                            "workspace": prior.workspace,
                            "sources": sources
                        })),
                    )?;
                    memory_ids.push(resolved_memory_id);
                }
                MemoryMutation::Remove { memory_id } => {
                    let resolved_memory_id = resolve_memory_id(
                        &transaction,
                        &MemoryActor::agent(&agent_id),
                        memory_id,
                        true,
                    )?;
                    let prior = query_active_for_agent_resolved(
                        &transaction,
                        &agent_id,
                        &resolved_memory_id,
                    )?
                    .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;
                    if prior.agent_id != agent_id {
                        return Err(MemoryError::Validation(format!(
                            "memory {memory_id} belongs to another agent"
                        )));
                    }
                    transaction.execute(
                        "UPDATE memory_records SET status='removed', updated_at=?1 WHERE revision_id=?2 AND agent_id=?3",
                        params![Utc::now().to_rfc3339(), prior.revision_id, agent_id],
                    )?;
                    insert_event(
                        &transaction,
                        &agent_id,
                        Some(&resolved_memory_id),
                        Some(&prior.revision_id),
                        "removed",
                        None,
                    )?;
                    memory_ids.push(resolved_memory_id);
                }
            }
        }
        let result = MemoryCommitResult {
            idempotency_key: key.clone(),
            memory_ids,
            replayed: false,
        };
        transaction.execute(
            "INSERT INTO memory_commits(idempotency_key, request_hash, result_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                key,
                request_hash,
                serde_json::to_string(&result)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        transaction.commit()?;
        Ok(result)
    }

    fn get_revision(&self, revision_id: &str) -> Result<MemoryRecord, MemoryError> {
        let connection = self.connection()?;
        let record = connection
            .query_row(
                "SELECT * FROM memory_records WHERE revision_id=?1",
                [revision_id],
                row_to_record,
            )
            .optional()?
            .ok_or_else(|| MemoryError::NotFound(revision_id.to_string()))?;
        self.attach_sources(record)
    }

    fn attach_sources(&self, mut record: MemoryRecord) -> Result<MemoryRecord, MemoryError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT source_type, locator, source_hash, is_primary FROM memory_sources
             WHERE revision_id=?1 ORDER BY is_primary DESC, id ASC",
        )?;
        record.sources = statement
            .query_map([&record.revision_id], |row| {
                Ok(MemorySource {
                    source_type: row.get(0)?,
                    locator: row.get(1)?,
                    source_hash: row.get(2)?,
                    primary: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(record)
    }

    fn latest_injection(
        &self,
        agent_id: &str,
        provider: &str,
        process_key: &str,
    ) -> Result<Option<StoredInjection>, MemoryError> {
        let connection = self.connection()?;
        let stored = connection
            .query_row(
                "SELECT revision_ids_json FROM memory_injections
                 WHERE agent_id=?1 AND provider=?2 AND provider_process_key=?3
                 ORDER BY created_at DESC LIMIT 1",
                params![agent_id, provider, process_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        stored
            .map(|value| {
                Ok(StoredInjection {
                    revision_ids: serde_json::from_str(&value)?,
                })
            })
            .transpose()
    }

    fn records_by_revision_ids(
        &self,
        revision_ids: &[String],
    ) -> Result<Vec<MemoryRecord>, MemoryError> {
        revision_ids
            .iter()
            .map(|revision_id| self.get_revision(revision_id))
            .collect()
    }
}

fn migrate(connection: &Connection) -> Result<(), MemoryError> {
    connection.execute_batch(&format!(r#"
        CREATE TABLE IF NOT EXISTS memory_schema (version INTEGER NOT NULL);
        INSERT INTO memory_schema(version) SELECT {SCHEMA_VERSION}
          WHERE NOT EXISTS (SELECT 1 FROM memory_schema);
        UPDATE memory_schema SET version={SCHEMA_VERSION};
        CREATE TABLE IF NOT EXISTS memory_records (
          revision_id TEXT PRIMARY KEY, memory_id TEXT NOT NULL, revision INTEGER NOT NULL,
          agent_id TEXT NOT NULL, workspace TEXT, kind TEXT NOT NULL, text TEXT NOT NULL,
          evidence_excerpt TEXT NOT NULL, evidence_hash TEXT NOT NULL, status TEXT NOT NULL,
          supersedes_revision_id TEXT, replaced_by_revision_id TEXT,
          created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_verified_at TEXT NOT NULL,
          idempotency_key TEXT UNIQUE,
          UNIQUE(memory_id, revision)
        );
        CREATE INDEX IF NOT EXISTS idx_memory_active ON memory_records(agent_id, workspace, status, kind);
        CREATE TABLE IF NOT EXISTS memory_sources (
          id INTEGER PRIMARY KEY AUTOINCREMENT, revision_id TEXT NOT NULL,
          source_type TEXT NOT NULL, locator TEXT, source_hash TEXT, is_primary INTEGER NOT NULL DEFAULT 0,
          FOREIGN KEY(revision_id) REFERENCES memory_records(revision_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS memory_events (
          event_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, memory_id TEXT, revision_id TEXT,
          action TEXT NOT NULL, payload_json TEXT, occurred_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_memory_events_agent ON memory_events(agent_id, occurred_at);
        CREATE TABLE IF NOT EXISTS memory_injections (
          injection_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, workspace TEXT, provider TEXT NOT NULL,
          provider_process_key TEXT NOT NULL, fingerprint TEXT NOT NULL, revision_ids_json TEXT NOT NULL,
          context_text TEXT NOT NULL, created_at TEXT NOT NULL
        );
        DROP INDEX IF EXISTS idx_memory_injection_unique;
        CREATE INDEX IF NOT EXISTS idx_memory_injection_checkpoint
          ON memory_injections(agent_id, provider, provider_process_key, fingerprint, created_at);
        CREATE INDEX IF NOT EXISTS idx_memory_injection_process ON memory_injections(agent_id, provider, provider_process_key, created_at);
        CREATE TABLE IF NOT EXISTS memory_consolidation_cursors (
          cursor_key TEXT PRIMARY KEY, agent_id TEXT NOT NULL, workspace TEXT,
          conversation_id TEXT, sequence INTEGER NOT NULL, updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memory_commits (
          idempotency_key TEXT PRIMARY KEY, request_hash TEXT NOT NULL,
          result_json TEXT NOT NULL, created_at TEXT NOT NULL
        );
        DROP TABLE IF EXISTS memory_capabilities;
        CREATE TABLE IF NOT EXISTS memory_process_capabilities (
          token_hash TEXT PRIMARY KEY, agent_id TEXT NOT NULL,
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_memory_process_capabilities_agent
          ON memory_process_capabilities(agent_id, created_at);
    "#))?;
    Ok(())
}

fn advance_consolidation_cursor(
    transaction: &Transaction<'_>,
    agent_id: &str,
    workspace: Option<&str>,
    cursor: &MemoryCursorUpdate,
) -> Result<(), MemoryError> {
    required("cursor_key", &cursor.cursor_key)?;
    let conversation_id = cursor
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let cursor_key =
        canonical_consolidation_cursor_key(agent_id, workspace, conversation_id.as_deref());
    let existing = transaction
        .query_row(
            "SELECT agent_id, conversation_id, sequence
             FROM memory_consolidation_cursors WHERE cursor_key=?1",
            [&cursor_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((existing_owner, _existing_conversation, existing_sequence)) = existing {
        if existing_owner != agent_id {
            return Err(MemoryError::AccessDenied {
                actor_agent_id: agent_id.to_string(),
                subject_agent_id: existing_owner,
            });
        }
        if cursor.sequence <= existing_sequence {
            return Err(MemoryError::Validation(format!(
                "consolidation cursor {cursor_key} cannot move from sequence {existing_sequence} to {}",
                cursor.sequence
            )));
        }
    }
    transaction.execute(
        "INSERT INTO memory_consolidation_cursors
         (cursor_key, agent_id, workspace, conversation_id, sequence, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(cursor_key) DO UPDATE SET workspace=excluded.workspace,
           sequence=excluded.sequence, updated_at=excluded.updated_at",
        params![
            cursor_key,
            agent_id,
            workspace,
            conversation_id,
            cursor.sequence,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

fn canonical_consolidation_cursor_key(
    agent_id: &str,
    workspace: Option<&str>,
    conversation_id: Option<&str>,
) -> String {
    let scope = format!(
        "{}\0{}\0{}",
        agent_id,
        workspace.unwrap_or_default(),
        conversation_id.unwrap_or_default()
    );
    format!("memory-consolidation:{}", hash_text(&scope))
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let kind: String = row.get("kind")?;
    let status: String = row.get("status")?;
    Ok(MemoryRecord {
        revision_id: row.get("revision_id")?,
        memory_id: row.get("memory_id")?,
        revision: row.get("revision")?,
        agent_id: row.get("agent_id")?,
        workspace: row.get("workspace")?,
        kind: MemoryKind::parse(&kind).map_err(to_sql_conversion)?,
        text: row.get("text")?,
        evidence_excerpt: row.get("evidence_excerpt")?,
        evidence_hash: row.get("evidence_hash")?,
        status: MemoryStatus::parse(&status).map_err(to_sql_conversion)?,
        supersedes_revision_id: row.get("supersedes_revision_id")?,
        replaced_by_revision_id: row.get("replaced_by_revision_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_verified_at: row.get("last_verified_at")?,
        idempotency_key: row.get("idempotency_key")?,
        sources: Vec::new(),
    })
}

fn to_sql_conversion(error: MemoryError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

/// Resolves a full memory ID or a unique actor-scoped prefix without exposing
/// records outside the actor's authorization boundary.
fn resolve_memory_id(
    connection: &Connection,
    actor: &MemoryActor,
    requested: &str,
    active_only: bool,
) -> Result<String, MemoryError> {
    let requested = required("memory_id", requested)?;
    let status_filter = if active_only {
        " AND status='active'"
    } else {
        ""
    };
    let agent_id = actor.agent_id()?;
    let sql = match agent_id {
        Some(_) => format!(
            "SELECT DISTINCT memory_id FROM memory_records WHERE agent_id=?1{status_filter}"
        ),
        None => format!("SELECT DISTINCT memory_id FROM memory_records WHERE 1=1{status_filter}"),
    };
    let mut statement = connection.prepare(&sql)?;
    let candidates = match agent_id.as_deref() {
        Some(agent_id) => statement
            .query_map([agent_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?,
        None => statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?,
    };
    resolve_memory_id_from_candidates(&requested, &candidates)
}

fn resolve_memory_id_from_candidates(
    requested: &str,
    candidates: &[String],
) -> Result<String, MemoryError> {
    let requested = required("memory_id", requested)?;
    if candidates.iter().any(|candidate| candidate == &requested) {
        return Ok(requested);
    }
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(&requested));
    let Some(first) = matches.next() else {
        return Err(MemoryError::NotFound(requested));
    };
    if matches.next().is_some() {
        return Err(MemoryError::MemoryIdAmbiguous(requested));
    }
    Ok(first.clone())
}

fn query_active(
    transaction: &Transaction<'_>,
    memory_id: &str,
) -> Result<Option<MemoryRecord>, MemoryError> {
    Ok(transaction.query_row(
        "SELECT * FROM memory_records WHERE memory_id=?1 AND status='active' ORDER BY revision DESC LIMIT 1",
        [memory_id], row_to_record,
    ).optional()?)
}

fn query_active_for_actor(
    transaction: &Transaction<'_>,
    actor: &MemoryActor,
    memory_id: &str,
) -> Result<Option<MemoryRecord>, MemoryError> {
    let memory_id = resolve_memory_id(transaction, actor, memory_id, true)?;
    match actor.agent_id()? {
        Some(agent_id) => query_active_for_agent_resolved(transaction, &agent_id, &memory_id),
        None => query_active(transaction, &memory_id),
    }
}

fn query_active_for_agent_resolved(
    transaction: &Transaction<'_>,
    agent_id: &str,
    memory_id: &str,
) -> Result<Option<MemoryRecord>, MemoryError> {
    Ok(transaction
        .query_row(
            "SELECT * FROM memory_records WHERE memory_id=?1 AND agent_id=?2 AND status='active' ORDER BY revision DESC LIMIT 1",
            params![memory_id, agent_id],
            row_to_record,
        )
        .optional()?)
}

fn idempotent_record(
    transaction: &Transaction<'_>,
    actor: &MemoryActor,
    key: Option<&str>,
) -> Result<Option<MemoryRecord>, MemoryError> {
    let Some(key) = key else {
        return Ok(None);
    };
    let record = transaction
        .query_row(
            "SELECT * FROM memory_records WHERE idempotency_key=?1",
            [key],
            row_to_record,
        )
        .optional()?;
    if let (Some(actor_agent_id), Some(record)) = (actor.agent_id()?, record.as_ref()) {
        if record.agent_id != actor_agent_id {
            return Err(MemoryError::Validation(
                "idempotency key is unavailable".into(),
            ));
        }
    }
    Ok(record)
}

fn sources_for_revision(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> Result<Vec<MemorySource>, MemoryError> {
    let mut statement = transaction.prepare(
        "SELECT source_type, locator, source_hash, is_primary FROM memory_sources
         WHERE revision_id=?1 ORDER BY source_type, locator, source_hash, is_primary",
    )?;
    let sources = statement
        .query_map([revision_id], |row| {
            Ok(MemorySource {
                source_type: row.get(0)?,
                locator: row.get(1)?,
                source_hash: row.get(2)?,
                primary: row.get::<_, i64>(3)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(sources)
}

fn normalize_sources(sources: &[MemorySource]) -> Result<Vec<MemorySource>, MemoryError> {
    let mut normalized = sources
        .iter()
        .map(|source| {
            Ok(MemorySource {
                source_type: required("source_type", &source.source_type)?,
                locator: normalize_optional(source.locator.as_deref()),
                source_hash: normalize_optional(source.source_hash.as_deref()),
                primary: source.primary,
            })
        })
        .collect::<Result<Vec<_>, MemoryError>>()?;
    normalized.sort_by(|left, right| {
        (
            &left.source_type,
            &left.locator,
            &left.source_hash,
            left.primary,
        )
            .cmp(&(
                &right.source_type,
                &right.locator,
                &right.source_hash,
                right.primary,
            ))
    });
    Ok(normalized)
}

fn insert_sources(
    transaction: &Transaction<'_>,
    revision_id: &str,
    sources: &[MemorySource],
) -> Result<(), MemoryError> {
    for source in sources {
        let source_type = required("source_type", &source.source_type)?;
        transaction.execute(
            "INSERT INTO memory_sources(revision_id, source_type, locator, source_hash, is_primary)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                revision_id,
                source_type,
                normalize_optional(source.locator.as_deref()),
                normalize_optional(source.source_hash.as_deref()),
                i64::from(source.primary)
            ],
        )?;
    }
    Ok(())
}

fn insert_event(
    transaction: &Transaction<'_>,
    agent_id: &str,
    memory_id: Option<&str>,
    revision_id: Option<&str>,
    action: &str,
    payload: Option<&serde_json::Value>,
) -> Result<(), MemoryError> {
    transaction.execute(
        "INSERT INTO memory_events(event_id, agent_id, memory_id, revision_id, action, payload_json, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![Uuid::new_v4().to_string(), agent_id, memory_id, revision_id, action,
            payload.map(serde_json::to_string).transpose()?, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn required(field: &str, value: &str) -> Result<String, MemoryError> {
    let value = value.trim();
    if value.is_empty() {
        Err(MemoryError::Validation(format!("{field} is required")))
    } else {
        Ok(value.to_string())
    }
}

fn normalize_text(value: &str) -> Result<String, MemoryError> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    required("text", &value)
}

fn normalize_evidence(value: &str) -> Result<String, MemoryError> {
    let value = value.trim();
    required("evidence_excerpt", value)
}

pub fn normalize_workspace(value: Option<&str>) -> Option<String> {
    normalize_workspace_for(value, cfg!(windows))
}

fn normalize_workspace_for(value: Option<&str>, windows: bool) -> Option<String> {
    normalize_optional(value).map(|value| {
        if !windows {
            if value == "/" || value == "//" {
                return value;
            }
            return value.trim_end_matches('/').to_string();
        }

        let normalized = value.replace('\\', "/").to_ascii_lowercase();
        let normalized = if let Some(path) = normalized.strip_prefix("//?/unc/") {
            format!("//{path}")
        } else if let Some(path) = normalized.strip_prefix("//?/") {
            path.to_string()
        } else {
            normalized
        };
        if normalized == "/"
            || (normalized.len() == 3
                && normalized.as_bytes()[1] == b':'
                && normalized.ends_with('/'))
        {
            return normalized;
        }
        let without_trailing = normalized.trim_end_matches('/');
        let unc_parts = without_trailing
            .strip_prefix("//")
            .map(|rest| rest.split('/').filter(|part| !part.is_empty()).count());
        if unc_parts == Some(2) {
            format!("{without_trailing}/")
        } else {
            without_trailing.to_string()
        }
    })
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn hash_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn memory_fingerprint(records: &[MemoryRecord]) -> String {
    let canonical = records
        .iter()
        .map(|record| {
            let stale = record.kind == MemoryKind::Current
                && DateTime::parse_from_rfc3339(&record.last_verified_at)
                    .map(|value| {
                        value.with_timezone(&Utc) < Utc::now() - Duration::days(DEFAULT_STALE_DAYS)
                    })
                    .unwrap_or(true);
            format!(
                "{}:{}:{:?}:{}:{}:{}:{}:{}",
                record.memory_id,
                record.revision,
                record.kind,
                record.workspace.as_deref().unwrap_or("agent-wide"),
                record.evidence_hash,
                record.text,
                stale,
                MEMORY_BUDGET_POLICY_VERSION
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    hash_text(&canonical)
}

fn render_brief(
    kind: MemoryBriefKind,
    records: &[MemoryRecord],
    removed: &[String],
    max_chars: usize,
) -> (String, usize) {
    if records.is_empty() && removed.is_empty() {
        return (String::new(), 0);
    }
    let title = match kind {
        MemoryBriefKind::Fresh => "# Wardian memory\n".to_string(),
        MemoryBriefKind::ResumeDelta => "# Wardian memory changes\n".to_string(),
    };

    if title.len() > max_chars {
        return (String::new(), records.len() + removed.len());
    }

    let now = Utc::now();
    let stale = records
        .iter()
        .map(|record| brief_record_is_stale(record, now))
        .collect::<Vec<_>>();
    let lines = records
        .iter()
        .zip(&stale)
        .map(|(record, stale)| brief_record_line(record, *stale))
        .collect::<Vec<_>>();
    let mut selected = vec![false; records.len()];
    let mut selected_removed = vec![false; removed.len()];
    let mut used = title.len();

    let stable_indices = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| (record.kind == MemoryKind::Stable).then_some(index))
        .collect::<Vec<_>>();
    let fresh_current_indices = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.kind == MemoryKind::Current && !stale[index]).then_some(index)
        })
        .collect::<Vec<_>>();
    let stale_current_indices = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.kind == MemoryKind::Current && stale[index]).then_some(index)
        })
        .collect::<Vec<_>>();

    // Reserve a whole stable and fresh current record together when the pair
    // fits. Preserve recall order within each selection tier.
    let anchors = fresh_current_indices.iter().find_map(|current_index| {
        stable_indices.iter().find_map(|stable_index| {
            let pair_len = title.len()
                + brief_section_heading(MemoryKind::Stable).len()
                + lines[*stable_index].len()
                + brief_section_heading(MemoryKind::Current).len()
                + lines[*current_index].len();
            (pair_len <= max_chars).then_some((*stable_index, *current_index))
        })
    });
    if let Some((stable_index, current_index)) = anchors {
        select_brief_record(
            stable_index,
            records,
            &lines,
            &mut selected,
            &mut used,
            max_chars,
        );
        select_brief_record(
            current_index,
            records,
            &lines,
            &mut selected,
            &mut used,
            max_chars,
        );
    } else {
        // If there is room for only one anchor, fresh current state takes
        // precedence. A stable entry is the fallback when no fresh checkpoint
        // can fit as a complete line.
        let current_anchor = fresh_current_indices.iter().copied().find(|index| {
            title.len() + brief_section_heading(MemoryKind::Current).len() + lines[*index].len()
                <= max_chars
        });
        let stable_anchor = stable_indices.iter().copied().find(|index| {
            title.len() + brief_section_heading(MemoryKind::Stable).len() + lines[*index].len()
                <= max_chars
        });
        if let Some(index) = current_anchor.or(stable_anchor) {
            select_brief_record(index, records, &lines, &mut selected, &mut used, max_chars);
        }
    }

    // Alternate stable history and fresh current state so either tier can use
    // remaining space without crowding the other one out.
    let stable_remaining = stable_indices
        .iter()
        .copied()
        .filter(|index| !selected[*index]);
    let fresh_current_remaining = fresh_current_indices
        .iter()
        .copied()
        .filter(|index| !selected[*index]);
    let stable_remaining = stable_remaining.collect::<Vec<_>>();
    let fresh_current_remaining = fresh_current_remaining.collect::<Vec<_>>();
    let mut stable_cursor = 0;
    let mut fresh_current_cursor = 0;
    loop {
        let mut selected_any = false;
        if let Some(index) = stable_remaining.get(stable_cursor).copied() {
            stable_cursor += 1;
            select_brief_record(index, records, &lines, &mut selected, &mut used, max_chars);
            selected_any = true;
        }
        if let Some(index) = fresh_current_remaining.get(fresh_current_cursor).copied() {
            fresh_current_cursor += 1;
            select_brief_record(index, records, &lines, &mut selected, &mut used, max_chars);
            selected_any = true;
        }
        if !selected_any {
            break;
        }
    }

    let removed_lines = removed
        .iter()
        .map(|memory_id| {
            let short_id = &memory_id[..memory_id.len().min(8)];
            format!("- [{short_id}] no longer applies\n")
        })
        .collect::<Vec<_>>();
    for (index, line) in removed_lines.iter().enumerate() {
        let heading_cost = if selected_removed.iter().any(|is_selected| *is_selected) {
            0
        } else {
            "\n## Removed or superseded\n".len()
        };
        if used + heading_cost + line.len() <= max_chars {
            selected_removed[index] = true;
            used += heading_cost + line.len();
        }
    }

    for index in &stale_current_indices {
        if !selected[*index] {
            select_brief_record(*index, records, &lines, &mut selected, &mut used, max_chars);
        }
    }

    let mut output = title;
    for (target_kind, heading) in [
        (MemoryKind::Stable, "\n## Stable memory\n"),
        (MemoryKind::Current, "\n## Current state\n"),
    ] {
        if selected
            .iter()
            .zip(records)
            .any(|(is_selected, record)| *is_selected && record.kind == target_kind)
        {
            output.push_str(heading);
            for (index, record) in records.iter().enumerate() {
                if selected[index] && record.kind == target_kind {
                    output.push_str(&lines[index]);
                }
            }
        }
    }
    if selected_removed.iter().any(|is_selected| *is_selected) {
        output.push_str("\n## Removed or superseded\n");
        for (index, is_selected) in selected_removed.iter().enumerate() {
            if *is_selected {
                output.push_str(&removed_lines[index]);
            }
        }
    }

    let omitted = selected.iter().filter(|is_selected| !**is_selected).count()
        + selected_removed
            .iter()
            .filter(|is_selected| !**is_selected)
            .count();
    if omitted > 0 {
        let notice = format!("\n_{omitted} additional memories omitted by the startup budget._\n");
        if output.len() + notice.len() <= max_chars {
            output.push_str(&notice);
        }
    }
    (output.trim().to_string(), omitted)
}

fn brief_section_heading(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Stable => "\n## Stable memory\n",
        MemoryKind::Current => "\n## Current state\n",
    }
}

fn brief_record_is_stale(record: &MemoryRecord, now: DateTime<Utc>) -> bool {
    record.kind == MemoryKind::Current
        && DateTime::parse_from_rfc3339(&record.last_verified_at)
            .map(|value| value.with_timezone(&Utc) < now - Duration::days(DEFAULT_STALE_DAYS))
            .unwrap_or(true)
}

fn brief_record_line(record: &MemoryRecord, stale: bool) -> String {
    let scope = record.workspace.as_deref().unwrap_or("agent-wide");
    let stale_label = if stale { " [stale]" } else { "" };
    let short_id = &record.memory_id[..record.memory_id.len().min(8)];
    format!(
        "- [{short_id}]{} {} (scope: {scope}; verified: {})\n",
        stale_label, record.text, record.last_verified_at
    )
}

fn select_brief_record(
    index: usize,
    records: &[MemoryRecord],
    lines: &[String],
    selected: &mut [bool],
    used: &mut usize,
    max_chars: usize,
) -> bool {
    let kind = records[index].kind;
    let has_section = selected
        .iter()
        .zip(records)
        .any(|(is_selected, record)| *is_selected && record.kind == kind);
    let heading_cost = if has_section {
        0
    } else {
        brief_section_heading(kind).len()
    };
    let added = heading_cost + lines[index].len();
    if *used + added > max_chars {
        return false;
    }
    selected[index] = true;
    *used += added;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(agent: &str, workspace: Option<&str>, text: &str) -> SaveMemoryRequest {
        SaveMemoryRequest {
            agent_id: agent.into(),
            workspace: workspace.map(str::to_string),
            kind: MemoryKind::Stable,
            text: text.into(),
            evidence_excerpt: format!("Evidence for {text}"),
            sources: vec![],
            idempotency_key: None,
        }
    }

    fn brief_record(
        memory_id: impl Into<String>,
        kind: MemoryKind,
        text: impl Into<String>,
        last_verified_at: impl Into<String>,
    ) -> MemoryRecord {
        let memory_id = memory_id.into();
        let text = text.into();
        let evidence_excerpt = format!("Evidence for {text}");
        MemoryRecord {
            revision_id: format!("revision-{memory_id}"),
            memory_id,
            revision: 1,
            agent_id: "agent-a".into(),
            workspace: Some("fixture-workspace".into()),
            kind,
            text,
            evidence_hash: hash_text(&evidence_excerpt),
            evidence_excerpt,
            status: MemoryStatus::Active,
            supersedes_revision_id: None,
            replaced_by_revision_id: None,
            created_at: String::new(),
            updated_at: String::new(),
            last_verified_at: last_verified_at.into(),
            idempotency_key: None,
            sources: vec![],
        }
    }

    #[test]
    fn lifecycle_preserves_revision_history_and_sources() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let mut create = request("agent-a", Some("C:\\Work"), "Prefer concise answers");
        create.sources.push(MemorySource {
            source_type: "conversation".into(),
            locator: Some("conv-1#4".into()),
            source_hash: None,
            primary: true,
        });
        let first = store.save(&MemoryActor::Operator, create).unwrap();
        let expected_workspace = if cfg!(windows) { "c:/work" } else { "C:\\Work" };
        assert_eq!(first.workspace.as_deref(), Some(expected_workspace));
        assert_eq!(first.sources.len(), 1);
        let second = store
            .update(
                &MemoryActor::Operator,
                UpdateMemoryRequest {
                    memory_id: first.memory_id.clone(),
                    text: "Prefer concise technical answers".into(),
                    evidence_excerpt: "User clarified the preference".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        assert_eq!(second.revision, 2);
        let history = store
            .history(&MemoryActor::Operator, &first.memory_id)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].status, MemoryStatus::Superseded);
        assert_eq!(
            store
                .remove(&MemoryActor::Operator, &first.memory_id)
                .unwrap()
                .status,
            MemoryStatus::Removed
        );
        assert!(store
            .list_active(&MemoryActor::Operator, "agent-a", Some("C:/Work"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn agent_memory_lifecycle_accepts_the_injected_short_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let first = store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("one"), "Remember this preference"),
            )
            .unwrap();
        let short_id = first.memory_id[..8].to_string();
        let actor = MemoryActor::agent("agent-a");

        assert_eq!(
            store.get(&actor, &short_id).unwrap().memory_id,
            first.memory_id
        );
        assert_eq!(store.history(&actor, &short_id).unwrap().len(), 1);

        let updated = store
            .update(
                &actor,
                UpdateMemoryRequest {
                    memory_id: short_id.clone(),
                    text: "Remember this preference precisely".into(),
                    evidence_excerpt: "The user clarified the preference".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        assert_eq!(updated.memory_id, first.memory_id);
        assert_eq!(updated.revision, 2);
        assert_eq!(store.history(&actor, &short_id).unwrap().len(), 2);

        let removed = store.remove(&actor, &short_id).unwrap();
        assert_eq!(removed.memory_id, first.memory_id);
        assert_eq!(removed.status, MemoryStatus::Removed);
    }

    #[test]
    fn recall_isolates_agents_and_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        store
            .save(
                &MemoryActor::Operator,
                request("agent-a", None, "Agent preference"),
            )
            .unwrap();
        store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("one"), "Project one"),
            )
            .unwrap();
        store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("two"), "Project two"),
            )
            .unwrap();
        store
            .save(
                &MemoryActor::Operator,
                request("agent-b", Some("one"), "Other agent"),
            )
            .unwrap();
        let recall = store
            .recall(&MemoryActor::agent("agent-a"), "agent-a", Some("one"))
            .unwrap();
        let texts = recall
            .stable
            .into_iter()
            .map(|entry| entry.record.text)
            .collect::<Vec<_>>();
        assert_eq!(texts.len(), 2);
        assert!(texts.contains(&"Agent preference".to_string()));
        assert!(texts.contains(&"Project one".to_string()));
    }

    #[test]
    fn workspace_normalization_preserves_platform_roots_and_posix_backslashes() {
        assert_eq!(
            normalize_workspace_for(Some("/"), false).as_deref(),
            Some("/")
        );
        assert_eq!(
            normalize_workspace_for(Some("//"), false).as_deref(),
            Some("//")
        );
        assert_eq!(
            normalize_workspace_for(Some("project\\data"), false).as_deref(),
            Some("project\\data")
        );
        assert_eq!(
            normalize_workspace_for(Some("project/data/"), false).as_deref(),
            Some("project/data")
        );
        assert_eq!(
            normalize_workspace_for(Some("C:\\"), true).as_deref(),
            Some("c:/")
        );
        assert_eq!(
            normalize_workspace_for(Some("\\\\Server\\Share\\"), true).as_deref(),
            Some("//server/share/")
        );
        assert_eq!(
            normalize_workspace_for(Some("\\\\Server\\Share\\Folder\\"), true).as_deref(),
            Some("//server/share/folder")
        );
        assert_eq!(
            normalize_workspace_for(Some("\\\\?\\C:\\Repo\\"), true).as_deref(),
            Some("c:/repo")
        );
        assert_eq!(
            normalize_workspace_for(Some("\\\\?\\UNC\\Server\\Share\\Folder\\"), true).as_deref(),
            Some("//server/share/folder")
        );
    }

    #[test]
    fn idempotency_returns_the_original_revision() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let mut first = request("agent-a", None, "Remember this");
        first.idempotency_key = Some("run-1:cursor-3".into());
        let original = store.save(&MemoryActor::Operator, first.clone()).unwrap();
        let retry = store.save(&MemoryActor::Operator, first.clone()).unwrap();
        assert_eq!(retry.revision_id, original.revision_id);
        assert_eq!(
            store
                .history(&MemoryActor::Operator, &original.memory_id)
                .unwrap()
                .len(),
            1
        );
        first.text = "Different retry payload".into();
        assert!(matches!(
            store.save(&MemoryActor::Operator, first),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn idempotency_keys_are_global_and_include_source_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let source = |locator: &str| MemorySource {
            source_type: "conversation".into(),
            locator: Some(locator.into()),
            source_hash: None,
            primary: true,
        };
        let mut first = request("agent-a", None, "Remember provenance");
        first.idempotency_key = Some("global-save-key".into());
        first.sources = vec![source("conv-a#1")];
        let saved = store.save(&MemoryActor::Operator, first.clone()).unwrap();

        let mut changed_source = first.clone();
        changed_source.sources = vec![source("conv-a#2")];
        assert!(matches!(
            store.save(&MemoryActor::Operator, changed_source),
            Err(MemoryError::Validation(_))
        ));
        let mut other_agent = request("agent-b", None, "Independent request");
        other_agent.idempotency_key = Some("global-save-key".into());
        assert!(matches!(
            store.save(&MemoryActor::Operator, other_agent),
            Err(MemoryError::Validation(_))
        ));

        let update = UpdateMemoryRequest {
            memory_id: saved.memory_id.clone(),
            text: "Remember provenance precisely".into(),
            evidence_excerpt: "Updated evidence".into(),
            sources: vec![source("conv-a#3")],
            idempotency_key: Some("global-update-key".into()),
        };
        store
            .update(&MemoryActor::Operator, update.clone())
            .unwrap();
        let mut changed_update_source = update;
        changed_update_source.sources = vec![source("conv-a#4")];
        assert!(matches!(
            store.update(&MemoryActor::Operator, changed_update_source),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn fresh_brief_and_resume_delta_are_fingerprinted() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let first = store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("one"), "Use the compact layout"),
            )
            .unwrap();
        let fresh = store
            .compile_brief(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                false,
                8_000,
            )
            .unwrap();
        assert!(fresh.context_text.contains("Stable memory"));
        assert!(fresh.context_text.contains("Use the compact layout"));
        let first_receipt = store
            .record_injection(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                &fresh,
            )
            .unwrap()
            .expect("first injection receipt");
        let second_receipt = store
            .record_injection(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                &fresh,
            )
            .unwrap()
            .expect("second injection receipt");
        assert_ne!(first_receipt, second_receipt);
        let loaded_events = store
            .list_events(&MemoryActor::agent("agent-a"), "agent-a")
            .unwrap()
            .into_iter()
            .filter(|event| event.action == "loaded")
            .collect::<Vec<_>>();
        assert_eq!(
            loaded_events.len(),
            2,
            "every successful launch injection needs its own audit receipt"
        );
        assert!(loaded_events.iter().all(|event| {
            event.payload["budget_policy_version"]
                == serde_json::json!(MEMORY_BUDGET_POLICY_VERSION)
        }));
        let unchanged = store
            .compile_brief(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                true,
                8_000,
            )
            .unwrap();
        assert_eq!(unchanged.kind, MemoryBriefKind::ResumeDelta);
        assert!(unchanged.context_text.contains("Use the compact layout"));

        store
            .update(
                &MemoryActor::Operator,
                UpdateMemoryRequest {
                    memory_id: first.memory_id,
                    text: "Use the extra compact layout".into(),
                    evidence_excerpt: "Preference was refined".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let delta = store
            .compile_brief(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                true,
                8_000,
            )
            .unwrap();
        assert_eq!(delta.kind, MemoryBriefKind::ResumeDelta);
        assert!(delta.context_text.contains("extra compact"));
    }

    #[test]
    fn memory_id_prefix_resolution_prefers_exact_ids_and_rejects_ambiguity() {
        let candidates = vec![
            "deadbeef-0000-0000-0000-000000000001".to_string(),
            "deadbeef-0000-0000-0000-000000000002".to_string(),
            "cafebabe-0000-0000-0000-000000000003".to_string(),
        ];

        assert_eq!(
            resolve_memory_id_from_candidates("cafebabe", &candidates).unwrap(),
            candidates[2]
        );
        assert_eq!(
            resolve_memory_id_from_candidates(&candidates[0], &candidates).unwrap(),
            candidates[0]
        );
        assert!(matches!(
            resolve_memory_id_from_candidates("deadbeef", &candidates),
            Err(MemoryError::MemoryIdAmbiguous(prefix)) if prefix == "deadbeef"
        ));
        assert!(matches!(
            resolve_memory_id_from_candidates("01234567", &candidates),
            Err(MemoryError::NotFound(id)) if id == "01234567"
        ));
    }

    #[test]
    fn consolidation_batch_is_atomic_and_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let batch = MemoryCommitBatch {
            agent_id: "agent-a".into(),
            workspace: Some("one".into()),
            idempotency_key: "run-1:conv-1:8".into(),
            operations: vec![MemoryMutation::Save {
                kind: MemoryKind::Stable,
                text: "Use metric units".into(),
                evidence_excerpt: "The user explicitly selected metric units.".into(),
                sources: vec![MemorySource {
                    source_type: "conversation".into(),
                    locator: Some("conv-1#8".into()),
                    source_hash: None,
                    primary: true,
                }],
            }],
            cursor: Some(MemoryCursorUpdate {
                cursor_key: "agent-a:one".into(),
                conversation_id: Some("conv-1".into()),
                sequence: 8,
            }),
        };
        let actor = MemoryActor::agent("agent-a");
        let first = store.commit_batch(&actor, batch.clone()).unwrap();
        let replay = store.commit_batch(&actor, batch).unwrap();
        assert!(!first.replayed);
        assert!(replay.replayed);
        assert_eq!(first.memory_ids, replay.memory_ids);
        assert_eq!(
            store
                .list_active(&actor, "agent-a", Some("one"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn consolidation_batch_mutations_canonicalize_short_ids() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let first = store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("one"), "Use the short ID safely"),
            )
            .unwrap();
        let short_id = first.memory_id[..8].to_string();
        let result = store
            .commit_batch(
                &MemoryActor::agent("agent-a"),
                MemoryCommitBatch {
                    agent_id: "agent-a".into(),
                    workspace: Some("one".into()),
                    idempotency_key: "run-short-id".into(),
                    operations: vec![MemoryMutation::Update {
                        memory_id: short_id.clone(),
                        text: "Use the canonical ID safely".into(),
                        evidence_excerpt: "The short ID resolved to this memory.".into(),
                        sources: vec![],
                    }],
                    cursor: None,
                },
            )
            .unwrap();

        assert_eq!(result.memory_ids, vec![first.memory_id.clone()]);
        assert_eq!(
            store
                .history(&MemoryActor::agent("agent-a"), &short_id)
                .unwrap()
                .len(),
            2
        );

        store
            .remove(&MemoryActor::agent("agent-a"), &short_id)
            .unwrap();
        let replay = store
            .commit_batch(
                &MemoryActor::agent("agent-a"),
                MemoryCommitBatch {
                    agent_id: "agent-a".into(),
                    workspace: Some("one".into()),
                    idempotency_key: "run-short-id".into(),
                    operations: vec![MemoryMutation::Update {
                        memory_id: first.memory_id.clone(),
                        text: "Use the canonical ID safely".into(),
                        evidence_excerpt: "The short ID resolved to this memory.".into(),
                        sources: vec![],
                    }],
                    cursor: None,
                },
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.memory_ids, vec![first.memory_id]);
    }

    #[test]
    fn recall_labels_stale_current_and_excludes_invalid_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory.db");
        let store = MemoryStore::open(&path).unwrap();
        let mut current = request("agent-a", Some("one"), "Release is awaiting validation");
        current.kind = MemoryKind::Current;
        let stale = store.save(&MemoryActor::Operator, current).unwrap();
        let invalid = store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("one"), "Corrupted evidence"),
            )
            .unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "UPDATE memory_records SET last_verified_at=?1 WHERE revision_id=?2",
                params![
                    (Utc::now() - Duration::days(DEFAULT_STALE_DAYS + 1)).to_rfc3339(),
                    stale.revision_id
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE memory_records SET evidence_hash='invalid' WHERE revision_id=?1",
                [&invalid.revision_id],
            )
            .unwrap();

        let recall = store
            .recall(&MemoryActor::agent("agent-a"), "agent-a", Some("one"))
            .unwrap();
        assert!(recall.stable.is_empty());
        assert_eq!(recall.current.len(), 1);
        assert!(recall.current[0].stale);
        assert_eq!(recall.current[0].record.memory_id, stale.memory_id);
    }

    #[test]
    fn bounded_brief_is_deterministic_and_reports_omissions() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        for index in 0..8 {
            store
                .save(
                    &MemoryActor::Operator,
                    request(
                        "agent-a",
                        Some("one"),
                        &format!("Preference {index}: {}", "x".repeat(90)),
                    ),
                )
                .unwrap();
        }

        let first = store
            .compile_brief(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "fresh-a",
                false,
                360,
            )
            .unwrap();
        let second = store
            .compile_brief(
                &MemoryActor::agent("agent-a"),
                "agent-a",
                Some("one"),
                "codex",
                "fresh-b",
                false,
                360,
            )
            .unwrap();
        assert_eq!(first.context_text, second.context_text);
        assert_eq!(first.fingerprint, second.fingerprint);
        assert!(first.omitted_count > 0);
        assert!(first.context_text.len() <= 360);
    }

    #[test]
    fn twelve_kilobyte_brief_keeps_current_checkpoint_with_66_stable_records() {
        // Mirrors the frozen 67-active-record source-policy case with sanitized
        // journal text and the checkpoint identity retained.
        let verified = (Utc::now() - Duration::days(1)).to_rfc3339();
        let mut records = (0..66)
            .map(|index| {
                brief_record(
                    format!("{index:08x}-0000-0000-0000-{index:012x}"),
                    MemoryKind::Stable,
                    format!("Stable journal {index}: {}", "x".repeat(250)),
                    verified.clone(),
                )
            })
            .collect::<Vec<_>>();
        records.push(brief_record(
            "e1f065d2-a347-4565-9cf0-53f2c4c97f48",
            MemoryKind::Current,
            "EE follow-up checkpoint: routing changes are applied; live-use benefit remains unverified. The existing EE-only deployment targets and Konnect writes remain guarded. A paired decision screen scored baseline 4/6 and candidate 6/6 with preservation gates passing; static review found no blockers. No live CAD or activation test ran, and no monitor is scheduled. On the next applicable audit, check branch preservation, electrical tradeoffs, and scope overreach.",
            verified,
        ));

        let (first, omitted) = render_brief(MemoryBriefKind::Fresh, &records, &[], 12_000);
        let (second, second_omitted) = render_brief(MemoryBriefKind::Fresh, &records, &[], 12_000);
        let visible_records = first.lines().filter(|line| line.starts_with("- [")).count();

        assert_eq!(records.len(), 67);
        assert!(first.len() <= 12_000);
        assert!(first.contains("[e1f065d2]"));
        assert!(first.contains("EE follow-up checkpoint: routing changes are applied"));
        assert!(first.contains("Stable journal 0:"));
        assert_eq!(omitted, records.len() - visible_records);
        assert!(omitted > 0);
        assert_eq!(first, second);
        assert_eq!(omitted, second_omitted);
    }

    #[test]
    fn twelve_kilobyte_brief_keeps_stable_history_with_66_current_records() {
        let verified = (Utc::now() - Duration::days(1)).to_rfc3339();
        let mut records = vec![brief_record(
            "stable-anchor-memory",
            MemoryKind::Stable,
            "Stable rule survives current-heavy history.",
            verified.clone(),
        )];
        records.extend((0..66).map(|index| {
            brief_record(
                format!("{index:08x}-1111-2222-3333-{index:012x}"),
                MemoryKind::Current,
                format!("Current journal {index}: {}", "c".repeat(250)),
                verified.clone(),
            )
        }));

        let (context, omitted) = render_brief(MemoryBriefKind::Fresh, &records, &[], 12_000);
        let visible_records = context
            .lines()
            .filter(|line| line.starts_with("- ["))
            .count();

        assert!(context.len() <= 12_000);
        assert!(context.contains("Stable rule survives current-heavy history."));
        assert!(context.contains("Current journal 0:"));
        assert!(omitted > 0);
        assert_eq!(omitted, records.len() - visible_records);
    }

    #[test]
    fn brief_selection_round_robins_stable_and_fresh_current_tiers() {
        let verified = (Utc::now() - Duration::days(1)).to_rfc3339();
        let mut records = (0..20)
            .map(|index| {
                brief_record(
                    format!("a{index:07x}-1111-2222-3333-{index:012x}"),
                    MemoryKind::Stable,
                    format!("Rule {index:02}: {}", "x".repeat(120)),
                    verified.clone(),
                )
            })
            .collect::<Vec<_>>();
        records.extend((0..20).map(|index| {
            brief_record(
                format!("b{index:07x}-1111-2222-3333-{index:012x}"),
                MemoryKind::Current,
                format!("Rule {index:02}: {}", "x".repeat(120)),
                verified.clone(),
            )
        }));

        let stable_line = brief_record_line(&records[0], false);
        let current_line = brief_record_line(&records[20], false);
        let max_chars = "# Wardian memory\n".len()
            + brief_section_heading(MemoryKind::Stable).len()
            + stable_line.len()
            + brief_section_heading(MemoryKind::Current).len()
            + current_line.len()
            + 5 * (stable_line.len() + current_line.len());
        let (context, omitted) = render_brief(MemoryBriefKind::Fresh, &records, &[], max_chars);
        let stable_count = context
            .split("## Stable memory\n")
            .nth(1)
            .unwrap()
            .split("## Current state\n")
            .next()
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("- ["))
            .count();
        let current_count = context
            .split("## Current state\n")
            .nth(1)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("- ["))
            .count();

        assert_eq!(stable_count, 6);
        assert_eq!(current_count, 6);
        assert_eq!(omitted, 28);
        assert!(context.len() <= max_chars);
    }

    #[test]
    fn brief_selection_never_truncates_a_record_to_fill_a_tight_budget() {
        let verified = Utc::now().to_rfc3339();
        let record = brief_record(
            "tight-budget-memory",
            MemoryKind::Current,
            "Preserve this complete checkpoint statement without changing its meaning.",
            verified,
        );
        let complete_line = brief_record_line(&record, false);
        let max_chars = "# Wardian memory\n".len()
            + brief_section_heading(MemoryKind::Current).len()
            + complete_line.len()
            - 1;

        let (context, omitted) = render_brief(MemoryBriefKind::Fresh, &[record], &[], max_chars);

        assert!(context.len() <= max_chars);
        assert!(!context.contains("Preserve this complete checkpoint"));
        assert_eq!(omitted, 1);
    }

    #[test]
    fn brief_selection_prefers_fresh_current_over_stale_current() {
        let now = Utc::now();
        let fresh = brief_record(
            "fresh-current-memory",
            MemoryKind::Current,
            "Fresh checkpoint stays visible.",
            now.to_rfc3339(),
        );
        let stale = brief_record(
            "stale-current-memory",
            MemoryKind::Current,
            "Stale checkpoint should be omitted.",
            (now - Duration::days(DEFAULT_STALE_DAYS + 1)).to_rfc3339(),
        );
        let max_chars = "# Wardian memory\n".len()
            + brief_section_heading(MemoryKind::Current).len()
            + brief_record_line(&fresh, false).len();

        let (context, omitted) =
            render_brief(MemoryBriefKind::Fresh, &[stale, fresh], &[], max_chars);

        assert!(context.len() <= max_chars);
        assert!(context.contains("Fresh checkpoint stays visible."));
        assert!(!context.contains("Stale checkpoint should be omitted."));
        assert_eq!(omitted, 1);
    }

    #[test]
    fn brief_selection_keeps_removal_notices_ahead_of_stale_current() {
        let stale = brief_record(
            "stale-current-memory",
            MemoryKind::Current,
            format!("Stale checkpoint: {}", "x".repeat(200)),
            (Utc::now() - Duration::days(DEFAULT_STALE_DAYS + 1)).to_rfc3339(),
        );
        let removed = vec!["removed-memory".into()];
        let max_chars = "# Wardian memory changes\n".len()
            + "\n## Removed or superseded\n".len()
            + "- [removed-] no longer applies\n".len();

        let (context, omitted) =
            render_brief(MemoryBriefKind::ResumeDelta, &[stale], &removed, max_chars);

        assert!(context.len() <= max_chars);
        assert!(context.contains("no longer applies"));
        assert!(!context.contains("Stale checkpoint:"));
        assert_eq!(omitted, 1);
    }

    #[test]
    fn brief_omitted_count_includes_unrendered_removal_notices() {
        let max_chars = "# Wardian memory changes\n".len();
        let removed = vec!["memory-1".into(), "memory-2".into(), "memory-3".into()];

        let (context, omitted) =
            render_brief(MemoryBriefKind::ResumeDelta, &[], &removed, max_chars);

        assert!(context.len() <= max_chars);
        assert_eq!(omitted, removed.len());
    }

    #[test]
    fn bounded_brief_scope_and_resume_keep_the_full_revision_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let actor = MemoryActor::agent("agent-a");
        let stable_anchor = store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("one"), "Workspace preference"),
            )
            .unwrap();
        let omitted_stable = store
            .save(
                &MemoryActor::Operator,
                request(
                    "agent-a",
                    Some("one"),
                    &format!("Historical journal: {}", "x".repeat(800)),
                ),
            )
            .unwrap();
        for index in 0..2 {
            store
                .save(
                    &MemoryActor::Operator,
                    request(
                        "agent-a",
                        Some("one"),
                        &format!("Historical journal {index}: {}", "y".repeat(800)),
                    ),
                )
                .unwrap();
        }
        let mut current_request = request("agent-a", Some("one"), "Fresh workspace checkpoint");
        current_request.kind = MemoryKind::Current;
        let current = store.save(&MemoryActor::Operator, current_request).unwrap();
        store
            .save(
                &MemoryActor::Operator,
                request("agent-a", Some("two"), "Other-workspace secret"),
            )
            .unwrap();
        store
            .save(
                &MemoryActor::Operator,
                request("agent-b", Some("one"), "Peer-agent secret"),
            )
            .unwrap();

        let initial = store
            .compile_brief(
                &actor,
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                false,
                600,
            )
            .unwrap();
        let unbounded = store
            .compile_brief(
                &actor,
                "agent-a",
                Some("one"),
                "codex",
                "session-unbounded",
                false,
                10_000,
            )
            .unwrap();
        let scoped = store.recall(&actor, "agent-a", Some("one")).unwrap();
        let scoped_revision_ids = scoped
            .stable
            .iter()
            .chain(scoped.current.iter())
            .map(|entry| entry.record.revision_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(initial.revision_ids, scoped_revision_ids);
        assert_eq!(initial.revision_ids.len(), 5);
        assert_eq!(initial.fingerprint, unbounded.fingerprint);
        assert_eq!(initial.revision_ids, unbounded.revision_ids);
        assert_ne!(initial.context_text, unbounded.context_text);
        assert!(initial.omitted_count >= 3);
        assert!(initial.context_text.contains("Workspace preference"));
        assert!(initial.context_text.contains("Fresh workspace checkpoint"));
        assert!(!initial.context_text.contains("Other-workspace secret"));
        assert!(!initial.context_text.contains("Peer-agent secret"));

        store
            .record_injection(
                &actor,
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                &initial,
            )
            .unwrap();
        store
            .update(
                &MemoryActor::Operator,
                UpdateMemoryRequest {
                    memory_id: current.memory_id.clone(),
                    text: "Updated workspace current checkpoint".into(),
                    evidence_excerpt: "The current workspace checkpoint was refreshed.".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();

        let current_delta = store
            .compile_brief(
                &actor,
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                true,
                600,
            )
            .unwrap();
        assert_eq!(current_delta.kind, MemoryBriefKind::ResumeDelta);
        assert!(current_delta
            .context_text
            .contains("Updated workspace current checkpoint"));
        assert!(!current_delta.context_text.contains("Workspace preference"));
        assert_eq!(current_delta.revision_ids.len(), 5);
        assert_ne!(current_delta.revision_ids, initial.revision_ids);
        assert_ne!(current_delta.fingerprint, initial.fingerprint);
        store
            .record_injection(
                &actor,
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                &current_delta,
            )
            .unwrap();

        store
            .update(
                &MemoryActor::Operator,
                UpdateMemoryRequest {
                    memory_id: omitted_stable.memory_id,
                    text: "Updated omitted journal checkpoint".into(),
                    evidence_excerpt: "The omitted journal was revised.".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();

        let resumed = store
            .compile_brief(
                &actor,
                "agent-a",
                Some("one"),
                "codex",
                "session-1",
                true,
                600,
            )
            .unwrap();
        let updated_scope = store.recall(&actor, "agent-a", Some("one")).unwrap();
        let updated_revision_ids = updated_scope
            .stable
            .iter()
            .chain(updated_scope.current.iter())
            .map(|entry| entry.record.revision_id.clone())
            .collect::<Vec<_>>();

        assert_eq!(resumed.kind, MemoryBriefKind::ResumeDelta);
        assert_ne!(resumed.fingerprint, current_delta.fingerprint);
        assert_eq!(resumed.revision_ids, updated_revision_ids);
        assert_eq!(resumed.revision_ids.len(), 5);
        assert!(resumed
            .context_text
            .contains("Updated omitted journal checkpoint"));
        assert!(!resumed
            .context_text
            .contains("Updated workspace current checkpoint"));
        assert!(!resumed.context_text.contains("Other-workspace secret"));
        assert!(!resumed.context_text.contains("Peer-agent secret"));
        assert_eq!(stable_anchor.kind, MemoryKind::Stable);
        assert_eq!(current.kind, MemoryKind::Current);
    }

    #[test]
    fn invalid_batch_rolls_back_operations_and_cursor() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory.db");
        let store = MemoryStore::open(&path).unwrap();
        let batch = MemoryCommitBatch {
            agent_id: "agent-a".into(),
            workspace: Some("one".into()),
            idempotency_key: "bad-batch".into(),
            operations: vec![
                MemoryMutation::Save {
                    kind: MemoryKind::Stable,
                    text: "Valid first operation".into(),
                    evidence_excerpt: "Valid evidence".into(),
                    sources: vec![],
                },
                MemoryMutation::Update {
                    memory_id: "missing-memory".into(),
                    text: "Cannot update".into(),
                    evidence_excerpt: "No source record".into(),
                    sources: vec![],
                },
            ],
            cursor: Some(MemoryCursorUpdate {
                cursor_key: "agent-a:one".into(),
                conversation_id: Some("conv-1".into()),
                sequence: 3,
            }),
        };

        assert!(matches!(
            store.commit_batch(&MemoryActor::agent("agent-a"), batch),
            Err(MemoryError::NotFound(_))
        ));
        assert!(store
            .list_active(&MemoryActor::agent("agent-a"), "agent-a", Some("one"))
            .unwrap()
            .is_empty());
        let connection = Connection::open(path).unwrap();
        let cursors: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_consolidation_cursors",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursors, 0);
    }

    #[test]
    fn stale_consolidation_batch_cannot_move_cursor_backwards_or_mutate_memory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory.db");
        let newer_store = MemoryStore::open(&path).unwrap();
        let stale_store = MemoryStore::open(&path).unwrap();
        let batch = |idempotency_key: &str,
                     cursor_key: &str,
                     conversation_id: &str,
                     sequence: u64,
                     text: &str| MemoryCommitBatch {
            agent_id: "agent-a".into(),
            workspace: Some("one".into()),
            idempotency_key: idempotency_key.into(),
            operations: vec![MemoryMutation::Save {
                kind: MemoryKind::Current,
                text: text.into(),
                evidence_excerpt: format!("Evidence at sequence {sequence}"),
                sources: vec![],
            }],
            cursor: Some(MemoryCursorUpdate {
                cursor_key: cursor_key.into(),
                conversation_id: Some(conversation_id.into()),
                sequence,
            }),
        };
        let actor = MemoryActor::agent("agent-a");

        newer_store
            .commit_batch(
                &actor,
                batch(
                    "newer-boundary",
                    "documented-key",
                    "conv-1",
                    20,
                    "Authoritative memory",
                ),
            )
            .unwrap();
        assert!(matches!(
            stale_store.commit_batch(
                &actor,
                batch(
                    "stale-boundary",
                    "model-selected-bypass-key",
                    "conv-1",
                    10,
                    "Stale duplicate",
                ),
            ),
            Err(MemoryError::Validation(message)) if message.contains("cannot move")
        ));

        let memories = newer_store
            .list_active(&actor, "agent-a", Some("one"))
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].text, "Authoritative memory");
        let connection = Connection::open(path).unwrap();
        let canonical_key =
            canonical_consolidation_cursor_key("agent-a", Some("one"), Some("conv-1"));
        let sequence: u64 = connection
            .query_row(
                "SELECT sequence FROM memory_consolidation_cursors WHERE cursor_key=?1",
                [canonical_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sequence, 20);
        let receipts: i64 = connection
            .query_row("SELECT COUNT(*) FROM memory_commits", [], |row| row.get(0))
            .unwrap();
        assert_eq!(receipts, 1);
    }

    #[test]
    fn consolidation_cursor_starts_a_distinct_epoch_for_each_conversation() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let actor = MemoryActor::agent("agent-a");
        for (conversation_id, idempotency_key, text) in [
            ("conv-1", "boundary-1", "First conversation"),
            ("conv-2", "boundary-2", "Second conversation"),
        ] {
            store
                .commit_batch(
                    &actor,
                    MemoryCommitBatch {
                        agent_id: "agent-a".into(),
                        workspace: Some("one".into()),
                        idempotency_key: idempotency_key.into(),
                        operations: vec![MemoryMutation::Save {
                            kind: MemoryKind::Current,
                            text: text.into(),
                            evidence_excerpt: format!("Evidence from {conversation_id}"),
                            sources: vec![],
                        }],
                        cursor: Some(MemoryCursorUpdate {
                            cursor_key: "memory-consolidation".into(),
                            conversation_id: Some(conversation_id.into()),
                            sequence: 1,
                        }),
                    },
                )
                .unwrap();
        }
        assert_eq!(
            store
                .list_active(&actor, "agent-a", Some("one"))
                .unwrap()
                .len(),
            2
        );
        let connection = Connection::open(store.path()).unwrap();
        let cursors: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_consolidation_cursors",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursors, 2);
    }

    #[test]
    fn agent_actor_cannot_read_or_mutate_peer_memory() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let mut peer_request = request("agent-b", Some("one"), "Peer-only preference");
        peer_request.idempotency_key = Some("peer-idempotency-key".into());
        let peer = store.save(&MemoryActor::Operator, peer_request).unwrap();
        let actor = MemoryActor::agent("agent-a");

        assert!(matches!(
            store.save(&actor, request("agent-b", Some("one"), "Unauthorized save")),
            Err(MemoryError::AccessDenied { .. })
        ));
        assert!(matches!(
            store.list_active(&actor, "agent-b", Some("one")),
            Err(MemoryError::AccessDenied { .. })
        ));
        assert!(matches!(
            store.recall(&actor, "agent-b", Some("one")),
            Err(MemoryError::AccessDenied { .. })
        ));
        assert!(matches!(
            store.get(&actor, &peer.memory_id),
            Err(MemoryError::NotFound(_))
        ));
        assert!(matches!(
            store.history(&actor, &peer.memory_id),
            Err(MemoryError::NotFound(_))
        ));
        assert!(matches!(
            store.update(
                &actor,
                UpdateMemoryRequest {
                    memory_id: peer.memory_id.clone(),
                    text: "Unauthorized update".into(),
                    evidence_excerpt: "No authority".into(),
                    sources: vec![],
                    idempotency_key: None,
                }
            ),
            Err(MemoryError::NotFound(_))
        ));
        assert!(matches!(
            store.update(
                &actor,
                UpdateMemoryRequest {
                    memory_id: peer.memory_id.clone(),
                    text: peer.text.clone(),
                    evidence_excerpt: peer.evidence_excerpt.clone(),
                    sources: vec![],
                    idempotency_key: Some("peer-idempotency-key".into()),
                }
            ),
            Err(MemoryError::Validation(message)) if message == "idempotency key is unavailable"
        ));
        assert!(matches!(
            store.remove(&actor, &peer.memory_id),
            Err(MemoryError::NotFound(_))
        ));
        assert!(matches!(
            store.list_events(&actor, "agent-b"),
            Err(MemoryError::AccessDenied { .. })
        ));

        let stored = store.get(&MemoryActor::Operator, &peer.memory_id).unwrap();
        assert_eq!(stored.text, "Peer-only preference");
        assert_eq!(stored.status, MemoryStatus::Active);
    }

    #[test]
    fn capabilities_are_agent_bound_and_support_concurrent_processes() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let first = store.issue_capability("agent-a").unwrap();
        assert!(store.validate_capability("agent-a", &first).unwrap());
        assert!(!store.validate_capability("agent-b", &first).unwrap());

        let second = store.issue_capability("agent-a").unwrap();
        assert_ne!(first, second);
        assert!(store.validate_capability("agent-a", &first).unwrap());
        assert!(store.validate_capability("agent-a", &second).unwrap());
    }

    #[test]
    fn dropping_one_process_capability_revokes_only_that_process() {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let first = store.issue_process_capability("agent-a").unwrap();
        let first_token = first.token().to_string();
        let second = store.issue_process_capability("agent-a").unwrap();
        let second_token = second.token().to_string();

        drop(first);

        assert!(!store.validate_capability("agent-a", &first_token).unwrap());
        assert!(store.validate_capability("agent-a", &second_token).unwrap());
        drop(second);
        assert!(!store.validate_capability("agent-a", &second_token).unwrap());
    }
}
