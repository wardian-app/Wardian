//! Explicit stdio MCP adapter. No home migration, provider launch, or configuration writes.
//!
//! Implements the tools-only subset of MCP 2025-11-25 and 2025-06-18. Tool
//! admission is bounded and calls run concurrently. Cancellation is advisory
//! and never interrupts admitted work.
//! Notifications never submit messages or revoke an already submitted delivery.

mod definitions;
mod messaging;
#[cfg(test)]
mod tests;

use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::{
    collections::HashSet,
    hash::BuildHasher,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_LINE_BYTES: usize = 1024 * 1024;
const VERSION: &str = "2025-11-25";
// Live MCP calls have a 61s transport timeout; this capacity drains in at most two waves.
const TOOL_WORKERS: usize = 4;
const TOOL_QUEUE: usize = 1;
// Buffer the full reservation budget so worker scheduling cannot look like overload.
const TOOL_WORK_CAPACITY: usize = TOOL_WORKERS + TOOL_QUEUE;
const OUTPUT_QUEUE: usize = TOOL_WORK_CAPACITY;
const INPUT_POLL: Duration = Duration::from_millis(20);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The explicit MCP server command shared by both CLI executables.
#[derive(Debug, clap::Subcommand)]
pub enum McpCommand {
    /// Serve Wardian messaging tools on stdio. Requires a managed sender and running Wardian app.
    Serve,
}

/// Run the explicit server without passing protocol output through CLI rendering.
pub fn run(_command: &McpCommand) -> i32 {
    match serve_concurrent(io::stdin(), io::stdout(), || messaging::Live) {
        Ok(()) => 0,
        Err(failure) => {
            eprintln!("Wardian MCP stdio transport failed ({:?}); delivery may be uncertain. Do not replay automatically.", failure.error.kind());
            if !failure.reader_joined || !failure.writer_joined {
                // Broken stdio handles cannot interrupt their blocked worker threads.
                // All admitted tool workers are joined before this bounded exit.
                std::process::exit(1);
            }
            1
        }
    }
}

struct ServeFailure {
    error: io::Error,
    reader_joined: bool,
    writer_joined: bool,
}

enum InputEvent {
    Frame(Vec<u8>),
    TooLarge,
    Truncated,
    Eof,
    Failed(io::Error),
}

struct AdmittedCall {
    id: Value,
    name: String,
    arguments: Value,
    idempotency_key: String,
}

// Counts executing and queued calls; channel occupancy alone depends on scheduling.
#[derive(Clone, Default)]
struct WorkBudget(Arc<std::sync::atomic::AtomicUsize>);

impl WorkBudget {
    fn try_reserve(&self) -> Option<WorkReservation> {
        let mut reserved = self.0.load(Ordering::Acquire);
        loop {
            if reserved >= TOOL_WORK_CAPACITY {
                return None;
            }
            match self.0.compare_exchange_weak(
                reserved,
                reserved + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(WorkReservation(Arc::clone(&self.0))),
                Err(current) => reserved = current,
            }
        }
    }
}

struct WorkReservation(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for WorkReservation {
    fn drop(&mut self) {
        let previous = self.0.fetch_sub(1, Ordering::Release);
        debug_assert!(previous > 0, "work reservation released more than once");
    }
}

struct ReservedCall {
    call: AdmittedCall,
    reservation: WorkReservation,
}

impl AdmittedCall {
    fn execute(self, backend: &mut impl messaging::Backend) -> Value {
        let result = messaging::call(&self.name, self.arguments, &self.idempotency_key, backend);
        json!({"jsonrpc":"2.0","id":self.id,"result":result})
    }

    fn queue_full(self) -> Value {
        error(
            self.id,
            -32002,
            "Server busy; no operation was executed. Retry with a new request ID.",
        )
    }

    fn unavailable(self) -> Value {
        error(
            self.id,
            -32603,
            "Server worker unavailable; no operation was executed.",
        )
    }
}

/// Run the framed server with bounded workers and output buffering.
/// A full work queue rejects before dispatch and consumes the request ID.
/// Concurrent operations may complete out of order; callers serialize dependent
/// mutations by awaiting each response before submitting the next one.
fn serve_concurrent<R, W, B, F>(input: R, output: W, create_backend: F) -> Result<(), ServeFailure>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
    B: messaging::Backend,
    F: Fn() -> B + Send + Sync + 'static,
{
    serve_concurrent_with_worker_start(input, output, create_backend, || {})
}

// Tests hold workers here to exercise admission before any dequeue can occur.
fn serve_concurrent_with_worker_start<R, W, B, F, H>(
    input: R,
    output: W,
    create_backend: F,
    worker_start: H,
) -> Result<(), ServeFailure>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
    B: messaging::Backend,
    F: Fn() -> B + Send + Sync + 'static,
    H: Fn() + Send + Sync + 'static,
{
    let output_failed = Arc::new(AtomicBool::new(false));
    let (output_tx, output_rx) = mpsc::sync_channel::<Value>(OUTPUT_QUEUE);
    let writer_failed = Arc::clone(&output_failed);
    let writer = thread::spawn(move || {
        let mut output = output;
        for response in output_rx {
            if let Err(error) = write_response(&mut output, response) {
                writer_failed.store(true, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    });

    let (input_tx, input_rx) = mpsc::sync_channel(1);
    let reader_done = Arc::new(AtomicBool::new(false));
    let reader_finished = Arc::clone(&reader_done);
    let reader = thread::spawn(move || {
        let mut input = BufReader::new(input);
        loop {
            let mut line = Vec::new();
            let length = match input
                .by_ref()
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
            {
                Ok(length) => length,
                Err(error) => {
                    let _ = input_tx.send(InputEvent::Failed(error));
                    break;
                }
            };
            let event = if length == 0 {
                InputEvent::Eof
            } else if length > MAX_LINE_BYTES {
                InputEvent::TooLarge
            } else if line.last() != Some(&b'\n') {
                InputEvent::Truncated
            } else {
                InputEvent::Frame(line)
            };
            let terminal = !matches!(event, InputEvent::Frame(_));
            if input_tx.send(event).is_err() || terminal {
                break;
            }
        }
        reader_finished.store(true, Ordering::Release);
    });

    let (work_tx, work_rx) = mpsc::sync_channel(TOOL_WORK_CAPACITY);
    let work_budget = WorkBudget::default();
    let workers = spawn_worker_pool::<B, F, H>(
        work_rx,
        TOOL_WORKERS,
        Arc::new(create_backend),
        Arc::new(worker_start),
        output_tx.clone(),
        Arc::clone(&output_failed),
    );

    let mut session = Session::default();
    let mut primary_error = None;
    let mut input_terminal = false;
    loop {
        if output_failed.load(Ordering::Acquire) {
            primary_error = Some(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "MCP output failed",
            ));
            break;
        }
        match input_rx.recv_timeout(INPUT_POLL) {
            Ok(InputEvent::Frame(line)) => {
                if output_failed.load(Ordering::Acquire) {
                    primary_error = Some(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "MCP output failed",
                    ));
                    break;
                }
                let action = match serde_json::from_slice(&line) {
                    Ok(value) => session.admit(value),
                    Err(_) => Admission::Immediate(Some(error(Value::Null, -32700, "Parse error"))),
                };
                match action {
                    Admission::Immediate(Some(response)) => {
                        if let Err(error) = enqueue_response(&output_tx, response, &output_failed) {
                            primary_error = Some(error);
                            break;
                        }
                    }
                    Admission::Immediate(None) => {}
                    Admission::Call(call) => {
                        let Some(reservation) = work_budget.try_reserve() else {
                            if let Err(error) =
                                enqueue_response(&output_tx, call.queue_full(), &output_failed)
                            {
                                primary_error = Some(error);
                                break;
                            }
                            continue;
                        };
                        let queued = ReservedCall { call, reservation };
                        match work_tx.try_send(queued) {
                            Ok(()) => {}
                            Err(TrySendError::Full(ReservedCall { call, reservation })) => {
                                let response = call.queue_full();
                                let result = enqueue_response(&output_tx, response, &output_failed);
                                drop(reservation);
                                if let Err(error) = result {
                                    primary_error = Some(error);
                                    break;
                                }
                            }
                            Err(TrySendError::Disconnected(ReservedCall { call, reservation })) => {
                                let _ = enqueue_response(
                                    &output_tx,
                                    call.unavailable(),
                                    &output_failed,
                                );
                                drop(reservation);
                                primary_error =
                                    Some(io::Error::other("MCP worker queue disconnected"));
                                break;
                            }
                        }
                    }
                }
            }
            Ok(InputEvent::TooLarge) => {
                let _ = enqueue_response(
                    &output_tx,
                    error(Value::Null, -32600, "Message too large"),
                    &output_failed,
                );
                input_terminal = true;
                break;
            }
            Ok(InputEvent::Truncated) => {
                let _ = enqueue_response(
                    &output_tx,
                    error(Value::Null, -32700, "Truncated message"),
                    &output_failed,
                );
                input_terminal = true;
                break;
            }
            Ok(InputEvent::Eof) => {
                input_terminal = true;
                break;
            }
            Ok(InputEvent::Failed(error)) => {
                input_terminal = true;
                primary_error = Some(error);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                input_terminal = true;
                primary_error = Some(io::Error::other("MCP input reader stopped"));
                break;
            }
        }
    }

    drop(input_rx);
    if primary_error.is_some() {
        for _ in 0..5 {
            if reader_done.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    drop(work_tx);
    for worker in workers {
        if worker.join().is_err() && primary_error.is_none() {
            primary_error = Some(io::Error::other("MCP tool worker panicked"));
        }
    }
    drop(output_tx);
    let writer_deadline = Instant::now() + OUTPUT_DRAIN_TIMEOUT;
    while !writer.is_finished() && Instant::now() < writer_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let writer_joined = if writer.is_finished() {
        match writer.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) if primary_error.is_none() => primary_error = Some(error),
            Err(_) if primary_error.is_none() => {
                primary_error = Some(io::Error::other("MCP output worker panicked"));
            }
            _ => {}
        }
        true
    } else {
        if primary_error.is_none() {
            primary_error = Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP output did not drain before its deadline",
            ));
        }
        false
    };

    let reader_joined = if input_terminal || reader_done.load(Ordering::Acquire) {
        reader.join().is_ok()
    } else {
        false
    };
    match primary_error {
        Some(error) => Err(ServeFailure {
            error,
            reader_joined,
            writer_joined,
        }),
        None if reader_joined && writer_joined => Ok(()),
        None => Err(ServeFailure {
            error: io::Error::other("MCP stdio worker did not terminate"),
            reader_joined,
            writer_joined,
        }),
    }
}

fn enqueue_response(
    output: &SyncSender<Value>,
    response: Value,
    output_failed: &AtomicBool,
) -> io::Result<()> {
    match output.try_send(response) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            output_failed.store(true, Ordering::Release);
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "MCP output queue is full",
            ))
        }
        Err(TrySendError::Disconnected(_)) => {
            output_failed.store(true, Ordering::Release);
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "MCP output worker stopped",
            ))
        }
    }
}

fn spawn_worker_pool<B, F, H>(
    receiver: Receiver<ReservedCall>,
    count: usize,
    create_backend: Arc<F>,
    worker_start: Arc<H>,
    output: SyncSender<Value>,
    output_failed: Arc<AtomicBool>,
) -> Vec<JoinHandle<()>>
where
    B: messaging::Backend,
    F: Fn() -> B + Send + Sync + 'static,
    H: Fn() + Send + Sync + 'static,
{
    let receiver = Arc::new(Mutex::new(receiver));
    (0..count)
        .map(|_| {
            let receiver = Arc::clone(&receiver);
            let create_backend = Arc::clone(&create_backend);
            let worker_start = Arc::clone(&worker_start);
            let output = output.clone();
            let output_failed = Arc::clone(&output_failed);
            thread::spawn(move || {
                worker_start();
                loop {
                    let call = match receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => return,
                    };
                    let Ok(queued) = call else { return };
                    let ReservedCall { call, reservation } = queued;
                    let response = call.execute(&mut create_backend());
                    // Complete admitted work even when its response cannot be queued.
                    let _ = enqueue_response(&output, response, &output_failed);
                    drop(reservation);
                }
            })
        })
        .collect()
}

fn write_response(output: &mut impl Write, value: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &value)?;
    output.write_all(b"\n")?;
    output.flush()
}

struct Session {
    initialized: bool,
    ready: bool,
    admission_namespace: String,
    calls: HashSet<String>,
}

impl Default for Session {
    fn default() -> Self {
        // RandomState obtains independently randomized hash keys from the OS.
        // Two independent values avoid dependence on PID/time for session identity.
        let random = || std::collections::hash_map::RandomState::new().hash_one("wardian-mcp");
        Self {
            initialized: false,
            ready: false,
            admission_namespace: format!("mcp-{:016x}{:016x}", random(), random()),
            calls: HashSet::new(),
        }
    }
}

impl Session {
    fn admit(&mut self, value: Value) -> Admission {
        let id = value.get("id").cloned();
        let valid_id = id
            .as_ref()
            .is_none_or(|id| id.is_string() || id.is_i64() || id.is_u64());
        if !value.is_object() || value["jsonrpc"] != "2.0" || !valid_id {
            return Admission::Immediate(Some(error(Value::Null, -32600, "Invalid request")));
        }
        let Some(method) = value["method"].as_str() else {
            // There are no server-to-client requests; unsolicited responses are ignored.
            if id.is_some() && (value.get("result").is_some() || value.get("error").is_some()) {
                return Admission::Immediate(None);
            }
            return Admission::Immediate(Some(error(
                id.unwrap_or(Value::Null),
                -32600,
                "Invalid request",
            )));
        };
        let Some(id) = id else {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            return Admission::Immediate(None);
        };
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Admission::Immediate(Some(error(id, -32602, "Invalid params")));
        }
        let result = match method {
            "ping" => json!({}),
            "initialize" => {
                if self.initialized {
                    return Admission::Immediate(Some(error(id, -32600, "Already initialized")));
                }
                let Some(version) = params["protocolVersion"].as_str() else {
                    return Admission::Immediate(Some(error(
                        id,
                        -32602,
                        "Missing protocolVersion",
                    )));
                };
                if !params["capabilities"].is_object()
                    || !params["clientInfo"]["name"].is_string()
                    || !params["clientInfo"]["version"].is_string()
                {
                    return Admission::Immediate(Some(error(
                        id,
                        -32602,
                        "Invalid initialization params",
                    )));
                }
                self.initialized = true;
                json!({
                    "protocolVersion": if version == "2025-06-18" { version } else { VERSION },
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "wardian", "version": env!("CARGO_PKG_VERSION")}
                })
            }
            "tools/list" | "tools/call" if !self.ready => {
                return Admission::Immediate(Some(error(id, -32000, "Server not initialized")));
            }
            "tools/list" => {
                if params.as_object().unwrap().keys().any(|key| key != "_meta") {
                    return Admission::Immediate(Some(error(
                        id,
                        -32602,
                        "Invalid tools/list params; no pagination",
                    )));
                }
                json!({"tools": definitions::definitions()})
            }
            "tools/call" => {
                if !params["name"]
                    .as_str()
                    .is_some_and(|name| definitions::NAMES.contains(&name))
                    || params
                        .as_object()
                        .unwrap()
                        .keys()
                        .any(|key| !matches!(key.as_str(), "name" | "arguments" | "_meta"))
                    || params
                        .get("arguments")
                        .is_some_and(|value| !value.is_object())
                {
                    return Admission::Immediate(Some(error(
                        id,
                        -32602,
                        "Unknown tool or invalid tools/call params",
                    )));
                }
                let call_id = id.to_string(); // String and numeric IDs remain distinct.
                if self.calls.contains(&call_id) {
                    return Admission::Immediate(Some(error(id, -32600, "Call ID reused; no operation was dispatched. Do not replay uncertain delivery")));
                }
                // The broker bounds the complete idempotency key to 256 bytes.
                if self.admission_namespace.len() + 1 + call_id.len() > 256 {
                    return Admission::Immediate(Some(error(
                        id,
                        -32600,
                        "Call ID is too long; no operation was dispatched",
                    )));
                }
                // Keep IDs for the connection's lifetime, including failed calls.
                // No response cache or eviction can turn uncertainty into a replay.
                self.calls.insert(call_id.clone());
                return Admission::Call(AdmittedCall {
                    id,
                    name: params["name"].as_str().unwrap().to_string(),
                    arguments: params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    idempotency_key: format!("{}:{call_id}", self.admission_namespace),
                });
            }
            _ => return Admission::Immediate(Some(error(id, -32601, "Method not found"))),
        };
        Admission::Immediate(Some(json!({"jsonrpc": "2.0", "id": id, "result": result})))
    }
}

enum Admission {
    Immediate(Option<Value>),
    Call(AdmittedCall),
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
