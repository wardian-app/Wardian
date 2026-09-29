// New Session can take several seconds. A repeated request for the same agent
// would only queue behind the first and then fail on its conversation lease.
const newSessionsInFlight = new Set<string>();

export async function runNewSessionAction(
  clearAgent: (agentId: string) => Promise<void>,
  agentId: string,
): Promise<void> {
  if (newSessionsInFlight.has(agentId)) {
    return;
  }
  newSessionsInFlight.add(agentId);
  try {
    await clearAgent(agentId);
  } catch (error) {
    console.error(error);
    window.alert(`Failed to start a new session: ${error}`);
  } finally {
    newSessionsInFlight.delete(agentId);
  }
}
