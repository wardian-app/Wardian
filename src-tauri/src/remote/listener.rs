//! One joined remote listener per desktop runtime.
use super::models::RemoteGatewayConfig;
use axum::Router;
use std::net::SocketAddr;
use tokio::{
    sync::{watch, Mutex},
    task::JoinHandle,
};

#[derive(Default)]
pub struct RemoteListener {
    state: Mutex<ListenerState>,
}

#[derive(Default)]
struct ListenerState {
    closed: bool,
    running: Option<RunningListener>,
}

struct RunningListener {
    config: RemoteGatewayConfig,
    stop: watch::Sender<bool>,
    task: JoinHandle<Result<(), String>>,
}

impl Drop for RunningListener {
    fn drop(&mut self) {
        // Cancellation of a settings request must not detach an unowned server.
        self.task.abort();
    }
}

impl RemoteListener {
    /// Serialize settings changes, release the previous socket, and bind before
    /// reporting success. A foreign listener is never terminated or adopted.
    pub async fn configure(
        &self,
        config: RemoteGatewayConfig,
        addr: SocketAddr,
        router: Router,
    ) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if state.closed {
            return Err("Remote gateway is shutting down".into());
        }
        if state.running.as_ref().is_some_and(|running| {
            config.enabled
                && running.config == config
                && !*running.stop.borrow()
                && !running.task.is_finished()
        }) {
            return Ok(());
        }
        stop_running(&mut state).await?;
        if !config.enabled {
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|error| format!("Remote gateway could not bind {addr}: {error}"))?;
        let (stop, mut stopping) = watch::channel(false);
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopping.wait_for(|value| *value).await;
                })
                .await
                .map_err(|error| error.to_string())
        });
        state.running = Some(RunningListener { config, stop, task });
        Ok(())
    }

    /// Fence late startup/settings work and join the task that owns the socket.
    pub async fn shutdown(&self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        state.closed = true;
        stop_running(&mut state).await
    }
}

async fn stop_running(state: &mut ListenerState) -> Result<(), String> {
    let result =
        if let Some(running) = state.running.as_mut() {
            let _ = running.stop.send(true);
            // Retain the slot until joined, including if this caller is cancelled.
            let result =
                match tokio::time::timeout(std::time::Duration::from_secs(2), &mut running.task)
                    .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        running.task.abort();
                        (&mut running.task).await
                    }
                };
            match result {
                Ok(result) => result,
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(format!("Remote gateway task failed: {error}")),
            }
        } else {
            Ok(())
        };
    state.running = None;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(port: u16) -> RemoteGatewayConfig {
        RemoteGatewayConfig {
            schema_version: 1,
            enabled: true,
            canonical_origin: "https://wardian.example".into(),
            loopback_host: "127.0.0.1".into(),
            loopback_port: port,
            gateway_identity_public_key: "test".into(),
            gateway_identity_fingerprint: "test".into(),
        }
    }

    async fn unused_addr() -> SocketAddr {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
    }

    #[tokio::test]
    async fn repeated_save_replace_disable_and_exit_release_real_sockets() {
        let runtime = RemoteListener::default();
        let addr = unused_addr().await;
        let first = config(addr.port());
        runtime
            .configure(first.clone(), addr, Router::new())
            .await
            .unwrap();
        runtime
            .configure(first.clone(), addr, Router::new())
            .await
            .unwrap();
        assert!(tokio::net::TcpListener::bind(addr).await.is_err());
        let mut replacement = first.clone();
        replacement.canonical_origin = "https://new.wardian.example".into();
        runtime
            .configure(replacement, addr, Router::new())
            .await
            .unwrap();
        assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
        let mut disabled = first.clone();
        disabled.enabled = false;
        runtime
            .configure(disabled, addr, Router::new())
            .await
            .unwrap();
        let rebound = tokio::net::TcpListener::bind(addr).await.unwrap();
        drop(rebound);
        runtime
            .configure(first.clone(), addr, Router::new())
            .await
            .unwrap();
        runtime.shutdown().await.unwrap();
        runtime.shutdown().await.unwrap();
        let _rebound = tokio::net::TcpListener::bind(addr).await.unwrap();
        assert!(runtime.configure(first, addr, Router::new()).await.is_err());
    }

    #[tokio::test]
    async fn foreign_listener_is_preserved_and_binding_can_retry() {
        let runtime = RemoteListener::default();
        let foreign = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = foreign.local_addr().unwrap();
        let error = runtime
            .configure(config(addr.port()), addr, Router::new())
            .await
            .unwrap_err();
        assert!(error.contains("could not bind"));
        assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
        drop(foreign);
        runtime
            .configure(config(addr.port()), addr, Router::new())
            .await
            .unwrap();
        runtime.shutdown().await.unwrap();
        let _rebound = tokio::net::TcpListener::bind(addr).await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_configurations_have_one_owned_listener() {
        let runtime = RemoteListener::default();
        let addr = unused_addr().await;
        let settings = config(addr.port());
        let (a, b) = tokio::join!(
            runtime.configure(settings.clone(), addr, Router::new()),
            runtime.configure(settings, addr, Router::new())
        );
        a.unwrap();
        b.unwrap();
        runtime.shutdown().await.unwrap();
        let _rebound = tokio::net::TcpListener::bind(addr).await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_drain_cannot_reuse_a_stopping_server() {
        use tokio::io::AsyncWriteExt;
        let runtime = RemoteListener::default();
        let addr = unused_addr().await;
        let first = config(addr.port());
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let finish = std::sync::Arc::new(tokio::sync::Notify::new());
        let router = Router::new().route(
            "/",
            axum::routing::get({
                let entered = entered.clone();
                let finish = finish.clone();
                move || {
                    let entered = entered.clone();
                    let finish = finish.clone();
                    async move {
                        entered.notify_one();
                        finish.notified().await;
                        "done"
                    }
                }
            }),
        );
        runtime
            .configure(first.clone(), addr, router)
            .await
            .unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        let mut replacement = first.clone();
        replacement.canonical_origin = "https://new.wardian.example".into();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(25),
            runtime.configure(replacement, addr, Router::new())
        )
        .await
        .is_err());
        // The original config must join the stopping server, not return success
        // while its in-flight request prevents shutdown from completing.
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(25),
            runtime.configure(first.clone(), addr, Router::new())
        )
        .await
        .is_err());
        finish.notify_one();
        runtime.configure(first, addr, Router::new()).await.unwrap();
        assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
        runtime.shutdown().await.unwrap();
        let _rebound = tokio::net::TcpListener::bind(addr).await.unwrap();
    }
}
