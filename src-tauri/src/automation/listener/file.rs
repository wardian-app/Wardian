//! File-watch listener arming: a `notify` watcher plus a debounce accumulator.
//!
//! Editors and build tools emit bursts, not single events. Firing per event
//! would launch dozens of agent runs for one save, so events accumulate into a
//! window and one quiet interval closes it.

use super::launch::{self, ListenerFire};
use notify::Watcher as _;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::Duration;
use tauri::AppHandle;
use wardian_core::listeners::{
    file as file_rules, AutomationListener, FileChangeKind, FileWatchTrigger,
};

/// Upper bound on paths reported in one payload.
///
/// A burst can touch thousands of files; the automation needs to know what
/// changed, not to receive a filesystem dump in an agent prompt.
const MAX_REPORTED_PATHS: usize = 200;

/// One accumulated change inside the debounce window.
#[derive(Debug)]
enum WatchSignal {
    Changed(String),
    /// The platform lost track of individual paths. Reported as a real fire
    /// with unknown paths, because dropping it would lose changes under
    /// exactly the load where the listener matters most.
    Rescan,
}

/// A live watcher plus its debounce thread. Dropping this stops both: the
/// closure holding the channel sender goes with the watcher, and the thread's
/// `recv_timeout` then reports a disconnected channel and exits.
pub struct FileWatch {
    _watcher: notify::RecommendedWatcher,
}

fn change_kind(kind: notify::EventKind) -> Option<FileChangeKind> {
    match kind {
        notify::EventKind::Create(_) => Some(FileChangeKind::Created),
        notify::EventKind::Modify(_) => Some(FileChangeKind::Modified),
        notify::EventKind::Remove(_) => Some(FileChangeKind::Removed),
        // Reading a watched file must never start an automation.
        notify::EventKind::Access(_) => None,
        notify::EventKind::Any | notify::EventKind::Other => Some(FileChangeKind::Modified),
    }
}

/// Stable identity for one debounced burst.
///
/// Built from the window and the paths rather than a counter, so replaying the
/// same burst resolves to the same run while two genuinely separate bursts over
/// the same files stay distinct events.
fn burst_identity(window_end_ms: u64, paths: &BTreeSet<String>, rescan: bool) -> String {
    let mut hasher = Sha256::new();
    hasher.update(window_end_ms.to_be_bytes());
    hasher.update([u8::from(rescan)]);
    for path in paths {
        hasher.update(path.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn payload(
    listener: &AutomationListener,
    root: &std::path::Path,
    paths: &BTreeSet<String>,
    rescan: bool,
) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("listener_id".into(), Value::String(listener.id.clone()));
    map.insert("listener_name".into(), Value::String(listener.name.clone()));
    map.insert("trigger_type".into(), Value::String("file_watch".into()));
    map.insert(
        "path".into(),
        Value::String(root.to_string_lossy().into_owned()),
    );
    map.insert(
        "paths".into(),
        Value::Array(
            paths
                .iter()
                .take(MAX_REPORTED_PATHS)
                .map(|path| Value::String(path.clone()))
                .collect(),
        ),
    );
    map.insert("path_count".into(), Value::from(paths.len()));
    map.insert(
        "truncated".into(),
        Value::Bool(paths.len() > MAX_REPORTED_PATHS),
    );
    map.insert("rescan".into(), Value::Bool(rescan));
    map.insert(
        "observed_at".into(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    map
}

/// Start watching for one listener.
///
/// The watch root is revalidated here rather than trusted from config: a path
/// that was safe when the listener was written may since have been deleted,
/// replaced by a symlink, or moved inside the Wardian home.
/// Map one `notify` event onto the signals it contributes.
///
/// Split out of the watcher closure so the filtering rules can be exercised
/// against synthetic events instead of racing a real filesystem.
fn signals_for(
    trigger: &FileWatchTrigger,
    root: &std::path::Path,
    event: &notify::Event,
) -> Vec<WatchSignal> {
    if event.need_rescan() {
        return vec![WatchSignal::Rescan];
    }
    let Some(kind) = change_kind(event.kind) else {
        return Vec::new();
    };
    event
        .paths
        .iter()
        .filter(|path| file_rules::matches(trigger, path, kind))
        .map(|path| {
            let relative = if path == root {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                file_rules::relative_glob_path(root, path)
                    .unwrap_or_else(|| path.to_string_lossy().into_owned())
            };
            WatchSignal::Changed(relative)
        })
        .collect()
}

/// Accumulate signals until one full quiet window passes, then flush.
///
/// Runs until the channel disconnects, which happens when the watcher is
/// dropped during a disarm. Taking `flush` as a callback keeps the timing
/// behavior testable without a Tauri app or a real watcher.
fn debounce_loop(
    rx: mpsc::Receiver<WatchSignal>,
    debounce: Duration,
    mut flush: impl FnMut(BTreeSet<String>, bool),
) {
    let mut pending: BTreeSet<String> = BTreeSet::new();
    let mut rescan = false;
    loop {
        match rx.recv_timeout(debounce) {
            Ok(WatchSignal::Changed(path)) => {
                pending.insert(path);
            }
            Ok(WatchSignal::Rescan) => rescan = true,
            // Quiet for a full window: the burst is over.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending.is_empty() && !rescan {
                    continue;
                }
                let paths = std::mem::take(&mut pending);
                let was_rescan = std::mem::replace(&mut rescan, false);
                flush(paths, was_rescan);
            }
            // The watcher was dropped; the listener is being disarmed.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Start watching for one listener.
///
/// The watch root is revalidated here rather than trusted from config: a path
/// that was safe when the listener was written may since have been deleted,
/// replaced by a symlink, or moved inside the Wardian home.
pub fn arm(
    app: AppHandle,
    listener: &AutomationListener,
    trigger: &FileWatchTrigger,
) -> Result<FileWatch, String> {
    let root = file_rules::validate_watch_root(&trigger.path)?;
    let (tx, rx) = mpsc::channel::<WatchSignal>();

    let matcher = trigger.clone();
    let match_root = root.clone();
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) => {
                for signal in signals_for(&matcher, &match_root, &event) {
                    let _ = tx.send(signal);
                }
            }
            Err(error) => {
                crate::utils::logging::log_debug(&format!(
                    "[automation] file watcher error: {error}"
                ));
            }
        })
        .map_err(|error| format!("could not create file watcher: {error}"))?;

    let mode = if trigger.recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    watcher
        .watch(&root, mode)
        .map_err(|error| format!("could not watch {}: {error}", root.display()))?;

    let debounce = Duration::from_millis(u64::from(trigger.debounce_ms.max(1)));
    let listener = listener.clone();
    let watch_root = root.clone();
    std::thread::spawn(move || {
        debounce_loop(rx, debounce, |paths, rescan| {
            let window_end_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
            let fire = ListenerFire {
                event_identity: burst_identity(window_end_ms, &paths, rescan),
                payload: payload(&listener, &watch_root, &paths, rescan),
            };
            let app = app.clone();
            let listener = listener.clone();
            tauri::async_runtime::spawn(async move {
                launch::fire(app, listener, fire).await;
            });
        });
    });

    Ok(FileWatch { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn watch_root() -> std::path::PathBuf {
        // Built rather than written as a literal so the same test reads the
        // same way on both platforms.
        if cfg!(windows) {
            std::path::PathBuf::from("C:/").join("work").join("repo")
        } else {
            std::path::PathBuf::from("/work/repo")
        }
    }

    fn watch_trigger(root: &std::path::Path, patterns: &[&str]) -> FileWatchTrigger {
        FileWatchTrigger {
            path: root.to_string_lossy().into_owned(),
            recursive: true,
            patterns: patterns.iter().map(|value| (*value).to_string()).collect(),
            ignore: Vec::new(),
            events: Vec::new(),
            debounce_ms: 50,
        }
    }

    fn modify_event(paths: Vec<std::path::PathBuf>) -> notify::Event {
        notify::Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths,
            attrs: Default::default(),
        }
    }

    #[test]
    fn an_event_contributes_only_its_matching_paths_relative_to_the_root() {
        let root = watch_root();
        let root = root.as_path();
        let trigger = watch_trigger(root, &["**/*.rs"]);
        let event = modify_event(vec![
            root.join("src").join("main.rs"),
            root.join("README.md"),
            root.join("node_modules").join("pkg").join("index.rs"),
        ]);

        let signals = signals_for(&trigger, root, &event);
        let paths: Vec<String> = signals
            .iter()
            .map(|signal| match signal {
                WatchSignal::Changed(path) => path.clone(),
                WatchSignal::Rescan => "<rescan>".into(),
            })
            .collect();
        assert_eq!(
            paths,
            vec!["src/main.rs".to_string()],
            "only the pattern-matching, non-ignored path should contribute"
        );
    }

    #[test]
    fn a_rescan_event_replaces_its_paths_with_a_rescan_signal() {
        let root = watch_root();
        let root = root.as_path();
        let mut event = modify_event(vec![root.join("a.rs")]);
        event.attrs.set_flag(notify::event::Flag::Rescan);

        let signals = signals_for(&watch_trigger(root, &[]), root, &event);
        assert!(matches!(signals.as_slice(), [WatchSignal::Rescan]));
    }

    #[test]
    fn a_burst_of_events_collapses_into_one_flush() {
        let (tx, rx) = mpsc::channel();
        let flushes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&flushes);

        let worker = std::thread::spawn(move || {
            debounce_loop(rx, Duration::from_millis(80), move |paths, rescan| {
                recorder
                    .lock()
                    .expect("flush recorder")
                    .push((paths, rescan));
            });
        });

        for name in ["a.rs", "b.rs", "c.rs", "a.rs"] {
            tx.send(WatchSignal::Changed(name.to_string()))
                .expect("send");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(250));
        drop(tx);
        worker.join().expect("debounce thread ends when disarmed");

        let recorded = flushes.lock().expect("flushes");
        assert_eq!(
            recorded.len(),
            1,
            "one editor save must not become four runs"
        );
        assert_eq!(recorded[0].0.len(), 3, "repeated paths are deduplicated");
        assert!(!recorded[0].1);
    }

    #[test]
    fn two_bursts_separated_by_quiet_flush_separately() {
        let (tx, rx) = mpsc::channel();
        let flushes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&flushes);

        let worker = std::thread::spawn(move || {
            debounce_loop(rx, Duration::from_millis(60), move |paths, rescan| {
                recorder
                    .lock()
                    .expect("flush recorder")
                    .push((paths, rescan));
            });
        });

        tx.send(WatchSignal::Changed("first.rs".into()))
            .expect("send");
        std::thread::sleep(Duration::from_millis(220));
        tx.send(WatchSignal::Changed("second.rs".into()))
            .expect("send");
        std::thread::sleep(Duration::from_millis(220));
        drop(tx);
        worker.join().expect("debounce thread ends when disarmed");

        let recorded = flushes.lock().expect("flushes");
        assert_eq!(recorded.len(), 2);
        assert!(recorded[0].0.contains("first.rs"));
        assert!(recorded[1].0.contains("second.rs"));
    }

    #[test]
    fn quiet_alone_never_flushes() {
        let (tx, rx) = mpsc::channel::<WatchSignal>();
        let flushed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&flushed);

        let worker = std::thread::spawn(move || {
            debounce_loop(rx, Duration::from_millis(20), move |_, _| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            });
        });
        std::thread::sleep(Duration::from_millis(150));
        drop(tx);
        worker.join().expect("debounce thread ends when disarmed");

        assert_eq!(flushed.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn reading_a_watched_file_never_fires() {
        assert!(change_kind(notify::EventKind::Access(notify::event::AccessKind::Read)).is_none());
    }

    #[test]
    fn create_modify_and_remove_map_to_their_change_kinds() {
        assert_eq!(
            change_kind(notify::EventKind::Create(notify::event::CreateKind::File)),
            Some(FileChangeKind::Created)
        );
        assert_eq!(
            change_kind(notify::EventKind::Remove(notify::event::RemoveKind::File)),
            Some(FileChangeKind::Removed)
        );
        assert_eq!(
            change_kind(notify::EventKind::Any),
            Some(FileChangeKind::Modified)
        );
    }

    #[test]
    fn the_same_burst_has_one_identity_and_different_bursts_do_not() {
        let first = burst_identity(1_000, &paths(&["a.rs", "b.rs"]), false);
        assert_eq!(
            first,
            burst_identity(1_000, &paths(&["b.rs", "a.rs"]), false),
            "path order must not change the identity"
        );
        assert_ne!(
            first,
            burst_identity(2_000, &paths(&["a.rs", "b.rs"]), false)
        );
        assert_ne!(first, burst_identity(1_000, &paths(&["a.rs"]), false));
        assert_ne!(
            first,
            burst_identity(1_000, &paths(&["a.rs", "b.rs"]), true),
            "a rescan is a different event from a known-path burst"
        );
    }

    #[test]
    fn a_large_burst_reports_bounded_paths_and_says_it_truncated() {
        let listener = wardian_core::listeners::AutomationListener {
            id: "l".into(),
            blueprint_id: "audit".into(),
            name: "Audit".into(),
            enabled: true,
            trigger: wardian_core::listeners::ListenerTrigger::FileWatch(FileWatchTrigger {
                path: "/watched".into(),
                recursive: true,
                patterns: Vec::new(),
                ignore: Vec::new(),
                events: Vec::new(),
                debounce_ms: 500,
            }),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: Default::default(),
            assignments: Default::default(),
            overlap: None,
            runtime: Default::default(),
        };
        let many: BTreeSet<String> = (0..500).map(|index| format!("file-{index}.rs")).collect();
        let map = payload(&listener, std::path::Path::new("/watched"), &many, false);

        assert_eq!(
            map["paths"].as_array().expect("paths").len(),
            MAX_REPORTED_PATHS
        );
        assert_eq!(map["path_count"], serde_json::json!(500));
        assert_eq!(map["truncated"], serde_json::json!(true));
        assert_eq!(map["rescan"], serde_json::json!(false));
    }

    #[test]
    fn a_rescan_payload_admits_it_does_not_know_the_paths() {
        let listener = wardian_core::listeners::AutomationListener {
            id: "l".into(),
            blueprint_id: "audit".into(),
            name: "Audit".into(),
            enabled: true,
            trigger: wardian_core::listeners::ListenerTrigger::FileWatch(FileWatchTrigger {
                path: "/watched".into(),
                recursive: true,
                patterns: Vec::new(),
                ignore: Vec::new(),
                events: Vec::new(),
                debounce_ms: 500,
            }),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: Default::default(),
            assignments: Default::default(),
            overlap: None,
            runtime: Default::default(),
        };
        let map = payload(
            &listener,
            std::path::Path::new("/watched"),
            &BTreeSet::new(),
            true,
        );
        assert_eq!(map["rescan"], serde_json::json!(true));
        assert_eq!(map["paths"], serde_json::json!([]));
        assert_eq!(map["path_count"], serde_json::json!(0));
    }
}
