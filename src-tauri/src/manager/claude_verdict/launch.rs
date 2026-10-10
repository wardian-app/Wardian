//! Per-launch verification files and the incremental tracker the Claude
//! watcher polls.
//!
//! Each interactive Claude launch owns
//! `<wardian-home>/agents/<session>/claude/turn-verdicts/<launch>/`. One
//! process writes one directory, so its event sequence numbers never mix with
//! another process's, including a resumed session's new process.

use super::receiver::TelemetryReceiver;
use super::telemetry_config::{existing_telemetry, loopback_telemetry_env};
use super::{
    verify_prompt, LedgerInsert, StopRoundLog, TelemetryEvent, TelemetryLedger, Verdict,
    VerifiedCompletion, COMMITTED_FILE, LAUNCH_FILE, STOP_ROUNDS_FILE, TELEMETRY_FILE,
};
use crate::manager::turn_completion::TurnCompletion;
use crate::utils::fs::ClaudeStopRoundCapture;
use crate::utils::logging::log_debug;
use std::collections::HashSet;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const TURN_VERDICTS_DIR: &str = "turn-verdicts";
/// Launch directories kept per agent, including the new one.
const RETAINED_LAUNCHES: usize = 8;
const VERIFIED_TIMESTAMP_SOURCE: &str = "claude_stop_hooks_settled_event";

fn verdict_root(session_id: &str) -> Option<PathBuf> {
    crate::utils::fs::get_wardian_home().map(|home| {
        home.join("agents")
            .join(session_id)
            .join("claude")
            .join(TURN_VERDICTS_DIR)
    })
}

/// Launch directories for `session_id`, oldest first.
pub(crate) fn launch_dirs(session_id: &str) -> Vec<PathBuf> {
    verdict_root(session_id)
        .map(|root| child_dirs(&root))
        .unwrap_or_default()
}

fn child_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    // Names start with the zero-padded launch time.
    dirs.sort();
    dirs
}

/// Removes all but the newest `keep` launch directories under `root`.
fn prune_launches(root: &Path, keep: usize) {
    let Ok(canonical_root) = root.canonicalize() else {
        return;
    };
    let dirs = child_dirs(root);
    let excess = dirs.len().saturating_sub(keep);
    for dir in dirs.into_iter().take(excess) {
        // Recursive deletion is confined to direct children of this agent's
        // verdict root, resolved through any links.
        let inside_root = dir
            .canonicalize()
            .ok()
            .is_some_and(|resolved| resolved.parent() == Some(canonical_root.as_path()));
        if inside_root {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                log_debug(&format!(
                    "[Wardian] Could not remove retired Claude launch record {}: {error}",
                    dir.display()
                ));
            }
        }
    }
}

/// A prepared Claude launch: its directory and, when Wardian may own the
/// launch's telemetry, the running receiver.
pub(crate) struct VerdictLaunch {
    dir: PathBuf,
    receiver: Option<TelemetryReceiver>,
}

impl VerdictLaunch {
    /// Creates the launch directory and starts the receiver unless existing
    /// telemetry configuration applies to `project_dir`.
    pub(crate) fn prepare(session_id: &str, project_dir: &Path) -> Result<Self, String> {
        let root = verdict_root(session_id).ok_or("Could not find Wardian home")?;
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        prune_launches(&root, RETAINED_LAUNCHES - 1);
        let dir = root.join(format!(
            "{:013}-{}",
            chrono::Utc::now().timestamp_millis().max(0),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

        let (receiver, unavailable) = match existing_telemetry(project_dir) {
            Some(existing) => (None, Some(existing.describe())),
            None => match TelemetryReceiver::start(dir.join(TELEMETRY_FILE)) {
                Ok(receiver) => (Some(receiver), None),
                Err(error) => (None, Some(error)),
            },
        };
        if let Some(reason) = unavailable.as_deref() {
            log_debug(&format!(
                "[Wardian] Verified Claude completions are unavailable for {session_id}: {reason}"
            ));
        }
        let verification = if receiver.is_some() {
            "available"
        } else {
            "unavailable"
        };
        let record = serde_json::json!({
            "verification": verification,
            "unavailable_reason": unavailable,
        });
        std::fs::write(dir.join(LAUNCH_FILE), record.to_string())
            .map_err(|error| error.to_string())?;
        Ok(Self { dir, receiver })
    }

    /// The Stop round capture for this launch's hook settings, or `None` when
    /// verification is unavailable and Stop records could never be verified.
    pub(crate) fn hook_capture(&self) -> Option<ClaudeStopRoundCapture> {
        self.receiver.as_ref().map(|receiver| ClaudeStopRoundCapture {
            round_log: self.dir.join(STOP_ROUNDS_FILE),
            settings_env: Some(loopback_telemetry_env(receiver.endpoint())),
        })
    }

    /// Hands the launch to the watcher; the receiver stops when the tracker
    /// is dropped.
    pub(crate) fn into_tracker(self, accepted_sessions: Vec<String>) -> VerdictTracker {
        VerdictTracker::open(self.dir, accepted_sessions, self.receiver)
    }
}

/// Incrementally reads one launch's files and reports newly verified prompts.
pub(crate) struct VerdictTracker {
    dir: PathBuf,
    accepted_sessions: Vec<String>,
    ledger: TelemetryLedger,
    ledger_offset: u64,
    ledger_partial: String,
    round_log_len: Option<u64>,
    round_log: StopRoundLog,
    resolved: HashSet<String>,
    _receiver: Option<TelemetryReceiver>,
}

impl VerdictTracker {
    pub(crate) fn open(
        dir: PathBuf,
        accepted_sessions: Vec<String>,
        receiver: Option<TelemetryReceiver>,
    ) -> Self {
        let resolved = std::fs::read_to_string(dir.join(COMMITTED_FILE))
            .map(|contents| {
                contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            dir,
            accepted_sessions,
            ledger: TelemetryLedger::default(),
            ledger_offset: 0,
            ledger_partial: String::new(),
            round_log_len: None,
            round_log: StopRoundLog::default(),
            resolved,
            _receiver: receiver,
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads new ledger and round records and returns prompts verified since
    /// the last poll. Unverifiable prompts are logged once and dropped.
    pub(crate) fn poll(&mut self) -> Vec<VerifiedCompletion> {
        // The ledger is read first: a Stop record is on disk before Claude
        // emits the round's completion event, so a completion seen here has
        // its record in the round log read next.
        let ledger_changed = self.read_ledger();
        let rounds_changed = self.read_round_log();
        if !ledger_changed && !rounds_changed {
            return Vec::new();
        }
        let mut verified = Vec::new();
        for prompt_id in self.ledger.stop_prompt_ids() {
            if self.resolved.contains(&prompt_id) {
                continue;
            }
            match verify_prompt(
                &self.ledger,
                &self.round_log,
                &prompt_id,
                &self.accepted_sessions,
            ) {
                Verdict::Verified(completion) => {
                    self.resolved.insert(prompt_id);
                    verified.push(completion);
                }
                Verdict::Unknown(reason) => {
                    log_debug(&format!(
                        "[Wardian] Claude prompt {prompt_id} has no verified final answer: {reason:?}"
                    ));
                    self.resolved.insert(prompt_id);
                }
                Verdict::Pending => {}
            }
        }
        verified
    }

    fn read_ledger(&mut self) -> bool {
        let Ok(mut file) = std::fs::File::open(self.dir.join(TELEMETRY_FILE)) else {
            return false;
        };
        let mut appended = String::new();
        if file
            .seek(std::io::SeekFrom::Start(self.ledger_offset))
            .and_then(|_| file.read_to_string(&mut appended))
            .is_err()
        {
            return false;
        }
        if appended.is_empty() {
            return false;
        }
        self.ledger_offset += appended.len() as u64;
        self.ledger_partial.push_str(&appended);
        let Some(end) = self.ledger_partial.rfind('\n') else {
            return false;
        };
        let complete = self.ledger_partial[..end].to_string();
        self.ledger_partial.replace_range(..=end, "");
        let mut changed = false;
        for line in complete.split('\n').filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<TelemetryEvent>(line) {
                Ok(event) => changed |= self.ledger.insert(event) != LedgerInsert::Duplicate,
                Err(_) => {
                    self.ledger.mark_conflicted();
                    changed = true;
                }
            }
        }
        changed
    }

    fn read_round_log(&mut self) -> bool {
        let path = self.dir.join(STOP_ROUNDS_FILE);
        let len = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
        if len == self.round_log_len {
            return false;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return false;
        };
        self.round_log_len = len;
        self.round_log = StopRoundLog::parse(&contents);
        true
    }
}

/// Records that `prompt_id`'s card is durably queued, so startup replay does
/// not recreate a card the user has since cleared.
pub(crate) fn record_committed(dir: &Path, prompt_id: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(COMMITTED_FILE))?;
    file.write_all(format!("{prompt_id}\n").as_bytes())?;
    file.sync_data()
}

/// The Inbox completion for a verified answer, timestamped by Claude's own
/// settled-Stop event when it carries a parsable time.
pub(crate) fn inbox_completion(verified: &VerifiedCompletion) -> Option<TurnCompletion> {
    let (timestamp, timestamp_source) = verified
        .completed_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|time| (time.timestamp_millis(), VERIFIED_TIMESTAMP_SOURCE))
        .unwrap_or_else(|| {
            (
                chrono::Utc::now().timestamp_millis(),
                "processing_time_fallback",
            )
        });
    TurnCompletion::new(
        &verified.prompt_id,
        &verified.response_text,
        timestamp,
        timestamp_source,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .expect("open");
        file.write_all(contents.as_bytes()).expect("write");
    }

    fn line<T: serde::Serialize>(value: &T) -> String {
        format!("{}\n", serde_json::to_string(value).expect("serialize"))
    }

    fn turn(prompt_id: &str) -> Vec<TelemetryEvent> {
        let event = |sequence: u64, name: &str, hook: Option<super::super::HookExecution>| {
            TelemetryEvent {
                sequence,
                name: name.to_string(),
                prompt_id: Some(prompt_id.to_string()),
                session_id: Some("session".to_string()),
                timestamp: Some("2026-10-10T07:00:00.000Z".to_string()),
                hook,
            }
        };
        let hook = |complete: bool| super::super::HookExecution {
            hook_event: "Stop".to_string(),
            num_hooks: Some(1),
            num_success: complete.then_some(1),
            num_blocking: complete.then_some(0),
            num_non_blocking_error: complete.then_some(0),
            num_cancelled: complete.then_some(0),
            additional_context_chars: complete.then_some(0),
        };
        vec![
            event(0, "user_prompt", None),
            event(1, "hook_execution_start", Some(hook(false))),
            event(2, "hook_execution_complete", Some(hook(true))),
        ]
    }

    fn round(prompt_id: &str) -> serde_json::Value {
        serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": "session",
            "prompt_id": prompt_id,
            "last_assistant_message": "answer",
        })
    }

    #[test]
    fn tracker_reports_a_verified_prompt_once_and_honours_commits() {
        let dir = tempfile::tempdir().expect("temp dir");
        let accepted = vec!["session".to_string()];
        write(dir.path(), STOP_ROUNDS_FILE, &line(&round("p1")));
        let events = turn("p1");
        // A partially written ledger line is completed by a later append.
        let first = line(&events[0]) + &line(&events[1]);
        let last = line(&events[2]);
        let (head, tail) = last.split_at(10);
        write(dir.path(), TELEMETRY_FILE, &(first + head));

        let mut tracker = VerdictTracker::open(dir.path().to_path_buf(), accepted.clone(), None);
        assert!(tracker.poll().is_empty());
        write(dir.path(), TELEMETRY_FILE, tail);
        let verified = tracker.poll();
        assert_eq!(verified.len(), 1);
        assert_eq!(verified[0].response_text, "answer");
        assert!(tracker.poll().is_empty());

        let completion = inbox_completion(&verified[0]).expect("completion");
        assert_eq!(completion.timestamp_source, VERIFIED_TIMESTAMP_SOURCE);
        assert_eq!(completion.evidence_id, "p1");

        record_committed(dir.path(), "p1").expect("commit");
        let mut replay = VerdictTracker::open(dir.path().to_path_buf(), accepted, None);
        assert!(replay.poll().is_empty());
    }

    #[test]
    fn pruning_keeps_the_newest_launches_and_only_touches_children() {
        let root = tempfile::tempdir().expect("temp dir");
        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("keep.txt"), "x").expect("outside file");
        for index in 0..5 {
            std::fs::create_dir_all(root.path().join(format!("{index:013}-launch")))
                .expect("launch dir");
        }
        prune_launches(root.path(), 2);
        let remaining = child_dirs(root.path())
            .into_iter()
            .filter_map(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()))
            .collect::<Vec<_>>();
        assert_eq!(remaining, ["0000000000003-launch", "0000000000004-launch"]);
        assert!(outside.path().join("keep.txt").exists());
    }

    #[test]
    fn a_corrupt_ledger_line_leaves_prompts_unverified() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), STOP_ROUNDS_FILE, &line(&round("p1")));
        let mut ledger = turn("p1").iter().map(line).collect::<String>();
        ledger.push_str("{corrupt\n");
        write(dir.path(), TELEMETRY_FILE, &ledger);
        let mut tracker =
            VerdictTracker::open(dir.path().to_path_buf(), vec!["session".to_string()], None);
        assert!(tracker.poll().is_empty());
    }
}
