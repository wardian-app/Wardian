use crate::commands::change_snapshot::{
    baseline_diverged, commit_resolves, first_snapshot_commit, latest_snapshot_commit,
};
use crate::commands::git::{
    git_diff_numstat_for_cwd, git_status_for_cwd, run_git, GitNumstatEntry,
};
use crate::state::{conversation_archive::ConversationArchiveState, AppState};
use crate::utils::fs::get_wardian_home;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use tauri::{AppHandle, State};
use wardian_core::conversations::{ConversationIndexEntry, ConversationTurnRecord};
use wardian_core::models::git::GitStatusResult;

const CHANGE_REVIEW_SCHEMA: u8 = 1;
const CHANGE_REVIEW_RECENT_CONVERSATION_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReviewBaseline {
    #[default]
    LastEffectiveTurn,
    ConversationStart,
    BranchPoint,
    Head,
    Unreviewed,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChangeReviewPrefs {
    pub schema: u8,
    pub baseline: ChangeReviewBaseline,
}

impl Default for ChangeReviewPrefs {
    fn default() -> Self {
        Self {
            schema: CHANGE_REVIEW_SCHEMA,
            baseline: ChangeReviewBaseline::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReviewEvidence {
    Attributed,
    Inferred,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReviewChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeReviewFileEntry {
    pub path: String,
    pub change_kind: ChangeReviewChangeKind,
    pub old_path: Option<String>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
    pub evidence: ChangeReviewEvidence,
    pub agent_ids: Vec<String>,
    pub turn_indices: Vec<u64>,
    pub binary: bool,
    pub truncated: bool,
    pub reviewed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeReviewSummary {
    pub schema: u8,
    pub baseline: ChangeReviewBaseline,
    pub baseline_ref: Option<String>,
    pub from_turn_index: Option<u64>,
    pub to_turn_index: Option<u64>,
    pub files: Vec<ChangeReviewFileEntry>,
    pub computed_at: String,
    pub truncated: bool,
    /// True when a pinned baseline has drifted far enough to be worth
    /// re-anchoring.
    ///
    /// Divergence is measured in turns and paths, never in bytes held. Both
    /// counters below are byproducts of work already done to build this summary,
    /// whereas bytes uniquely held by the pin would need a repository-wide object
    /// walk run only to decide whether to show a warning.
    #[serde(default)]
    pub diverged: bool,
    #[serde(default)]
    pub turns_since_baseline: Option<u64>,
    #[serde(default)]
    pub paths_since_baseline: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeReviewWatermark {
    pub schema: u8,
    pub agent_id: String,
    pub workspace: String,
    pub reviewed_turn_index: u64,
    pub reviewed_at: String,
    pub reviewed_head: Option<String>,
    #[serde(default)]
    pub reviewed_paths: Vec<ChangeReviewReviewedPath>,
    /// Snapshot commit captured at the moment of review.
    ///
    /// This is the content anchor Phase 1 lacked. With it, a file edited and then
    /// reverted to its reviewed content reads as unchanged, because the
    /// comparison is against bytes rather than against a numstat signature.
    /// Absent for watermarks written before Phase 2, which keep the Phase 1
    /// signature comparison.
    #[serde(default)]
    pub reviewed_snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeReviewReviewedPath {
    pub path: String,
    pub change_kind: ChangeReviewChangeKind,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadChangeReviewRequest {
    pub cwd: String,
    pub baseline: ChangeReviewBaseline,
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeReviewLoadResponse {
    pub summary: ChangeReviewSummary,
    pub git_available: bool,
    pub head_ref: Option<String>,
    pub skipped_turn_records: u64,
}

#[derive(Debug, Clone, Default)]
struct Attribution {
    agent_ids: BTreeSet<String>,
    turn_indices: BTreeSet<u64>,
}

#[derive(Debug, Clone)]
struct TurnWithContext {
    entry: ConversationIndexEntry,
    turn: ConversationTurnRecord,
}

type WatermarkIndex = BTreeMap<String, ChangeReviewWatermark>;

fn path_identity(path: &str) -> String {
    #[cfg(windows)]
    {
        path.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string()
    }
}

fn normalized_path(cwd: &str, path: &str) -> String {
    let mut value = path.trim().replace('\\', "/");
    let normalized_cwd = cwd
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    let cwd_compare = path_identity(&normalized_cwd);
    let value_compare = path_identity(&value);
    if value_compare == cwd_compare {
        value.clear();
    } else if value_compare.starts_with(&(cwd_compare.clone() + "/")) {
        value = value[normalized_cwd.len() + 1..].to_string();
    }
    while let Some(stripped) = value.strip_prefix("./") {
        value = stripped.to_string();
    }
    path_identity(&value)
}

fn same_workspace(cwd: &str, workspace: &str) -> bool {
    if workspace.trim().is_empty() {
        return false;
    }
    normalized_path(cwd, workspace).is_empty()
        || normalized_path(workspace, cwd).is_empty()
        || normalized_path(cwd, workspace) == normalized_path(workspace, cwd)
}

fn watermark_key(agent_id: &str, workspace: &str) -> String {
    format!("{}\n{}", agent_id.trim(), workspace.trim())
}

fn watermark_path(home: &Path) -> std::path::PathBuf {
    home.join("changes").join("watermarks.json")
}

fn prefs_path(home: &Path) -> std::path::PathBuf {
    home.join("changes").join("prefs.json")
}

fn load_prefs_from_home(home: &Path) -> ChangeReviewPrefs {
    let Ok(content) = std::fs::read_to_string(prefs_path(home)) else {
        return ChangeReviewPrefs::default();
    };
    let Ok(prefs) = serde_json::from_str::<ChangeReviewPrefs>(&content) else {
        return ChangeReviewPrefs::default();
    };
    if prefs.schema != CHANGE_REVIEW_SCHEMA {
        return ChangeReviewPrefs::default();
    }
    prefs
}

fn save_prefs_to_home(home: &Path, prefs: &ChangeReviewPrefs) -> Result<(), String> {
    let changes_dir = home.join("changes");
    std::fs::create_dir_all(&changes_dir).map_err(|error| error.to_string())?;
    let path = prefs_path(home);
    let normalized = ChangeReviewPrefs {
        schema: CHANGE_REVIEW_SCHEMA,
        baseline: prefs.baseline,
    };
    let json = serde_json::to_string_pretty(&normalized).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| error.to_string())
}

fn load_watermark_index(home: &Path) -> WatermarkIndex {
    let path = watermark_path(home);
    let Ok(content) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub(crate) fn remove_change_review_watermarks_for_agent(
    home: &Path,
    agent_id: &str,
) -> Result<(), String> {
    let path = watermark_path(home);
    if !path.exists() {
        return Ok(());
    }
    let mut index = load_watermark_index(home);
    index.retain(|_, watermark| watermark.agent_id != agent_id);
    let json = serde_json::to_string_pretty(&index).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| error.to_string())
}

fn load_watermark(agent_id: Option<&str>, workspace: &str) -> Option<ChangeReviewWatermark> {
    let agent_id = agent_id?.trim();
    if agent_id.is_empty() {
        return None;
    }
    let home = get_wardian_home()?;
    load_watermark_index(&home).remove(&watermark_key(agent_id, workspace))
}

fn read_turns_for_workspace(
    archive: &ConversationArchiveState,
    cwd: &str,
) -> Result<(Vec<TurnWithContext>, u64), String> {
    let entries = archive
        .list(None, true)
        .map_err(|error| error.to_string())?;
    let matching_entries = entries
        .into_iter()
        .filter(|entry| same_workspace(cwd, &entry.workspace))
        .collect::<Vec<_>>();
    let mut active_conversation_ids = BTreeSet::new();
    for entry in &matching_entries {
        if let Some(conversation_id) = archive
            .active_conversation_id(&entry.agent_id)
            .map_err(|error| error.to_string())?
        {
            active_conversation_ids.insert(conversation_id);
        }
    }
    let selected_entries =
        select_conversation_entries_for_change_review(matching_entries, &active_conversation_ids);
    archive
        .turn_records_for_conversations_resilient(&selected_entries)
        .map(|(records, skipped_records)| {
            (
                records
                    .into_iter()
                    .map(|(entry, turn)| TurnWithContext { entry, turn })
                    .collect(),
                skipped_records as u64,
            )
        })
        .map_err(|error| error.to_string())
}

fn conversation_recency(entry: &ConversationIndexEntry) -> &str {
    entry
        .ended_at
        .as_deref()
        .unwrap_or(entry.started_at.as_str())
}

fn select_conversation_entries_for_change_review(
    mut entries: Vec<ConversationIndexEntry>,
    active_conversation_ids: &BTreeSet<String>,
) -> Vec<ConversationIndexEntry> {
    entries.sort_by(|left, right| {
        conversation_recency(right)
            .cmp(conversation_recency(left))
            .then_with(|| right.conversation_id.cmp(&left.conversation_id))
    });

    let mut selected = entries
        .iter()
        .take(CHANGE_REVIEW_RECENT_CONVERSATION_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    for entry in entries
        .iter()
        .filter(|entry| active_conversation_ids.contains(&entry.conversation_id))
    {
        if selected
            .iter()
            .all(|selected_entry| selected_entry.conversation_id != entry.conversation_id)
        {
            selected.push(entry.clone());
        }
    }
    selected
}

fn add_claim(
    claims: &mut HashMap<String, Attribution>,
    cwd: &str,
    path: &str,
    agent_id: &str,
    turn_index: u64,
) {
    let key = normalized_path(cwd, path);
    if key.is_empty() {
        return;
    }
    let attribution = claims.entry(key).or_default();
    if !agent_id.trim().is_empty() {
        attribution.agent_ids.insert(agent_id.trim().to_string());
    }
    attribution.turn_indices.insert(turn_index);
}

fn attribution_for_turns(
    cwd: &str,
    turns: &[TurnWithContext],
) -> (HashMap<String, Attribution>, Option<u64>, Option<u64>) {
    let mut claims = HashMap::new();
    let mut first_turn = None;
    let mut latest_effective_turn = None;

    for record in turns {
        first_turn = Some(first_turn.map_or(record.turn.turn_index, |value: u64| {
            value.min(record.turn.turn_index)
        }));
        let mut claimed_any_path = false;
        for path in &record.turn.files.written {
            add_claim(
                &mut claims,
                cwd,
                path,
                &record.entry.agent_id,
                record.turn.turn_index,
            );
            claimed_any_path = true;
        }
        for side_effect in &record.turn.external_side_effects {
            for path in &side_effect.paths {
                add_claim(
                    &mut claims,
                    cwd,
                    path,
                    &record.entry.agent_id,
                    record.turn.turn_index,
                );
                claimed_any_path = true;
            }
        }
        if claimed_any_path {
            latest_effective_turn = Some(
                latest_effective_turn.map_or(record.turn.turn_index, |value: u64| {
                    value.max(record.turn.turn_index)
                }),
            );
        }
    }

    (claims, first_turn, latest_effective_turn)
}

fn status_change_kind(status: &str) -> ChangeReviewChangeKind {
    match status {
        "?" => ChangeReviewChangeKind::Untracked,
        "A" => ChangeReviewChangeKind::Added,
        "D" => ChangeReviewChangeKind::Deleted,
        "R" => ChangeReviewChangeKind::Renamed,
        _ => ChangeReviewChangeKind::Modified,
    }
}

fn status_entries(status: &GitStatusResult) -> BTreeMap<String, (String, ChangeReviewChangeKind)> {
    let mut entries = BTreeMap::new();
    for file in &status.files {
        let key = file.path.replace('\\', "/");
        let kind = status_change_kind(&file.status);
        entries
            .entry(key.clone())
            .and_modify(|(_, current_kind)| {
                if *current_kind == ChangeReviewChangeKind::Untracked
                    && kind != ChangeReviewChangeKind::Untracked
                {
                    *current_kind = kind;
                }
            })
            .or_insert((file.path.clone(), kind));
    }
    entries
}

fn numstat_entries(entries: Vec<GitNumstatEntry>) -> BTreeMap<String, GitNumstatEntry> {
    entries
        .into_iter()
        .map(|entry| (entry.path.replace('\\', "/"), entry))
        .collect()
}

fn build_files(
    cwd: &str,
    status: &GitStatusResult,
    numstats: Vec<GitNumstatEntry>,
    claims: &HashMap<String, Attribution>,
) -> Vec<ChangeReviewFileEntry> {
    let mut status_by_path = status_entries(status);
    let numstats = numstat_entries(numstats);
    let mut paths = BTreeSet::new();
    paths.extend(status_by_path.keys().cloned());
    paths.extend(numstats.keys().cloned());

    paths
        .into_iter()
        .filter_map(|path_key| {
            let status_entry = status_by_path.remove(&path_key);
            let numstat = numstats.get(&path_key);
            let path = status_entry
                .as_ref()
                .map(|(path, _)| path.clone())
                .or_else(|| numstat.map(|entry| entry.path.clone()))?;
            let kind = status_entry
                .as_ref()
                .map(|(_, kind)| *kind)
                .or_else(|| {
                    numstat.and_then(|entry| {
                        entry
                            .old_path
                            .as_ref()
                            .map(|_| ChangeReviewChangeKind::Renamed)
                    })
                })
                .unwrap_or(ChangeReviewChangeKind::Modified);
            let claim = claims.get(&normalized_path(cwd, &path));
            Some(ChangeReviewFileEntry {
                path,
                change_kind: kind,
                old_path: numstat.and_then(|entry| entry.old_path.clone()),
                insertions: numstat.and_then(|entry| entry.insertions),
                deletions: numstat.and_then(|entry| entry.deletions),
                evidence: if claim.is_some_and(|value| {
                    !value.agent_ids.is_empty() || !value.turn_indices.is_empty()
                }) {
                    ChangeReviewEvidence::Attributed
                } else {
                    ChangeReviewEvidence::Inferred
                },
                agent_ids: claim
                    .map(|value| value.agent_ids.iter().cloned().collect())
                    .unwrap_or_default(),
                turn_indices: claim
                    .map(|value| value.turn_indices.iter().copied().collect())
                    .unwrap_or_default(),
                binary: numstat.is_some_and(|entry| entry.binary),
                truncated: false,
                reviewed: false,
            })
        })
        .collect()
}

fn reviewed_path_matches(
    cwd: &str,
    file: &ChangeReviewFileEntry,
    reviewed_path: &ChangeReviewReviewedPath,
) -> bool {
    normalized_path(cwd, &file.path) == normalized_path(cwd, &reviewed_path.path)
        && file.change_kind == reviewed_path.change_kind
        && file.insertions == reviewed_path.insertions
        && file.deletions == reviewed_path.deletions
}

/// Paths whose content differs from the reviewed snapshot.
///
/// One `git diff --numstat` against the snapshot commit answers the question for
/// the whole change set, so the content anchor costs one invocation rather than
/// one per file.
fn paths_changed_since_snapshot(cwd: &str, snapshot: &str) -> Option<BTreeSet<String>> {
    let entries = git_diff_numstat_for_cwd(cwd, Some(snapshot)).ok()?;
    Some(
        entries
            .into_iter()
            .map(|entry| normalized_path(cwd, &entry.path))
            .collect(),
    )
}

fn annotate_reviewed_files(
    baseline: ChangeReviewBaseline,
    cwd: &str,
    files: &mut [ChangeReviewFileEntry],
    watermark: Option<&ChangeReviewWatermark>,
) {
    if baseline != ChangeReviewBaseline::Unreviewed {
        return;
    }

    // Prefer the content anchor. Phase 1 could only compare a numstat signature,
    // so a file edited back to its reviewed state read as reviewed by accident;
    // with a snapshot the same conclusion is reached on the bytes.
    let changed_since_snapshot = watermark
        .and_then(|value| value.reviewed_snapshot.as_deref())
        .filter(|snapshot| commit_resolves(cwd, snapshot))
        .and_then(|snapshot| paths_changed_since_snapshot(cwd, snapshot));

    for file in files {
        file.reviewed = match &changed_since_snapshot {
            // Reviewed means "identical to what I looked at", which is exactly
            // "absent from the diff against the snapshot I looked at".
            Some(changed) => !changed.contains(&normalized_path(cwd, &file.path)),
            None => watermark.is_some_and(|value| {
                value
                    .reviewed_paths
                    .iter()
                    .any(|reviewed_path| reviewed_path_matches(cwd, file, reviewed_path))
            }),
        };
    }
}

fn build_non_git_files(claims: &HashMap<String, Attribution>) -> Vec<ChangeReviewFileEntry> {
    let mut paths = claims.keys().cloned().collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let claim = &claims[&path];
            ChangeReviewFileEntry {
                path,
                change_kind: ChangeReviewChangeKind::Modified,
                old_path: None,
                insertions: None,
                deletions: None,
                evidence: ChangeReviewEvidence::Attributed,
                agent_ids: claim.agent_ids.iter().cloned().collect(),
                turn_indices: claim.turn_indices.iter().copied().collect(),
                binary: false,
                truncated: false,
                reviewed: false,
            }
        })
        .collect()
}

fn current_head(cwd: &str) -> Option<String> {
    run_git(cwd, &["rev-parse", "HEAD"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn branch_point(cwd: &str, head: Option<&str>) -> Option<String> {
    let head = head?;
    let symbolic_default = run_git(
        cwd,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());
    let candidates = symbolic_default.into_iter().chain([
        "origin/main".to_string(),
        "origin/master".to_string(),
        "main".to_string(),
        "master".to_string(),
    ]);
    for candidate in candidates {
        if let Ok(value) = run_git(cwd, &["merge-base", head, &candidate]) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    Some(head.to_string())
}

/// Resolves a turn-scoped baseline to a snapshot commit when one exists.
///
/// This is what Phase 2 buys: `last_effective_turn` and `conversation_start`
/// compare against real content rather than falling back to `HEAD`. An orphaned
/// snapshot — a commit lost to an operator rebase, amend, or `gc --prune` —
/// degrades to the Phase 1 behaviour instead of erroring.
fn snapshot_revision_for_baseline(
    cwd: &str,
    baseline: ChangeReviewBaseline,
    agent_id: Option<&str>,
    conversation_id: Option<&str>,
) -> Option<String> {
    let agent_id = agent_id?;
    let commit = match baseline {
        ChangeReviewBaseline::LastEffectiveTurn => {
            latest_snapshot_commit(cwd, agent_id, conversation_id)
        }
        ChangeReviewBaseline::ConversationStart => {
            first_snapshot_commit(cwd, agent_id, conversation_id)
        }
        _ => None,
    }?;
    commit_resolves(cwd, &commit).then_some(commit)
}

fn revision_for_baseline(
    cwd: &str,
    baseline: ChangeReviewBaseline,
    head: Option<&str>,
    watermark: Option<&ChangeReviewWatermark>,
    agent_id: Option<&str>,
    conversation_id: Option<&str>,
) -> Option<String> {
    match baseline {
        ChangeReviewBaseline::BranchPoint => branch_point(cwd, head),
        ChangeReviewBaseline::Head => head.map(ToString::to_string),
        ChangeReviewBaseline::LastEffectiveTurn | ChangeReviewBaseline::ConversationStart => {
            snapshot_revision_for_baseline(cwd, baseline, agent_id, conversation_id)
                .or_else(|| head.map(ToString::to_string))
        }
        ChangeReviewBaseline::Unreviewed => watermark
            .and_then(|value| value.reviewed_snapshot.as_deref())
            .filter(|commit| commit_resolves(cwd, commit))
            .map(ToString::to_string)
            .or_else(|| {
                watermark
                    .and_then(|value| value.reviewed_head.as_deref())
                    .filter(|revision| run_git(cwd, &["rev-parse", "--verify", revision]).is_ok())
                    .map(ToString::to_string)
            })
            .or_else(|| head.map(ToString::to_string)),
    }
}

fn load_change_review_for_state(
    request: &LoadChangeReviewRequest,
    state: &AppState,
) -> Result<ChangeReviewLoadResponse, String> {
    let cwd = request.cwd.trim();
    if cwd.is_empty() {
        return Err("workspace is required".to_string());
    }

    let (turns, skipped_turn_records) = read_turns_for_workspace(&state.conversation_archive, cwd)?;
    let (claims, first_turn, latest_effective_turn) = attribution_for_turns(cwd, &turns);
    let watermark = load_watermark(request.agent_id.as_deref(), cwd);
    let head = current_head(cwd);
    let active_conversation = request.agent_id.as_deref().and_then(|agent_id| {
        state
            .conversation_archive
            .active_conversation_id(agent_id)
            .ok()
            .flatten()
    });
    let snapshot_revision = snapshot_revision_for_baseline(
        cwd,
        request.baseline,
        request.agent_id.as_deref(),
        active_conversation.as_deref(),
    );
    let diff_revision = revision_for_baseline(
        cwd,
        request.baseline,
        head.as_deref(),
        watermark.as_ref(),
        request.agent_id.as_deref(),
        active_conversation.as_deref(),
    );
    // A turn-scoped baseline now carries a ref when a snapshot backs it, so the
    // surface can read real content. Without a snapshot it stays absent, which is
    // the Phase 1 signal that only a file list is available.
    let baseline_ref = match request.baseline {
        ChangeReviewBaseline::LastEffectiveTurn | ChangeReviewBaseline::ConversationStart => {
            snapshot_revision.clone()
        }
        _ => diff_revision.clone(),
    };
    let from_turn_index = match request.baseline {
        ChangeReviewBaseline::LastEffectiveTurn => latest_effective_turn,
        ChangeReviewBaseline::ConversationStart => first_turn,
        ChangeReviewBaseline::Unreviewed => watermark
            .as_ref()
            .map(|value| value.reviewed_turn_index.saturating_add(1)),
        ChangeReviewBaseline::BranchPoint | ChangeReviewBaseline::Head => None,
    };
    let to_turn_index = latest_effective_turn.or(first_turn);

    let status = match git_status_for_cwd(cwd) {
        Ok(status) => status,
        Err(_) => {
            let mut files = build_non_git_files(&claims);
            annotate_reviewed_files(request.baseline, cwd, &mut files, watermark.as_ref());
            return Ok(ChangeReviewLoadResponse {
                summary: ChangeReviewSummary {
                    schema: CHANGE_REVIEW_SCHEMA,
                    baseline: request.baseline,
                    baseline_ref: None,
                    from_turn_index,
                    to_turn_index,
                    files,
                    computed_at: chrono::Utc::now().to_rfc3339(),
                    truncated: false,
                    diverged: false,
                    turns_since_baseline: None,
                    paths_since_baseline: 0,
                },
                git_available: false,
                head_ref: None,
                skipped_turn_records,
            });
        }
    };

    let numstats = git_diff_numstat_for_cwd(cwd, diff_revision.as_deref()).unwrap_or_default();
    let mut files = build_files(cwd, &status, numstats, &claims);
    annotate_reviewed_files(request.baseline, cwd, &mut files, watermark.as_ref());

    let turns_since_baseline = match (from_turn_index, to_turn_index) {
        (Some(from), Some(to)) => Some(to.saturating_sub(from)),
        _ => None,
    };
    let paths_since_baseline = files.len() as u64;
    // Only a pinned baseline can accumulate unbounded cost. `head` and
    // `branch_point` are recomputed from the repository every time and pin
    // nothing, so they never warn.
    let diverged = matches!(request.baseline, ChangeReviewBaseline::ConversationStart)
        && baseline_diverged(turns_since_baseline.unwrap_or(0), paths_since_baseline);

    Ok(ChangeReviewLoadResponse {
        summary: ChangeReviewSummary {
            schema: CHANGE_REVIEW_SCHEMA,
            baseline: request.baseline,
            baseline_ref,
            from_turn_index,
            to_turn_index,
            files,
            computed_at: chrono::Utc::now().to_rfc3339(),
            truncated: false,
            diverged,
            turns_since_baseline,
            paths_since_baseline,
        },
        git_available: true,
        head_ref: head,
        skipped_turn_records,
    })
}

#[tauri::command]
pub async fn load_change_review(
    request: LoadChangeReviewRequest,
    state: State<'_, AppState>,
) -> Result<ChangeReviewLoadResponse, String> {
    load_change_review_for_state(&request, state.inner())
}

#[tauri::command]
pub async fn load_change_review_prefs(_app: AppHandle) -> Result<ChangeReviewPrefs, String> {
    Ok(get_wardian_home()
        .map(|home| load_prefs_from_home(&home))
        .unwrap_or_default())
}

#[tauri::command]
pub async fn save_change_review_prefs(
    prefs: ChangeReviewPrefs,
    _app: AppHandle,
) -> Result<(), String> {
    let home = get_wardian_home().ok_or_else(|| "Could not find home directory".to_string())?;
    save_prefs_to_home(&home, &prefs)
}

#[tauri::command]
pub async fn load_change_review_watermark(
    agent_id: String,
    workspace: String,
) -> Result<Option<ChangeReviewWatermark>, String> {
    Ok(load_watermark(Some(&agent_id), &workspace))
}

#[tauri::command]
pub async fn save_change_review_watermark(watermark: ChangeReviewWatermark) -> Result<(), String> {
    let home = get_wardian_home().ok_or_else(|| "Could not find home directory".to_string())?;
    let changes_dir = home.join("changes");
    std::fs::create_dir_all(&changes_dir).map_err(|error| error.to_string())?;
    let path = watermark_path(&home);
    let mut index = load_watermark_index(&home);
    // The content anchor is resolved here rather than sent by the caller: the
    // frontend has no notion of snapshot commits, and the latest snapshot at the
    // moment of review is exactly what "what I looked at" means.
    let reviewed_snapshot = watermark
        .reviewed_snapshot
        .clone()
        .or_else(|| latest_snapshot_commit(&watermark.workspace, &watermark.agent_id, None));
    index.insert(
        watermark_key(&watermark.agent_id, &watermark.workspace),
        ChangeReviewWatermark {
            schema: CHANGE_REVIEW_SCHEMA,
            reviewed_snapshot,
            ..watermark
        },
    );
    let json = serde_json::to_string_pretty(&index).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::conversations::{ConversationBoundaryReason, ConversationStatus};
    use wardian_core::models::git::GitFileEntry;

    fn conversation_entry(
        conversation_id: &str,
        started_at: &str,
        ended_at: Option<&str>,
    ) -> ConversationIndexEntry {
        ConversationIndexEntry {
            schema: 1,
            conversation_id: conversation_id.to_string(),
            agent_id: "agent-1".to_string(),
            agent_name: "Reviewer".to_string(),
            agent_class: "default".to_string(),
            workspace: "C:/repo".to_string(),
            provider: "mock".to_string(),
            provider_session_ids: Vec::new(),
            started_at: started_at.to_string(),
            ended_at: ended_at.map(str::to_string),
            status: ConversationStatus::Closed,
            boundary_reason: ConversationBoundaryReason::Shutdown,
            first_prompt_excerpt: None,
            last_record_excerpt: None,
            record_count: 0,
            turn_count: 0,
            has_turns: false,
            lifecycle_only: false,
            artifact_count: 0,
            path: format!("C:/archive/{conversation_id}"),
        }
    }

    fn status_for(path: &str, status: &str) -> GitStatusResult {
        GitStatusResult {
            branch: "main".to_string(),
            upstream: None,
            has_upstream: false,
            files: vec![GitFileEntry {
                path: path.to_string(),
                status: status.to_string(),
                is_staged: false,
            }],
            ahead: 0,
            behind: 0,
            rebase_in_progress: false,
        }
    }

    #[test]
    fn bounded_turn_read_keeps_active_conversation_outside_recent_window() {
        let entries = (0..22)
            .map(|index| {
                let timestamp = format!("2026-08-01T00:{index:02}:00.000Z");
                conversation_entry(&format!("conv-{index}"), &timestamp, Some(&timestamp))
            })
            .collect::<Vec<_>>();
        let active_conversation_ids = ["conv-0".to_string()].into_iter().collect();

        let selected =
            select_conversation_entries_for_change_review(entries, &active_conversation_ids);

        assert_eq!(selected.len(), CHANGE_REVIEW_RECENT_CONVERSATION_LIMIT + 1);
        assert!(selected
            .iter()
            .any(|entry| entry.conversation_id == "conv-0"));
        assert!(selected
            .iter()
            .any(|entry| entry.conversation_id == "conv-21"));
        assert!(!selected
            .iter()
            .any(|entry| entry.conversation_id == "conv-1"));
    }

    #[test]
    fn unclaimed_git_file_is_inferred_with_empty_attribution() {
        let files = build_files(
            "C:/repo",
            &status_for("src/shell-written.ts", "M"),
            vec![GitNumstatEntry {
                path: "src/shell-written.ts".to_string(),
                old_path: None,
                insertions: Some(2),
                deletions: Some(1),
                binary: false,
            }],
            &HashMap::new(),
        );

        assert_eq!(files[0].evidence, ChangeReviewEvidence::Inferred);
        assert!(files[0].agent_ids.is_empty());
        assert!(files[0].turn_indices.is_empty());
    }

    #[test]
    fn claimed_git_file_is_attributed_without_filtering_other_files() {
        let mut claims = HashMap::new();
        add_claim(&mut claims, "C:/repo", "src/agent.ts", "agent-1", 7);
        let status = GitStatusResult {
            files: vec![
                GitFileEntry {
                    path: "src/agent.ts".to_string(),
                    status: "M".to_string(),
                    is_staged: false,
                },
                GitFileEntry {
                    path: "src/shell.ts".to_string(),
                    status: "M".to_string(),
                    is_staged: false,
                },
            ],
            ..status_for("unused", "M")
        };
        let files = build_files("C:/repo", &status, Vec::new(), &claims);

        assert_eq!(files.len(), 2);
        let attributed = files
            .iter()
            .find(|file| file.path == "src/agent.ts")
            .unwrap();
        assert_eq!(attributed.evidence, ChangeReviewEvidence::Attributed);
        assert_eq!(attributed.agent_ids, vec!["agent-1"]);
        assert_eq!(attributed.turn_indices, vec![7]);
        let inferred = files
            .iter()
            .find(|file| file.path == "src/shell.ts")
            .unwrap();
        assert_eq!(inferred.evidence, ChangeReviewEvidence::Inferred);
    }

    #[test]
    fn unreviewed_watermark_keeps_unclaimed_shell_path() {
        let mut files = build_files(
            "C:/repo",
            &status_for("src/shell-written.ts", "M"),
            vec![GitNumstatEntry {
                path: "src/shell-written.ts".to_string(),
                old_path: None,
                insertions: Some(2),
                deletions: Some(1),
                binary: false,
            }],
            &HashMap::new(),
        );
        let watermark = ChangeReviewWatermark {
            schema: CHANGE_REVIEW_SCHEMA,
            agent_id: "agent-1".to_string(),
            workspace: "C:/repo".to_string(),
            reviewed_turn_index: 3,
            reviewed_at: "2026-08-01T00:00:00Z".to_string(),
            reviewed_head: Some("head-1".to_string()),
            reviewed_snapshot: None,
            reviewed_paths: vec![ChangeReviewReviewedPath {
                path: "src/agent-written.ts".to_string(),
                change_kind: ChangeReviewChangeKind::Modified,
                insertions: Some(1),
                deletions: Some(0),
            }],
        };

        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            "C:/repo",
            &mut files,
            Some(&watermark),
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].evidence, ChangeReviewEvidence::Inferred);
        assert!(files[0].agent_ids.is_empty());
        assert!(!files[0].reviewed);
    }

    #[test]
    fn reviewed_path_signature_marks_only_the_matching_entry() {
        let status = GitStatusResult {
            files: vec![
                GitFileEntry {
                    path: "src/kept.ts".to_string(),
                    status: "M".to_string(),
                    is_staged: false,
                },
                GitFileEntry {
                    path: "src/changed.ts".to_string(),
                    status: "M".to_string(),
                    is_staged: false,
                },
            ],
            ..status_for("unused", "M")
        };
        let mut files = build_files(
            "C:/repo",
            &status,
            vec![
                GitNumstatEntry {
                    path: "src/kept.ts".to_string(),
                    old_path: None,
                    insertions: Some(2),
                    deletions: Some(1),
                    binary: false,
                },
                GitNumstatEntry {
                    path: "src/changed.ts".to_string(),
                    old_path: None,
                    insertions: Some(3),
                    deletions: Some(1),
                    binary: false,
                },
            ],
            &HashMap::new(),
        );
        let watermark = ChangeReviewWatermark {
            schema: CHANGE_REVIEW_SCHEMA,
            agent_id: "agent-1".to_string(),
            workspace: "C:/repo".to_string(),
            reviewed_turn_index: 3,
            reviewed_at: "2026-08-01T00:00:00Z".to_string(),
            reviewed_head: Some("head-1".to_string()),
            reviewed_snapshot: None,
            reviewed_paths: vec![ChangeReviewReviewedPath {
                path: "src/kept.ts".to_string(),
                change_kind: ChangeReviewChangeKind::Modified,
                insertions: Some(2),
                deletions: Some(1),
            }],
        };

        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            "C:/repo",
            &mut files,
            Some(&watermark),
        );

        assert!(
            files
                .iter()
                .find(|file| file.path == "src/kept.ts")
                .unwrap()
                .reviewed
        );
        assert!(
            !files
                .iter()
                .find(|file| file.path == "src/changed.ts")
                .unwrap()
                .reviewed
        );
    }

    #[test]
    fn an_unreviewed_baseline_never_empties_a_non_empty_git_change_set() {
        let mut files = build_files(
            "C:/repo",
            &status_for("src/current.ts", "M"),
            Vec::new(),
            &HashMap::new(),
        );
        let watermark = ChangeReviewWatermark {
            schema: CHANGE_REVIEW_SCHEMA,
            agent_id: "agent-1".to_string(),
            workspace: "C:/repo".to_string(),
            reviewed_turn_index: 3,
            reviewed_at: "2026-08-01T00:00:00Z".to_string(),
            reviewed_head: Some("head-1".to_string()),
            reviewed_snapshot: None,
            reviewed_paths: Vec::new(),
        };

        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            "C:/repo",
            &mut files,
            Some(&watermark),
        );

        assert_eq!(files.len(), 1);
        assert!(!files[0].reviewed);
    }

    #[cfg(not(windows))]
    #[test]
    fn normalized_path_preserves_case_on_case_sensitive_targets() {
        assert_ne!(
            normalized_path("/repo", "/repo/src/README.md"),
            normalized_path("/repo", "/repo/src/readme.md"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn normalized_path_is_case_insensitive_on_windows() {
        assert_eq!(
            normalized_path("C:/repo", "C:/repo/src/README.md"),
            normalized_path("c:/REPO", "c:/repo/src/readme.md"),
        );
    }

    #[test]
    fn missing_change_review_prefs_use_the_default_baseline() {
        let temp = tempfile::tempdir().expect("temp home");

        assert_eq!(
            load_prefs_from_home(temp.path()),
            ChangeReviewPrefs::default()
        );
    }

    #[test]
    fn unparseable_change_review_prefs_use_the_default_baseline() {
        let temp = tempfile::tempdir().expect("temp home");
        let changes_dir = temp.path().join("changes");
        std::fs::create_dir_all(&changes_dir).expect("changes directory");
        std::fs::write(prefs_path(temp.path()), "not json").expect("prefs file");

        assert_eq!(
            load_prefs_from_home(temp.path()),
            ChangeReviewPrefs::default()
        );
    }

    #[test]
    fn saving_change_review_prefs_creates_the_global_preferences_file() {
        let temp = tempfile::tempdir().expect("temp home");
        let prefs = ChangeReviewPrefs {
            schema: CHANGE_REVIEW_SCHEMA,
            baseline: ChangeReviewBaseline::BranchPoint,
        };

        save_prefs_to_home(temp.path(), &prefs).expect("save prefs");

        assert_eq!(load_prefs_from_home(temp.path()), prefs);
        assert!(prefs_path(temp.path()).is_file());
    }

    #[test]
    fn malformed_turn_record_keeps_the_change_set_and_reports_skipped_count() {
        let _env_guard = crate::utils::wardian_test_env_lock();
        let previous_home = std::env::var_os("WARDIAN_HOME");
        let home = tempfile::tempdir().expect("temporary Wardian home");
        std::env::set_var("WARDIAN_HOME", home.path());

        let workspace = home.path().join("workspace");
        let workspace_string = workspace.to_string_lossy().to_string();
        std::fs::create_dir_all(workspace.join("src")).expect("workspace");
        run_git(&workspace_string, &["init"]).expect("initialize git workspace");
        std::fs::write(workspace.join("src/attributed.ts"), "attributed\n")
            .expect("attributed file");
        std::fs::write(workspace.join("src/shell-written.ts"), "shell\n")
            .expect("shell-written file");

        let conversation_directory =
            wardian_core::paths::agent_conversation_dir("agent-1", "conv-1")
                .expect("conversation directory");
        std::fs::create_dir_all(&conversation_directory).expect("conversation directory exists");
        let index_path = wardian_core::paths::agent_conversations_dir("agent-1")
            .expect("agent conversations directory")
            .join("index.jsonl");
        let index_entry = serde_json::json!({
            "schema": 1,
            "conversation_id": "conv-1",
            "agent_id": "agent-1",
            "agent_name": "Reviewer",
            "agent_class": "default",
            "workspace": workspace_string,
            "provider": "mock",
            "provider_session_ids": [],
            "started_at": "2026-08-01T00:00:00.000Z",
            "ended_at": null,
            "status": "open",
            "boundary_reason": "spawn",
            "first_prompt_excerpt": null,
            "last_record_excerpt": null,
            "record_count": 1,
            "turn_count": 1,
            "has_turns": true,
            "lifecycle_only": false,
            "artifact_count": 0,
            "path": conversation_directory.to_string_lossy()
        });
        std::fs::write(
            &index_path,
            format!("{}\n", serde_json::to_string(&index_entry).unwrap()),
        )
        .expect("conversation index");

        let valid_turn = serde_json::json!({
            "schema": 3,
            "conversation_id": "conv-1",
            "turn_index": 1,
            "turn_key": "conv-1:turn:000001",
            "status": "responded",
            "status_source": "unknown",
            "seq_start": 1,
            "seq_end": 1,
            "started_at": "2026-08-01T00:00:01.000Z",
            "updated_at": "2026-08-01T00:00:01.000Z",
            "request": {
                "seq": 1,
                "kind": "user_request",
                "text": null,
                "text_truncated": false
            },
            "counts": {
                "records": 1,
                "assistant_messages": 0,
                "tool_calls": 0,
                "tool_results": 0,
                "nonzero_tool_results": 0,
                "failed_tool_results": 0,
                "timeouts": 0
            },
            "tools_used": {},
            "files": {
                "read": [],
                "written": ["src/attributed.ts"],
                "mentioned": []
            },
            "external_side_effects": [],
            "failure_signals": [],
            "record_refs": {
                "seq_start": 1,
                "seq_end": 1
            }
        });
        std::fs::write(
            conversation_directory.join("turns.jsonl"),
            format!(
                "{}\n{{not-json}}\n",
                serde_json::to_string(&valid_turn).unwrap()
            ),
        )
        .expect("turn records");

        let state = AppState::new();
        let response = load_change_review_for_state(
            &LoadChangeReviewRequest {
                cwd: workspace_string,
                baseline: ChangeReviewBaseline::LastEffectiveTurn,
                agent_id: Some("agent-1".to_string()),
            },
            &state,
        )
        .expect("change review response");

        assert_eq!(response.skipped_turn_records, 1);
        assert!(response
            .summary
            .files
            .iter()
            .any(|file| file.path == "src/shell-written.ts"));
        let attributed = response
            .summary
            .files
            .iter()
            .find(|file| file.path == "src/attributed.ts")
            .expect("attributed path");
        assert_eq!(attributed.evidence, ChangeReviewEvidence::Attributed);

        match previous_home {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }

    // ---- Phase 2: snapshot-backed baselines --------------------------------

    struct SnapshotRepo {
        _home: tempfile::TempDir,
        repo: tempfile::TempDir,
        previous_home: Option<std::ffi::OsString>,
        // `WARDIAN_HOME` is process-global and the coverage job runs tests in
        // parallel, so the home directory has to be serialized.
        _env_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl SnapshotRepo {
        fn new() -> Self {
            let _env_guard = crate::utils::wardian_test_env_lock();
            let home = tempfile::tempdir().unwrap();
            let repo = tempfile::tempdir().unwrap();
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home.path());

            let this = Self {
                _home: home,
                repo,
                previous_home,
                _env_guard,
            };
            let cwd = this.cwd().to_string();
            run_git(&cwd, &["init"]).unwrap();
            run_git(&cwd, &["config", "user.email", "test@example.com"]).unwrap();
            run_git(&cwd, &["config", "user.name", "Test"]).unwrap();
            run_git(&cwd, &["config", "commit.gpgsign", "false"]).unwrap();
            this.write("tracked.txt", "committed\n");
            run_git(&cwd, &["add", "-A"]).unwrap();
            run_git(&cwd, &["commit", "-m", "initial"]).unwrap();
            this
        }

        fn cwd(&self) -> &str {
            self.repo.path().to_str().unwrap()
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.repo.path().join(name), contents).unwrap();
        }

        fn snapshot(&self, turn_index: u64) -> String {
            let request = crate::commands::change_snapshot::SnapshotRequest {
                cwd: self.cwd().to_string(),
                agent_id: "agent-1".to_string(),
                conversation_id: "conv-1".to_string(),
                turn_index,
            };
            match crate::commands::change_snapshot::take_snapshot(&request).unwrap() {
                crate::commands::change_snapshot::SnapshotOutcome::Created(snapshot) => {
                    snapshot.commit_id.clone()
                }
                other => panic!("expected a snapshot, got {other:?}"),
            }
        }
    }

    impl Drop for SnapshotRepo {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    #[test]
    fn a_turn_scoped_baseline_resolves_to_a_snapshot_commit() {
        let repo = SnapshotRepo::new();
        repo.write("tracked.txt", "turn one\n");
        let first = repo.snapshot(1);
        repo.write("tracked.txt", "turn two\n");
        let latest = repo.snapshot(2);

        assert_eq!(
            snapshot_revision_for_baseline(
                repo.cwd(),
                ChangeReviewBaseline::ConversationStart,
                Some("agent-1"),
                Some("conv-1"),
            ),
            Some(first),
        );
        assert_eq!(
            snapshot_revision_for_baseline(
                repo.cwd(),
                ChangeReviewBaseline::LastEffectiveTurn,
                Some("agent-1"),
                Some("conv-1"),
            ),
            Some(latest),
        );
    }

    #[test]
    fn a_baseline_without_an_agent_falls_back_to_head() {
        let repo = SnapshotRepo::new();
        repo.write("tracked.txt", "turn one\n");
        repo.snapshot(1);
        let head = current_head(repo.cwd());

        // Without an agent there is nothing to attribute a snapshot to, so the
        // Phase 1 behaviour has to survive intact.
        assert_eq!(
            snapshot_revision_for_baseline(
                repo.cwd(),
                ChangeReviewBaseline::LastEffectiveTurn,
                None,
                None,
            ),
            None,
        );
        assert_eq!(
            revision_for_baseline(
                repo.cwd(),
                ChangeReviewBaseline::LastEffectiveTurn,
                head.as_deref(),
                None,
                None,
                None,
            ),
            head,
        );
    }

    #[test]
    fn a_file_reverted_to_its_reviewed_content_reads_as_reviewed() {
        // This is the Phase 1 limitation the content anchor removes. Phase 1
        // compared a numstat signature and could not tell a revert from a fresh
        // edit; the snapshot compares bytes.
        let repo = SnapshotRepo::new();
        repo.write("tracked.txt", "reviewed state\n");
        let reviewed = repo.snapshot(1);

        let watermark = ChangeReviewWatermark {
            schema: CHANGE_REVIEW_SCHEMA,
            agent_id: "agent-1".to_string(),
            workspace: repo.cwd().to_string(),
            reviewed_turn_index: 1,
            reviewed_at: "2026-08-02T00:00:00Z".to_string(),
            reviewed_head: None,
            reviewed_snapshot: Some(reviewed),
            reviewed_paths: Vec::new(),
        };

        let mut files = vec![ChangeReviewFileEntry {
            path: "tracked.txt".to_string(),
            change_kind: ChangeReviewChangeKind::Modified,
            old_path: None,
            insertions: Some(1),
            deletions: Some(1),
            evidence: ChangeReviewEvidence::Inferred,
            agent_ids: Vec::new(),
            turn_indices: Vec::new(),
            binary: false,
            truncated: false,
            reviewed: false,
        }];

        // The working tree still matches what was reviewed.
        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            repo.cwd(),
            &mut files,
            Some(&watermark),
        );
        assert!(
            files[0].reviewed,
            "content identical to the snapshot is reviewed"
        );

        // Edit it, and it stops being reviewed.
        repo.write("tracked.txt", "edited after review\n");
        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            repo.cwd(),
            &mut files,
            Some(&watermark),
        );
        assert!(
            !files[0].reviewed,
            "content differing from the snapshot is not reviewed"
        );

        // Revert it, and it is reviewed again. Phase 1 could not reach this.
        repo.write("tracked.txt", "reviewed state\n");
        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            repo.cwd(),
            &mut files,
            Some(&watermark),
        );
        assert!(
            files[0].reviewed,
            "a revert to the reviewed bytes reads as reviewed"
        );
    }

    #[test]
    fn the_content_anchor_never_removes_a_path_from_the_change_set() {
        // The annotate-never-filter rule, restated against the new mechanism:
        // a snapshot is a better baseline, never a filter.
        let repo = SnapshotRepo::new();
        repo.write("tracked.txt", "reviewed state\n");
        let reviewed = repo.snapshot(1);
        repo.write("shell-written.txt", "written through a shell\n");

        let watermark = ChangeReviewWatermark {
            schema: CHANGE_REVIEW_SCHEMA,
            agent_id: "agent-1".to_string(),
            workspace: repo.cwd().to_string(),
            reviewed_turn_index: 1,
            reviewed_at: "2026-08-02T00:00:00Z".to_string(),
            reviewed_head: None,
            reviewed_snapshot: Some(reviewed),
            reviewed_paths: Vec::new(),
        };

        let mut files = vec![
            ChangeReviewFileEntry {
                path: "tracked.txt".to_string(),
                change_kind: ChangeReviewChangeKind::Modified,
                old_path: None,
                insertions: Some(1),
                deletions: Some(1),
                evidence: ChangeReviewEvidence::Inferred,
                agent_ids: Vec::new(),
                turn_indices: Vec::new(),
                binary: false,
                truncated: false,
                reviewed: false,
            },
            ChangeReviewFileEntry {
                path: "shell-written.txt".to_string(),
                change_kind: ChangeReviewChangeKind::Untracked,
                old_path: None,
                insertions: None,
                deletions: None,
                evidence: ChangeReviewEvidence::Inferred,
                agent_ids: Vec::new(),
                turn_indices: Vec::new(),
                binary: false,
                truncated: false,
                reviewed: false,
            },
        ];

        annotate_reviewed_files(
            ChangeReviewBaseline::Unreviewed,
            repo.cwd(),
            &mut files,
            Some(&watermark),
        );

        assert_eq!(files.len(), 2, "annotation must never drop an entry");
        assert!(files.iter().any(|file| file.path == "shell-written.txt"));
    }
}
