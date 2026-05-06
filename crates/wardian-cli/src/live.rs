use std::{io, time::Duration};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{AgentListResponse, AgentResponse, ControlRequest};
use wardian_core::identity::AgentIdentity;
use wardian_core::models::WorkflowDefinition;

const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn list_agents() -> io::Result<Vec<AgentIdentity>> {
    let runtime = build_runtime()?;
    let value = timeout_block(&runtime, send_request(ControlRequest::AgentList))?;
    let response: AgentListResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(response.agents)
}

pub fn agent_kill(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        send_request(ControlRequest::AgentKill {
            target: target.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn agent_pause(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        send_request(ControlRequest::AgentPause {
            target: target.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn agent_resume(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        send_request(ControlRequest::AgentResume {
            target: target.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn agent_spawn(
    class: &str,
    name: Option<&str>,
    workspace: Option<&str>,
) -> io::Result<AgentIdentity> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        send_request(ControlRequest::AgentSpawn {
            class: class.to_string(),
            name: name.map(str::to_string),
            workspace: workspace.map(str::to_string),
        }),
    )?;
    let resp: AgentResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(resp.agent)
}

pub fn agent_clone(target: &str, name: Option<&str>) -> io::Result<AgentIdentity> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        send_request(ControlRequest::AgentClone {
            target: target.to_string(),
            name: name.map(str::to_string),
        }),
    )?;
    let resp: AgentResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(resp.agent)
}

pub fn workflow_run(id: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        send_request(ControlRequest::WorkflowRun { id: id.to_string() }),
    )
    .map(|_| ())
}

pub fn workflow_stop(run_instance_id: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        send_request(ControlRequest::WorkflowStop {
            run_instance_id: run_instance_id.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn send_message(target: &str, message: &str, thread: Option<&str>) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        send_request(ControlRequest::SendMessage {
            target: target.to_string(),
            message: message.to_string(),
            thread: thread.map(str::to_string),
        }),
    )
    .map(|_| ())
}

pub fn list_workflows_from_disk() -> io::Result<Vec<WorkflowDefinition>> {
    let home = wardian_core::paths::wardian_home()
        .ok_or_else(|| io::Error::other("WARDIAN_HOME not set"))?;
    let workflows_dir = home.join("workflows");
    if !workflows_dir.exists() {
        return Ok(vec![]);
    }
    let mut workflows = vec![];
    for entry in std::fs::read_dir(&workflows_dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
            let content = std::fs::read_to_string(entry.path())?;
            if let Ok(wf) = serde_json::from_str::<WorkflowDefinition>(&content) {
                workflows.push(wf);
            }
        }
    }
    Ok(workflows)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn build_runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| io::Error::other(e.to_string()))
}

fn timeout_block(
    runtime: &tokio::runtime::Runtime,
    fut: impl std::future::Future<Output = io::Result<serde_json::Value>>,
) -> io::Result<serde_json::Value> {
    match runtime.block_on(async { tokio::time::timeout(CONTROL_TIMEOUT, fut).await }) {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Wardian control endpoint timed out",
        )),
    }
}

async fn send_request(req: ControlRequest) -> io::Result<serde_json::Value> {
    #[cfg(windows)]
    {
        send_request_windows(req).await
    }
    #[cfg(unix)]
    {
        send_request_unix(req).await
    }
}

#[cfg(windows)]
async fn send_request_windows(req: ControlRequest) -> io::Result<serde_json::Value> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| io::Error::other("could not resolve Wardian control pipe"))?;
    let mut stream = ClientOptions::new().open(pipe_name)?;
    exchange_json(&mut stream, req).await
}

#[cfg(unix)]
async fn send_request_unix(req: ControlRequest) -> io::Result<serde_json::Value> {
    use tokio::net::UnixStream;

    let socket_path = wardian_core::control::socket_path()
        .ok_or_else(|| io::Error::other("could not resolve Wardian control socket"))?;
    let mut stream = UnixStream::connect(socket_path).await?;
    exchange_json(&mut stream, req).await
}

async fn exchange_json<T>(stream: &mut T, req: ControlRequest) -> io::Result<serde_json::Value>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let request =
        serde_json::to_string(&req).map_err(|e| io::Error::other(e.to_string()))?;
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut line).await?;

    // Detect backend error envelope {"error": {...}}
    let value: serde_json::Value =
        serde_json::from_str(&line).map_err(|e| io::Error::other(e.to_string()))?;
    if let Some(err) = value.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(io::Error::other(msg.to_string()));
    }

    Ok(value)
}

