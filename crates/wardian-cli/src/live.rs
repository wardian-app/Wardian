use std::{io, time::Duration};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{AgentListResponse, ControlRequest};
use wardian_core::identity::AgentIdentity;

const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);

pub fn list_agents() -> io::Result<Vec<AgentIdentity>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| io::Error::other(error.to_string()))?;

    match runtime
        .block_on(async { tokio::time::timeout(CONTROL_TIMEOUT, request_agent_list_async()).await })
    {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Wardian control endpoint timed out",
        )),
    }
}

#[cfg(windows)]
async fn request_agent_list_async() -> std::io::Result<Vec<AgentIdentity>> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control pipe"))?;
    let mut stream = ClientOptions::new().open(pipe_name)?;
    request_agent_list(&mut stream).await
}

#[cfg(unix)]
async fn request_agent_list_async() -> std::io::Result<Vec<AgentIdentity>> {
    use tokio::net::UnixStream;

    let socket_path = wardian_core::control::socket_path()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control socket"))?;
    let mut stream = UnixStream::connect(socket_path).await?;
    request_agent_list(&mut stream).await
}

async fn request_agent_list<T>(stream: &mut T) -> std::io::Result<Vec<AgentIdentity>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let request = serde_json::to_string(&ControlRequest::AgentList)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut line).await?;
    let response: AgentListResponse =
        serde_json::from_str(&line).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(response.agents)
}
