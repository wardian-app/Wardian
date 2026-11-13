//! A controlled contained process plus supported RPC frames test the stop contract.
//! This is protocol/OS evidence, not real-provider acceptance.
use super::*;
use crate::delivery::codex_shared::*;
use futures_util::{SinkExt, StreamExt};
use std::path::Path;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterCase {
    Owned,
    AlreadyUnloaded,
    DaemonHosted,
    ForeignLoaded,
    MissingUnload,
    UnverifiedExit,
}

pub(crate) struct StopFixture {
    pub owner: Arc<CodexSharedOwner>,
    pub requests: Arc<StdMutex<Vec<serde_json::Value>>>,
    pub external_writer: Option<tokio::process::Child>,
    server: tokio::task::JoinHandle<()>,
    backup_job: Option<crate::utils::process::RuntimeProcessJob>,
}

impl StopFixture {
    pub(crate) async fn restore_containment(&mut self) {
        if let Some(job) = self.backup_job.take() {
            *self.owner.job.lock().await = Some(job);
        }
    }

    pub(crate) async fn cleanup(mut self) {
        self.restore_containment().await;
        self.owner.shutdown().await.unwrap();
        if let Some(child) = self.external_writer.as_mut() {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
        }
        self.server.abort();
        let _ = self.server.await;
    }
}

fn child_command() -> tokio::process::Command {
    let mut command = crate::utils::process::new_silent_command("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Start-Sleep -Seconds 120",
    ]);
    command.kill_on_drop(true);
    command
}

pub(crate) async fn stop_fixture(
    directory: &Path,
    agent_id: &str,
    generation: u64,
    thread_id: &str,
    case: WriterCase,
) -> StopFixture {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let recorded = requests.clone();
    let thread = thread_id.to_owned();
    let server = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(server_io).await.unwrap();
        let mut unloaded = matches!(case, WriterCase::DaemonHosted | WriterCase::AlreadyUnloaded);
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let request: serde_json::Value = serde_json::from_str(&text).unwrap();
            recorded.lock().unwrap().push(request.clone());
            let result = match request["method"].as_str().unwrap() {
                "thread/loaded/list" => {
                    json!({"data": if unloaded { vec![] } else if case == WriterCase::ForeignLoaded { vec!["foreign".to_string()] } else { vec![thread.clone()] }, "nextCursor":null})
                }
                "thread/unsubscribe" => {
                    if case == WriterCase::MissingUnload {
                        json!({})
                    } else {
                        unloaded = true;
                        json!({"status":"unsubscribed"})
                    }
                }
                other => panic!("unexpected stop request: {other}"),
            };
            if socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let (socket, _) = tokio_tungstenite::client_async("ws://localhost", client_io)
        .await
        .unwrap();
    let client = CodexSharedClient::from_connected(agent_id.to_owned(), generation, socket, None);
    client.bind(&json!({"thread":{"id":thread_id,"canAcceptDirectInput":true,"status":{"type":"idle"},"turns":[]}})).unwrap();
    let (child, real_job) =
        crate::utils::process::OwnedBootstrapChild::spawn_contained_runtime(&mut child_command())
            .unwrap();
    let (job, backup_job) = if case == WriterCase::UnverifiedExit {
        (
            crate::utils::process::RuntimeProcessJob::fallback(
                crate::utils::process::create_kill_on_close_job("unverified test boundary")
                    .unwrap(),
            ),
            Some(real_job),
        )
    } else {
        (real_job, None)
    };
    let socket_path = directory.join("controlled-owner.sock");
    let listener = uds_windows::UnixListener::bind(&socket_path).unwrap();
    drop(listener);
    let policy_args = [
        "app-server",
        "-c",
        "sandbox_mode='workspace-write'",
        "-c",
        "approval_policy='on-request'",
    ]
    .map(str::to_owned);
    let owner = Arc::new(CodexSharedOwner {
        client,
        observed_version: "0.154.0".into(),
        attachment: super::super::CodexTuiAttachment {
            model_override: None,
            permission_args: Vec::new(),
            expected_resume_id: Some(thread_id.to_owned()),
            codex_home: directory.to_path_buf(),
            generation,
        },
        child: Mutex::new(Some(child)),
        stderr_capture: Mutex::new(None),
        interactive: true,
        initial_context: String::new(),
        policy: super::super::policy::ExpectedPolicy::from_server_args(&policy_args).unwrap(),
        socket: attachment::OwnedSocket::capture(&socket_path).unwrap(),
        launch_config: Mutex::new(None),
        settings_operation: Mutex::new(()),
        _memory: None,
        verified_thread: StdMutex::new(
            (case != WriterCase::DaemonHosted).then(|| thread_id.to_owned()),
        ),
        job: Mutex::new(Some(job)),
    });
    let external_writer =
        (case == WriterCase::DaemonHosted).then(|| child_command().spawn().unwrap());
    StopFixture {
        owner,
        requests,
        external_writer,
        server,
        backup_job,
    }
}

#[tokio::test]
async fn receipt_requires_exact_binding_supported_unload_and_verified_exit() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    for case in [
        WriterCase::Owned,
        WriterCase::AlreadyUnloaded,
        WriterCase::DaemonHosted,
        WriterCase::ForeignLoaded,
        WriterCase::MissingUnload,
        WriterCase::UnverifiedExit,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let fixture = stop_fixture(directory.path(), "owner", 7, "thread", case).await;
        let result = fixture
            .owner
            .stop_after_tui("owner", 7, Some("thread"), true)
            .await;
        if case == WriterCase::UnverifiedExit {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap().is_some(),
                matches!(case, WriterCase::Owned | WriterCase::AlreadyUnloaded)
            );
        }
        if case == WriterCase::Owned {
            assert_eq!(
                fixture
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|request| request["method"] == "thread/unsubscribe")
                    .count(),
                1
            );
        }
        fixture.cleanup().await;
    }
}

#[tokio::test]
async fn cancelled_before_runtime_capture_preserves_the_live_registered_owner() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let directory = tempfile::tempdir().unwrap();
    let fixture = stop_fixture(directory.path(), "owner", 7, "thread", WriterCase::Owned).await;
    let broker = Arc::new(crate::delivery::native_broker::NativeDeliveryBroker::new());
    broker
        .install_stop_test_owner("owner", 7, fixture.owner.clone())
        .await;
    let captured = broker.capture_codex_stop("owner", Some(7)).await.unwrap();
    drop(captured);
    assert!(fixture
        .owner
        .child
        .lock()
        .await
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert!(fixture.owner.client.receipt("still_owned").is_ok());
    broker.dispose_agent("owner").await.unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
async fn wrong_generation_or_missing_tui_exit_cannot_issue_receipt() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    for (agent, generation, thread, tui_exited) in [
        ("owner", 8, "thread", true),
        ("foreign", 7, "thread", true),
        ("owner", 7, "foreign", true),
        ("owner", 7, "thread", false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let fixture = stop_fixture(directory.path(), "owner", 7, "thread", WriterCase::Owned).await;
        assert!(fixture
            .owner
            .stop_after_tui(agent, generation, Some(thread), tui_exited)
            .await
            .unwrap()
            .is_none());
        assert!(fixture.requests.lock().unwrap().is_empty());
        fixture.cleanup().await;
    }
}

#[tokio::test]
async fn private_receipt_cannot_release_another_thread_agent_or_generation() {
    use wardian_core::conversation_lease::{load_leases_checked, save_leases, ConversationLease};
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let fixture = stop_fixture(home.path(), "owner", 7, "thread", WriterCase::Owned).await;
    let receipt = fixture
        .owner
        .stop_after_tui("owner", 7, Some("thread"), true)
        .await
        .unwrap()
        .unwrap();
    let hold = ConversationLease {
        agent_id: "owner".into(),
        provider: "codex".into(),
        resume_session: "thread".into(),
        owner_kind: "prior_provider_hold".into(),
        owner_id: "held-owner".into(),
        acquisition_id: "held-acquisition".into(),
        owner_node_id: None,
        mode: "uncertain_previous_provider".into(),
        started_at: "2026-11-13T00:00:00Z".into(),
        heartbeat_at: "2026-11-13T00:00:00Z".into(),
        expires_at: "9999-12-31T23:59:59Z".into(),
    };
    for (agent, thread, generation) in [
        ("foreign", "thread", 7),
        ("owner", "foreign", 7),
        ("owner", "thread", 8),
    ] {
        let mut foreign = hold.clone();
        foreign.agent_id = agent.into();
        foreign.resume_session = thread.into();
        save_leases(std::slice::from_ref(&foreign)).unwrap();
        assert!(receipt
            .release_hold(&foreign, generation)
            .unwrap_err()
            .contains("does not match"));
        assert_eq!(load_leases_checked().unwrap(), vec![foreign]);
    }
    save_leases(std::slice::from_ref(&hold)).unwrap();
    receipt.release_hold(&hold, 7).unwrap();
    assert!(load_leases_checked().unwrap().is_empty());
    fixture.cleanup().await;
}
