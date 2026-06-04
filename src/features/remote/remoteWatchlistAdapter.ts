import type { AgentConfig, RemoteAgentSummary } from "../../types";

export function remoteAgentToWatchlistAgent(agent: RemoteAgentSummary): AgentConfig {
  return {
    session_id: agent.session_id,
    session_name: agent.session_name,
    agent_class: agent.agent_class,
    provider: agent.provider,
    folder: agent.workspace,
    is_off: agent.status.toLowerCase() === "off",
  };
}
