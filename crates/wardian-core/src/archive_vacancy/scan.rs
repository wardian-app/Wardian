//! Schema-specific offline scan. Provider paths and content integers are never followed.
mod tail;
use super::{
    boundary::Root, conflict, RuleOutcome, SequenceVacancy, VacancyEvidence, VacancyReport,
    MAX_VACANCIES, MAX_VACANCY_LINE_BYTES,
};
use crate::conversations::{
    ConversationManifest, ConversationNarrativeRecord, ConversationSourceRecord,
    ConversationTurnRecord,
};
use crate::models::chat::AgentChatEvent;
use serde::{
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
    path::Path,
};

const NARRATIVE: &[&str] = &[
    "schema",
    "seq",
    "turn_id",
    "at",
    "kind",
    "role",
    "speaker_type",
    "input_origin",
    "input_purpose",
    "request_root_id",
    "causal_ref",
    "text",
    "tool",
    "status",
    "summary",
    "excerpt",
    "event_refs",
    "source_refs",
    "artifact_refs",
];
const EVENT: &[&str] = &[
    "id",
    "session_id",
    "provider",
    "kind",
    "role",
    "text",
    "title",
    "status",
    "turn_id",
    "source",
    "command",
    "exit_code",
    "path",
    "language",
    "created_at",
    "sequence",
    "metadata",
];
const SOURCE: &[&str] = &[
    "schema",
    "source_id",
    "provider",
    "provider_session_id",
    "source_kind",
    "source_path",
    "cursor",
    "offset",
    "row_id",
    "provider_event_type",
    "hash",
    "artifact_ref",
];
const TURN: &[&str] = &[
    "schema",
    "conversation_id",
    "turn_index",
    "turn_key",
    "status",
    "status_source",
    "seq_start",
    "seq_end",
    "started_at",
    "updated_at",
    "request",
    "assistant_result",
    "counts",
    "tools_used",
    "files",
    "external_side_effects",
    "failure_signals",
    "record_refs",
    "provider_refs",
];
const MANIFEST: &[&str] = &[
    "schema",
    "conversation_id",
    "agent_id",
    "agent_name",
    "agent_class",
    "workspace",
    "provider",
    "provider_session_ids",
    "provider_source_key",
    "provider_native_refs",
    "effective_logging",
    "created_at",
    "updated_at",
    "closed_at",
    "status",
    "boundary_reason",
    "format_versions",
    "record_count",
    "turn_count",
    "has_turns",
    "lifecycle_only",
];

pub(super) fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub(super) fn check_identity(id: &str, sequence: u64) -> io::Result<()> {
    require(safe_id(id), sequence, "conversation_identity")
}

fn require(condition: bool, sequence: u64, rule: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(conflict(sequence, rule))
    }
}

fn disk(error: io::Error, sequence: u64) -> io::Error {
    let rule = match error.kind() {
        io::ErrorKind::PermissionDenied => "conversation_contained",
        io::ErrorKind::Unsupported => "platform_supported",
        _ => "archive_readable",
    };
    conflict(sequence, rule)
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn body_matches(directory: &Path, reference: &str, expected: &[u8]) -> io::Result<bool> {
    let root = Root::new(directory)?;
    let (length, digest) = root.hash(&body_path(reference, 0)?)?;
    Ok(length == expected.len() as u64 && digest == hash(expected))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
fn keys(value: &Value, allowed: &[&str], seq: u64) -> io::Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| conflict(seq, "structured_evidence"))?;
    require(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        seq,
        "schema_supported",
    )
}
fn typed<T: DeserializeOwned>(value: &Value, seq: u64) -> io::Result<T> {
    serde_json::from_value(value.clone()).map_err(|_| conflict(seq, "structured_evidence"))
}
fn strings(value: &Value, key: &str, seq: u64) -> io::Result<Vec<String>> {
    let Some(value) = value.get(key) else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let array = value
        .as_array()
        .ok_or_else(|| conflict(seq, "structured_evidence"))?;
    let mut result = Vec::new();
    for item in array {
        let item = item
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| conflict(seq, "structured_evidence"))?;
        result.push(item.to_owned());
    }
    require(
        result.iter().collect::<BTreeSet<_>>().len() == result.len(),
        seq,
        "ownership_unique",
    )?;
    Ok(result)
}

/// Duplicate object keys are rejected even inside a payload rather than silently
/// allowing a last-wins decoder to hide structural evidence.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Json;
        impl<'de> Visitor<'de> for Json {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unique-key JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut out = Vec::new();
                while let Some(Unique(v)) = a.next_element()? {
                    out.push(v);
                }
                Ok(Unique(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some((key, Unique(value))) = a.next_entry::<String, Unique>()? {
                    if out.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate key"));
                    }
                }
                Ok(Unique(Value::Object(out)))
            }
        }
        d.deserialize_any(Json)
    }
}

fn json(bytes: &[u8], seq: u64) -> io::Result<Value> {
    serde_json::from_slice::<Unique>(bytes)
        .map(|v| v.0)
        .map_err(|_| conflict(seq, "structured_evidence"))
}

pub(super) fn read_ledger(directory: &Path, id: &str) -> io::Result<Vec<SequenceVacancy>> {
    let root = match Root::new(directory) {
        Ok(root) => root,
        // First capture has no conversation directory and no vacancy authority.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(disk(error, 0)),
    };
    let bytes = match root.read(
        "vacancies.jsonl",
        Some(MAX_VACANCIES * MAX_VACANCY_LINE_BYTES),
    ) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(disk(error, 0)),
    };
    require(
        bytes.is_empty() || bytes.ends_with(b"\n"),
        0,
        "ledger_lf_terminated",
    )?;
    let mut rows = Vec::new();
    let mut sequences = BTreeSet::new();
    let mut authorizations = BTreeSet::new();
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        require(line.len() <= MAX_VACANCY_LINE_BYTES, 0, "ledger_line_limit")?;
        require(
            !line.contains(&b'\r') && line.len() > 1,
            0,
            "ledger_lf_terminated",
        )?;
        let value = json(&line[..line.len() - 1], 0)?;
        let seq = value["sequence"].as_u64().unwrap_or(0);
        let row: SequenceVacancy = typed(&value, seq)?;
        require(
            row.schema == 1 && row.kind == "sequence_vacancy",
            seq,
            "ledger_schema",
        )?;
        require(row.conversation_id == id, seq, "conversation_identity")?;
        require(
            seq > 1 && seq < row.evidence.committed_head_sequence,
            seq,
            "sequence_bounds",
        )?;
        require(
            valid_hash(&row.evidence.prev_record_hash)
                && valid_hash(&row.evidence.next_record_hash)
                && valid_hash(&row.evidence.scan_digest),
            seq,
            "evidence_hash_encoding",
        )?;
        require(
            [
                &row.authorization_id,
                &row.approval_reference,
                &row.approval_text,
            ]
            .iter()
            .all(|s| !s.trim().is_empty()),
            seq,
            "authorization_complete",
        )?;
        require(
            chrono::DateTime::parse_from_rfc3339(&row.created_at).is_ok(),
            seq,
            "authorization_timestamp",
        )?;
        require(
            sequences.insert(seq) && authorizations.insert(row.authorization_id.clone()),
            seq,
            "ledger_unique",
        )?;
        rows.push(row);
        require(rows.len() <= MAX_VACANCIES, seq, "ledger_record_limit")?;
    }
    Ok(rows)
}

#[derive(Clone)]
struct Row {
    value: Value,
    raw: Vec<u8>,
    ordinal: usize,
}

pub(super) enum Purpose {
    Authorization,
    ExistingLedger {
        committed_head: u64,
        vacancies: BTreeSet<u64>,
    },
}

pub(super) struct Snapshot {
    root: Root,
    id: String,
    head: u64,
    records: BTreeMap<u64, Row>,
    pending_records: BTreeMap<u64, Row>,
    events: BTreeMap<String, Row>,
    aliases: BTreeMap<String, String>,
    event_owners: BTreeMap<String, u64>,
    sources: BTreeMap<String, Row>,
    source_rows: Vec<(String, Row)>,
    references: BTreeSet<u64>,
    reservations: BTreeSet<u64>,
    inventory: BTreeMap<String, (u64, [u8; 32])>,
    purpose: Purpose,
    pending_native: BTreeMap<String, u64>,
    pending_sources: BTreeMap<String, Vec<ConversationSourceRecord>>,
}

impl Snapshot {
    pub(super) fn load(directory: &Path, id: &str, seq: u64, purpose: Purpose) -> io::Result<Self> {
        let root = Root::new(directory).map_err(|e| disk(e, seq))?;
        let inventory = inventory(&root, seq)?;
        let mut snapshot = Self {
            root,
            id: id.to_owned(),
            head: 0,
            records: BTreeMap::new(),
            pending_records: BTreeMap::new(),
            events: BTreeMap::new(),
            aliases: BTreeMap::new(),
            event_owners: BTreeMap::new(),
            sources: BTreeMap::new(),
            source_rows: Vec::new(),
            references: BTreeSet::new(),
            reservations: BTreeSet::new(),
            inventory,
            purpose,
            pending_native: BTreeMap::new(),
            pending_sources: BTreeMap::new(),
        };
        let manifest_bytes = snapshot
            .root
            .read("manifest.json", None)
            .map_err(|e| disk(e, seq))?;
        let manifest = json(&manifest_bytes, seq)?;
        keys(&manifest, MANIFEST, seq)?;
        let parsed: ConversationManifest = typed(&manifest, seq)?;
        if let Some(refs) = manifest["provider_native_refs"].as_array() {
            for item in refs {
                keys(
                    item,
                    &[
                        "provider",
                        "provider_session_id",
                        "source_kind",
                        "source_path",
                    ],
                    seq,
                )?;
            }
        }
        require(
            parsed.schema == 1
                && parsed.format_versions.manifest == 1
                && parsed.format_versions.conversation == 1
                && parsed.format_versions.events == 1
                && parsed.format_versions.sources == 1
                && parsed.format_versions.turns == 3,
            seq,
            "schema_supported",
        )?;
        keys(
            &manifest["format_versions"],
            &["manifest", "conversation", "events", "sources", "turns"],
            seq,
        )?;
        require(
            parsed.conversation_id == id && safe_id(id),
            seq,
            "conversation_identity",
        )?;
        for row in snapshot.rows("conversation.jsonl", seq)? {
            keys(&row.value, NARRATIVE, seq)?;
            let record: ConversationNarrativeRecord = typed(&row.value, seq)?;
            require(
                record.schema == 1 && record.seq > 0,
                seq,
                "schema_supported",
            )?;
            snapshot.head = snapshot.head.max(record.seq);
            require(
                snapshot.records.insert(record.seq, row).is_none(),
                seq,
                "ownership_unique",
            )?;
        }
        for row in snapshot.rows("events.jsonl", seq)? {
            keys(&row.value, EVENT, seq)?;
            let event: AgentChatEvent = typed(&row.value, seq)?;
            require(
                event.session_id == parsed.agent_id && !event.id.is_empty(),
                seq,
                "conversation_identity",
            )?;
            let primary = event.id.clone();
            let mut aliases = strings(&event.metadata, "legacy_event_ids", seq)?;
            if event.metadata.get("provider_observation_ids").is_some() {
                require(
                    event.provider == "codex"
                        && event.metadata["provider_log"] == true
                        && event.role.as_ref().is_some_and(|role| {
                            matches!(role, crate::models::chat::AgentChatRole::Assistant)
                        })
                        && matches!(
                            event.source.as_deref(),
                            Some("event_msg" | "response_item" | "item.completed")
                        )
                        && text(&event.metadata, "log_path").is_some()
                        && text(&event.metadata, "provider_turn_id").is_some(),
                    seq,
                    "structured_evidence",
                )?;
                aliases.extend(strings(&event.metadata, "provider_observation_ids", seq)?);
            }
            aliases.push(primary.clone());
            for alias in aliases {
                if let Some(existing) = snapshot.aliases.insert(alias, primary.clone()) {
                    require(existing == primary, seq, "ownership_unique")?;
                }
            }
            require(
                snapshot.events.insert(primary, row).is_none(),
                seq,
                "ownership_unique",
            )?;
        }
        for row in snapshot.rows("sources.jsonl", seq)? {
            keys(&row.value, SOURCE, seq)?;
            let source: ConversationSourceRecord = typed(&row.value, seq)?;
            require(
                source.schema == 1 && !source.source_id.is_empty(),
                seq,
                "schema_supported",
            )?;
            if let Some(sender) = source.source_id.strip_prefix("agent:") {
                require(
                    source.provider == "wardian"
                        && source.source_kind == "wardian_agent"
                        && source.provider_session_id.as_deref() == Some(sender),
                    seq,
                    "ownership_unique",
                )?;
                let cursor = decimal(
                    source
                        .cursor
                        .as_deref()
                        .ok_or_else(|| conflict(seq, "structured_evidence"))?,
                    seq,
                )?;
                require(
                    !snapshot.source_rows.iter().any(|(id, row)| {
                        id == &source.source_id
                            && text(&row.value, "cursor") == Some(cursor.to_string().as_str())
                    }),
                    seq,
                    "ownership_unique",
                )?;
                snapshot
                    .sources
                    .entry(source.source_id.clone())
                    .or_insert_with(|| row.clone());
            } else {
                require(
                    snapshot
                        .sources
                        .insert(source.source_id.clone(), row.clone())
                        .is_none(),
                    seq,
                    "ownership_unique",
                )?;
            }
            snapshot.source_rows.push((source.source_id, row));
        }
        // Legacy generated events already have physical narrative authority;
        // only unowned modern envelopes can supply pending recovery evidence.
        snapshot.bind_physical_event_owners(seq)?;
        // Partial writes can leave durable generated envelopes before narrative.
        // Keep their proven ownership separate from physical head/neighbours.
        for (id, event) in &snapshot.events {
            if event.value["metadata"]["generated"] != true {
                continue;
            }
            if event.value["metadata"].get("archive_record").is_none() {
                snapshot.require_legacy_generated_owner(id, seq)?;
                continue;
            }
            let payload = &event.value["metadata"]["archive_record"];
            keys(payload, NARRATIVE, seq)?;
            let record: ConversationNarrativeRecord = typed(payload, seq)?;
            require(
                record.schema == 1 && record.seq > 0,
                seq,
                "schema_supported",
            )?;
            require(
                id == &format!("generated:{}:{}", snapshot.id, record.seq)
                    && record.event_refs.contains(id),
                seq,
                "structured_evidence",
            )?;
            if record.seq != seq && !snapshot.records.contains_key(&record.seq) {
                require(
                    snapshot
                        .pending_records
                        .insert(
                            record.seq,
                            Row {
                                value: payload.clone(),
                                raw: Vec::new(),
                                ordinal: event.ordinal,
                            },
                        )
                        .is_none(),
                    seq,
                    "ownership_unique",
                )?;
            }
        }
        let turns = snapshot.rows("turns.jsonl", seq)?;
        for row in &turns {
            snapshot.turn(&row.value, seq)?;
        }
        snapshot.resolve(&manifest, seq)?;
        snapshot.stable(seq)?;
        Ok(snapshot)
    }

    fn rows(&self, name: &str, seq: u64) -> io::Result<Vec<Row>> {
        let bytes = self.root.read(name, None).map_err(|e| disk(e, seq))?;
        require(
            bytes.is_empty() || bytes.ends_with(b"\n"),
            seq,
            "archive_lf_terminated",
        )?;
        bytes
            .split_inclusive(|b| *b == b'\n')
            .enumerate()
            .map(|(ordinal, line)| {
                let raw = &line[..line.len() - 1];
                require(!raw.is_empty(), seq, "structured_evidence")?;
                Ok(Row {
                    value: json(raw, seq)?,
                    raw: raw.to_vec(),
                    ordinal,
                })
            })
            .collect()
    }

    fn bind_physical_event_owners(&mut self, seq: u64) -> io::Result<()> {
        for (number, row) in &self.records {
            for id in strings(&row.value, "event_refs", seq)? {
                let canonical = self
                    .aliases
                    .get(&id)
                    .ok_or_else(|| conflict(seq, "references_resolved"))?
                    .clone();
                if let Some(prior) = self.event_owners.insert(canonical, *number) {
                    require(prior == *number, seq, "ownership_unique")?;
                }
            }
        }
        Ok(())
    }

    // Before recovery envelopes existed, the canonical generated ID and its
    // unique physical narrative owner supplied the same already-published slot.
    // Never use this representation to reconstruct a missing narrative.
    fn require_legacy_generated_owner(&self, id: &str, seq: u64) -> io::Result<()> {
        require(
            self.event_owners.get(id).is_some_and(|number| {
                self.records.contains_key(number) && id == format!("generated:{}:{number}", self.id)
            }),
            seq,
            "legacy_generated_owner_proven",
        )
    }

    fn add_id(&mut self, id: &str, seq: u64) -> io::Result<()> {
        if let Some(tail) = id.strip_prefix("generated:") {
            let (conversation, number) = tail
                .rsplit_once(':')
                .ok_or_else(|| conflict(seq, "structured_evidence"))?;
            require(conversation == self.id, seq, "conversation_identity")?;
            self.references.insert(decimal(number, seq)?);
        } else if let Some(number) = id.strip_prefix("wardian:input:") {
            self.references.insert(decimal(number, seq)?);
        } else if let Some(number) = id
            .strip_prefix("src_")
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        {
            self.references.insert(decimal(number, seq)?);
        }
        Ok(())
    }

    fn turn(&mut self, value: &Value, seq: u64) -> io::Result<()> {
        keys(value, TURN, seq)?;
        let turn: ConversationTurnRecord = typed(value, seq)?;
        require(turn.schema == 3, seq, "schema_supported")?;
        require(
            turn.conversation_id == self.id,
            seq,
            "conversation_identity",
        )?;
        require(
            turn.seq_start > 0 && turn.seq_start <= turn.seq_end,
            seq,
            "structured_evidence",
        )?;
        keys(
            &value["record_refs"],
            &[
                "seq_start",
                "seq_end",
                "conversation_seq_start",
                "conversation_seq_end",
                "event_count",
                "source_count",
                "artifact_count",
            ],
            seq,
        )?;
        keys(
            &value["request"],
            &[
                "seq",
                "kind",
                "text",
                "text_truncated",
                "objective_text",
                "objective_text_truncated",
            ],
            seq,
        )?;
        if !value["assistant_result"].is_null() {
            keys(
                &value["assistant_result"],
                &["seq", "text", "text_truncated"],
                seq,
            )?;
        }
        keys(
            &value["counts"],
            &[
                "records",
                "assistant_messages",
                "tool_calls",
                "tool_results",
                "nonzero_tool_results",
                "failed_tool_results",
                "timeouts",
            ],
            seq,
        )?;
        keys(&value["files"], &["read", "written", "mentioned"], seq)?;
        self.references.extend([
            turn.seq_start,
            turn.seq_end,
            turn.record_refs.seq_start,
            turn.record_refs.seq_end,
            turn.request.seq,
        ]);
        if let Some(answer) = turn.assistant_result {
            self.references.insert(answer.seq);
        }
        if !value["assistant_result"].is_null() {
            keys(
                &value["assistant_result"],
                &["seq", "text", "text_truncated"],
                seq,
            )?;
        }
        keys(
            &value["counts"],
            &[
                "records",
                "assistant_messages",
                "tool_calls",
                "tool_results",
                "nonzero_tool_results",
                "failed_tool_results",
                "timeouts",
            ],
            seq,
        )?;
        keys(&value["files"], &["read", "written", "mentioned"], seq)?;
        for failure in turn.failure_signals {
            self.references.insert(failure.seq);
        }
        for effect in turn.external_side_effects {
            self.references.insert(effect.evidence_seq);
        }
        for field in ["failure_signals", "external_side_effects", "provider_refs"] {
            if let Some(array) = value[field].as_array() {
                for item in array {
                    let allowed: &[&str] = match field {
                        "failure_signals" => &["kind", "seq", "tool", "summary"],
                        "external_side_effects" => &["kind", "evidence_seq", "summary", "paths"],
                        _ => &[
                            "provider",
                            "provider_session_id",
                            "source_kind",
                            "source_id",
                        ],
                    };
                    keys(item, allowed, seq)?;
                }
            }
        }
        for provider in turn.provider_refs {
            if let Some(source) = provider.source_id {
                require(
                    self.sources.contains_key(&source),
                    seq,
                    "references_resolved",
                )?;
                self.add_id(&source, seq)?;
            }
        }
        Ok(())
    }

    fn event_owner(&self, id: &str, seq: u64) -> io::Result<Option<u64>> {
        let canonical = self.aliases.get(id).map_or(id, String::as_str);
        let _ = seq;
        Ok(self.event_owners.get(canonical).copied())
    }

    fn resolve(&mut self, manifest: &Value, seq: u64) -> io::Result<()> {
        let records: Vec<_> = self
            .records
            .iter()
            .chain(self.pending_records.iter())
            .map(|(n, r)| (*n, r.value.clone()))
            .collect();
        let mut source_owners: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
        let mut bodies = BTreeSet::new();
        for (number, record) in &records {
            for field in ["request_root_id", "causal_ref"] {
                if let Some(id) = text(record, field) {
                    self.add_id(id, seq)?;
                }
            }
            for id in strings(record, "event_refs", seq)? {
                require(self.aliases.contains_key(&id), seq, "references_resolved")?;
                self.add_id(&id, seq)?;
                let primary = self.aliases.get(&id).expect("resolved alias").clone();
                if let Some(prior) = self.event_owners.insert(primary, *number) {
                    require(prior == *number, seq, "ownership_unique")?;
                }
            }
        }
        let events: Vec<_> = self
            .events
            .iter()
            .map(|(id, row)| (id.clone(), row.value.clone()))
            .collect();
        let mut unowned_native = Vec::new();
        for (id, event) in &events {
            let metadata = &event["metadata"];
            require(
                metadata.is_object() || metadata.is_null(),
                seq,
                "structured_evidence",
            )?;
            if let Some(object) = metadata.as_object() {
                for key in object.keys() {
                    let known = matches!(
                        key.as_str(),
                        "wardian_archive_coalesced_narrative_sequences"
                            | "_wardian_mock_retired_sequences"
                            | "chat_compatibility_source_sequence"
                            | "chat_legacy_source_sequence"
                            | "sequence"
                    );
                    require(!key.contains("seq") || known, seq, "schema_supported")?;
                }
            }
            for field in [
                "failure_signals",
                "external_side_effects",
                "record_refs",
                "request",
                "assistant_result",
                "tool_refs",
                "invocation_ref",
                "result_ref",
                "approval_ref",
            ] {
                require(metadata.get(field).is_none(), seq, "schema_supported")?;
            }
            require(
                !id.starts_with("generated:") || metadata["generated"] == true,
                seq,
                "structured_evidence",
            )?;
            self.add_id(id, seq)?;
            if let Some(aliases) = metadata.get("provider_mirror_request_root_ids") {
                require(aliases.is_array(), seq, "structured_evidence")?;
            }
            for field in [
                "legacy_event_ids",
                "provider_observation_ids",
                "provider_mirror_request_root_ids",
            ] {
                for alias in strings(metadata, field, seq)? {
                    self.add_id(&alias, seq)?;
                }
            }
            for field in ["request_root_id", "causal_ref"] {
                if let Some(value) = metadata.get(field) {
                    require(
                        value.is_null() || value.is_string(),
                        seq,
                        "structured_evidence",
                    )?;
                }
                if let Some(id) = text(metadata, field) {
                    self.add_id(id, seq)?;
                }
            }
            let owner = self.event_owner(id, seq)?;
            if metadata.get("archive_record").is_some() {
                require(metadata["generated"] == true, seq, "structured_evidence")?;
                keys(&metadata["archive_record"], NARRATIVE, seq)?;
                let record: ConversationNarrativeRecord = typed(&metadata["archive_record"], seq)?;
                require(
                    record.schema == 1 && record.seq > 0,
                    seq,
                    "schema_supported",
                )?;
                require(
                    id == &format!("generated:{}:{}", self.id, record.seq),
                    seq,
                    "structured_evidence",
                )?;
                require(
                    owner.is_none_or(|n| n == record.seq),
                    seq,
                    "ownership_unique",
                )?;
                self.references.insert(record.seq);
                for event in &record.event_refs {
                    self.add_id(event, seq)?;
                }
                for source in &record.source_refs {
                    self.add_id(source, seq)?;
                }
                require(
                    owner.is_some() || record.seq == seq,
                    seq,
                    "references_resolved",
                )?;
            } else if metadata["generated"] == true {
                self.require_legacy_generated_owner(id, seq)?;
            } else if tail::native_narrative(text(event, "kind")) && owner.is_none() {
                unowned_native.push(id.clone());
            }
            for marker in [
                "wardian_archive_coalesced_narrative_sequences",
                "_wardian_mock_retired_sequences",
            ] {
                if let Some(values) = metadata.get(marker) {
                    let values = values
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .ok_or_else(|| conflict(seq, "reservation_proven"))?;
                    require(
                        marker != "_wardian_mock_retired_sequences" || values.len() == 1,
                        seq,
                        "reservation_proven",
                    )?;
                    let pairs: Vec<_> = events
                        .iter()
                        .filter(|(other_id, other)| {
                            other_id != id
                                && if marker.starts_with("wardian_") {
                                    codex_pair(event, other)
                                } else {
                                    mock_pair(event, other, manifest)
                                }
                        })
                        .collect();
                    require(
                        pairs.len() == 1
                            && owner.is_some_and(|n| self.records.contains_key(&n))
                            && self.event_owner(&pairs[0].0, seq)? == owner,
                        seq,
                        "reservation_proven",
                    )?;
                    if marker.starts_with("wardian_") {
                        let owner_record = &self.records[&owner.expect("proved owner")].value;
                        require(
                            owner_record["kind"] == "message" && owner_record["role"] == "user",
                            seq,
                            "reservation_proven",
                        )?;
                    }
                    for value in values {
                        let n = value
                            .as_u64()
                            .filter(|n| *n > 0)
                            .ok_or_else(|| conflict(seq, "reservation_proven"))?;
                        require(
                            !self.records.contains_key(&n) && self.reservations.insert(n),
                            seq,
                            "reservation_proven",
                        )?;
                    }
                }
            }
        }
        self.prepare_load_tail(&unowned_native, seq)?;
        for (number, record) in &records {
            for source in strings(record, "source_refs", seq)? {
                require(
                    self.sources.contains_key(&source)
                        || self.pending_source_for_record(&source, *number),
                    seq,
                    "references_resolved",
                )?;
                source_owners
                    .entry(source.clone())
                    .or_default()
                    .insert(*number);
                self.add_id(&source, seq)?;
            }
            for body in strings(record, "artifact_refs", seq)? {
                bodies.insert(body_path(&body, seq)?);
            }
        }
        let sources: Vec<_> = self
            .source_rows
            .iter()
            .map(|(id, row)| (id.clone(), row.value.clone()))
            .collect();
        for (id, source) in &sources {
            let parsed: ConversationSourceRecord = typed(source, seq)?;
            let pending = self.pending_source_row(&parsed);
            let owners = source_owners.get(id);
            require(owners.is_some() || pending, seq, "references_resolved")?;
            self.add_id(id, seq)?;
            if source["provider"] == "wardian"
                && matches!(
                    text(source, "source_kind"),
                    Some("wardian_agent" | "wardian_lifecycle")
                )
            {
                let cursor =
                    text(source, "cursor").ok_or_else(|| conflict(seq, "structured_evidence"))?;
                let n = decimal(cursor, seq)?;
                self.references.insert(n);
                require(
                    owners.is_some_and(|owners| owners.contains(&n))
                        || pending
                        || n == seq
                        || self.reservations.contains(&n),
                    seq,
                    "references_resolved",
                )?;
            }
            if let Some(body) = text(source, "artifact_ref") {
                bodies.insert(body_path(body, seq)?);
            }
        }
        for (id, event) in &events {
            let owner = self.event_owner(id, seq)?;
            if let Some(source) = text(event, "source") {
                if source.starts_with("src_")
                    || source.starts_with("agent:")
                    || source.starts_with("watch:")
                {
                    require(
                        self.sources.contains_key(source)
                            || self.pending_source_for_event(id, source),
                        seq,
                        "references_resolved",
                    )?;
                    self.add_id(source, seq)?;
                }
            }
            let mut event_bodies = strings(&event["metadata"], "text_artifact_refs", seq)?;
            event_bodies.extend(strings(&event["metadata"], "artifact_refs", seq)?);
            for body in event_bodies {
                require(
                    owner.is_some() || self.pending_native_event(id),
                    seq,
                    "references_resolved",
                )?;
                bodies.insert(body_path(&body, seq)?);
            }
        }
        let actual: BTreeSet<_> = self
            .inventory
            .keys()
            .filter(|p| p.starts_with("artifacts/"))
            .cloned()
            .collect();
        require(actual == bodies, seq, "bodies_resolved")?;
        for n in &self.references {
            require(
                *n > 0
                    && (self.records.contains_key(n)
                        || self.pending_records.contains_key(n)
                        || self.pending_native_sequence(*n)
                        || self.reservations.contains(n)
                        || *n == seq),
                seq,
                "references_resolved",
            )?;
        }
        Ok(())
    }

    pub(super) fn stable(&self, seq: u64) -> io::Result<()> {
        require(
            inventory(&self.root, seq)? == self.inventory,
            seq,
            "snapshot_stable",
        )
    }

    fn neighbour(&self, seq: u64, candidate: u64) -> io::Result<String> {
        let record = self
            .records
            .get(&seq)
            .ok_or_else(|| conflict(candidate, "physical_neighbours"))?;
        let mut events = BTreeSet::new();
        for id in strings(&record.value, "event_refs", candidate)? {
            let primary = self
                .aliases
                .get(&id)
                .ok_or_else(|| conflict(candidate, "references_resolved"))?;
            events.insert(hash(&self.events[primary].raw));
        }
        let mut sources = BTreeSet::new();
        let cursor = seq.to_string();
        for id in strings(&record.value, "source_refs", candidate)? {
            let row = if id.starts_with("agent:") {
                self.source_rows
                    .iter()
                    .find(|(source, row)| {
                        source == &id && text(&row.value, "cursor") == Some(cursor.as_str())
                    })
                    .map(|(_, row)| row)
            } else {
                self.sources.get(&id)
            }
            .ok_or_else(|| conflict(candidate, "references_resolved"))?;
            sources.insert(hash(&row.raw));
        }
        let mut owned = BTreeMap::new();
        for body in strings(&record.value, "artifact_refs", candidate)? {
            let name = body_path(&body, candidate)?;
            let digest = self
                .inventory
                .get(&name)
                .ok_or_else(|| conflict(candidate, "bodies_resolved"))?
                .1;
            owned.insert(name, digest);
        }
        let mut digest = Sha256::new();
        frame(&mut digest, b"wardian.vacancy.neighbour.v1");
        section(&mut digest, b"narrative", &[record.raw.as_slice()]);
        for (tag, hashes) in [
            (b"events".as_slice(), events),
            (b"sources".as_slice(), sources),
        ] {
            frame(&mut digest, tag);
            digest.update((hashes.len() as u64).to_be_bytes());
            for item in hashes {
                frame(&mut digest, &item);
            }
        }
        frame(&mut digest, b"bodies");
        digest.update((owned.len() as u64).to_be_bytes());
        for (path, hash) in owned {
            frame(&mut digest, path.as_bytes());
            frame(&mut digest, &hash);
        }
        Ok(hex(&digest.finalize()))
    }

    pub(super) fn inspect(&self, seq: u64) -> io::Result<VacancyReport> {
        let neighbours = seq
            .checked_sub(1)
            .zip(seq.checked_add(1))
            .is_some_and(|(a, b)| self.records.contains_key(&a) && self.records.contains_key(&b));
        let rules = [
            ("sequence_bounds", seq > 1 && seq < self.head),
            ("physical_sequence_absent", !self.records.contains_key(&seq)),
            ("physical_neighbours", neighbours),
            ("exact_reference_absent", !self.references.contains(&seq)),
            ("reservation_absent", !self.reservations.contains(&seq)),
        ]
        .into_iter()
        .map(|(rule, passed)| RuleOutcome {
            rule: rule.to_owned(),
            passed,
        })
        .collect::<Vec<_>>();
        let eligible = rules.iter().all(|rule| rule.passed);
        let evidence = if eligible {
            Some(VacancyEvidence {
                committed_head_sequence: self.head,
                prev_record_hash: self.neighbour(seq - 1, seq)?,
                next_record_hash: self.neighbour(seq + 1, seq)?,
                scan_digest: manifest_digest(&self.inventory),
            })
        } else {
            None
        };
        Ok(VacancyReport {
            sequence: seq,
            conversation_id: self.id.clone(),
            committed_head_sequence: self.head,
            eligible,
            rules,
            evidence,
        })
    }

    pub(super) fn validate(&self, row: &SequenceVacancy) -> io::Result<()> {
        let report = self.inspect(row.sequence)?;
        if let Some(rule) = report.rules.iter().find(|rule| !rule.passed) {
            return Err(conflict(row.sequence, &rule.rule));
        }
        let current = report
            .evidence
            .ok_or_else(|| conflict(row.sequence, "neighbour_hashes_match"))?;
        require(
            row.evidence.committed_head_sequence <= self.head,
            row.sequence,
            "committed_head",
        )?;
        require(
            current.prev_record_hash == row.evidence.prev_record_hash
                && current.next_record_hash == row.evidence.next_record_hash,
            row.sequence,
            "neighbour_hashes_match",
        )
    }
}

fn decimal(value: &str, seq: u64) -> io::Result<u64> {
    let n = value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or_else(|| conflict(seq, "structured_evidence"))?;
    Ok(n)
}

fn body_path(value: &str, seq: u64) -> io::Result<String> {
    let normalized = value.replace('\\', "/");
    let value = normalized.as_str();
    require(
        !value.is_empty() && !value.contains(':') && !value.starts_with('/'),
        seq,
        "conversation_contained",
    )?;
    let value = if value.starts_with("artifacts/") {
        value.to_owned()
    } else {
        require(!value.contains('/'), seq, "conversation_contained")?;
        format!("artifacts/{value}")
    };
    require(
        value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        seq,
        "conversation_contained",
    )?;
    Ok(value)
}

fn inventory(root: &Root, seq: u64) -> io::Result<BTreeMap<String, (u64, [u8; 32])>> {
    let canonical = [
        "manifest.json",
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
    ];
    let names = root.list("").map_err(|e| disk(e, seq))?;
    require(
        canonical.iter().all(|name| names.iter().any(|n| n == name)),
        seq,
        "canonical_files_present",
    )?;
    let mut files = BTreeMap::new();
    let mut pending = Vec::new();
    for name in names {
        if name == "artifacts" {
            pending.push(name);
            continue;
        }
        require(
            canonical.contains(&name.as_str()) || name == "vacancies.jsonl",
            seq,
            "conversation_files_known",
        )?;
        files.insert(name.clone(), root.hash(&name).map_err(|e| disk(e, seq))?);
    }
    while let Some(directory) = pending.pop() {
        for name in root.list(&directory).map_err(|e| disk(e, seq))? {
            let path = format!("{directory}/{name}");
            match root.hash(&path) {
                Ok(digest) => {
                    files.insert(path, digest);
                }
                Err(error) => {
                    // Legacy artifacts/... paths may have nested owned directories.
                    // Both probes stay handle-relative and never follow a link.
                    if root.list(&path).is_ok() {
                        pending.push(path);
                    } else {
                        return Err(disk(error, seq));
                    }
                }
            }
        }
    }
    Ok(files)
}

fn frame(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}
fn section(hash: &mut Sha256, tag: &[u8], items: &[&[u8]]) {
    frame(hash, tag);
    hash.update((items.len() as u64).to_be_bytes());
    for item in items {
        frame(hash, item);
    }
}
fn manifest_digest(files: &BTreeMap<String, (u64, [u8; 32])>) -> String {
    let mut hash = Sha256::new();
    frame(&mut hash, b"wardian.vacancy.scan.v1");
    hash.update((files.len() as u64).to_be_bytes());
    for (path, (length, digest)) in files {
        frame(&mut hash, path.as_bytes());
        hash.update(length.to_be_bytes());
        frame(&mut hash, digest);
    }
    hex(&hash.finalize())
}

fn codex_pair(a: &Value, b: &Value) -> bool {
    let (request, mirror) = match (text(a, "source"), text(b, "source")) {
        (Some("response_item"), Some("event_msg")) => (a, b),
        (Some("event_msg"), Some("response_item")) => (b, a),
        _ => return false,
    };
    let digest = |value: &Value| {
        text(value, "text")
            .filter(|s| !s.is_empty())
            .map(|s| hex(&hash(s.as_bytes())))
            .or_else(|| {
                text(&value["metadata"], "codex_user_text_sha256")
                    .filter(|s| valid_hash(s))
                    .map(str::to_owned)
            })
    };
    let left = digest(request);
    left.is_some()
        && left == digest(mirror)
        && [request, mirror].iter().all(|e| {
            text(e, "provider").is_some_and(|s| s.eq_ignore_ascii_case("codex"))
                && e["kind"] == "message"
                && e["role"] == "user"
                && e["metadata"]["provider_log"] == true
                && text(e, "text").is_none_or(|text| !text.is_empty())
                && e["metadata"]["input_origin"] == "human_input"
                && e["metadata"]["input_purpose"] == "request"
                && text(&e["metadata"], "request_root_id").is_some()
        })
        && request["metadata"]["raw_type"] == "message"
        && mirror["metadata"]["raw_type"] == "user_message"
        && request["session_id"] == mirror["session_id"]
        && ["log_path", "provider_turn_id"].iter().all(|key| {
            text(&request["metadata"], key).is_some_and(|s| !s.is_empty())
                && request["metadata"][key] == mirror["metadata"][key]
        })
        && request["metadata"]["provider_session_id"] == mirror["metadata"]["provider_session_id"]
}

fn mock_pair(watch: &Value, native: &Value, manifest: &Value) -> bool {
    let Some(binding) = watch["metadata"].get("_wardian_mock_watch") else {
        return false;
    };
    let Some(receipt) = native["metadata"].get("_wardian_mock_native") else {
        return false;
    };
    if keys(
        binding,
        &[
            "agent",
            "source_key",
            "path",
            "session",
            "incarnation",
            "observation",
            "payload",
            "kind",
            "role",
            "source",
            "turn",
        ],
        0,
    )
    .is_err()
        || keys(
            receipt,
            &["identity", "coordinate", "epoch", "start", "end", "policy"],
            0,
        )
        .is_err()
        || [
            "agent",
            "source_key",
            "path",
            "session",
            "incarnation",
            "observation",
            "payload",
        ]
        .iter()
        .any(|key| text(binding, key).is_none_or(str::is_empty))
        || text(receipt, "epoch").is_none_or(str::is_empty)
        || receipt["policy"].as_u64().is_none()
    {
        return false;
    }
    let observation_prefix = format!("{}:", text(binding, "incarnation").unwrap_or(""));
    if text(binding, "observation")
        .and_then(|s| s.strip_prefix(&observation_prefix))
        .is_none_or(|n| decimal(n, 0).is_err())
        || [
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
            "chat_source_admission",
            "chat_display_physical_id",
            "chat_display_member_ids",
            "chat_recent_native_key",
            "chat_recent_native_version",
        ]
        .iter()
        .any(|key| !watch["metadata"][key].is_null())
    {
        return false;
    }
    let describes = |event: &Value| {
        let metadata = &event["metadata"];
        event["provider"] == "mock"
            && event["session_id"] == binding["agent"]
            && event["kind"] == binding["kind"]
            && event["role"] == binding["role"]
            && event["source"] == binding["source"]
            && event["turn_id"] == binding["turn"]
            && metadata["mock_mirror_logged"] == true
            && metadata["mock_observation_id"] == binding["observation"]
            && metadata["mock_provider_incarnation"] == binding["incarnation"]
            && metadata["mock_provider_session_id"] == binding["session"]
            && metadata["mock_observation_digest"] == binding["payload"]
    };
    let canonical_id = text(native, "id").unwrap_or("");
    let prefix = format!("source:{}:", text(binding, "agent").unwrap_or(""));
    let coordinate = canonical_id.strip_prefix(&prefix).is_some_and(|tail| {
        let parts: Vec<_> = tail.split(':').collect();
        parts.len() == 2
            && parts
                .iter()
                .all(|p| p.len() == 20 && p.bytes().all(|b| b.is_ascii_digit()))
    });
    let incarnation = text(binding, "incarnation")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .is_some_and(|u| {
            !u.is_nil() && Some(u.to_string().as_str()) == text(binding, "incarnation")
        });
    let mapper_prefix = format!("{}:", text(binding, "agent").unwrap_or(""));
    let mapper = text(watch, "id")
        .and_then(|s| s.strip_prefix(&mapper_prefix))
        .and_then(|s| s.split_once(':'))
        .is_some_and(|(n, source)| {
            n.len() == 16
                && n.bytes().all(|b| b.is_ascii_hexdigit())
                && Some(source) == text(binding, "source")
        });
    describes(watch)
        && describes(native)
        && incarnation
        && coordinate
        && mapper
        && binding["kind"] == "message"
        && binding["role"] == "assistant"
        && matches!(text(binding, "source"), Some("model" | "message" | "info"))
        && text(binding, "payload").is_some_and(valid_hash)
        && binding["source_key"] == manifest["provider_source_key"]
        && receipt["identity"] == *binding
        && watch["metadata"]["provider_log"] != true
        && native["metadata"]["provider_log"] == true
        && receipt["coordinate"] == native["id"]
        && native["metadata"]["chat_source_ref"] == receipt["coordinate"]
        && native["metadata"]["chat_source_epoch"] == receipt["epoch"]
        && native["metadata"]["chat_source_start"] == receipt["start"]
        && native["metadata"]["chat_source_end"] == receipt["end"]
        && receipt["start"]
            .as_u64()
            .zip(receipt["end"].as_u64())
            .is_some_and(|(a, b)| a < b)
}
