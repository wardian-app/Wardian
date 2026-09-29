export async function runNewSessionAction(
  clearAgent: (agentId: string) => Promise<void>,
  agentId: string,
): Promise<void> {
  try {
    await clearAgent(agentId);
  } catch (error) {
    console.error(error);
    window.alert(`Failed to start a new session: ${error}`);
  }
}
