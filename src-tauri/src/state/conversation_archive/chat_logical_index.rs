//! Derived display relations. Physical rows, source coordinates and capture
//! cursors are never changed by this index. Every update touches a bounded
//! component; readers use certificates materialized in published row objects.
use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::json;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

use super::chat_read_store::{digest, valid_ref, Store};
use super::ConversationArchiveContext;
use crate::providers::chat_transcript::{codex_display_evidence, CodexDisplaySide};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Index {
    scope: String,
    buckets: Option<String>,
    members: Option<String>,
    pending: Option<String>,
    claims: Option<String>,
    before: Option<String>,
    pub(super) legacy_ready: bool,
    pub(super) complete_prefix: bool,
    admission: String,
    #[serde(default)]
    narrative_deferred: bool,
    #[serde(default)]
    narrative_watermark: Option<u64>,
    #[serde(default)]
    narrative_queue: Option<String>,
    #[serde(default)]
    narrative_before: Option<String>,
    #[serde(default)]
    sequence_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Relation {
    pub(super) id: String,
    pub(super) members: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Member {
    token: String,
    id: String,
    row: String,
    buckets: Vec<String>,
    legacy: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Bucket {
    scope: String,
    kind: String,
    left: Vec<String>,
    right: Vec<String>,
    conflict: bool,
    watermark: u64,
}

pub(super) struct Admission<'a> {
    pub(super) context: &'a ConversationArchiveContext,
    pub(super) conversation: Option<&'a str>,
    pub(super) epoch: &'a str,
    pub(super) admission: &'a str,
    pub(super) path: &'a str,
    pub(super) watermark: u64,
    pub(super) sequence_trusted: bool,
}

impl Admission<'_> {
    fn scope(&self) -> String {
        digest(
            &serde_json::to_vec(&json!([
                self.context.agent_id,
                self.context.provider,
                self.context.provider_source_key,
                self.context.provider_session_ids,
                self.epoch,
                self.admission,
                self.path,
            ]))
            .unwrap(),
        )
    }

    pub(super) fn native(&self, event: &AgentChatEvent) -> bool {
        let Some((start, end)) = event.metadata["chat_source_start"]
            .as_u64()
            .zip(event.metadata["chat_source_end"].as_u64())
        else {
            return false;
        };
        let prefix = format!("source:{}:{}:{start}:", self.context.agent_id, self.epoch);
        let Some((raw, ordinal)) = event
            .id
            .strip_prefix(&prefix)
            .and_then(|s| s.split_once(':'))
        else {
            return false;
        };
        start < end
            && end <= self.watermark
            && valid_ref(raw)
            && ordinal == "0"
            && event.metadata["chat_source_ref"].as_str() == Some(event.id.as_str())
            && event.metadata["chat_source_epoch"].as_str() == Some(self.epoch)
    }

    fn owned(&self, event: &AgentChatEvent) -> bool {
        event.session_id == self.context.agent_id
            && event.provider == self.context.provider
            && event.kind == AgentChatEventKind::Message
            && event.role.is_some()
            && event.metadata["provider_log"] == true
            && event.metadata["generated"] != true
            && event.metadata["log_path"]
                .as_str()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .is_some_and(|p| p.to_string_lossy() == self.path)
            && event.metadata["provider_session_id"]
                .as_str()
                .is_none_or(|id| {
                    !id.is_empty()
                        && self
                            .context
                            .provider_session_ids
                            .iter()
                            .any(|known| known == id)
                })
            && event.metadata["conversation_archive_id"]
                .as_str()
                .is_none_or(|id| self.conversation == Some(id))
            && event.metadata["chat_source_epoch"]
                .as_str()
                .is_none_or(|epoch| epoch == self.epoch)
    }
}

impl Index {
    fn bucket(&self, store: &mut Store, key: &str) -> io::Result<Option<Bucket>> {
        store
            .get(&self.buckets, key)?
            .map(|r| store.read(&r))
            .transpose()
    }

    fn member(&self, store: &mut Store, token: &str) -> io::Result<Member> {
        let r = store
            .get(&self.members, token)?
            .ok_or_else(|| io::Error::other("missing logical member"))?;
        store.read(&r)
    }

    fn qualified(&self, bucket: &Bucket) -> bool {
        bucket.scope == self.scope
            && !bucket.conflict
            && bucket.left.len() == 1
            && bucket.right.len() == 1
            && (bucket.kind != "narrative"
                || !self.narrative_deferred
                || self
                    .narrative_watermark
                    .is_some_and(|watermark| bucket.watermark <= watermark))
            && (bucket.kind == "narrative" || self.legacy_ready)
            && (bucket.kind != "sequence" || self.sequence_ready)
            && (bucket.kind != "prefix" || self.complete_prefix)
    }

    /// Register one owned physical occurrence. Saturating counts retain enough
    /// evidence to refuse ambiguity without an unbounded list in any object.
    pub(super) fn observe(
        &mut self,
        store: &mut Store,
        admission: &Admission<'_>,
        event: &AgentChatEvent,
        row: &str,
    ) -> io::Result<Vec<(String, Option<Relation>)>> {
        let scope = admission.scope();
        if self.scope != scope {
            *self = Self {
                scope,
                admission: admission.admission.into(),
                narrative_deferred: self.narrative_deferred,
                sequence_ready: admission.sequence_trusted,
                ..Self::default()
            };
        }
        if !admission.owned(event) {
            return Ok(Vec::new());
        }
        let native = admission.native(event);
        let legacy = !event.metadata["chat_source_ref"].is_string()
            && event.metadata["chat_legacy_identity_ineligible"] != true
            && matches!(event.provider.as_str(), "codex" | "pi");
        if !native && !legacy {
            return Ok(Vec::new());
        }
        let token = digest(
            &serde_json::to_vec(&json!([
                self.scope,
                event.id,
                if native { "" } else { row },
            ]))
            .unwrap(),
        );
        let mut keys = Vec::new();
        if native {
            if let Some((evidence, side)) = codex_display_evidence(event) {
                let side = match side {
                    CodexDisplaySide::UserRequest | CodexDisplaySide::AssistantLive => false,
                    CodexDisplaySide::UserMirror | CodexDisplaySide::AssistantFinal => true,
                };
                keys.push((
                    digest(format!("{}:narrative:{evidence}", self.scope).as_bytes()),
                    "narrative",
                    side,
                ));
            }
        }
        if matches!(event.provider.as_str(), "codex" | "pi") {
            let old_id = if native {
                event.metadata["chat_compatibility_legacy_id"].as_str()
            } else {
                Some(event.id.as_str())
            };
            if let Some(old_id) = old_id.filter(|id| !id.is_empty()) {
                let sequence = if native {
                    event.metadata["chat_compatibility_source_sequence"].as_u64()
                } else if legacy {
                    event.metadata["chat_legacy_source_sequence"].as_u64()
                } else {
                    None
                };
                let binding = json!([
                    self.scope,
                    event.provider,
                    event.kind,
                    event.role,
                    event.metadata["log_path"],
                    admission.context.provider_session_ids,
                    old_id
                ]);
                if let Some(sequence) = sequence.filter(|s| *s > 0) {
                    keys.push((
                        digest(
                            &serde_json::to_vec(&json!(["sequence", binding, sequence])).unwrap(),
                        ),
                        "sequence",
                        legacy,
                    ));
                }
                // Only unsequenced historical rows enter the prefix alternative;
                // every native occurrence counts, including ones with a sequence.
                if native || sequence.is_none() {
                    keys.push((
                        digest(&serde_json::to_vec(&json!(["prefix", binding])).unwrap()),
                        "prefix",
                        legacy,
                    ));
                }
            }
        }
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let member = Member {
            token: token.clone(),
            id: event.id.clone(),
            row: row.into(),
            buckets: keys.iter().map(|(k, _, _)| k.clone()).collect(),
            legacy,
        };
        if let Some(reference) = store.get(&self.members, &token)? {
            let prior: Member = store.read(&reference)?;
            if prior.buckets != member.buckets {
                // A requalified occurrence cannot transfer an earlier relation.
                for key in prior.buckets {
                    if let Some(mut bucket) = self.bucket(store, &key)? {
                        bucket.conflict = true;
                        let r = store.put(&bucket)?;
                        self.buckets = Some(store.insert(&self.buckets, &key, &r)?);
                    }
                }
                return self.updates(store, &[token]);
            }
        }
        let r = store.put(&member)?;
        self.members = Some(store.insert(&self.members, &token, &r)?);
        let mut affected = vec![token.clone()];
        for (key, kind, right) in keys {
            let mut bucket = self.bucket(store, &key)?.unwrap_or(Bucket {
                scope: self.scope.clone(),
                kind: kind.into(),
                left: Vec::new(),
                right: Vec::new(),
                conflict: false,
                watermark: admission.watermark,
            });
            let list = if right {
                &mut bucket.right
            } else {
                &mut bucket.left
            };
            if !list.contains(&token) {
                if list.len() < 2 {
                    list.push(token.clone());
                } else {
                    bucket.conflict = true;
                }
            }
            bucket.watermark = admission.watermark;
            affected.extend(bucket.left.iter().chain(&bucket.right).cloned());
            let r = store.put(&bucket)?;
            self.buckets = Some(store.insert(&self.buckets, &key, &r)?);
            if kind != "narrative" {
                self.pending = Some(store.insert(&self.pending, &key, &r)?);
                self.claims = Some(store.insert(&self.claims, &key, &r)?);
                self.before = None;
            }
        }
        self.updates(store, &affected)
    }

    fn updates(
        &self,
        store: &mut Store,
        affected: &[String],
    ) -> io::Result<Vec<(String, Option<Relation>)>> {
        let mut seeds = BTreeSet::from_iter(affected.iter().cloned());
        let mut updates = BTreeMap::new();
        while let Some(seed) = seeds.pop_first() {
            let mut members = BTreeMap::new();
            let mut todo = vec![seed];
            let mut narrative = BTreeSet::new();
            while let Some(token) = todo.pop() {
                if members.contains_key(&token) {
                    continue;
                }
                // One narrative pair plus a one-to-one historical owner for
                // each native member. A larger component is ambiguous.
                if members.len() >= 8 {
                    break;
                }
                let member = self.member(store, &token)?;
                for key in &member.buckets {
                    if let Some(bucket) = self.bucket(store, key)? {
                        if self.qualified(&bucket) {
                            if bucket.kind == "narrative" {
                                narrative.insert(key.clone());
                            }
                            todo.extend(bucket.left.iter().chain(&bucket.right).cloned());
                        }
                    }
                }
                members.insert(token, member);
            }
            let relation = if members.len() > 1 && members.len() <= 4 && todo.is_empty() {
                let id = if let Some(key) = narrative.first() {
                    format!("display:{key}")
                } else {
                    members
                        .values()
                        .filter(|m| m.legacy)
                        .map(|m| m.id.clone())
                        .min()
                        .unwrap_or_else(|| members.values().map(|m| m.id.clone()).min().unwrap())
                };
                Some(Relation {
                    id,
                    members: members.values().map(|m| m.id.clone()).collect(),
                })
            } else {
                None
            };
            for (token, member) in members {
                seeds.remove(&token);
                updates.insert(member.row, relation.clone());
            }
        }
        Ok(updates.into_iter().collect())
    }

    /// Activate already-counted claims in fixed checkpoints after the archive
    /// prefix is indexed. No page request walks this queue or native history.
    pub(super) fn activate(
        &mut self,
        store: &mut Store,
        limit: usize,
    ) -> io::Result<Vec<(String, Option<Relation>)>> {
        if !self.legacy_ready {
            return Ok(Vec::new());
        }
        let entries = store.page(&self.pending, self.before.as_deref(), limit)?;
        let mut affected = Vec::new();
        for (key, _) in &entries {
            if let Some(bucket) = self.bucket(store, key)? {
                affected.extend(bucket.left.iter().chain(&bucket.right).cloned());
            }
        }
        self.before = entries.last().map(|(key, _)| key.clone());
        if entries.len() < limit {
            self.pending = None;
            self.before = None;
        }
        self.updates(store, &affected)
    }

    pub(super) fn pending(&self) -> bool {
        self.legacy_ready && self.pending.is_some()
    }

    pub(super) fn qualify_legacy(&mut self, complete_prefix: bool) {
        if !self.legacy_ready || self.complete_prefix != complete_prefix {
            self.pending = self.claims.clone();
            self.before = None;
        }
        self.legacy_ready = true;
        self.complete_prefix = complete_prefix;
    }

    /// Preserve original sequence counts while acquisition is still pending;
    /// qualification can later change without rewriting a physical member's
    /// evidence keys or treating newly trusted evidence as an identity conflict.
    pub(super) fn qualify_sequences(&mut self, ready: bool) {
        if self.sequence_ready != ready {
            self.pending = self.claims.clone();
            self.before = None;
        }
        self.sequence_ready = ready;
    }

    pub(super) fn admits(&self, admission: &str) -> bool {
        self.scope.is_empty() || self.admission == admission
    }
    pub(super) fn has_evidence(&self) -> bool {
        self.members.is_some()
    }

    /// Source-tail candidates cannot certify occurrence uniqueness. The saved
    /// watermark still qualifies unchanged buckets from an earlier complete
    /// interval; newly touched buckets must await the next completed interval.
    pub(super) fn defer_narratives(&mut self) {
        self.narrative_deferred = true;
    }

    pub(super) fn narratives_completed(&self, watermark: u64) -> bool {
        self.narrative_watermark == Some(watermark)
    }

    pub(super) fn completed_watermark(&self) -> Option<u64> {
        self.narrative_watermark
    }

    /// Pin an immutable bucket root after the caller completes all admitted
    /// counts through this watermark. This is an index queue, never a source
    /// replay. Its root/cursor survive restart in the published owner head.
    pub(super) fn qualify_narratives(&mut self, watermark: u64) {
        if !self.narratives_completed(watermark) {
            self.narrative_watermark = Some(watermark);
            self.narrative_queue = self.buckets.clone();
            self.narrative_before = None;
        }
    }

    pub(super) fn narrative_pending(&self) -> bool {
        self.narrative_queue.is_some()
    }

    pub(super) fn activate_narratives(
        &mut self,
        store: &mut Store,
        limit: usize,
    ) -> io::Result<Vec<(String, Option<Relation>)>> {
        let entries = store.page(
            &self.narrative_queue,
            self.narrative_before.as_deref(),
            limit,
        )?;
        let mut affected = Vec::new();
        for (key, _) in &entries {
            if let Some(bucket) = self.bucket(store, key)? {
                if bucket.kind == "narrative" {
                    affected.extend(bucket.left.iter().chain(&bucket.right).cloned());
                }
            }
        }
        self.narrative_before = entries.last().map(|(key, _)| key.clone());
        if entries.len() < limit {
            self.narrative_queue = None;
            self.narrative_before = None;
        }
        self.updates(store, &affected)
    }
}

/// Revalidate the saved acquisition owner before using any retained claims.
/// This reads only capture/policy metadata, the fixed native header and overlap.
pub(super) struct SourceProof {
    pub(super) path: String,
    pub(super) epoch: String,
    pub(super) admission: String,
    pub(super) watermark: u64,
    pub(super) sequence_trusted: bool,
    pub(super) narrative_complete: bool,
    pub(super) complete_prefix: bool,
    pub(super) policy: super::chat_read::SourcePolicy,
}

pub(super) fn source_proof(
    context: &ConversationArchiveContext,
) -> io::Result<Option<SourceProof>> {
    use std::io::Read;
    if !matches!(context.provider.as_str(), "codex" | "pi") {
        return Ok(None);
    }
    let state = super::read_capture_state(&context.agent_id)?;
    let Some(source) = state
        .provider_log_sources
        .iter()
        .find(|s| context.provider_source_key.as_deref() == Some(s.provider_source_key.as_str()))
    else {
        return Ok(None);
    };
    let policy = match super::chat_read::source_policy(context) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    if source.native_identity != policy.identity || context.provider_session_ids.is_empty() {
        return Ok(None);
    }
    let mut file = match std::fs::File::open(&source.path) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    if crate::commands::provider_log_acquisition::native_file_identity(&file)?
        != source.native_identity
        || !crate::commands::provider_log_acquisition::published_anchor_matches(
            &mut file,
            &source.continuity_anchor,
        )?
    {
        return Ok(None);
    }
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut header = Vec::new();
    (&mut file).take(4096).read_to_end(&mut header)?;
    let Some(end) = header.iter().position(|b| *b == b'\n') else {
        return Ok(None);
    };
    let Ok(header) = serde_json::from_slice::<serde_json::Value>(&header[..end]) else {
        return Ok(None);
    };
    let native_session = if context.provider == "codex" && header["type"] == "session_meta" {
        header["payload"]["id"].as_str()
    } else if context.provider == "pi" && header["type"] == "session" {
        header["id"].as_str()
    } else {
        None
    };
    if native_session.is_none_or(|id| !context.provider_session_ids.iter().any(|s| s == id)) {
        return Ok(None);
    }
    let sequence_trusted = source.unknown_before_offset.is_none()
        && source.disabled_spans.is_empty()
        && source.open_disabled_from.is_none()
        && source.reason.is_none();
    let narrative_complete = sequence_trusted
        && source.status == "complete"
        && !source.normalizer.has_pending_events()
        && file.metadata()?.len() == source.committed_offset;
    Ok(Some(SourceProof {
        path: source.path.clone(),
        epoch: digest(&serde_json::to_vec(&source.native_identity).map_err(io::Error::other)?),
        admission: policy.admission_digest()?,
        watermark: source.committed_offset,
        // EOF proves acquisition continuity, not that this conversation's
        // candidate contains every occurrence in the complete native prefix.
        // Do not enable the optional unsequenced bridge without a separately
        // persisted whole-prefix occurrence ledger.
        complete_prefix: false,
        sequence_trusted,
        narrative_complete,
        policy,
    }))
}

impl SourceProof {
    pub(super) fn admission<'a>(
        &'a self,
        context: &'a ConversationArchiveContext,
        conversation: &'a str,
    ) -> Admission<'a> {
        Admission {
            context,
            conversation: Some(conversation),
            epoch: &self.epoch,
            admission: &self.admission,
            path: &self.path,
            watermark: self.watermark,
            sequence_trusted: self.sequence_trusted,
        }
    }
}

#[cfg(test)]
#[path = "chat_logical_index_tests.rs"]
mod tests;
