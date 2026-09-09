import { spawn, execFile } from "node:child_process";
import { createInterface } from "node:readline";

/** A bounded test client. It never retries requests or executes server callbacks. */
export function startStdioRpc(command, args, { cwd, env, onNotification = () => {} }) {
  const child = spawn(command, args, { cwd, env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
  const pending = new Map();
  let sequence = 0;
  let stderrBytes = 0;
  let exitResult;
  let terminalError;
  let closing = false;
  let resolveExited;
  const exited = new Promise((resolve) => { resolveExited = resolve; });
  const rejectPending = (error) => {
    for (const entry of pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(error);
    }
    pending.clear();
  };
  const settleExit = (result, error) => {
    if (exitResult) return;
    exitResult = result;
    rejectPending(error);
    resolveExited();
  };
  child.once("exit", (code, signal) => {
    settleExit({ code, signal }, new Error(`RPC child exited (${code ?? signal})`));
  });
  child.on("error", (error) => {
    terminalError = error;
    rejectPending(error);
    // Failed spawn need not emit exit. An error on an existing PID (for
    // example a failed kill) does not prove that process has exited.
    if (child.pid == null) settleExit({ code: null, signal: null, spawn_error: error.code ?? "spawn_failed" }, error);
  });
  child.stdin.on("error", (error) => { terminalError = error; rejectPending(error); });
  child.stderr.on("data", (chunk) => { stderrBytes += chunk.length; });
  const lines = createInterface({ input: child.stdout });
  const write = (value) => child.stdin.write(`${JSON.stringify(value)}\n`);
  lines.on("line", (line) => {
    let value;
    try { value = JSON.parse(line); } catch { return; }
    if (value.method && value.id != null) {
      write({ jsonrpc: "2.0", id: value.id, error: { code: -32601, message: "Unexpected server request in messaging probe" } });
    } else if (value.id != null && pending.has(value.id)) {
      const entry = pending.get(value.id);
      pending.delete(value.id);
      clearTimeout(entry.timer);
      if (value.error) entry.reject(new Error(`RPC ${entry.method}: ${value.error.code}: ${value.error.message}`));
      else entry.resolve(value.result);
    } else if (value.method) onNotification(value);
  });
  return {
    notify(method, params = {}) { write({ jsonrpc: "2.0", method, params }); },
    request(method, params = {}, timeoutMs = 30_000) {
      if (terminalError) return Promise.reject(terminalError);
      if (exitResult || closing) return Promise.reject(new Error("RPC child already exited or closing"));
      const id = ++sequence;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`RPC timeout: ${method}; submission was not retried`));
        }, timeoutMs);
        pending.set(id, { resolve, reject, timer, method });
        write({ jsonrpc: "2.0", id, method, params });
      });
    },
    async close() {
      closing = true;
      const waitForExit = async () => {
        let timer;
        try {
          await Promise.race([exited, new Promise((resolve) => { timer = setTimeout(resolve, 3000); })]);
        } finally { clearTimeout(timer); }
      };
      try {
        if (!exitResult) {
          child.stdin.end();
          await waitForExit();
        }
        if (!exitResult) {
          if (process.platform === "win32") {
            await new Promise((resolve) => execFile("taskkill", ["/PID", String(child.pid), "/T", "/F"],
              { windowsHide: true, timeout: 3000 }, () => resolve()));
          } else child.kill("SIGKILL");
          await waitForExit();
        }
        if (!exitResult) {
          const error = new Error(`RPC child termination was not confirmed (pid ${child.pid})`);
          rejectPending(error);
          throw error;
        }
        return { ...exitResult, stderr_bytes: stderrBytes };
      } finally {
        lines.close();
      }
    },
  };
}
