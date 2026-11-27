use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use wardian_core::models::chat::AgentChatEvent;

use super::copy::{Fault, Reader};
use super::{component, digest, digest_ref};
use crate::state::conversation_archive::chat_read::Head;

pub(super) const OBJECT_BYTES: usize = 16 * 1024;
const PACK_BYTES: usize = 256 * 1024;
const PAGE_ROWS: usize = 80;
const TREE_HEIGHT: usize = 96;

#[derive(Deserialize)]
pub(super) struct HeadLinks {
    pub(super) parent: Option<String>,
    pub(super) recent_page: Option<String>,
    pub(super) row_count: usize,
    pub(super) bootstrap: Option<serde::de::IgnoredAny>,
    changes: Vec<String>,
}

pub(super) struct Published {
    pub(super) head: Head,
    pub(super) links: HeadLinks,
}

impl Published {
    pub(super) fn decode(bytes: &[u8], agent: &str) -> Result<Self, Fault> {
        let head: Head = serde_json::from_slice(bytes).map_err(|_| Fault::Invalid)?;
        let links: HeadLinks = serde_json::from_slice(bytes).map_err(|_| Fault::Invalid)?;
        if head.agent_id != agent
            || !component(&head.conversation_id)
            || head.generation.is_empty()
            || !matches!(head.progress.as_str(), "ready" | "indexing")
            || links.changes.len() > PAGE_ROWS
            || links
                .changes
                .iter()
                .chain(links.parent.iter())
                .chain(links.recent_page.iter())
                .chain(head.root.iter())
                .chain(head.identities.iter())
                .any(|r| !digest_ref(r))
        {
            return Err(Fault::Invalid);
        }
        Ok(Self { head, links })
    }

    fn legacy_binding(&self) -> Result<String, Fault> {
        self.head.legacy_pack_binding().map_err(|_| Fault::Invalid)
    }
}

pub(super) fn object(
    reader: &mut Reader,
    directory: &Path,
    reference: &str,
    limit: usize,
) -> Result<Vec<u8>, Fault> {
    if !digest_ref(reference) {
        return Err(Fault::Invalid);
    }
    let bytes = reader.file(&directory.join(reference), limit)?;
    if digest(&bytes) != reference {
        return Err(Fault::Invalid);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    key: String,
    value: String,
    left: Option<String>,
    right: Option<String>,
    height: usize,
}

pub(super) fn tree(
    reader: &mut Reader,
    directory: &Path,
    root: Option<&str>,
) -> Result<BTreeMap<String, String>, Fault> {
    struct Walk<'a> {
        reader: &'a mut Reader,
        directory: &'a Path,
        seen: HashSet<String>,
        entries: BTreeMap<String, String>,
        last: Option<String>,
    }
    impl Walk<'_> {
        fn visit(&mut self, reference: Option<&str>, depth: usize) -> Result<usize, Fault> {
            let Some(reference) = reference else {
                return Ok(0);
            };
            if depth >= TREE_HEIGHT || !self.seen.insert(reference.to_owned()) {
                return Err(Fault::Invalid);
            }
            let bytes = object(self.reader, self.directory, reference, OBJECT_BYTES)?;
            let node: Node = serde_json::from_slice(&bytes).map_err(|_| Fault::Invalid)?;
            let left = self.visit(node.left.as_deref(), depth + 1)?;
            if node.key.is_empty()
                || !digest_ref(&node.value)
                || self.last.as_ref().is_some_and(|last| last >= &node.key)
            {
                return Err(Fault::Invalid);
            }
            self.last = Some(node.key.clone());
            self.entries.insert(node.key, node.value);
            let right = self.visit(node.right.as_deref(), depth + 1)?;
            if node.height != 1 + left.max(right) || left.abs_diff(right) > 1 {
                return Err(Fault::Invalid);
            }
            Ok(node.height)
        }
    }
    let mut walk = Walk {
        reader,
        directory,
        seen: HashSet::new(),
        entries: BTreeMap::new(),
        last: None,
    };
    walk.visit(root, 0)?;
    Ok(walk.entries)
}

pub(super) fn numeric_key(key: &str) -> Result<u64, Fault> {
    if key.len() != 20 || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Fault::Invalid);
    }
    key.parse().map_err(|_| Fault::Invalid)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Row {
    agent_id: String,
    conversation_id: String,
    key: String,
    pub(super) event: AgentChatEvent,
    pub(super) body: Option<Body>,
    #[serde(default, rename = "turn_closure_binding")]
    _turn_closure_binding: Option<String>,
    #[serde(default)]
    pub(super) relation: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) removed_display_ids: Vec<String>,
}

impl Row {
    pub(super) fn decode(bytes: &[u8], head: &Head, key: &str) -> Result<Self, Fault> {
        let row: Self = serde_json::from_slice(bytes).map_err(|_| Fault::Invalid)?;
        if row.agent_id != head.agent_id
            || row.conversation_id != head.conversation_id
            || row.event.session_id != head.agent_id
            || row.event.id.is_empty()
            || row.key != key
            || numeric_key(key)?.checked_add(1) != row.event.sequence
        {
            return Err(Fault::Invalid);
        }
        Ok(row)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Body {
    binding: String,
    root: Option<String>,
    pub(super) bytes: u64,
    pub(super) complete: bool,
}

#[derive(PartialEq, Eq)]
pub(super) struct Payload {
    pub(super) bytes: u64,
    pub(super) digest: String,
}

impl Body {
    pub(super) fn payload(&self, reader: &mut Reader, directory: &Path) -> Result<Payload, Fault> {
        if !digest_ref(&self.binding) {
            return Err(Fault::Invalid);
        }
        let chunks = tree(reader, directory, self.root.as_deref())?;
        let mut extent = 0_u64;
        let mut hash = Sha256::new();
        for (offset, reference) in chunks {
            if numeric_key(&offset)? != extent {
                return Err(Fault::Invalid);
            }
            let bytes = object(reader, directory, &reference, OBJECT_BYTES)?;
            if std::str::from_utf8(&bytes).is_err() || (bytes.is_empty() && self.bytes != 0) {
                return Err(Fault::Invalid);
            }
            extent = extent
                .checked_add(bytes.len() as u64)
                .ok_or(Fault::Invalid)?;
            hash.update(&bytes);
        }
        if extent != self.bytes {
            return Err(Fault::Invalid);
        }
        Ok(Payload {
            bytes: extent,
            digest: format!("{:x}", hash.finalize()),
        })
    }
}

#[derive(Deserialize)]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedEntry {
    key: String,
    reference: String,
    header: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pack {
    binding: Binding,
    entries: Vec<PackedEntry>,
    has_older: bool,
    lookahead: Option<(String, String)>,
    #[serde(default, rename = "presentation")]
    _presentation: Option<serde::de::IgnoredAny>,
}

/// Recover protected rows only from an authenticated v1 pack with the exact
/// immutable Head binding and authenticated row-window membership.
pub(super) fn legacy_pack(
    reader: &mut Reader,
    directory: &Path,
    reference: &str,
    published: &Published,
    positions: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Row>, Fault> {
    let bytes = object(reader, directory, reference, PACK_BYTES)?;
    let pack: Pack = serde_json::from_slice(&bytes).map_err(|_| Fault::Invalid)?;
    let binding = pack.binding;
    let head = &published.head;
    if binding.version != 1
        || binding.head_binding != published.legacy_binding()?
        || binding.agent_id != head.agent_id
        || binding.conversation_id != head.conversation_id
        || binding.generation != head.generation
        || binding.source_key != head.source_key
        || binding.source_epoch != head.source_epoch
        || binding.root != head.root
        || binding.row_count != published.links.row_count
        || pack.entries.len() != positions.len().min(PAGE_ROWS)
        || pack.has_older != (positions.len() > PAGE_ROWS)
        || pack.lookahead.as_ref().map(|(key, row)| (key, row))
            != positions.iter().rev().nth(PAGE_ROWS)
    {
        return Err(Fault::Invalid);
    }
    let mut rows = BTreeMap::new();
    for (entry, (key, reference)) in pack.entries.into_iter().zip(positions.iter().rev()) {
        reader.attempt()?;
        if &entry.key != key
            || &entry.reference != reference
            || entry.header.len() > OBJECT_BYTES
            || digest(entry.header.as_bytes()) != *reference
        {
            return Err(Fault::Invalid);
        }
        rows.insert(
            key.clone(),
            Row::decode(entry.header.as_bytes(), head, key)?,
        );
    }
    Ok(rows)
}
