//! Explicit stdio MCP adapter. No home migration, provider launch, or configuration writes.
//!
//! Implements the tools-only subset of MCP 2025-11-25 and 2025-06-18. Calls are
//! serialized with a bounded receive wait, without retrying. Cancellation is
//! advisory: it never cancels a task or interrupts an agent.
//! Notifications never submit messages or revoke an already submitted delivery.

mod definitions;
mod messaging;
#[cfg(test)]
mod tests;

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::{collections::HashSet, hash::BuildHasher};

const MAX_LINE_BYTES: usize = 1024 * 1024;
const VERSION: &str = "2025-11-25";

#[derive(Debug, clap::Subcommand)]
pub enum McpCommand {
    /// Serve six agent messaging tools on stdio. Requires a managed sender and running Wardian app.
    Serve,
}

/// Run the explicit server without passing protocol output through CLI rendering.
pub fn run(_command: &McpCommand) -> i32 {
    match serve(
        io::stdin().lock(),
        io::stdout().lock(),
        &mut messaging::Live,
    ) {
        Ok(()) => 0,
        Err(_) => {
            eprintln!("Wardian MCP stdio transport failed; delivery may be uncertain. Do not replay automatically.");
            1
        }
    }
}

/// Read at most one MiB per frame; oversized or truncated frames never execute.
/// Closing stdin ends the server after any bounded in-flight control exchange.
fn serve<R: BufRead, W: Write, B: messaging::Backend>(
    mut input: R,
    mut output: W,
    backend: &mut B,
) -> io::Result<()> {
    let mut session = Session::default();
    loop {
        let mut line = Vec::new();
        let mut limited = input.by_ref().take((MAX_LINE_BYTES + 1) as u64);
        use std::io::Read;
        let length = limited.read_until(b'\n', &mut line)?;
        if length == 0 {
            return Ok(());
        }
        if length > MAX_LINE_BYTES {
            write_response(&mut output, error(Value::Null, -32600, "Message too large"))?;
            return Ok(()); // Fail closed instead of interpreting the frame's remainder.
        }
        if line.last() != Some(&b'\n') {
            write_response(&mut output, error(Value::Null, -32700, "Truncated message"))?;
            return Ok(());
        }
        let response = match serde_json::from_slice(&line) {
            Ok(value) => session.handle(value, backend),
            Err(_) => Some(error(Value::Null, -32700, "Parse error")),
        };
        if let Some(response) = response {
            write_response(&mut output, response)?;
        }
    }
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
    fn handle(&mut self, value: Value, backend: &mut impl messaging::Backend) -> Option<Value> {
        let id = value.get("id").cloned();
        let valid_id = id
            .as_ref()
            .is_none_or(|id| id.is_string() || id.is_i64() || id.is_u64());
        if !value.is_object() || value["jsonrpc"] != "2.0" || !valid_id {
            return Some(error(Value::Null, -32600, "Invalid request"));
        }
        let Some(method) = value["method"].as_str() else {
            // There are no server-to-client requests; unsolicited responses are ignored.
            if id.is_some() && (value.get("result").is_some() || value.get("error").is_some()) {
                return None;
            }
            return Some(error(id.unwrap_or(Value::Null), -32600, "Invalid request"));
        };
        let Some(id) = id else {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            return None;
        };
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(error(id, -32602, "Invalid params"));
        }
        let result = match method {
            "ping" => json!({}),
            "initialize" => {
                if self.initialized {
                    return Some(error(id, -32600, "Already initialized"));
                }
                let Some(version) = params["protocolVersion"].as_str() else {
                    return Some(error(id, -32602, "Missing protocolVersion"));
                };
                if !params["capabilities"].is_object()
                    || !params["clientInfo"]["name"].is_string()
                    || !params["clientInfo"]["version"].is_string()
                {
                    return Some(error(id, -32602, "Invalid initialization params"));
                }
                self.initialized = true;
                json!({
                    "protocolVersion": if version == "2025-06-18" { version } else { VERSION },
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "wardian", "version": env!("CARGO_PKG_VERSION")}
                })
            }
            "tools/list" | "tools/call" if !self.ready => {
                return Some(error(id, -32000, "Server not initialized"));
            }
            "tools/list" => {
                if params.as_object().unwrap().keys().any(|key| key != "_meta") {
                    return Some(error(
                        id,
                        -32602,
                        "Invalid tools/list params; no pagination",
                    ));
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
                    return Some(error(
                        id,
                        -32602,
                        "Unknown tool or invalid tools/call params",
                    ));
                }
                let call_id = id.to_string(); // String and numeric IDs remain distinct.
                if self.calls.contains(&call_id) {
                    return Some(error(id, -32600, "Call ID reused; no operation was dispatched. Do not replay uncertain delivery"));
                }
                // The broker bounds the complete idempotency key to 256 bytes.
                if self.admission_namespace.len() + 1 + call_id.len() > 256 {
                    return Some(error(
                        id,
                        -32600,
                        "Call ID is too long; no operation was dispatched",
                    ));
                }
                // Keep IDs for the connection's lifetime, including failed calls.
                // No response cache or eviction can turn uncertainty into a replay.
                self.calls.insert(call_id.clone());
                let result = messaging::call(
                    params["name"].as_str().unwrap(),
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    &format!("{}:{call_id}", self.admission_namespace),
                    backend,
                );
                let response = json!({"jsonrpc":"2.0","id":id,"result":result});
                return Some(response);
            }
            _ => return Some(error(id, -32601, "Method not found")),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
