//! Unmanaged caller crosses the real CLI transport; fixtures never use the desktop home.
use serde_json::{json, Value};
use std::{path::Path, process::Command, sync::mpsc, thread, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

async fn exchange(mut stream: impl AsyncRead + AsyncWrite + Unpin, response: String) -> Value {
    let mut line = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut line)
        .await
        .unwrap();
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    stream.flush().await.unwrap();
    let mut closed = [0];
    let _ = stream.read(&mut closed).await;
    serde_json::from_str(&line).unwrap()
}
fn endpoint(home: &Path, response: Value) -> thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(15), async {
                #[cfg(windows)]
                {
                    let hash = home
                        .to_string_lossy()
                        .as_bytes()
                        .iter()
                        .fold(0xcbf29ce484222325u64, |hash, byte| {
                            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                        });
                    let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                        .first_pipe_instance(true)
                        .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                        .unwrap();
                    tx.send(()).unwrap();
                    pipe.connect().await.unwrap();
                    exchange(pipe, response.to_string()).await
                }
                #[cfg(unix)]
                {
                    std::fs::create_dir_all(home.join("run")).unwrap();
                    let listener =
                        tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                    tx.send(()).unwrap();
                    let (stream, _) = listener.accept().await.unwrap();
                    exchange(stream, response.to_string()).await
                }
            })
            .await
            .expect("preparation CLI must finish the isolated exchange")
        })
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    handle
}
#[test]
fn prepare_requires_no_sender_and_preserves_response_and_backend_rejection() {
    let home = tempfile::tempdir().unwrap();
    let expected = json!({"schema":1,"ok":true,"source_folder":"/repo",
        "worktree_folder":"/repo.wt/review","branch_name":"wardian/review",
        "commit":"abc123","cleared_session":false});
    let server = endpoint(home.path(), expected.clone());
    let output = Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .args([
            "agent", "worktree", "prepare", "--source", "/repo", "--name", "review", "--ref",
            "HEAD~1",
        ])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected
    );
    let request = server.join().unwrap();
    assert_eq!(
        request,
        json!({"command":"agent_worktree_prepare","source":"/repo","name":"review","reference":"HEAD~1"})
    );
    // Each exchange owns its transport namespace; Unix socket files outlive listeners.
    let home = tempfile::tempdir().unwrap();
    let server = endpoint(
        home.path(),
        json!({"schema":1,"error":{"code":"request_failed","message":"occupied target"}}),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .args([
            "agent", "worktree", "prepare", "--source", "/repo", "--name", "review", "--ref",
            "HEAD",
        ])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "request_failed");
    assert_eq!(error["error"]["message"], "occupied target");
    server.join().unwrap();
}
