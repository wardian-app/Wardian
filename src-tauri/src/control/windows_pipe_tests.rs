use super::*;
use crate::control::test_support::TestWardianHome;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use wardian_core::control::ControlRequest;

fn unique_pipe() -> String {
    format!(r"\\.\pipe\wardian-control-test-{}", uuid::Uuid::new_v4())
}

async fn open_client(pipe_name: &str) -> io::Result<NamedPipeClient> {
    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => return Ok(client),
            Err(error) if error.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn agent_list_exchange(pipe_name: &str) -> io::Result<serde_json::Value> {
    let mut client = open_client(pipe_name).await?;
    let request = serde_json::to_string(&ControlRequest::AgentList).unwrap();
    client.write_all(request.as_bytes()).await?;
    client.write_all(b"\n").await?;
    client.flush().await?;
    let mut response = String::new();
    BufReader::new(client).read_line(&mut response).await?;
    serde_json::from_str(&response).map_err(io::Error::other)
}

async fn dispatch_read_only(server: NamedPipeServer, requests: Arc<AtomicUsize>) -> io::Result<()> {
    let mut reader = BufReader::new(server);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let request: ControlRequest = serde_json::from_str(&line).map_err(io::Error::other)?;
    if !matches!(request, ControlRequest::AgentList) {
        return Err(io::Error::other(
            "fixture accepts only the read-only agent list",
        ));
    }
    requests.fetch_add(1, Ordering::SeqCst);
    reader
        .get_mut()
        .write_all(b"{\"schema\":1,\"agents\":[]}\n")
        .await?;
    reader.get_mut().flush().await
}

struct TestTasks {
    listener: JoinHandle<io::Result<()>>,
    heartbeat: JoinHandle<()>,
    handlers: Arc<Mutex<Vec<JoinHandle<io::Result<()>>>>>,
}

impl TestTasks {
    async fn shutdown(&mut self) {
        self.listener.abort();
        self.heartbeat.abort();
        let handlers = std::mem::take(&mut *self.handlers.lock().unwrap());
        for handler in &handlers {
            handler.abort();
        }
        let _ = (&mut self.listener).await;
        let _ = (&mut self.heartbeat).await;
        for handler in handlers {
            let _ = handler.await;
        }
    }
}

impl Drop for TestTasks {
    fn drop(&mut self) {
        self.listener.abort();
        self.heartbeat.abort();
        if let Ok(mut handlers) = self.handlers.lock() {
            for handler in handlers.drain(..) {
                handler.abort();
            }
        }
    }
}

#[tokio::test]
async fn pipe_capacity_and_exclusive_claim_have_distinct_os_errors() {
    let capacity_name = unique_pipe();
    let capacity = ServerOptions::new()
        .first_pipe_instance(true)
        .max_instances(1)
        .create(&capacity_name)
        .unwrap();
    let error = ServerOptions::new()
        .max_instances(1)
        .create(&capacity_name)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(231));
    drop(capacity);

    let exclusive_name = unique_pipe();
    let exclusive = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&exclusive_name)
        .unwrap();
    let error = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&exclusive_name)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(5));
    drop(exclusive);
}

#[tokio::test]
async fn listener_recovers_replenishment_with_incomplete_client_and_live_heartbeat() {
    let _home = TestWardianHome::new_async().await;
    let pipe_name = unique_pipe();
    let first_server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap();
    let mut client_a = ClientOptions::new().open(&pipe_name).unwrap();
    client_a.write_all(b"{").await.unwrap();

    let (fault_tx, fault_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let mut fault = Some((fault_tx, release_rx));
    let factory_name = pipe_name.clone();
    let creates = Arc::new(AtomicUsize::new(0));
    let factory_creates = creates.clone();
    let dispatch_creates = creates.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let handler_requests = requests.clone();
    let handlers = Arc::new(Mutex::new(Vec::new()));
    let handler_tasks = handlers.clone();
    let listener = tokio::spawn(serve_with_factory(
        first_server,
        move || {
            factory_creates.fetch_add(1, Ordering::SeqCst);
            let fault = fault.take();
            let pipe_name = factory_name.clone();
            async move {
                if let Some((fault_tx, release_rx)) = fault {
                    let _ = fault_tx.send(());
                    release_rx.await.unwrap();
                    // The capacity fixture above observes this exact CreateNamedPipe error.
                    Err(io::Error::from_raw_os_error(231))
                } else {
                    ServerOptions::new().create(&pipe_name)
                }
            }
        },
        move |server| {
            assert!(
                dispatch_creates.load(Ordering::SeqCst) >= 2,
                "replacement must exist before dispatch"
            );
            handler_tasks
                .lock()
                .unwrap()
                .push(tokio::spawn(dispatch_read_only(
                    server,
                    handler_requests.clone(),
                )));
        },
    ));
    let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel(1);
    let heartbeat = tokio::spawn(async move {
        loop {
            if heartbeat_tx.send(()).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let mut tasks = TestTasks {
        listener,
        heartbeat,
        handlers,
    };
    tokio::time::timeout(Duration::from_secs(2), fault_rx)
        .await
        .expect("production loop must reach the replenishment barrier")
        .unwrap();
    let foreign = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap_err();
    assert_eq!(
        foreign.raw_os_error(),
        Some(5),
        "owned endpoint must stay claimed"
    );
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(1), heartbeat_rx.recv())
            .await
            .expect("independent heartbeat must remain live")
            .expect("heartbeat task must remain owned");
    }
    release_tx.send(()).unwrap();
    let exchange =
        tokio::time::timeout(Duration::from_secs(1), agent_list_exchange(&pipe_name)).await;
    assert!(
        exchange.is_ok(),
        "client B must complete while A lacks newline framing; listener_finished={}, creates={}",
        tasks.listener.is_finished(),
        creates.load(Ordering::SeqCst)
    );
    assert_eq!(
        exchange.unwrap().unwrap(),
        serde_json::json!({"schema":1,"agents":[]})
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "incomplete A must not dispatch"
    );
    assert!(
        !tasks.listener.is_finished(),
        "listener must survive the transient error"
    );
    tasks.shutdown().await;
}

#[tokio::test]
async fn closed_client_before_accept_does_not_retire_the_listener() {
    let _home = TestWardianHome::new_async().await;
    let pipe_name = unique_pipe();
    let first_server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap();
    // A synchronous client closes its OS handle before acceptance. Mio treats
    // ERROR_NO_DATA as acceptance; the resulting EOF belongs to this handler.
    drop(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&pipe_name)
            .unwrap(),
    );
    let listener_name = pipe_name.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let handler_requests = requests.clone();
    let handlers = Arc::new(Mutex::new(Vec::new()));
    let handler_tasks = handlers.clone();
    let listener = tokio::spawn(async move {
        serve(&listener_name, first_server, move |server| {
            handler_tasks
                .lock()
                .unwrap()
                .push(tokio::spawn(dispatch_read_only(
                    server,
                    handler_requests.clone(),
                )));
        })
        .await
    });
    let mut tasks = TestTasks {
        listener,
        heartbeat: tokio::spawn(async {}),
        handlers,
    };
    let response = tokio::time::timeout(Duration::from_secs(1), agent_list_exchange(&pipe_name))
        .await
        .expect("a closed client must not retire the listener")
        .unwrap();
    assert_eq!(response, serde_json::json!({"schema":1,"agents":[]}));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(!tasks.listener.is_finished());
    tasks.shutdown().await;
}

#[tokio::test]
async fn fatal_replenishment_error_is_not_retried_or_dispatched() {
    let pipe_name = unique_pipe();
    let first_server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .unwrap();
    let _client = ClientOptions::new().open(&pipe_name).unwrap();
    let mut creates = 0;
    let mut dispatched = 0;
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        serve_with_factory(
            first_server,
            || {
                creates += 1;
                std::future::ready(Err(io::Error::from_raw_os_error(5)))
            },
            |_| dispatched += 1,
        ),
    )
    .await
    .expect("unclassified ownership failures must return promptly")
    .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(5));
    assert_eq!(creates, 1);
    assert_eq!(dispatched, 0);
}
