//! Content-addressed, persistent AVL indexes. Published roots never change.
//! Readers spend one shared byte/node budget, including failed reads.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use super::chat_read::packed_recent::{PackedPage, NODE_BYTES, PACKED_BYTES, TREE_HEIGHT};
use super::chat_read::PAGE_ROWS;

pub(super) const READ_BYTES: usize = 1024 * 1024;
pub(super) const READ_OBJECTS: usize = 512;
pub(super) const OBJECT_BYTES: usize = 16 * 1024;

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn valid_ref(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// An operation-local cache of already hash-validated immutable seek nodes.
/// Each Store retains its existing hard byte and object limits.
#[derive(Default)]
pub(super) struct SeekCache {
    nodes: HashMap<String, Node>,
    bytes: usize,
}

/// Only schema-defined links participate in checkpoint reachability. Object
/// payloads and index keys can contain user text that happens to look like a hash.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum Dependency {
    Node(String),
    Object(String),
}

impl Dependency {
    fn reference(&self) -> &str {
        match self {
            Self::Node(reference) | Self::Object(reference) => reference,
        }
    }
}

struct PendingNode {
    bytes: Vec<u8>,
    order: usize,
}

struct PendingNodes {
    nodes: HashMap<String, PendingNode>,
    links: HashMap<String, Vec<Dependency>>,
    ram_bytes: usize,
    ram_limit: usize,
    object_limit: usize,
}

impl PendingNodes {
    fn new(ram_limit: usize, object_limit: usize) -> Self {
        Self {
            nodes: HashMap::new(),
            links: HashMap::new(),
            ram_bytes: 0,
            ram_limit,
            object_limit,
        }
    }

    fn reserve(&mut self, bytes: usize) -> io::Result<()> {
        let next = self
            .ram_bytes
            .checked_add(bytes)
            .ok_or_else(|| io::Error::other("chat pending node RAM budget exhausted"))?;
        if next > self.ram_limit || self.nodes.len() + self.links.len() >= self.object_limit {
            return Err(io::Error::other("chat pending node RAM budget exhausted"));
        }
        self.ram_bytes = next;
        Ok(())
    }
}

// Reserve traversal/maps as well as actual owned byte/string capacities. This
// covers the bounded child/value frontier without a second unbounded allocation.
const PENDING_ENTRY_RAM: usize = 1536;

pub(super) struct Store {
    pub(super) dir: PathBuf,
    pub(super) bytes: usize,
    pub(super) objects: usize,
    byte_limit: usize,
    object_limit: usize,
    bounded_writes: bool,
    write_objects: Cell<usize>,
    write_bytes: Cell<usize>,
    pending: Option<PendingNodes>,
}

impl Store {
    pub(super) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            bytes: 0,
            objects: 0,
            byte_limit: READ_BYTES,
            object_limit: READ_OBJECTS,
            bounded_writes: false,
            write_objects: Cell::new(0),
            write_bytes: Cell::new(0),
            pending: None,
        }
    }

    pub(super) fn writer(dir: &Path) -> Self {
        Self {
            byte_limit: 8 * READ_BYTES,
            object_limit: 8192,
            ..Self::new(dir)
        }
    }

    /// Cold admission budgets attempted writes as well as immutable reads.
    pub(super) fn bounded_writer(dir: &Path) -> Self {
        Self {
            bounded_writes: true,
            ..Self::writer(dir)
        }
    }

    /// Checkpoint publications stage transient AVL versions privately. Direct row
    /// and body objects retain immediate durable writes; published roots remain
    /// immutable, and other writers keep their existing persistence semantics.
    pub(super) fn checkpoint_writer(dir: &Path) -> Self {
        let mut store = Self::bounded_writer(dir);
        let ram_limit = 2 * store.byte_limit + PENDING_ENTRY_RAM * store.object_limit;
        store.pending = Some(PendingNodes::new(ram_limit, store.object_limit));
        store
    }

    pub(super) fn checkpoint_due(&self) -> bool {
        self.objects > 5000
            || self.write_objects.get() > 5000
            || self.bytes > 6 * READ_BYTES
            || self.write_bytes.get() > 6 * READ_BYTES
    }

    pub(super) fn read_bytes(&mut self, reference: &str, limit: usize) -> io::Result<Vec<u8>> {
        if !valid_ref(reference) || self.objects >= self.object_limit {
            return Err(io::Error::other("chat read reference/budget invalid"));
        }
        self.objects += 1;
        if let Some(node) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.nodes.get(reference))
        {
            let len = node.bytes.len();
            if len > limit || len > self.byte_limit.saturating_sub(self.bytes) {
                return Err(io::Error::other("chat read byte budget exhausted"));
            }
            let bytes = node.bytes.clone();
            self.bytes += bytes.len();
            if digest(&bytes) != reference {
                return Err(io::Error::other("chat read object changed"));
            }
            return Ok(bytes);
        }
        let mut file = fs::File::open(self.dir.join(reference))?;
        let len = usize::try_from(file.metadata()?.len()).map_err(io::Error::other)?;
        if len > limit || len > self.byte_limit.saturating_sub(self.bytes) {
            return Err(io::Error::other("chat read byte budget exhausted"));
        }
        // Read the pinned extent only. Growth cannot extend the request budget.
        let mut bytes = Vec::with_capacity(len);
        let result = (&mut file).take(len as u64).read_to_end(&mut bytes);
        self.bytes += bytes.len();
        result?;
        if bytes.len() != len || digest(&bytes) != reference {
            return Err(io::Error::other("chat read object changed"));
        }
        Ok(bytes)
    }

    pub(super) fn read<T: DeserializeOwned>(&mut self, reference: &str) -> io::Result<T> {
        serde_json::from_slice(&self.read_bytes(reference, OBJECT_BYTES)?).map_err(io::Error::other)
    }

    fn account_write(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > OBJECT_BYTES {
            return Err(io::Error::other("oversized chat index object"));
        }
        self.account_write_extent(bytes.len())
    }

    // Shared attempt accounting is private. Only the typed packed-page path can
    // spend a larger extent; ordinary object methods retain OBJECT_BYTES.
    fn account_write_extent(&self, len: usize) -> io::Result<()> {
        if self.bounded_writes {
            if self.write_objects.get() >= self.object_limit
                || len > self.byte_limit.saturating_sub(self.write_bytes.get())
            {
                return Err(io::Error::other("chat projection write budget exhausted"));
            }
            self.write_objects.set(self.write_objects.get() + 1);
            self.write_bytes.set(self.write_bytes.get() + len);
        }
        Ok(())
    }

    pub(super) fn put_bytes(&self, bytes: &[u8]) -> io::Result<String> {
        self.account_write(bytes)?;
        self.persist_bytes(bytes)
    }

    fn persist_bytes(&self, bytes: &[u8]) -> io::Result<String> {
        fs::create_dir_all(&self.dir)?;
        let reference = digest(bytes);
        let target = self.dir.join(&reference);
        if target.exists() {
            return Ok(reference);
        }
        let staging = self.dir.join(format!("{}.pending", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(staging, target)?;
        Ok(reference)
    }

    pub(super) fn put<T: Serialize>(&self, value: &T) -> io::Result<String> {
        self.put_bytes(&serde_json::to_vec(value).map_err(io::Error::other)?)
    }

    /// Register explicit child roots for a durable payload that can still refer
    /// to private nodes. Neither serialized metadata nor logical IDs are scanned.
    pub(super) fn put_linked<T: Serialize>(
        &mut self,
        value: &T,
        dependencies: Vec<Dependency>,
    ) -> io::Result<String> {
        let reference = self.put(value)?;
        self.register_links(&reference, dependencies)?;
        Ok(reference)
    }

    fn register_links(&mut self, reference: &str, dependencies: Vec<Dependency>) -> io::Result<()> {
        let Some(pending) = self.pending.as_mut() else {
            return Ok(());
        };
        if dependencies.is_empty() {
            return Ok(());
        }
        if dependencies.iter().any(|link| !valid_ref(link.reference())) {
            return Err(io::Error::other("invalid chat checkpoint dependency"));
        }
        if let Some(previous) = pending.links.get(reference) {
            if previous != &dependencies {
                return Err(io::Error::other(
                    "chat checkpoint payload dependencies changed",
                ));
            }
            return Ok(());
        }
        let ram = PENDING_ENTRY_RAM
            + reference.len()
            + dependencies.capacity() * std::mem::size_of::<Dependency>()
            + dependencies
                .iter()
                .map(|link| match link {
                    Dependency::Node(value) | Dependency::Object(value) => value.capacity(),
                })
                .sum::<usize>();
        pending.reserve(ram)?;
        pending.links.insert(reference.to_owned(), dependencies);
        Ok(())
    }

    /// Reserve the worst valid recent traversal, bounded encoding attempt, typed
    /// links, and one ordinary Head write without changing any Store allowance.
    pub(super) fn can_prepare_packed_recent(&self) -> bool {
        let nodes = PAGE_ROWS + 1 + TREE_HEIGHT;
        let dependencies = 1 + PAGE_ROWS + 1 + PAGE_ROWS;
        let link_ram =
            PENDING_ENTRY_RAM + 64 + dependencies * (std::mem::size_of::<Dependency>() + 64);
        nodes + PAGE_ROWS < self.object_limit.saturating_sub(self.objects)
            && nodes * NODE_BYTES + PAGE_ROWS * OBJECT_BYTES + PACKED_BYTES
                <= self.byte_limit.saturating_sub(self.bytes)
            && (!self.bounded_writes
                || (2 <= self.object_limit.saturating_sub(self.write_objects.get())
                    && PACKED_BYTES + 1 + OBJECT_BYTES
                        <= self.byte_limit.saturating_sub(self.write_bytes.get())))
            && self.pending.as_ref().is_none_or(|pending| {
                pending.nodes.len() + pending.links.len() < pending.object_limit
                    && link_ram <= pending.ram_limit.saturating_sub(pending.ram_bytes)
            })
    }

    /// The only larger-object writer accepts an authenticated recent-page type.
    /// Even an oversized serialization spends its attempted object and bounded
    /// bytes. I/O, dependency, and budget errors propagate to the publisher.
    pub(super) fn put_packed_recent(&mut self, page: &PackedPage) -> io::Result<Option<String>> {
        let (bytes, oversized) = page.encode()?;
        self.account_write_extent(bytes.len())?;
        if oversized {
            return Ok(None);
        }
        let reference = self.persist_bytes(&bytes)?;
        // persist_bytes may find an existing target. Validate that target too,
        // and retain these read attempts if an orphan was damaged or replaced.
        if self.read_bytes(&reference, PACKED_BYTES)? != bytes {
            return Err(io::Error::other("chat packed page is not durable"));
        }
        self.register_links(&reference, page.dependencies())?;
        Ok(Some(reference))
    }

    /// Packed bytes are hash-checked afresh; embedded header decodes spend the
    /// same object budget as original Row reads. Their bytes are already charged
    /// by the enclosing packed file and are not charged a second time.
    pub(super) fn read_packed_recent(&mut self, reference: &str) -> io::Result<PackedPage> {
        let bytes = self.read_bytes(reference, PACKED_BYTES)?;
        PackedPage::decode(self, &bytes)
    }

    pub(super) fn account_packed_header(&mut self) -> io::Result<()> {
        if self.objects >= self.object_limit {
            return Err(io::Error::other("chat packed header budget exhausted"));
        }
        self.objects += 1;
        Ok(())
    }

    fn packed_node(&mut self, reference: &str) -> io::Result<Node> {
        let node: Node = serde_json::from_slice(&self.read_bytes(reference, NODE_BYTES)?)
            .map_err(io::Error::other)?;
        if node.key.len() != 20
            || !node.key.bytes().all(|byte| byte.is_ascii_digit())
            || !valid_ref(&node.value)
            || node.height == 0
            || node.height as usize > TREE_HEIGHT
            || node
                .left
                .iter()
                .chain(&node.right)
                .any(|child| !valid_ref(child))
        {
            return Err(io::Error::other("invalid packed chat canonical node"));
        }
        Ok(node)
    }

    /// Reopen the canonical root on every packed request, even when this Store
    /// previously served the same pack. No interior-node/Row cache is involved.
    pub(super) fn validate_packed_root(&mut self, root: &Option<String>) -> io::Result<()> {
        if let Some(reference) = root {
            self.packed_node(reference)?;
        }
        Ok(())
    }

    /// Equivalent to page(root, None, PAGE_ROWS + 1) for canonical AVL trees.
    /// Canonical keys bound a valid tree's height, and each visited node has a
    /// small fixed representation. Enforcing both bounds makes preflight sound.
    pub(super) fn packed_recent_entries(
        &mut self,
        root: &Option<String>,
    ) -> io::Result<Vec<(String, String)>> {
        let mut current = root.clone();
        let mut stack = Vec::new();
        let mut out = Vec::new();
        let mut visited = 0;
        while current.is_some() || !stack.is_empty() {
            while let Some(reference) = current.take() {
                if visited >= PAGE_ROWS + 1 + TREE_HEIGHT || stack.len() >= TREE_HEIGHT {
                    return Err(io::Error::other("packed chat canonical traversal exceeded"));
                }
                let node = self.packed_node(&reference)?;
                current = node.right.clone();
                stack.push(node);
                visited += 1;
            }
            let node = stack.pop().expect("a traversed canonical node is queued");
            current = node.left;
            out.push((node.key, node.value));
            if out.len() == PAGE_ROWS + 1 {
                break;
            }
        }
        Ok(out)
    }

    /// Durably flush only private nodes reachable through typed final roots.
    /// Already durable references terminate the walk. All final dependencies
    /// finish write/sync/rename before the caller can publish its head pointer.
    /// Errors leave that pointer and caller state unchanged; dropping the store
    /// discards every remaining transient node, while durable orphans are safe
    /// for an idempotent retry.
    pub(super) fn finish_checkpoint(&mut self, roots: &[Dependency]) -> io::Result<()> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(());
        };
        let mut frontier = roots.to_vec();
        let mut seen = HashSet::new();
        let mut nodes = Vec::new();
        while let Some(link) = frontier.pop() {
            if !valid_ref(link.reference()) {
                return Err(io::Error::other("invalid chat checkpoint dependency"));
            }
            if !seen.insert(link.clone()) {
                continue;
            }
            if seen.len() > 3 * self.object_limit + roots.len() {
                return Err(io::Error::other(
                    "chat checkpoint dependency budget exhausted",
                ));
            }
            match &link {
                Dependency::Node(reference) => {
                    if let Some(saved) = pending.nodes.get(reference) {
                        let node: Node =
                            serde_json::from_slice(&saved.bytes).map_err(io::Error::other)?;
                        frontier.extend(node.left.into_iter().map(Dependency::Node));
                        frontier.extend(node.right.into_iter().map(Dependency::Node));
                        frontier.push(Dependency::Object(node.value));
                        nodes.push((saved.order, reference.clone()));
                        continue;
                    }
                }
                Dependency::Object(reference) => {
                    if let Some(links) = pending.links.get(reference) {
                        frontier.extend(links.iter().cloned());
                    }
                }
            }
            if !fs::metadata(self.dir.join(link.reference()))?.is_file() {
                return Err(io::Error::other(
                    "chat checkpoint dependency is not durable",
                ));
            }
        }
        // Children are created before their parent versions. Keep that order
        // while omitting unreachable versions produced by subsequent inserts.
        nodes.sort_unstable_by_key(|(order, _)| *order);
        for (_, reference) in nodes {
            let saved = &pending.nodes[&reference];
            if digest(&saved.bytes) != reference {
                return Err(io::Error::other("chat pending node changed"));
            }
            match fs::metadata(self.dir.join(&reference)) {
                Ok(metadata) => {
                    if !metadata.is_file() || metadata.len() != saved.bytes.len() as u64 {
                        return Err(io::Error::other("chat checkpoint node is not durable"));
                    }
                    let mut bytes = Vec::with_capacity(saved.bytes.len());
                    File::open(self.dir.join(&reference))?
                        .take(saved.bytes.len() as u64 + 1)
                        .read_to_end(&mut bytes)?;
                    if bytes != saved.bytes {
                        return Err(io::Error::other("chat checkpoint node changed"));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            // Attempt accounting happened when this version was created;
            // flushing it must neither recharge nor enlarge the write budget.
            self.persist_bytes(&saved.bytes)?;
        }
        let pending = self.pending.as_mut().expect("checkpoint writer retained");
        pending.nodes.clear();
        pending.links.clear();
        pending.ram_bytes = 0;
        Ok(())
    }

    fn node(&mut self, root: &Option<String>) -> io::Result<Option<Node>> {
        root.as_deref()
            .map(|reference| self.read(reference))
            .transpose()
    }

    fn height(&mut self, root: &Option<String>) -> io::Result<u32> {
        Ok(self.node(root)?.map_or(0, |node| node.height))
    }

    fn save(&mut self, mut node: Node) -> io::Result<String> {
        node.height = 1 + self.height(&node.left)?.max(self.height(&node.right)?);
        if self.pending.is_none() {
            return self.put(&node);
        }
        let bytes = serde_json::to_vec(&node).map_err(io::Error::other)?;
        self.account_write(&bytes)?;
        let reference = digest(&bytes);
        let pending = self.pending.as_mut().expect("checkpoint writer retained");
        // A newly durable body blob may contain exactly these node bytes.
        // File existence cannot prove that its typed children are durable yet.
        if !pending.nodes.contains_key(&reference) {
            pending.reserve(PENDING_ENTRY_RAM + reference.capacity() + bytes.capacity())?;
            let order = pending.nodes.len();
            pending
                .nodes
                .insert(reference.clone(), PendingNode { bytes, order });
        }
        Ok(reference)
    }

    fn rotate_left(&mut self, mut node: Node) -> io::Result<String> {
        let mut right = self
            .node(&node.right)?
            .ok_or_else(|| io::Error::other("missing right node"))?;
        node.right = right.left.take();
        right.left = Some(self.save(node)?);
        self.save(right)
    }

    fn rotate_right(&mut self, mut node: Node) -> io::Result<String> {
        let mut left = self
            .node(&node.left)?
            .ok_or_else(|| io::Error::other("missing left node"))?;
        node.left = left.right.take();
        left.right = Some(self.save(node)?);
        self.save(left)
    }

    pub(super) fn insert(
        &mut self,
        root: &Option<String>,
        key: &str,
        value: &str,
    ) -> io::Result<String> {
        if key.len() > 512 || !valid_ref(value) {
            return Err(io::Error::other("invalid chat index entry"));
        }
        let Some(mut node) = self.node(root)? else {
            return self.save(Node {
                key: key.into(),
                value: value.into(),
                left: None,
                right: None,
                height: 1,
            });
        };
        match key.cmp(&node.key) {
            std::cmp::Ordering::Less => node.left = Some(self.insert(&node.left, key, value)?),
            std::cmp::Ordering::Greater => {
                node.right = Some(self.insert(&node.right, key, value)?)
            }
            std::cmp::Ordering::Equal => {
                node.value = value.into();
                return self.save(node);
            }
        }
        let left = self.height(&node.left)?;
        let right = self.height(&node.right)?;
        if left > right + 1 {
            let child = self
                .node(&node.left)?
                .ok_or_else(|| io::Error::other("missing left node"))?;
            if self.height(&child.right)? > self.height(&child.left)? {
                node.left = Some(self.rotate_left(child)?);
            }
            return self.rotate_right(node);
        }
        if right > left + 1 {
            let child = self
                .node(&node.right)?
                .ok_or_else(|| io::Error::other("missing right node"))?;
            if self.height(&child.left)? > self.height(&child.right)? {
                node.right = Some(self.rotate_right(child)?);
            }
            return self.rotate_left(node);
        }
        self.save(node)
    }

    pub(super) fn get(&mut self, root: &Option<String>, key: &str) -> io::Result<Option<String>> {
        let mut root = root.clone();
        while let Some(node) = self.node(&root)? {
            match key.cmp(&node.key) {
                std::cmp::Ordering::Equal => return Ok(Some(node.value)),
                std::cmp::Ordering::Less => root = node.left,
                std::cmp::Ordering::Greater => root = node.right,
            }
        }
        Ok(None)
    }

    pub(super) fn get_reusing_nodes(
        &mut self,
        root: &Option<String>,
        key: &str,
        cache: &mut SeekCache,
    ) -> io::Result<Option<String>> {
        let mut root = root.clone();
        while let Some(reference) = root {
            let node = if let Some(node) = cache.nodes.get(&reference) {
                node.clone()
            } else {
                let bytes_before = self.bytes;
                let node: Node = self.read(&reference)?;
                let bytes = self.bytes - bytes_before;
                if cache.nodes.len() < READ_OBJECTS
                    && bytes <= READ_BYTES.saturating_sub(cache.bytes)
                {
                    cache.bytes += bytes;
                    cache.nodes.insert(reference, node.clone());
                }
                node
            };
            match key.cmp(&node.key) {
                std::cmp::Ordering::Equal => return Ok(Some(node.value)),
                std::cmp::Ordering::Less => root = node.left,
                std::cmp::Ordering::Greater => root = node.right,
            }
        }
        Ok(None)
    }

    /// Descending range walk visits only the seek path and requested rows.
    pub(super) fn page(
        &mut self,
        root: &Option<String>,
        before: Option<&str>,
        limit: usize,
    ) -> io::Result<Vec<(String, String)>> {
        fn walk(
            store: &mut Store,
            root: &Option<String>,
            before: Option<&str>,
            limit: usize,
            out: &mut Vec<(String, String)>,
        ) -> io::Result<()> {
            if out.len() >= limit {
                return Ok(());
            }
            let Some(node) = store.node(root)? else {
                return Ok(());
            };
            if before.is_none_or(|key| node.key.as_str() < key) {
                walk(store, &node.right, before, limit, out)?;
                if out.len() < limit {
                    out.push((node.key.clone(), node.value));
                }
            }
            walk(store, &node.left, before, limit, out)
        }
        let mut out = Vec::new();
        walk(self, root, before, limit, &mut out)?;
        Ok(out)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Node {
    key: String,
    value: String,
    left: Option<String>,
    right: Option<String>,
    height: u32,
}

#[cfg(test)]
#[path = "chat_read_store_tests.rs"]
mod tests;

#[cfg(test)]
mod packed_budget_tests {
    use super::*;

    #[test]
    fn packed_preflight_respects_existing_read_write_and_pending_caps() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::checkpoint_writer(temp.path());
        assert!(store.can_prepare_packed_recent());
        let byte_limit = store.byte_limit;
        let object_limit = store.object_limit;
        store.bytes = byte_limit - PAGE_ROWS * OBJECT_BYTES;
        assert!(!store.can_prepare_packed_recent());
        assert_eq!(store.bytes, byte_limit - PAGE_ROWS * OBJECT_BYTES);
        store.bytes = 0;
        store.objects = object_limit - PAGE_ROWS;
        assert!(!store.can_prepare_packed_recent());
        store.objects = 0;
        store.write_bytes.set(byte_limit - PACKED_BYTES);
        assert!(!store.can_prepare_packed_recent());
        assert_eq!(store.write_bytes.get(), byte_limit - PACKED_BYTES);
        store.write_bytes.set(0);
        store.write_objects.set(object_limit - 1);
        assert!(
            !store.can_prepare_packed_recent(),
            "reserve the eventual Head too"
        );
        store.write_objects.set(0);
        let pending = store.pending.as_mut().unwrap();
        let ram_limit = pending.ram_limit;
        pending.ram_bytes = ram_limit - 1;
        assert!(!store.can_prepare_packed_recent());
        assert_eq!(store.pending.as_ref().unwrap().ram_limit, ram_limit);
        assert_eq!(
            (store.byte_limit, store.object_limit),
            (byte_limit, object_limit)
        );
    }

    #[test]
    fn failed_persistence_keeps_actual_write_attempts() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, b"block").unwrap();
        let store = Store::checkpoint_writer(&blocked);
        assert!(store.put_bytes(b"payload").is_err());
        assert_eq!((store.write_objects.get(), store.write_bytes.get()), (1, 7));
    }
}
