//! Content-addressed, persistent AVL indexes. Published roots never change.
//! Readers spend one shared byte/node budget, including failed reads.
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::HashMap;

pub(super) const READ_BYTES: usize = 1024 * 1024;
pub(super) const READ_OBJECTS: usize = 512;

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn valid_ref(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A publication-local cache of already hash-validated immutable seek nodes.
/// Normal readers retain their existing per-read accounting and hard limits.
#[derive(Default)]
pub(super) struct SeekCache {
    nodes: HashMap<String, Node>,
    bytes: usize,
}

pub(super) struct Store {
    pub(super) dir: PathBuf,
    pub(super) bytes: usize,
    pub(super) objects: usize,
    byte_limit: usize,
    object_limit: usize,
    bounded_writes: bool,
    write_objects: Cell<usize>,
    write_bytes: Cell<usize>,
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
        let mut file = fs::File::open(self.dir.join(reference))?;
        let len = usize::try_from(file.metadata()?.len()).map_err(io::Error::other)?;
        if len > limit || len > self.byte_limit.saturating_sub(self.bytes) {
            return Err(io::Error::other("chat read byte budget exhausted"));
        }
        // Read the pinned extent only. Growth cannot extend the request budget.
        let mut bytes = Vec::with_capacity(len);
        (&mut file).take(len as u64).read_to_end(&mut bytes)?;
        self.bytes += bytes.len();
        if bytes.len() != len || digest(&bytes) != reference {
            return Err(io::Error::other("chat read object changed"));
        }
        Ok(bytes)
    }

    pub(super) fn read<T: DeserializeOwned>(&mut self, reference: &str) -> io::Result<T> {
        serde_json::from_slice(&self.read_bytes(reference, 16 * 1024)?).map_err(io::Error::other)
    }

    pub(super) fn put_bytes(&self, bytes: &[u8]) -> io::Result<String> {
        if bytes.len() > 16 * 1024 {
            return Err(io::Error::other("oversized chat index object"));
        }
        if self.bounded_writes {
            if self.write_objects.get() >= self.object_limit
                || bytes.len() > self.byte_limit.saturating_sub(self.write_bytes.get())
            {
                return Err(io::Error::other("chat projection write budget exhausted"));
            }
            self.write_objects.set(self.write_objects.get() + 1);
            self.write_bytes.set(self.write_bytes.get() + bytes.len());
        }
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
        self.put(&node)
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
