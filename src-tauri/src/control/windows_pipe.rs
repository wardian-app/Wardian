use std::future::Future;
use std::io;
use std::time::Duration;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

const ERROR_PIPE_BUSY: i32 = 231;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(10);
const MAX_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Accept control connections independently of their request framing and dispatch.
pub(super) async fn serve(
    pipe_name: &str,
    first_server: NamedPipeServer,
    dispatch: impl FnMut(NamedPipeServer),
) -> io::Result<()> {
    serve_with_factory(
        first_server,
        || std::future::ready(ServerOptions::new().create(pipe_name)),
        dispatch,
    )
    .await
}

async fn serve_with_factory<F, Fut, H>(
    first_server: NamedPipeServer,
    mut create: F,
    mut dispatch: H,
) -> io::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = io::Result<NamedPipeServer>>,
    H: FnMut(NamedPipeServer),
{
    let mut server = first_server;
    loop {
        server.connect().await?;

        // Retain the accepted handle until its replacement exists. Dispatching first
        // could drop the last instance and allow another process to claim the endpoint.
        let mut retry_delay = INITIAL_RETRY_DELAY;
        let mut busy_burst = false;
        let next_server = loop {
            match create().await {
                Ok(next_server) => break next_server,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if !busy_burst {
                        crate::utils::logging::log_debug(&format!(
                            "[Wardian] control pipe replenishment recovery: os_code={ERROR_PIPE_BUSY}"
                        ));
                        busy_burst = true;
                    }
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                }
                Err(error) => return Err(error),
            }
        };
        dispatch(server);
        server = next_server;
    }
}

#[cfg(test)]
#[path = "windows_pipe_tests.rs"]
mod tests;
