//! Per-turn content snapshots for change review.
//!
//! A snapshot is a parentless commit in the **working tree's own object store**,
//! referenced under `refs/wardian/<agent_id>/<conversation_id>/<turn_index>` and
//! written through a dedicated index file. HEAD, the operator's index, and all
//! branches are never modified. Blobs dedup against existing history, so the
//! storage floor is approximately zero.
//!
//! See `docs/specs/2026-08-02-agent-change-snapshots.md`.

use crate::commands::git::{run_git, run_git_with_env};
use crate::state::conversation_archive::ConversationArchiveState;
use crate::state::AppState;
use crate::utils::fs::get_wardian_home;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const CHANGE_SNAPSHOT_SCHEMA: u8 = 1;

/// Ref namespace for snapshots. Absent from `git branch` and from default push
/// and fetch refspecs, and independently deletable so retention is a ref drop.
pub(crate) const SNAPSHOT_REF_ROOT: &str = "refs/wardian";

/// Rolling window of snapshots retained for an active conversation.
pub(crate) const SNAPSHOT_ACTIVE_WINDOW: usize = 20;

/// Divergence thresholds for a pinned baseline. Both counters are byproducts of
/// work the pane already performs, unlike bytes-held, which would need a
/// repository-wide object walk to decide whether to show a warning.
pub(crate) const SNAPSHOT_DIVERGENCE_TURNS: u64 = 100;
pub(crate) const SNAPSHOT_DIVERGENCE_PATHS: u64 = 200;

/// Shell tools whose presence in a turn means the tree may have changed even
/// though `files.written` is empty. Shell commands are not parsed, so a turn
/// that ran one cannot be skipped.
const SHELL_TOOLS: &[&str] = &[
    "bash",
    "shell",
    "run_command",
    "execute_command",
    "terminal",
    "powershell",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeSnapshotRef {
    pub agent_id: String,
    pub conversation_id: String,
    pub turn_index: u64,
    pub commit_id: String,
    pub tree_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChangeSnapshotIndex {
    pub schema: u8,
    pub workspace: String,
    pub snapshots: Vec<ChangeSnapshotRef>,
    pub last_tree_id: Option<String>,
}

/// Outcome of a snapshot attempt, kept explicit so callers can distinguish an
/// intentional no-op from a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// A new snapshot commit was written and its ref created.
    Created(Box<ChangeSnapshotRef>),
    /// The tree matched the previous snapshot, so the existing commit was reused.
    TreeUnchanged,
    /// The workspace is not a git repository.
    NotAGitRepository,
    // There is deliberately no "skipped, no writes" variant. That decision is
    // made by `snapshot_request_for_turn` before a request exists, which is what
    // keeps a read-only turn off the tree-walk path entirely.
}

/// Describes the turn a snapshot is taken for.
#[derive(Debug, Clone)]
pub struct SnapshotRequest {
    pub cwd: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub turn_index: u64,
}

/// True when a turn cannot be skipped: it claimed writes, or it ran a shell tool
/// whose effects are not recorded in `files.written`.
pub(crate) fn turn_may_have_written(written: &[String], tools_used: &[String]) -> bool {
    if !written.is_empty() {
        return true;
    }
    tools_used.iter().any(|tool| {
        let tool = tool.to_ascii_lowercase();
        SHELL_TOOLS.iter().any(|shell| tool.contains(shell))
    })
}

fn workspace_key(workspace: &str) -> String {
    // A stable, filesystem-safe key. Case is normalized only on Windows, where
    // two spellings of one path are the same workspace.
    #[cfg(windows)]
    let normalized = workspace.to_ascii_lowercase().replace('\\', "/");
    #[cfg(not(windows))]
    let normalized = workspace.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/').to_string();

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)
}

pub(crate) fn snapshot_dir() -> Result<PathBuf, String> {
    let home = get_wardian_home().ok_or_else(|| "Could not find home directory".to_string())?;
    Ok(home.join("changes").join("snapshots"))
}

/// The dedicated index is **per workspace, not per agent.** A working tree is one
/// filesystem: when agents share a workspace their writes are already interleaved
/// on disk, so no index arrangement can produce a tree reflecting one agent's
/// writes and not another's. A per-agent index would imply a content timeline git
/// cannot deliver and would multiply hashing by the number of agents.
pub(crate) fn snapshot_index_path(workspace: &str) -> Result<PathBuf, String> {
    Ok(snapshot_dir()?.join(format!("{}.index", workspace_key(workspace))))
}

fn snapshot_state_path(workspace: &str) -> Result<PathBuf, String> {
    Ok(snapshot_dir()?.join(format!("{}.json", workspace_key(workspace))))
}

pub(crate) fn snapshot_ref_name(agent_id: &str, conversation_id: &str, turn_index: u64) -> String {
    format!(
        "{}/{}/{}/{}",
        SNAPSHOT_REF_ROOT,
        sanitize_ref_component(agent_id),
        sanitize_ref_component(conversation_id),
        turn_index
    )
}

/// Git refuses many characters in ref names. Identifiers are UUID-shaped in
/// practice, but a malformed one must not produce an unusable ref.
fn sanitize_ref_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn is_git_repository(cwd: &str) -> bool {
    run_git(cwd, &["rev-parse", "--git-dir"]).is_ok()
}

fn git_dir(cwd: &str) -> Result<PathBuf, String> {
    let raw = run_git(cwd, &["rev-parse", "--absolute-git-dir"])?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("git directory could not be resolved".to_string());
    }
    Ok(PathBuf::from(trimmed))
}

/// Seeds the dedicated index by **byte-copying `.git/index`**.
///
/// This is a shipping requirement, not an optimisation. A copied index carries
/// valid stat data for every tracked file, so `add -A` hashes only what changed.
/// Measured on a 1466-file repository: 78 ms seeded against 85582 ms for a fresh
/// index, a factor of 1097. An index built empty or through `read-tree` makes the
/// first snapshot a minute and a half of disk thrash.
///
/// Loss of the file therefore costs a re-seed, not a re-hash.
pub(crate) fn ensure_seeded_index(cwd: &str, index_path: &Path) -> Result<(), String> {
    if index_path.is_file() {
        return Ok(());
    }
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create snapshot directory: {}", error))?;
    }

    let source = git_dir(cwd)?.join("index");
    if source.is_file() {
        std::fs::copy(&source, index_path)
            .map_err(|error| format!("Failed to seed snapshot index: {}", error))?;
        return Ok(());
    }

    // A repository with no commits has no index to seed from. The first snapshot
    // then pays the cold cost once, which is why this path is not an error.
    Ok(())
}

fn load_state(workspace: &str) -> ChangeSnapshotIndex {
    let default = ChangeSnapshotIndex {
        schema: CHANGE_SNAPSHOT_SCHEMA,
        workspace: workspace.to_string(),
        snapshots: Vec::new(),
        last_tree_id: None,
    };
    let Ok(path) = snapshot_state_path(workspace) else {
        return default;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return default;
    };
    // A corrupt state file degrades to "no snapshots yet" rather than breaking
    // the pane, matching how a corrupt watermark index is treated.
    serde_json::from_str::<ChangeSnapshotIndex>(&raw)
        .ok()
        .filter(|state| state.schema == CHANGE_SNAPSHOT_SCHEMA)
        .unwrap_or(default)
}

fn save_state(workspace: &str, state: &ChangeSnapshotIndex) -> Result<(), String> {
    let path = snapshot_state_path(workspace)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create snapshot directory: {}", error))?;
    }
    let raw = serde_json::to_string_pretty(state)
        .map_err(|error| format!("Failed to serialize snapshot state: {}", error))?;
    std::fs::write(&path, raw).map_err(|error| format!("Failed to write snapshot state: {}", error))
}

/// Takes one snapshot of the working tree.
///
/// Leaves HEAD, the operator's index, and all branches unmodified. The caller is
/// responsible for holding the workspace lock and for running this off the
/// agent's critical path.
pub(crate) fn take_snapshot(request: &SnapshotRequest) -> Result<SnapshotOutcome, String> {
    let cwd = request.cwd.as_str();
    if !is_git_repository(cwd) {
        return Ok(SnapshotOutcome::NotAGitRepository);
    }

    let index_path = snapshot_index_path(cwd)?;
    ensure_seeded_index(cwd, &index_path)?;
    let index = index_path.to_string_lossy().to_string();
    let env: Vec<(&str, &str)> = vec![("GIT_INDEX_FILE", index.as_str())];

    // `core.preloadIndex` parallelises the stat pass; `core.untrackedCache`
    // avoids re-walking unchanged directories. Both are set per invocation so
    // the operator's own config is never rewritten.
    run_git_with_env(
        cwd,
        &[
            "-c",
            "core.preloadIndex=true",
            "-c",
            "core.untrackedCache=true",
            "add",
            "-A",
        ],
        &env,
    )?;
    let tree_id = run_git_with_env(cwd, &["write-tree"], &env)?
        .trim()
        .to_string();
    if tree_id.is_empty() {
        return Err("write-tree produced no tree".to_string());
    }

    let mut state = load_state(cwd);

    // The reliable no-op check. `git_watch` does not observe working-tree writes,
    // so an unchanged tree is the only trustworthy signal that nothing happened.
    if state.last_tree_id.as_deref() == Some(tree_id.as_str()) {
        return Ok(SnapshotOutcome::TreeUnchanged);
    }

    let message = format!(
        "wardian snapshot: agent {} conversation {} turn {}",
        request.agent_id, request.conversation_id, request.turn_index
    );
    // Parentless: no `-p`. The commit is independently deletable, so retention is
    // a ref drop plus garbage collection with no history rewriting.
    let commit_id = run_git(
        cwd,
        &["commit-tree", tree_id.as_str(), "-m", message.as_str()],
    )?
    .trim()
    .to_string();
    if commit_id.is_empty() {
        return Err("commit-tree produced no commit".to_string());
    }

    let ref_name = snapshot_ref_name(
        &request.agent_id,
        &request.conversation_id,
        request.turn_index,
    );
    run_git(cwd, &["update-ref", ref_name.as_str(), commit_id.as_str()])?;

    let snapshot = ChangeSnapshotRef {
        agent_id: request.agent_id.clone(),
        conversation_id: request.conversation_id.clone(),
        turn_index: request.turn_index,
        commit_id,
        tree_id: tree_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    state.schema = CHANGE_SNAPSHOT_SCHEMA;
    state.workspace = cwd.to_string();
    state.last_tree_id = Some(tree_id);
    state.snapshots.retain(|existing| {
        existing.turn_index != snapshot.turn_index
            || existing.conversation_id != snapshot.conversation_id
            || existing.agent_id != snapshot.agent_id
    });
    state.snapshots.push(snapshot.clone());

    let dropped = apply_retention(&mut state);
    save_state(cwd, &state)?;
    let dropped_any = !dropped.is_empty();
    for ref_name in dropped {
        // A failed ref drop costs disk, never correctness.
        let _ = run_git(cwd, &["update-ref", "-d", ref_name.as_str()]);
    }

    if dropped_any {
        // Collection runs only after retention has actually released refs, and
        // only through `--auto`, which is a no-op unless git's own thresholds are
        // exceeded. Snapshots are already off the turn boundary and serialized per
        // workspace, so this never runs during a turn or against itself.
        let _ = run_git(cwd, &["gc", "--auto", "--quiet"]);
    }

    Ok(SnapshotOutcome::Created(Box::new(snapshot)))
}

/// Retention is bounded by policy, not by turn count.
///
/// The active conversation keeps a rolling window; every other conversation
/// keeps only its first and last snapshot. Returns the refs that should be
/// dropped.
fn apply_retention(state: &mut ChangeSnapshotIndex) -> Vec<String> {
    let Some(active) = state.snapshots.last().map(|s| s.conversation_id.clone()) else {
        return Vec::new();
    };

    let mut by_conversation: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, snapshot) in state.snapshots.iter().enumerate() {
        by_conversation
            .entry(snapshot.conversation_id.clone())
            .or_default()
            .push(position);
    }

    let mut keep = vec![false; state.snapshots.len()];
    for (conversation, positions) in &by_conversation {
        if *conversation == active {
            let start = positions.len().saturating_sub(SNAPSHOT_ACTIVE_WINDOW);
            for position in &positions[start..] {
                keep[*position] = true;
            }
        } else {
            if let Some(first) = positions.first() {
                keep[*first] = true;
            }
            if let Some(last) = positions.last() {
                keep[*last] = true;
            }
        }
    }

    let mut dropped = Vec::new();
    let mut retained = Vec::with_capacity(state.snapshots.len());
    for (position, snapshot) in state.snapshots.drain(..).enumerate() {
        if keep[position] {
            retained.push(snapshot);
        } else {
            dropped.push(snapshot_ref_name(
                &snapshot.agent_id,
                &snapshot.conversation_id,
                snapshot.turn_index,
            ));
        }
    }
    state.snapshots = retained;
    dropped
}

/// The most recent snapshot commit for a conversation, used to resolve the
/// `last_effective_turn` baseline to real content.
pub(crate) fn latest_snapshot_commit(
    workspace: &str,
    agent_id: &str,
    conversation_id: Option<&str>,
) -> Option<String> {
    let state = load_state(workspace);
    state
        .snapshots
        .iter()
        .rfind(|snapshot| {
            snapshot.agent_id == agent_id
                && conversation_id.is_none_or(|id| snapshot.conversation_id == id)
        })
        .map(|snapshot| snapshot.commit_id.clone())
}

/// The earliest snapshot commit for a conversation, used to resolve the
/// `conversation_start` baseline.
pub(crate) fn first_snapshot_commit(
    workspace: &str,
    agent_id: &str,
    conversation_id: Option<&str>,
) -> Option<String> {
    let state = load_state(workspace);
    state
        .snapshots
        .iter()
        .filter(|snapshot| snapshot.agent_id == agent_id)
        .filter(|snapshot| conversation_id.is_none_or(|id| snapshot.conversation_id == id))
        .map(|snapshot| snapshot.commit_id.clone())
        .next()
}

/// True when a commit still resolves. A rebase, amend, or operator `gc --prune`
/// can orphan a snapshot, and an orphaned baseline must degrade to Phase 1
/// rather than error.
pub(crate) fn commit_resolves(cwd: &str, commit: &str) -> bool {
    run_git(
        cwd,
        &["rev-parse", "--verify", &format!("{}^{{commit}}", commit)],
    )
    .is_ok()
}

/// Removes every snapshot ref for an agent, used when an agent is deleted.
pub(crate) fn drop_agent_snapshots(workspace: &str, agent_id: &str) -> Result<(), String> {
    let mut state = load_state(workspace);
    let (dropped, retained): (Vec<_>, Vec<_>) = state
        .snapshots
        .drain(..)
        .partition(|snapshot| snapshot.agent_id == agent_id);
    state.snapshots = retained;
    if state.snapshots.is_empty() {
        state.last_tree_id = None;
    }
    save_state(workspace, &state)?;
    for snapshot in dropped {
        let ref_name = snapshot_ref_name(
            &snapshot.agent_id,
            &snapshot.conversation_id,
            snapshot.turn_index,
        );
        let _ = run_git(workspace, &["update-ref", "-d", ref_name.as_str()]);
    }
    Ok(())
}

/// Decides whether a completed turn warrants a snapshot, and describes it.
///
/// Returns `None` when the turn wrote nothing and ran no shell tool, which is
/// the skip that keeps read-only analysis turns off the tree-walk path entirely.
pub(crate) fn snapshot_request_for_turn(
    cwd: &str,
    agent_id: &str,
    conversation_id: &str,
    turn_index: u64,
    written: &[String],
    tools_used: &[String],
) -> Option<SnapshotRequest> {
    if !turn_may_have_written(written, tools_used) {
        return None;
    }
    Some(SnapshotRequest {
        cwd: cwd.to_string(),
        agent_id: agent_id.to_string(),
        conversation_id: conversation_id.to_string(),
        turn_index,
    })
}

/// Resolves the workspace an agent is running in.
async fn agent_workspace(state: &AppState, session_id: &str) -> Option<String> {
    let agents = state.agents.lock().await;
    let agent = agents.get(session_id)?;
    let folder = agent.config.lock().ok()?.folder.clone();
    let folder = folder.trim().to_string();
    if folder.is_empty() {
        None
    } else {
        Some(folder)
    }
}

/// The most recent turn record for a conversation, with the fields that decide
/// whether it warrants a snapshot.
fn latest_turn_for_conversation(
    archive: &ConversationArchiveState,
    agent_id: &str,
    conversation_id: &str,
) -> Option<(u64, Vec<String>, Vec<String>)> {
    let entries = archive.list(Some(agent_id), false).ok()?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.conversation_id == conversation_id)?;
    let (records, _skipped) = archive
        .turn_records_for_conversations_resilient(std::slice::from_ref(&entry))
        .ok()?;
    let (_entry, turn) = records
        .into_iter()
        .max_by_key(|(_, turn)| turn.turn_index)?;
    Some((
        turn.turn_index,
        turn.files.written.clone(),
        turn.tools_used.keys().cloned().collect(),
    ))
}

/// Takes a snapshot for a turn that has just completed.
///
/// Runs entirely off the agent's critical path: the caller spawns this, and a
/// failure is logged and dropped rather than surfaced. A turn that wrote nothing
/// and ran no shell tool returns before any tree walk happens.
pub(crate) async fn snapshot_completed_turn(state: &AppState, session_id: &str) {
    let Some(cwd) = agent_workspace(state, session_id).await else {
        return;
    };
    let Ok(Some(conversation_id)) = state
        .conversation_archive
        .active_conversation_id(session_id)
    else {
        return;
    };
    let Some((turn_index, written, tools_used)) =
        latest_turn_for_conversation(&state.conversation_archive, session_id, &conversation_id)
    else {
        return;
    };
    let Some(request) = snapshot_request_for_turn(
        &cwd,
        session_id,
        &conversation_id,
        turn_index,
        &written,
        &tools_used,
    ) else {
        return;
    };

    state.change_snapshots.snapshot(request).await;
}

/// A pinned baseline diverges by turns and paths, never by bytes held.
///
/// Bytes uniquely held by a pin is the intuitive metric and the wrong one:
/// computing it needs `rev-list --objects` plus `cat-file --batch-check` over the
/// pinned commit, a repository-wide walk run only to decide whether to show a
/// warning. That inverts the governing constraint of this feature.
pub(crate) fn baseline_diverged(turns_since_pin: u64, paths_since_pin: u64) -> bool {
    turns_since_pin > SNAPSHOT_DIVERGENCE_TURNS || paths_since_pin > SNAPSHOT_DIVERGENCE_PATHS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_with_writes_is_never_skipped() {
        assert!(turn_may_have_written(&["src/a.ts".to_string()], &[]));
    }

    #[test]
    fn a_turn_with_a_shell_tool_is_never_skipped() {
        // Shell writes are not recorded in files.written, so an empty claim list
        // does not prove the tree is unchanged.
        assert!(turn_may_have_written(&[], &["Bash".to_string()]));
        assert!(turn_may_have_written(&[], &["run_command".to_string()]));
    }

    #[test]
    fn a_read_only_turn_is_skipped_without_a_tree_walk() {
        assert!(!turn_may_have_written(
            &[],
            &["read".to_string(), "grep".to_string()]
        ));
        assert!(!turn_may_have_written(&[], &[]));
    }

    #[test]
    fn a_read_only_turn_produces_no_snapshot_request() {
        // The skip has to happen before any request is built, so the tree walk
        // never starts for an analysis-only turn.
        assert!(snapshot_request_for_turn("/w", "agent-1", "conv-1", 3, &[], &[]).is_none());
        assert!(snapshot_request_for_turn(
            "/w",
            "agent-1",
            "conv-1",
            3,
            &[],
            &["read".to_string()]
        )
        .is_none());
    }

    #[test]
    fn a_writing_turn_produces_a_request_carrying_its_identity() {
        let request =
            snapshot_request_for_turn("/w", "agent-1", "conv-1", 7, &["a.ts".to_string()], &[])
                .expect("a writing turn must produce a request");

        assert_eq!(request.cwd, "/w");
        assert_eq!(request.agent_id, "agent-1");
        assert_eq!(request.conversation_id, "conv-1");
        assert_eq!(request.turn_index, 7);
    }

    #[test]
    fn a_shell_only_turn_still_produces_a_request() {
        // Shell writes never appear in files.written, so skipping on an empty
        // claim list would drop exactly the changes `inferred` exists to surface.
        assert!(snapshot_request_for_turn(
            "/w",
            "agent-1",
            "conv-1",
            3,
            &[],
            &["Bash".to_string()]
        )
        .is_some());
    }

    #[test]
    fn ref_names_live_under_the_private_namespace() {
        let name = snapshot_ref_name("agent-1", "conv-1", 4);
        assert_eq!(name, "refs/wardian/agent-1/conv-1/4");
        assert!(name.starts_with(SNAPSHOT_REF_ROOT));
    }

    #[test]
    fn ref_components_are_sanitized_without_collapsing_to_empty() {
        assert_eq!(sanitize_ref_component("a/b c"), "a-b-c");
        assert_eq!(sanitize_ref_component(""), "unknown");
        assert_eq!(sanitize_ref_component("~^:?*["), "------");
    }

    #[test]
    fn workspace_keys_are_stable_and_distinguish_workspaces() {
        assert_eq!(workspace_key("/a/b"), workspace_key("/a/b/"));
        assert_eq!(workspace_key("/a/b"), workspace_key("/a\\b"));
        assert_ne!(workspace_key("/a/b"), workspace_key("/a/c"));
    }

    #[cfg(windows)]
    #[test]
    fn workspace_keys_ignore_case_only_on_windows() {
        assert_eq!(workspace_key("C:/Work"), workspace_key("c:/work"));
    }

    #[cfg(not(windows))]
    #[test]
    fn workspace_keys_preserve_case_off_windows() {
        assert_ne!(workspace_key("/Work"), workspace_key("/work"));
    }

    fn snapshot(conversation: &str, turn: u64) -> ChangeSnapshotRef {
        ChangeSnapshotRef {
            agent_id: "agent-1".to_string(),
            conversation_id: conversation.to_string(),
            turn_index: turn,
            commit_id: format!("commit-{}-{}", conversation, turn),
            tree_id: format!("tree-{}-{}", conversation, turn),
            created_at: "2026-08-02T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn retention_keeps_a_rolling_window_for_the_active_conversation() {
        let mut state = ChangeSnapshotIndex {
            schema: CHANGE_SNAPSHOT_SCHEMA,
            workspace: "/w".to_string(),
            snapshots: (0..25).map(|turn| snapshot("conv-active", turn)).collect(),
            last_tree_id: None,
        };

        let dropped = apply_retention(&mut state);

        assert_eq!(state.snapshots.len(), SNAPSHOT_ACTIVE_WINDOW);
        assert_eq!(dropped.len(), 5);
        assert_eq!(state.snapshots.first().unwrap().turn_index, 5);
        assert_eq!(state.snapshots.last().unwrap().turn_index, 24);
    }

    #[test]
    fn retention_keeps_only_the_first_and_last_of_a_closed_conversation() {
        let mut state = ChangeSnapshotIndex {
            schema: CHANGE_SNAPSHOT_SCHEMA,
            workspace: "/w".to_string(),
            snapshots: vec![
                snapshot("conv-closed", 1),
                snapshot("conv-closed", 2),
                snapshot("conv-closed", 3),
                snapshot("conv-active", 4),
            ],
            last_tree_id: None,
        };

        let dropped = apply_retention(&mut state);

        let closed: Vec<u64> = state
            .snapshots
            .iter()
            .filter(|s| s.conversation_id == "conv-closed")
            .map(|s| s.turn_index)
            .collect();
        assert_eq!(closed, vec![1, 3]);
        assert_eq!(dropped, vec!["refs/wardian/agent-1/conv-closed/2"]);
    }

    #[test]
    fn divergence_needs_both_thresholds_crossed_independently() {
        assert!(!baseline_diverged(
            SNAPSHOT_DIVERGENCE_TURNS,
            SNAPSHOT_DIVERGENCE_PATHS
        ));
        assert!(baseline_diverged(SNAPSHOT_DIVERGENCE_TURNS + 1, 0));
        assert!(baseline_diverged(0, SNAPSHOT_DIVERGENCE_PATHS + 1));
    }

    // ---- Real-repository tests -------------------------------------------
    //
    // These hold the invariants that matter and cannot be proven with pure
    // logic: that snapshots never touch operator state, that the index is
    // seeded rather than built, and that an unchanged tree creates no ref.
    //
    // Each test isolates WARDIAN_HOME so snapshot state never escapes into the
    // developer's real home directory. `--test-threads=1` is required because
    // the environment variable is process-global.

    struct TestRepo {
        _home: tempfile::TempDir,
        repo: tempfile::TempDir,
        previous_home: Option<std::ffi::OsString>,
        // `WARDIAN_HOME` is process-global, and the Linux coverage job runs
        // `cargo llvm-cov --workspace` without `--test-threads=1`. Without this
        // guard the home directory races between concurrently running tests.
        _env_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TestRepo {
        fn new() -> Self {
            let _env_guard = crate::utils::wardian_test_env_lock();
            let home = tempfile::tempdir().unwrap();
            let repo = tempfile::tempdir().unwrap();
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home.path());

            let cwd = repo.path().to_str().unwrap();
            run_git(cwd, &["init"]).unwrap();
            run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
            run_git(cwd, &["config", "user.name", "Test"]).unwrap();
            run_git(cwd, &["config", "commit.gpgsign", "false"]).unwrap();

            let this = Self {
                _home: home,
                repo,
                previous_home,
                _env_guard,
            };
            this.write("tracked.txt", "one\n");
            run_git(this.cwd(), &["add", "-A"]).unwrap();
            run_git(this.cwd(), &["commit", "-m", "initial"]).unwrap();
            this
        }

        fn cwd(&self) -> &str {
            self.repo.path().to_str().unwrap()
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.repo.path().join(name), contents).unwrap();
        }

        fn request(&self, turn_index: u64) -> SnapshotRequest {
            SnapshotRequest {
                cwd: self.cwd().to_string(),
                agent_id: "agent-1".to_string(),
                conversation_id: "conv-1".to_string(),
                turn_index,
            }
        }

        fn git(&self, args: &[&str]) -> String {
            run_git(self.cwd(), args).unwrap().trim().to_string()
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    #[test]
    fn a_snapshot_leaves_head_the_operator_index_and_branches_unmodified() {
        let repo = TestRepo::new();
        let head_before = repo.git(&["rev-parse", "HEAD"]);
        let branches_before = repo.git(&["branch", "--list"]);
        let index_before = std::fs::read(repo.repo.path().join(".git").join("index")).unwrap();
        let status_before = repo.git(&["status", "--porcelain"]);

        repo.write("added.txt", "new\n");
        let outcome = take_snapshot(&repo.request(1)).unwrap();
        assert!(matches!(outcome, SnapshotOutcome::Created(_)));

        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head_before);
        assert_eq!(repo.git(&["branch", "--list"]), branches_before);
        assert_eq!(
            std::fs::read(repo.repo.path().join(".git").join("index")).unwrap(),
            index_before,
            "the operator's index must not be touched",
        );
        // The new file is still untracked from the operator's point of view.
        assert!(repo.git(&["status", "--porcelain"]).contains("added.txt"));
        assert_ne!(repo.git(&["status", "--porcelain"]), status_before);
    }

    #[test]
    fn a_snapshot_ref_is_invisible_to_git_branch() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();

        assert!(!repo.git(&["branch", "--list"]).contains("wardian"));
        let refs = repo.git(&["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_ROOT]);
        assert_eq!(refs, "refs/wardian/agent-1/conv-1/1");
    }

    #[test]
    fn the_dedicated_index_is_seeded_by_byte_copy_from_the_operator_index() {
        let repo = TestRepo::new();
        let source = std::fs::read(repo.repo.path().join(".git").join("index")).unwrap();
        let index_path = snapshot_index_path(repo.cwd()).unwrap();
        assert!(!index_path.exists());

        ensure_seeded_index(repo.cwd(), &index_path).unwrap();

        assert_eq!(
            std::fs::read(&index_path).unwrap(),
            source,
            "seeding must be a byte copy; a rebuilt index re-hashes the whole tree",
        );
    }

    #[test]
    fn a_lost_index_is_reseeded_rather_than_rebuilt() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();

        let index_path = snapshot_index_path(repo.cwd()).unwrap();
        std::fs::remove_file(&index_path).unwrap();
        assert!(!index_path.exists());

        repo.write("second.txt", "more\n");
        let outcome = take_snapshot(&repo.request(2)).unwrap();

        assert!(matches!(outcome, SnapshotOutcome::Created(_)));
        assert!(
            index_path.is_file(),
            "the index must be re-seeded, not left missing"
        );
    }

    #[test]
    fn an_unchanged_tree_creates_no_new_ref_and_reuses_the_previous_commit() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();

        let outcome = take_snapshot(&repo.request(2)).unwrap();

        assert_eq!(outcome, SnapshotOutcome::TreeUnchanged);
        let refs = repo.git(&["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_ROOT]);
        assert_eq!(
            refs, "refs/wardian/agent-1/conv-1/1",
            "an unchanged tree must not mint a second ref",
        );
    }

    #[test]
    fn a_changed_tree_after_an_unchanged_one_still_snapshots() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();
        assert_eq!(
            take_snapshot(&repo.request(2)).unwrap(),
            SnapshotOutcome::TreeUnchanged
        );

        repo.write("added.txt", "changed\n");
        let outcome = take_snapshot(&repo.request(3)).unwrap();

        assert!(matches!(outcome, SnapshotOutcome::Created(_)));
        assert!(repo
            .git(&["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_ROOT])
            .contains("refs/wardian/agent-1/conv-1/3"));
    }

    #[test]
    fn a_snapshot_commit_is_parentless() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        let SnapshotOutcome::Created(snapshot) = take_snapshot(&repo.request(1)).unwrap() else {
            panic!("expected a snapshot");
        };

        let parents = repo.git(&["rev-list", "--parents", "-n", "1", &snapshot.commit_id]);
        assert_eq!(
            parents.split_whitespace().count(),
            1,
            "a snapshot commit must have no parents so it stays independently deletable",
        );
    }

    #[test]
    fn a_snapshot_captures_working_tree_content_not_head() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "modified\n");
        let SnapshotOutcome::Created(snapshot) = take_snapshot(&repo.request(1)).unwrap() else {
            panic!("expected a snapshot");
        };

        let content = repo.git(&["show", &format!("{}:tracked.txt", snapshot.commit_id)]);
        assert_eq!(content, "modified");
    }

    #[test]
    fn a_non_git_workspace_yields_no_snapshot_rather_than_an_error() {
        let _env_guard = crate::utils::wardian_test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", home.path());
        let plain = tempfile::tempdir().unwrap();

        let outcome = take_snapshot(&SnapshotRequest {
            cwd: plain.path().to_str().unwrap().to_string(),
            agent_id: "agent-1".to_string(),
            conversation_id: "conv-1".to_string(),
            turn_index: 1,
        });

        match previous {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        assert_eq!(outcome.unwrap(), SnapshotOutcome::NotAGitRepository);
    }

    #[test]
    fn deleting_an_agent_drops_its_refs_and_leaves_others_intact() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();
        repo.write("added.txt", "other\n");
        take_snapshot(&SnapshotRequest {
            cwd: repo.cwd().to_string(),
            agent_id: "agent-2".to_string(),
            conversation_id: "conv-2".to_string(),
            turn_index: 1,
        })
        .unwrap();

        drop_agent_snapshots(repo.cwd(), "agent-1").unwrap();

        let refs = repo.git(&["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_ROOT]);
        assert!(!refs.contains("agent-1"));
        assert!(refs.contains("agent-2"));
    }

    #[test]
    fn baselines_resolve_to_the_first_and_latest_snapshot_commits() {
        let repo = TestRepo::new();
        repo.write("added.txt", "one\n");
        let SnapshotOutcome::Created(first) = take_snapshot(&repo.request(1)).unwrap() else {
            panic!("expected a snapshot");
        };
        repo.write("added.txt", "two\n");
        let SnapshotOutcome::Created(latest) = take_snapshot(&repo.request(2)).unwrap() else {
            panic!("expected a snapshot");
        };

        assert_eq!(
            first_snapshot_commit(repo.cwd(), "agent-1", Some("conv-1")),
            Some(first.commit_id.clone()),
        );
        assert_eq!(
            latest_snapshot_commit(repo.cwd(), "agent-1", Some("conv-1")),
            Some(latest.commit_id.clone()),
        );
        assert!(commit_resolves(repo.cwd(), &latest.commit_id));
        assert!(!commit_resolves(
            repo.cwd(),
            "0000000000000000000000000000000000000000"
        ));
    }

    // ---- Budget gate ------------------------------------------------------
    //
    // Phase 2 ships only if snapshots stay inside their budgets, so the budget
    // lives here rather than in prose. See the "Gate Result" section of
    // `docs/specs/2026-08-02-agent-change-snapshots.md`.
    //
    // What this proves and what it does not: the reference measurement was taken
    // on a 1466-file repository (p95 ~860 ms against a 1 s budget), and a fixture
    // repository is smaller and therefore faster. This test does not reproduce
    // that measurement. It catches a pathological regression — an unseeded index,
    // a per-file invocation, a full re-hash — any of which blows the budget by
    // one to two orders of magnitude rather than by a few percent. The headroom
    // is deliberate so ordinary CI variance cannot fail the build.

    const FIRST_SNAPSHOT_BUDGET_MS: u128 = 2_000;
    const PER_TURN_BUDGET_P95_MS: u128 = 1_000;

    #[test]
    fn snapshots_stay_within_their_measured_budgets() {
        let repo = TestRepo::new();
        for index in 0..300 {
            repo.write(
                &format!("file-{index:03}.txt"),
                &format!("contents {index}\n"),
            );
        }
        run_git(repo.cwd(), &["add", "-A"]).unwrap();
        run_git(repo.cwd(), &["commit", "-m", "fixture"]).unwrap();

        repo.write("file-000.txt", "changed by the first turn\n");
        let started = std::time::Instant::now();
        let outcome = take_snapshot(&repo.request(1)).unwrap();
        let first_ms = started.elapsed().as_millis();
        assert!(matches!(outcome, SnapshotOutcome::Created(_)));
        assert!(
            first_ms <= FIRST_SNAPSHOT_BUDGET_MS,
            "first snapshot took {first_ms} ms, budget is {FIRST_SNAPSHOT_BUDGET_MS} ms; \
             an unseeded index is the usual cause",
        );

        let mut samples = Vec::new();
        for turn in 2..12u64 {
            repo.write("file-001.txt", &format!("turn {turn}\n"));
            let started = std::time::Instant::now();
            let outcome = take_snapshot(&repo.request(turn)).unwrap();
            samples.push(started.elapsed().as_millis());
            assert!(matches!(outcome, SnapshotOutcome::Created(_)));
        }

        samples.sort_unstable();
        // p95 of ten samples is the slowest one.
        let p95 = *samples.last().unwrap();
        let median = samples[samples.len() / 2];
        assert!(
            p95 <= PER_TURN_BUDGET_P95_MS,
            "per-turn snapshot p95 was {p95} ms (median {median} ms), \
             budget is {PER_TURN_BUDGET_P95_MS} ms; samples: {samples:?}",
        );
    }

    #[test]
    fn a_no_op_snapshot_writes_no_object_and_no_ref() {
        // The cheap path has to stay cheap structurally, not just quickly: an
        // unchanged tree must reach neither commit-tree nor update-ref.
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();
        let refs_before = repo.git(&["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_ROOT]);

        for turn in 2..6u64 {
            assert_eq!(
                take_snapshot(&repo.request(turn)).unwrap(),
                SnapshotOutcome::TreeUnchanged
            );
        }

        assert_eq!(
            repo.git(&["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_ROOT]),
            refs_before,
        );
    }

    #[test]
    fn snapshots_from_one_workspace_do_not_leak_into_another() {
        let repo = TestRepo::new();
        repo.write("added.txt", "new\n");
        take_snapshot(&repo.request(1)).unwrap();

        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            latest_snapshot_commit(other.path().to_str().unwrap(), "agent-1", Some("conv-1")),
            None,
        );
    }
}
