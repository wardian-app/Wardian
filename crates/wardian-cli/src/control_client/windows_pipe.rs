use std::{io, time::Duration};

use tokio::net::windows::named_pipe::ClientOptions;
use wardian_core::control::ControlRequest;

const ERROR_PIPE_BUSY: i32 = 231;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(10);
const MAX_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Retry only a busy open under the caller's deadline, then exchange one request.
pub(super) async fn send_request(
    pipe_name: &str,
    request: ControlRequest,
) -> io::Result<serde_json::Value> {
    let mut stream = open_with_retry(|| ClientOptions::new().open(pipe_name)).await?;
    super::exchange_json(&mut stream, request).await
}

async fn open_with_retry<T>(mut open: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut retry_delay = INITIAL_RETRY_DELAY;

    loop {
        match open() {
            Ok(stream) => return Ok(stream),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                // Yield between attempts so the caller's existing timeout can cancel this loop.
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::windows::named_pipe::{ClientOptions, ServerOptions},
    };

    use super::*;

    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    static NEXT_PIPE_ID: AtomicU64 = AtomicU64::new(1);

    fn private_pipe_name() -> String {
        let id = NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed);
        format!(r"\\.\pipe\wardian-cli-live-{}-{id}", std::process::id())
    }

    #[test]
    fn busy_pipe_open_recovers_when_a_private_pipe_instance_becomes_available() {
        let runtime = super::super::build_runtime().unwrap();
        runtime.block_on(async {
            let pipe_name = private_pipe_name();
            let mut options = ServerOptions::new();
            options.first_pipe_instance(true).max_instances(2);
            let server = options.create(&pipe_name).unwrap();
            let occupied_client = ClientOptions::new().open(&pipe_name).unwrap();
            server.connect().await.unwrap();

            let busy = ClientOptions::new().open(&pipe_name).unwrap_err();
            assert_eq!(busy.raw_os_error(), Some(ERROR_PIPE_BUSY));

            let retry_name = pipe_name.clone();
            let opening = tokio::spawn(async move {
                open_with_retry(|| ClientOptions::new().open(&retry_name)).await
            });
            tokio::time::sleep(MAX_RETRY_DELAY + INITIAL_RETRY_DELAY).await;
            assert!(!opening.is_finished(), "busy open must keep retrying");

            let mut options = ServerOptions::new();
            options.max_instances(2);
            let available_server = options.create(&pipe_name).unwrap();
            let accepting = tokio::spawn(async move { available_server.connect().await });
            let client = tokio::time::timeout(Duration::from_secs(1), opening)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            accepting.await.unwrap().unwrap();

            drop((client, occupied_client, server));
        });
    }

    #[test]
    fn outer_timeout_cancels_retry_while_the_private_pipe_stays_busy() {
        let runtime = super::super::build_runtime().unwrap();
        runtime.block_on(async {
            let pipe_name = private_pipe_name();
            let mut options = ServerOptions::new();
            options.first_pipe_instance(true).max_instances(1);
            let server = options.create(&pipe_name).unwrap();
            let occupied_client = ClientOptions::new().open(&pipe_name).unwrap();
            server.connect().await.unwrap();

            let busy = ClientOptions::new().open(&pipe_name).unwrap_err();
            assert_eq!(busy.raw_os_error(), Some(ERROR_PIPE_BUSY));

            let result = tokio::time::timeout(
                Duration::from_millis(125),
                open_with_retry(|| ClientOptions::new().open(&pipe_name)),
            )
            .await;
            assert!(result.is_err(), "the caller deadline must end busy retries");

            drop((occupied_client, server));
        });
    }

    #[test]
    fn nonbusy_open_errors_return_without_retrying() {
        let runtime = super::super::build_runtime().unwrap();
        runtime.block_on(async {
            for raw_error in [ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND] {
                let mut attempts = 0;
                let error = open_with_retry::<()>(|| {
                    attempts += 1;
                    Err(io::Error::from_raw_os_error(raw_error))
                })
                .await
                .unwrap_err();

                assert_eq!(error.raw_os_error(), Some(raw_error));
                assert_eq!(attempts, 1);
            }
        });
    }

    async fn assert_failed_response_is_not_replayed(response: Option<&'static str>) {
        let pipe_name = private_pipe_name();
        let mut options = ServerOptions::new();
        options.first_pipe_instance(true);
        let mut server = options.create(&pipe_name).unwrap();
        let request_pipe_name = pipe_name.clone();
        let request = tokio::spawn(async move {
            send_request(&request_pipe_name, ControlRequest::AgentList).await
        });
        server.connect().await.unwrap();

        // A second instance observes any reconnect after the first response.
        let probe = ServerOptions::new().create(&pipe_name).unwrap();
        let mut request_line = String::new();
        BufReader::new(&mut server)
            .read_line(&mut request_line)
            .await
            .unwrap();
        assert!(
            !request_line.is_empty(),
            "the connected request must be sent"
        );

        if let Some(response) = response {
            server.write_all(response.as_bytes()).await.unwrap();
            server.write_all(b"\n").await.unwrap();
            server.flush().await.unwrap();
        }
        drop(server);

        assert!(
            tokio::time::timeout(Duration::from_millis(500), probe.connect())
                .await
                .is_err(),
            "a failed response must not open another pipe connection"
        );
        let result = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn connected_malformed_or_failed_responses_are_never_reopened_or_replayed() {
        let runtime = super::super::build_runtime().unwrap();
        runtime.block_on(async {
            assert_failed_response_is_not_replayed(Some("not-json")).await;
            assert_failed_response_is_not_replayed(Some(
                r#"{"error":{"code":"request_failed","message":"rejected"}}"#,
            ))
            .await;
            assert_failed_response_is_not_replayed(None).await;
        });
    }
}
