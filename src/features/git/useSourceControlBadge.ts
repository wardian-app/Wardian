import type { AgentConfig } from "../../types";
import { useSelectedAgentGitStatus } from "./useSelectedAgentGitStatus";

interface SourceControlBadgeState {
  changeCount: number;
}

export function useSourceControlBadge(
  selectedAgentIds: Set<string>,
  agents: AgentConfig[],
): SourceControlBadgeState {
  const { changeCount } = useSelectedAgentGitStatus(selectedAgentIds, agents);
  return { changeCount };
}
