import net from "node:net";
import { spawnSync } from "node:child_process";

/** Ask the OS for a free port by binding :0, then release it for the child. */
export function reserveEphemeralPort(host = "127.0.0.1") {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.once("error", reject);
    server.listen({ host, port: 0 }, () => {
      const { port } = server.address();
      server.close((error) => (error ? reject(error) : resolve(port)));
    });
  });
}
/**
 * Resolve true when nothing is listening on the port.
 *
 * Between releasing a reserved port and the child binding it, an unrelated
 * process can claim it. Asserting the port is closed immediately before spawn
 * is what stops the harness adopting a listener it does not own: a port that is
 * already open is somebody else's, so the run fails instead of attaching to it.
 */
export function portIsFree({ port, host = "127.0.0.1", timeoutMs = 1000 }) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host, port });
    const settle = (free) => {
      socket.destroy();
      resolve(free);
    };
    socket.setTimeout(timeoutMs, () => settle(true));
    socket.once("connect", () => settle(false));
    socket.once("error", () => settle(true));
  });
}

/**
 * Pid currently listening on a local TCP port, or null when it cannot be read.
 *
 * Uses whatever the platform already provides rather than adding a service.
 */
export function listeningPid(port) {
  try {
    if (process.platform === "win32") {
      const result = spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-Command",
          `(Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction SilentlyContinue | ` +
            "Select-Object -First 1 -ExpandProperty OwningProcess)",
        ],
        { encoding: "utf8" },
      );
      const pid = Number.parseInt(String(result.stdout).trim(), 10);
      return Number.isInteger(pid) && pid > 0 ? pid : null;
    }
    const result = spawnSync("sh", ["-c", `lsof -nP -iTCP:${port} -sTCP:LISTEN -t 2>/dev/null | head -1`], {
      encoding: "utf8",
    });
    const pid = Number.parseInt(String(result.stdout).trim(), 10);
    return Number.isInteger(pid) && pid > 0 ? pid : null;
  } catch {
    return null;
  }
}

/** Walk parent links to decide whether `pid` sits under `ancestorPid`. */
export function pidIsDescendantOf(pid, ancestorPid, maxDepth = 8) {
  if (!Number.isInteger(pid) || !Number.isInteger(ancestorPid)) {
    return false;
  }
  let current = pid;
  for (let depth = 0; depth < maxDepth; depth += 1) {
    if (current === ancestorPid) {
      return true;
    }
    const parent = parentPid(current);
    if (!Number.isInteger(parent) || parent <= 0 || parent === current) {
      return false;
    }
    current = parent;
  }
  return false;
}

function parentPid(pid) {
  try {
    if (process.platform === "win32") {
      const result = spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-Command",
          `(Get-CimInstance Win32_Process -Filter "ProcessId = ${pid}" -ErrorAction SilentlyContinue | ` +
            "Select-Object -First 1 -ExpandProperty ParentProcessId)",
        ],
        { encoding: "utf8" },
      );
      const parsed = Number.parseInt(String(result.stdout).trim(), 10);
      return Number.isInteger(parsed) ? parsed : null;
    }
    const result = spawnSync("ps", ["-o", "ppid=", "-p", String(pid)], { encoding: "utf8" });
    const parsed = Number.parseInt(String(result.stdout).trim(), 10);
    return Number.isInteger(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/**
 * Prove the listener on `port` belongs to this run.
 *
 * A pre-spawn free check plus a live child only establishes that the port was
 * free at one instant and that our child has not exited. Neither says the
 * process now answering on that port is ours: anything could have bound it in
 * between. Resolving the owning pid and matching it to the driver's tree is
 * what makes the endpoint provably ours.
 *
 * When the platform cannot report an owner, this returns `unverified` rather
 * than claiming ownership it did not establish.
 */
export function assertPortOwnedBy({ port, processRef }) {
  const owner = listeningPid(port);
  if (owner === null) {
    // Fail closed. An unresolvable owner is not evidence of ownership, and
    // continuing would hand the endpoint to Selenium on the strength of a
    // check that did not complete. If a platform cannot report the listening
    // pid, teach `listeningPid` about it rather than proceeding unverified.
    throw new Error(
      `Could not determine which process is listening on port ${port}, so this run cannot prove the ` +
        `endpoint belongs to the driver it started (pid ${processRef.pid}). Refusing to continue.`,
    );
  }
  if (owner === processRef.pid || pidIsDescendantOf(owner, processRef.pid)) {
    return { verified: true, owner };
  }
  throw new Error(
    `Port ${port} is held by pid ${owner}, which is not the driver this run started ` +
      `(pid ${processRef.pid}) or one of its children. Refusing to use an endpoint this run does not own.`,
  );
}

export async function allocateSessionPorts() {
  const driverPort = await reserveEphemeralPort();
  let nativePort = await reserveEphemeralPort();
  // Two consecutive :0 reservations can hand back the same number once the
  // first is released, which would put the driver and its child on one port.
  for (let attempt = 0; attempt < 5 && nativePort === driverPort; attempt += 1) {
    nativePort = await reserveEphemeralPort();
  }
  if (nativePort === driverPort) {
    throw new Error("Could not reserve distinct driver and native driver ports.");
  }
  return { driverPort, nativePort };
}
