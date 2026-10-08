//! One process-owned Codex child observer, separate from bulk usage ingest.
//! Filesystem hints never confer ownership. Canonical roots and the complete
//! recorded parent chain are checked before a staged lifecycle is published.

use super::codex_lifecycle::{
    CurrentNativePresence, LifecycleActivityBaseline, LifecycleObserver, LifecyclePendingReason,
    LifecyclePublication, LifecycleReadBudget, LifecycleSourceIdentity, LifecycleStage,
    ValidatedCodexSource, CODEX_LIFECYCLE_BYTES_PER_PASS,
};
use super::worker_events::{CodexWorkerEventWatcher, WorkerEventBatch};
use super::{known_session_ids, resolve_shared_codex_home, AgentDescriptor, CodexRolloutMeta};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{File, ReadDir};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tauri::{Emitter, Manager};
use wardian_core::temporary_workers::{
    self, AutomationWorkerOrigin, CodexChildObservationQualification, ObserveCodexProviderChild,
    RegisterProviderChild, TemporaryWorkerRecord, TemporaryWorkerState,
};

const RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(300);
const MAX_SOURCES: usize = 4096;
const DIRECTORY_ENTRIES_PER_PASS: usize = 128;
const MAX_ANCESTRY: usize = 64;
const HEADER_BYTES_PER_PASS: u64 = 96 * 1024;
const SOURCE_BYTES_PER_PASS: u64 = 16 * 1024;
const LIFECYCLE_BYTES_PER_SOURCE: u64 = 64 * 1024;
const MAX_PARTIAL_HEADERS: usize = 4 * 1024 * 1024;
static STARTED: AtomicBool = AtomicBool::new(false);
type OwnerGates = HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>;

#[derive(Clone, Debug)]
struct Owner {
    root_agent_id: Option<String>,
    root_worker_id: Option<String>,
    runtime_session_id: String,
    workspace: String,
    sessions: BTreeSet<String>,
    origin: Option<AutomationWorkerOrigin>,
    live_session: Option<String>,
    epoch: RuntimeEpoch,
    automation_root: Option<TemporaryWorkerRecord>,
}

#[derive(Clone, Default)]
struct RuntimeEpoch {
    generation: Option<u64>,
    config_token: usize,
    runtime_token: usize,
    is_off: bool,
    // Weak anchors retain the allocation identity without keeping an obsolete
    // runtime alive. Pointer tokens cannot be recycled while a binding exists.
    config_anchor: Option<std::sync::Weak<std::sync::Mutex<wardian_core::models::AgentConfig>>>,
    runtime_anchor: Option<std::sync::Weak<std::sync::Mutex<String>>>,
}

impl std::fmt::Debug for RuntimeEpoch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeEpoch")
            .field("generation", &self.generation)
            .field("config_token", &self.config_token)
            .field("runtime_token", &self.runtime_token)
            .field("is_off", &self.is_off)
            .finish()
    }
}

impl PartialEq for RuntimeEpoch {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.config_token == other.config_token
            && self.runtime_token == other.runtime_token
            && self.is_off == other.is_off
            && match (&self.config_anchor, &other.config_anchor) {
                (Some(a), Some(b)) => std::sync::Weak::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && match (&self.runtime_anchor, &other.runtime_anchor) {
                (Some(a), Some(b)) => std::sync::Weak::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for RuntimeEpoch {}

#[derive(Clone, Debug)]
struct Root {
    logical: PathBuf,
    canonical: PathBuf,
    owner: Owner,
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    native: NativeFileIdentity,
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl Stamp {
    fn read(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let meta = file.metadata().ok()?;
        let native = native_file_identity(&file).ok()?;
        meta.is_file().then(|| Self {
            native,
            size: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
        })
    }
}

struct Header {
    stamp: Stamp,
    bytes: Vec<u8>,
    offset: u64,
    complete: bool,
    meta: Option<CodexRolloutMeta>,
}

struct Directory {
    root: PathBuf,
    entries: ReadDir,
}

#[derive(Clone)]
struct Binding {
    root: Root,
    meta: CodexRolloutMeta,
    parent_worker_id: Option<String>,
    depth: usize,
    stamp: Stamp,
}

#[derive(Default)]
struct PassReport {
    changed: bool,
    bytes: u64,
    deferred: bool,
    discovery_progress: bool,
}

struct WorkerRuntime {
    watcher: CodexWorkerEventWatcher,
    observer: LifecycleObserver,
    roots: Vec<Root>,
    root_keys: Vec<String>,
    directories: VecDeque<Directory>,
    visited_directories: HashSet<PathBuf>,
    headers: HashMap<PathBuf, Header>,
    header_queue: VecDeque<PathBuf>,
    source_cursor: usize,
    next_discovery: Instant,
    pending: Option<(Binding, LifecycleSourceIdentity)>,
    published: HashMap<String, TemporaryWorkerRecord>,
    published_bindings: HashMap<String, Binding>,
    bootstrap_seen: HashSet<String>,
    bootstrap_roots: Vec<Root>,
    bootstrap_queue: VecDeque<(TemporaryWorkerRecord, Root)>,
    activity_baselines: HashMap<PathBuf, (String, LifecycleActivityBaseline)>,
}

impl WorkerRuntime {
    fn new() -> Self {
        Self {
            watcher: CodexWorkerEventWatcher::new(MAX_SOURCES),
            observer: LifecycleObserver::new(),
            roots: Vec::new(),
            root_keys: Vec::new(),
            directories: VecDeque::new(),
            visited_directories: HashSet::new(),
            headers: HashMap::new(),
            header_queue: VecDeque::new(),
            source_cursor: 0,
            next_discovery: Instant::now(),
            pending: None,
            published: HashMap::new(),
            published_bindings: HashMap::new(),
            bootstrap_seen: HashSet::new(),
            bootstrap_roots: Vec::new(),
            bootstrap_queue: VecDeque::new(),
            activity_baselines: HashMap::new(),
        }
    }

    fn refresh_roots(
        &mut self,
        descriptors: &[AgentDescriptor],
        epochs: &HashMap<String, RuntimeEpoch>,
    ) {
        let roots = current_roots(descriptors, epochs);
        for root in &roots {
            let key = bootstrap_key(root);
            if self.bootstrap_seen.insert(key) {
                self.bootstrap_roots.push(root.clone());
            }
        }
        let keys: Vec<_> = roots
            .iter()
            .map(|root| {
                format!(
                    "{:?}|{}|{:?}",
                    root.logical,
                    bootstrap_key(root),
                    root.owner.sessions
                )
            })
            .collect();
        if keys == self.root_keys {
            return;
        }
        self.roots = roots;
        self.root_keys = keys;
        self.headers
            .retain(|path, _| canonical_root(path, &self.roots).is_some());
        self.header_queue
            .retain(|path| self.headers.contains_key(path));
        self.observer
            .retain_canonical_paths(&self.headers.keys().cloned().collect::<Vec<_>>());
        self.activity_baselines
            .retain(|path, _| self.headers.contains_key(path));
        // Capture unread bytes already present at owner entry. Reusing only
        // the parser's last observed length would misclassify old growth.
        for binding in self.bindings() {
            let _ = self.activity_source(&binding);
        }
        let _ = self.watcher.update_sessions_roots(
            self.roots
                .iter()
                .map(|root| root.canonical.clone())
                .collect(),
        );
        self.restart_discovery();
    }

    fn restart_discovery(&mut self) {
        self.directories.clear();
        self.visited_directories.clear();
        let paths: BTreeSet<_> = self
            .roots
            .iter()
            .map(|root| root.canonical.clone())
            .collect();
        for path in paths {
            self.add_directory(path.clone(), path);
        }
        self.next_discovery = Instant::now() + DISCOVERY_INTERVAL;
    }

    fn activity_source(&mut self, binding: &Binding) -> Option<ValidatedCodexSource> {
        let epoch = format!(
            "{}|{:?}",
            bootstrap_key(&binding.root),
            binding.stamp.native
        );
        if self
            .activity_baselines
            .get(&binding.meta.path)
            .is_none_or(|(previous, baseline)| {
                previous != &epoch || !baseline.matches_file(&binding.meta.path)
            })
        {
            let baseline = LifecycleActivityBaseline::capture(&binding.meta.path).ok()?;
            self.activity_baselines
                .insert(binding.meta.path.clone(), (epoch.clone(), baseline));
        }
        let baseline = &self.activity_baselines[&binding.meta.path].1;
        Some(
            ValidatedCodexSource::from_verified_ancestry(
                binding.meta.path.clone(),
                binding.meta.thread_id.clone(),
                binding.meta.parent_thread_id.clone(),
            )
            .with_activity_epoch(epoch, baseline.clone()),
        )
    }

    fn add_directory(&mut self, root: PathBuf, path: PathBuf) {
        if self.visited_directories.len() >= MAX_SOURCES || self.visited_directories.contains(&path)
        {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(&path) {
            self.visited_directories.insert(path);
            self.directories.push_back(Directory { root, entries });
        }
    }

    fn queue_header(&mut self, path: PathBuf) {
        let Ok(path) = std::fs::canonicalize(path) else {
            return;
        };
        if canonical_root(&path, &self.roots).is_none()
            || path.extension().and_then(|s| s.to_str()) != Some("jsonl")
        {
            return;
        }
        let Some(stamp) = Stamp::read(&path) else {
            return;
        };
        if !self.headers.contains_key(&path) {
            if self.headers.len() >= MAX_SOURCES {
                // New child events must not be permanently crowded out by a
                // historical or unqualified discovery cache. Live publications
                // remain protected; capacity never manufactures a live state.
                let retired = self
                    .headers
                    .iter()
                    .filter(|(_, header)| {
                        header.complete
                            && header.meta.as_ref().is_none_or(|meta| {
                                meta.parent_thread_id.is_none()
                                    || self.published.get(&meta.thread_id).is_none_or(|row| {
                                        row.state != TemporaryWorkerState::Running
                                            && row.state != TemporaryWorkerState::Requested
                                    })
                            })
                    })
                    .min_by_key(|(_, header)| header.stamp.modified)
                    .map(|(path, _)| path.clone());
                let Some(retired) = retired else {
                    return;
                };
                self.headers.remove(&retired);
                self.activity_baselines.remove(&retired);
                self.header_queue.retain(|path| path != &retired);
                self.observer
                    .retain_canonical_paths(&self.headers.keys().cloned().collect::<Vec<_>>());
            }
            self.headers.insert(
                path.clone(),
                Header {
                    stamp,
                    bytes: Vec::new(),
                    offset: 0,
                    complete: false,
                    meta: None,
                },
            );
            self.header_queue.push_back(path);
        }
    }

    fn discover(&mut self, hints: WorkerEventBatch) {
        if hints.overflowed
            || (self.directories.is_empty() && Instant::now() >= self.next_discovery)
        {
            self.restart_discovery();
        }
        for path in hints.paths {
            self.queue_header(path);
        }
        for _ in 0..DIRECTORY_ENTRIES_PER_PASS {
            let Some(mut directory) = self.directories.pop_front() else {
                break;
            };
            if let Some(result) = directory.entries.next() {
                let Ok(entry) = result else {
                    self.directories.push_back(directory);
                    continue;
                };
                if let Ok(path) = std::fs::canonicalize(entry.path()) {
                    if path.starts_with(&directory.root) {
                        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                            self.add_directory(directory.root.clone(), path);
                        } else {
                            self.queue_header(path);
                        }
                    }
                }
                self.directories.push_back(directory);
            }
        }
    }

    fn read_headers(&mut self, report: &mut PassReport) {
        // Reserve bytes for body progress even during continuous file discovery.
        let mut remaining = HEADER_BYTES_PER_PASS;
        let count = self.header_queue.len();
        let mut partial: usize = self.headers.values().map(|header| header.bytes.len()).sum();
        for _ in 0..count {
            let Some(path) = self.header_queue.pop_front() else {
                break;
            };
            let Some(header) = self.headers.get_mut(&path) else {
                continue;
            };
            let Some(stamp) = Stamp::read(&path) else {
                self.header_queue.push_back(path);
                continue;
            };
            if header.stamp != stamp {
                // An append leaves the already validated first line unchanged.
                // The lifecycle observer independently validates native generations.
                if stamp.native != header.stamp.native
                    || stamp.created != header.stamp.created
                    || stamp.size < header.stamp.size
                    || (header.complete && header.meta.is_none() && stamp.size != header.stamp.size)
                    || (stamp.size == header.stamp.size && stamp.modified != header.stamp.modified)
                {
                    partial = partial.saturating_sub(header.bytes.len());
                    header.bytes.clear();
                    header.offset = 0;
                    header.complete = false;
                    header.meta = None;
                }
                header.stamp = stamp;
            }
            if !header.complete
                && header.offset < header.stamp.size
                && remaining > 0
                && partial < MAX_PARTIAL_HEADERS
            {
                let allowance = remaining
                    .min(SOURCE_BYTES_PER_PASS)
                    .min((MAX_PARTIAL_HEADERS - partial) as u64);
                let before = header.bytes.len();
                let bytes = advance_header(&path, header, allowance);
                partial = partial
                    .saturating_sub(before)
                    .saturating_add(header.bytes.len());
                remaining = remaining.saturating_sub(bytes);
                report.bytes += bytes;
            }
            self.header_queue.push_back(path);
        }
    }

    fn bindings(&self) -> Vec<Binding> {
        let mut by_id: HashMap<(PathBuf, String), Vec<&CodexRolloutMeta>> = HashMap::new();
        for header in self.headers.values() {
            if let Some(meta) = &header.meta {
                if let Some(root) = canonical_root(&meta.path, &self.roots) {
                    by_id
                        .entry((root, meta.thread_id.clone()))
                        .or_default()
                        .push(meta);
                }
            }
        }
        let mut bindings = Vec::new();
        for metas in by_id.values().filter(|values| values.len() == 1) {
            let meta = metas[0];
            let Some(root_path) = canonical_root(&meta.path, &self.roots) else {
                continue;
            };
            let mut parent = meta.parent_thread_id.clone();
            let mut seen = HashSet::from([meta.thread_id.clone()]);
            for depth in 0..MAX_ANCESTRY {
                let Some(parent_id) = parent else {
                    break;
                };
                if !seen.insert(parent_id.clone()) {
                    break;
                }
                let owners: Vec<_> = self
                    .roots
                    .iter()
                    .filter(|root| {
                        root.canonical == root_path && root.owner.sessions.contains(&parent_id)
                    })
                    .collect();
                if owners.len() == 1 {
                    let root = owners[0].clone();
                    let direct_parent = meta.parent_thread_id.as_deref().unwrap_or_default();
                    let parent_worker_id = if depth == 0 {
                        root.owner.root_worker_id.clone()
                    } else {
                        self.published
                            .get(direct_parent)
                            .map(|worker| worker.worker_id.clone())
                    };
                    let stamp = self.headers[&meta.path].stamp.clone();
                    bindings.push(Binding {
                        root,
                        meta: meta.clone(),
                        parent_worker_id,
                        depth,
                        stamp,
                    });
                    break;
                }
                if !owners.is_empty() {
                    break;
                }
                let Some(ancestors) = by_id.get(&(root_path.clone(), parent_id)) else {
                    break;
                };
                if ancestors.len() != 1 {
                    break;
                }
                parent = ancestors[0].parent_thread_id.clone();
            }
        }
        bindings.sort_by(|a, b| (a.depth, &a.meta.thread_id).cmp(&(b.depth, &b.meta.thread_id)));
        bindings
    }

    fn reconcile(
        &mut self,
        descriptors: &[AgentDescriptor],
        epochs: &HashMap<String, RuntimeEpoch>,
        gates: &OwnerGates,
        state: &super::AppState,
    ) -> PassReport {
        let mut report = PassReport::default();
        self.refresh_roots(descriptors, epochs);
        self.bootstrap(&mut report, state, gates);
        let hints = self.watcher.drain(DIRECTORY_ENTRIES_PER_PASS);
        report.discovery_progress = !self.directories.is_empty();
        self.discover(hints);
        self.read_headers(&mut report);
        let bindings = self.bindings();
        // A previously live row must not outlive its verified ownership graph.
        // This negative update uses only an already registered, matching row;
        // rejected headers never create a provider-child identity.
        let retired: Vec<_> = self
            .published_bindings
            .iter()
            .filter(|(_, previous)| {
                !bindings
                    .iter()
                    .any(|current| same_binding(current, previous))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in retired {
            let mut acknowledged = true;
            if let Some(record) = self.published.get(&id) {
                match deactivate_in_current_owner(
                    record,
                    &self.published_bindings[&id].root,
                    state,
                    gates,
                ) {
                    Ok(Some(next)) => {
                        report.changed |= semantic_changed(Some(record), &next);
                        self.published.insert(id.clone(), next);
                    }
                    Ok(None) => {}
                    Err(()) => {
                        acknowledged = false;
                        report.deferred = true;
                    }
                }
            }
            if acknowledged {
                self.published_bindings.remove(&id);
            }
        }
        if let Some((pending, identity)) = self.pending.take() {
            if !bindings
                .iter()
                .any(|binding| same_binding(binding, &pending))
            {
                self.observer.retain_canonical_paths(
                    &self
                        .headers
                        .keys()
                        .filter(|path| *path != &identity.canonical_path)
                        .cloned()
                        .collect::<Vec<_>>(),
                );
            } else if let Some(stage) = self.observer.retry_pending() {
                match publish_in_current_owner(stage, &pending, &mut self.published, state, gates) {
                    Ok(changed) => {
                        report.changed |= changed;
                        self.published_bindings
                            .insert(pending.meta.thread_id.clone(), pending);
                    }
                    Err(()) => {
                        self.pending = Some((pending, identity));
                        report.deferred = true;
                        return report;
                    }
                }
            }
        }
        let count = bindings.len();
        for _ in 0..count.min(DIRECTORY_ENTRIES_PER_PASS) {
            if report.bytes >= CODEX_LIFECYCLE_BYTES_PER_PASS {
                report.deferred = true;
                break;
            }
            let mut binding = bindings[self.source_cursor % count].clone();
            self.source_cursor = self.source_cursor.wrapping_add(1);
            if !root_is_current(&binding.root) {
                continue;
            }
            if binding.depth > 0 {
                binding.parent_worker_id = binding
                    .meta
                    .parent_thread_id
                    .as_deref()
                    .and_then(|id| self.published.get(id))
                    .map(|worker| worker.worker_id.clone());
                if binding.parent_worker_id.is_none() {
                    report.deferred = true;
                    continue;
                }
            }
            let Some(source) = self.activity_source(&binding) else {
                report.deferred = true;
                continue;
            };
            let mut budget = LifecycleReadBudget::new(
                (CODEX_LIFECYCLE_BYTES_PER_PASS - report.bytes).min(LIFECYCLE_BYTES_PER_SOURCE),
            );
            let Ok(stage) =
                self.observer
                    .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
            else {
                report.deferred = true;
                break;
            };
            report.bytes += budget.consumed();
            report.deferred |= matches!(
                stage.publication(),
                LifecyclePublication::Pending {
                    reason: LifecyclePendingReason::BootstrapIncomplete,
                    ..
                }
            );
            if matches!(stage.publication(), LifecyclePublication::Rejected { .. }) {
                let _ = stage.commit_rejection();
                if let Some(previous) = self.published.get(&binding.meta.thread_id) {
                    match deactivate_in_current_owner(previous, &binding.root, state, gates) {
                        Ok(Some(next)) => {
                            report.changed |= semantic_changed(Some(previous), &next);
                            self.published.insert(binding.meta.thread_id.clone(), next);
                        }
                        Ok(None) => {}
                        Err(()) => {
                            report.deferred = true;
                        }
                    }
                }
                if let Some(header) = self.headers.get_mut(&binding.meta.path) {
                    header.complete = false;
                    header.meta = None;
                    header.offset = 0;
                    header.bytes.clear();
                }
                continue;
            }
            let identity = publication_identity(stage.publication()).clone();
            match publish_in_current_owner(stage, &binding, &mut self.published, state, gates) {
                Ok(changed) => {
                    report.changed |= changed;
                    self.published_bindings
                        .insert(binding.meta.thread_id.clone(), binding);
                }
                Err(()) => {
                    self.pending = Some((binding, identity));
                    report.deferred = true;
                    break;
                }
            }
        }
        report.deferred |=
            !self.directories.is_empty() || self.headers.values().any(|header| !header.complete);
        report
    }

    fn bootstrap(&mut self, report: &mut PassReport, state: &super::AppState, gates: &OwnerGates) {
        if !self.bootstrap_roots.is_empty() {
            // The frozen core API returns a complete snapshot. Take it only
            // once per actual runtime/root generation, never on the 5s tick.
            match temporary_workers::telemetry_records() {
                Ok(rows) => {
                    let by_id: HashMap<_, _> = rows
                        .iter()
                        .map(|row| (row.worker_id.as_str(), row))
                        .collect();
                    for row in &rows {
                        if self.bootstrap_queue.len() >= MAX_SOURCES {
                            break;
                        }
                        let owners: Vec<_> = self
                            .bootstrap_roots
                            .iter()
                            .filter(|root| bootstrap_matches(row, root, &by_id))
                            .collect();
                        if owners.len() == 1 {
                            self.bootstrap_queue
                                .push_back((row.clone(), owners[0].clone()));
                        }
                    }
                    self.bootstrap_roots.clear();
                }
                Err(_) => report.deferred = true,
            }
        }
        let count = self.bootstrap_queue.len().min(DIRECTORY_ENTRIES_PER_PASS);
        for _ in 0..count {
            let Some((previous, root)) = self.bootstrap_queue.pop_front() else {
                break;
            };
            // A newer generation supersedes this negative-only work; it must
            // never clear a row on behalf of an obsolete runtime.
            if !self
                .roots
                .iter()
                .any(|current| bootstrap_key(current) == bootstrap_key(&root))
            {
                continue;
            }
            if previous.provider_session_id.as_deref().is_some_and(|id| {
                self.published
                    .get(id)
                    .is_some_and(|row| same_registered_identity(row, &previous))
                    && self
                        .published_bindings
                        .get(id)
                        .is_some_and(|binding| bootstrap_key(&binding.root) == bootstrap_key(&root))
            }) {
                // A successful staged observation has already superseded this
                // failed bootstrap write in the same generation.
                continue;
            }
            match deactivate_in_current_owner(&previous, &root, state, gates) {
                Ok(Some(next)) => report.changed |= semantic_changed(Some(&previous), &next),
                Ok(None) => {}
                Err(()) => {
                    self.bootstrap_queue.push_back((previous, root));
                    report.deferred = true;
                }
            }
        }
        report.deferred |= !self.bootstrap_queue.is_empty();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeFileIdentity {
    platform: &'static str,
    primary: u64,
    secondary: u64,
}

fn native_file_identity(file: &File) -> io::Result<NativeFileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        return Ok(NativeFileIdentity {
            platform: "unix",
            primary: metadata.dev(),
            secondary: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle as _;

        #[repr(C)]
        #[allow(non_snake_case)]
        struct FileTime {
            dwLowDateTime: u32,
            dwHighDateTime: u32,
        }
        #[repr(C)]
        #[allow(non_snake_case)]
        struct ByHandleFileInformation {
            dwFileAttributes: u32,
            ftCreationTime: FileTime,
            ftLastAccessTime: FileTime,
            ftLastWriteTime: FileTime,
            dwVolumeSerialNumber: u32,
            nFileSizeHigh: u32,
            nFileSizeLow: u32,
            nNumberOfLinks: u32,
            nFileIndexHigh: u32,
            nFileIndexLow: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                file: *mut c_void,
                information: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
        // SAFETY: `file` is an open OS handle and Windows initializes the full
        // output structure when the call reports success.
        let succeeded =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful call above initialized `information`.
        let information = unsafe { information.assume_init() };
        return Ok(NativeFileIdentity {
            platform: "windows",
            primary: u64::from(information.dwVolumeSerialNumber),
            secondary: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        });
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Codex rollout native identity is unavailable on this platform",
    ))
}

fn same_binding(a: &Binding, b: &Binding) -> bool {
    a.meta.thread_id == b.meta.thread_id
        && a.meta.parent_thread_id == b.meta.parent_thread_id
        && a.meta.path == b.meta.path
        && a.root.canonical == b.root.canonical
        && a.stamp.native == b.stamp.native
        && a.stamp.size >= b.stamp.size
        && bootstrap_key(&a.root) == bootstrap_key(&b.root)
        && a.root.owner.sessions == b.root.owner.sessions
        && root_is_current(&b.root)
}

fn publication_identity(publication: &LifecyclePublication) -> &LifecycleSourceIdentity {
    match publication {
        LifecyclePublication::Pending { source, .. }
        | LifecyclePublication::Rejected { source, .. } => source,
        LifecyclePublication::Qualified { snapshot } => &snapshot.source,
    }
}

fn root_is_current(root: &Root) -> bool {
    std::fs::canonicalize(&root.logical).is_ok_and(|path| path == root.canonical)
}

fn canonical_root(path: &Path, roots: &[Root]) -> Option<PathBuf> {
    // Root snapshots were canonicalized this pass. Recheck the selected logical
    // root immediately before publish, rather than doing filesystem work once
    // per owner for every cached header.
    roots
        .iter()
        .filter(|root| path.starts_with(&root.canonical))
        .max_by_key(|root| root.canonical.components().count())
        .map(|root| root.canonical.clone())
}

fn current_roots(
    descriptors: &[AgentDescriptor],
    epochs: &HashMap<String, RuntimeEpoch>,
) -> Vec<Root> {
    let mut owners = Vec::new();
    for agent in descriptors
        .iter()
        .filter(|agent| agent.provider == "codex")
        .take(MAX_SOURCES)
    {
        owners.push(Owner {
            root_agent_id: Some(agent.session_id.clone()),
            root_worker_id: None,
            runtime_session_id: agent.session_id.clone(),
            workspace: agent.workspace.clone().unwrap_or_default(),
            sessions: known_session_ids(agent),
            origin: None,
            live_session: agent.provider_session_id.clone(),
            epoch: epochs.get(&agent.session_id).cloned().unwrap_or_default(),
            automation_root: None,
        });
    }
    for worker in temporary_workers::codex_automation_roots()
        .unwrap_or_default()
        .into_iter()
        .take(MAX_SOURCES.saturating_sub(owners.len()))
    {
        let expected_root = worker.clone();
        let Some(session) = worker.provider_session_id else {
            continue;
        };
        let Some((blueprint_id, (run_id, node_id))) =
            worker.blueprint_id.zip(worker.run_id.zip(worker.node_id))
        else {
            continue;
        };
        owners.push(Owner {
            root_agent_id: None,
            root_worker_id: Some(worker.worker_id),
            runtime_session_id: worker.runtime_session_id,
            workspace: worker.workspace,
            sessions: BTreeSet::from([session.clone()]),
            origin: Some(AutomationWorkerOrigin {
                blueprint_id,
                run_id,
                node_id,
            }),
            live_session: Some(session),
            epoch: RuntimeEpoch {
                generation: worker.runtime_generation,
                config_token: 0,
                runtime_token: 0,
                is_off: worker.state.is_terminal(),
                config_anchor: None,
                runtime_anchor: None,
            },
            automation_root: Some(expected_root),
        });
    }
    let shared = resolve_shared_codex_home(
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
        dirs::home_dir().as_deref(),
    )
    .map(|home| home.join("sessions"));
    let wardian_home = crate::utils::fs::get_wardian_home();
    let mut roots = Vec::new();
    for owner in owners {
        let private = wardian_home.as_ref().map(|home| {
            home.join("agents")
                .join(&owner.runtime_session_id)
                .join("habitat/.codex/sessions")
        });
        let mut seen = HashSet::new();
        for logical in private.into_iter().chain(shared.clone()) {
            if let Ok(canonical) = std::fs::canonicalize(&logical) {
                if seen.insert(canonical.clone()) && roots.len() < MAX_SOURCES {
                    roots.push(Root {
                        logical,
                        canonical,
                        owner: owner.clone(),
                    });
                }
            }
        }
    }
    roots.sort_by_key(|root| format!("{root:?}"));
    roots
}

fn advance_header(path: &Path, header: &mut Header, allowance: u64) -> u64 {
    if allowance == 0 || header.complete {
        return 0;
    }
    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    if file.seek(SeekFrom::Start(header.offset)).is_err() {
        return 0;
    }
    let mut consumed = 0;
    while consumed < allowance && header.bytes.len() < CODEX_LIFECYCLE_BYTES_PER_PASS as usize {
        let remaining = (allowance - consumed)
            .min(8192)
            .min(CODEX_LIFECYCLE_BYTES_PER_PASS - header.bytes.len() as u64);
        let mut buffer = vec![0; remaining as usize];
        let Ok(read) = file.read(&mut buffer) else {
            break;
        };
        if read == 0 {
            header.complete = true;
            header.bytes.clear();
            header.offset = 0;
            break;
        }
        consumed += read as u64;
        header.offset += read as u64;
        if let Some(end) = buffer[..read].iter().position(|byte| *byte == b'\n') {
            header.bytes.extend_from_slice(&buffer[..end]);
            header.meta = parse_header(path, &header.bytes);
            header.complete = true;
            header.bytes.clear();
            break;
        }
        header.bytes.extend_from_slice(&buffer[..read]);
        if header.offset >= header.stamp.size {
            // An incomplete first line supplies no ownership proof. Cache that
            // negative result until the file grows; unchanged files do no reads.
            header.complete = true;
            header.bytes.clear();
            header.offset = 0;
            break;
        }
    }
    if header.bytes.len() >= CODEX_LIFECYCLE_BYTES_PER_PASS as usize {
        header.complete = true;
        header.bytes.clear();
    }
    consumed
}

fn parse_header(path: &Path, bytes: &[u8]) -> Option<CodexRolloutMeta> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = value.get("payload")?;
    let thread_id = payload.get("id")?.as_str()?.trim();
    let name = path
        .file_name()?
        .to_str()?
        .strip_prefix("rollout-")?
        .strip_suffix(".jsonl")?;
    if thread_id.is_empty()
        || name.get(19..20) != Some("-")
        || name.get(20..)?.split('_').next() != Some(thread_id)
    {
        return None;
    }
    let parent_thread_id = payload
        .get("source")
        .and_then(|source| source.get("subagent"))
        .and_then(|subagent| subagent.get("thread_spawn"))
        .and_then(|spawn| spawn.get("parent_thread_id"))
        .and_then(|id| id.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    Some(CodexRolloutMeta {
        thread_id: thread_id.to_owned(),
        parent_thread_id,
        path: path.to_owned(),
        requested_at: value
            .get("timestamp")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn semantic_changed(
    previous: Option<&TemporaryWorkerRecord>,
    next: &TemporaryWorkerRecord,
) -> bool {
    previous.is_none_or(|previous| {
        let mut comparable = next.clone();
        comparable
            .last_observed_at
            .clone_from(&previous.last_observed_at);
        comparable != *previous
    })
}

/// Keep publication acknowledgement and private checkpoint advancement in the
/// same caller operation. A failed store drops the stage into retry_pending.
fn publish_and_commit(
    stage: LifecycleStage<'_>,
    binding: &Binding,
    published: &mut HashMap<String, TemporaryWorkerRecord>,
    store: impl for<'a> FnOnce(ObserveCodexProviderChild<'a>) -> Result<TemporaryWorkerRecord, ()>,
) -> Result<bool, ()> {
    let changed = publish(stage.publication(), binding, published, store)?;
    stage.commit().map_err(|_| ())?;
    Ok(changed)
}

fn write_core(input: ObserveCodexProviderChild<'_>) -> Result<TemporaryWorkerRecord, ()> {
    temporary_workers::observe_codex_provider_child(input).map_err(|_| ())
}

/// The existing lifecycle gate excludes pause/replacement/removal. Durable
/// roster admission follows that gate's established lock order. Keep both
/// guards through validation, storage and private checkpoint acknowledgement.
fn with_owner_exclusion<T, G>(
    gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    admit_roster: impl FnOnce() -> Result<G, ()>,
    owner_current: impl FnOnce() -> bool,
    operation: impl FnOnce() -> Result<T, ()>,
) -> Result<T, ()> {
    let _lifecycle = gate.try_lock_owned().map_err(|_| ())?;
    let _roster = admit_roster()?;
    if !owner_current() {
        return Err(());
    }
    operation()
}

fn publish_in_current_owner(
    stage: LifecycleStage<'_>,
    binding: &Binding,
    published: &mut HashMap<String, TemporaryWorkerRecord>,
    state: &super::AppState,
    gates: &OwnerGates,
) -> Result<bool, ()> {
    publish_in_current_owner_with_store(stage, binding, published, state, gates, |input| {
        match &binding.root.owner.automation_root {
            Some(expected_root) => {
                temporary_workers::observe_codex_provider_child_for_automation(input, expected_root)
                    .map_err(|_| ())
            }
            None => write_core(input),
        }
    })
}

fn publish_in_current_owner_with_store(
    stage: LifecycleStage<'_>,
    binding: &Binding,
    published: &mut HashMap<String, TemporaryWorkerRecord>,
    state: &super::AppState,
    gates: &OwnerGates,
    store: impl for<'a> FnOnce(ObserveCodexProviderChild<'a>) -> Result<TemporaryWorkerRecord, ()>,
) -> Result<bool, ()> {
    if binding.root.owner.automation_root.is_some() {
        if !root_is_current(&binding.root) {
            return Err(());
        }
        return publish_and_commit(stage, binding, published, store);
    }
    let id = binding.root.owner.root_agent_id.as_deref().ok_or(())?;
    let gate = gates.get(id).cloned().ok_or(())?;
    with_owner_exclusion(
        gate,
        || {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                .map_err(|_| ())?
                .ok_or(())
        },
        || owner_is_current(&binding.root, state),
        || publish_and_commit(stage, binding, published, store),
    )
}

fn deactivate_in_current_owner(
    previous: &TemporaryWorkerRecord,
    root: &Root,
    state: &super::AppState,
    gates: &OwnerGates,
) -> Result<Option<TemporaryWorkerRecord>, ()> {
    if root.owner.automation_root.is_some() {
        return deactivate_registered(previous, root, state);
    }
    let id = root.owner.root_agent_id.as_deref().ok_or(())?;
    let gate = gates.get(id).cloned().ok_or(())?;
    with_owner_exclusion(
        gate,
        || {
            wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                .map_err(|_| ())?
                .ok_or(())
        },
        || owner_is_current(root, state),
        || deactivate_registered(previous, root, state),
    )
}

fn publish<'a>(
    publication: &'a LifecyclePublication,
    binding: &'a Binding,
    published: &mut HashMap<String, TemporaryWorkerRecord>,
    store: impl FnOnce(ObserveCodexProviderChild<'a>) -> Result<TemporaryWorkerRecord, ()>,
) -> Result<bool, ()> {
    let (state, outcome, requested_at, terminal_at, turn_started_at, qualification) =
        match publication {
            LifecyclePublication::Pending { .. } => (
                TemporaryWorkerState::Unknown,
                None,
                binding.meta.requested_at.as_deref(),
                None,
                None,
                CodexChildObservationQualification::PendingOrUnqualified,
            ),
            LifecyclePublication::Qualified { snapshot } => (
                snapshot.state,
                snapshot.outcome.as_deref(),
                snapshot.requested_at.as_deref(),
                snapshot.terminal_at.as_deref(),
                snapshot.turn_started_at.as_deref(),
                CodexChildObservationQualification::Qualified,
            ),
            LifecyclePublication::Rejected { .. } => return Err(()),
        };
    if !root_is_current(&binding.root) {
        return Err(());
    }
    let Some(parent) = binding.meta.parent_thread_id.as_deref() else {
        return Err(());
    };
    let owner = &binding.root.owner;
    let path = binding.meta.path.to_str().ok_or(())?;
    if qualification == CodexChildObservationQualification::Qualified
        && !Stamp::read(&binding.meta.path).is_some_and(|stamp| {
            stamp.native == binding.stamp.native && stamp.size >= binding.stamp.size
        })
    {
        return Err(());
    }
    let next = store(ObserveCodexProviderChild {
        registration: RegisterProviderChild {
            provider: "codex",
            workspace: &owner.workspace,
            root_agent_id: owner.root_agent_id.as_deref(),
            parent_worker_id: binding.parent_worker_id.as_deref(),
            parent_provider_session_id: parent,
            runtime_session_id: &owner.runtime_session_id,
            provider_session_id: &binding.meta.thread_id,
            automation_origin: owner.origin.as_ref(),
            state,
            outcome,
            source_path: path,
            coverage: "codex_rollout_verified",
            requested_at,
            terminal_at,
        },
        qualification,
        turn_started_at,
    })?;
    let changed = semantic_changed(published.get(&binding.meta.thread_id), &next);
    published.insert(binding.meta.thread_id.clone(), next);
    Ok(changed)
}

fn bootstrap_key(root: &Root) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        root.canonical,
        root.owner.root_agent_id,
        root.owner.root_worker_id,
        root.owner.runtime_session_id,
        root.owner.live_session,
        root.owner.epoch,
        root.owner.origin,
        root.owner.workspace,
        root.owner.automation_root.as_ref().map(|row| (
            &row.worker_id,
            row.kind,
            &row.provider,
            row.runtime_generation,
            &row.owner_instance_id,
            row.attempt,
            row.state,
            &row.provider_session_id
        ))
    )
}

fn record_owner_matches(row: &TemporaryWorkerRecord, root: &Root) -> bool {
    let origin = root.owner.origin.as_ref();
    row.kind == temporary_workers::TemporaryWorkerKind::ProviderChild
        && row.provider == "codex"
        && row.root_agent_id == root.owner.root_agent_id
        && row.runtime_session_id == root.owner.runtime_session_id
        && row.workspace == root.owner.workspace
        && row.blueprint_id.as_deref() == origin.map(|origin| origin.blueprint_id.as_str())
        && row.run_id.as_deref() == origin.map(|origin| origin.run_id.as_str())
        && row.node_id.as_deref() == origin.map(|origin| origin.node_id.as_str())
        && row
            .source_path
            .as_deref()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .is_some_and(|path| path.starts_with(&root.canonical))
}

fn bootstrap_matches(
    row: &TemporaryWorkerRecord,
    root: &Root,
    records: &HashMap<&str, &TemporaryWorkerRecord>,
) -> bool {
    if !matches!(
        row.state,
        TemporaryWorkerState::Running | TemporaryWorkerState::Requested
    ) {
        return false;
    }
    let Some(live_session) = root.owner.live_session.as_deref() else {
        return false;
    };
    let mut current = row;
    let mut seen = HashSet::new();
    for _ in 0..MAX_ANCESTRY {
        if !record_owner_matches(current, root) || !seen.insert(current.worker_id.as_str()) {
            return false;
        }
        let Some(parent_session) = current.parent_provider_session_id.as_deref() else {
            return false;
        };
        if parent_session == live_session {
            return current.parent_worker_id == root.owner.root_worker_id;
        }
        let Some(parent) = current
            .parent_worker_id
            .as_deref()
            .and_then(|id| records.get(id))
            .copied()
        else {
            return false;
        };
        if parent.provider_session_id.as_deref() != Some(parent_session) {
            return false;
        }
        current = parent;
    }
    false
}

fn same_registered_identity(a: &TemporaryWorkerRecord, b: &TemporaryWorkerRecord) -> bool {
    a.worker_id == b.worker_id
        && a.kind == b.kind
        && a.provider == b.provider
        && a.runtime_session_id == b.runtime_session_id
        && a.runtime_generation == b.runtime_generation
        && a.root_agent_id == b.root_agent_id
        && a.parent_worker_id == b.parent_worker_id
        && a.parent_provider_session_id == b.parent_provider_session_id
        && a.provider_session_id == b.provider_session_id
        && a.blueprint_id == b.blueprint_id
        && a.run_id == b.run_id
        && a.node_id == b.node_id
        && a.workspace == b.workspace
        && a.source_path == b.source_path
        && a.source_key == b.source_key
}

fn owner_is_current(root: &Root, state: &super::AppState) -> bool {
    if !root_is_current(root) {
        return false;
    }
    if let Some(id) = &root.owner.root_agent_id {
        let snapshot = {
            let agents = state.agents.blocking_lock();
            agents.get(id).map(|agent| {
                (
                    agent.config.clone(),
                    RuntimeEpoch {
                        generation: agent.runtime_generation,
                        config_token: std::sync::Arc::as_ptr(&agent.config) as usize,
                        runtime_token: std::sync::Arc::as_ptr(&agent.current_status) as usize,
                        is_off: false,
                        config_anchor: Some(std::sync::Arc::downgrade(&agent.config)),
                        runtime_anchor: Some(std::sync::Arc::downgrade(&agent.current_status)),
                    },
                )
            })
        };
        let Some((config, mut epoch)) = snapshot else {
            return false;
        };
        let Ok(config) = config.lock() else {
            return false;
        };
        epoch.is_off = config.is_off;
        if epoch != root.owner.epoch {
            return false;
        }
        let live_matches = config.provider == "codex"
            && config.resume_session == root.owner.live_session
            && config.folder == root.owner.workspace;
        drop(config);
        return live_matches
            && crate::manager::persisted_agent_config(id).is_some_and(|persisted| {
                persisted.provider == "codex"
                    && persisted.resume_session == root.owner.live_session
                    && persisted.folder == root.owner.workspace
                    && persisted.is_off == root.owner.epoch.is_off
            });
    }
    root.owner
        .root_worker_id
        .as_deref()
        .and_then(|id| temporary_workers::load(id).ok().flatten())
        .is_some_and(|row| {
            row.provider == "codex"
                && row.kind == temporary_workers::TemporaryWorkerKind::Automation
                && row.runtime_session_id == root.owner.runtime_session_id
                && row.runtime_generation == root.owner.epoch.generation
                && row.provider_session_id == root.owner.live_session
                && row.workspace == root.owner.workspace
                && root.owner.origin.as_ref().is_some_and(|origin| {
                    row.blueprint_id.as_deref() == Some(origin.blueprint_id.as_str())
                        && row.run_id.as_deref() == Some(origin.run_id.as_str())
                        && row.node_id.as_deref() == Some(origin.node_id.as_str())
                })
        })
}

async fn runtime_snapshot(
    state: &super::AppState,
) -> (
    Vec<AgentDescriptor>,
    HashMap<String, RuntimeEpoch>,
    OwnerGates,
) {
    let snapshots: Vec<_> = {
        let agents = state.agents.lock().await;
        agents
            .iter()
            .map(|(id, agent)| {
                (
                    id.clone(),
                    agent.config.clone(),
                    RuntimeEpoch {
                        generation: agent.runtime_generation,
                        config_token: std::sync::Arc::as_ptr(&agent.config) as usize,
                        runtime_token: std::sync::Arc::as_ptr(&agent.current_status) as usize,
                        is_off: false,
                        config_anchor: Some(std::sync::Arc::downgrade(&agent.config)),
                        runtime_anchor: Some(std::sync::Arc::downgrade(&agent.current_status)),
                    },
                )
            })
            .collect()
    };
    let mut descriptors = Vec::new();
    let mut epochs = HashMap::new();
    for (session_id, config, mut epoch) in snapshots {
        let Ok(config) = config.lock() else {
            continue;
        };
        epoch.is_off = config.is_off;
        epochs.insert(session_id.clone(), epoch);
        descriptors.push(AgentDescriptor {
            session_id,
            provider: config.provider.clone(),
            provider_session_id: config.resume_session.clone(),
            workspace: Some(config.folder.clone()).filter(|folder| !folder.trim().is_empty()),
            is_off: config.is_off,
            verified_source_paths: Vec::new(),
        });
    }
    let mut gates = HashMap::new();
    for id in epochs.keys() {
        gates.insert(id.clone(), state.agent_lifecycle_lock_for(id).await);
    }
    (descriptors, epochs, gates)
}

fn deactivate_registered(
    previous: &TemporaryWorkerRecord,
    root: &Root,
    state: &super::AppState,
) -> Result<Option<TemporaryWorkerRecord>, ()> {
    if previous.state != TemporaryWorkerState::Running
        && previous.state != TemporaryWorkerState::Requested
    {
        return Ok(None);
    }
    let Some(current) = temporary_workers::load(&previous.worker_id).map_err(|_| ())? else {
        return Ok(None);
    };
    if !same_registered_identity(&current, previous) {
        return Ok(None);
    }
    if current.state != TemporaryWorkerState::Running
        && current.state != TemporaryWorkerState::Requested
    {
        return Ok(Some(current));
    }
    if !owner_is_current(root, state) || !record_owner_matches(&current, root) {
        return Ok(None);
    }
    // There is one production Codex child-state publisher. Re-read the recorded
    // ancestry and exact row immediately before the negative write; automation
    // state writers own Automation records, and follow-up only updates clocks.
    let mut ancestors = Vec::new();
    let mut parent = current.parent_worker_id.clone();
    for _ in 0..MAX_ANCESTRY {
        let Some(id) = parent else {
            break;
        };
        if root.owner.root_worker_id.as_deref() == Some(&id) {
            break;
        }
        let Some(row) = temporary_workers::load(&id).map_err(|_| ())? else {
            return Ok(None);
        };
        parent = row.parent_worker_id.clone();
        ancestors.push(row);
    }
    let by_id: HashMap<_, _> = ancestors
        .iter()
        .map(|row| (row.worker_id.as_str(), row))
        .collect();
    if !bootstrap_matches(&current, root, &by_id) {
        return Ok(None);
    }
    let Some(latest) = temporary_workers::load(&current.worker_id).map_err(|_| ())? else {
        return Ok(None);
    };
    if !same_registered_identity(&latest, &current) || !owner_is_current(root, state) {
        return Ok(None);
    }
    if !matches!(
        latest.state,
        TemporaryWorkerState::Running | TemporaryWorkerState::Requested
    ) {
        return Ok(Some(latest));
    }
    let origin = current
        .blueprint_id
        .as_ref()
        .zip(current.run_id.as_ref())
        .zip(current.node_id.as_ref())
        .map(|((blueprint_id, run_id), node_id)| AutomationWorkerOrigin {
            blueprint_id: blueprint_id.clone(),
            run_id: run_id.clone(),
            node_id: node_id.clone(),
        });
    let Some(parent) = current.parent_provider_session_id.as_deref() else {
        return Err(());
    };
    let Some(child) = current.provider_session_id.as_deref() else {
        return Err(());
    };
    let Some(path) = current.source_path.as_deref() else {
        return Err(());
    };
    let input = ObserveCodexProviderChild {
        registration: RegisterProviderChild {
            provider: "codex",
            workspace: &current.workspace,
            root_agent_id: current.root_agent_id.as_deref(),
            parent_worker_id: current.parent_worker_id.as_deref(),
            parent_provider_session_id: parent,
            runtime_session_id: &current.runtime_session_id,
            provider_session_id: child,
            automation_origin: origin.as_ref(),
            state: TemporaryWorkerState::Unknown,
            outcome: None,
            source_path: path,
            coverage: "codex_owner_unqualified",
            requested_at: Some(&current.requested_at),
            terminal_at: None,
        },
        qualification: CodexChildObservationQualification::PendingOrUnqualified,
        turn_started_at: None,
    };
    match &root.owner.automation_root {
        Some(expected_root) => {
            temporary_workers::observe_codex_provider_child_for_automation(input, expected_root)
                .map_err(|_| ())
                .map(Some)
        }
        None => write_core(input).map(Some),
    }
}

/// Start exactly one observer. Event wakes affect lifecycle reconciliation only;
/// large usage backfills cannot delay child-state publication.
pub(super) fn start(app: tauri::AppHandle) {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut runtime = WorkerRuntime::new();
        let mut wake = runtime.watcher.event_wake();
        let mut wake_open = true;
        let mut recovery = RECOVERY_INTERVAL;
        loop {
            let (descriptors, epochs, gates) =
                runtime_snapshot(&app.state::<super::AppState>()).await;
            let pass_app = app.clone();
            let pass = tokio::task::spawn_blocking(move || {
                let report = runtime.reconcile(
                    &descriptors,
                    &epochs,
                    &gates,
                    &pass_app.state::<super::AppState>(),
                );
                (runtime, report)
            })
            .await;
            match pass {
                Ok((returned, report)) => {
                    runtime = returned;
                    recovery = if report.deferred && (report.bytes > 0 || report.discovery_progress)
                    {
                        Duration::from_millis(50)
                    } else if report.deferred {
                        Duration::from_secs(1)
                    } else {
                        RECOVERY_INTERVAL
                    };
                    if report.changed {
                        let _ = app.emit("telemetry-updated", ());
                    }
                }
                Err(error) => {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Codex worker observer failed: {error}"
                    ));
                    runtime = WorkerRuntime::new();
                    wake = runtime.watcher.event_wake();
                    wake_open = true;
                    tokio::time::sleep(RECOVERY_INTERVAL).await;
                }
            }
            if wake_open {
                tokio::select! { open = wake.notified() => { wake_open = open; }, _ = tokio::time::sleep(recovery) => {} }
            } else {
                tokio::time::sleep(recovery).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, AtomicUsize};

    fn root(path: &Path) -> Root {
        Root {
            logical: path.to_owned(),
            canonical: std::fs::canonicalize(path).unwrap(),
            owner: Owner {
                root_agent_id: Some("agent".into()),
                root_worker_id: None,
                runtime_session_id: "agent".into(),
                workspace: String::new(),
                sessions: BTreeSet::from(["root".into()]),
                origin: None,
                live_session: Some("root".into()),
                epoch: RuntimeEpoch::default(),
                automation_root: None,
            },
        }
    }

    fn rollout(directory: &Path, id: &str, parent: &str, suffix: &str, padding: usize) -> PathBuf {
        let path = directory.join(format!("rollout-2026-10-05T00-00-00-{id}{suffix}.jsonl"));
        let header = serde_json::json!({"type":"session_meta", "timestamp":"2026-10-05T00:00:00Z",
            "payload":{"id":id, "source":{"subagent":{"thread_spawn":{"parent_thread_id":parent}}}, "padding":"x".repeat(padding)}});
        std::fs::write(&path, format!("{header}\n")).unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    fn row(input: ObserveCodexProviderChild<'_>) -> TemporaryWorkerRecord {
        let registration = input.registration;
        TemporaryWorkerRecord {
            worker_id: "worker".into(),
            kind: temporary_workers::TemporaryWorkerKind::ProviderChild,
            provider: "codex".into(),
            workspace: registration.workspace.into(),
            root_agent_id: registration.root_agent_id.map(str::to_owned),
            parent_worker_id: registration.parent_worker_id.map(str::to_owned),
            parent_provider_session_id: Some(registration.parent_provider_session_id.into()),
            blueprint_id: None,
            run_id: None,
            node_id: None,
            attempt: None,
            runtime_session_id: registration.runtime_session_id.into(),
            provider_session_id: Some(registration.provider_session_id.into()),
            runtime_generation: None,
            owner_instance_id: None,
            state: registration.state,
            outcome: registration.outcome.map(str::to_owned),
            capabilities: temporary_workers::TemporaryWorkerCapabilities::observe_only("fixture"),
            coverage: registration.coverage.into(),
            source_key: None,
            source_path: Some(registration.source_path.into()),
            requested_at: registration
                .requested_at
                .unwrap_or("2026-10-05T00:00:00Z")
                .into(),
            started_at: input.turn_started_at.map(str::to_owned),
            terminal_at: registration.terminal_at.map(str::to_owned),
            last_observed_at: "first".into(),
            last_follow_up_accepted_at: None,
            resumable_until: None,
            detail_retained_until: None,
            error: None,
        }
    }

    fn binding(path: &Path, root: Root) -> Binding {
        let bytes = std::fs::read(path).unwrap();
        let end = bytes.iter().position(|byte| *byte == b'\n').unwrap();
        Binding {
            root,
            meta: parse_header(path, &bytes[..end]).unwrap(),
            parent_worker_id: None,
            depth: 0,
            stamp: Stamp::read(path).unwrap(),
        }
    }

    fn active_fixture(id: &str, generation: u64) -> crate::state::ActiveAgent {
        use std::sync::{Arc, Mutex};
        crate::state::ActiveAgent {
            config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
                session_id: id.into(),
                provider: "codex".into(),
                folder: String::new(),
                is_off: false,
                resume_session: Some("root".into()),
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            memory_capability: None,
            runtime_generation: Some(generation),
            process_id: None,
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            last_query_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Idle".into())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
                id.into(),
                16,
                1024,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    #[tokio::test]
    async fn production_publisher_fences_snapshot_replacement_deletion_and_missing_durable_roster()
    {
        for transition in ["replace", "delete", "missing_durable"] {
            let id = uuid::Uuid::new_v4().to_string();
            let state = std::sync::Arc::new(super::super::AppState::new());
            state
                .agents
                .lock()
                .await
                .insert(id.clone(), active_fixture(&id, 1));
            let (_, epochs, gates) = runtime_snapshot(&state).await;
            // Exclude an unrelated roster-admission failure as the reason the
            // stale publisher never reached its injected real storage boundary.
            let admission = tokio::task::spawn_blocking(|| {
                wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
                    .unwrap()
                    .expect("roster admission available")
            })
            .await
            .unwrap();
            drop(admission);
            let directory = tempfile::tempdir().unwrap();
            let path = rollout(directory.path(), "child", "root", "", 0);
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            for kind in ["task_started", "task_complete"] {
                writeln!(file, "{}", serde_json::json!({"timestamp":"2026-10-05T02:00:00Z","type":"event_msg","payload":{"type":kind,"turn_id":"T"}})).unwrap();
            }
            file.flush().unwrap();
            let mut owner = root(directory.path());
            owner.owner.root_agent_id = Some(id.clone());
            owner.owner.runtime_session_id = id.clone();
            owner.owner.epoch = epochs[&id].clone();
            let binding = binding(&path, owner);
            let source =
                ValidatedCodexSource::from_verified_ancestry(path, "child", Some("root".into()));
            let (staged, read) = tokio::sync::oneshot::channel();
            let (resume, resumed) = tokio::sync::oneshot::channel();
            let caller_state = state.clone();
            let caller_gates = gates.clone();
            let caller = tokio::task::spawn_blocking(move || {
                let mut observer = LifecycleObserver::new();
                let mut budget = LifecycleReadBudget::for_pass();
                let stage = observer
                    .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
                    .unwrap();
                assert!(matches!(
                    stage.publication(),
                    LifecyclePublication::Qualified { .. }
                ));
                let expected = stage.publication().clone();
                let bytes = budget.consumed();
                staged.send(()).unwrap();
                resumed.blocking_recv().unwrap();
                let mut published = HashMap::new();
                let stores = AtomicUsize::new(0);
                assert!(publish_in_current_owner_with_store(
                    stage,
                    &binding,
                    &mut published,
                    &caller_state,
                    &caller_gates,
                    |input| {
                        stores.fetch_add(1, Ordering::Relaxed);
                        Ok(row(input))
                    }
                )
                .is_err());
                assert!(published.is_empty());
                assert_eq!(stores.load(Ordering::Relaxed), 0);
                let retry = observer.retry_pending().unwrap();
                assert_eq!(retry.publication(), &expected);
                assert!(publish_in_current_owner_with_store(
                    retry,
                    &binding,
                    &mut published,
                    &caller_state,
                    &caller_gates,
                    |input| {
                        stores.fetch_add(1, Ordering::Relaxed);
                        Ok(row(input))
                    }
                )
                .is_err());
                assert_eq!(stores.load(Ordering::Relaxed), 0);
                assert_eq!(budget.consumed(), bytes);
                assert_eq!(observer.retry_pending().unwrap().publication(), &expected);
            });
            read.await.unwrap();
            let exclusion = gates[&id].clone().lock_owned().await;
            match transition {
                "replace" => {
                    state
                        .agents
                        .lock()
                        .await
                        .insert(id.clone(), active_fixture(&id, 2));
                }
                "delete" => {
                    state.agents.lock().await.remove(&id);
                }
                "missing_durable" => {}
                _ => unreachable!(),
            }
            drop(exclusion);
            resume.send(()).unwrap();
            caller.await.unwrap();
        }
    }

    #[tokio::test]
    async fn owner_transition_after_snapshot_rejects_store_and_obsolete_pending_retry() {
        for deleted in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = rollout(directory.path(), "child", "root", "", 0);
            let binding = binding(&path, root(directory.path()));
            let source =
                ValidatedCodexSource::from_verified_ancestry(path, "child", Some("root".into()));
            let gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
            let current_epoch = AtomicU64::new(1);
            let owner_present = AtomicBool::new(true);
            let stores = AtomicUsize::new(0);
            let mut observer = LifecycleObserver::new();
            let mut budget = LifecycleReadBudget::for_pass();
            let stage = observer
                .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
                .unwrap();
            let expected = stage.publication().clone();
            let read_bytes = budget.consumed();
            let mut published = HashMap::new();
            // The captured stage predates an actual lifecycle-gated transition.
            let transition = gate.clone().lock_owned().await;
            if deleted {
                owner_present.store(false, Ordering::Release);
            } else {
                current_epoch.store(2, Ordering::Release);
            }
            drop(transition);
            let current = || {
                owner_present.load(Ordering::Acquire) && current_epoch.load(Ordering::Acquire) == 1
            };
            assert!(with_owner_exclusion(
                gate.clone(),
                || Ok(()),
                current,
                || publish_and_commit(stage, &binding, &mut published, |input| {
                    stores.fetch_add(1, Ordering::Relaxed);
                    Ok(row(input))
                })
            )
            .is_err());
            assert!(published.is_empty());
            assert_eq!(stores.load(Ordering::Relaxed), 0);
            let retry = observer.retry_pending().unwrap();
            assert_eq!(retry.publication(), &expected);
            assert!(with_owner_exclusion(
                gate,
                || Ok(()),
                current,
                || publish_and_commit(retry, &binding, &mut published, |input| {
                    stores.fetch_add(1, Ordering::Relaxed);
                    Ok(row(input))
                })
            )
            .is_err());
            assert_eq!(stores.load(Ordering::Relaxed), 0);
            assert_eq!(budget.consumed(), read_bytes);
            assert_eq!(observer.retry_pending().unwrap().publication(), &expected);
        }
    }

    #[tokio::test]
    async fn busy_lifecycle_retains_stage_and_success_holds_both_guards_through_commit() {
        struct RosterProbe(std::sync::Arc<AtomicBool>);
        impl Drop for RosterProbe {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = rollout(directory.path(), "child", "root", "", 0);
        let binding = binding(&path, root(directory.path()));
        let source =
            ValidatedCodexSource::from_verified_ancestry(path, "child", Some("root".into()));
        let gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let roster = std::sync::Arc::new(AtomicBool::new(false));
        let mut observer = LifecycleObserver::new();
        let mut published = HashMap::new();
        let stage = observer
            .observe(
                &source,
                &mut LifecycleReadBudget::for_pass(),
                CurrentNativePresence::Unobserved,
            )
            .unwrap();
        let expected = stage.publication().clone();
        let transition = gate.clone().lock_owned().await;
        assert!(with_owner_exclusion(
            gate.clone(),
            || Ok(()),
            || true,
            || publish_and_commit(stage, &binding, &mut published, |_| panic!(
                "busy lifecycle stored"
            ))
        )
        .is_err());
        drop(transition);
        let retry = observer.retry_pending().unwrap();
        assert_eq!(retry.publication(), &expected);
        assert!(with_owner_exclusion(
            gate.clone(),
            || {
                roster.store(true, Ordering::Release);
                Ok(RosterProbe(roster.clone()))
            },
            || roster.load(Ordering::Acquire),
            || publish_and_commit(retry, &binding, &mut published, |input| {
                assert!(gate.clone().try_lock_owned().is_err());
                assert!(roster.load(Ordering::Acquire));
                Ok(row(input))
            })
        )
        .unwrap());
        assert!(!roster.load(Ordering::Acquire));
        assert!(gate.try_lock_owned().is_ok());
        assert!(observer.retry_pending().is_none());
    }

    #[test]
    fn owner_entry_baseline_includes_unread_growth_and_reuses_current_epoch_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let path = rollout(directory.path(), "child", "root", "", 0);
        let mut binding = binding(&path, root(directory.path()));
        let mut runtime = WorkerRuntime::new();
        runtime.activity_source(&binding).unwrap();
        let old_baseline = runtime.activity_baselines[&path].1.clone();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"pre-epoch"}})).unwrap();
        file.flush().unwrap();
        binding.root.owner.epoch.generation = Some(2);
        runtime.activity_source(&binding).unwrap();
        let entered = runtime.activity_baselines[&path].1.clone();
        assert_ne!(entered, old_baseline);
        assert_eq!(entered, LifecycleActivityBaseline::capture(&path).unwrap());
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message"}})
        )
        .unwrap();
        file.flush().unwrap();
        runtime.activity_source(&binding).unwrap();
        assert_eq!(runtime.activity_baselines[&path].1, entered);
    }

    fn registered(binding: &Binding) -> TemporaryWorkerRecord {
        row(ObserveCodexProviderChild {
            registration: RegisterProviderChild {
                provider: "codex",
                workspace: &binding.root.owner.workspace,
                root_agent_id: binding.root.owner.root_agent_id.as_deref(),
                parent_worker_id: binding.parent_worker_id.as_deref(),
                parent_provider_session_id: binding.meta.parent_thread_id.as_deref().unwrap(),
                runtime_session_id: &binding.root.owner.runtime_session_id,
                provider_session_id: &binding.meta.thread_id,
                automation_origin: binding.root.owner.origin.as_ref(),
                state: TemporaryWorkerState::Running,
                outcome: None,
                source_path: binding.meta.path.to_str().unwrap(),
                coverage: "fixture",
                requested_at: None,
                terminal_at: None,
            },
            qualification: CodexChildObservationQualification::Qualified,
            turn_started_at: None,
        })
    }

    #[test]
    fn bootstrap_is_negative_only_and_requires_current_root_and_exact_recorded_chain() {
        let directory = tempfile::tempdir().unwrap();
        let child = binding(
            &rollout(directory.path(), "child", "root", "", 0),
            root(directory.path()),
        );
        let record = registered(&child);
        assert!(bootstrap_matches(&record, &child.root, &HashMap::new()));
        for state in [
            TemporaryWorkerState::Waiting,
            TemporaryWorkerState::Unknown,
            TemporaryWorkerState::Succeeded,
            TemporaryWorkerState::Failed,
            TemporaryWorkerState::Cancelled,
        ] {
            let mut other = record.clone();
            other.state = state;
            assert!(!bootstrap_matches(&other, &child.root, &HashMap::new()));
        }
        let mut historical = record.clone();
        historical.parent_provider_session_id = Some("past-root".into());
        let mut owner = child.root.clone();
        owner.owner.sessions.insert("past-root".into());
        assert!(!bootstrap_matches(&historical, &owner, &HashMap::new()));
        let mut foreign = record.clone();
        foreign.provider = "claude".into();
        assert!(!bootstrap_matches(&foreign, &owner, &HashMap::new()));
        foreign = record.clone();
        foreign.runtime_session_id = "foreign-runtime".into();
        assert!(!bootstrap_matches(&foreign, &owner, &HashMap::new()));
        foreign = record.clone();
        foreign.kind = temporary_workers::TemporaryWorkerKind::Automation;
        assert!(!bootstrap_matches(&foreign, &owner, &HashMap::new()));
        let grandchild = binding(
            &rollout(directory.path(), "grandchild", "child", "", 0),
            owner.clone(),
        );
        let mut descendant = registered(&grandchild);
        descendant.worker_id = "descendant".into();
        descendant.parent_worker_id = Some(record.worker_id.clone());
        let ancestors = HashMap::from([(record.worker_id.as_str(), &record)]);
        assert!(bootstrap_matches(&descendant, &owner, &ancestors));
        descendant.parent_provider_session_id = Some("wrong-child".into());
        assert!(!bootstrap_matches(&descendant, &owner, &ancestors));
        let mut newer = record.clone();
        newer.runtime_generation = Some(2);
        assert!(!same_registered_identity(&record, &newer));
        let previous_key = bootstrap_key(&owner);
        owner.owner.epoch.generation = Some(2);
        assert_ne!(previous_key, bootstrap_key(&owner));
        owner.owner.epoch.generation = None;
        owner.owner.sessions.insert("another-history".into());
        assert_eq!(previous_key, bootstrap_key(&owner));
    }

    #[test]
    fn shared_header_budget_counts_physical_reads_and_progresses_large_headers() {
        let directory = tempfile::tempdir().unwrap();
        let large = rollout(directory.path(), "large", "root", "", 64 * 1024);
        let small = rollout(directory.path(), "small", "root", "", 0);
        let mut runtime = WorkerRuntime::new();
        runtime.roots = vec![root(directory.path())];
        runtime.queue_header(large.clone());
        runtime.queue_header(small.clone());
        let mut report = PassReport::default();
        runtime.read_headers(&mut report);
        assert!(report.bytes <= HEADER_BYTES_PER_PASS);
        assert_eq!(runtime.headers[&large].offset, SOURCE_BYTES_PER_PASS);
        assert!(runtime.headers[&small].meta.is_some());
        for _ in 0..8 {
            let mut report = PassReport::default();
            runtime.read_headers(&mut report);
            assert!(report.bytes <= HEADER_BYTES_PER_PASS);
        }
        assert!(runtime.headers[&large].meta.is_some());
        let mut unchanged = PassReport::default();
        runtime.read_headers(&mut unchanged);
        assert_eq!(unchanged.bytes, 0);
    }

    #[test]
    fn zero_header_allowance_reads_no_bytes_and_invalid_utf8_is_accounted() {
        let directory = tempfile::tempdir().unwrap();
        let path = rollout(directory.path(), "child", "root", "", 0);
        std::fs::write(&path, [0xff, b'\n']).unwrap();
        let mut header = Header {
            stamp: Stamp::read(&path).unwrap(),
            bytes: Vec::new(),
            offset: 0,
            complete: false,
            meta: None,
        };
        assert_eq!(advance_header(&path, &mut header, 0), 0);
        assert_eq!(header.offset, 0);
        assert_eq!(advance_header(&path, &mut header, 2), 2);
        assert!(header.complete && header.meta.is_none());
    }

    #[test]
    fn ownership_rejects_ambiguous_ids_cycles_and_foreign_parents() {
        let directory = tempfile::tempdir().unwrap();
        let child = rollout(directory.path(), "child", "root", "", 0);
        let grandchild = rollout(directory.path(), "grandchild", "child", "", 0);
        let unrelated = rollout(directory.path(), "foreign", "other-root", "", 0);
        let cycle_a = rollout(directory.path(), "cycle-a", "cycle-b", "", 0);
        let cycle_b = rollout(directory.path(), "cycle-b", "cycle-a", "", 0);
        let mut runtime = WorkerRuntime::new();
        runtime.roots = vec![root(directory.path())];
        for path in [&child, &grandchild, &unrelated, &cycle_a, &cycle_b] {
            runtime.queue_header(path.clone());
        }
        runtime.read_headers(&mut PassReport::default());
        assert_eq!(runtime.bindings().len(), 2);
        let duplicate = rollout(directory.path(), "child", "root", "_copy", 0);
        runtime.queue_header(duplicate);
        runtime.read_headers(&mut PassReport::default());
        assert!(runtime.bindings().is_empty());
    }

    #[test]
    fn failed_publication_retries_the_exact_stage_without_body_rescan() {
        let directory = tempfile::tempdir().unwrap();
        let path = rollout(directory.path(), "child", "root", "", 0);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", serde_json::json!({"timestamp":"2026-10-05T00:01:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn"}})).unwrap();
        writeln!(file, "{}", serde_json::json!({"timestamp":"2026-10-05T00:02:00Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"turn"}})).unwrap();
        let binding = binding(&path, root(directory.path()));
        let source =
            ValidatedCodexSource::from_verified_ancestry(path, "child", Some("root".into()));
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::for_pass();
        let stage = observer
            .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
            .unwrap();
        let expected = stage.publication().clone();
        let mut published = HashMap::new();
        assert!(publish_and_commit(stage, &binding, &mut published, |_| Err(())).is_err());
        let stage = observer.retry_pending().unwrap();
        assert_eq!(stage.publication(), &expected);
        assert!(
            publish_and_commit(stage, &binding, &mut published, |input| Ok(row(input))).unwrap()
        );
        let stage = observer
            .observe(
                &source,
                &mut LifecycleReadBudget::new(0),
                CurrentNativePresence::Unobserved,
            )
            .unwrap();
        assert!(
            !publish_and_commit(stage, &binding, &mut published, |input| Ok(row(input))).unwrap()
        );
    }

    #[test]
    fn historical_open_turn_is_pending_and_observation_clock_is_not_semantic() {
        let directory = tempfile::tempdir().unwrap();
        let path = rollout(directory.path(), "child", "root", "", 0);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", serde_json::json!({"timestamp":"2026-10-05T00:01:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"old-turn"}})).unwrap();
        let binding = binding(&path, root(directory.path()));
        let source =
            ValidatedCodexSource::from_verified_ancestry(path, "child", Some("root".into()));
        let mut observer = LifecycleObserver::new();
        let mut budget = LifecycleReadBudget::for_pass();
        let stage = observer
            .observe(&source, &mut budget, CurrentNativePresence::Unobserved)
            .unwrap();
        let mut published = HashMap::new();
        publish_and_commit(stage, &binding, &mut published, |input| {
            assert_eq!(
                input.qualification,
                CodexChildObservationQualification::PendingOrUnqualified
            );
            assert_eq!(input.registration.state, TemporaryWorkerState::Unknown);
            assert!(
                input.registration.outcome.is_none()
                    && input.registration.terminal_at.is_none()
                    && input.turn_started_at.is_none()
            );
            Ok(row(input))
        })
        .unwrap();
        let previous = &published["child"];
        let mut next = previous.clone();
        next.last_observed_at = "later".into();
        assert!(!semantic_changed(Some(previous), &next));
        next.state = TemporaryWorkerState::Running;
        assert!(semantic_changed(Some(previous), &next));
    }
}
