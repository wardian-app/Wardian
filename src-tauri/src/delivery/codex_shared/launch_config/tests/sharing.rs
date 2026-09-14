//! Actual Windows sharing contention at the checked publication boundary.
use super::*;
use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

fn reader_without_delete_sharing(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(3) // FILE_SHARE_READ | FILE_SHARE_WRITE, deliberately no DELETE.
        .open(path)
        .unwrap()
}

#[test]
fn launch_publication_survives_short_windows_reader() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    fs::write(&path, "model = 'old'\n").unwrap();
    let before = storage::read_snapshot(&path).unwrap();
    let held = reader_without_delete_sharing(&path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(75));
        drop(held);
    });
    let result = storage::publish(&path, &before, b"model = 'new'\n", None);
    release.join().unwrap();
    result.unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "model = 'new'\n");
}

#[test]
fn launch_publication_stays_bounded_when_windows_reader_remains() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    fs::write(&path, "model = 'old'\n").unwrap();
    let before = storage::read_snapshot(&path).unwrap();
    let held = reader_without_delete_sharing(&path);
    let started = Instant::now();
    let error = storage::publish(&path, &before, b"model = 'new'\n", None).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(error.to_string().contains("config.toml"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "model = 'old'\n");
    drop(held);
    storage::publish(&path, &before, b"model = 'new'\n", None).unwrap();
}

#[test]
fn launch_publication_rechecks_peer_after_windows_contention() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let peer = home.path().join(journal::FILE);
    fs::write(&path, "model = 'old'\n").unwrap();
    fs::write(&peer, "original intent").unwrap();
    let before = storage::read_snapshot(&path).unwrap();
    let peer_before = storage::read_snapshot(&peer).unwrap();
    let held = reader_without_delete_sharing(&path);
    let changed_peer = peer.clone();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(75));
        fs::write(changed_peer, "external intent").unwrap();
        drop(held);
    });
    let result = storage::publish(
        &path,
        &before,
        b"model = 'new'\n",
        Some((&peer, &peer_before)),
    );
    release.join().unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("changed during preparation"));
    assert_eq!(fs::read_to_string(path).unwrap(), "model = 'old'\n");
    assert_eq!(fs::read_to_string(peer).unwrap(), "external intent");
}

#[test]
fn launch_publication_preserves_destination_edit_during_windows_contention() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    fs::write(&path, "model = 'old'\n").unwrap();
    let before = storage::read_snapshot(&path).unwrap();
    let held = reader_without_delete_sharing(&path);
    let edited = path.clone();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(75));
        fs::write(edited, "model = 'external'\n").unwrap();
        drop(held);
    });
    let result = storage::publish(&path, &before, b"model = 'new'\n", None);
    release.join().unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("changed during preparation"));
    assert_eq!(fs::read_to_string(path).unwrap(), "model = 'external'\n");
}

#[test]
fn launch_restore_survives_short_windows_journal_reader() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), "model = 'old'\n").unwrap();
    let mut guard = prepare_launch_config(home.path(), &args(&["model='new'"])).unwrap();
    let journal_path = home.path().join(journal::FILE);
    let held = reader_without_delete_sharing(&journal_path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(75));
        drop(held);
    });
    let result = guard.restore();
    release.join().unwrap();
    result.unwrap();
    assert_eq!(read(home.path())["model"].as_str(), Some("old"));
    assert!(!journal_path.exists());
}

#[test]
fn launch_restore_retains_journal_when_windows_reader_remains() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), "model = 'old'\n").unwrap();
    let mut guard = prepare_launch_config(home.path(), &args(&["model='new'"])).unwrap();
    let journal_path = home.path().join(journal::FILE);
    let held = reader_without_delete_sharing(&journal_path);
    let started = Instant::now();
    let error = guard.restore().unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(error.to_string().contains("removing launch journal"));
    assert!(journal_path.exists());
    assert_eq!(read(home.path())["model"].as_str(), Some("old"));
    drop(held);
    guard.restore().unwrap();
    assert!(!journal_path.exists());
}
