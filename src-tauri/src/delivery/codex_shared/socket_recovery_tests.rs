use super::*;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

#[test]
fn socket_recovery_removes_a_refused_socket_and_allows_rebind() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    recover_stale_socket(&socket).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    drop(listener);
    recover_stale_socket(&socket).unwrap();
    assert!(!socket.exists());
    let _replacement = UnixListener::bind(&socket).unwrap();
}

#[test]
fn socket_recovery_preserves_a_live_listener() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let error = recover_stale_socket(&socket).unwrap_err();
    // Windows may report a pending nonblocking connect even with queue space.
    assert!(
        error.message.contains("live listener")
            || error
                .message
                .contains("whether the existing Codex socket is stale")
    );
    assert!(UnixStream::connect(&socket).is_ok());
}

#[test]
fn socket_recovery_preserves_a_saturated_listener_without_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sock");
    let address = socket2::SockAddr::unix(&path).unwrap();
    let listener =
        socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
    listener.bind(&address).unwrap();
    listener.listen(1).unwrap();
    let mut clients = Vec::new();
    for _ in 0..32 {
        let client =
            socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
        client.set_nonblocking(true).unwrap();
        let _ = client.connect(&address);
        clients.push(client);
    }
    let started = std::time::Instant::now();
    assert!(recover_stale_socket(&path).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(path.exists());
}

#[test]
fn socket_recovery_preserves_a_competing_startups_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    drop(listener);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.path().join("app-server-startup.lock"))
        .unwrap();
    lock.lock().unwrap();
    let error = recover_stale_socket(&socket).unwrap_err();
    assert!(error.message.contains("startup is already in progress"));
    assert!(socket.exists());
}

#[test]
fn socket_recovery_preserves_regular_files_and_replacement_identities() {
    let directory = tempfile::tempdir().unwrap();
    let ordinary = directory.path().join("ordinary");
    std::fs::write(&ordinary, "retain me").unwrap();
    assert!(recover_stale_socket(&ordinary).is_err());
    assert_eq!(std::fs::read_to_string(&ordinary).unwrap(), "retain me");

    let socket = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let previous = OwnedSocket::capture(&socket).unwrap();
    drop(listener);
    std::fs::rename(&socket, directory.path().join("previous.sock")).unwrap();
    let _replacement = UnixListener::bind(&socket).unwrap();
    assert!(previous.remove_after_exit().is_err());
    assert!(UnixStream::connect(&socket).is_ok());
}
