//! Revision-bound memory maintenance for Wardian operator review.
//!
//! Provides strict one-owner plan preview, replay, atomic apply, and receipt lookup.

use super::{
    hash_text, normalize_evidence, normalize_text, normalize_workspace, required,
    sources_for_revision, MemoryActor, MemoryError, MemoryKind, MemoryRecord, MemorySource,
    MemoryStore,
};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const MAX_PLAN_BYTES: usize = 1024 * 1024;
pub const MAX_OPERATIONS: usize = 100;
pub const MAX_TEXT_CHARS: usize = 8192;
pub const MAX_LOCATOR_CHARS: usize = 4096;
pub const MAX_SOURCES_PER_OP: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryMaintenanceScope {
    Agent,
    Workspace { path: String },
}

impl<'de> Deserialize<'de> for MemoryMaintenanceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;

        let kind_val = map
            .get("kind")
            .ok_or_else(|| serde::de::Error::missing_field("kind"))?;
        let kind = kind_val
            .as_str()
            .ok_or_else(|| serde::de::Error::custom("field 'kind' must be a string"))?;

        match kind {
            "agent" => {
                for key in map.keys() {
                    if key != "kind" {
                        return Err(serde::de::Error::unknown_field(key, &["kind"]));
                    }
                }
                Ok(Self::Agent)
            }
            "workspace" => {
                for key in map.keys() {
                    if key != "kind" && key != "path" {
                        return Err(serde::de::Error::unknown_field(key, &["kind", "path"]));
                    }
                }
                let path_val = map
                    .get("path")
                    .ok_or_else(|| serde::de::Error::missing_field("path"))?;
                let path = path_val
                    .as_str()
                    .ok_or_else(|| serde::de::Error::custom("field 'path' must be a string"))?;
                Ok(Self::Workspace {
                    path: path.to_string(),
                })
            }
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["agent", "workspace"],
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryMaintenanceOperation {
    Revise {
        memory_id: String,
        expected_revision_id: String,
        text: String,
        kind: MemoryKind,
        scope: MemoryMaintenanceScope,
        evidence_excerpt: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add_sources: Vec<MemorySource>,
    },
    Create {
        client_key: String,
        text: String,
        kind: MemoryKind,
        scope: MemoryMaintenanceScope,
        evidence_excerpt: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sources: Vec<MemorySource>,
    },
    Retire {
        memory_id: String,
        expected_revision_id: String,
        reason: String,
    },
    RetireInto {
        memory_id: String,
        expected_revision_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_memory_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_client_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_expected_revision_id: Option<String>,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMaintenancePlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub agent_id: String,
    pub idempotency_key: String,
    pub operations: Vec<MemoryMaintenanceOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySummary {
    #[serde(default)]
    pub memory_id: Option<String>,
    #[serde(default)]
    pub revision_id: Option<String>,
    pub text: String,
    pub kind: MemoryKind,
    pub scope: MemoryMaintenanceScope,
    pub evidence_excerpt: String,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMaintenanceChange {
    pub operation_index: usize,
    pub op: String,
    #[serde(default)]
    pub before: Option<MemorySummary>,
    #[serde(default)]
    pub after: Option<MemorySummary>,
    #[serde(default)]
    pub source_additions: Vec<MemorySource>,
    #[serde(default)]
    pub absorbed_memory_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMaintenanceConflict {
    pub operation_index: usize,
    pub code: String,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMaintenancePreview {
    pub plan_id: String,
    pub agent_id: String,
    pub operation_count: usize,
    pub preview_digest: String,
    pub changes: Vec<MemoryMaintenanceChange>,
    pub conflicts: Vec<MemoryMaintenanceConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMaintenanceOperationReceipt {
    pub operation_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    pub memory_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMaintenanceReceipt {
    pub plan_id: String,
    pub agent_id: String,
    pub idempotency_key: String,
    pub preview_digest: String,
    pub applied_at: String,
    pub operations: Vec<MemoryMaintenanceOperationReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TargetKey {
    Memory(String),
    ClientKey(String),
}

fn require_operator(actor: &MemoryActor, subject_agent_id: &str) -> Result<(), MemoryError> {
    match actor {
        MemoryActor::Operator => Ok(()),
        MemoryActor::Agent(actor_agent_id) => Err(MemoryError::AccessDenied {
            actor_agent_id: actor_agent_id.clone(),
            subject_agent_id: subject_agent_id.to_string(),
        }),
    }
}

fn validate_uuid(field: &str, value: &str) -> Result<(), MemoryError> {
    Uuid::parse_str(value.trim()).map_err(|_| {
        MemoryError::Validation(format!("{field} '{value}' is not a valid full UUID"))
    })?;
    Ok(())
}

fn validate_text_length(field: &str, value: &str, max_chars: usize) -> Result<(), MemoryError> {
    if value.chars().count() > max_chars {
        return Err(MemoryError::Validation(format!(
            "{field} exceeds maximum of {max_chars} characters"
        )));
    }
    Ok(())
}

fn validate_sources(sources: &[MemorySource]) -> Result<(), MemoryError> {
    if sources.len() > MAX_SOURCES_PER_OP {
        return Err(MemoryError::Validation(format!(
            "sources count {} exceeds maximum of {MAX_SOURCES_PER_OP}",
            sources.len()
        )));
    }
    for source in sources {
        required("source_type", &source.source_type)?;
        if let Some(locator) = &source.locator {
            if locator.chars().count() > MAX_LOCATOR_CHARS {
                return Err(MemoryError::Validation(format!(
                    "source locator exceeds maximum of {MAX_LOCATOR_CHARS} characters"
                )));
            }
        }
    }
    Ok(())
}

fn validate_scope(scope: &MemoryMaintenanceScope) -> Result<(), MemoryError> {
    match scope {
        MemoryMaintenanceScope::Agent => Ok(()),
        MemoryMaintenanceScope::Workspace { path } => {
            let path = required("scope path", path)?;
            if !std::path::Path::new(&path).is_absolute() {
                return Err(MemoryError::Validation(format!(
                    "workspace scope path '{path}' must be an absolute path"
                )));
            }
            Ok(())
        }
    }
}

pub fn validate_plan_structure(plan: &MemoryMaintenancePlan) -> Result<(), MemoryError> {
    if plan.schema_version != 1 {
        return Err(MemoryError::Validation(format!(
            "unsupported plan schema_version {}",
            plan.schema_version
        )));
    }
    required("plan_id", &plan.plan_id)?;
    required("agent_id", &plan.agent_id)?;
    required("idempotency_key", &plan.idempotency_key)?;

    if plan.operations.is_empty() || plan.operations.len() > MAX_OPERATIONS {
        return Err(MemoryError::Validation(format!(
            "operations count {} outside allowed range 1-100",
            plan.operations.len()
        )));
    }

    let encoded = serde_json::to_vec(plan)?;
    if encoded.len() > MAX_PLAN_BYTES {
        return Err(MemoryError::Validation(format!(
            "plan exceeds 1 MiB limit: {} bytes",
            encoded.len()
        )));
    }

    let mut source_memories: HashSet<String> = HashSet::new();
    let mut client_keys: HashSet<String> = HashSet::new();
    let mut retired_memories: HashSet<String> = HashSet::new();
    let mut revise_targets: HashSet<String> = HashSet::new();

    for operation in &plan.operations {
        match operation {
            MemoryMaintenanceOperation::Revise {
                memory_id,
                expected_revision_id,
                text,
                evidence_excerpt,
                add_sources,
                scope,
                ..
            } => {
                validate_uuid("memory_id", memory_id)?;
                validate_uuid("expected_revision_id", expected_revision_id)?;
                validate_text_length("text", text, MAX_TEXT_CHARS)?;
                validate_text_length("evidence_excerpt", evidence_excerpt, MAX_TEXT_CHARS)?;
                validate_sources(add_sources)?;
                validate_scope(scope)?;

                if !source_memories.insert(memory_id.clone()) {
                    return Err(MemoryError::Validation(format!(
                        "memory_id {memory_id} appears in multiple source operations"
                    )));
                }
                revise_targets.insert(memory_id.clone());
            }
            MemoryMaintenanceOperation::Create {
                client_key,
                text,
                evidence_excerpt,
                sources,
                scope,
                ..
            } => {
                required("client_key", client_key)?;
                if !client_keys.insert(client_key.clone()) {
                    return Err(MemoryError::Validation(format!(
                        "duplicate client_key '{client_key}'"
                    )));
                }
                validate_text_length("text", text, MAX_TEXT_CHARS)?;
                validate_text_length("evidence_excerpt", evidence_excerpt, MAX_TEXT_CHARS)?;
                validate_sources(sources)?;
                validate_scope(scope)?;
            }
            MemoryMaintenanceOperation::Retire {
                memory_id,
                expected_revision_id,
                reason,
            } => {
                validate_uuid("memory_id", memory_id)?;
                validate_uuid("expected_revision_id", expected_revision_id)?;
                required("reason", reason)?;
                validate_text_length("reason", reason, MAX_TEXT_CHARS)?;

                if !source_memories.insert(memory_id.clone()) {
                    return Err(MemoryError::Validation(format!(
                        "memory_id {memory_id} appears in multiple source operations"
                    )));
                }
                retired_memories.insert(memory_id.clone());
            }
            MemoryMaintenanceOperation::RetireInto {
                memory_id,
                expected_revision_id,
                target_memory_id,
                target_client_key,
                target_expected_revision_id,
                reason,
            } => {
                validate_uuid("memory_id", memory_id)?;
                validate_uuid("expected_revision_id", expected_revision_id)?;
                required("reason", reason)?;
                validate_text_length("reason", reason, MAX_TEXT_CHARS)?;

                if !source_memories.insert(memory_id.clone()) {
                    return Err(MemoryError::Validation(format!(
                        "memory_id {memory_id} appears in multiple source operations"
                    )));
                }
                retired_memories.insert(memory_id.clone());

                match (target_memory_id, target_client_key) {
                    (Some(t_mem), None) => {
                        validate_uuid("target_memory_id", t_mem)?;
                        if t_mem == memory_id {
                            return Err(MemoryError::Validation(format!(
                                "self-targeting retire_into on memory_id {memory_id}"
                            )));
                        }
                        if let Some(target_exp_rev) = target_expected_revision_id {
                            validate_uuid("target_expected_revision_id", target_exp_rev)?;
                        }
                    }
                    (None, Some(t_key)) => {
                        required("target_client_key", t_key)?;
                        if target_expected_revision_id.is_some() {
                            return Err(MemoryError::Validation(
                                "target_expected_revision_id cannot be set when targeting target_client_key".into(),
                            ));
                        }
                    }
                    _ => {
                        return Err(MemoryError::Validation(
                            "retire_into requires exactly one of target_memory_id or target_client_key".into(),
                        ));
                    }
                }
            }
        }
    }

    // Second pass for structural references
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::RetireInto {
            target_memory_id,
            target_client_key,
            target_expected_revision_id,
            ..
        } = operation
        {
            if let Some(t_key) = target_client_key {
                if !client_keys.contains(t_key) {
                    return Err(MemoryError::Validation(format!(
                        "target_client_key '{t_key}' not defined in plan"
                    )));
                }
            } else if let Some(t_mem) = target_memory_id {
                if retired_memories.contains(t_mem) {
                    return Err(MemoryError::Validation(format!(
                        "target_memory_id {t_mem} is retired in the same plan"
                    )));
                }
                if !revise_targets.contains(t_mem) && target_expected_revision_id.is_none() {
                    return Err(MemoryError::Validation(format!(
                        "target_expected_revision_id is required for target_memory_id {t_mem} without a revise operation"
                    )));
                }
            }
        }
    }

    Ok(())
}

pub fn compute_plan_hash(plan: &MemoryMaintenancePlan) -> Result<String, MemoryError> {
    let canonical = serde_json::to_string(plan)?;
    Ok(hash_text(&canonical))
}

fn write_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn compute_preview_digest(
    plan: &MemoryMaintenancePlan,
    active_revisions: &[(String, String)],
) -> Result<String, MemoryError> {
    let mut hasher = Sha256::new();
    hasher.update(b"wardian:memory_maintenance:v1\0");

    write_len_prefixed(&mut hasher, plan.plan_id.as_bytes());
    write_len_prefixed(&mut hasher, plan.agent_id.as_bytes());
    write_len_prefixed(&mut hasher, plan.idempotency_key.as_bytes());

    let canonical_ops = serde_json::to_string(&plan.operations)?;
    write_len_prefixed(&mut hasher, canonical_ops.as_bytes());

    let mut sorted_revisions = active_revisions.to_vec();
    sorted_revisions.sort();
    hasher.update((sorted_revisions.len() as u64).to_be_bytes());
    for (mem_id, rev_id) in sorted_revisions {
        write_len_prefixed(&mut hasher, mem_id.as_bytes());
        write_len_prefixed(&mut hasher, rev_id.as_bytes());
    }

    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn normalize_source(source: &MemorySource) -> Result<MemorySource, MemoryError> {
    let source_type = required("source_type", &source.source_type)?;
    let locator = source
        .locator
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let source_hash = source
        .source_hash
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(MemorySource {
        source_type,
        locator,
        source_hash,
        primary: source.primary,
    })
}

fn combine_sources(
    existing: &[MemorySource],
    additions: &[MemorySource],
    absorbed: &[MemorySource],
) -> Result<(Vec<MemorySource>, Vec<MemorySource>), MemoryError> {
    let norm_existing: Vec<MemorySource> = existing
        .iter()
        .map(normalize_source)
        .collect::<Result<_, _>>()?;
    let norm_additions: Vec<MemorySource> = additions
        .iter()
        .map(normalize_source)
        .collect::<Result<_, _>>()?;
    let norm_absorbed: Vec<MemorySource> = absorbed
        .iter()
        .map(normalize_source)
        .collect::<Result<_, _>>()?;

    let mut order = Vec::new();
    let mut source_map: HashMap<(String, Option<String>, Option<String>), MemorySource> =
        HashMap::new();

    for source in &norm_existing {
        let key = (
            source.source_type.clone(),
            source.locator.clone(),
            source.source_hash.clone(),
        );
        if let Some(entry) = source_map.get_mut(&key) {
            entry.primary = entry.primary || source.primary;
        } else {
            order.push(key.clone());
            source_map.insert(key, source.clone());
        }
    }

    let mut added = Vec::new();
    for source in norm_additions.iter().chain(norm_absorbed.iter()) {
        let key = (
            source.source_type.clone(),
            source.locator.clone(),
            source.source_hash.clone(),
        );
        if let Some(entry) = source_map.get_mut(&key) {
            entry.primary = entry.primary || source.primary;
        } else {
            order.push(key.clone());
            source_map.insert(key, source.clone());
            added.push(source.clone());
        }
    }

    let final_sources = order
        .into_iter()
        .filter_map(|k| source_map.remove(&k))
        .collect();
    Ok((final_sources, added))
}

fn record_to_summary(record: &MemoryRecord) -> MemorySummary {
    let scope = match &record.workspace {
        Some(path) => MemoryMaintenanceScope::Workspace { path: path.clone() },
        None => MemoryMaintenanceScope::Agent,
    };
    MemorySummary {
        memory_id: Some(record.memory_id.clone()),
        revision_id: Some(record.revision_id.clone()),
        text: record.text.clone(),
        kind: record.kind,
        scope,
        evidence_excerpt: record.evidence_excerpt.clone(),
        sources: record.sources.clone(),
    }
}

fn query_record_status(
    conn: &Connection,
    memory_id: &str,
) -> Result<Option<(String, String)>, MemoryError> {
    Ok(conn
        .query_row(
            "SELECT agent_id, status FROM memory_records WHERE memory_id=?1 ORDER BY revision DESC LIMIT 1",
            [memory_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn query_active_record(
    conn: &Connection,
    agent_id: &str,
    memory_id: &str,
) -> Result<Option<MemoryRecord>, MemoryError> {
    let record: Option<MemoryRecord> = conn
        .query_row(
            "SELECT * FROM memory_records WHERE memory_id=?1 AND agent_id=?2 AND status='active' ORDER BY revision DESC LIMIT 1",
            params![memory_id, agent_id],
            super::row_to_record,
        )
        .optional()?;

    if let Some(mut rec) = record {
        rec.sources = sources_for_revision(conn, &rec.revision_id)?;
        Ok(Some(rec))
    } else {
        Ok(None)
    }
}

fn evaluate_preview(
    conn: &Connection,
    plan: &MemoryMaintenancePlan,
) -> Result<MemoryMaintenancePreview, MemoryError> {
    let mut conflicts = Vec::new();
    let mut active_revisions: Vec<(String, String)> = Vec::new();
    let mut active_records: HashMap<String, MemoryRecord> = HashMap::new();

    // 1. Fetch and validate all referenced memories
    for (op_idx, operation) in plan.operations.iter().enumerate() {
        match operation {
            MemoryMaintenanceOperation::Revise {
                memory_id,
                expected_revision_id,
                ..
            }
            | MemoryMaintenanceOperation::Retire {
                memory_id,
                expected_revision_id,
                ..
            } => match query_active_record(conn, &plan.agent_id, memory_id)? {
                Some(rec) => {
                    if &rec.revision_id != expected_revision_id {
                        conflicts.push(MemoryMaintenanceConflict {
                            operation_index: op_idx,
                            code: "revision_changed".into(),
                            explanation: "Expected revision is no longer active".into(),
                        });
                    }
                    active_revisions.push((memory_id.clone(), rec.revision_id.clone()));
                    active_records.insert(memory_id.clone(), rec);
                }
                None => match query_record_status(conn, memory_id)? {
                    Some((owner, _)) if owner != plan.agent_id => {
                        conflicts.push(MemoryMaintenanceConflict {
                            operation_index: op_idx,
                            code: "owner_mismatch".into(),
                            explanation: format!("Memory {memory_id} belongs to another agent"),
                        });
                    }
                    Some((_, status)) if status != "active" => {
                        conflicts.push(MemoryMaintenanceConflict {
                            operation_index: op_idx,
                            code: "record_inactive".into(),
                            explanation: format!(
                                "Memory {memory_id} is no longer active (status: {status})"
                            ),
                        });
                    }
                    _ => {
                        conflicts.push(MemoryMaintenanceConflict {
                            operation_index: op_idx,
                            code: "record_not_found".into(),
                            explanation: format!(
                                "Memory record {memory_id} not found for agent {}",
                                plan.agent_id
                            ),
                        });
                    }
                },
            },
            MemoryMaintenanceOperation::RetireInto {
                memory_id,
                expected_revision_id,
                target_memory_id,
                target_expected_revision_id,
                ..
            } => {
                // Check source memory
                match query_active_record(conn, &plan.agent_id, memory_id)? {
                    Some(rec) => {
                        if &rec.revision_id != expected_revision_id {
                            conflicts.push(MemoryMaintenanceConflict {
                                operation_index: op_idx,
                                code: "revision_changed".into(),
                                explanation: "Expected revision is no longer active".into(),
                            });
                        }
                        active_revisions.push((memory_id.clone(), rec.revision_id.clone()));
                        active_records.insert(memory_id.clone(), rec);
                    }
                    None => match query_record_status(conn, memory_id)? {
                        Some((owner, _)) if owner != plan.agent_id => {
                            conflicts.push(MemoryMaintenanceConflict {
                                operation_index: op_idx,
                                code: "owner_mismatch".into(),
                                explanation: format!("Memory {memory_id} belongs to another agent"),
                            });
                        }
                        Some((_, status)) if status != "active" => {
                            conflicts.push(MemoryMaintenanceConflict {
                                operation_index: op_idx,
                                code: "record_inactive".into(),
                                explanation: format!(
                                    "Memory {memory_id} is no longer active (status: {status})"
                                ),
                            });
                        }
                        _ => {
                            conflicts.push(MemoryMaintenanceConflict {
                                operation_index: op_idx,
                                code: "record_not_found".into(),
                                explanation: format!(
                                    "Memory record {memory_id} not found for agent {}",
                                    plan.agent_id
                                ),
                            });
                        }
                    },
                }

                // If target is an existing memory, check it as well
                if let Some(target_id) = target_memory_id {
                    match query_active_record(conn, &plan.agent_id, target_id)? {
                        Some(target_rec) => {
                            if let Some(expected_target_rev) = target_expected_revision_id {
                                if &target_rec.revision_id != expected_target_rev {
                                    conflicts.push(MemoryMaintenanceConflict {
                                        operation_index: op_idx,
                                        code: "revision_changed".into(),
                                        explanation: "Expected target revision is no longer active"
                                            .into(),
                                    });
                                }
                            }
                            if !active_records.contains_key(target_id) {
                                active_revisions
                                    .push((target_id.clone(), target_rec.revision_id.clone()));
                                active_records.insert(target_id.clone(), target_rec);
                            }
                        }
                        None => match query_record_status(conn, target_id)? {
                            Some((owner, _)) if owner != plan.agent_id => {
                                conflicts.push(MemoryMaintenanceConflict {
                                    operation_index: op_idx,
                                    code: "owner_mismatch".into(),
                                    explanation: format!(
                                        "Target memory {target_id} belongs to another agent"
                                    ),
                                });
                            }
                            Some((_, status)) if status != "active" => {
                                conflicts.push(MemoryMaintenanceConflict {
                                    operation_index: op_idx,
                                    code: "target_inactive".into(),
                                    explanation: format!(
                                        "Target memory {target_id} is not active (status: {status})"
                                    ),
                                });
                            }
                            _ => {
                                conflicts.push(MemoryMaintenanceConflict {
                                    operation_index: op_idx,
                                    code: "target_not_found".into(),
                                    explanation: format!(
                                        "Target memory {target_id} not found for agent {}",
                                        plan.agent_id
                                    ),
                                });
                            }
                        },
                    }
                }
            }
            MemoryMaintenanceOperation::Create { .. } => {}
        }
    }

    // 2. Pre-collect absorption mapping
    let mut absorbed_map: HashMap<TargetKey, (Vec<String>, Vec<MemorySource>)> = HashMap::new();
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::RetireInto {
            memory_id,
            target_memory_id,
            target_client_key,
            ..
        } = operation
        {
            let target_key = if let Some(t_mem) = target_memory_id {
                TargetKey::Memory(t_mem.clone())
            } else if let Some(t_key) = target_client_key {
                TargetKey::ClientKey(t_key.clone())
            } else {
                continue;
            };

            let entry = absorbed_map
                .entry(target_key)
                .or_insert_with(|| (Vec::new(), Vec::new()));
            entry.0.push(memory_id.clone());
            if let Some(src_rec) = active_records.get(memory_id) {
                entry.1.extend(src_rec.sources.clone());
            }
        }
    }

    // 3. Build changes for each operation
    let mut changes = Vec::with_capacity(plan.operations.len());
    for (op_idx, operation) in plan.operations.iter().enumerate() {
        match operation {
            MemoryMaintenanceOperation::Revise {
                memory_id,
                text,
                kind,
                scope,
                evidence_excerpt,
                add_sources,
                ..
            } => {
                let before = active_records.get(memory_id).map(record_to_summary);
                let existing_sources = active_records
                    .get(memory_id)
                    .map(|r| r.sources.as_slice())
                    .unwrap_or(&[]);

                let target_key = TargetKey::Memory(memory_id.clone());
                let (absorbed_ids, absorbed_sources) =
                    absorbed_map.get(&target_key).cloned().unwrap_or_default();

                let (final_sources, source_additions) =
                    combine_sources(existing_sources, add_sources, &absorbed_sources)?;

                let after = MemorySummary {
                    memory_id: Some(memory_id.clone()),
                    revision_id: None,
                    text: text.clone(),
                    kind: *kind,
                    scope: scope.clone(),
                    evidence_excerpt: evidence_excerpt.clone(),
                    sources: final_sources,
                };

                changes.push(MemoryMaintenanceChange {
                    operation_index: op_idx,
                    op: "revise".into(),
                    before,
                    after: Some(after),
                    source_additions,
                    absorbed_memory_ids: absorbed_ids,
                    reason: None,
                });
            }
            MemoryMaintenanceOperation::Create {
                client_key,
                text,
                kind,
                scope,
                evidence_excerpt,
                sources,
            } => {
                let target_key = TargetKey::ClientKey(client_key.clone());
                let (absorbed_ids, absorbed_sources) =
                    absorbed_map.get(&target_key).cloned().unwrap_or_default();

                let (final_sources, _) = combine_sources(&[], sources, &absorbed_sources)?;

                let after = MemorySummary {
                    memory_id: None,
                    revision_id: None,
                    text: text.clone(),
                    kind: *kind,
                    scope: scope.clone(),
                    evidence_excerpt: evidence_excerpt.clone(),
                    sources: final_sources.clone(),
                };

                changes.push(MemoryMaintenanceChange {
                    operation_index: op_idx,
                    op: "create".into(),
                    before: None,
                    after: Some(after),
                    source_additions: final_sources,
                    absorbed_memory_ids: absorbed_ids,
                    reason: None,
                });
            }
            MemoryMaintenanceOperation::Retire {
                memory_id, reason, ..
            } => {
                let before = active_records.get(memory_id).map(record_to_summary);
                changes.push(MemoryMaintenanceChange {
                    operation_index: op_idx,
                    op: "retire".into(),
                    before,
                    after: None,
                    source_additions: Vec::new(),
                    absorbed_memory_ids: Vec::new(),
                    reason: Some(reason.clone()),
                });
            }
            MemoryMaintenanceOperation::RetireInto {
                memory_id, reason, ..
            } => {
                let before = active_records.get(memory_id).map(record_to_summary);
                changes.push(MemoryMaintenanceChange {
                    operation_index: op_idx,
                    op: "retire_into".into(),
                    before,
                    after: None,
                    source_additions: Vec::new(),
                    absorbed_memory_ids: Vec::new(),
                    reason: Some(reason.clone()),
                });
            }
        }
    }

    let preview_digest = compute_preview_digest(plan, &active_revisions)?;

    Ok(MemoryMaintenancePreview {
        plan_id: plan.plan_id.clone(),
        agent_id: plan.agent_id.clone(),
        operation_count: plan.operations.len(),
        preview_digest,
        changes,
        conflicts,
    })
}

pub fn preview_maintenance(
    store: &MemoryStore,
    actor: &MemoryActor,
    plan: &MemoryMaintenancePlan,
) -> Result<MemoryMaintenancePreview, MemoryError> {
    require_operator(actor, &plan.agent_id)?;
    validate_plan_structure(plan)?;

    let connection = store.connection()?;
    evaluate_preview(&connection, plan)
}

pub fn maintenance_replay(
    store: &MemoryStore,
    actor: &MemoryActor,
    plan: &MemoryMaintenancePlan,
) -> Result<Option<MemoryMaintenanceReceipt>, MemoryError> {
    require_operator(actor, &plan.agent_id)?;
    validate_plan_structure(plan)?;

    let connection = store.connection()?;
    let plan_hash = compute_plan_hash(plan)?;

    let existing: Option<(String, String, String)> = connection
        .query_row(
            "SELECT plan_id, plan_hash, receipt_json FROM memory_maintenance_receipts
             WHERE agent_id=?1 AND idempotency_key=?2",
            params![plan.agent_id, plan.idempotency_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    match existing {
        Some((stored_plan_id, stored_plan_hash, receipt_json)) => {
            if stored_plan_id != plan.plan_id || stored_plan_hash != plan_hash {
                return Err(MemoryError::Validation(format!(
                    "idempotency key '{}' was already used with a different maintenance plan",
                    plan.idempotency_key
                )));
            }
            let receipt: MemoryMaintenanceReceipt = serde_json::from_str(&receipt_json)?;
            Ok(Some(receipt))
        }
        None => Ok(None),
    }
}

pub fn apply_maintenance(
    store: &MemoryStore,
    actor: &MemoryActor,
    plan: &MemoryMaintenancePlan,
    preview_digest: &str,
) -> Result<MemoryMaintenanceReceipt, MemoryError> {
    require_operator(actor, &plan.agent_id)?;
    validate_plan_structure(plan)?;

    let mut connection = store.connection()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // 1. Check idempotency replay inside immediate transaction
    let plan_hash = compute_plan_hash(plan)?;
    let existing: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT plan_id, plan_hash, receipt_json FROM memory_maintenance_receipts
             WHERE agent_id=?1 AND idempotency_key=?2",
            params![plan.agent_id, plan.idempotency_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    if let Some((stored_plan_id, stored_plan_hash, receipt_json)) = existing {
        if stored_plan_id != plan.plan_id || stored_plan_hash != plan_hash {
            return Err(MemoryError::Validation(format!(
                "idempotency key '{}' was already used with a different maintenance plan",
                plan.idempotency_key
            )));
        }
        let receipt: MemoryMaintenanceReceipt = serde_json::from_str(&receipt_json)?;
        return Ok(receipt);
    }

    // 2. Evaluate preview and verify state inside transaction
    let preview = evaluate_preview(&transaction, plan)?;
    if !preview.conflicts.is_empty() {
        let first = &preview.conflicts[0];
        return Err(MemoryError::Conflict(format!(
            "operation {} conflict [{}]: {}",
            first.operation_index, first.code, first.explanation
        )));
    }

    if preview.preview_digest != preview_digest {
        return Err(MemoryError::Conflict(format!(
            "preview digest mismatch: expected {}, got {}",
            preview.preview_digest, preview_digest
        )));
    }

    let applied_at = Utc::now().to_rfc3339();

    // Map targets to absorbed memories and sources
    let mut absorbed_map: HashMap<TargetKey, (Vec<String>, Vec<MemorySource>)> = HashMap::new();
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::RetireInto {
            memory_id,
            target_memory_id,
            target_client_key,
            ..
        } = operation
        {
            let target_key = if let Some(t_mem) = target_memory_id {
                TargetKey::Memory(t_mem.clone())
            } else if let Some(t_key) = target_client_key {
                TargetKey::ClientKey(t_key.clone())
            } else {
                continue;
            };

            let entry = absorbed_map
                .entry(target_key)
                .or_insert_with(|| (Vec::new(), Vec::new()));
            entry.0.push(memory_id.clone());

            let sources = sources_for_revision(
                &transaction,
                &query_active_record(&transaction, &plan.agent_id, memory_id)?
                    .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?
                    .revision_id,
            )?;
            entry.1.extend(sources);
        }
    }

    // Track targets: client_key -> (memory_id, revision_id)
    let mut created_targets: HashMap<String, (String, String)> = HashMap::new();
    // Track memory_id -> new_revision_id for revised or target memories
    let mut revised_targets: HashMap<String, String> = HashMap::new();
    let mut revise_ops: HashSet<String> = HashSet::new();

    // 3. Process Create operations
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::Create {
            client_key,
            text,
            kind,
            scope,
            evidence_excerpt,
            sources,
        } = operation
        {
            let new_memory_id = Uuid::new_v4().to_string();
            let new_revision_id = Uuid::new_v4().to_string();

            let target_key = TargetKey::ClientKey(client_key.clone());
            let (absorbed_ids, absorbed_sources) =
                absorbed_map.get(&target_key).cloned().unwrap_or_default();

            let (final_sources, _) = combine_sources(&[], sources, &absorbed_sources)?;

            let workspace = match scope {
                MemoryMaintenanceScope::Agent => None,
                MemoryMaintenanceScope::Workspace { path } => normalize_workspace(Some(path)),
            };

            let normalized_text = normalize_text(text)?;
            let normalized_evidence = normalize_evidence(evidence_excerpt)?;
            let evidence_hash = hash_text(&normalized_evidence);

            transaction.execute(
                "INSERT INTO memory_records (
                    revision_id, memory_id, revision, agent_id, workspace, kind, text,
                    evidence_excerpt, evidence_hash, status, supersedes_revision_id,
                    replaced_by_revision_id, created_at, updated_at, last_verified_at,
                    idempotency_key
                ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, 'active', NULL, NULL, ?9, ?9, ?9, NULL)",
                params![
                    new_revision_id,
                    new_memory_id,
                    plan.agent_id,
                    workspace,
                    kind.as_str(),
                    normalized_text,
                    normalized_evidence,
                    evidence_hash,
                    applied_at,
                ],
            )?;

            super::insert_sources(&transaction, &new_revision_id, &final_sources)?;

            let payload = serde_json::json!({
                "actor": "desktop_operator",
                "confirmation_method": "native_dialog",
                "plan_id": plan.plan_id,
                "client_key": client_key,
                "absorbed_memory_ids": absorbed_ids,
            });
            super::insert_event(
                &transaction,
                &plan.agent_id,
                Some(&new_memory_id),
                Some(&new_revision_id),
                "create",
                Some(&payload),
            )?;

            created_targets.insert(client_key.clone(), (new_memory_id, new_revision_id));
        }
    }

    // 4. Process Revise operations
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::Revise {
            memory_id,
            text,
            kind,
            scope,
            evidence_excerpt,
            add_sources,
            ..
        } = operation
        {
            let current = query_active_record(&transaction, &plan.agent_id, memory_id)?
                .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;

            let new_revision_id = Uuid::new_v4().to_string();
            let new_revision_num = current.revision + 1;

            let target_key = TargetKey::Memory(memory_id.clone());
            let (absorbed_ids, absorbed_sources) =
                absorbed_map.get(&target_key).cloned().unwrap_or_default();

            let (final_sources, _) =
                combine_sources(&current.sources, add_sources, &absorbed_sources)?;

            let workspace = match scope {
                MemoryMaintenanceScope::Agent => None,
                MemoryMaintenanceScope::Workspace { path } => normalize_workspace(Some(path)),
            };

            let normalized_text = normalize_text(text)?;
            let normalized_evidence = normalize_evidence(evidence_excerpt)?;
            let evidence_hash = hash_text(&normalized_evidence);

            // Mark old revision superseded
            transaction.execute(
                "UPDATE memory_records SET status='superseded', replaced_by_revision_id=?1, updated_at=?2
                 WHERE revision_id=?3",
                params![new_revision_id, applied_at, current.revision_id],
            )?;

            // Insert new revision
            transaction.execute(
                "INSERT INTO memory_records (
                    revision_id, memory_id, revision, agent_id, workspace, kind, text,
                    evidence_excerpt, evidence_hash, status, supersedes_revision_id,
                    replaced_by_revision_id, created_at, updated_at, last_verified_at,
                    idempotency_key
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10, NULL, ?11, ?12, ?13, NULL)",
                params![
                    new_revision_id,
                    current.memory_id,
                    new_revision_num,
                    plan.agent_id,
                    workspace,
                    kind.as_str(),
                    normalized_text,
                    normalized_evidence,
                    evidence_hash,
                    current.revision_id,
                    current.created_at,
                    applied_at,
                    current.last_verified_at,
                ],
            )?;

            super::insert_sources(&transaction, &new_revision_id, &final_sources)?;

            let payload = serde_json::json!({
                "actor": "desktop_operator",
                "confirmation_method": "native_dialog",
                "plan_id": plan.plan_id,
                "supersedes_revision_id": current.revision_id,
                "absorbed_memory_ids": absorbed_ids,
            });
            super::insert_event(
                &transaction,
                &plan.agent_id,
                Some(&current.memory_id),
                Some(&new_revision_id),
                "revise",
                Some(&payload),
            )?;

            revised_targets.insert(memory_id.clone(), new_revision_id);
            revise_ops.insert(memory_id.clone());
        }
    }

    // 5. Update any RetireInto target memory that was NOT in Revise
    for (target_key, (absorbed_ids, absorbed_sources)) in &absorbed_map {
        if let TargetKey::Memory(target_mem_id) = target_key {
            if !revise_ops.contains(target_mem_id) {
                let current = query_active_record(&transaction, &plan.agent_id, target_mem_id)?
                    .ok_or_else(|| MemoryError::NotFound(target_mem_id.clone()))?;

                let new_revision_id = Uuid::new_v4().to_string();
                let new_revision_num = current.revision + 1;

                let (final_sources, _) = combine_sources(&current.sources, &[], absorbed_sources)?;

                transaction.execute(
                    "UPDATE memory_records SET status='superseded', replaced_by_revision_id=?1, updated_at=?2
                     WHERE revision_id=?3",
                    params![new_revision_id, applied_at, current.revision_id],
                )?;

                transaction.execute(
                    "INSERT INTO memory_records (
                        revision_id, memory_id, revision, agent_id, workspace, kind, text,
                        evidence_excerpt, evidence_hash, status, supersedes_revision_id,
                        replaced_by_revision_id, created_at, updated_at, last_verified_at,
                        idempotency_key
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10, NULL, ?11, ?12, ?13, NULL)",
                    params![
                        new_revision_id,
                        current.memory_id,
                        new_revision_num,
                        plan.agent_id,
                        current.workspace,
                        current.kind.as_str(),
                        current.text,
                        current.evidence_excerpt,
                        current.evidence_hash,
                        current.revision_id,
                        current.created_at,
                        applied_at,
                        current.last_verified_at,
                    ],
                )?;

                super::insert_sources(&transaction, &new_revision_id, &final_sources)?;

                let payload = serde_json::json!({
                    "actor": "desktop_operator",
                    "confirmation_method": "native_dialog",
                    "plan_id": plan.plan_id,
                    "supersedes_revision_id": current.revision_id,
                    "absorbed_memory_ids": absorbed_ids,
                });
                super::insert_event(
                    &transaction,
                    &plan.agent_id,
                    Some(&current.memory_id),
                    Some(&new_revision_id),
                    "revise",
                    Some(&payload),
                )?;

                revised_targets.insert(target_mem_id.clone(), new_revision_id);
            }
        }
    }

    // 6. Process Retire operations
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::Retire {
            memory_id, reason, ..
        } = operation
        {
            let current = query_active_record(&transaction, &plan.agent_id, memory_id)?
                .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;

            transaction.execute(
                "UPDATE memory_records SET status='removed', updated_at=?1 WHERE revision_id=?2",
                params![applied_at, current.revision_id],
            )?;

            let payload = serde_json::json!({
                "actor": "desktop_operator",
                "confirmation_method": "native_dialog",
                "plan_id": plan.plan_id,
                "reason": reason,
            });
            super::insert_event(
                &transaction,
                &plan.agent_id,
                Some(&current.memory_id),
                Some(&current.revision_id),
                "retire",
                Some(&payload),
            )?;
        }
    }

    // 7. Process RetireInto operations
    for operation in &plan.operations {
        if let MemoryMaintenanceOperation::RetireInto {
            memory_id,
            target_memory_id,
            target_client_key,
            reason,
            ..
        } = operation
        {
            let current = query_active_record(&transaction, &plan.agent_id, memory_id)?
                .ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;

            let (target_id, target_rev_id) = if let Some(t_key) = target_client_key {
                let (tid, trev) = created_targets.get(t_key).ok_or_else(|| {
                    MemoryError::Validation(format!("target_client_key '{t_key}' not resolved"))
                })?;
                (tid.clone(), trev.clone())
            } else if let Some(t_mem) = target_memory_id {
                let trev = revised_targets.get(t_mem).ok_or_else(|| {
                    MemoryError::Validation(format!("target_memory_id '{t_mem}' not resolved"))
                })?;
                (t_mem.clone(), trev.clone())
            } else {
                return Err(MemoryError::Validation("retire_into missing target".into()));
            };

            transaction.execute(
                "UPDATE memory_records SET status='removed', replaced_by_revision_id=?1, updated_at=?2
                 WHERE revision_id=?3",
                params![target_rev_id, applied_at, current.revision_id],
            )?;

            let payload = serde_json::json!({
                "actor": "desktop_operator",
                "confirmation_method": "native_dialog",
                "plan_id": plan.plan_id,
                "reason": reason,
                "target_memory_id": target_id,
                "target_revision_id": target_rev_id,
            });
            super::insert_event(
                &transaction,
                &plan.agent_id,
                Some(&current.memory_id),
                Some(&current.revision_id),
                "retire_into",
                Some(&payload),
            )?;
        }
    }

    // 8. Construct operation receipts in exact plan order
    let mut operation_receipts = Vec::with_capacity(plan.operations.len());
    for (op_idx, operation) in plan.operations.iter().enumerate() {
        match operation {
            MemoryMaintenanceOperation::Revise { memory_id, .. } => {
                let rev_id = revised_targets.get(memory_id).cloned();
                operation_receipts.push(MemoryMaintenanceOperationReceipt {
                    operation_index: op_idx,
                    client_key: None,
                    memory_id: memory_id.clone(),
                    revision_id: rev_id,
                });
            }
            MemoryMaintenanceOperation::Create { client_key, .. } => {
                let (mem_id, rev_id) = created_targets.get(client_key).cloned().unwrap_or_default();
                operation_receipts.push(MemoryMaintenanceOperationReceipt {
                    operation_index: op_idx,
                    client_key: Some(client_key.clone()),
                    memory_id: mem_id,
                    revision_id: Some(rev_id),
                });
            }
            MemoryMaintenanceOperation::Retire { memory_id, .. } => {
                operation_receipts.push(MemoryMaintenanceOperationReceipt {
                    operation_index: op_idx,
                    client_key: None,
                    memory_id: memory_id.clone(),
                    revision_id: None,
                });
            }
            MemoryMaintenanceOperation::RetireInto { memory_id, .. } => {
                operation_receipts.push(MemoryMaintenanceOperationReceipt {
                    operation_index: op_idx,
                    client_key: None,
                    memory_id: memory_id.clone(),
                    revision_id: None,
                });
            }
        }
    }

    let receipt = MemoryMaintenanceReceipt {
        plan_id: plan.plan_id.clone(),
        agent_id: plan.agent_id.clone(),
        idempotency_key: plan.idempotency_key.clone(),
        preview_digest: preview_digest.to_string(),
        applied_at: applied_at.clone(),
        operations: operation_receipts,
    };

    let receipt_json = serde_json::to_string(&receipt)?;

    transaction.execute(
        "INSERT INTO memory_maintenance_receipts (
            idempotency_key, agent_id, plan_id, plan_hash, preview_digest, receipt_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            plan.idempotency_key,
            plan.agent_id,
            plan.plan_id,
            plan_hash,
            preview_digest,
            receipt_json,
            applied_at,
        ],
    )?;

    transaction.commit()?;

    Ok(receipt)
}

pub fn maintenance_receipt(
    store: &MemoryStore,
    actor: &MemoryActor,
    agent_id: &str,
    idempotency_key: &str,
) -> Result<Option<MemoryMaintenanceReceipt>, MemoryError> {
    require_operator(actor, agent_id)?;
    let connection = store.connection()?;

    let receipt_json: Option<String> = connection
        .query_row(
            "SELECT receipt_json FROM memory_maintenance_receipts WHERE agent_id=?1 AND idempotency_key=?2",
            params![agent_id, idempotency_key],
            |row| row.get(0),
        )
        .optional()?;

    receipt_json
        .map(|json| serde_json::from_str(&json).map_err(MemoryError::from))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryStatus, SaveMemoryRequest, UpdateMemoryRequest};

    fn make_test_store() -> (tempfile::TempDir, MemoryStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(temp.path().join("memory.db")).unwrap();
        (temp, store)
    }

    fn seed_record(
        store: &MemoryStore,
        agent_id: &str,
        workspace: Option<&str>,
        kind: MemoryKind,
        text: &str,
        sources: Vec<MemorySource>,
    ) -> MemoryRecord {
        store
            .save(
                &MemoryActor::Operator,
                SaveMemoryRequest {
                    agent_id: agent_id.into(),
                    workspace: workspace.map(str::to_string),
                    kind,
                    text: text.into(),
                    evidence_excerpt: format!("Evidence for {text}"),
                    sources,
                    idempotency_key: None,
                },
            )
            .unwrap()
    }

    #[test]
    fn test_one_owner_isolation() {
        let (_temp, store) = make_test_store();
        let rec_a = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Rule A",
            vec![],
        );

        let plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-1".into(),
            agent_id: "agent-a".into(),
            idempotency_key: "idem-1".into(),
            operations: vec![MemoryMaintenanceOperation::Revise {
                memory_id: rec_a.memory_id.clone(),
                expected_revision_id: rec_a.revision_id.clone(),
                text: "Updated Rule A".into(),
                kind: MemoryKind::Stable,
                scope: MemoryMaintenanceScope::Agent,
                evidence_excerpt: "Clarified".into(),
                add_sources: vec![],
            }],
        };

        // Managed Agent actor rejected
        let res = store.preview_maintenance(&MemoryActor::Agent("agent-a".into()), &plan);
        assert!(matches!(res, Err(MemoryError::AccessDenied { .. })));

        let res_apply =
            store.apply_maintenance(&MemoryActor::Agent("agent-a".into()), &plan, "sha256:dummy");
        assert!(matches!(res_apply, Err(MemoryError::AccessDenied { .. })));

        // Foreign agent plan trying to touch agent-a's record
        let foreign_plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-2".into(),
            agent_id: "agent-b".into(),
            idempotency_key: "idem-2".into(),
            operations: vec![MemoryMaintenanceOperation::Revise {
                memory_id: rec_a.memory_id.clone(),
                expected_revision_id: rec_a.revision_id.clone(),
                text: "Attacking Rule A".into(),
                kind: MemoryKind::Stable,
                scope: MemoryMaintenanceScope::Agent,
                evidence_excerpt: "Clarified".into(),
                add_sources: vec![],
            }],
        };

        let preview = store
            .preview_maintenance(&MemoryActor::Operator, &foreign_plan)
            .unwrap();
        assert_eq!(preview.conflicts.len(), 1);
        assert_eq!(preview.conflicts[0].code, "owner_mismatch");
    }

    #[test]
    fn test_exact_revision_conflicts() {
        let (_temp, store) = make_test_store();
        let rec = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Rule A",
            vec![],
        );

        // Update the record to revision 2
        let rec2 = store
            .update(
                &MemoryActor::Operator,
                UpdateMemoryRequest {
                    memory_id: rec.memory_id.clone(),
                    text: "Rule A v2".into(),
                    evidence_excerpt: "Updated evidence".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        assert_eq!(rec2.revision, 2);

        // Plan with outdated revision 1
        let plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-1".into(),
            agent_id: "agent-a".into(),
            idempotency_key: "idem-1".into(),
            operations: vec![MemoryMaintenanceOperation::Revise {
                memory_id: rec.memory_id.clone(),
                expected_revision_id: rec.revision_id.clone(), // old revision!
                text: "Stale update".into(),
                kind: MemoryKind::Stable,
                scope: MemoryMaintenanceScope::Agent,
                evidence_excerpt: "Stale evidence".into(),
                add_sources: vec![],
            }],
        };

        let preview = store
            .preview_maintenance(&MemoryActor::Operator, &plan)
            .unwrap();
        assert_eq!(preview.conflicts.len(), 1);
        assert_eq!(preview.conflicts[0].code, "revision_changed");

        // Apply must fail with Conflict
        let apply_res =
            store.apply_maintenance(&MemoryActor::Operator, &plan, &preview.preview_digest);
        assert!(matches!(apply_res, Err(MemoryError::Conflict(_))));
    }

    #[test]
    fn test_all_or_nothing_atomic_failure() {
        let (_temp, store) = make_test_store();
        let rec1 = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Rule 1",
            vec![],
        );
        let rec2 = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Rule 2",
            vec![],
        );

        let fake_uuid = Uuid::new_v4().to_string();
        let plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-atomic".into(),
            agent_id: "agent-a".into(),
            idempotency_key: "idem-atomic".into(),
            operations: vec![
                MemoryMaintenanceOperation::Revise {
                    memory_id: rec1.memory_id.clone(),
                    expected_revision_id: rec1.revision_id.clone(),
                    text: "Rule 1 revised".into(),
                    kind: MemoryKind::Stable,
                    scope: MemoryMaintenanceScope::Agent,
                    evidence_excerpt: "Evidence 1".into(),
                    add_sources: vec![],
                },
                MemoryMaintenanceOperation::Retire {
                    memory_id: rec2.memory_id.clone(),
                    expected_revision_id: fake_uuid, // Bad revision!
                    reason: "Obsolete".into(),
                },
            ],
        };

        let preview = store
            .preview_maintenance(&MemoryActor::Operator, &plan)
            .unwrap();
        assert!(!preview.conflicts.is_empty());

        let res = store.apply_maintenance(&MemoryActor::Operator, &plan, &preview.preview_digest);
        assert!(matches!(res, Err(MemoryError::Conflict(_))));

        // Verify rec1 is completely unchanged
        let current_rec1 = store.get(&MemoryActor::Operator, &rec1.memory_id).unwrap();
        assert_eq!(current_rec1.revision, 1);
        assert_eq!(current_rec1.text, "Rule 1");
    }

    #[test]
    fn test_idempotent_replay() {
        let (temp, store) = make_test_store();
        let work_path = temp.path().join("work").display().to_string();
        let rec = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Rule Initial",
            vec![],
        );

        let plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-idemp".into(),
            agent_id: "agent-a".into(),
            idempotency_key: "key-unique-1".into(),
            operations: vec![MemoryMaintenanceOperation::Revise {
                memory_id: rec.memory_id.clone(),
                expected_revision_id: rec.revision_id.clone(),
                text: "Rule Revised".into(),
                kind: MemoryKind::Current,
                scope: MemoryMaintenanceScope::Workspace { path: work_path },
                evidence_excerpt: "Revised evidence".into(),
                add_sources: vec![],
            }],
        };

        // First replay before apply returns None
        let replay_none = store
            .maintenance_replay(&MemoryActor::Operator, &plan)
            .unwrap();
        assert!(replay_none.is_none());

        let preview = store
            .preview_maintenance(&MemoryActor::Operator, &plan)
            .unwrap();
        assert!(preview.conflicts.is_empty());

        let receipt1 = store
            .apply_maintenance(&MemoryActor::Operator, &plan, &preview.preview_digest)
            .unwrap();
        assert_eq!(receipt1.operations.len(), 1);
        assert!(receipt1.operations[0].revision_id.is_some());

        // Second apply returns identical receipt
        let receipt2 = store
            .apply_maintenance(&MemoryActor::Operator, &plan, &preview.preview_digest)
            .unwrap();
        assert_eq!(receipt1, receipt2);

        // Replay returns the receipt
        let replay_some = store
            .maintenance_replay(&MemoryActor::Operator, &plan)
            .unwrap();
        assert_eq!(replay_some.as_ref(), Some(&receipt1));

        // Receipt lookup returns the receipt
        let lookup = store
            .maintenance_receipt(&MemoryActor::Operator, "agent-a", &plan.idempotency_key)
            .unwrap();
        assert_eq!(lookup.as_ref(), Some(&receipt1));

        // Reusing same idempotency key with different plan fails with Validation error
        let mut different_plan = plan.clone();
        different_plan.plan_id = "plan-different".into();
        let mismatch_res = store.apply_maintenance(
            &MemoryActor::Operator,
            &different_plan,
            &preview.preview_digest,
        );
        assert!(matches!(mismatch_res, Err(MemoryError::Validation(_))));
    }

    #[test]
    fn test_kind_and_scope_revision_and_sources() {
        let (_temp, store) = make_test_store();
        let src1 = MemorySource {
            source_type: "conversation".into(),
            locator: Some("turn-1".into()),
            source_hash: None,
            primary: true,
        };
        let src2 = MemorySource {
            source_type: "conversation".into(),
            locator: Some("turn-2".into()),
            source_hash: None,
            primary: false,
        };
        let rec1 = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Old Text 1",
            vec![src1.clone()],
        );
        let rec2 = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Current,
            "Old Text 2 to Retire",
            vec![src2.clone()],
        );

        let src3 = MemorySource {
            source_type: "artifact".into(),
            locator: Some("notes.md".into()),
            source_hash: None,
            primary: false,
        };

        let plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-merge".into(),
            agent_id: "agent-a".into(),
            idempotency_key: "idem-merge".into(),
            operations: vec![
                MemoryMaintenanceOperation::Revise {
                    memory_id: rec1.memory_id.clone(),
                    expected_revision_id: rec1.revision_id.clone(),
                    text: "Consolidated Rule".into(),
                    kind: MemoryKind::Current,
                    scope: MemoryMaintenanceScope::Workspace {
                        path: "C:/Project".into(),
                    },
                    evidence_excerpt: "Merged evidence".into(),
                    add_sources: vec![src3.clone()],
                },
                MemoryMaintenanceOperation::RetireInto {
                    memory_id: rec2.memory_id.clone(),
                    expected_revision_id: rec2.revision_id.clone(),
                    target_memory_id: Some(rec1.memory_id.clone()),
                    target_client_key: None,
                    target_expected_revision_id: None,
                    reason: "Merged into Rule 1".into(),
                },
            ],
        };

        let preview = store
            .preview_maintenance(&MemoryActor::Operator, &plan)
            .unwrap();
        assert!(preview.conflicts.is_empty());
        assert_eq!(preview.changes.len(), 2);
        assert_eq!(
            preview.changes[0].absorbed_memory_ids,
            vec![rec2.memory_id.clone()]
        );
        assert_eq!(preview.changes[0].after.as_ref().unwrap().sources.len(), 3);

        let receipt = store
            .apply_maintenance(&MemoryActor::Operator, &plan, &preview.preview_digest)
            .unwrap();
        assert_eq!(receipt.operations.len(), 2);

        // Check updated rec1
        let updated_rec1 = store.get(&MemoryActor::Operator, &rec1.memory_id).unwrap();
        assert_eq!(updated_rec1.revision, 2);
        assert_eq!(updated_rec1.kind, MemoryKind::Current);
        assert_eq!(updated_rec1.last_verified_at, rec1.last_verified_at);
        assert_eq!(updated_rec1.sources.len(), 3);

        // Check retired rec2
        let history_rec2 = store
            .history(&MemoryActor::Operator, &rec2.memory_id)
            .unwrap();
        assert_eq!(history_rec2[0].status, MemoryStatus::Removed);
        assert_eq!(
            history_rec2[0].replaced_by_revision_id.as_deref(),
            Some(updated_rec1.revision_id.as_str())
        );
    }

    #[test]
    fn test_create_and_retire_into_created() {
        let (_temp, store) = make_test_store();
        let src = MemorySource {
            source_type: "conversation".into(),
            locator: Some("turn-9".into()),
            source_hash: None,
            primary: true,
        };
        let old_rec = seed_record(
            &store,
            "agent-a",
            None,
            MemoryKind::Stable,
            "Mixed Memory",
            vec![src.clone()],
        );

        let plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan-split".into(),
            agent_id: "agent-a".into(),
            idempotency_key: "idem-split".into(),
            operations: vec![
                MemoryMaintenanceOperation::Create {
                    client_key: "clean-rule".into(),
                    text: "Clean rule text".into(),
                    kind: MemoryKind::Stable,
                    scope: MemoryMaintenanceScope::Agent,
                    evidence_excerpt: "New evidence".into(),
                    sources: vec![],
                },
                MemoryMaintenanceOperation::RetireInto {
                    memory_id: old_rec.memory_id.clone(),
                    expected_revision_id: old_rec.revision_id.clone(),
                    target_memory_id: None,
                    target_client_key: Some("clean-rule".into()),
                    target_expected_revision_id: None,
                    reason: "Splitting into clean rule".into(),
                },
            ],
        };

        let preview = store
            .preview_maintenance(&MemoryActor::Operator, &plan)
            .unwrap();
        assert!(preview.conflicts.is_empty());
        assert_eq!(
            preview.changes[0].absorbed_memory_ids,
            vec![old_rec.memory_id.clone()]
        );
        assert_eq!(preview.changes[0].after.as_ref().unwrap().sources.len(), 1);

        let receipt = store
            .apply_maintenance(&MemoryActor::Operator, &plan, &preview.preview_digest)
            .unwrap();

        let created_op = &receipt.operations[0];
        assert_eq!(created_op.client_key.as_deref(), Some("clean-rule"));
        let created_record = store
            .get(&MemoryActor::Operator, &created_op.memory_id)
            .unwrap();
        assert_eq!(created_record.text, "Clean rule text");
        assert_eq!(created_record.sources.len(), 1);
        assert_eq!(created_record.sources[0].locator, Some("turn-9".into()));
    }

    #[test]
    fn test_malformed_and_bounds_rejection() {
        let (_temp, store) = make_test_store();

        // 0 operations
        let empty_plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p".into(),
            agent_id: "a".into(),
            idempotency_key: "k".into(),
            operations: vec![],
        };
        assert!(matches!(
            store.preview_maintenance(&MemoryActor::Operator, &empty_plan),
            Err(MemoryError::Validation(_))
        ));

        // Text exceeds 8192 characters
        let long_text_plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p".into(),
            agent_id: "a".into(),
            idempotency_key: "k".into(),
            operations: vec![MemoryMaintenanceOperation::Create {
                client_key: "c".into(),
                text: "a".repeat(8193),
                kind: MemoryKind::Stable,
                scope: MemoryMaintenanceScope::Agent,
                evidence_excerpt: "e".into(),
                sources: vec![],
            }],
        };
        assert!(matches!(
            store.preview_maintenance(&MemoryActor::Operator, &long_text_plan),
            Err(MemoryError::Validation(_))
        ));

        // Invalid non-UUID memory_id
        let bad_uuid_plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p".into(),
            agent_id: "a".into(),
            idempotency_key: "k".into(),
            operations: vec![MemoryMaintenanceOperation::Revise {
                memory_id: "not-a-uuid".into(),
                expected_revision_id: Uuid::new_v4().to_string(),
                text: "text".into(),
                kind: MemoryKind::Stable,
                scope: MemoryMaintenanceScope::Agent,
                evidence_excerpt: "e".into(),
                add_sources: vec![],
            }],
        };
        assert!(matches!(
            store.preview_maintenance(&MemoryActor::Operator, &bad_uuid_plan),
            Err(MemoryError::Validation(_))
        ));

        // Self-targeting retire_into
        let uuid = Uuid::new_v4().to_string();
        let self_target_plan = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p".into(),
            agent_id: "a".into(),
            idempotency_key: "k".into(),
            operations: vec![MemoryMaintenanceOperation::RetireInto {
                memory_id: uuid.clone(),
                expected_revision_id: Uuid::new_v4().to_string(),
                target_memory_id: Some(uuid),
                target_client_key: None,
                target_expected_revision_id: None,
                reason: "Self".into(),
            }],
        };
        assert!(matches!(
            store.preview_maintenance(&MemoryActor::Operator, &self_target_plan),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn test_schema_v5_receipts_table_and_version() {
        let (_temp, store) = make_test_store();
        let conn = store.connection().unwrap();

        let version: i64 = conn
            .query_row("SELECT version FROM memory_schema", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 5);

        // Receipts table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_maintenance_receipts'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some();
        assert!(table_exists);
    }

    #[test]
    fn test_rejects_unknown_fields_in_embedded_memory_source() {
        let json_with_unknown_source_field = serde_json::json!({
            "schema_version": 1,
            "plan_id": "plan-1",
            "agent_id": "agent-a",
            "idempotency_key": "idem-1",
            "operations": [
                {
                    "op": "create",
                    "client_key": "c1",
                    "text": "text",
                    "kind": "stable",
                    "scope": { "kind": "agent" },
                    "evidence_excerpt": "ev",
                    "sources": [
                        {
                            "source_type": "artifact",
                            "unknown_source_key": "disallowed"
                        }
                    ]
                }
            ]
        });

        let res: Result<MemoryMaintenancePlan, _> =
            serde_json::from_value(json_with_unknown_source_field);
        assert!(
            res.is_err(),
            "deserialization must fail when embedded MemorySource contains unknown fields"
        );
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown field `unknown_source_key`"),
            "error message should cite unknown field: {err_msg}"
        );

        let json_with_unknown_revise_source = serde_json::json!({
            "op": "revise",
            "memory_id": Uuid::new_v4().to_string(),
            "expected_revision_id": Uuid::new_v4().to_string(),
            "text": "text",
            "kind": "stable",
            "scope": { "kind": "agent" },
            "evidence_excerpt": "ev",
            "add_sources": [
                {
                    "source_type": "artifact",
                    "unexpected_extra_key": 42
                }
            ]
        });

        let res_op: Result<MemoryMaintenanceOperation, _> =
            serde_json::from_value(json_with_unknown_revise_source);
        assert!(
            res_op.is_err(),
            "MemoryMaintenanceOperation::Revise must reject unknown source fields"
        );
        let err_op_msg = res_op.unwrap_err().to_string();
        assert!(
            err_op_msg.contains("unexpected_extra_key"),
            "error message should cite unknown field: {err_op_msg}"
        );
    }

    #[test]
    fn test_scope_rejects_unknown_and_misplaced_keys() {
        // Valid Agent scope
        let valid_agent = serde_json::json!({ "kind": "agent" });
        let scope: MemoryMaintenanceScope = serde_json::from_value(valid_agent).unwrap();
        assert_eq!(scope, MemoryMaintenanceScope::Agent);

        // Valid Workspace scope
        let valid_ws = serde_json::json!({ "kind": "workspace", "path": "/valid/path" });
        let scope_ws: MemoryMaintenanceScope = serde_json::from_value(valid_ws).unwrap();
        assert_eq!(
            scope_ws,
            MemoryMaintenanceScope::Workspace {
                path: "/valid/path".into()
            }
        );

        // Agent scope with misplaced path: { "kind": "agent", "path": "/wrong" }
        let agent_with_path = serde_json::json!({ "kind": "agent", "path": "/wrong" });
        let res_agent_path: Result<MemoryMaintenanceScope, _> =
            serde_json::from_value(agent_with_path);
        assert!(
            res_agent_path.is_err(),
            "agent scope with path must be rejected"
        );
        let err_agent_path = res_agent_path.unwrap_err().to_string();
        assert!(
            err_agent_path.contains("unknown field `path`"),
            "error should indicate unknown field path: {err_agent_path}"
        );

        // Agent scope with extra key: { "kind": "agent", "extra": 123 }
        let agent_with_extra = serde_json::json!({ "kind": "agent", "extra": 123 });
        let res_agent_extra: Result<MemoryMaintenanceScope, _> =
            serde_json::from_value(agent_with_extra);
        assert!(
            res_agent_extra.is_err(),
            "agent scope with extra key must be rejected"
        );

        // Workspace scope with extra key: { "kind": "workspace", "path": "/valid", "extra": 123 }
        let ws_with_extra =
            serde_json::json!({ "kind": "workspace", "path": "/valid", "extra": 123 });
        let res_ws_extra: Result<MemoryMaintenanceScope, _> = serde_json::from_value(ws_with_extra);
        assert!(
            res_ws_extra.is_err(),
            "workspace scope with extra key must be rejected"
        );
        let err_ws_extra = res_ws_extra.unwrap_err().to_string();
        assert!(
            err_ws_extra.contains("unknown field `extra`"),
            "error should indicate unknown field extra: {err_ws_extra}"
        );

        // Workspace scope missing path: { "kind": "workspace" }
        let ws_no_path = serde_json::json!({ "kind": "workspace" });
        let res_ws_no_path: Result<MemoryMaintenanceScope, _> = serde_json::from_value(ws_no_path);
        assert!(
            res_ws_no_path.is_err(),
            "workspace scope without path must be rejected"
        );

        // Embedded inside an operation
        let op_agent_with_path = serde_json::json!({
            "op": "create",
            "client_key": "c1",
            "text": "text",
            "kind": "stable",
            "scope": { "kind": "agent", "path": "/wrong" },
            "evidence_excerpt": "ev",
            "sources": []
        });
        let res_op: Result<MemoryMaintenanceOperation, _> =
            serde_json::from_value(op_agent_with_path);
        assert!(
            res_op.is_err(),
            "operation with agent scope containing path must be rejected"
        );

        let op_ws_with_extra = serde_json::json!({
            "op": "create",
            "client_key": "c1",
            "text": "text",
            "kind": "stable",
            "scope": { "kind": "workspace", "path": "/valid", "extra": "forbidden" },
            "evidence_excerpt": "ev",
            "sources": []
        });
        let res_op_ws: Result<MemoryMaintenanceOperation, _> =
            serde_json::from_value(op_ws_with_extra);
        assert!(
            res_op_ws.is_err(),
            "operation with workspace scope containing extra key must be rejected"
        );
    }

    #[test]
    fn test_preview_digest_resists_adversarial_newline_collisions() {
        let op = MemoryMaintenanceOperation::Create {
            client_key: "k1".into(),
            text: "t1".into(),
            kind: MemoryKind::Stable,
            scope: MemoryMaintenanceScope::Agent,
            evidence_excerpt: "e1".into(),
            sources: vec![],
        };

        // Adversarial tuples where boundary sliding across newlines could produce collisions
        // under naive newline concatenation: ("p\na", "b", "k") vs ("p", "a\nb", "k")
        let plan_1 = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan\nowner".into(),
            agent_id: "agent".into(),
            idempotency_key: "key".into(),
            operations: vec![op.clone()],
        };
        let plan_2 = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan".into(),
            agent_id: "\nowner\nagent".into(),
            idempotency_key: "key".into(),
            operations: vec![op.clone()],
        };
        let plan_3 = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan".into(),
            agent_id: "owner".into(),
            idempotency_key: "\nagent\nkey".into(),
            operations: vec![op.clone()],
        };
        let plan_4 = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "plan".into(),
            agent_id: "owner".into(),
            idempotency_key: "agent\nkey".into(),
            operations: vec![op.clone()],
        };

        let active_revs = vec![("m1".into(), "r1".into())];

        let digest_1 = compute_preview_digest(&plan_1, &active_revs).unwrap();
        let digest_2 = compute_preview_digest(&plan_2, &active_revs).unwrap();
        let digest_3 = compute_preview_digest(&plan_3, &active_revs).unwrap();
        let digest_4 = compute_preview_digest(&plan_4, &active_revs).unwrap();

        let mut digests = HashSet::new();
        digests.insert(digest_1);
        digests.insert(digest_2);
        digests.insert(digest_3);
        digests.insert(digest_4);
        assert_eq!(
            digests.len(),
            4,
            "all adversarial newline plans must produce distinct digests"
        );
    }

    #[test]
    fn test_workspace_scope_rejects_relative_path() {
        let op_relative = MemoryMaintenanceOperation::Create {
            client_key: "c1".into(),
            text: "text".into(),
            kind: MemoryKind::Stable,
            scope: MemoryMaintenanceScope::Workspace {
                path: "relative/path/to/project".into(),
            },
            evidence_excerpt: "evidence".into(),
            sources: vec![],
        };
        let plan_relative = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p1".into(),
            agent_id: "a1".into(),
            idempotency_key: "idem".into(),
            operations: vec![op_relative],
        };

        let err = validate_plan_structure(&plan_relative).unwrap_err();
        assert!(
            matches!(err, MemoryError::Validation(ref msg) if msg.contains("must be an absolute path")),
            "relative path must be rejected with absolute path validation error: {err:?}"
        );

        let op_dot = MemoryMaintenanceOperation::Create {
            client_key: "c2".into(),
            text: "text".into(),
            kind: MemoryKind::Stable,
            scope: MemoryMaintenanceScope::Workspace {
                path: "./project".into(),
            },
            evidence_excerpt: "evidence".into(),
            sources: vec![],
        };
        let plan_dot = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p2".into(),
            agent_id: "a1".into(),
            idempotency_key: "idem".into(),
            operations: vec![op_dot],
        };
        assert!(validate_plan_structure(&plan_dot).is_err());

        // Platform-appropriate absolute path should succeed validation
        let abs_path = if cfg!(windows) {
            "C:\\project\\dir"
        } else {
            "/project/dir"
        };
        let op_abs = MemoryMaintenanceOperation::Create {
            client_key: "c3".into(),
            text: "text".into(),
            kind: MemoryKind::Stable,
            scope: MemoryMaintenanceScope::Workspace {
                path: abs_path.into(),
            },
            evidence_excerpt: "evidence".into(),
            sources: vec![],
        };
        let plan_abs = MemoryMaintenancePlan {
            schema_version: 1,
            plan_id: "p3".into(),
            agent_id: "a1".into(),
            idempotency_key: "idem".into(),
            operations: vec![op_abs],
        };
        assert!(validate_plan_structure(&plan_abs).is_ok());
    }

    #[test]
    fn test_agent_scope_rejects_null_path() {
        let json_null_path = serde_json::json!({
            "kind": "agent",
            "path": null
        });
        let res: Result<MemoryMaintenanceScope, _> = serde_json::from_value(json_null_path);
        assert!(res.is_err(), "agent scope with path: null must be rejected");
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown field `path`"),
            "error should cite unknown field path: {err_msg}"
        );

        let op_null_path = serde_json::json!({
            "op": "create",
            "client_key": "c1",
            "text": "text",
            "kind": "stable",
            "scope": {
                "kind": "agent",
                "path": null
            },
            "evidence_excerpt": "ev",
            "sources": []
        });
        let res_op: Result<MemoryMaintenanceOperation, _> = serde_json::from_value(op_null_path);
        assert!(
            res_op.is_err(),
            "operation with agent scope containing path: null must be rejected"
        );
    }

    #[test]
    fn test_source_dedup_normalizes_before_merge() {
        let existing = vec![MemorySource {
            source_type: "conversation".into(),
            locator: Some("conv-123".into()),
            source_hash: Some("sha256:abc".into()),
            primary: true,
        }];

        // Un-trimmed duplicate in additions
        let additions = vec![MemorySource {
            source_type: "  conversation  ".into(),
            locator: Some("  conv-123  ".into()),
            source_hash: Some("  sha256:abc  ".into()),
            primary: false,
        }];

        // Un-trimmed duplicate in absorbed
        let absorbed = vec![MemorySource {
            source_type: "conversation".into(),
            locator: Some("conv-123\n".into()),
            source_hash: Some("\tsha256:abc ".into()),
            primary: false,
        }];

        let (final_sources, added) = combine_sources(&existing, &additions, &absorbed).unwrap();
        assert_eq!(
            final_sources.len(),
            1,
            "dedup after normalization should coalesce into single source"
        );
        assert_eq!(final_sources[0].source_type, "conversation");
        assert_eq!(final_sources[0].locator.as_deref(), Some("conv-123"));
        assert_eq!(final_sources[0].source_hash.as_deref(), Some("sha256:abc"));
        assert!(final_sources[0].primary);
        assert!(
            added.is_empty(),
            "duplicate sources with varying whitespace must not be treated as additions"
        );
    }
}
