const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Wait for this launch's caller-owned Mock Init and its persisted exclusion
 * release before a fixture changes status or exercises lifecycle commands.
 * A visible status alone can be seeded while provider startup is still active.
 * The event capture must be armed before the launch; missing ownership state
 * never proves release. This observer does not modify or retry the provider.
 */
export async function waitForMockStartup({
  sessionId,
  providerSessionId,
  startedAt,
  readEvents,
  readLeases,
  timeoutMs = 30000,
  now = () => performance.now(),
  pause = sleep,
}) {
  const started = Date.parse(startedAt);
  if (!sessionId || !providerSessionId || !Number.isFinite(started)) {
    throw new Error("Mock startup requires the agent, provider identity and launch timestamp");
  }
  const deadline = now() + timeoutMs;
  let last = null;
  while (now() < deadline) {
    const events = await readEvents();
    const file = await readLeases();
    if (file !== null && (file.schema !== 1 || !Array.isArray(file.leases)
      || file.leases.some((lease) => !lease
        || [lease.agent_id, lease.provider, lease.resume_session].some((value) =>
          typeof value !== "string" || !value.trim())))) {
      throw new Error("Invalid persisted conversation lease snapshot during Mock startup");
    }
    const init = events.find((event) => event.session_id === sessionId
      && event.data?.type === "init" && event.data.session_id === providerSessionId
      && Date.parse(event.data.timestamp) >= started);
    const pending = file?.leases.filter((lease) => lease.agent_id === sessionId
      || (lease.provider === "mock" && lease.resume_session === providerSessionId)) ?? [];
    last = { init_observed: Boolean(init), lease_file_available: file !== null, pending };
    if (init && file !== null && pending.length === 0) return { init, leases: file };
    await pause(250);
  }
  throw new Error(`Mock startup did not settle for ${sessionId}: ${JSON.stringify(last)}`);
}
