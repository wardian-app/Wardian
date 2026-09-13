//! Scheduled telemetry ingest.
//!
//! Phase 1 built a store that can advance a provider source; this is the thing
//! that decides *which* sources exist and *when* to advance them. It runs on its
//! own cadence, deliberately not on the 5s metrics tick: that tick is on the
//! critical path for status and readiness, and ingest reads whole log deltas and
//! holds the state database's write lock. Sharing the tick would trade a live
//! status surface for a historical one.
//!
//! Discovery is separated from execution so the mapping from agents to sources
//! can be tested without an app, a database, or a provider.

use crate::manager::opencode::opencode_database_path;
use crate::state::AppState;
use crate::utils::fs::get_wardian_home;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::Manager;
use wardian_core::telemetry::identity::{canonical_path, source_key};
use wardian_core::telemetry::ingest::{ingest_source, IngestError};
use wardian_core::telemetry::sources::opencode::sessions_in_directory;
use wardian_core::telemetry::sources::{is_supported, uses_archive, SourceContext, SourceError};

/// How often ingest runs while at least one agent is alive.
///
/// Well below the hour a rollup bucket covers, so the newest bucket is never
/// more than a minute stale, and far above the cost of a delta read.
const INGEST_INTERVAL_ACTIVE: std::time::Duration = std::time::Duration::from_secs(60);

/// How often ingest runs when nothing is running.
///
/// Sources cannot grow without an agent writing to them, so polling at the
/// active cadence would spend its time confirming that files have not changed.
/// It is not zero because an agent can write through a headless run this state
/// does not observe.
const INGEST_INTERVAL_IDLE: std::time::Duration = std::time::Duration::from_secs(300);

/// How long the background loop may reuse an unchanged source topology.
///
/// Live session and workspace changes invalidate it immediately. The bounded
/// refresh exists for provider sessions created outside Wardian, which have no
/// in-memory lifecycle event to announce their arrival.
const DISCOVERY_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// A single ingest pass is considered slow after this long.
///
/// A first pass over a very large backlog is legitimately slow, so this is
/// generous. It is a diagnostic threshold, not a cancellation deadline:
/// blocking work cannot be safely cancelled after it starts.
const INGEST_PASS_SLOW_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(300);

/// How many bytes of provider log one pass will read before stopping.
///
/// An agent's whole history is hundreds of rollout files and can run to several
/// gigabytes, which is far too much to read before the first Dashboard paint.
/// Sources are visited newest first, so the horizons a reader is actually
/// looking at are correct after the first pass and older history fills in over
/// the following ones.
const INGEST_BYTES_PER_PASS: u64 = 128 * 1024 * 1024;

/// What an agent contributes to discovery.
///
/// Deliberately owned and inert rather than a handle to live state: discovery
/// must not hold the agents lock while touching the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDescriptor {
    pub session_id: String,
    pub provider: String,
    /// The provider's own session identifier (`resume_session`). Without it
    /// there is nothing to look the source up by.
    pub provider_session_id: Option<String>,
    /// Directory this agent works in.
    ///
    /// Opencode stamps every session with the directory it ran in, which is the
    /// only way to find sessions that predate the conversation archive or that
    /// ran headless — the equivalent of the per-agent habitat the file-backed
    /// providers get.
    pub workspace: Option<String>,
    pub is_off: bool,
    /// Exact provider sources established by a canonical ownership adapter.
    /// When present, discovery must not widen this record through workspace or
    /// projected-home inference.
    pub verified_source_paths: Vec<PathBuf>,
}

/// A resolved, ingestable source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSource {
    pub session_id: String,
    pub provider: String,
    /// Every provider-native session this source should be read for.
    ///
    /// A file-backed source holds exactly one, because the file *is* the
    /// session. A database source holds every session the agent has ever run,
    /// because they all live in one file and are separated only by this list.
    pub provider_session_ids: Vec<String>,
    pub path: PathBuf,
    /// Modified time as epoch milliseconds, used to read the newest history
    /// first. Zero when the source has no filesystem identity to ask.
    pub modified_ms: u64,
}

impl DiscoveredSource {
    fn context(&self) -> SourceContext {
        SourceContext::new(&self.session_id, &self.provider, &self.path)
            .with_provider_session_ids(self.provider_session_ids.clone())
    }
}

type DiscoveryIdentity = Vec<(String, String, Option<String>, Option<String>, String)>;

/// Source paths change much less often than source contents. Retain the path
/// topology between background passes so the one-minute delta reader does not
/// recursively rediscover every historical transcript first.
#[derive(Default)]
struct BackgroundDiscoveryCache {
    identity: DiscoveryIdentity,
    sources: Vec<DiscoveredSource>,
    discovered_at: Option<Instant>,
}

impl BackgroundDiscoveryCache {
    fn refresh(&mut self, agents: &[AgentDescriptor], now: Instant) {
        let identity = discovery_identity(agents);
        if self.should_rediscover(&identity, now) {
            let discovery_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                crate::utils::runtime_profile::RuntimeMetric::TelemetryIngestDiscover,
            );
            self.sources = discover_sources(agents);
            discovery_profile.finish(self.sources.len() as u64);
            self.identity = identity;
            self.discovered_at = Some(now);
        } else {
            refresh_source_order(&mut self.sources);
        }
    }

    fn should_rediscover(&self, identity: &DiscoveryIdentity, now: Instant) -> bool {
        self.identity != *identity
            || self.discovered_at.is_none_or(|last| {
                now.checked_duration_since(last).unwrap_or_default() >= DISCOVERY_REFRESH_INTERVAL
            })
    }
}

fn discovery_identity(agents: &[AgentDescriptor]) -> DiscoveryIdentity {
    let mut identity: DiscoveryIdentity = agents
        .iter()
        .filter(|agent| is_supported(&agent.provider))
        .map(|agent| {
            (
                agent.session_id.clone(),
                agent.provider.clone(),
                agent.provider_session_id.clone(),
                agent.workspace.clone(),
                agent
                    .verified_source_paths
                    .iter()
                    .map(|path| canonical_path(path).to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })
        .collect();
    identity.sort();
    identity.dedup();
    identity
}

/// Where an agent's past sessions are looked up.
///
/// Behind a trait because the real implementation answers from this machine's
/// filesystem and conversation archive, which would make every assertion about
/// *which sessions belong to an agent* depend on what happens to exist here.
pub trait SessionCatalog {
    /// Every codex rollout file belonging to this agent, newest last.
    fn codex_rollouts(&self, agent: &AgentDescriptor) -> Vec<PathBuf>;

    /// Every Claude Code transcript belonging to this agent.
    fn claude_transcripts(&self, agent: &AgentDescriptor) -> Vec<PathBuf>;

    /// Every pi session log belonging to this agent.
    ///
    /// Pi writes into a Wardian-owned directory per agent rather than a shared
    /// provider home, so everything under it belongs to that agent by
    /// construction and no session-id attribution is needed.
    fn pi_sessions(&self, agent: &AgentDescriptor) -> Vec<PathBuf>;

    /// Every archived conversation turn file belonging to this agent.
    ///
    /// Used for providers with no native reader, where Wardian's own record of
    /// what happened is the only record there is.
    fn archive_turn_files(&self, agent: &AgentDescriptor) -> Vec<PathBuf>;

    /// Opencode sessions this agent owns outright, from its own recorded ids.
    ///
    /// Higher confidence than a directory match: these came from the agent's own
    /// live session or its conversation archive, so no other agent can claim
    /// them.
    fn opencode_sessions(&self, agent: &AgentDescriptor) -> Vec<String>;

    /// Opencode sessions that merely ran in this agent's workspace.
    ///
    /// Weaker evidence, and deliberately separate: several agents can share one
    /// directory, and a session that ran there belongs to exactly one of them.
    fn opencode_sessions_in_workspace(&self, agent: &AgentDescriptor) -> Vec<String>;

    /// The single database every opencode agent on this machine shares.
    fn opencode_database(&self) -> Option<PathBuf>;
}

/// The catalog backed by this machine.
pub struct MachineCatalog {
    /// Session id to rollout path for the shared codex home, built once.
    ///
    /// Resolving a session id used to walk the whole `sessions` tree, which was
    /// affordable when one agent meant one lookup and is not now that it means
    /// one lookup per session the agent has ever run.
    shared_codex: HashMap<String, PathBuf>,
    shared_codex_root: Option<PathBuf>,
    shared_codex_paths: HashSet<PathBuf>,
    /// Session id to transcript path for the shared claude home.
    shared_claude: HashMap<String, PathBuf>,
    shared_claude_root: Option<PathBuf>,
    shared_claude_paths: HashSet<PathBuf>,
}

impl Default for MachineCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl MachineCatalog {
    pub fn new() -> Self {
        let home = dirs::home_dir();
        let shared_codex_root = home
            .as_ref()
            .map(|home| home.join(".codex").join("sessions"));
        let shared_codex = shared_codex_root
            .as_deref()
            .map(index_transcripts)
            .unwrap_or_default();
        let shared_codex_paths = shared_codex
            .values()
            .map(|path| canonical_path(path))
            .collect();
        let shared_claude_root = home
            .as_ref()
            .map(|home| home.join(".claude").join("projects"));
        let shared_claude = shared_claude_root
            .as_deref()
            .map(index_transcripts)
            .unwrap_or_default();
        let shared_claude_paths = shared_claude
            .values()
            .map(|path| canonical_path(path))
            .collect();
        Self {
            shared_codex,
            shared_codex_root,
            shared_codex_paths,
            shared_claude,
            shared_claude_root,
            shared_claude_paths,
        }
    }

    /// Resolve an agent's sessions against a projected home first, then the
    /// shared one.
    fn resolve(
        &self,
        agent: &AgentDescriptor,
        projected: &[&str],
        shared: &HashMap<String, PathBuf>,
        shared_root: Option<&Path>,
        shared_paths: &HashSet<PathBuf>,
    ) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let known = known_session_ids(agent);
        let mut projected_is_shared = false;

        // Wardian can project a per-agent provider home. Everything under it
        // belongs to this agent by construction, so no attribution guesswork is
        // needed and sessions Wardian never observed are still found.
        if let Some(home) = get_wardian_home() {
            let mut root = home.join("agents").join(&agent.session_id).join("habitat");
            for segment in projected {
                root = root.join(segment);
            }
            if shared_root
                .is_some_and(|shared_root| projected_root_matches_shared(&root, shared_root))
            {
                // The common projected-home layout is a junction to the
                // machine-wide provider home. Rewalking that same tree for
                // every agent made discovery scale as agents x all rollouts.
                // The shared catalog already walked it once.
                projected_is_shared = true;
                paths.extend(known.iter().filter_map(|id| shared.get(id).cloned()));
            } else {
                paths.extend(index_transcripts(&root).into_values().filter(|path| {
                    // A projected provider home is normally private, but it can
                    // contain a nested junction into the shared provider home.
                    // Retain private files and only this agent's shared files.
                    let physical = canonical_path(path);
                    if !shared_paths.contains(&physical) {
                        return true;
                    }
                    transcript_session_id(path).is_some_and(|id| known.contains(&id))
                }));
            }
        }

        // Agents without a projected home write into the shared one, where a
        // file is only attributable through a session id we recorded.
        if !projected_is_shared {
            for id in known {
                if let Some(path) = shared.get(&id) {
                    paths.push(path.clone());
                }
            }
        }

        paths
    }
}

impl SessionCatalog for MachineCatalog {
    fn codex_rollouts(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
        if !agent.verified_source_paths.is_empty() {
            return agent.verified_source_paths.clone();
        }
        self.resolve(
            agent,
            &[".codex", "sessions"],
            &self.shared_codex,
            self.shared_codex_root.as_deref(),
            &self.shared_codex_paths,
        )
    }

    fn claude_transcripts(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
        if !agent.verified_source_paths.is_empty() {
            return agent.verified_source_paths.clone();
        }
        self.resolve(
            agent,
            &[".claude", "projects"],
            &self.shared_claude,
            self.shared_claude_root.as_deref(),
            &self.shared_claude_paths,
        )
    }

    fn pi_sessions(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
        if !agent.verified_source_paths.is_empty() {
            return agent.verified_source_paths.clone();
        }
        let Some(home) = get_wardian_home() else {
            return Vec::new();
        };
        let sessions = home
            .join("agents")
            .join(&agent.session_id)
            .join("pi")
            .join("sessions");
        let Ok(entries) = std::fs::read_dir(&sessions) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            })
            .collect();
        // Pi names its logs with a leading timestamp, so sorting is oldest
        // first, matching the "newest last" order the other readers return.
        paths.sort();
        paths
    }

    fn archive_turn_files(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
        if !agent.verified_source_paths.is_empty() {
            return agent.verified_source_paths.clone();
        }
        let Some(home) = get_wardian_home() else {
            return Vec::new();
        };
        let conversations = home
            .join("agents")
            .join(&agent.session_id)
            .join("conversations");
        let Ok(entries) = std::fs::read_dir(&conversations) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path().join("turns.jsonl"))
            .filter(|path| path.is_file())
            .collect()
    }

    fn opencode_sessions(&self, agent: &AgentDescriptor) -> Vec<String> {
        known_session_ids(agent).into_iter().collect()
    }

    fn opencode_sessions_in_workspace(&self, agent: &AgentDescriptor) -> Vec<String> {
        // Sessions Wardian never archived are still someone's work. Opencode
        // records the directory each ran in, so the workspace attributes them
        // the way a projected habitat attributes a rollout file — but a
        // directory is not exclusive, so discovery decides who ends up owning
        // these.
        let (Some(path), Some(workspace)) = (opencode_database_path(), agent.workspace.as_deref())
        else {
            return Vec::new();
        };
        sessions_in_directory(&path, workspace).unwrap_or_default()
    }

    fn opencode_database(&self) -> Option<PathBuf> {
        opencode_database_path()
    }
}

fn projected_root_matches_shared(projected: &Path, shared: &Path) -> bool {
    canonical_path(projected) == canonical_path(shared)
}

/// Every provider session this agent is known to have run.
///
/// The live `resume_session` is only the conversation open right now. An agent
/// accumulates a new provider session every time it is restarted, and the
/// conversation archive is the record of them, so reading only the live one
/// reports the agent's newest conversation as its entire history.
fn known_session_ids(agent: &AgentDescriptor) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = archived_session_ids(&agent.session_id, &agent.provider);
    if let Some(live) = agent
        .provider_session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        ids.insert(live.to_string());
    }
    ids
}

/// Session ids recorded by the conversation archive for one agent.
fn archived_session_ids(agent_id: &str, provider: &str) -> BTreeSet<String> {
    let Some(home) = get_wardian_home() else {
        return BTreeSet::new();
    };
    let index = home
        .join("agents")
        .join(agent_id)
        .join("conversations")
        .join("index.jsonl");
    let Ok(entries) = wardian_core::conversations::read_latest_index_entries(&index) else {
        return BTreeSet::new();
    };
    entries
        .into_iter()
        .filter(|entry| entry.provider == provider)
        .flat_map(|entry| entry.provider_session_ids)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Map every transcript under a provider home to its session id.
///
/// Codex lays rollouts out as `sessions/<year>/<month>/<day>/rollout-<stamp>-<uuid>.jsonl`
/// and claude uses `projects/<encoded-cwd>/<uuid>.jsonl`, but the depth is not
/// load-bearing here: the walk takes whatever nesting it finds, so a layout
/// change costs coverage rather than correctness.
fn index_transcripts(root: &Path) -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => {
                    if let Some(id) = transcript_session_id(&path) {
                        found.insert(id, path);
                    }
                }
                _ => {}
            }
        }
    }

    found
}

/// The session id a rollout filename ends with.
///
/// The uuid contains the same `-` the rest of the name is built from, so it is
/// taken by length from the end rather than by splitting.
fn transcript_session_id(path: &Path) -> Option<String> {
    const UUID_LEN: usize = 36;
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".jsonl")?;
    if stem.len() < UUID_LEN {
        return None;
    }
    let id = &stem[stem.len() - UUID_LEN..];
    id.chars()
        .all(|c| c.is_ascii_hexdigit() || c == '-')
        .then(|| id.to_string())
}

/// Resolve every agent that has an ingestable source right now.
///
/// An agent being off is not a reason to skip it. Its log still holds work that
/// was never ingested — the app may have been closed for most of the session —
/// and re-reading an unchanging file costs one cursor comparison. Skipping off
/// agents would make history depend on whether Wardian happened to be running.
pub fn discover_sources(agents: &[AgentDescriptor]) -> Vec<DiscoveredSource> {
    discover_sources_with(agents, &MachineCatalog::new())
}

/// Discovery against an arbitrary catalog.
///
/// Sources are returned newest first so a caller that cannot afford to read
/// everything in one pass reads the history that is being asked about.
pub fn discover_sources_with(
    agents: &[AgentDescriptor],
    catalog: &dyn SessionCatalog,
) -> Vec<DiscoveredSource> {
    let mut sources = Vec::new();
    // A Codex/Claude file is one physical source even when several projected
    // agent homes expose it. The core store intentionally keys those sources
    // by provider and physical path, so discovery must use the same identity or
    // each claimant will rewrite the source owner and its facts.
    //
    // The opencode database is different: it is shared by every opencode agent,
    // but the provider session ids are part of the source's read context, so
    // its key remains agent-scoped.
    let mut seen: HashMap<String, DiscoveredSource> = HashMap::new();
    let mut ambiguous = HashSet::new();
    let verified_source_keys: HashSet<String> = agents
        .iter()
        .flat_map(|agent| {
            agent.verified_source_paths.iter().map(|path| {
                source_key(
                    &agent.provider,
                    &agent.session_id,
                    &canonical_path(path).to_string_lossy(),
                )
            })
        })
        .collect();

    // Opencode sessions are assigned to exactly one agent. A session's rows are
    // stored under whichever agent's source read them, so letting two agents in
    // one workspace both claim a session would file the same turns twice and
    // credit one agent's work to its neighbour.
    let owned_opencode = assign_opencode_sessions(agents, catalog);

    for agent in agents {
        if !is_supported(&agent.provider) {
            continue;
        }

        let resolved: Vec<(PathBuf, Vec<String>)> = match agent.provider.as_str() {
            // One file per session: the path carries the identity, so no id
            // list is needed to select rows out of it.
            "codex" => catalog
                .codex_rollouts(agent)
                .into_iter()
                .map(|path| (path, Vec::new()))
                .collect(),
            "claude" => catalog
                .claude_transcripts(agent)
                .into_iter()
                .map(|path| (path, Vec::new()))
                .collect(),
            "pi" => catalog
                .pi_sessions(agent)
                .into_iter()
                .map(|path| (path, Vec::new()))
                .collect(),
            // One database for every agent and every session it ever ran. It
            // stays a single source with a single cursor, and the id list is
            // what narrows it to this agent.
            "opencode" => {
                let ids = owned_opencode
                    .get(&agent.session_id)
                    .cloned()
                    .unwrap_or_default();
                match catalog.opencode_database() {
                    Some(path) if !ids.is_empty() => vec![(path, ids)],
                    _ => Vec::new(),
                }
            }
            // No native reader: Wardian's own record of the conversation is the
            // only record there is. One source per archived conversation, each
            // with its own cursor.
            provider if uses_archive(provider) => catalog
                .archive_turn_files(agent)
                .into_iter()
                .map(|path| (path, Vec::new()))
                .collect(),
            _ => Vec::new(),
        };

        for (path, provider_session_ids) in resolved {
            let path = canonical_path(&path);
            let provider_session_ids = if matches!(agent.provider.as_str(), "codex" | "claude") {
                transcript_session_id(&path).into_iter().collect::<Vec<_>>()
            } else {
                provider_session_ids
            };
            let key = source_key(&agent.provider, &agent.session_id, &path.to_string_lossy());
            if agent.verified_source_paths.is_empty() && verified_source_keys.contains(&key) {
                continue;
            }
            if ambiguous.contains(&key) {
                continue;
            }
            let modified_ms = modified_epoch_ms(&path);
            let candidate = DiscoveredSource {
                session_id: agent.session_id.clone(),
                provider: agent.provider.clone(),
                provider_session_ids,
                path,
                modified_ms,
            };
            if let Some(previous) = seen.get(&key) {
                if previous.session_id != candidate.session_id
                    && matches!(candidate.provider.as_str(), "codex" | "claude")
                {
                    // The same physical transcript has been claimed by two
                    // agents. The catalog failed to prove a unique recorded
                    // provider-session owner, so drop both claims rather than
                    // letting roster order decide who receives the history.
                    seen.remove(&key);
                    ambiguous.insert(key);
                }
                continue;
            }
            seen.insert(key, candidate);
        }
    }

    sources.extend(seen.into_values());
    sort_sources(&mut sources);
    sources
}

/// Decide which agent owns each opencode session.
///
/// Two passes, because the two kinds of evidence are not equal. An agent's own
/// recorded ids — its live session and its conversation archive — name sessions
/// it definitely ran, so those are claimed first and can never be taken by a
/// neighbour. A workspace match only says a session ran in the same directory,
/// which several agents can share; those are handed out afterwards, and only to
/// an agent nobody has already claimed them for.
///
/// A contested session goes to one agent rather than none: it is real work by
/// one of them, and dropping it would lose history to protect an attribution
/// that is already approximate. Ordering is by agent id so the choice is stable
/// across passes rather than flipping with roster order.
fn assign_opencode_sessions(
    agents: &[AgentDescriptor],
    catalog: &dyn SessionCatalog,
) -> HashMap<String, Vec<String>> {
    let mut owner: HashMap<String, String> = HashMap::new();
    let mut opencode: Vec<&AgentDescriptor> = agents
        .iter()
        .filter(|agent| agent.provider == "opencode")
        .collect();
    opencode.sort_by(|left, right| left.session_id.cmp(&right.session_id));

    for agent in &opencode {
        for id in catalog.opencode_sessions(agent) {
            owner.insert(id, agent.session_id.clone());
        }
    }
    for agent in &opencode {
        for id in catalog.opencode_sessions_in_workspace(agent) {
            owner.entry(id).or_insert_with(|| agent.session_id.clone());
        }
    }

    let mut assigned: HashMap<String, Vec<String>> = HashMap::new();
    for (session, agent_id) in owner {
        assigned.entry(agent_id).or_default().push(session);
    }
    for sessions in assigned.values_mut() {
        sessions.sort();
    }
    assigned
}

/// Last-modified time in epoch milliseconds, or zero when it cannot be read.
fn modified_epoch_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn refresh_source_order(sources: &mut [DiscoveredSource]) {
    for source in sources.iter_mut() {
        source.modified_ms = modified_epoch_ms(&source.path);
    }
    sort_sources(sources);
}

fn sort_sources(sources: &mut [DiscoveredSource]) {
    sources.sort_by(|left, right| {
        right
            .modified_ms
            .cmp(&left.modified_ms)
            .then_with(|| left.path.cmp(&right.path))
    });
}

/// What one pass over every source accomplished.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestPassReport {
    pub sources: usize,
    pub advanced: usize,
    pub turns: usize,
    pub edits: usize,
    pub intervals: usize,
    pub buckets_recomputed: usize,
    /// Sources that were not readable this pass for an expected reason — busy or
    /// not yet written. Counted rather than listed, because they are the steady
    /// state for an idle agent and listing them would be noise every minute.
    pub unavailable: usize,
    /// Sources that failed for a reason worth seeing, as `provider/agent: why`.
    pub failures: Vec<String>,
    /// Sources left unread because the pass ran out of budget. Non-zero means a
    /// backfill is still in progress, not that anything went wrong.
    pub deferred: usize,
}

impl IngestPassReport {
    /// Whether anything changed, and therefore whether a surface needs telling.
    pub fn changed(&self) -> bool {
        self.advanced > 0
    }
}

/// Advance every discovered source once.
///
/// Blocking: this reads files and holds the state database's lock, so callers on
/// an async runtime must run it under `spawn_blocking`.
///
/// One source failing never stops the pass. A locked opencode database or a
/// rotated codex log is a normal transient condition, and letting it abort the
/// pass would let one bad source starve every other agent's history.
pub fn run_ingest_pass(sources: &[DiscoveredSource]) -> IngestPassReport {
    let mut report = IngestPassReport {
        sources: sources.len(),
        ..Default::default()
    };
    let mut budget = INGEST_BYTES_PER_PASS;

    for (index, source) in sources.iter().enumerate() {
        // Budget is spent on bytes actually read, so a source already level with
        // its file costs a cursor comparison and never consumes any. That is
        // what lets the steady state still visit every source each pass while a
        // first run over a large backlog is spread across several.
        if budget == 0 {
            report.deferred = sources.len() - index;
            break;
        }
        let ctx = source.context();
        // The ingest result is returned *through* `get_db_conn` rather than
        // mapped into its boxed error type, so the error stays typed and an
        // expected unavailability can still be told apart from a real fault.
        let outcome = wardian_core::db::get_db_conn(|conn| Ok(ingest_source(conn, &ctx)));

        match outcome {
            Ok(Ok(outcome)) => {
                if outcome.advanced() {
                    report.advanced += 1;
                }
                // Only byte cursors measure bytes. A database cursor is a
                // timestamp, so its difference is meaningless here and is
                // charged nothing; the opencode source is one row-bounded read
                // per agent rather than a backlog to work through.
                if matches!(source.provider.as_str(), "codex" | "claude") {
                    let read = outcome.cursor_after.saturating_sub(outcome.cursor_before);
                    budget = budget.saturating_sub(read.max(0) as u64);
                }
                report.turns += outcome.turns;
                report.edits += outcome.edits;
                report.intervals += outcome.intervals;
                report.buckets_recomputed += outcome.buckets_recomputed;
            }
            Ok(Err(error)) => {
                if !is_reportable(&error) {
                    report.unavailable += 1;
                    continue;
                }
                report.failures.push(format!(
                    "{}/{}: {error}",
                    source.provider, source.session_id
                ));
            }
            // The database itself is unavailable, so no later source will fare
            // better this pass.
            Err(error) => {
                report
                    .failures
                    .push(format!("telemetry store unavailable: {error}"));
                break;
            }
        }
    }

    report
}

/// Whether a source-level failure is worth logging at all.
///
/// A source that is merely busy or not yet present is the expected steady state
/// for an agent that has not written anything, and logging it every minute would
/// bury the failures that do mean something.
pub fn failure_is_noteworthy(error: &SourceError) -> bool {
    !matches!(error, SourceError::Busy(_) | SourceError::Unavailable(_))
}

/// Whether an ingest failure should be surfaced rather than counted.
///
/// Everything that is not a transient source condition is reportable, including
/// store errors — a failing write is a defect, not weather.
fn is_reportable(error: &IngestError) -> bool {
    match error {
        IngestError::Source(source) => failure_is_noteworthy(source),
        IngestError::UnsupportedProvider(_)
        | IngestError::SourceOwnership { .. }
        | IngestError::InvalidFacts(_)
        | IngestError::Store(_) => true,
    }
}

/// Snapshot the agents currently known to the app.
pub async fn agent_descriptors(state: &AppState) -> Vec<AgentDescriptor> {
    let agents = state.agents.lock().await;
    agents
        .iter()
        .map(|(session_id, agent)| {
            let config = agent.config.lock().unwrap();
            AgentDescriptor {
                session_id: session_id.clone(),
                provider: config.provider.clone(),
                provider_session_id: config.resume_session.clone(),
                workspace: Some(config.folder.clone()).filter(|folder| !folder.trim().is_empty()),
                is_off: config.is_off,
                verified_source_paths: Vec::new(),
            }
        })
        .collect()
}

const CODEX_META_BYTES: u64 = 256 * 1024;
const CODEX_TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
struct CodexRolloutMeta {
    thread_id: String,
    parent_thread_id: Option<String>,
    path: PathBuf,
    state: wardian_core::temporary_workers::TemporaryWorkerState,
    outcome: Option<String>,
    requested_at: Option<String>,
    terminal_at: Option<String>,
}

fn read_codex_rollout_meta(path: &Path) -> Option<CodexRolloutMeta> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    reader
        .by_ref()
        .take(CODEX_META_BYTES + 1)
        .read_line(&mut first_line)
        .ok()?;
    if first_line.len() as u64 > CODEX_META_BYTES || !first_line.ends_with('\n') {
        return None;
    }
    let meta: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    if meta.get("type").and_then(|value| value.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = meta.get("payload")?;
    let requested_at = meta
        .get("timestamp")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let thread_id = payload.get("id")?.as_str()?.trim().to_string();
    if thread_id.is_empty() {
        return None;
    }
    let parent_thread_id = payload
        .get("source")
        .and_then(|source| source.get("subagent"))
        .and_then(|subagent| subagent.get("thread_spawn"))
        .and_then(|spawn| spawn.get("parent_thread_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let mut state = wardian_core::temporary_workers::TemporaryWorkerState::Unknown;
    let mut outcome = None;
    let mut terminal_at = None;
    let mut file = reader.into_inner();
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(CODEX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    file.take(CODEX_TAIL_BYTES).read_to_string(&mut tail).ok()?;
    for (index, line) in tail.lines().enumerate() {
        if start > 0 && index == 0 {
            continue;
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let event_type = event.get("type").and_then(|value| value.as_str());
        let payload_type = event
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(|value| value.as_str());
        match (event_type, payload_type) {
            (Some("event_msg"), Some("user_message" | "user_message_event")) => {
                state = wardian_core::temporary_workers::TemporaryWorkerState::Running;
                outcome = None;
                terminal_at = None;
            }
            (Some("event_msg"), Some("task_complete")) | (Some("turn.completed"), _) => {
                state = wardian_core::temporary_workers::TemporaryWorkerState::Succeeded;
                outcome = Some("completed".to_string());
                terminal_at = event
                    .get("timestamp")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
            }
            (Some("event_msg"), Some("turn_aborted")) => {
                state = wardian_core::temporary_workers::TemporaryWorkerState::Cancelled;
                outcome = Some("aborted".to_string());
                terminal_at = event
                    .get("timestamp")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);
            }
            _ => {}
        }
    }

    Some(CodexRolloutMeta {
        thread_id,
        parent_thread_id,
        path: canonical_path(path),
        state,
        outcome,
        requested_at,
        terminal_at,
    })
}

fn temporary_worker_descriptors() -> Vec<AgentDescriptor> {
    wardian_core::temporary_workers::telemetry_records()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|worker| {
            let source_path = worker.source_path?;
            Some(AgentDescriptor {
                session_id: worker.worker_id,
                provider: worker.provider,
                provider_session_id: worker.provider_session_id,
                workspace: Some(worker.workspace).filter(|value| !value.trim().is_empty()),
                is_off: worker.state.is_terminal(),
                verified_source_paths: vec![PathBuf::from(source_path)],
            })
        })
        .collect()
}

fn reconcile_codex_children(permanent: &[AgentDescriptor], catalog: &MachineCatalog) {
    let automation_roots =
        wardian_core::temporary_workers::codex_automation_roots().unwrap_or_default();
    let mut candidate_paths: HashSet<PathBuf> = catalog
        .shared_codex
        .values()
        .map(|path| canonical_path(path))
        .collect();
    for agent in permanent.iter().filter(|agent| agent.provider == "codex") {
        candidate_paths.extend(
            catalog
                .codex_rollouts(agent)
                .into_iter()
                .map(|path| canonical_path(&path)),
        );
    }
    for root in &automation_roots {
        let descriptor = AgentDescriptor {
            session_id: root.runtime_session_id.clone(),
            provider: root.provider.clone(),
            provider_session_id: root.provider_session_id.clone(),
            workspace: Some(root.workspace.clone()),
            is_off: root.state.is_terminal(),
            verified_source_paths: Vec::new(),
        };
        candidate_paths.extend(
            catalog
                .codex_rollouts(&descriptor)
                .into_iter()
                .map(|path| canonical_path(&path)),
        );
    }

    let metas: Vec<CodexRolloutMeta> = candidate_paths
        .iter()
        .filter_map(|path| read_codex_rollout_meta(path))
        .collect();

    for agent in permanent.iter().filter(|agent| agent.provider == "codex") {
        let roots = known_session_ids(agent);
        reconcile_codex_tree(
            &metas,
            roots,
            Some(agent.session_id.as_str()),
            None,
            &agent.session_id,
            agent.workspace.as_deref().unwrap_or_default(),
            None,
        );
    }
    for root in automation_roots {
        let Some(provider_session_id) = root.provider_session_id.as_deref() else {
            continue;
        };
        if let Some(meta) = metas
            .iter()
            .find(|meta| meta.thread_id == provider_session_id)
        {
            let _ = wardian_core::temporary_workers::attach_verified_source(
                &root.worker_id,
                &meta.path.to_string_lossy(),
                "codex_rollout_verified",
            );
        }
        let Some(origin) = root
            .blueprint_id
            .as_ref()
            .zip(root.run_id.as_ref())
            .zip(root.node_id.as_ref())
            .map(|((blueprint_id, run_id), node_id)| {
                wardian_core::temporary_workers::AutomationWorkerOrigin {
                    blueprint_id: blueprint_id.clone(),
                    run_id: run_id.clone(),
                    node_id: node_id.clone(),
                }
            })
        else {
            continue;
        };
        reconcile_codex_tree(
            &metas,
            [provider_session_id.to_string()].into_iter().collect(),
            None,
            Some(root.worker_id.as_str()),
            &root.runtime_session_id,
            &root.workspace,
            Some(&origin),
        );
    }
}

fn reconcile_codex_tree(
    metas: &[CodexRolloutMeta],
    roots: BTreeSet<String>,
    root_agent_id: Option<&str>,
    root_worker_id: Option<&str>,
    runtime_session_id: &str,
    workspace: &str,
    automation_origin: Option<&wardian_core::temporary_workers::AutomationWorkerOrigin>,
) {
    let descendants = verified_codex_descendants(metas, &roots);
    let mut worker_by_provider_session: HashMap<String, Option<String>> = roots
        .into_iter()
        .map(|root| (root, root_worker_id.map(str::to_string)))
        .collect();
    for (parent_provider_session_id, child) in descendants {
        let parent_worker_id = worker_by_provider_session
            .get(&parent_provider_session_id)
            .and_then(|worker| worker.as_deref());
        let registered = wardian_core::temporary_workers::register_provider_child(
            wardian_core::temporary_workers::RegisterProviderChild {
                provider: "codex",
                workspace,
                root_agent_id,
                parent_worker_id,
                parent_provider_session_id: &parent_provider_session_id,
                runtime_session_id,
                provider_session_id: &child.thread_id,
                automation_origin,
                state: child.state,
                outcome: child.outcome.as_deref(),
                source_path: &child.path.to_string_lossy(),
                coverage: "codex_parent_thread_id_verified",
                requested_at: child.requested_at.as_deref(),
                terminal_at: child.terminal_at.as_deref(),
            },
        );
        if let Ok(worker) = registered {
            worker_by_provider_session.insert(child.thread_id.clone(), Some(worker.worker_id));
        }
    }
}

fn verified_codex_descendants<'a>(
    metas: &'a [CodexRolloutMeta],
    roots: &BTreeSet<String>,
) -> Vec<(String, &'a CodexRolloutMeta)> {
    let mut children: HashMap<&str, Vec<&CodexRolloutMeta>> = HashMap::new();
    for meta in metas {
        if let Some(parent) = meta.parent_thread_id.as_deref() {
            children.entry(parent).or_default().push(meta);
        }
    }
    let mut queue: Vec<String> = roots.iter().cloned().collect();
    let mut visited = HashSet::new();
    let mut descendants = Vec::new();
    while let Some(parent_provider_session_id) = queue.pop() {
        if !visited.insert(parent_provider_session_id.clone()) {
            continue;
        }
        for child in children
            .get(parent_provider_session_id.as_str())
            .into_iter()
            .flatten()
        {
            descendants.push((parent_provider_session_id.clone(), *child));
            queue.push(child.thread_id.clone());
        }
    }
    descendants
}

/// Run one full cycle: snapshot agents, resolve sources, advance them.
pub async fn run_ingest_cycle(state: &AppState) -> IngestPassReport {
    let agents = agent_descriptors(state).await;
    tokio::task::spawn_blocking(move || {
        let catalog = MachineCatalog::new();
        let _ = wardian_core::temporary_workers::reconcile_stale_automation_owners();
        reconcile_codex_children(&agents, &catalog);
        let _ = wardian_core::temporary_workers::apply_detail_retention();
        let mut agents = agents;
        agents.extend(temporary_worker_descriptors());
        let discovery_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
            crate::utils::runtime_profile::RuntimeMetric::TelemetryIngestDiscover,
        );
        let sources = discover_sources_with(&agents, &catalog);
        discovery_profile.finish(sources.len() as u64);
        let pass_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
            crate::utils::runtime_profile::RuntimeMetric::TelemetryIngestPass,
        );
        let report = run_ingest_pass(&sources);
        pass_profile.finish(report.sources as u64);
        report
    })
    .await
    .unwrap_or_default()
}

/// How long to wait before resuming an unfinished backfill.
///
/// Long enough that the write lock is released and the UI stays responsive,
/// short enough that a large history is caught up in minutes rather than days.
const INGEST_INTERVAL_BACKFILL: std::time::Duration = std::time::Duration::from_secs(5);

/// Cadence for the next pass, given what the app is currently doing.
///
/// An unfinished backfill outranks both steady-state cadences: waiting a full
/// interval between chunks would turn a bounded pass into a history that takes
/// days to become true.
fn next_interval(
    any_agent_known: bool,
    any_agent_live: bool,
    deferred: usize,
) -> std::time::Duration {
    if deferred > 0 {
        INGEST_INTERVAL_BACKFILL
    } else if any_agent_live || !any_agent_known {
        // Startup can reach this loop before restoration installs its agents.
        // Treating that transient empty roster as settled-idle delayed the first
        // real ingest for five minutes.
        INGEST_INTERVAL_ACTIVE
    } else {
        INGEST_INTERVAL_IDLE
    }
}

/// Retention runs only after an ingest pass and only when its in-memory deadline
/// has elapsed. Checking the deadline is the only added work on ordinary passes.
fn telemetry_maintenance_is_due(now: Instant, next_attempt: Instant) -> bool {
    now >= next_attempt
}

/// Start the background ingest loop.
///
/// The first pass is immediate rather than one interval away, so opening the app
/// after a long headless stretch shows that work without a minute of blank
/// Dashboard.
pub fn start_telemetry_ingest(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut discovery_cache = BackgroundDiscoveryCache::default();
        let mut last_worker_reconcile = None;
        let mut next_maintenance_attempt =
            Instant::now() + crate::state::telemetry_maintenance::initial_delay();
        loop {
            let state = app_handle.state::<AppState>();
            let permanent_descriptors = agent_descriptors(&state).await;
            let any_agent_known = !permanent_descriptors.is_empty();
            let any_agent_live = permanent_descriptors.iter().any(|agent| !agent.is_off);
            let mut pass_cache = std::mem::take(&mut discovery_cache);
            let reconcile_due = last_worker_reconcile
                .is_none_or(|last: Instant| last.elapsed() >= DISCOVERY_REFRESH_INTERVAL);

            let pass = tokio::task::spawn_blocking(move || {
                if reconcile_due {
                    let catalog = MachineCatalog::new();
                    let _ = wardian_core::temporary_workers::reconcile_stale_automation_owners();
                    reconcile_codex_children(&permanent_descriptors, &catalog);
                }
                let _ = wardian_core::temporary_workers::apply_detail_retention();
                let mut descriptors = permanent_descriptors;
                descriptors.extend(temporary_worker_descriptors());
                pass_cache.refresh(&descriptors, Instant::now());
                let pass_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                    crate::utils::runtime_profile::RuntimeMetric::TelemetryIngestPass,
                );
                let report = run_ingest_pass(&pass_cache.sources);
                pass_profile.finish(report.sources as u64);
                (report, pass_cache)
            });

            // Keep the ingest loop single-flight. Dropping a JoinHandle after
            // a timeout detaches the blocking pass, allowing another pass to
            // contend for the database while the old one is still running.
            // A slow pass delays the next cadence, but cannot multiply work.
            let pass_started = std::time::Instant::now();
            let mut deferred = 0;
            match pass.await {
                Ok((report, returned_cache)) => {
                    discovery_cache = returned_cache;
                    if reconcile_due {
                        last_worker_reconcile = Some(Instant::now());
                    }
                    for failure in &report.failures {
                        crate::utils::logging::log_debug(&format!(
                            "[Wardian] Telemetry ingest source failed: {failure}"
                        ));
                    }
                    deferred = report.deferred;
                    if deferred > 0 {
                        crate::utils::logging::log_debug(&format!(
                            "[Wardian] Telemetry backfill in progress: {deferred} sources remaining"
                        ));
                    }
                    if report.changed() {
                        use tauri::Emitter;
                        let _ = app_handle.emit("telemetry-updated", ());
                    }
                }
                Err(error) => crate::utils::logging::log_debug(&format!(
                    "[Wardian] Telemetry ingest pass failed; continuing: {error}"
                )),
            }
            if pass_started.elapsed() >= INGEST_PASS_SLOW_THRESHOLD {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] Telemetry ingest pass took {}s; next pass deferred until it completed",
                    pass_started.elapsed().as_secs()
                ));
            }

            let now = Instant::now();
            if telemetry_maintenance_is_due(now, next_maintenance_attempt) {
                let delay = crate::state::telemetry_maintenance::run_if_due_after_ingest().await;
                next_maintenance_attempt = Instant::now() + delay;
            }

            tokio::time::sleep(next_interval(any_agent_known, any_agent_live, deferred)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(session: &str, provider: &str, resume: Option<&str>) -> AgentDescriptor {
        AgentDescriptor {
            session_id: session.to_string(),
            provider: provider.to_string(),
            provider_session_id: resume.map(str::to_string),
            workspace: None,
            is_off: false,
            verified_source_paths: Vec::new(),
        }
    }

    /// Stands in for the machine: an agent owns one rollout per session it has
    /// run, and every opencode agent shares one database the way they really do.
    #[derive(Default)]
    struct StubCatalog {
        /// Agent id to the sessions it has ever run.
        history: HashMap<String, Vec<String>>,
    }

    impl StubCatalog {
        fn with(agent_id: &str, sessions: &[&str]) -> Self {
            let mut history = HashMap::new();
            history.insert(
                agent_id.to_string(),
                sessions.iter().map(|id| id.to_string()).collect(),
            );
            Self { history }
        }

        fn sessions(&self, agent: &AgentDescriptor) -> Vec<String> {
            let mut ids: BTreeSet<String> = self
                .history
                .get(&agent.session_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            if let Some(live) = agent.provider_session_id.clone() {
                if !live.trim().is_empty() {
                    ids.insert(live);
                }
            }
            ids.into_iter().collect()
        }
    }

    impl SessionCatalog for StubCatalog {
        fn codex_rollouts(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
            self.sessions(agent)
                .into_iter()
                .map(|id| PathBuf::from(format!("/logs/{id}.jsonl")))
                .collect()
        }

        fn claude_transcripts(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
            self.sessions(agent)
                .into_iter()
                .map(|id| PathBuf::from(format!("/claude/{id}.jsonl")))
                .collect()
        }

        fn pi_sessions(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
            self.sessions(agent)
                .into_iter()
                .map(|id| PathBuf::from(format!("/pi/{id}.jsonl")))
                .collect()
        }

        fn opencode_sessions(&self, agent: &AgentDescriptor) -> Vec<String> {
            self.sessions(agent)
        }

        fn opencode_sessions_in_workspace(&self, _agent: &AgentDescriptor) -> Vec<String> {
            Vec::new()
        }

        fn archive_turn_files(&self, agent: &AgentDescriptor) -> Vec<PathBuf> {
            self.sessions(agent)
                .into_iter()
                .map(|id| PathBuf::from(format!("/archive/{id}/turns.jsonl")))
                .collect()
        }

        fn opencode_database(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/data/opencode/opencode.db"))
        }
    }

    #[test]
    fn a_provider_wardian_does_not_recognise_is_not_discovered() {
        let agents = vec![agent("a1", "mock", Some("ses_1"))];
        assert!(discover_sources_with(&agents, &StubCatalog::default()).is_empty());
    }

    #[test]
    fn antigravity_is_discovered_through_the_conversation_archive() {
        // It publishes no token accounting and no parseable transcript, but
        // Wardian watched its turns happen. Reporting those agents as having
        // done nothing was a gap in the reader, not a fact about the agents.
        let catalog = StubCatalog::with("a1", &["conv-1", "conv-2"]);
        let sources = discover_sources_with(&[agent("a1", "antigravity", None)], &catalog);
        assert_eq!(sources.len(), 2);
        assert!(sources
            .iter()
            .all(|source| source.path.ends_with("turns.jsonl")));
    }

    #[test]
    fn claude_agents_are_discovered() {
        // Seven of this habitat's agents ran on claude and appeared to have
        // done nothing at all, because the provider had no reader.
        let catalog = StubCatalog::with("a1", &["ses-old"]);
        let sources = discover_sources_with(&[agent("a1", "claude", Some("ses-live"))], &catalog);
        assert_eq!(sources.len(), 2);
        assert!(sources.iter().all(|source| source.provider == "claude"));
    }

    #[test]
    fn every_codex_session_an_agent_ran_becomes_a_source() {
        // The defect this pins: discovery used to resolve the agent's *live*
        // session only, so an agent with a hundred past conversations reported
        // its newest one as the whole of its history.
        let catalog = StubCatalog::with("a1", &["ses-1", "ses-2", "ses-3"]);
        let sources = discover_sources_with(&[agent("a1", "codex", Some("ses-4"))], &catalog);

        assert_eq!(
            sources.len(),
            4,
            "three archived sessions plus the live one"
        );
        let paths: HashSet<_> = sources.iter().map(|source| source.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("/logs/ses-1.jsonl")));
        assert!(paths.contains(&PathBuf::from("/logs/ses-4.jsonl")));
    }

    #[test]
    fn an_opencode_agent_stays_one_source_carrying_every_session() {
        // The database is one file with one cursor, so it must not fan out into
        // a source per session; the id list is what narrows it to this agent.
        let catalog = StubCatalog::with("a1", &["ses_old", "ses_older"]);
        let sources = discover_sources_with(&[agent("a1", "opencode", Some("ses_live"))], &catalog);

        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].provider_session_ids,
            vec![
                "ses_live".to_string(),
                "ses_old".to_string(),
                "ses_older".to_string()
            ]
        );
    }

    #[test]
    fn an_opencode_agent_is_discovered_from_its_workspace_alone() {
        // The real failure this closes: an opencode agent with no archived
        // conversations and no live session resolved to nothing at all, so it
        // reported no work despite having run. Opencode stamps every session
        // with its directory, which attributes them without an id list.
        let catalog = WorkspaceCatalog;
        let mut agent = agent("a1", "opencode", None);
        agent.workspace = Some("D:/Development/Wardian".to_string());

        let sources = discover_sources_with(&[agent], &catalog);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].provider_session_ids, vec!["ses_by_directory"]);
    }

    /// Resolves sessions only by workspace, never by recorded id.
    struct WorkspaceCatalog;

    impl SessionCatalog for WorkspaceCatalog {
        fn codex_rollouts(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn claude_transcripts(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn pi_sessions(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn archive_turn_files(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn opencode_sessions(&self, _agent: &AgentDescriptor) -> Vec<String> {
            Vec::new()
        }
        fn opencode_sessions_in_workspace(&self, agent: &AgentDescriptor) -> Vec<String> {
            agent
                .workspace
                .as_ref()
                .map(|_| vec!["ses_by_directory".to_string()])
                .unwrap_or_default()
        }
        fn opencode_database(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/data/opencode/opencode.db"))
        }
    }

    /// Two agents in one directory, where only one can prove ownership.
    struct SharedWorkspaceCatalog;

    impl SessionCatalog for SharedWorkspaceCatalog {
        fn codex_rollouts(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn claude_transcripts(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn pi_sessions(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn archive_turn_files(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn opencode_sessions(&self, agent: &AgentDescriptor) -> Vec<String> {
            // Only "a2" has this session in its own archive.
            if agent.session_id == "a2" {
                vec!["ses_owned".to_string()]
            } else {
                Vec::new()
            }
        }
        fn opencode_sessions_in_workspace(&self, _agent: &AgentDescriptor) -> Vec<String> {
            vec!["ses_shared".to_string(), "ses_owned".to_string()]
        }
        fn opencode_database(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/data/opencode/opencode.db"))
        }
    }

    #[test]
    fn a_shared_workspace_does_not_give_two_agents_the_same_session() {
        // Rows are stored under whichever agent's source read them, so letting
        // both claim a session files the same turns twice and credits one
        // agent's work to its neighbour.
        let agents = vec![agent("a1", "opencode", None), agent("a2", "opencode", None)];
        let sources = discover_sources_with(&agents, &SharedWorkspaceCatalog);

        let mut claimed: Vec<String> = sources
            .iter()
            .flat_map(|source| source.provider_session_ids.clone())
            .collect();
        let total = claimed.len();
        claimed.sort();
        claimed.dedup();
        assert_eq!(claimed.len(), total, "a session was claimed twice");
        assert_eq!(total, 2, "both sessions are still attributed to someone");
    }

    #[test]
    fn a_recorded_session_outranks_a_directory_match() {
        // A workspace match only says a session ran in the same folder; an
        // agent's own archive says that agent ran it.
        let agents = vec![agent("a1", "opencode", None), agent("a2", "opencode", None)];
        let sources = discover_sources_with(&agents, &SharedWorkspaceCatalog);
        let owner = sources
            .iter()
            .find(|source| {
                source
                    .provider_session_ids
                    .iter()
                    .any(|id| id == "ses_owned")
            })
            .expect("someone owns it");
        assert_eq!(owner.session_id, "a2");
    }

    #[test]
    fn agents_sharing_one_database_are_all_discovered() {
        // Every opencode agent on the machine reads the same file. Deduping on
        // the path alone would silently ingest only the first agent's history.
        let agents = vec![
            agent("a1", "opencode", Some("ses_1")),
            agent("a2", "opencode", Some("ses_2")),
        ];
        let sources = discover_sources_with(&agents, &StubCatalog::default());
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].path, sources[1].path);
        assert_ne!(sources[0].session_id, sources[1].session_id);
    }

    #[test]
    fn a_duplicated_agent_is_only_discovered_once() {
        let duplicate = agent("a1", "codex", Some("ses-codex"));
        let agents = vec![duplicate.clone(), duplicate];
        assert_eq!(
            discover_sources_with(&agents, &StubCatalog::default()).len(),
            1
        );
    }

    /// A malformed/shared catalog can expose one physical rollout through two
    /// projected homes. Discovery must use the same physical source identity
    /// as the store, so it cannot schedule two ownership writes for that file.
    struct SharedCodexCatalog;

    impl SessionCatalog for SharedCodexCatalog {
        fn codex_rollouts(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            vec![PathBuf::from(
                "rollout-2026-09-04T00-00-00-019fef5a-e0ef-7011-bc3d-06581a3dfaac.jsonl",
            )]
        }

        fn claude_transcripts(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn pi_sessions(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn archive_turn_files(&self, _agent: &AgentDescriptor) -> Vec<PathBuf> {
            Vec::new()
        }
        fn opencode_sessions(&self, _agent: &AgentDescriptor) -> Vec<String> {
            Vec::new()
        }
        fn opencode_sessions_in_workspace(&self, _agent: &AgentDescriptor) -> Vec<String> {
            Vec::new()
        }
        fn opencode_database(&self) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn a_shared_codex_rollout_is_discovered_once_with_its_file_identity() {
        let agents = vec![
            agent("agent-a", "codex", None),
            agent("agent-b", "codex", None),
        ];
        let sources = discover_sources_with(&agents, &SharedCodexCatalog);
        assert!(
            sources.is_empty(),
            "an ambiguous shared transcript must not be assigned by roster order"
        );
    }

    #[test]
    fn verified_worker_source_wins_over_projected_home_claim() {
        let inferred = agent("agent-a", "codex", None);
        let mut verified = agent("worker-child", "codex", None);
        verified.verified_source_paths = vec![PathBuf::from(
            "rollout-2026-09-04T00-00-00-019fef5a-e0ef-7011-bc3d-06581a3dfaac.jsonl",
        )];

        let sources = discover_sources_with(&[inferred, verified], &SharedCodexCatalog);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].session_id, "worker-child");
    }

    #[test]
    fn off_agents_are_still_discovered() {
        // An agent's log holds work done while Wardian was closed. Filtering on
        // `is_off` would make recorded history depend on whether the app
        // happened to be running, which is exactly what this store exists to
        // stop being true. `is_off` informs cadence only.
        let mut off = agent("a1", "codex", Some("ses-codex"));
        off.is_off = true;
        let sources = discover_sources_with(&[off], &StubCatalog::default());
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].session_id, "a1");
    }

    #[test]
    fn an_agent_that_has_never_run_resolves_to_nothing() {
        // Not an error condition — an agent with no session yet simply has no
        // telemetry to read.
        for resume in [None, Some(""), Some("   ")] {
            let empty = StubCatalog::default();
            assert!(discover_sources_with(&[agent("a1", "codex", resume)], &empty).is_empty());
            assert!(discover_sources_with(&[agent("a1", "opencode", resume)], &empty).is_empty());
        }
    }

    #[test]
    fn a_rollout_filename_yields_its_session_id() {
        assert_eq!(
            transcript_session_id(Path::new(
                "rollout-2026-08-11T01-45-38-019fef5a-e0ef-7011-bc3d-06581a3dfaac.jsonl"
            ))
            .as_deref(),
            Some("019fef5a-e0ef-7011-bc3d-06581a3dfaac")
        );
        assert_eq!(transcript_session_id(Path::new("notes.txt")), None);
        assert_eq!(transcript_session_id(Path::new("short.jsonl")), None);
    }

    #[test]
    fn codex_child_discovery_follows_only_verified_parent_thread_ancestry() {
        let meta = |thread_id: &str, parent_thread_id: Option<&str>| CodexRolloutMeta {
            thread_id: thread_id.to_string(),
            parent_thread_id: parent_thread_id.map(str::to_string),
            path: PathBuf::from(format!("{thread_id}.jsonl")),
            state: wardian_core::temporary_workers::TemporaryWorkerState::Unknown,
            outcome: None,
            requested_at: None,
            terminal_at: None,
        };
        let metas = vec![
            meta("root", None),
            meta("child", Some("root")),
            meta("grandchild", Some("child")),
            meta("unrelated", Some("different-root")),
        ];
        let roots = ["root".to_string()].into_iter().collect();

        let descendants = verified_codex_descendants(&metas, &roots)
            .into_iter()
            .map(|(_, child)| child.thread_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(descendants, vec!["child", "grandchild"]);
    }

    #[test]
    fn codex_rollout_meta_reads_raw_thread_spawn_shape_and_terminal_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rollout.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"root\",\"depth\":1}}}}}\n",
                "{\"timestamp\":\"2026-09-13T00:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();

        let meta = read_codex_rollout_meta(&path).unwrap();

        assert_eq!(meta.thread_id, "child");
        assert_eq!(meta.parent_thread_id.as_deref(), Some("root"));
        assert_eq!(
            meta.state,
            wardian_core::temporary_workers::TemporaryWorkerState::Succeeded
        );
        assert_eq!(meta.terminal_at.as_deref(), Some("2026-09-13T00:01:00Z"));
    }

    #[test]
    fn idle_and_active_cadences_differ() {
        assert_eq!(next_interval(true, true, 0), INGEST_INTERVAL_ACTIVE);
        assert_eq!(next_interval(true, false, 0), INGEST_INTERVAL_IDLE);
        assert!(next_interval(true, false, 0) > next_interval(true, true, 0));
        assert_eq!(next_interval(false, false, 0), INGEST_INTERVAL_ACTIVE);
    }

    #[test]
    fn an_unfinished_backfill_outranks_both_steady_cadences() {
        // Waiting a full interval between bounded chunks would turn a large
        // history into one that takes days to become true.
        assert_eq!(next_interval(true, false, 12), INGEST_INTERVAL_BACKFILL);
        assert!(next_interval(true, false, 12) < next_interval(true, true, 0));
    }

    #[test]
    fn retention_only_runs_after_its_in_memory_deadline() {
        let now = Instant::now();
        let elapsed_deadline = now.checked_sub(Duration::from_secs(1)).unwrap();

        assert!(telemetry_maintenance_is_due(now, elapsed_deadline));
        assert!(!telemetry_maintenance_is_due(
            now,
            now + Duration::from_secs(1)
        ));
    }

    #[test]
    fn background_discovery_identity_tracks_topology_not_liveness() {
        let first = agent("a1", "codex", Some("session-one"));
        let mut off = first.clone();
        off.is_off = true;

        assert_eq!(discovery_identity(&[first]), discovery_identity(&[off]));

        let changed = agent("a1", "codex", Some("session-two"));
        assert_ne!(
            discovery_identity(&[changed]),
            discovery_identity(&[agent("a1", "codex", Some("session-one"))])
        );
    }

    #[test]
    fn background_discovery_is_reused_then_boundedly_refreshed() {
        let now = Instant::now();
        let agents = vec![agent("a1", "codex", Some("session-one"))];
        let identity = discovery_identity(&agents);
        let cache = BackgroundDiscoveryCache {
            identity: identity.clone(),
            sources: Vec::new(),
            discovered_at: Some(now),
        };

        assert!(!cache.should_rediscover(&identity, now + INGEST_INTERVAL_ACTIVE));
        assert!(cache.should_rediscover(&identity, now + DISCOVERY_REFRESH_INTERVAL));

        let changed = discovery_identity(&[agent("a1", "codex", Some("session-two"))]);
        assert!(cache.should_rediscover(&changed, now));
    }

    #[test]
    fn identical_physical_provider_roots_are_recognized_as_shared() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();

        assert!(projected_root_matches_shared(dir.path(), dir.path()));
        assert!(!projected_root_matches_shared(dir.path(), &other));
    }

    #[test]
    fn an_empty_pass_reports_no_change() {
        let report = run_ingest_pass(&[]);
        assert_eq!(report.sources, 0);
        assert!(!report.changed());
        assert!(report.failures.is_empty());
    }

    #[test]
    fn busy_and_missing_sources_are_not_worth_logging() {
        // These are the steady state for an agent that has written nothing, and
        // logging them each minute would bury the failures that matter.
        assert!(!failure_is_noteworthy(&SourceError::Busy("locked".into())));
        assert!(!failure_is_noteworthy(&SourceError::Unavailable(
            "not found".into()
        )));
        assert!(failure_is_noteworthy(&SourceError::Read(
            "malformed record".into()
        )));
    }

    #[test]
    fn a_store_failure_is_always_reported() {
        // A busy source is weather; a failing write is a defect. Counting the
        // second one silently alongside the first would let the store fail on
        // every pass without anything ever saying so.
        assert!(is_reportable(&IngestError::Store("disk full".into())));
        assert!(is_reportable(&IngestError::UnsupportedProvider(
            "gemini".into()
        )));
        assert!(is_reportable(&IngestError::Source(SourceError::Read(
            "malformed record".into()
        ))));
        assert!(!is_reportable(&IngestError::Source(SourceError::Busy(
            "locked".into()
        ))));
    }
}
