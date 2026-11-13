//! Authenticated persisted recent headers. Each request revalidates the pack
//! and canonical root; Older and detail retain their original object reads.
use std::collections::HashSet;
use std::io::{self, Write};

use serde::{Deserialize, Serialize, Serializer};

use super::super::chat_read_store::{digest, valid_ref, Dependency, Store, OBJECT_BYTES};
use super::{Head, Row, PAGE_ROWS};

pub(in crate::state::conversation_archive) const PACKED_BYTES: usize = 256 * 1024;
pub(in crate::state::conversation_archive) const NODE_BYTES: usize = 512;
// Even all 10^20 possible canonical keys cannot form an AVL tree this tall:
// the minimum node count at height 96 exceeds 10^20. Node JSON is <512 bytes
// with a 20-digit key, three 64-character references, and this height bound.
pub(in crate::state::conversation_archive) const TREE_HEIGHT: usize = 96;

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u8,
    head_binding: String,
    agent_id: String,
    conversation_id: String,
    generation: String,
    source_key: Option<String>,
    source_epoch: Option<String>,
    root: Option<String>,
    row_count: usize,
}

impl Binding {
    fn from_head(head: &Head) -> io::Result<Self> {
        // Bind every admission, logical, bootstrap, and publication field while
        // excluding the only field that would introduce a content-hash cycle.
        let mut unbound = head.clone();
        unbound.recent_page = None;
        let head_binding = digest(&serde_json::to_vec(&unbound).map_err(io::Error::other)?);
        Ok(Self {
            version: 1,
            head_binding,
            agent_id: head.agent_id.clone(),
            conversation_id: head.conversation_id.clone(),
            generation: head.generation.clone(),
            source_key: head.source_key.clone(),
            source_epoch: head.source_epoch.clone(),
            root: head.root.clone(),
            row_count: head.row_count,
        })
    }
}

/// Newest-first physical entries, exactly as the legacy recent tree walk emits
/// them. The parent owns display conversion, detail prefixes, and selection.
pub(in crate::state::conversation_archive) struct PackedPage {
    pub(super) entries: Vec<PackedEntry>,
    pub(super) has_older: bool,
    binding: Binding,
    headers: Vec<String>,
    lookahead: Option<(String, String)>,
}

pub(super) struct PackedEntry {
    pub(super) key: String,
    pub(super) reference: String,
    pub(super) row: Row,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncodedEntry {
    key: String,
    reference: String,
    // Preserve original bytes, including absent historical default fields.
    header: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncodedPage {
    binding: Binding,
    entries: Vec<EncodedEntry>,
    has_older: bool,
    lookahead: Option<(String, String)>,
}

impl Serialize for PackedPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Entry<'a> {
            key: &'a str,
            reference: &'a str,
            header: &'a str,
        }
        #[derive(Serialize)]
        struct Page<'a> {
            binding: &'a Binding,
            entries: Vec<Entry<'a>>,
            has_older: bool,
            lookahead: &'a Option<(String, String)>,
        }
        Page {
            binding: &self.binding,
            entries: self
                .entries
                .iter()
                .zip(&self.headers)
                .map(|(entry, header)| Entry {
                    key: &entry.key,
                    reference: &entry.reference,
                    header,
                })
                .collect(),
            has_older: self.has_older,
            lookahead: &self.lookahead,
        }
        .serialize(serializer)
    }
}

// Stop serialization at the first over-cap byte instead of allocating the
// maximum 80 * 16 KiB headers again after JSON string escaping.
#[derive(Default)]
struct BoundedEncoding {
    bytes: Vec<u8>,
    oversized: bool,
}

impl Write for BoundedEncoding {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let available = (PACKED_BYTES + 1).saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(available)]);
        if self.bytes.len() > PACKED_BYTES {
            self.oversized = true;
            return Err(io::Error::other("oversized packed chat recent page"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl PackedPage {
    pub(in crate::state::conversation_archive) fn encode(&self) -> io::Result<(Vec<u8>, bool)> {
        let mut encoding = BoundedEncoding::default();
        if let Err(error) = serde_json::to_writer(&mut encoding, self) {
            if !encoding.oversized {
                return Err(io::Error::other(error));
            }
        }
        Ok((encoding.bytes, encoding.oversized))
    }

    pub(in crate::state::conversation_archive) fn decode(
        store: &mut Store,
        bytes: &[u8],
    ) -> io::Result<Self> {
        let encoded: EncodedPage = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        if encoded.entries.len() > PAGE_ROWS {
            return Err(io::Error::other("too many packed chat headers"));
        }
        let mut entries = Vec::with_capacity(encoded.entries.len());
        let mut headers = Vec::with_capacity(encoded.entries.len());
        for entry in encoded.entries {
            // The file read charged the enclosing bytes, but each embedded Row
            // is an additional decode attempt, including a failing attempt.
            store.account_packed_header()?;
            let row = decode_header(&entry.header, &entry.reference)?;
            entries.push(PackedEntry {
                key: entry.key,
                reference: entry.reference,
                row,
            });
            headers.push(entry.header);
        }
        Ok(Self {
            binding: encoded.binding,
            entries,
            headers,
            has_older: encoded.has_older,
            lookahead: encoded.lookahead,
        })
    }

    fn validate(&self, head: &Head) -> io::Result<()> {
        if self.binding != Binding::from_head(head)?
            || self.entries.len() > PAGE_ROWS
            || self.entries.len() != self.headers.len()
            || self.entries.len() > head.row_count
            || self.has_older != self.lookahead.is_some()
            || (self.has_older && self.entries.len() != PAGE_ROWS)
            || (head.root.is_some() == self.entries.is_empty())
        {
            return Err(io::Error::other("packed chat page binding/count mismatch"));
        }
        let mut keys = HashSet::new();
        let mut references = HashSet::new();
        let mut previous: Option<&str> = None;
        for (key, reference) in self
            .entries
            .iter()
            .map(|entry| (&entry.key, &entry.reference))
            .chain(
                self.lookahead
                    .iter()
                    .map(|(key, reference)| (key, reference)),
            )
        {
            if key.len() != 20
                || !key.bytes().all(|byte| byte.is_ascii_digit())
                || key
                    .parse::<usize>()
                    .ok()
                    .is_none_or(|index| index >= head.row_count)
                || !valid_ref(reference)
                || previous.is_some_and(|previous| previous <= key.as_str())
                || !keys.insert(key)
                || !references.insert(reference)
            {
                return Err(io::Error::other("packed chat page key/reference mismatch"));
            }
            previous = Some(key);
        }
        for entry in &self.entries {
            let row = &entry.row;
            if row.agent_id != head.agent_id
                || row.conversation_id != head.conversation_id
                || row.event.session_id != head.agent_id
                || row.key != entry.key
                || row
                    .body
                    .as_ref()
                    .and_then(|body| body.root.as_deref())
                    .is_some_and(|root| !valid_ref(root))
            {
                return Err(io::Error::other("foreign packed chat row"));
            }
        }
        Ok(())
    }

    pub(in crate::state::conversation_archive) fn dependencies(&self) -> Vec<Dependency> {
        let mut dependencies = Vec::with_capacity(2 + 2 * PAGE_ROWS);
        dependencies.extend(self.binding.root.iter().cloned().map(Dependency::Node));
        for entry in &self.entries {
            dependencies.push(Dependency::Object(entry.reference.clone()));
            dependencies.extend(
                entry
                    .row
                    .body
                    .as_ref()
                    .and_then(|body| body.root.clone())
                    .map(Dependency::Node),
            );
        }
        dependencies.extend(
            self.lookahead
                .iter()
                .map(|(_, reference)| Dependency::Object(reference.clone())),
        );
        dependencies
    }
}

fn decode_header(header: &str, reference: &str) -> io::Result<Row> {
    if header.len() > OBJECT_BYTES
        || !valid_ref(reference)
        || digest(header.as_bytes()) != reference
    {
        return Err(io::Error::other("packed chat header changed/oversized"));
    }
    serde_json::from_str(header).map_err(io::Error::other)
}

/// Prepare an optional representation of the final canonical recent window.
/// Insufficient unspent allowance or a serialized oversize omits the pack.
/// Once work starts, failures propagate without resetting any Store counters.
pub(super) fn prepare(store: &mut Store, head: &Head) -> io::Result<Option<String>> {
    let mut trial = head.clone();
    trial.recent_page = Some("0".repeat(64));
    if serde_json::to_vec(&trial).map_err(io::Error::other)?.len() > OBJECT_BYTES
        || !store.can_prepare_packed_recent()
    {
        return Ok(None);
    }
    let mut canonical = store.packed_recent_entries(&head.root)?;
    let lookahead =
        (canonical.len() > PAGE_ROWS).then(|| canonical.pop().expect("lookahead exists"));
    let mut entries = Vec::with_capacity(canonical.len());
    let mut headers = Vec::with_capacity(canonical.len());
    for (key, reference) in canonical {
        let bytes = store.read_bytes(&reference, OBJECT_BYTES)?;
        let header = String::from_utf8(bytes).map_err(io::Error::other)?;
        let row = decode_header(&header, &reference)?;
        entries.push(PackedEntry {
            key,
            reference,
            row,
        });
        headers.push(header);
    }
    let packed = PackedPage {
        binding: Binding::from_head(head)?,
        has_older: lookahead.is_some(),
        lookahead,
        entries,
        headers,
    };
    packed.validate(head)?;
    store.put_packed_recent(&packed)
}

/// Read and validate a present pack. Missing, malformed, stale, oversized, or
/// corrupt objects fail closed. Individual Row/interior files are deliberately
/// not reopened: the pack is an independently authenticated representation.
pub(super) fn read(store: &mut Store, reference: &str, head: &Head) -> io::Result<PackedPage> {
    store.validate_packed_root(&head.root)?;
    let page = store.read_packed_recent(reference)?;
    page.validate(head)?;
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::fs;
    use std::path::Path;
    use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

    fn head() -> Head {
        serde_json::from_value(json!({
            "agent_id": "agent", "conversation_id": "conversation",
            "source_key": "mock:one", "source_epoch": null, "generation": "generation",
            "root": null, "identities": null, "committed_output_bytes": 0,
            "capture_stamp": "capture", "progress": "complete", "parent": null,
            "changes": [], "recent_start": 0, "row_count": 0
        }))
        .unwrap()
    }

    fn row(index: usize) -> Row {
        Row {
            agent_id: "agent".into(),
            conversation_id: "conversation".into(),
            key: format!("{index:020}"),
            event: AgentChatEvent {
                id: format!("event-{index}"),
                session_id: "agent".into(),
                provider: "mock".into(),
                kind: AgentChatEventKind::Message,
                role: Some(AgentChatRole::Assistant),
                text: Some(format!("preview-{index}")),
                title: None,
                status: None,
                turn_id: None,
                source: None,
                command: None,
                exit_code: None,
                path: None,
                language: None,
                created_at: None,
                sequence: Some(index as u64 + 1),
                metadata: json!({}),
            },
            body: None,
            relation: None,
            removed_display_ids: Vec::new(),
        }
    }

    fn fixture(path: &Path, count: usize) -> Head {
        let mut head = head();
        let mut writer = Store::writer(path);
        for index in 0..count {
            let row = row(index);
            let reference = writer.put(&row).unwrap();
            head.root = Some(writer.insert(&head.root, &row.key, &reference).unwrap());
        }
        head.row_count = count;
        head
    }

    fn packed(path: &Path, head: &Head) -> String {
        prepare(&mut Store::checkpoint_writer(path), head)
            .unwrap()
            .unwrap()
    }

    fn edited_pack(path: &Path, reference: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut value: Value =
            serde_json::from_slice(&fs::read(path.join(reference)).unwrap()).unwrap();
        edit(&mut value);
        let bytes = serde_json::to_vec(&value).unwrap();
        let reference = digest(&bytes);
        fs::write(path.join(&reference), bytes).unwrap();
        reference
    }

    #[test]
    fn packed_matches_legacy_rows_and_exact_lookahead() {
        for count in [0, 1, 8, 24, 80, 81, 100] {
            let temp = tempfile::tempdir().unwrap();
            let head = fixture(temp.path(), count);
            let reference = packed(temp.path(), &head);
            let packed = read(&mut Store::new(temp.path()), &reference, &head).unwrap();
            let mut legacy = Store::new(temp.path());
            let entries = legacy.page(&head.root, None, PAGE_ROWS + 1).unwrap();
            assert_eq!(packed.has_older, entries.len() > PAGE_ROWS);
            assert_eq!(packed.lookahead, entries.get(PAGE_ROWS).cloned());
            assert_eq!(packed.entries.len(), entries.len().min(PAGE_ROWS));
            for (actual, (key, reference)) in packed.entries.iter().zip(&entries) {
                let expected: Row = legacy.read(reference).unwrap();
                assert_eq!((&actual.key, &actual.reference), (key, reference));
                assert_eq!(
                    serde_json::to_value(&actual.row).unwrap(),
                    serde_json::to_value(expected).unwrap()
                );
            }
        }
    }

    #[test]
    fn complete_headers_preserve_relations_retractions_and_body_dependencies() {
        let temp = tempfile::tempdir().unwrap();
        let mut head = head();
        head.row_count = 1;
        let mut writer = Store::checkpoint_writer(temp.path());
        let body = writer.put_bytes(b"detail payload").unwrap();
        let body_root = writer.insert(&None, "00000000000000000000", &body).unwrap();
        let mut row = row(0);
        row.body = Some(super::super::Body {
            binding: digest(b"binding"),
            root: Some(body_root.clone()),
            bytes: 14,
            complete: false,
        });
        row.relation = Some(super::super::super::chat_logical_index::Relation {
            id: "logical".into(),
            members: vec!["event-0".into(), "related".into()],
        });
        row.removed_display_ids = vec!["retired".into()];
        let reference = writer.put(&row).unwrap();
        head.root = Some(writer.insert(&None, &row.key, &reference).unwrap());
        let reference = prepare(&mut writer, &head).unwrap().unwrap();
        assert!(!temp.path().join(&body_root).exists());
        // The pack alone is a typed root, including explicit body dependencies.
        writer
            .finish_checkpoint(&[Dependency::Object(reference.clone())])
            .unwrap();
        assert!(temp.path().join(body_root).is_file());
        let actual = read(&mut Store::new(temp.path()), &reference, &head).unwrap();
        assert_eq!(
            serde_json::to_value(&actual.entries[0].row).unwrap(),
            serde_json::to_value(row).unwrap()
        );
    }

    #[test]
    fn historical_row_bytes_and_absent_optional_head_field_remain_readable() {
        let temp = tempfile::tempdir().unwrap();
        let mut head = head();
        assert!(head.recent_page.is_none());
        head.row_count = 1;
        let mut old = serde_json::to_value(row(0)).unwrap();
        old.as_object_mut().unwrap().remove("relation");
        old.as_object_mut().unwrap().remove("removed_display_ids");
        let mut writer = Store::writer(temp.path());
        let old_ref = writer.put(&old).unwrap();
        head.root = Some(
            writer
                .insert(&None, "00000000000000000000", &old_ref)
                .unwrap(),
        );
        let reference = packed(temp.path(), &head);
        let page = read(&mut Store::new(temp.path()), &reference, &head).unwrap();
        assert_eq!(page.entries[0].reference, old_ref);
        assert!(page.entries[0].row.relation.is_none());
        let (legacy, next) =
            super::super::page_limited(&mut Store::new(temp.path()), "head", &head, None, 8)
                .unwrap();
        assert_eq!(legacy.len(), 1);
        assert!(next.is_none());
    }

    #[test]
    fn valid_pack_does_not_open_row_or_interior_files() {
        let temp = tempfile::tempdir().unwrap();
        let head = fixture(temp.path(), 100);
        let reference = packed(temp.path(), &head);
        for file in fs::read_dir(temp.path()).unwrap() {
            let file = file.unwrap();
            let name = file.file_name();
            if name != reference.as_str() && Some(name.to_str().unwrap()) != head.root.as_deref() {
                fs::remove_file(file.path()).unwrap();
            }
        }
        let mut reader = Store::new(temp.path());
        let actual = read(&mut reader, &reference, &head).unwrap();
        assert_eq!(actual.entries.len(), PAGE_ROWS);
        assert_eq!(
            reader.objects,
            PAGE_ROWS + 2,
            "root + pack + embedded headers"
        );
        let bytes = fs::metadata(temp.path().join(&reference)).unwrap().len()
            + fs::metadata(temp.path().join(head.root.as_ref().unwrap()))
                .unwrap()
                .len();
        assert_eq!(reader.bytes as u64, bytes);
        assert!(Store::new(temp.path())
            .page(&head.root, None, PAGE_ROWS + 1)
            .is_err());
        assert!(Store::new(temp.path())
            .read::<Row>(&actual.entries[0].reference)
            .is_err());
    }

    #[test]
    fn corruption_deletion_truncation_and_oversize_are_rejected_on_second_read() {
        for root in [false, true] {
            for damage in ["corrupt", "delete", "truncate", "oversize"] {
                let temp = tempfile::tempdir().unwrap();
                let head = fixture(temp.path(), 3);
                let reference = packed(temp.path(), &head);
                let mut reader = Store::new(temp.path());
                read(&mut reader, &reference, &head).unwrap();
                let target = temp.path().join(if root {
                    head.root.as_ref().unwrap()
                } else {
                    &reference
                });
                match damage {
                    "corrupt" => fs::write(&target, b"different bytes").unwrap(),
                    "delete" => fs::remove_file(&target).unwrap(),
                    "truncate" => fs::OpenOptions::new()
                        .write(true)
                        .open(&target)
                        .unwrap()
                        .set_len(4)
                        .unwrap(),
                    "oversize" => fs::write(&target, vec![b'x'; PACKED_BYTES + 1]).unwrap(),
                    _ => unreachable!(),
                }
                let spent = reader.objects;
                assert!(
                    read(&mut reader, &reference, &head).is_err(),
                    "{root}: {damage}"
                );
                assert!(reader.objects > spent);
            }
        }
    }

    #[test]
    fn authenticated_but_mismatched_bindings_order_counts_and_headers_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let head = fixture(temp.path(), 81);
        let reference = packed(temp.path(), &head);
        let changes: Vec<fn(&mut Value)> = vec![
            |p| p["binding"]["agent_id"] = json!("foreign"),
            |p| p["binding"]["conversation_id"] = json!("foreign"),
            |p| p["binding"]["generation"] = json!("stale"),
            |p| p["binding"]["root"] = json!("0".repeat(64)),
            |p| p["binding"]["row_count"] = json!(82),
            |p| p["binding"]["source_key"] = json!("foreign"),
            |p| p["binding"]["version"] = json!(2),
            |p| p["has_older"] = json!(false),
            |p| p["lookahead"] = Value::Null,
            |p| {
                p["entries"].as_array_mut().unwrap().pop();
            },
            |p| {
                let e = p["entries"][0].clone();
                p["entries"].as_array_mut().unwrap().push(e);
            },
            |p| p["entries"].as_array_mut().unwrap().swap(0, 1),
            |p| p["entries"][0]["reference"] = p["entries"][1]["reference"].clone(),
            |p| p["entries"][0]["key"] = json!("1"),
            |p| p["entries"][0]["header"] = json!("{}"),
            |p| p["lookahead"][0] = p["entries"][0]["key"].clone(),
        ];
        for edit in changes {
            let changed = edited_pack(temp.path(), &reference, edit);
            assert!(read(&mut Store::new(temp.path()), &changed, &head).is_err());
        }
        let foreign_row = edited_pack(temp.path(), &reference, |page| {
            let mut row: Value =
                serde_json::from_str(page["entries"][0]["header"].as_str().unwrap()).unwrap();
            row["agent_id"] = json!("foreign");
            let header = serde_json::to_string(&row).unwrap();
            page["entries"][0]["reference"] = json!(digest(header.as_bytes()));
            page["entries"][0]["header"] = json!(header);
        });
        assert!(read(&mut Store::new(temp.path()), &foreign_row, &head).is_err());
        // A second request must validate scope even after a successful read.
        let mut reader = Store::new(temp.path());
        read(&mut reader, &reference, &head).unwrap();
        let mut changed = head.clone();
        changed.generation = "new generation".into();
        assert!(read(&mut reader, &reference, &changed).is_err());
    }

    #[test]
    fn maximum_header_size_is_accepted_but_generic_limit_stays_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let mut head = fixture(temp.path(), 0);
        head.row_count = 1;
        let mut row = row(0);
        row.event.metadata["padding"] = json!("");
        let size = serde_json::to_vec(&row).unwrap().len();
        row.event.metadata["padding"] = json!("x".repeat(OBJECT_BYTES - size));
        let bytes = serde_json::to_vec(&row).unwrap();
        assert_eq!(bytes.len(), OBJECT_BYTES);
        let mut writer = Store::writer(temp.path());
        let row_ref = writer.put_bytes(&bytes).unwrap();
        head.root = Some(writer.insert(&None, &row.key, &row_ref).unwrap());
        let reference = packed(temp.path(), &head);
        let actual = read(&mut Store::new(temp.path()), &reference, &head).unwrap();
        assert_eq!(
            serde_json::to_vec(&actual.entries[0].row).unwrap().len(),
            OBJECT_BYTES
        );
        assert!(Store::new(temp.path()).read::<Value>(&reference).is_err());
        assert!(writer.put_bytes(&vec![0; OBJECT_BYTES + 1]).is_err());
        let oversized = edited_pack(temp.path(), &reference, |page| {
            let header = format!("{} ", page["entries"][0]["header"].as_str().unwrap());
            page["entries"][0]["reference"] = json!(digest(header.as_bytes()));
            page["entries"][0]["header"] = json!(header);
        });
        let mut reader = Store::new(temp.path());
        assert!(read(&mut reader, &oversized, &head).is_err());
        assert_eq!(
            reader.objects, 3,
            "failing embedded header attempt is retained"
        );
    }

    #[test]
    fn embedded_attempts_share_existing_budget_and_failures_keep_spent_work() {
        let temp = tempfile::tempdir().unwrap();
        let head = fixture(temp.path(), 8);
        let reference = packed(temp.path(), &head);
        let mut reader = Store::new(temp.path());
        reader.objects = super::super::super::chat_read_store::READ_OBJECTS - 5;
        let before = reader.objects;
        assert!(read(&mut reader, &reference, &head).is_err());
        assert_eq!(
            reader.objects,
            super::super::super::chat_read_store::READ_OBJECTS
        );
        assert!(reader.objects > before && reader.bytes > 0);
        assert!(
            reader.page(&head.root, None, 1).is_err(),
            "no reset on fallback"
        );
    }

    #[test]
    fn near_limit_head_omits_before_any_tree_work() {
        let temp = tempfile::tempdir().unwrap();
        let mut head = fixture(temp.path(), 1);
        head.capture_stamp.clear();
        let base = serde_json::to_vec(&head).unwrap().len();
        head.capture_stamp = "x".repeat(OBJECT_BYTES - base);
        assert_eq!(serde_json::to_vec(&head).unwrap().len(), OBJECT_BYTES);
        let mut writer = Store::checkpoint_writer(temp.path());
        writer.objects = 7;
        writer.bytes = 123;
        assert!(prepare(&mut writer, &head).unwrap().is_none());
        assert_eq!((writer.objects, writer.bytes), (7, 123));
        assert!(
            writer.put(&head).is_ok(),
            "original Head remains publishable"
        );
    }

    #[test]
    fn cleared_head_digest_binds_publication_state_without_a_hash_cycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut head = fixture(temp.path(), 2);
        let reference = packed(temp.path(), &head);
        head.recent_page = Some(reference.clone());
        read(&mut Store::new(temp.path()), &reference, &head).unwrap();
        let changes: Vec<fn(&mut Value)> = vec![
            |h| h["capture_stamp"] = json!("new capture"),
            |h| h["progress"] = json!("indexing"),
            |h| h["parent"] = json!("0".repeat(64)),
            |h| h["changes"] = json!(["0".repeat(64)]),
            |h| h["recent_start"] = json!(1),
            |h| h["committed_output_bytes"] = json!(123),
            |h| h["logical"]["narrative_watermark"] = json!(123),
        ];
        for edit in changes {
            let mut changed = serde_json::to_value(&head).unwrap();
            edit(&mut changed);
            let changed: Head = serde_json::from_value(changed).unwrap();
            assert!(read(&mut Store::new(temp.path()), &reference, &changed).is_err());
        }
    }

    #[test]
    fn oversized_serialization_is_omitted_and_charged_before_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let mut head = head();
        head.row_count = PAGE_ROWS;
        let mut source = Store::writer(temp.path());
        for index in 0..PAGE_ROWS {
            let mut row = row(index);
            row.event.metadata["padding"] = json!("x".repeat(4000));
            let reference = source.put(&row).unwrap();
            head.root = Some(source.insert(&head.root, &row.key, &reference).unwrap());
        }
        let mut writer = Store::checkpoint_writer(temp.path());
        assert!(prepare(&mut writer, &head).unwrap().is_none());
        let before = (writer.objects, writer.bytes);
        assert!(before.0 >= 2 * PAGE_ROWS && before.1 > PACKED_BYTES);
        let fallback = writer.page(&head.root, None, 1).unwrap();
        assert_eq!(fallback.len(), 1);
        assert!(writer.objects > before.0 && writer.bytes > before.1);
        // Serialization stopped at cap+1 and charged exactly that failed
        // attempt. Filling the unchanged 8 MiB allowance proves no reset.
        let remaining = 8 * super::super::super::chat_read_store::READ_BYTES - PACKED_BYTES - 1;
        let chunk = vec![b'x'; OBJECT_BYTES];
        for _ in 0..remaining / OBJECT_BYTES {
            writer.put_bytes(&chunk).unwrap();
        }
        writer
            .put_bytes(&chunk[..remaining % OBJECT_BYTES])
            .unwrap();
        assert!(writer.put_bytes(b"x").is_err());
    }

    #[test]
    fn exhausted_preflight_preserves_existing_source_work_without_opening_tree() {
        let temp = tempfile::tempdir().unwrap();
        let head = fixture(temp.path(), 1);
        let mut writer = Store::checkpoint_writer(temp.path());
        writer.objects = 8191;
        writer.bytes = 123;
        fs::remove_file(temp.path().join(head.root.as_ref().unwrap())).unwrap();
        assert!(prepare(&mut writer, &head).unwrap().is_none());
        assert_eq!((writer.objects, writer.bytes), (8191, 123));
    }

    #[test]
    fn pack_write_failure_preserves_previously_published_head() {
        let temp = tempfile::tempdir().unwrap();
        let head = fixture(temp.path(), 1);
        let reference = packed(temp.path(), &head);
        let old_head = Store::writer(temp.path()).put(&head).unwrap();
        let pointer = temp.path().join("head-pointer");
        fs::write(&pointer, &old_head).unwrap();
        fs::remove_file(temp.path().join(&reference)).unwrap();
        // An existing invalid directory is not a successfully persisted pack.
        fs::create_dir(temp.path().join(&reference)).unwrap();
        let mut writer = Store::checkpoint_writer(temp.path());
        let result = prepare(&mut writer, &head);
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(pointer).unwrap(), old_head);
        assert!(writer.objects > 0 && writer.bytes > 0);
    }
}
