//! Verify normal lifecycle CLI requests against the bounded synthetic endpoint.
use std::{fs, path::Path, process::Command, sync::mpsc, thread, time::Duration};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-cli"));
    command
        .args(["agent", "new-session", "paused-agent"])
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID");
    command
}

async fn serve_stream(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    response: Option<&Value>,
    receipt_home: Option<&Path>,
) -> Value {
    let mut request = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut request)
        .await
        .unwrap();
    let request: Value = serde_json::from_str(&request).unwrap();
    if let Some(home) = receipt_home {
        if request["command"] == "agent_spawn_async" {
            let operation_id = request["operation_id"].as_str().unwrap();
            let receipt: Value = serde_json::from_slice(
                &fs::read(
                    home.join("spawn-submissions")
                        .join(format!("{operation_id}.json")),
                )
                .expect("generated key must be durable before admission"),
            )
            .unwrap();
            assert_eq!(receipt["operation_id"], operation_id);
        }
    }
    let Some(response) = response else {
        return request;
    };
    let mut response = response.clone();
    if let Some(operation) = response.get_mut("operation") {
        operation["operation_id"] = request["operation_id"].clone();
    }
    stream
        .write_all(response.to_string().as_bytes())
        .await
        .unwrap();
    stream.write_all(b"\n").await.unwrap();
    stream.flush().await.unwrap();
    // Named-pipe buffers must remain alive until the client consumes the reply.
    let mut closed = [0_u8; 1];
    assert_eq!(stream.read(&mut closed).await.unwrap(), 0);
    request
}

fn endpoint(home: &Path, response: String) -> thread::JoinHandle<Vec<Value>> {
    endpoint_sequence(
        home,
        vec![Some(serde_json::from_str(&response).unwrap())],
        false,
    )
}

fn endpoint_sequence(
    home: &Path,
    responses: Vec<Option<Value>>,
    require_receipt: bool,
) -> thread::JoinHandle<Vec<Value>> {
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
                        let mut pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                            .first_pipe_instance(true)
                            .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                            .unwrap();
                        ready_tx.send(()).unwrap();
                        let mut requests = Vec::new();
                        for response in &responses {
                            pipe.connect().await.unwrap();
                            let next = tokio::net::windows::named_pipe::ServerOptions::new()
                                .create(format!(r"\\.\pipe\wardian-control-{hash:016x}"))
                                .unwrap();
                            requests.push(
                                serve_stream(
                                    pipe,
                                    response.as_ref(),
                                    require_receipt.then_some(home.as_path()),
                                )
                                .await,
                            );
                            pipe = next;
                        }
                        assert!(
                            tokio::time::timeout(Duration::from_millis(250), pipe.connect())
                                .await
                                .is_err(),
                            "CLI replayed a completed or uncertain submission"
                        );
                        requests
                    }
                    #[cfg(unix)]
                    {
                        fs::create_dir_all(home.join("run")).unwrap();
                        let listener =
                            tokio::net::UnixListener::bind(home.join("run/control.sock")).unwrap();
                        ready_tx.send(()).unwrap();
                        let mut requests = Vec::new();
                        for response in &responses {
                            let (stream, _) = listener.accept().await.unwrap();
                            requests.push(
                                serve_stream(
                                    stream,
                                    response.as_ref(),
                                    require_receipt.then_some(home.as_path()),
                                )
                                .await,
                            );
                        }
                        assert!(
                            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                                .await
                                .is_err(),
                            "CLI replayed a completed or uncertain submission"
                        );
                        requests
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
fn new_session_sends_one_explicit_request() {
    let home = TempDir::new().unwrap();
    let server = endpoint(home.path(), json!({"schema":1,"ok":true}).to_string());
    let output = command(home.path()).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        server.join().unwrap(),
        vec![json!({"command":"agent_new_session","target":"paused-agent"})]
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["target"], "paused-agent");
    assert_eq!(result["ok"], true);
}

#[test]
fn new_session_preserves_backend_rejections_without_fallback_or_disk_mutation() {
    for (code, message) in [
        ("bad_request", "unknown variant agent_new_session"),
        ("request_failed", "conversation lifecycle lease is held"),
        (
            "request_failed",
            "Failed to acquire the closing provider log: conversation archive append failed: conflicting source identity\n\n\
             Your current conversation is preserved. Retry New Session. \
             If the error persists, report this capture error.",
        ),
        ("not_found", "agent not found: paused-agent"),
    ] {
        let home = TempDir::new().unwrap();
        let state = home.path().join("settings/state.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        let original =
            b"[{\"session_id\":\"saved-agent\",\"is_off\":true,\"resume_session\":null}]";
        fs::write(&state, original).unwrap();
        let server = endpoint(
            home.path(),
            json!({"schema":1,"error":{"code":code,"message":message}}).to_string(),
        );
        let output = command(home.path()).output().unwrap();
        assert!(!output.status.success());
        if code == "request_failed" {
            assert_eq!(output.status.code(), Some(1));
        }
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
        assert_eq!(error["error"]["message"], message);
        assert_eq!(
            server.join().unwrap(),
            vec![json!({"command":"agent_new_session","target":"paused-agent"})]
        );
        assert_eq!(fs::read(&state).unwrap(), original);
    }
}

#[test]
fn new_session_requires_the_app_even_when_saved_state_exists() {
    let home = TempDir::new().unwrap();
    let state = home.path().join("settings/state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    let original = b"[{\"session_id\":\"paused-agent\",\"is_off\":true,\"resume_session\":null}]";
    fs::write(&state, original).unwrap();
    let output = command(home.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "app_not_running");
    assert_eq!(fs::read(&state).unwrap(), original);
}

fn spawn_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-cli"));
    command
        .args([
            "agent",
            "spawn",
            "--provider",
            "codex",
            "--class",
            "Coder",
            "--name",
            "Fixture",
            "--workspace",
        ])
        .arg(home)
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID");
    command
}

fn operation(state: &str) -> Value {
    json!({
        "operation_id":"filled-from-request",
        "agent_id":"9f13776e-df94-4bc4-a72b-2d8ea25948ce",
        "agent_name":"Fixture", "provider":"codex", "class":"Coder", "state":state,
        "created_at":"2026-11-10T00:00:00Z", "updated_at":"2026-11-10T00:00:00Z"
    })
}

#[test]
fn asynchronous_spawn_persists_generated_key_before_one_admission() {
    let home = TempDir::new().unwrap();
    let server = endpoint_sequence(
        home.path(),
        vec![Some(json!({"schema":1,"operation":operation("queued")}))],
        true,
    );
    let output = spawn_command(home.path()).arg("--async").output().unwrap();
    let requests = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["command"], "agent_spawn_async");
    let key = requests[0]["operation_id"].as_str().unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["operation_id"], key);
    assert_eq!(result["operation"]["state"], "queued");
    assert!(result["lookup"].as_str().unwrap().contains(key));
    assert!(requests[0]["request"].get("async_mode").is_none());
    assert!(requests[0]["request"].get("wait_timeout").is_none());
}

#[test]
fn normal_spawn_admits_waits_then_preserves_identity_field_output() {
    let home = TempDir::new().unwrap();
    let key = "5d4ebea1-1ce4-44fa-a67e-410f74aa1e0c";
    let server = endpoint_sequence(
        home.path(),
        vec![
            Some(json!({"schema":1,"operation":operation("queued")})),
            Some(
                json!({"schema":1,"operation":operation("registered"),"timed_out":false,
                    "availability":"current", "current_agent":{
                    "name":"Fixture", "uuid":"9f13776e-df94-4bc4-a72b-2d8ea25948ce",
                    "class":"Coder", "provider":"codex", "status":"idle", "status_source":"live",
                    "pid":null, "started_at":null, "workspace":home.path(), "last_status_at":null
                }}),
            ),
        ],
        false,
    );
    let output = spawn_command(home.path())
        .args(["--operation-id", key, "--fields", "uuid,name"])
        .output()
        .unwrap();
    let requests = server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request["command"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["agent_spawn_async", "agent_spawn_operation_wait"]
    );
    assert_eq!(requests[0]["operation_id"], key);
    assert_eq!(requests[1]["operation_id"], key);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({"schema":1,"agent":{"uuid":"9f13776e-df94-4bc4-a72b-2d8ea25948ce","name":"Fixture"}})
    );
}

#[test]
fn registered_retry_requires_authoritative_current_roster_identity() {
    for availability in ["missing", "incompatible", "unavailable"] {
        let home = TempDir::new().unwrap();
        let key = "5d4ebea1-1ce4-44fa-a67e-410f74aa1e0c";
        let server = endpoint_sequence(
            home.path(),
            vec![
                Some(json!({"schema":1,"operation":operation("registered")})),
                Some(
                    json!({"schema":1,"operation":operation("registered"),"timed_out":false,"availability":availability}),
                ),
            ],
            false,
        );
        let output = spawn_command(home.path())
            .args(["--operation-id", key])
            .output()
            .unwrap();
        let requests = server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(requests.len(), 2);
        assert!(requests
            .iter()
            .all(|request| request["operation_id"] == key));
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().contains(key));
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("current roster identity"));
    }
}

const VACANCY_AGENT: &str = "1199071a-27b5-443b-bbb1-4c9818425500";
const VACANCY_AUTHORIZATION: &str = "a5f6ad8a-0ac5-4ec2-99e0-b856663594a8";

fn archive_vacancy_command(home: &Path, dry_run: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wardian-cli"));
    command
        .args([
            "agent",
            "archive-vacancy",
            VACANCY_AGENT,
            "--conversation-id",
            "conversation-copy",
            "--sequence",
            "2",
        ])
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID");
    if dry_run {
        command.arg("--dry-run");
    } else {
        command.args([
            "--authorization-id",
            VACANCY_AUTHORIZATION,
            "--approval-reference",
            "recorded-approval-1",
            "--approval-text",
            "I approve exactly this vacancy.\nKeep this text verbatim.",
        ]);
    }
    command
}

fn copied_vacancy_home() -> TempDir {
    let home = tempfile::Builder::new()
        .prefix("wardian-vacancy # %-")
        .tempdir()
        .unwrap();
    let directory = home
        .path()
        .join("agents")
        .join(VACANCY_AGENT)
        .join("conversations/conversation-copy");
    fs::create_dir_all(&directory).unwrap();
    let manifest = json!({
        "schema": 1, "conversation_id": "conversation-copy", "agent_id": VACANCY_AGENT,
        "agent_name": "Copied fixture", "agent_class": "coder", "workspace": "<workspace-path>",
        "provider": "codex", "provider_session_ids": [], "effective_logging": "enabled",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        "closed_at": null, "status": "open", "boundary_reason": "spawn",
        "format_versions": {"manifest": 1, "conversation": 1, "events": 1, "sources": 1, "turns": 1}
    });
    fs::write(directory.join("manifest.json"), manifest.to_string()).unwrap();
    for file in [
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
    ] {
        fs::write(directory.join(file), "").unwrap();
    }
    home
}

type VacancyHomeSnapshot = Vec<(std::path::PathBuf, Option<Vec<u8>>, std::time::SystemTime)>;

fn vacancy_home_snapshot(home: &Path) -> VacancyHomeSnapshot {
    fn visit(root: &Path, directory: &Path, result: &mut VacancyHomeSnapshot) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            // Windows DirEntry metadata can retain an earlier directory-index
            // timestamp. Query the path afresh before claiming a write occurred.
            let metadata = fs::metadata(entry.path()).unwrap();
            result.push((
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                metadata.is_file().then(|| fs::read(entry.path()).unwrap()),
                metadata.modified().unwrap(),
            ));
            if metadata.is_dir() {
                visit(root, &entry.path(), result);
            }
        }
    }
    let mut result = Vec::new();
    visit(home, home, &mut result);
    result.sort();
    result
}

#[test]
fn archive_vacancy_inventory_is_stable_during_direct_read_only_inspection() {
    for _ in 0..32 {
        let home = copied_vacancy_home();
        let directory = home
            .path()
            .join("agents")
            .join(VACANCY_AGENT)
            .join("conversations/conversation-copy");
        let before = vacancy_home_snapshot(home.path());
        wardian_core::archive_vacancy::inspect_vacancy(&directory, "conversation-copy", 2).unwrap();
        assert!(
            vacancy_home_snapshot(home.path()) == before,
            "inventory changed during direct read-only inspection without a CLI process"
        );
    }
}

#[test]
fn archive_vacancy_exact_uuid_dry_run_needs_only_a_copied_conversation_and_writes_nothing() {
    let home = copied_vacancy_home();
    let before = vacancy_home_snapshot(home.path());
    let output = archive_vacancy_command(home.path(), true).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema"], 1);
    assert_eq!(report["agent_id"], VACANCY_AGENT);
    assert_eq!(report["conversation_id"], "conversation-copy");
    assert_eq!(report["sequence"], 2);
    assert_eq!(report["dry_run"], true);
    assert_eq!(
        report["eligible"], false,
        "fixture has no committed-head evidence"
    );
    let rules = report["rules"].as_array().expect("named rule outcomes");
    assert!(!rules.is_empty());
    assert!(rules.iter().all(
        |rule| rule["rule"].as_str().is_some_and(|name| !name.is_empty())
            && rule["passed"].is_boolean()
    ));
    assert!(rules.iter().any(|rule| rule["passed"] == false));
    assert_eq!(vacancy_home_snapshot(home.path()), before);
    assert!(!home.path().join("state.db").exists());
}

#[test]
fn archive_vacancy_exact_uuid_dry_run_reports_eligible_evidence_without_writes() {
    let home = copied_vacancy_home();
    let directory = home
        .path()
        .join("agents")
        .join(VACANCY_AGENT)
        .join("conversations/conversation-copy");
    let manifest_path = directory.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["format_versions"]["turns"] = json!(3);
    manifest["record_count"] = json!(2);
    manifest["turn_count"] = json!(0);
    manifest["has_turns"] = json!(false);
    fs::write(manifest_path, manifest.to_string()).unwrap();
    let mut records = String::new();
    let mut events = String::new();
    for seq in [1, 3] {
        let role = if seq == 1 { "user" } else { "assistant" };
        records.push_str(
            &json!({"schema":1,"seq":seq,"at":"2026-01-01T00:00:00Z",
            "kind":"message","role":role,"speaker_type":null,"text":"synthetic fixture",
            "event_refs":[format!("event-{seq}")],"source_refs":[],"artifact_refs":[]})
            .to_string(),
        );
        records.push('\n');
        events.push_str(
            &json!({"id":format!("event-{seq}"),"session_id":VACANCY_AGENT,
            "provider":"codex","kind":"message","role":role,"text":"synthetic fixture",
            "metadata":{}})
            .to_string(),
        );
        events.push('\n');
    }
    fs::write(directory.join("conversation.jsonl"), records).unwrap();
    fs::write(directory.join("events.jsonl"), events).unwrap();
    let before = vacancy_home_snapshot(home.path());
    let output = archive_vacancy_command(home.path(), true).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["eligible"], true, "{report}");
    assert_eq!(report["committed_head_sequence"], 3);
    assert!(report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .all(|rule| rule["passed"] == true));
    for field in ["prev_record_hash", "next_record_hash", "scan_digest"] {
        assert_eq!(report["evidence"][field].as_str().unwrap().len(), 64);
    }
    assert_eq!(vacancy_home_snapshot(home.path()), before);
    assert!(!home.path().join("state.db").exists());
}

#[test]
fn archive_vacancy_unavailable_endpoint_never_performs_offline_repairs() {
    let home = copied_vacancy_home();
    let before = vacancy_home_snapshot(home.path());
    let output = archive_vacancy_command(home.path(), false)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(vacancy_home_snapshot(home.path()), before);
}

#[test]
fn archive_vacancy_older_endpoint_rejection_is_terminal_and_approval_is_exact() {
    let home = copied_vacancy_home();
    let server = endpoint(
        home.path(),
        json!({
            "schema": 1,
            "error": {"code": "bad_request", "message": "unknown variant agent_archive_vacancy"}
        })
        .to_string(),
    );
    let before = vacancy_home_snapshot(home.path());
    let output = archive_vacancy_command(home.path(), false)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "bad_request");
    assert_eq!(
        server.join().unwrap(),
        vec![json!({
            "command": "agent_archive_vacancy", "target": VACANCY_AGENT,
            "request": {"conversation_id": "conversation-copy", "sequence": 2,
                "authorization_id": VACANCY_AUTHORIZATION,
                "approval_reference": "recorded-approval-1",
                "approval_text": "I approve exactly this vacancy.\nKeep this text verbatim."}
        })]
    );
    assert_eq!(vacancy_home_snapshot(home.path()), before);
}

#[test]
fn archive_vacancy_incompatible_success_response_cannot_report_authorization() {
    let home = copied_vacancy_home();
    let server = endpoint(home.path(), json!({"schema": 1, "ok": true}).to_string());
    let before = vacancy_home_snapshot(home.path());
    let output = archive_vacancy_command(home.path(), false)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(server.join().unwrap().len(), 1);
    assert_eq!(vacancy_home_snapshot(home.path()), before);
}

#[test]
fn archive_vacancy_reports_the_authoritative_retry_verdict_without_local_writes() {
    let home = copied_vacancy_home();
    let response = json!({
        "schema": 1, "agent_id": VACANCY_AGENT, "conversation_id": "conversation-copy",
        "sequence": 2, "created": false, "adoption_regrant_required": true,
        "reason": "Re-grant archive adoption before capture resumes."
    });
    let server = endpoint(home.path(), response.to_string());
    let before = vacancy_home_snapshot(home.path());
    let output = archive_vacancy_command(home.path(), false)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        response
    );
    assert_eq!(server.join().unwrap().len(), 1);
    assert_eq!(vacancy_home_snapshot(home.path()), before);
}

#[test]
fn archive_vacancy_rejects_invalid_scope_and_approval_before_endpoint_access() {
    for (flag, invalid) in [
        ("--authorization-id", "invalid".to_string()),
        ("--conversation-id", "../conversation-copy".to_string()),
        ("--approval-reference", " \t".to_string()),
        ("--approval-text", "\n".to_string()),
        ("--approval-reference", "x".repeat(4097)),
        ("--approval-text", "x".repeat(16385)),
    ] {
        let home = copied_vacancy_home();
        let before = vacancy_home_snapshot(home.path());
        let command = archive_vacancy_command(home.path(), false);
        let mut args: Vec<_> = command.get_args().map(|arg| arg.to_os_string()).collect();
        let value = args.iter().position(|arg| arg == flag).unwrap() + 1;
        args[value] = invalid.into();
        let output = Command::new(command.get_program())
            .args(args)
            .env("WARDIAN_HOME", home.path())
            .env_remove("WARDIAN_SESSION_ID")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flag}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "generic", "{flag}: {error}");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(&flag.trim_start_matches('-').replace('-', "_")),
            "{error}"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(vacancy_home_snapshot(home.path()), before);
    }
}

#[test]
fn archive_vacancy_name_dry_run_uses_persisted_identity_without_db_migrations_or_writes() {
    let home = copied_vacancy_home();
    let conn = rusqlite::Connection::open(home.path().join("state.db")).unwrap();
    wardian_core::db::run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO agents (session_id, session_name, agent_class) VALUES (?1, 'copied-agent', 'Coder')",
        [VACANCY_AGENT],
    ).unwrap();
    drop(conn);
    let before = vacancy_home_snapshot(home.path());
    let command = archive_vacancy_command(home.path(), true);
    let mut args: Vec<_> = command.get_args().map(|arg| arg.to_os_string()).collect();
    args[2] = "copied-agent".into();
    let output = Command::new(command.get_program())
        .args(args)
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["agent_id"], VACANCY_AGENT);
    assert_eq!(vacancy_home_snapshot(home.path()), before);
}

#[test]
fn rejected_or_lost_spawn_admission_retains_key_without_legacy_fallback_or_replay() {
    for response in [
        Some(
            json!({"schema":1,"error":{"code":"bad_request", "message":"malformed control request JSON: unknown variant `agent_spawn_async`"}}),
        ),
        None,
    ] {
        let home = TempDir::new().unwrap();
        let state = home.path().join("settings/state.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, "[]").unwrap();
        let old_app = response.is_some();
        let server = endpoint_sequence(home.path(), vec![response], true);
        let output = spawn_command(home.path()).output().unwrap();
        let requests = server.join().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["command"], "agent_spawn_async");
        let key = requests[0]["operation_id"].as_str().unwrap();
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().contains(key));
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("wardian agent spawn-status"));
        if old_app {
            assert_eq!(error["error"]["code"], "unsupported_version");
        }
        assert_eq!(fs::read_to_string(&state).unwrap(), "[]");
        let receipts = fs::read_dir(home.path().join("spawn-submissions"))
            .unwrap()
            .count();
        assert_eq!(receipts, 1);
    }
}
