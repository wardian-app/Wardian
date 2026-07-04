use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::Watcher as _;

/// Keeps the watcher alive for the app's lifetime via `app.manage`.
/// Mutex makes the non-Sync notify watcher satisfy Tauri's Sync bound.
#[allow(dead_code)]
pub struct TopologyWatcherHandle(pub std::sync::Mutex<notify::RecommendedWatcher>);

const DEBOUNCE: Duration = Duration::from_millis(150);

/// Watch `<home>/topology.json` for external changes (CLI writes, hand edits)
/// and invoke `emit` once per debounced burst.
///
/// Watches the home DIRECTORY (non-recursive), not the file: the file may not
/// exist yet, and save_topology replaces it by atomic rename, which can drop a
/// direct file watch on some platforms. Events are filtered to the exact
/// `topology.json` file name (the `.json.tmp` sibling does not match).
///
/// The returned watcher must be kept alive; dropping it stops watching and
/// ends the debounce thread.
pub fn spawn_topology_watcher(
    home: PathBuf,
    emit: impl Fn() + Send + 'static,
) -> notify::Result<notify::RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let is_topology = event
                .paths
                .iter()
                .any(|path| path.file_name().is_some_and(|name| name == "topology.json"));
            if is_topology {
                let _ = tx.send(());
            }
        }
    })?;
    watcher.watch(&home, notify::RecursiveMode::NonRecursive)?;

    std::thread::spawn(move || {
        // Debounce: first event opens a window; further events within DEBOUNCE
        // extend it; emit fires once when the burst goes quiet.
        while rx.recv().is_ok() {
            loop {
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            emit();
        }
    });

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn emits_once_per_burst_when_topology_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (emitted_tx, emitted_rx) = mpsc::channel::<()>();
        let _watcher = spawn_topology_watcher(dir.path().to_path_buf(), move || {
            let _ = emitted_tx.send(());
        })
        .unwrap();

        // Burst of writes: temp file + rename, like save_topology.
        std::fs::write(dir.path().join("topology.json.tmp"), b"{}").unwrap();
        std::fs::rename(
            dir.path().join("topology.json.tmp"),
            dir.path().join("topology.json"),
        )
        .unwrap();

        // One debounced emit arrives (generous timeout: notify latency varies by platform).
        emitted_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected a topology-changed emit");
    }

    #[test]
    fn ignores_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let (emitted_tx, emitted_rx) = mpsc::channel::<()>();
        let _watcher = spawn_topology_watcher(dir.path().to_path_buf(), move || {
            let _ = emitted_tx.send(());
        })
        .unwrap();

        std::fs::write(dir.path().join("other.json"), b"{}").unwrap();

        assert!(
            emitted_rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "unrelated file must not emit"
        );
    }
}
