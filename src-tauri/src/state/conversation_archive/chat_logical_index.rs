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
use crate::providers::chat_transcript::{
    codex_display_evidence, codex_display_evidence_from_bounded_header, CodexDisplaySide,
};

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

impl Relation {
    pub(super) fn prefer_member_first(&mut self, member_id: &str) {
        if let Some(index) = self.members.iter().position(|id| id == member_id) {
            self.members.swap(0, index);
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Member {
    token: String,
    id: String,
    row: String,
    buckets: Vec<String>,
    legacy: bool,
    #[serde(default)]
    commentary_id: Option<String>,
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

fn narrative_bucket_key(scope: &str, evidence: &str) -> String {
    digest(format!("{scope}:narrative:{evidence}").as_bytes())
}

fn narrative_side_is_right(side: CodexDisplaySide) -> bool {
    matches!(
        side,
        CodexDisplaySide::UserMirror | CodexDisplaySide::AssistantFinal
    )
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

/// Ownership of complete saved archive envelopes, independent of native-source
/// admission. Member-to-group presentation aliases reconcile archived display
/// rows only; this scope grants no native source-coordinate correspondence.
pub(super) struct ArchiveAdmission<'a> {
    pub(super) context: &'a ConversationArchiveContext,
    pub(super) conversation: &'a str,
    pub(super) generation: &'a str,
    pub(super) extent: u64,
}

impl ArchiveAdmission<'_> {
    fn scope(&self) -> String {
        digest(
            &serde_json::to_vec(&json!([
                "archive-display-v1",
                self.context.agent_id,
                self.context.provider,
                self.context.provider_source_key,
                self.context.provider_session_ids,
                self.conversation,
                self.generation,
                self.extent,
            ]))
            .unwrap(),
        )
    }

    fn owned(&self, event: &AgentChatEvent) -> bool {
        event.session_id == self.context.agent_id
            && event.provider == "codex"
            && event.provider == self.context.provider
            && event.kind == AgentChatEventKind::Message
            && event.role == Some(wardian_core::models::chat::AgentChatRole::Assistant)
            && event.metadata["provider_log"] == true
            && event.metadata["generated"] != true
            && event.metadata["chat_provisional"] != true
            && event.metadata["chat_legacy_identity_ineligible"] != true
            && !event.id.starts_with("source:")
            && [
                "chat_source_ref",
                "chat_source_start",
                "chat_source_end",
                "chat_source_epoch",
            ]
            .iter()
            .all(|key| event.metadata[*key].is_null())
            && event.metadata["conversation_archive_id"]
                .as_str()
                .is_none_or(|id| id == self.conversation)
            && event.metadata["provider_session_id"]
                .as_str()
                .is_none_or(|id| {
                    self.context
                        .provider_session_ids
                        .iter()
                        .any(|known| known == id)
                })
    }

    fn evidence(&self, event: &AgentChatEvent) -> Option<(String, bool)> {
        let required = |key: &str| {
            event
                .metadata
                .get(key)?
                .as_str()
                .filter(|s| !s.trim().is_empty())
        };
        let optional = |key: &str| match event.metadata.get(key) {
            None | Some(serde_json::Value::Null) => Some(None),
            Some(_) => required(key).map(Some),
        };
        let phase = optional("provider_phase")?;
        let completed = match (event.source.as_deref(), required("raw_type")) {
            (Some("event_msg"), Some("agent_message" | "assistant_message"))
                if event.turn_id.is_none()
                    && matches!(phase, None | Some("final_answer" | "commentary")) =>
            {
                false
            }
            (Some("response_item"), Some("message"))
                if event
                    .turn_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty())
                    && matches!(phase, Some("final_answer" | "commentary")) =>
            {
                true
            }
            _ => return None,
        };
        // An excerpt cannot establish full-body equality. Older archives without
        // complete inline text remain separate rather than trusting clipped data.
        if !event.metadata["text_excerpt"].is_null()
            || !event.metadata["text_artifact_refs"].is_null()
            || event.metadata["codex_display_turn_conflict"] == true
        {
            return None;
        }
        let text = event
            .text
            .as_deref()
            .filter(|text| !text.trim().is_empty())?;
        let evidence = serde_json::to_string(&json!([
            "archive-assistant-v1",
            required("log_path")?,
            required("provider_turn_id")?,
            optional("provider_session_id")?,
            optional("source_path")?,
            optional("conversation_archive_id")?,
            phase == Some("commentary"),
            digest(text.as_bytes()),
            text.len(),
        ]))
        .ok()?;
        Some((evidence, completed))
    }
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
        let Ok(ordinal_value) = ordinal.parse::<u64>() else {
            return false;
        };
        start < end
            && end <= self.watermark
            && valid_ref(raw)
            && ordinal_value.to_string() == ordinal
            && event.metadata["chat_source_ref"].as_str() == Some(event.id.as_str())
            && event.metadata["chat_source_epoch"].as_str() == Some(self.epoch)
    }

    /// Checks source ownership independently from narrative-message eligibility.
    /// Native tool rows remain covered source records even though they do not
    /// contribute observations to the display relation index.
    pub(super) fn source_owned(&self, event: &AgentChatEvent) -> bool {
        event.session_id == self.context.agent_id
            && event.provider == self.context.provider
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

    pub(super) fn owned(&self, event: &AgentChatEvent) -> bool {
        event.kind == AgentChatEventKind::Message
            && event.role.is_some()
            && self.source_owned(event)
    }
}

impl Index {
    /// Count an owned legacy assistant envelope before body or metadata clipping.
    /// Activation must wait for the complete immutable archive extent. Native
    /// rows and compatibility bridges never enter this separate display index.
    pub(super) fn observe_archive(
        &mut self,
        store: &mut Store,
        admission: &ArchiveAdmission<'_>,
        event: &AgentChatEvent,
        row: &str,
    ) -> io::Result<Vec<(String, Option<Relation>)>> {
        if !admission.owned(event) {
            return Ok(Vec::new());
        }
        let Some((evidence, completed)) = admission.evidence(event) else {
            return Ok(Vec::new());
        };
        let scope = admission.scope();
        if self.scope != scope {
            *self = Self {
                admission: scope.clone(),
                scope,
                narrative_deferred: true,
                ..Self::default()
            };
        }
        let key = narrative_bucket_key(&self.scope, &evidence);
        let member = Member {
            token: digest(&serde_json::to_vec(&json!([self.scope, event.id, row])).unwrap()),
            id: event.id.clone(),
            row: row.into(),
            buckets: vec![key.clone()],
            legacy: false,
            commentary_id: None,
        };
        self.register_member(
            store,
            member,
            vec![(key, "narrative", completed)],
            admission.extent,
        )
    }

    /// Persistent trees only. `before`/`narrative_before` are seek keys, while
    /// member/bucket fields contain logical IDs and ordinal keys, not roots.
    pub(super) fn node_roots(&self) -> impl Iterator<Item = &str> {
        [
            self.buckets.as_deref(),
            self.members.as_deref(),
            self.pending.as_deref(),
            self.claims.as_deref(),
            self.narrative_queue.as_deref(),
        ]
        .into_iter()
        .flatten()
    }

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

    /// Count a complete bounded native interval with the same narrative bucket
    /// and one-to-one qualification used by the persisted index. The caller
    /// must prove that the interval covers every admitted row before using it.
    pub(super) fn bounded_relations<'a>(
        admission: &Admission<'_>,
        events: impl IntoIterator<Item = &'a AgentChatEvent>,
    ) -> Vec<Relation> {
        let scope = admission.scope();
        let index = Self {
            scope: scope.clone(),
            admission: admission.admission.to_owned(),
            narrative_deferred: true,
            narrative_watermark: Some(admission.watermark),
            ..Self::default()
        };
        let mut buckets = BTreeMap::<String, Bucket>::new();
        let mut members = BTreeMap::<String, Member>::new();

        for event in events {
            if !admission.owned(event) || !admission.native(event) {
                continue;
            }
            let display_evidence = if event.metadata["chat_provisional"] == true {
                codex_display_evidence_from_bounded_header(event)
            } else {
                codex_display_evidence(event)
            };
            let Some((evidence, side)) = display_evidence else {
                continue;
            };
            let key = narrative_bucket_key(&scope, &evidence);
            let token = digest(&serde_json::to_vec(&json!([scope, event.id, ""])).unwrap());
            let commentary_id = (side == CodexDisplaySide::AssistantLive
                && event.source.as_deref() == Some("event_msg")
                && event.metadata["provider_phase"].as_str() == Some("commentary"))
            .then(|| event.id.clone());
            let member = Member {
                token: token.clone(),
                id: event.id.clone(),
                row: String::new(),
                buckets: vec![key.clone()],
                legacy: false,
                commentary_id,
            };
            if let Some(prior) = members.get(&token) {
                if prior.buckets != member.buckets {
                    for conflicting in prior.buckets.iter().chain(std::iter::once(&key)) {
                        buckets
                            .entry(conflicting.clone())
                            .or_insert_with(|| Bucket {
                                scope: scope.clone(),
                                kind: "narrative".into(),
                                left: Vec::new(),
                                right: Vec::new(),
                                conflict: false,
                                watermark: admission.watermark,
                            })
                            .conflict = true;
                    }
                }
                continue;
            }
            members.insert(token.clone(), member);
            let bucket = buckets.entry(key).or_insert_with(|| Bucket {
                scope: scope.clone(),
                kind: "narrative".into(),
                left: Vec::new(),
                right: Vec::new(),
                conflict: false,
                watermark: admission.watermark,
            });
            let side_members = if narrative_side_is_right(side) {
                &mut bucket.right
            } else {
                &mut bucket.left
            };
            if !side_members.contains(&token) {
                if side_members.len() < 2 {
                    side_members.push(token);
                } else {
                    bucket.conflict = true;
                }
            }
        }

        buckets
            .into_iter()
            .filter_map(|(key, bucket)| {
                if !index.qualified(&bucket) {
                    return None;
                }
                let tokens = bucket
                    .left
                    .iter()
                    .chain(&bucket.right)
                    .cloned()
                    .collect::<BTreeSet<_>>();
                if tokens.len() != 2 {
                    return None;
                }
                let component = tokens
                    .iter()
                    .filter_map(|token| members.get(token))
                    .collect::<Vec<_>>();
                if component.len() != 2 {
                    return None;
                }
                let commentary_ids = component
                    .iter()
                    .filter_map(|member| member.commentary_id.as_deref())
                    .collect::<Vec<_>>();
                let stable_commentary_id = (commentary_ids.len() == 1).then(|| commentary_ids[0]);
                let mut relation = Relation {
                    id: stable_commentary_id
                        .map(|id| id.to_owned())
                        .unwrap_or_else(|| format!("display:{key}")),
                    members: component.iter().map(|member| member.id.clone()).collect(),
                };
                if let Some(id) = stable_commentary_id {
                    relation.prefer_member_first(id);
                }
                Some(relation)
            })
            .collect()
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
        let mut commentary_id = None;
        if native {
            let display_evidence = if event.metadata["chat_provisional"] == true {
                codex_display_evidence_from_bounded_header(event)
            } else {
                codex_display_evidence(event)
            };
            if let Some((evidence, side)) = display_evidence {
                if side == CodexDisplaySide::AssistantLive
                    && event.source.as_deref() == Some("event_msg")
                    && event.metadata["provider_phase"].as_str() == Some("commentary")
                {
                    commentary_id = Some(event.id.clone());
                }
                keys.push((
                    narrative_bucket_key(&self.scope, &evidence),
                    "narrative",
                    narrative_side_is_right(side),
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
            commentary_id,
        };
        self.register_member(store, member, keys, admission.watermark)
    }

    fn register_member(
        &mut self,
        store: &mut Store,
        member: Member,
        keys: Vec<(String, &str, bool)>,
        watermark: u64,
    ) -> io::Result<Vec<(String, Option<Relation>)>> {
        let token = member.token.clone();
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
                watermark,
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
            bucket.watermark = watermark;
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
                let (id, stable_commentary_id) = if let Some(key) = narrative.first() {
                    let commentary_ids = members
                        .values()
                        .filter_map(|member| member.commentary_id.as_deref())
                        .collect::<Vec<_>>();
                    let stable_commentary_id = (members.len() == 2 && commentary_ids.len() == 1)
                        .then(|| commentary_ids[0]);
                    let id = if let Some(id) = stable_commentary_id {
                        id.to_owned()
                    } else {
                        format!("display:{key}")
                    };
                    (id, stable_commentary_id)
                } else {
                    (
                        members
                            .values()
                            .filter(|m| m.legacy)
                            .map(|m| m.id.clone())
                            .min()
                            .unwrap_or_else(|| {
                                members.values().map(|m| m.id.clone()).min().unwrap()
                            }),
                        None,
                    )
                };
                let mut relation = Relation {
                    id,
                    members: members.values().map(|m| m.id.clone()).collect(),
                };
                if let Some(id) = stable_commentary_id {
                    relation.prefer_member_first(id);
                }
                Some(relation)
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
    if !matches!(context.provider.as_str(), "codex" | "pi") {
        return Ok(None);
    }
    checked_source_proof(context)
}

/// Capture publication calls this only after its required native cursor and
/// policy commit. This records coverage; it grants no display relation claims.
pub(super) fn mock_canonical_coverage(
    context: &ConversationArchiveContext,
) -> io::Result<Option<super::chat_read::CanonicalSourceCoverage>> {
    if context.provider != "mock" {
        return Ok(None);
    }
    Ok(
        checked_source_proof(context)?.map(|proof| super::chat_read::CanonicalSourceCoverage {
            source_key: context.provider_source_key.clone().unwrap_or_default(),
            epoch: proof.epoch,
            admission: proof.admission,
            extent: proof.watermark,
            normalizer_settled: proof.narrative_complete,
        }),
    )
}

fn checked_source_proof(context: &ConversationArchiveContext) -> io::Result<Option<SourceProof>> {
    use std::io::Read;
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
    if source.native_identity != policy.identity
        || source.policy_generation != policy.generation
        || source.unknown_before_offset.unwrap_or(0) != policy.unknown_before
        || source.disabled_spans != policy.disabled
        || source.open_disabled_from != policy.open_disabled
        || context.provider_session_ids.is_empty()
    {
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
    } else if context.provider == "mock" && header["type"] == "init" {
        header["session_id"].as_str()
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
    let privacy_exclusion_only = matches!(
        source.reason.as_deref(),
        None | Some("provider_log_prefix_policy_unknown")
            | Some("provider_log_disabled_span_skipped")
            | Some("provider_log_policy_boundary_recorded")
    );
    let narrative_complete = privacy_exclusion_only
        && policy.open_disabled.is_none()
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
