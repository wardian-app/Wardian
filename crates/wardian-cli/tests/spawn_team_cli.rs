//! Team spawn receipt through a real CLI process and isolated local control endpoint.
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command, sync::mpsc, thread, time::Duration};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
async fn serve_stream(mut stream: impl AsyncRead + AsyncWrite + Unpin, response: &str) -> Value {
    let mut request = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut request)
        .await
        .unwrap();
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    stream.flush().await.unwrap();
    // Named-pipe buffers must remain alive until the client consumes the reply.
    let mut closed = [0_u8; 1];
    assert_eq!(stream.read(&mut closed).await.unwrap(), 0);
    serde_json::from_str(&request).unwrap()
}

fn endpoint(home: &Path, response: String) -> thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(15), async {
                    #[cfg(windows)]
                    {
                        let hash = home
                            .to_string_lossy()
                            .as_bytes()
                            .iter()
                            .fold(0xcbf29ce484222325_u64, |hash, byte| {
                                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                            });
                        let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                            .first_pipe_instance(true)
                            .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                            .unwrap();
                        ready_tx.send(()).unwrap();
                        pipe.connect().await.unwrap();
                        serve_stream(pipe, &response).await
                    }
                    #[cfg(unix)]
                    {
                        fs::create_dir_all(home.join("run")).unwrap();
                        let listener =
                            tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                        ready_tx.send(()).unwrap();
                        let (stream, _) = listener.accept().await.unwrap();
                        serve_stream(stream, &response).await
                    }
                })
                .await
                .expect("one bounded CLI request must finish")
            })
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    server
}

#[test]
fn explicit_team_spawn_receipt_matches_durable_membership() {
    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join("watchlists")).unwrap();
    fs::write(home.path().join("watchlists/index.json"),json!({"version":2,"teams":[{"id":"dev","name":"Dev Team","agentIds":["existing"]}],"watchlists":[{"id":"watch","name":"Dev","entries":[{"type":"team","teamId":"dev"}]}]}).to_string()).unwrap();
    let server=endpoint(home.path(),json!({"schema":1,"agent":{
        "name":"created","uuid":"created-id","class":"Coder","provider":"mock","status":"idle",
        "workspace":null,"pid":null,"started_at":null,"last_status_at":null,"status_source":"live"
    }}).to_string());
    let output = Command::new(env!("CARGO_BIN_EXE_wardian-cli"))
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .args([
            "agent",
            "spawn",
            "--provider",
            "mock",
            "--class",
            "Coder",
            "--team",
            "Dev Team",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(server.join().unwrap()["command"], "agent_spawn");
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["agent"]["uuid"], "created-id");
    assert_eq!(receipt["team"]["id"], "dev");
    assert_eq!(
        receipt["team"]["agent_ids"],
        json!(["existing", "created-id"])
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(home.path().join("watchlists/index.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["teams"][0]["agentIds"],
        json!(["existing", "created-id"])
    );
    assert_eq!(saved["watchlists"][0]["entries"][0]["teamId"], "dev");
}
