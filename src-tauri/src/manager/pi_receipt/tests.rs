//! No provider processes: owned files, protocol callbacks and mock terminal actors.
use super::*;
use crate::state::terminal_session::TerminalRuntimeHandles;
use std::io::Write;

struct Fixture {
    _root: tempfile::TempDir,
    receipt: Arc<Receipt>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("receipt owned");
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.join("events.jsonl"))
            .unwrap();
        let identity = same_file::Handle::from_file(file.try_clone().unwrap()).unwrap();
        std::fs::write(directory.join("extension.mjs"), "fixture").unwrap();
        let receipt = Arc::new(Receipt {
            home: root.path().into(),
            agent_id: "agent".into(),
            native_id: "native".into(),
            nonce: "nonce".into(),
            directory,
            file: Mutex::new(file),
            identity,
            state: Mutex::new(Stream::default()),
            generation: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
            child_started: AtomicBool::new(false),
            watcher: Mutex::new(None),
        });
        Self {
            _root: root,
            receipt,
        }
    }
    fn append(&self, seq: u64, kind: &str, text: Option<&str>) {
        self.raw(&format!("{}\n", record(seq, kind, text)));
    }
    fn raw(&self, text: &str) {
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.receipt.directory.join("events.jsonl"))
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }
    fn ready(&self) {
        self.append(1, "ready", None);
        self.receipt.read_events().unwrap();
    }
}
fn record(seq: u64, kind: &str, text: Option<&str>) -> serde_json::Value {
    serde_json::json!({"v":1,"launch":"nonce","stream":"stream","seq":seq,"native_session_id":"native","kind":kind,
        "text_sha256":text.map(|s| format!("{:x}",Sha256::digest(s.as_bytes()))),"text_bytes":text.map(str::len)})
}
fn broker() -> Arc<TerminalSessionBroker> {
    Arc::new(TerminalSessionBroker::default())
}

#[test]
fn no_handshake_cannot_arm_input_and_owned_argv_is_not_shell_quoted() {
    let f = Fixture::new();
    assert!(f.receipt.ticket("prompt", broker()).is_err());
    let mut args = vec!["--no-extensions".into(), "--".into(), "positional".into()];
    f.receipt.append_args(&mut args);
    assert_eq!(args[1], "--extension");
    assert_eq!(
        args[2],
        f.receipt.directory.join("extension.mjs").to_string_lossy()
    );
    assert_eq!(args[3], "--");
}

#[test]
fn first_user_start_matches_before_any_assistant_or_session_file() {
    let f = Fixture::new();
    f.ready();
    let ticket = f.receipt.ticket("alpha\nβ", broker()).unwrap();
    f.append(2, "loop_start", None);
    f.append(3, "user_start", Some("alpha\nβ"));
    let events = f.receipt.read_events().unwrap();
    assert_eq!(events.len(), 1);
    assert!(
        !f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
    f.receipt.state.lock().unwrap().published(&events[0]);
    assert!(
        f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
    drop(ticket);
    assert!(f.receipt.state.lock().unwrap().pending.is_none());
}

#[test]
fn buffered_prewrite_user_and_transformed_native_text_do_not_match() {
    let f = Fixture::new();
    f.ready();
    f.append(2, "loop_start", None);
    f.append(3, "user_start", Some("wanted"));
    let _ticket = f.receipt.ticket("wanted", broker()).unwrap();
    f.append(4, "user_start", Some("transformed"));
    for event in f.receipt.read_events().unwrap() {
        f.receipt.state.lock().unwrap().published(&event);
    }
    assert!(
        !f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
}

#[test]
fn partial_record_started_before_arm_cannot_become_a_new_receipt() {
    let f = Fixture::new();
    f.ready();
    f.append(2, "loop_start", None);
    f.receipt.read_events().unwrap();
    let line = record(3, "user_start", Some("prompt")).to_string();
    f.raw(&line[..15]);
    assert!(f.receipt.read_events().unwrap().is_empty());
    let _ticket = f.receipt.ticket("prompt", broker()).unwrap();
    f.raw(&format!("{}\n", &line[15..]));
    for event in f.receipt.read_events().unwrap() {
        f.receipt.state.lock().unwrap().published(&event);
    }
    assert!(
        !f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
}

#[test]
fn identical_consecutive_inputs_need_distinct_new_events() {
    let f = Fixture::new();
    f.ready();
    let first = f.receipt.ticket("same", broker()).unwrap();
    f.append(2, "loop_start", None);
    f.append(3, "user_start", Some("same"));
    for event in f.receipt.read_events().unwrap() {
        f.receipt.state.lock().unwrap().published(&event);
    }
    assert!(
        f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
    drop(first);
    let _second = f.receipt.ticket("same", broker()).unwrap();
    f.append(3, "user_start", Some("same"));
    assert!(f.receipt.read_events().unwrap().is_empty());
    assert!(
        !f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
    f.append(4, "user_start", Some("same"));
    for event in f.receipt.read_events().unwrap() {
        f.receipt.state.lock().unwrap().published(&event);
    }
    assert!(
        f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
}

#[test]
fn single_pending_ticket_and_cancellation_do_not_replay_or_consume_new_claim() {
    let f = Fixture::new();
    f.ready();
    let first = f.receipt.ticket("first", broker()).unwrap();
    assert!(f.receipt.ticket("second", broker()).is_err());
    drop(first);
    let second = f.receipt.ticket("second", broker()).unwrap();
    f.append(2, "loop_start", None);
    f.append(3, "user_start", Some("first"));
    for event in f.receipt.read_events().unwrap() {
        f.receipt.state.lock().unwrap().published(&event);
    }
    assert!(
        !f.receipt
            .state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .accepted
    );
    drop(second);
}

#[test]
fn reload_foreign_identity_and_out_of_loop_records_are_rejected() {
    for invalid in [
        serde_json::json!({"stream":"reloaded"}),
        serde_json::json!({"native_session_id":"foreign"}),
        serde_json::json!({"kind":"user_start"}),
    ] {
        let f = Fixture::new();
        f.ready();
        let mut value = record(2, "loop_start", None);
        for (key, v) in invalid.as_object().unwrap() {
            value[key] = v.clone();
        }
        f.raw(&format!("{value}\n"));
        assert!(f.receipt.read_events().is_err());
    }
}

#[test]
fn replaced_truncated_and_oversized_streams_fail_closed() {
    let f = Fixture::new();
    f.ready();
    let path = f.receipt.directory.join("events.jsonl");
    std::fs::rename(&path, f.receipt.directory.join("old.jsonl")).unwrap();
    std::fs::write(&path, "").unwrap();
    assert!(f.receipt.read_events().is_err());
    let f = Fixture::new();
    f.ready();
    OpenOptions::new()
        .write(true)
        .open(f.receipt.directory.join("events.jsonl"))
        .unwrap()
        .set_len(0)
        .unwrap();
    assert!(f.receipt.read_events().is_err());
    let f = Fixture::new();
    f.raw(&"x".repeat(MAX_RECORD_BYTES + 1));
    assert!(f.receipt.read_events().is_err());
}

#[tokio::test]
async fn replacement_generation_rejects_input_and_receipt_without_touching_new_runtime() {
    let f = Fixture::new();
    f.ready();
    let broker = broker();
    let (tx, mut old_rx) = tokio::sync::mpsc::channel(8);
    let geometry = wardian_core::models::TerminalGeometry { cols: 80, rows: 24 };
    let generation = broker
        .start_or_replace_runtime(
            "agent",
            TerminalRuntimeHandles::new(tx, |_| Ok(())),
            geometry,
        )
        .await
        .unwrap();
    f.receipt.generation.store(generation, Ordering::Release);
    let ticket = f.receipt.ticket("prompt", broker.clone()).unwrap();
    let (tx, mut new_rx) = tokio::sync::mpsc::channel(8);
    let replacement_generation = broker
        .start_or_replace_runtime(
            "agent",
            TerminalRuntimeHandles::new(tx, |_| Ok(())),
            geometry,
        )
        .await
        .unwrap();
    assert!(ticket.send_bytes(b"prompt".to_vec()).await.is_err());
    let watch = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
        "agent".into(),
        32,
        4096,
    )));
    assert!(broker
        .record_turn_started_for_generation("agent", generation, watch.clone())
        .await
        .is_err());
    assert!(watch
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap()
        .events
        .is_empty());
    assert!(old_rx.try_recv().is_err());
    assert!(new_rx.try_recv().is_err());
    broker
        .terminate_and_remove_runtime("agent", replacement_generation)
        .await
        .unwrap();
}

#[derive(Debug)]
struct FakeChild {
    exited: bool,
}
impl portable_pty::ChildKiller for FakeChild {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(Self {
            exited: self.exited,
        })
    }
}
impl portable_pty::Child for FakeChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(self
            .exited
            .then(|| portable_pty::ExitStatus::with_exit_code(0)))
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        panic!("cleanup must poll with a bound")
    }
    fn process_id(&self) -> Option<u32> {
        None
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

#[test]
fn cleanup_joins_watcher_after_exit_and_preserves_uncertain_or_newer_launch() {
    let old = Fixture::new();
    let newer = Fixture::new();
    let joined = Arc::new(AtomicBool::new(false));
    let signal = joined.clone();
    old.receipt.retain_watcher(std::thread::spawn(move || {
        signal.store(true, Ordering::Release);
    }));
    child::finish(
        Box::new(FakeChild { exited: true }),
        old.receipt.clone(),
        Duration::ZERO,
    );
    assert!(joined.load(Ordering::Acquire));
    assert!(!old.receipt.directory.exists());
    assert!(newer.receipt.directory.exists());
    child::finish(
        Box::new(FakeChild { exited: false }),
        newer.receipt.clone(),
        Duration::ZERO,
    );
    assert!(newer.receipt.directory.join("events.jsonl").exists());
}

#[tokio::test]
async fn timeout_and_stopped_stream_leave_submission_uncertain_without_accepting_late_input() {
    let f = Fixture::new();
    f.ready();
    let ticket = f.receipt.ticket("prompt", broker()).unwrap();
    assert!(ticket.wait_for(Duration::ZERO).await.is_err());
    drop(ticket);
    f.append(2, "loop_start", None);
    f.append(3, "user_start", Some("prompt"));
    for event in f.receipt.read_events().unwrap() {
        f.receipt.state.lock().unwrap().published(&event);
    }
    assert!(f.receipt.state.lock().unwrap().pending.is_none());
    f.receipt.stop();
    assert!(f.receipt.ticket("prompt", broker()).is_err());
}

#[test]
fn delayed_native_log_retains_completion_but_cannot_double_count_hook_start() {
    use wardian_core::models::provider::AgentProvider;
    let f = Fixture::new();
    f.ready();
    f.append(2, "loop_start", None);
    f.append(3, "user_start", Some("prompt"));
    let hook_starts = f.receipt.read_events().unwrap().len();
    let provider = crate::providers::pi::PiProvider::new();
    let user=provider.parse_output(r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#).unwrap();
    assert!(log_activity(user).is_none());
    let assistant = provider
        .parse_output(r#"{"type":"message","message":{"role":"assistant","stopReason":"stop"}}"#)
        .unwrap();
    assert!(log_activity(assistant).is_some());
    assert_eq!(hook_starts, 1);
}
