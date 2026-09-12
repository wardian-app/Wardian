// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";

import {
  allocateSessionPorts,
  assertPortOwnedBy,
  pidIsDescendantOf,
  portIsFree,
} from "../lib/sessionPorts.mjs";

function listenOnEphemeralPort() {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.listen(0, "127.0.0.1", () => resolve({ server, port: server.address().port }));
  });
}

test("each session reserves two distinct ports so concurrent runs cannot collide", async () => {
  const first = await allocateSessionPorts();
  const second = await allocateSessionPorts();

  assert.notEqual(first.driverPort, first.nativePort, "driver and native driver need separate ports");
  assert.notEqual(second.driverPort, second.nativePort);
  for (const port of [first.driverPort, first.nativePort, second.driverPort, second.nativePort]) {
    assert.ok(Number.isInteger(port) && port > 0);
  }
});

test("an occupied port is reported as unavailable rather than assumed free", async () => {
  const { server, port } = await listenOnEphemeralPort();
  try {
    assert.equal(await portIsFree({ port }), false);
  } finally {
    server.close();
  }
});

/**
 * The central correction: a pre-spawn free check plus a live child proves only
 * that the port was free at one instant and that our process has not exited.
 * Neither says the listener answering now is ours.
 */
test("a foreign native-driver listener is refused after the free-check race", async () => {
  const { server, port } = await listenOnEphemeralPort();
  try {
    // This process owns the socket, standing in for a listener that claimed
    // the native-driver port after the pre-spawn free check.
    const owned = assertPortOwnedBy({ port, processRef: { pid: process.pid } });
    assert.equal(owned.verified, true);
    assert.equal(owned.owner, process.pid);

    // A tauri-driver with a different root must refuse the same native port,
    // even though the port answers and that driver is notionally alive.
    assert.throws(
      () => assertPortOwnedBy({ port, processRef: { pid: 999_999 } }),
      /Refusing to use an endpoint this run does not own/,
    );
  } finally {
    server.close();
  }
});

// Previously this returned {verified:false} and the caller carried on, which
// let a run reach Selenium on the strength of a check that never completed.
// An unresolvable owner is not evidence of ownership, so it now fails closed.
test("an unresolvable listener owner fails the run instead of proceeding unverified", () => {
  // Nothing is listening on this port, so no owning pid can be resolved.
  assert.throws(
    () => assertPortOwnedBy({ port: 1, processRef: { pid: process.pid } }),
    /Could not determine which process is listening on port 1/,
  );
});

test("descendant matching accepts a process's own pid and rejects unrelated pids", () => {
  assert.equal(pidIsDescendantOf(process.pid, process.pid), true);
  assert.equal(pidIsDescendantOf(process.pid, 999_999), false);
  assert.equal(pidIsDescendantOf(undefined, process.pid), false);
});
