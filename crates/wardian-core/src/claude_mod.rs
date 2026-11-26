//! Generation-scoped Claude plugin delivery. Files are evidence, never a retry timer.
//!
//! The application owns process containment and durable delivery state. In particular,
//! an implementation of [`GenerationExitVerifier`] is a trusted application boundary,
//! not a cryptographically authenticated exit receipt.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atomic_file;
use crate::native_transport::{NativeDeliveryPhase, NativeMessageEnvelope};

const MAX_JS_INTEGER: u64 = 9_007_199_254_740_991;
const PLUGIN_PREFIX: &str = "The wardian plugin sent a message:";

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn validate_id(id: &str) -> io::Result<()> {
    // Portable single path components, including on Windows (no device names).
    let stem = id.split('.').next().unwrap_or("").to_ascii_uppercase();
    if id.is_empty()
        || id.len() > 200
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(invalid("invalid portable delivery identifier"));
    }
    Ok(())
}

/// Immutable launch data read by the generation's bundled mod.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeModLaunchConfig {
    pub schema: u32,
    pub agent_id: String,
    pub runtime_generation: u64,
    pub expected_claude_session_id: String,
    pub inbox: PathBuf,
}

/// An application-embedded plugin asset. Paths are relative to `mod/`.
pub struct ModAsset<'a> {
    pub relative_path: &'a str,
    pub contents: &'a [u8],
}

/// Generation namespace and launch environment preparation, before process spawn.
#[derive(Debug, Clone)]
pub struct ClaudeModLaunchPlan {
    config: ClaudeModLaunchConfig,
}

impl ClaudeModLaunchPlan {
    /// Return the highest numeric generation already present for this agent.
    ///
    /// Generation folders are immutable audit namespaces, so allocation must
    /// advance past every one left by an earlier app process.
    pub fn highest_existing_generation(wardian_home: &Path, agent_id: &str) -> io::Result<u64> {
        validate_id(agent_id)?;
        if !wardian_home.is_absolute() {
            return Err(invalid("absolute home required"));
        }
        let delivery_root = wardian_home
            .join("agents")
            .join(agent_id)
            .join("claude/delivery");
        let mut entries = match fs::read_dir(delivery_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };

        entries.try_fold(0, |highest, entry| {
            let entry = entry?;
            let generation = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
                .unwrap_or(0);
            Ok(highest.max(generation))
        })
    }

    /// Compute a fresh namespace; this does not mutate disk.
    pub fn new(
        wardian_home: &Path,
        agent_id: &str,
        generation: u64,
        expected_session_id: &str,
    ) -> io::Result<Self> {
        validate_id(agent_id)?;
        validate_id(expected_session_id)?;
        if !wardian_home.is_absolute() || generation == 0 || generation > MAX_JS_INTEGER {
            return Err(invalid(
                "absolute home and JavaScript-safe nonzero generation required",
            ));
        }
        Ok(Self {
            config: ClaudeModLaunchConfig {
                schema: 1,
                agent_id: agent_id.to_owned(),
                runtime_generation: generation,
                expected_claude_session_id: expected_session_id.to_owned(),
                inbox: wardian_home
                    .join("agents")
                    .join(agent_id)
                    .join("claude/delivery")
                    .join(generation.to_string())
                    .join("inbox"),
            },
        })
    }

    pub fn config(&self) -> &ClaudeModLaunchConfig {
        &self.config
    }

    pub fn inbox(&self) -> &Path {
        &self.config.inbox
    }

    pub fn mod_dir(&self) -> PathBuf {
        self.config
            .inbox
            .parent()
            .expect("generation directory")
            .join("mod")
    }

    /// Publish embedded assets and launch data without rewriting existing content.
    /// Call only before launch, with a single owner for this generation. A failed
    /// materialization must never be followed by a process spawn.
    pub fn materialize(&self, assets: &[ModAsset<'_>]) -> io::Result<()> {
        let mut names = BTreeSet::new();
        for asset in assets {
            let path = Path::new(asset.relative_path);
            if asset.relative_path.is_empty()
                || asset.relative_path.contains('\\')
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || path == Path::new("launch.json")
                || !names.insert(path)
            {
                return Err(invalid("invalid or duplicate plugin asset path"));
            }
        }
        // Compare everything first so a mismatch never updates another asset.
        let mut files: Vec<(PathBuf, Vec<u8>)> = assets
            .iter()
            .map(|a| (self.mod_dir().join(a.relative_path), a.contents.to_vec()))
            .collect();
        let mut launch = serde_json::to_vec_pretty(&self.config).map_err(io::Error::other)?;
        launch.push(b'\n');
        files.push((self.mod_dir().join("launch.json"), launch));
        for (path, contents) in &files {
            match fs::read(path) {
                Ok(existing) if existing != *contents => {
                    return Err(invalid("generation content differs"))
                }
                Ok(_) => (),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        for directory in [
            "pending", "claimed", "queued", "dropped", "rejected", "sessions",
        ] {
            fs::create_dir_all(self.inbox().join(directory))?;
        }
        for (path, contents) in files {
            if !path.try_exists()? {
                atomic_file::write_bytes_atomic_durable(&path, &contents)?;
            }
        }
        Ok(())
    }

    /// Append the mod directory using the platform path-list encoding.
    pub fn plugin_dirs(&self, existing: Option<&OsStr>) -> io::Result<OsString> {
        let mut paths = existing
            .filter(|v| !v.is_empty())
            .map(|v| std::env::split_paths(v).collect::<Vec<_>>())
            .unwrap_or_default();
        paths.push(self.mod_dir());
        std::env::join_paths(paths).map_err(io::Error::other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    ModNotLoaded,
    SessionMismatch,
    HeartbeatStale,
    GenerationMismatch,
}

impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModNotLoaded => "mod_not_loaded",
            Self::SessionMismatch => "session_mismatch",
            Self::HeartbeatStale => "heartbeat_stale",
            Self::GenerationMismatch => "generation_mismatch",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadyRecord {
    pub generation: u64,
    pub session_id: String,
    pub mod_version: String,
    #[serde(default)]
    pub supports_as_user: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatRecord {
    pub generation: u64,
    pub session_id: String,
    #[serde(with = "epoch_millis")]
    pub at: DateTime<Utc>,
}

// Match the bundled mod's Date.now(): integer Unix epoch milliseconds.
// The workspace deliberately does not enable chrono's optional serde feature.
mod epoch_millis {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(value.timestamp_millis())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let millis = i64::deserialize(deserializer)?;
        DateTime::from_timestamp_millis(millis)
            .ok_or_else(|| serde::de::Error::custom("heartbeat epoch milliseconds out of range"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRecord {
    pub generation: u64,
    pub session_id: String,
}

/// Complete bound mod outcome. Claim contents are deliberately never parsed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutcomeRecord {
    pub generation: u64,
    pub interaction_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasonRecord {
    #[serde(flatten)]
    pub outcome: OutcomeRecord,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingDelivery {
    pub schema: u32,
    pub interaction_id: String,
    pub runtime_generation: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub as_user: bool,
}

/// A projection, not a durable transition. Proof-bearing recovery is separate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryProjection {
    pub phase: NativeDeliveryPhase,
    pub detail: Option<String>,
    pub claim_exists: bool,
}

/// Only this module's evidence resolver can create a recovery proof.
///
/// ```compile_fail
/// use wardian_core::claude_mod::NonSubmissionProof;
/// let proof = NonSubmissionProof { agent_id: "a".into(), generation: 1, interaction_id: "i".into() };
/// ```
#[derive(Debug)]
pub struct NonSubmissionProof {
    agent_id: String,
    generation: u64,
    interaction_id: String,
}

impl NonSubmissionProof {
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn interaction_id(&self) -> &str {
        &self.interaction_id
    }
}

/// Trusted adapter for the application's verified containment receipt.
///
/// Return true only after the exact generation's kill-on-close containment job
/// has proved the whole process tree exited. PID absence, a root exit, heartbeat
/// age or an unverified boolean are insufficient. Core cannot authenticate an
/// app-owned receipt: implementors are responsible for this evidence boundary.
pub trait GenerationExitVerifier {
    fn process_tree_exited(&self, agent_id: &str, generation: u64) -> bool;
}

/// Reader/publisher bound to one immutable launch configuration.
pub struct ClaudeModResolver {
    config: ClaudeModLaunchConfig,
    last_heartbeat: Option<HeartbeatRecord>,
}

// Missing/torn records are not evidence. Other filesystem errors must not be
// mistaken for absence, particularly when checking claims for safe recovery.
fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn exists(path: &Path) -> io::Result<bool> {
    // symlink_metadata counts dangling links and directories as claims too.
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

impl ClaudeModResolver {
    /// Reopen only a canonical generation inbox whose launch data matches its path.
    pub fn open(inbox: &Path) -> io::Result<Self> {
        if !inbox.is_absolute() || inbox.file_name() != Some(OsStr::new("inbox")) {
            return Err(invalid("absolute generation inbox required"));
        }
        let root = inbox
            .parent()
            .ok_or_else(|| invalid("missing generation"))?;
        let config: ClaudeModLaunchConfig = read_record(&root.join("mod/launch.json"))?
            .ok_or_else(|| invalid("missing or incomplete launch config"))?;
        let delivery = root
            .parent()
            .ok_or_else(|| invalid("missing delivery directory"))?;
        let claude = delivery
            .parent()
            .ok_or_else(|| invalid("missing Claude directory"))?;
        let agent = claude
            .parent()
            .ok_or_else(|| invalid("missing agent directory"))?;
        validate_id(&config.agent_id)?;
        validate_id(&config.expected_claude_session_id)?;
        if config.schema != 1
            || config.runtime_generation == 0
            || config.runtime_generation > MAX_JS_INTEGER
            || config.inbox != inbox
            || root.file_name() != Some(OsStr::new(&config.runtime_generation.to_string()))
            || delivery.file_name() != Some(OsStr::new("delivery"))
            || claude.file_name() != Some(OsStr::new("claude"))
            || agent.file_name() != Some(OsStr::new(&config.agent_id))
            || agent.parent().and_then(Path::file_name) != Some(OsStr::new("agents"))
        {
            return Err(invalid("launch config does not bind this namespace"));
        }
        Ok(Self {
            config,
            last_heartbeat: None,
        })
    }

    pub fn config(&self) -> &ClaudeModLaunchConfig {
        &self.config
    }

    /// Readiness affects only new publication. Retain the last complete heartbeat
    /// through torn rewrites, but never refresh its timestamp artificially.
    pub fn readiness(
        &mut self,
        now: DateTime<Utc>,
        tracked_session_ids: &BTreeSet<String>,
        as_user: bool,
    ) -> io::Result<Result<(), FallbackReason>> {
        let Some(ready) = read_record::<ReadyRecord>(&self.config.inbox.join("ready.json"))? else {
            return Ok(Err(FallbackReason::ModNotLoaded));
        };
        if ready.generation != self.config.runtime_generation {
            return Ok(Err(FallbackReason::GenerationMismatch));
        }
        if ready.session_id != self.config.expected_claude_session_id {
            return Ok(Err(FallbackReason::SessionMismatch));
        }
        if ready.mod_version.trim().is_empty() || (as_user && !ready.supports_as_user) {
            return Ok(Err(FallbackReason::ModNotLoaded));
        }
        if let Some(heartbeat) =
            read_record::<HeartbeatRecord>(&self.config.inbox.join("heartbeat.json"))?
        {
            self.last_heartbeat = Some(heartbeat);
        }
        let Some(heartbeat) = &self.last_heartbeat else {
            return Ok(Err(FallbackReason::HeartbeatStale));
        };
        if heartbeat.generation != self.config.runtime_generation {
            return Ok(Err(FallbackReason::GenerationMismatch));
        }
        if heartbeat.session_id != self.config.expected_claude_session_id
            && !tracked_session_ids.contains(&heartbeat.session_id)
        {
            return Ok(Err(FallbackReason::SessionMismatch));
        }
        let age = now.signed_duration_since(heartbeat.at);
        if age < chrono::Duration::zero() || age > chrono::Duration::seconds(5) {
            return Ok(Err(FallbackReason::HeartbeatStale));
        }
        Ok(Ok(()))
    }

    /// Publish complete JSON atomically. The application serializes publication
    /// per generation and supplies monotonically increasing sequence numbers.
    /// A repeated identical publication is idempotent; changed payloads refuse.
    pub fn publish(&self, seq: u64, id: &str, text: &str, as_user: bool) -> io::Result<PathBuf> {
        validate_id(id)?;
        if seq > MAX_JS_INTEGER {
            return Err(invalid("sequence exceeds JavaScript-safe range"));
        }
        let delivery = PendingDelivery {
            schema: 1,
            interaction_id: id.to_owned(),
            runtime_generation: self.config.runtime_generation,
            text: text.to_owned(),
            as_user,
        };
        let path = self
            .config
            .inbox
            .join("pending")
            .join(format!("{seq:020}-{id}.json"));
        // Never publish the same interaction under a new sequence, even if a
        // previous caller lost its publication acknowledgement.
        for entry in fs::read_dir(self.config.inbox.join("pending"))? {
            let entry = entry?;
            let name = entry.file_name();
            let duplicate = name
                .to_str()
                .and_then(|name| name.split_once('-'))
                .and_then(|(_, rest)| rest.strip_suffix(".json"))
                == Some(id);
            if duplicate {
                if entry.path() == path
                    && read_record::<PendingDelivery>(&path)?.as_ref() == Some(&delivery)
                {
                    return Ok(path);
                }
                return Err(invalid("interaction already published"));
            }
        }
        atomic_file::write_json_atomic(&path, &delivery)?;
        Ok(path)
    }

    fn record_path(&self, kind: &str, id: &str) -> PathBuf {
        self.config.inbox.join(kind).join(format!("{id}.json"))
    }

    fn bound(&self, record: &OutcomeRecord, id: &str) -> bool {
        record.generation == self.config.runtime_generation
            && record.interaction_id == id
            && validate_id(&record.session_id).is_ok()
    }

    fn outcome(&self, kind: &str, id: &str) -> io::Result<Option<OutcomeRecord>> {
        Ok(
            read_record::<OutcomeRecord>(&self.record_path(kind, id))?
                .filter(|r| self.bound(r, id)),
        )
    }

    fn reason(&self, kind: &str, id: &str) -> io::Result<Option<ReasonRecord>> {
        Ok(read_record::<ReasonRecord>(&self.record_path(kind, id))?
            .filter(|r| self.bound(&r.outcome, id) && !r.reason.trim().is_empty()))
    }

    /// A complete claim proves this interaction was picked up by this mod
    /// generation. A torn claim still prevents replay without proving pickup.
    pub fn has_bound_claim(&self, id: &str) -> io::Result<bool> {
        validate_id(id)?;
        Ok(self.outcome("claimed", id)?.is_some())
    }

    /// Check the complete claim and the exact owned session in this generation.
    /// Session switches use the same generation's keyed history record.
    pub fn has_composer_source_claim(&self, id: &str, session_id: &str) -> io::Result<bool> {
        validate_id(id)?;
        validate_id(session_id)?;
        let Some(claim) = self.outcome("claimed", id)? else {
            return Ok(false);
        };
        if claim.session_id == session_id || self.config.expected_claude_session_id == session_id {
            return Ok(true);
        }
        Ok(read_record::<SessionRecord>(
            &self
                .config
                .inbox
                .join("sessions")
                .join(format!("{session_id}.json")),
        )?
        .is_some_and(|record| {
            record.generation == self.config.runtime_generation && record.session_id == session_id
        }))
    }

    /// Conflicting outcomes fail closed. A lone claim or rejection remains
    /// uncertain until transcript evidence or the proof-bearing resolver acts.
    pub fn project(&self, id: &str) -> io::Result<DeliveryProjection> {
        validate_id(id)?;
        let claim_exists = exists(&self.record_path("claimed", id))?;
        let queued = self.outcome("queued", id)?;
        let dropped = self.reason("dropped", id)?;
        let rejected = self.reason("rejected", id)?;
        let count = usize::from(queued.is_some())
            + usize::from(dropped.is_some())
            + usize::from(rejected.is_some());
        let (phase, detail) = if count > 1 || (claim_exists && rejected.is_some()) {
            (
                NativeDeliveryPhase::SubmittedUnconfirmed,
                Some("conflicting mod records".into()),
            )
        } else if queued.is_some() {
            (NativeDeliveryPhase::ProviderAccepted, None)
        } else if let Some(record) = dropped {
            (NativeDeliveryPhase::Failed, Some(record.reason))
        } else {
            (
                NativeDeliveryPhase::SubmittedUnconfirmed,
                rejected.map(|r| r.reason),
            )
        };
        Ok(DeliveryProjection {
            phase,
            detail,
            claim_exists,
        })
    }

    fn proof(&self, id: &str) -> NonSubmissionProof {
        NonSubmissionProof {
            agent_id: self.config.agent_id.clone(),
            generation: self.config.runtime_generation,
            interaction_id: id.to_owned(),
        }
    }

    fn recovery_blocked(&self, id: &str) -> io::Result<bool> {
        // Even torn outcomes conflict with a no-submit claim. Only claim absence
        // after verified exit, or a consistent complete rejection, is sufficient.
        Ok(exists(&self.record_path("claimed", id))?
            || exists(&self.record_path("queued", id))?
            || exists(&self.record_path("dropped", id))?)
    }

    /// Validate the full rejection binding and rule out contradictory evidence.
    pub fn resolve_rejected(&self, id: &str) -> io::Result<Option<NonSubmissionProof>> {
        validate_id(id)?;
        if self.reason("rejected", id)?.is_none() || self.recovery_blocked(id)? {
            return Ok(None);
        }
        Ok(Some(self.proof(id)))
    }

    /// The adapter must verify the exact generation's entire process tree exited.
    /// Claim absence is checked *after* that verification. Without closure a live
    /// mod could create a claim immediately after any filesystem check.
    pub fn resolve_after_verified_exit(
        &self,
        id: &str,
        verifier: &impl GenerationExitVerifier,
    ) -> io::Result<Option<NonSubmissionProof>> {
        validate_id(id)?;
        if !verifier.process_tree_exited(&self.config.agent_id, self.config.runtime_generation)
            || self.recovery_blocked(id)?
        {
            return Ok(None);
        }
        Ok(Some(self.proof(id)))
    }

    /// Union complete, path-bound mod history with application-tracked sessions.
    /// Claim/outcome session IDs deliberately do not restrict attribution.
    pub fn session_history(
        &self,
        tracked_session_ids: &BTreeSet<String>,
    ) -> io::Result<BTreeSet<String>> {
        let mut sessions = tracked_session_ids.clone();
        sessions.insert(self.config.expected_claude_session_id.clone());
        for entry in fs::read_dir(self.config.inbox.join("sessions"))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if let Some(record) = read_record::<SessionRecord>(&entry.path())? {
                if record.generation == self.config.runtime_generation
                    && validate_id(&record.session_id).is_ok()
                    && entry.file_name() == OsStr::new(&format!("{}.json", record.session_id))
                {
                    sessions.insert(record.session_id);
                }
            }
        }
        Ok(sessions)
    }
}

/// Match only a user plugin row in an attributed transcript session, using an
/// exact identifier token (never a substring). Pass the session of the source
/// transcript file; an embedded sessionId, when present, must agree.
pub fn transcript_row_matches(
    row: &Value,
    session_id: &str,
    session_history: &BTreeSet<String>,
    interaction_id: &str,
) -> bool {
    if validate_id(interaction_id).is_err() {
        return false;
    }
    let Some(text) = transcript_user_text(row, session_id, session_history) else {
        return false;
    };
    let Some(payload) = text.strip_prefix(PLUGIN_PREFIX) else {
        return false;
    };
    payload
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .any(|token| token == interaction_id)
}

/// Check the complete composer envelope in a user row from an attributed
/// Claude transcript. The caller supplies the mod's claimed delivery.
pub fn composer_transcript_row_matches(
    row: &Value,
    session_id: &str,
    session_history: &BTreeSet<String>,
    envelope: &NativeMessageEnvelope,
) -> bool {
    session_history.contains(session_id)
        && composer_transcript_token(row, session_id).is_some_and(|(id, header)| {
            id == envelope.interaction_id && header == envelope.transcript_header()
        })
}

/// Read the interaction selector and complete literal envelope from an owned
/// Claude file record. A caller must still verify the durable delivery and claim.
pub fn composer_transcript_token(row: &Value, session_id: &str) -> Option<(String, String)> {
    let other_plugin = row
        .pointer("/origin/name")
        .and_then(Value::as_str)
        .is_some_and(|name| name != "wardian");
    if row.get("isMeta").and_then(Value::as_bool) == Some(true)
        || row.get("isSidechain").and_then(Value::as_bool) == Some(true)
        || other_plugin
    {
        return None;
    }
    let text = user_text_in_session(row, session_id)?;
    // Some rows still carry the plugin frame. The bound claim and first-line
    // envelope supply the same identity in either representation.
    let payload = text
        .strip_prefix(PLUGIN_PREFIX)
        .map_or(text.as_str(), str::trim_start);
    let header = payload.lines().next()?;
    let (_, fields) = header
        .strip_prefix("[Wardian message_id=")?
        .split_once(" interaction_id=")?;
    let (interaction, _) = fields.split_once(" generation=")?;
    validate_id(interaction).ok()?;
    Some((interaction.to_owned(), header.to_owned()))
}

fn transcript_user_text(
    row: &Value,
    session_id: &str,
    session_history: &BTreeSet<String>,
) -> Option<String> {
    if !session_history.contains(session_id) {
        return None;
    }
    user_text_in_session(row, session_id)
}

fn user_text_in_session(row: &Value, session_id: &str) -> Option<String> {
    if row.get("type").and_then(Value::as_str) != Some("user")
        || row
            .get("sessionId")
            .is_some_and(|v| v.as_str() != Some(session_id))
        || row.pointer("/message/role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    match row.pointer("/message/content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => Some(
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup() -> (tempfile::TempDir, ClaudeModLaunchPlan, ClaudeModResolver) {
        let temp = tempfile::tempdir().unwrap();
        let plan = ClaudeModLaunchPlan::new(temp.path(), "agent-1", 7, "session-a").unwrap();
        plan.materialize(&[ModAsset {
            relative_path: "hooks/register.ts",
            contents: b"mod",
        }])
        .unwrap();
        let resolver = ClaudeModResolver::open(plan.inbox()).unwrap();
        (temp, plan, resolver)
    }

    fn write(resolver: &ClaudeModResolver, path: &str, value: Value) {
        fs::write(
            resolver.config.inbox.join(path),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }

    fn outcome() -> Value {
        json!({"generation":7,"interaction_id":"ask-1","session_id":"session-a"})
    }

    fn rejection() -> Value {
        json!({"generation":7,"interaction_id":"ask-1","session_id":"session-a","reason":"invalid payload"})
    }

    struct Exit(bool);
    impl GenerationExitVerifier for Exit {
        fn process_tree_exited(&self, agent: &str, generation: u64) -> bool {
            assert_eq!(agent, "agent-1");
            assert_eq!(generation, 7);
            self.0
        }
    }

    #[test]
    fn immutable_generation_materialization_and_path_binding() {
        let (temp, plan, _) = setup();
        let launch = plan.mod_dir().join("launch.json");
        let original = fs::read(&launch).unwrap();
        plan.materialize(&[ModAsset {
            relative_path: "hooks/register.ts",
            contents: b"mod",
        }])
        .unwrap();
        assert_eq!(fs::read(&launch).unwrap(), original);
        assert!(plan
            .materialize(&[ModAsset {
                relative_path: "hooks/register.ts",
                contents: b"changed"
            }])
            .is_err());
        assert_eq!(
            fs::read(plan.mod_dir().join("hooks/register.ts")).unwrap(),
            b"mod"
        );
        let next = ClaudeModLaunchPlan::new(temp.path(), "agent-1", 8, "session-a").unwrap();
        next.materialize(&[]).unwrap();
        assert_ne!(plan.inbox(), next.inbox());
        let mut config = plan.config().clone();
        config.expected_claude_session_id = "session-b".into();
        atomic_file::write_json_atomic(&launch, &config).unwrap();
        assert!(plan.materialize(&[]).is_err());
        config.runtime_generation = 8;
        atomic_file::write_json_atomic(&launch, &config).unwrap();
        assert!(ClaudeModResolver::open(plan.inbox()).is_err());
    }

    #[test]
    fn highest_existing_generation_ignores_non_numeric_entries() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            ClaudeModLaunchPlan::highest_existing_generation(temp.path(), "agent-1").unwrap(),
            0
        );
        let delivery = temp.path().join("agents/agent-1/claude/delivery");
        fs::create_dir_all(delivery.join("1")).unwrap();
        fs::create_dir_all(delivery.join("7")).unwrap();
        fs::write(delivery.join("pending"), b"not a generation").unwrap();

        assert_eq!(
            ClaudeModLaunchPlan::highest_existing_generation(temp.path(), "agent-1").unwrap(),
            7
        );
        assert!(ClaudeModLaunchPlan::highest_existing_generation(temp.path(), "../agent").is_err());
    }

    #[test]
    fn highest_existing_generation_rejects_an_unreadable_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join("agents/agent-1/claude");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("delivery"), b"not a directory").unwrap();

        assert!(ClaudeModLaunchPlan::highest_existing_generation(temp.path(), "agent-1").is_err());
    }

    #[test]
    fn rejects_path_escape_and_unsafe_numeric_values() {
        let temp = tempfile::tempdir().unwrap();
        for id in ["../agent", "a/b", "a\\b", "", "CON", "LPT1"] {
            assert!(ClaudeModLaunchPlan::new(temp.path(), id, 1, "session").is_err());
        }
        for generation in [0, MAX_JS_INTEGER + 1] {
            assert!(ClaudeModLaunchPlan::new(temp.path(), "agent", generation, "session").is_err());
        }
        let (_temp, plan, resolver) = setup();
        for path in ["../escape", "/absolute", "launch.json", "hooks\\escape"] {
            assert!(plan
                .materialize(&[ModAsset {
                    relative_path: path,
                    contents: b"x"
                }])
                .is_err());
        }
        assert!(resolver.publish(1, "../escape", "x", false).is_err());
        assert!(resolver
            .publish(MAX_JS_INTEGER + 1, "ask", "x", false)
            .is_err());
    }

    #[test]
    fn environment_appends_without_losing_existing_directories() {
        let (temp, plan, _) = setup();
        let first = temp.path().join("one");
        let second = temp.path().join("two");
        let prior = std::env::join_paths([&first, &second]).unwrap();
        let result = plan.plugin_dirs(Some(&prior)).unwrap();
        assert_eq!(
            std::env::split_paths(&result).collect::<Vec<_>>(),
            vec![first, second, plan.mod_dir()]
        );
        assert_eq!(
            std::env::split_paths(&plan.plugin_dirs(None).unwrap()).collect::<Vec<_>>(),
            vec![plan.mod_dir()]
        );
    }

    #[test]
    fn atomic_publication_orders_and_never_rewrites_an_interaction() {
        let (_temp, _plan, resolver) = setup();
        let path = resolver.publish(9, "ask-1", "exact\ntext", false).unwrap();
        resolver.publish(10, "ask-2", "second", false).unwrap();
        let bytes = fs::read(&path).unwrap();
        let value: PendingDelivery = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value.runtime_generation, 7);
        assert_eq!(value.text, "exact\ntext");
        assert_eq!(
            resolver.publish(9, "ask-1", "exact\ntext", false).unwrap(),
            path
        );
        assert!(resolver.publish(10, "ask-1", "exact\ntext", false).is_err());
        assert!(resolver.publish(9, "ask-1", "changed", false).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let mut names = fs::read_dir(resolver.config.inbox.join("pending"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names.len(), 2);
        assert!(names[0].to_string_lossy().ends_with("-ask-1.json"));
        assert!(names[1].to_string_lossy().ends_with("-ask-2.json"));
        resolver
            .publish(11, "prefix-ask-3", "distinct", false)
            .unwrap();
        resolver
            .publish(12, "ask-3", "also distinct", false)
            .unwrap();
    }

    fn ready(resolver: &ClaudeModResolver, now: DateTime<Utc>) {
        write(
            resolver,
            "ready.json",
            json!({"generation":7,"session_id":"session-a","mod_version":"1"}),
        );
        write(
            resolver,
            "heartbeat.json",
            json!({"generation":7,"session_id":"session-a","at":now.timestamp_millis()}),
        );
    }

    #[test]
    fn composer_requires_a_capable_ready_record_and_the_existing_heartbeat() {
        let (_temp, _plan, mut resolver) = setup();
        let now = DateTime::from_timestamp_millis(1794484800123).unwrap();
        let tracked = BTreeSet::new();
        ready(&resolver, now);
        assert_eq!(resolver.readiness(now, &tracked, false).unwrap(), Ok(()));
        assert_eq!(
            resolver.readiness(now, &tracked, true).unwrap(),
            Err(FallbackReason::ModNotLoaded)
        );
        write(
            &resolver,
            "ready.json",
            json!({"generation":7,"session_id":"session-a","mod_version":"0.2.0","supports_as_user":true}),
        );
        assert_eq!(resolver.readiness(now, &tracked, true).unwrap(), Ok(()));
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::seconds(6), &tracked, true)
                .unwrap(),
            Err(FallbackReason::HeartbeatStale)
        );

        let path = resolver
            .publish(1, "composer-1", "tagged text", true)
            .unwrap();
        let value: PendingDelivery = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(value.as_user);
        assert!(resolver
            .publish(1, "composer-1", "tagged text", false)
            .is_err());
        assert!(!resolver.has_bound_claim("composer-1").unwrap());
        write(
            &resolver,
            "claimed/composer-1.json",
            json!({"generation":8,"interaction_id":"composer-1","session_id":"session-a"}),
        );
        assert!(!resolver.has_bound_claim("composer-1").unwrap());
        write(
            &resolver,
            "claimed/composer-1.json",
            json!({"generation":7,"interaction_id":"composer-1","session_id":"session-a"}),
        );
        assert!(resolver.has_bound_claim("composer-1").unwrap());
    }

    #[test]
    fn composer_receipt_requires_exact_envelope_in_an_attributed_user_row() {
        let envelope = NativeMessageEnvelope {
            interaction_id: "composer-1".into(),
            message_id: "composer-1".into(),
            target_agent_id: "agent-1".into(),
            sender_agent_id: None,
            parent_interaction_id: None,
            caller_idempotency_key: None,
            generation: 7,
            operation: crate::native_transport::NativeMessageOperation::StartTurn,
            deadline_at: None,
            body: "same visible text".into(),
        };
        let history = BTreeSet::from(["session-a".to_string()]);
        let content = format!("{}\nsame visible text", envelope.transcript_header());
        let mut row = json!({
            "type":"user","sessionId":"session-a",
            "origin":{"kind":"plugin","name":"wardian","asUser":true},
            "message":{"role":"user","content":content},
        });
        assert!(composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
        assert!(!composer_transcript_row_matches(
            &row,
            "session-b",
            &history,
            &envelope
        ));
        row["origin"]["name"] = json!("other");
        assert!(!composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
        row["message"]["content"] = json!(format!("{PLUGIN_PREFIX}\n{content}"));
        assert!(!composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
        row.as_object_mut().unwrap().remove("origin");
        assert!(composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
        row["message"]["content"] = json!(content);
        assert!(composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
        row["message"]["content"] = json!(format!(
            "{PLUGIN_PREFIX}\n{}",
            content.replace("generation=7", "generation=8")
        ));
        assert!(!composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
        row["message"]["content"] =
            json!(format!("{PLUGIN_PREFIX}\ncomposer-1\nsame visible text"));
        assert!(!composer_transcript_row_matches(
            &row,
            "session-a",
            &history,
            &envelope
        ));
    }

    #[test]
    fn producer_epoch_millis_heartbeat_obeys_five_second_bound_and_torn_cache() {
        let (_temp, _plan, mut resolver) = setup();
        // Exact register.ts wire shape: JSON.stringify({ schema: 1, ..., at: Date.now() }).
        let wire =
            json!({"schema":1,"generation":7,"session_id":"session-a","at":1794484800123_i64});
        let now = DateTime::from_timestamp_millis(1794484800123).unwrap();
        ready(&resolver, now);
        write(&resolver, "heartbeat.json", wire.clone());
        let parsed: HeartbeatRecord = serde_json::from_value(wire).unwrap();
        assert_eq!(parsed.at, now);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["at"],
            json!(1794484800123_i64)
        );
        let tracked = BTreeSet::new();
        assert_eq!(resolver.readiness(now, &tracked, false).unwrap(), Ok(()));
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::milliseconds(5000), &tracked, false)
                .unwrap(),
            Ok(())
        );
        fs::write(
            resolver.config.inbox.join("heartbeat.json"),
            b"{\"schema\":1,\"at\":179448",
        )
        .unwrap();
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::milliseconds(5000), &tracked, false)
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::milliseconds(5001), &tracked, false)
                .unwrap(),
            Err(FallbackReason::HeartbeatStale)
        );
        // A fresh reader cannot use a torn record without an earlier complete sample.
        let mut reopened = ClaudeModResolver::open(&resolver.config.inbox).unwrap();
        assert_eq!(
            reopened.readiness(now, &tracked, false).unwrap(),
            Err(FallbackReason::HeartbeatStale)
        );
    }

    #[test]
    fn heartbeat_rejects_non_integer_and_out_of_range_timestamps() {
        for at in [
            json!("2026-11-12T12:00:00Z"),
            json!(1.5),
            json!(null),
            json!(i64::MAX),
        ] {
            assert!(serde_json::from_value::<HeartbeatRecord>(
                json!({"generation":7,"session_id":"session-a","at":at})
            )
            .is_err());
        }
    }

    #[test]
    fn readiness_covers_fallback_codes_and_session_switch() {
        let (_temp, _plan, mut resolver) = setup();
        let now = DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut tracked = BTreeSet::new();
        assert_eq!(
            resolver.readiness(now, &tracked, false).unwrap(),
            Err(FallbackReason::ModNotLoaded)
        );
        ready(&resolver, now);
        assert_eq!(resolver.readiness(now, &tracked, false).unwrap(), Ok(()));
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::seconds(5), &tracked, false)
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::milliseconds(5001), &tracked, false)
                .unwrap(),
            Err(FallbackReason::HeartbeatStale)
        );
        assert_eq!(
            resolver
                .readiness(now - chrono::Duration::seconds(1), &tracked, false)
                .unwrap(),
            Err(FallbackReason::HeartbeatStale)
        );
        write(
            &resolver,
            "heartbeat.json",
            json!({"generation":8,"session_id":"session-a","at":now.timestamp_millis()}),
        );
        assert_eq!(
            resolver.readiness(now, &tracked, false).unwrap(),
            Err(FallbackReason::GenerationMismatch)
        );
        write(
            &resolver,
            "heartbeat.json",
            json!({"generation":7,"session_id":"session-b","at":now.timestamp_millis()}),
        );
        assert_eq!(
            resolver.readiness(now, &tracked, false).unwrap(),
            Err(FallbackReason::SessionMismatch)
        );
        tracked.insert("session-b".into());
        assert_eq!(resolver.readiness(now, &tracked, false).unwrap(), Ok(()));
        write(
            &resolver,
            "ready.json",
            json!({"generation":7,"session_id":"session-b","mod_version":"1"}),
        );
        assert_eq!(
            resolver.readiness(now, &tracked, false).unwrap(),
            Err(FallbackReason::SessionMismatch)
        );
        write(
            &resolver,
            "ready.json",
            json!({"generation":8,"session_id":"session-a","mod_version":"1"}),
        );
        assert_eq!(
            resolver.readiness(now, &tracked, false).unwrap(),
            Err(FallbackReason::GenerationMismatch)
        );
        for (reason, code) in [
            (FallbackReason::ModNotLoaded, "mod_not_loaded"),
            (FallbackReason::SessionMismatch, "session_mismatch"),
            (FallbackReason::HeartbeatStale, "heartbeat_stale"),
            (FallbackReason::GenerationMismatch, "generation_mismatch"),
        ] {
            assert_eq!(reason.as_str(), code);
        }
    }

    #[test]
    fn torn_heartbeat_keeps_last_complete_value_without_extending_freshness() {
        let (_temp, _plan, mut resolver) = setup();
        let now = Utc::now();
        ready(&resolver, now);
        assert_eq!(
            resolver.readiness(now, &BTreeSet::new(), false).unwrap(),
            Ok(())
        );
        fs::write(
            resolver.config.inbox.join("heartbeat.json"),
            b"{\"generation\":",
        )
        .unwrap();
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::seconds(4), &BTreeSet::new(), false)
                .unwrap(),
            Ok(())
        );
        assert_eq!(
            resolver
                .readiness(now + chrono::Duration::seconds(6), &BTreeSet::new(), false)
                .unwrap(),
            Err(FallbackReason::HeartbeatStale)
        );
    }

    #[test]
    fn every_record_combination_has_conservative_projection_and_proof() {
        for bits in 0..16 {
            let (_temp, _plan, resolver) = setup();
            let claim = bits & 1 != 0;
            let queued = bits & 2 != 0;
            let dropped = bits & 4 != 0;
            let rejected = bits & 8 != 0;
            if claim {
                fs::write(resolver.record_path("claimed", "ask-1"), b"").unwrap();
            }
            if queued {
                write(&resolver, "queued/ask-1.json", outcome());
            }
            if dropped {
                write(&resolver, "dropped/ask-1.json", rejection());
            }
            if rejected {
                write(&resolver, "rejected/ask-1.json", rejection());
            }
            let count = usize::from(queued) + usize::from(dropped) + usize::from(rejected);
            let expected = if count > 1 || (claim && rejected) {
                NativeDeliveryPhase::SubmittedUnconfirmed
            } else if queued {
                NativeDeliveryPhase::ProviderAccepted
            } else if dropped {
                NativeDeliveryPhase::Failed
            } else {
                NativeDeliveryPhase::SubmittedUnconfirmed
            };
            let projection = resolver.project("ask-1").unwrap();
            assert_eq!(projection.phase, expected, "bits {bits}");
            assert_eq!(projection.claim_exists, claim);
            assert_eq!(
                resolver.resolve_rejected("ask-1").unwrap().is_some(),
                rejected && !claim && !queued && !dropped
            );
            assert_eq!(
                resolver
                    .resolve_after_verified_exit("ask-1", &Exit(true))
                    .unwrap()
                    .is_some(),
                !claim && !queued && !dropped
            );
            assert!(resolver
                .resolve_after_verified_exit("ask-1", &Exit(false))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn incomplete_or_wrongly_bound_records_never_count() {
        let (_temp, _plan, resolver) = setup();
        for record in [
            json!({"generation":7,"interaction_id":"ask-1"}),
            json!({"generation":8,"interaction_id":"ask-1","session_id":"session-a","reason":"no"}),
            json!({"generation":7,"interaction_id":"ask-2","session_id":"session-a","reason":"no"}),
            json!({"generation":7,"interaction_id":"ask-1","session_id":"../bad","reason":"no"}),
            json!({"generation":7,"interaction_id":"ask-1","session_id":"session-a","reason":""}),
        ] {
            write(&resolver, "rejected/ask-1.json", record);
            assert!(resolver.resolve_rejected("ask-1").unwrap().is_none());
        }
        fs::write(resolver.record_path("queued", "ask-1"), b"{broken").unwrap();
        assert_eq!(
            resolver.project("ask-1").unwrap().phase,
            NativeDeliveryPhase::SubmittedUnconfirmed
        );
        assert!(resolver
            .resolve_after_verified_exit("ask-1", &Exit(true))
            .unwrap()
            .is_none());
        write(&resolver, "dropped/ask-1.json", rejection());
        assert_eq!(
            resolver.project("ask-1").unwrap().detail.as_deref(),
            Some("invalid payload")
        );
    }

    #[test]
    fn claim_of_any_content_or_directory_blocks_recovery() {
        let (_temp, _plan, resolver) = setup();
        write(&resolver, "rejected/ask-1.json", rejection());
        let path = resolver.record_path("claimed", "ask-1");
        for bytes in [b"".as_slice(), b"{partial", b"{\"generation\":99}"] {
            fs::write(&path, bytes).unwrap();
            assert!(resolver.resolve_rejected("ask-1").unwrap().is_none());
            assert!(resolver
                .resolve_after_verified_exit("ask-1", &Exit(true))
                .unwrap()
                .is_none());
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(resolver.resolve_rejected("ask-1").unwrap().is_none());
        assert!(resolver
            .resolve_after_verified_exit("ask-1", &Exit(true))
            .unwrap()
            .is_none());
    }

    #[test]
    fn dangling_claim_symlink_blocks_both_proofs() {
        let (_temp, _plan, resolver) = setup();
        write(&resolver, "rejected/ask-1.json", rejection());
        #[cfg(unix)]
        std::os::unix::fs::symlink("missing-target", resolver.record_path("claimed", "ask-1"))
            .unwrap();
        #[cfg(windows)]
        {
            // Dangling junctions cover Windows reparse entries without requiring
            // the symlink privilege. Both are inspected by symlink_metadata.
            let target = resolver.config.inbox.join("claim-target");
            fs::create_dir(&target).unwrap();
            junction::create(&target, resolver.record_path("claimed", "ask-1")).unwrap();
            fs::remove_dir(target).unwrap();
        }
        assert!(resolver.resolve_rejected("ask-1").unwrap().is_none());
        assert!(resolver
            .resolve_after_verified_exit("ask-1", &Exit(true))
            .unwrap()
            .is_none());
    }

    #[test]
    fn closure_is_checked_before_claim_absence() {
        let (_temp, _plan, resolver) = setup();
        struct Closing<'a>(&'a ClaudeModResolver);
        impl GenerationExitVerifier for Closing<'_> {
            fn process_tree_exited(&self, _: &str, _: u64) -> bool {
                fs::write(self.0.record_path("claimed", "ask-1"), b"late claim").unwrap();
                true
            }
        }
        assert!(resolver
            .resolve_after_verified_exit("ask-1", &Closing(&resolver))
            .unwrap()
            .is_none());
    }

    #[test]
    fn proof_is_bound_and_only_recovers_submitted_unconfirmed() {
        let (_temp, _plan, resolver) = setup();
        write(&resolver, "rejected/ask-1.json", rejection());
        let proof = resolver.resolve_rejected("ask-1").unwrap().unwrap();
        assert_eq!(proof.agent_id(), "agent-1");
        assert_eq!(proof.generation(), 7);
        assert_eq!(proof.interaction_id(), "ask-1");
        for phase in [
            NativeDeliveryPhase::Queued,
            NativeDeliveryPhase::Dispatching,
            NativeDeliveryPhase::SubmittedUnconfirmed,
            NativeDeliveryPhase::ProviderAccepted,
            NativeDeliveryPhase::TurnStarted,
            NativeDeliveryPhase::Completed,
            NativeDeliveryPhase::FailedBeforeSubmit,
            NativeDeliveryPhase::Failed,
            NativeDeliveryPhase::CancelRequested,
            NativeDeliveryPhase::Cancelled,
            NativeDeliveryPhase::Expired,
            NativeDeliveryPhase::StaleGeneration,
            NativeDeliveryPhase::Withdrawn,
            NativeDeliveryPhase::Superseded,
        ] {
            assert_eq!(
                phase.resolve_proven_non_submission(&proof),
                (phase == NativeDeliveryPhase::SubmittedUnconfirmed)
                    .then_some(NativeDeliveryPhase::FailedBeforeSubmit)
            );
        }
        assert!(!NativeDeliveryPhase::SubmittedUnconfirmed
            .can_transition_to(NativeDeliveryPhase::FailedBeforeSubmit));
    }

    #[test]
    fn history_and_transcript_reconcile_into_a_different_session() {
        let (_temp, _plan, resolver) = setup();
        write(&resolver, "claimed/ask-1.json", outcome());
        write(
            &resolver,
            "sessions/session-b.json",
            json!({"generation":7,"session_id":"session-b"}),
        );
        write(
            &resolver,
            "sessions/wrong-path.json",
            json!({"generation":7,"session_id":"injected"}),
        );
        write(
            &resolver,
            "sessions/old.json",
            json!({"generation":6,"session_id":"old"}),
        );
        let tracked = BTreeSet::from(["session-c".to_owned()]);
        let history = resolver.session_history(&tracked).unwrap();
        assert_eq!(
            history,
            BTreeSet::from(["session-a".into(), "session-b".into(), "session-c".into()])
        );
        let row = json!({"type":"user","sessionId":"session-b","message":{"role":"user","content":"The wardian plugin sent a message:\ninteraction_id: ask-1"}});
        assert!(transcript_row_matches(&row, "session-b", &history, "ask-1"));
        assert!(!transcript_row_matches(
            &row,
            "session-a",
            &history,
            "ask-1"
        ));
        assert!(!transcript_row_matches(
            &row,
            "untracked",
            &history,
            "ask-1"
        ));
        assert_eq!(
            resolver.project("ask-1").unwrap().phase,
            NativeDeliveryPhase::SubmittedUnconfirmed
        );
        assert!(resolver
            .resolve_after_verified_exit("ask-1", &Exit(true))
            .unwrap()
            .is_none());
    }

    #[test]
    fn transcript_requires_user_plugin_prefix_and_exact_id() {
        let history = BTreeSet::from(["session-a".into()]);
        let base = json!({"type":"user","message":{"role":"user","content":"The wardian plugin sent a message:\nask-1"}});
        assert!(transcript_row_matches(
            &base,
            "session-a",
            &history,
            "ask-1"
        ));
        for text in [
            "The other plugin sent a message: ask-1",
            "quoted: The wardian plugin sent a message: ask-1",
            "The wardian plugin sent a message: ask-10",
            "The wardian plugin sent a message: prefix-ask-1",
            "The wardian plugin sent a message: ask-1_suffix",
        ] {
            let mut row = base.clone();
            row["message"]["content"] = json!(text);
            assert!(
                !transcript_row_matches(&row, "session-a", &history, "ask-1"),
                "{text}"
            );
        }
        let mut row = base.clone();
        row["type"] = json!("assistant");
        assert!(!transcript_row_matches(
            &row,
            "session-a",
            &history,
            "ask-1"
        ));
        row = base.clone();
        row["message"]["role"] = json!("assistant");
        assert!(!transcript_row_matches(
            &row,
            "session-a",
            &history,
            "ask-1"
        ));
        row = base;
        row["message"]["content"] = json!([{"type":"text","text":"The wardian plugin sent a message:"}, {"type":"text","text":"ask-1"}]);
        assert!(transcript_row_matches(&row, "session-a", &history, "ask-1"));
        row["message"]["content"][1]["type"] = json!("tool_result");
        assert!(!transcript_row_matches(
            &row,
            "session-a",
            &history,
            "ask-1"
        ));
    }
}
