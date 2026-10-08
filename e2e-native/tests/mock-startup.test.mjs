// @tier ci — Pure orchestration checks; no app, provider or compiler starts.
import test from "node:test";
import assert from "node:assert/strict";
import { waitForMockStartup } from "../lib/mock-startup.mjs";

const startedAt = "2026-01-01T00:00:01.000Z";
const init = { session_id: "agent", data: {
  type: "init", session_id: "provider", timestamp: startedAt,
} };
const lease = { agent_id: "agent", provider: "mock", resume_session: "provider",
  owner_kind: "provider_spawn", owner_id: "runtime", acquisition_id: "acquisition" };
const snapshot = (leases) => ({ schema: 1, leases });

function observedLaunch({ events, leases, timeoutMs = 1000 }) {
  let elapsed = 0;
  let reads = 0;
  return {
    options: {
      sessionId: "agent", providerSessionId: "provider", startedAt, timeoutMs,
      now: () => elapsed,
      pause: async (ms) => { elapsed += ms; reads += 1; },
      readEvents: async () => events(reads),
      readLeases: async () => leases(reads),
    },
    reads: () => reads,
  };
}

test("a matching Init remains blocked until the startup lease is released", async () => {
  const held = Object.freeze(lease);
  const launch = observedLaunch({
    events: (read) => read === 0 ? [] : [init],
    leases: (read) => snapshot(read < 2 ? [held] : []),
  });
  const result = await waitForMockStartup(launch.options);
  assert.equal(launch.reads(), 2);
  assert.equal(result.init, init);
  assert.deepEqual(result.leases.leases, []);
  assert.equal(held.owner_kind, "provider_spawn", "the observer must not delete or change ownership");
});

test("an old or foreign Init cannot establish this launch's readiness", async () => {
  const old = { ...init, data: { ...init.data, timestamp: "2026-01-01T00:00:00Z" } };
  const foreign = { ...init, data: { ...init.data, session_id: "other-provider" } };
  const otherAgent = { ...init, session_id: "other-agent" };
  const launch = observedLaunch({
    events: (read) => read < 2 ? [old, foreign, otherAgent] : [old, foreign, otherAgent, init],
    leases: () => snapshot([]),
  });
  await waitForMockStartup(launch.options);
  assert.equal(launch.reads(), 2);
});

test("an empty lease set is insufficient without this launch's Init", async () => {
  const launch = observedLaunch({ events: () => [], leases: () => snapshot([]) });
  await assert.rejects(waitForMockStartup(launch.options), /init_observed.*false/);
  assert.equal(launch.reads(), 4);
});

test("Init alone and a missing lease file do not prove completion", async () => {
  const launch = observedLaunch({ events: () => [init], leases: () => null });
  await assert.rejects(waitForMockStartup(launch.options), /lease_file_available.*false/);
  assert.equal(launch.reads(), 4, "the original finite deadline is not renewed");
});

test("resume also waits for its inherited lifecycle exclusion", async () => {
  const inherited = { ...lease, owner_kind: "agent_lifecycle" };
  const launch = observedLaunch({ events: () => [init], leases: () => snapshot([inherited]) });
  await assert.rejects(waitForMockStartup(launch.options), /agent_lifecycle/);
  assert.equal(launch.reads(), 4);
});

test("a shared provider identity blocks completion even under another agent", async () => {
  const shared = { ...lease, agent_id: "other-agent" };
  const launch = observedLaunch({ events: () => [init], leases: () => snapshot([shared]) });
  await assert.rejects(waitForMockStartup(launch.options), /other-agent/);
});

test("malformed ownership is rejected instead of becoming an empty lease set", async () => {
  const launch = observedLaunch({ events: () => [init], leases: () => ({ schema: 2, leases: [] }) });
  await assert.rejects(waitForMockStartup(launch.options), /Invalid persisted conversation lease/);
  assert.equal(launch.reads(), 0);
});
