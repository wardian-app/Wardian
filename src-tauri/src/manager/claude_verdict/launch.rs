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
    settle_prompt, LedgerInsert, SettledCompletion, StopRoundLog, TelemetryEvent, TelemetryLedger,
    UnknownReason, Verdict, COMMITTED_FILE, LAUNCH_FILE, STOP_ROUNDS_FILE, TELEMETRY_FILE,
};
use crate::manager::turn_completion::TurnCompletion;
use crate::utils::fs::ClaudeStopRoundCapture;
use crate::utils::logging::log_debug;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const TURN_VERDICTS_DIR: &str = "turn-verdicts";
/// Launch directories kept per agent, including the new one. A launch that
/// still holds an uncommitted completion is kept beyond this count.
const RETAINED_LAUNCHES: usize = 8;
const SETTLED_TIMESTAMP_SOURCE: &str = "claude_stop_hooks_settled_event";

/// Whether Claude Code supplies a supported verdict for an interactive turn.
///
/// It does not (verified against Claude Code 2.1.296).
/// `hook_execution_complete` reports only that the Stop hooks of a round
/// settled. Claude's Stop handler can still continue the turn without any hook
/// blocking it (brief-mode enforcement, goal check-ins, structured-output
/// enforcement), or end it through `continue: false`, and it emits no event
/// after that decision. Settled hooks are a necessary condition and never a
/// verdict. Until a supported provider verdict is also correlated, launches
/// capture no Stop or telemetry evidence and create no Inbox completion
/// (#1492).
fn interactive_turn_verdict() -> Result<(), &'static str> {
    Err("Claude Code exposes no supported interactive turn-end verdict")
}

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

/// Whether `dir` holds a settled completion whose card is not yet committed.
/// Such a launch is the only durable copy of that completion.
fn holds_uncommitted_completion(dir: &Path, accepted_sessions: &[String]) -> bool {
    !VerdictTracker::open(dir.to_path_buf(), accepted_sessions.to_vec(), None)
        .poll()
        .is_empty()
}

/// Removes all but the newest `keep` launch directories under `root`, except
/// those that still hold an uncommitted completion.
fn prune_launches(root: &Path, keep: usize, accepted_sessions: &[String]) {
    let Ok(canonical_root) = root.canonicalize() else {
        return;
    };
    let dirs = child_dirs(root);
    let excess = dirs.len().saturating_sub(keep);
    for dir in dirs.into_iter().take(excess) {
        if holds_uncommitted_completion(&dir, accepted_sessions) {
            continue;
        }
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

/// A prepared Claude launch: its directory and, when evidence may be
/// captured, the running receiver.
pub(crate) struct VerdictLaunch {
    dir: PathBuf,
    receiver: Option<TelemetryReceiver>,
}

impl VerdictLaunch {
    /// Creates the launch directory. Evidence capture starts only when Claude
    /// supplies a supported turn verdict and no existing telemetry
    /// configuration applies to `project_dir`.
    pub(crate) fn prepare(
        session_id: &str,
        project_dir: &Path,
        accepted_sessions: &[String],
    ) -> Result<Self, String> {
        let root = verdict_root(session_id).ok_or("Could not find Wardian home")?;
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        prune_launches(&root, RETAINED_LAUNCHES - 1, accepted_sessions);
        let dir = root.join(format!(
            "{:013}-{}",
            chrono::Utc::now().timestamp_millis().max(0),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

        let (receiver, unavailable) = match interactive_turn_verdict() {
            Err(reason) => (None, Some(reason.to_string())),
            Ok(()) => match existing_telemetry(project_dir) {
                Some(existing) => (None, Some(existing.describe())),
                None => match TelemetryReceiver::start(dir.join(TELEMETRY_FILE)) {
                    Ok(receiver) => (Some(receiver), None),
                    Err(error) => (None, Some(error)),
                },
            },
        };
        if let Some(reason) = unavailable.as_deref() {
            log_debug(&format!(
                "[Wardian] Claude Inbox completions are unavailable for {session_id}: {reason}"
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
    /// no evidence is captured.
    pub(crate) fn hook_capture(&self) -> Option<ClaudeStopRoundCapture> {
        self.receiver
            .as_ref()
            .map(|receiver| ClaudeStopRoundCapture {
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

/// Incrementally reads one launch's files and reports newly settled prompts.
pub(crate) struct VerdictTracker {
    dir: PathBuf,
    accepted_sessions: Vec<String>,
    ledger: TelemetryLedger,
    ledger_offset: u64,
    round_log_len: Option<u64>,
    round_log: StopRoundLog,
    /// Prompts already reported or committed; they are never reported again.
    resolved: HashSet<String>,
    /// The last reason logged for each unresolved prompt.
    reported_unknown: HashMap<String, UnknownReason>,
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
            round_log_len: None,
            round_log: StopRoundLog::default(),
            resolved,
            reported_unknown: HashMap::new(),
            _receiver: receiver,
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads new ledger and round records and returns prompts settled since
    /// the last poll. A prompt that cannot settle is logged once per reason
    /// and evaluated again whenever new evidence arrives, so a delayed batch
    /// can still complete it.
    pub(crate) fn poll(&mut self) -> Vec<SettledCompletion> {
        // The ledger is read first: a Stop record is on disk before Claude
        // emits the round's completion event, so a completion seen here has
        // its record in the round log read next.
        let ledger_changed = self.read_ledger();
        let rounds_changed = self.read_round_log();
        if !ledger_changed && !rounds_changed {
            return Vec::new();
        }
        let mut settled = Vec::new();
        for prompt_id in self.ledger.stop_prompt_ids() {
            if self.resolved.contains(&prompt_id) {
                continue;
            }
            match settle_prompt(
                &self.ledger,
                &self.round_log,
                &prompt_id,
                &self.accepted_sessions,
            ) {
                Verdict::Settled(completion) => {
                    self.reported_unknown.remove(&prompt_id);
                    self.resolved.insert(prompt_id);
                    settled.push(completion);
                }
                Verdict::Unknown(reason) => {
                    if self.reported_unknown.insert(prompt_id.clone(), reason) != Some(reason) {
                        log_debug(&format!(
                            "[Wardian] Claude prompt {prompt_id} has no settled final answer: {reason:?}"
                        ));
                    }
                }
                Verdict::Pending => {}
            }
        }
        settled
    }

    /// Consumes complete ledger lines only. The receiver truncates a torn
    /// tail after a failed append, so bytes after the last newline are never
    /// consumed and the offset always ends on a committed line boundary.
    fn read_ledger(&mut self) -> bool {
        let Ok(mut file) = std::fs::File::open(self.dir.join(TELEMETRY_FILE)) else {
            return false;
        };
        if file
            .metadata()
            .is_ok_and(|metadata| metadata.len() < self.ledger_offset)
        {
            // Committed lines never shrink; the stream is not trustworthy.
            self.ledger.mark_conflicted();
            return true;
        }
        let mut appended = Vec::new();
        if file
            .seek(std::io::SeekFrom::Start(self.ledger_offset))
            .and_then(|_| file.read_to_end(&mut appended))
            .is_err()
        {
            return false;
        }
        let Some(end) = appended.iter().rposition(|byte| *byte == b'\n') else {
            return false;
        };
        self.ledger_offset += end as u64 + 1;
        let Ok(complete) = std::str::from_utf8(&appended[..end]) else {
            self.ledger.mark_conflicted();
            return true;
        };
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

/// The Inbox completion for a settled answer, timestamped by Claude's own
/// settled-Stop event when it carries a parsable time. Only launches with
/// evidence capture, which requires a supported turn verdict, produce one.
pub(crate) fn inbox_completion(settled: &SettledCompletion) -> Option<TurnCompletion> {
    let (timestamp, timestamp_source) = settled
        .completed_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|time| (time.timestamp_millis(), SETTLED_TIMESTAMP_SOURCE))
        .unwrap_or_else(|| {
            (
                chrono::Utc::now().timestamp_millis(),
                "processing_time_fallback",
            )
        });
    TurnCompletion::new(
        &settled.prompt_id,
        &settled.response_text,
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
        let event =
            |sequence: u64, name: &str, hook: Option<super::super::HookExecution>| TelemetryEvent {
                sequence,
                name: name.to_string(),
                prompt_id: Some(prompt_id.to_string()),
                session_id: Some("session".to_string()),
                timestamp: Some("2026-10-10T07:00:00.000Z".to_string()),
                hook,
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
    fn tracker_reports_a_settled_prompt_once_and_honours_commits() {
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
        let settled = tracker.poll();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].response_text, "answer");
        assert!(tracker.poll().is_empty());

        let completion = inbox_completion(&settled[0]).expect("completion");
        assert_eq!(completion.timestamp_source, SETTLED_TIMESTAMP_SOURCE);
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
        prune_launches(root.path(), 2, &["session".to_string()]);
        let remaining = child_dirs(root.path())
            .into_iter()
            .filter_map(|dir| {
                dir.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        assert_eq!(remaining, ["0000000000003-launch", "0000000000004-launch"]);
        assert!(outside.path().join("keep.txt").exists());
    }

    #[test]
    fn a_prompt_start_delivered_after_its_final_round_still_settles() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), STOP_ROUNDS_FILE, &line(&round("p1")));
        let events = turn("p1");
        // The exporter delivered the Stop round's batch before the batch
        // holding the prompt's start.
        write(
            dir.path(),
            TELEMETRY_FILE,
            &(line(&events[1]) + &line(&events[2])),
        );
        let mut tracker =
            VerdictTracker::open(dir.path().to_path_buf(), vec!["session".to_string()], None);
        assert!(tracker.poll().is_empty());

        write(dir.path(), TELEMETRY_FILE, &line(&events[0]));
        let settled = tracker.poll();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].prompt_id, "p1");
        assert!(tracker.poll().is_empty());
    }

    #[test]
    fn a_torn_ledger_tail_is_not_consumed_before_its_newline() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), STOP_ROUNDS_FILE, &line(&round("p1")));
        let events = turn("p1");
        let committed = line(&events[0]) + &line(&events[1]);
        write(
            dir.path(),
            TELEMETRY_FILE,
            &(committed.clone() + "{\"sequence\":2,\"na"),
        );
        let mut tracker =
            VerdictTracker::open(dir.path().to_path_buf(), vec!["session".to_string()], None);
        assert!(tracker.poll().is_empty());

        // The receiver rolls the torn tail back and the retry appends whole lines.
        std::fs::write(
            dir.path().join(TELEMETRY_FILE),
            committed + &line(&events[2]),
        )
        .expect("rewrite ledger");
        assert_eq!(tracker.poll().len(), 1);
    }

    #[test]
    fn pruning_keeps_an_old_launch_until_its_completion_is_committed() {
        let root = tempfile::tempdir().expect("temp dir");
        let accepted = vec!["session".to_string()];
        let pending = root.path().join(format!("{:013}-launch", 0));
        std::fs::create_dir_all(&pending).expect("launch dir");
        write(&pending, STOP_ROUNDS_FILE, &line(&round("p1")));
        write(
            &pending,
            TELEMETRY_FILE,
            &turn("p1").iter().map(line).collect::<String>(),
        );
        for index in 1..10 {
            std::fs::create_dir_all(root.path().join(format!("{index:013}-launch")))
                .expect("launch dir");
        }

        prune_launches(root.path(), 7, &accepted);
        assert!(
            pending.exists(),
            "an uncommitted completion must survive pruning"
        );
        assert_eq!(child_dirs(root.path()).len(), 8);

        record_committed(&pending, "p1").expect("commit");
        prune_launches(root.path(), 7, &accepted);
        assert!(!pending.exists());
        assert_eq!(child_dirs(root.path()).len(), 7);
    }

    #[test]
    fn interactive_launches_capture_no_evidence_without_a_turn_verdict() {
        let _guard = crate::utils::wardian_test_env_lock();
        let home = tempfile::tempdir().expect("temp home");
        let previous = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
        let launch = VerdictLaunch::prepare("agent-1", home.path(), &["session".to_string()]);
        match previous {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }

        let launch = launch.expect("prepared launch");
        assert!(
            launch.hook_capture().is_none(),
            "no Stop rounds or telemetry are captured"
        );
        let record: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(launch.dir.join(LAUNCH_FILE)).expect("launch record"),
        )
        .expect("launch json");
        assert_eq!(record["verification"], "unavailable");
        assert!(record["unavailable_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("turn-end verdict")));
        assert!(!launch.dir.join(TELEMETRY_FILE).exists());
    }

    #[test]
    fn a_corrupt_ledger_line_leaves_prompts_unsettled() {
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
