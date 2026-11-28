import { create } from "zustand";

/** Display names by session id, so Chat can name the agent behind a delivered message. */
export const useAgentNameStore = create<{ names: Readonly<Record<string, string>> }>(() => ({ names: {} }));

export function publishAgentNames(agents: readonly { session_id: string; session_name?: string | null }[]) {
  const names: Record<string, string> = {};
  for (const agent of agents) if (agent.session_name) names[agent.session_id] = agent.session_name;
  const current = useAgentNameStore.getState().names;
  const keys = Object.keys(names);
  if (keys.length === Object.keys(current).length && keys.every((key) => current[key] === names[key])) return;
  useAgentNameStore.setState({ names });
}
