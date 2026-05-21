import { create } from "zustand";
import type { RemoteAgentSummary, RemoteWorkflowSummary } from "../../types";
import { remoteClient, RemoteRequestError } from "./remoteClient";

type RemoteStatus = "loading" | "ready" | "unreachable" | "session_expired";

interface RemoteState {
  agents: RemoteAgentSummary[];
  workflows: RemoteWorkflowSummary[];
  status: RemoteStatus;
  selectedAgentIds: Set<string>;
  sending: boolean;
  load: () => Promise<void>;
  toggleAgent: (id: string) => void;
  sendPrompt: (prompt: string) => Promise<void>;
  broadcastPrompt: (prompt: string) => Promise<void>;
  runAgentAction: (action: string, target: string) => Promise<void>;
  runWorkflow: (workflowId: string) => Promise<void>;
}

const statusFromError = (error: unknown): RemoteStatus =>
  error instanceof RemoteRequestError && error.status === 401 ? "session_expired" : "unreachable";

const sendPromptToTargets = async (prompt: string, agentIds: string[]) => {
  const results = await Promise.allSettled(agentIds.map((target) => remoteClient.sendPrompt(target, prompt)));
  const rejected = results.find((result): result is PromiseRejectedResult => result.status === "rejected");
  if (!rejected) return;
  if (rejected.reason instanceof RemoteRequestError && rejected.reason.status === 401) {
    throw rejected.reason;
  }
  const acceptedCount = results.filter((result) => result.status === "fulfilled").length;
  throw new Error(`${acceptedCount}/${agentIds.length} prompts accepted.`);
};

export const useRemoteStore = create<RemoteState>((set, get) => ({
  agents: [],
  workflows: [],
  status: "loading",
  selectedAgentIds: new Set(),
  sending: false,
  async load() {
    try {
      await remoteClient.loadSession();
      const [agents, workflows] = await Promise.all([remoteClient.listAgents(), remoteClient.listWorkflows()]);
      set((state) => {
        const liveAgentIds = new Set(agents.map((agent) => agent.session_id));
        const selectedAgentIds = new Set([...state.selectedAgentIds].filter((id) => liveAgentIds.has(id)));
        return { agents, workflows, status: "ready", selectedAgentIds };
      });
    } catch (error) {
      set({ status: statusFromError(error) });
    }
  },
  toggleAgent(id) {
    set((state) => {
      const selectedAgentIds = new Set(state.selectedAgentIds);
      if (selectedAgentIds.has(id)) selectedAgentIds.delete(id);
      else selectedAgentIds.add(id);
      return { selectedAgentIds };
    });
  },
  async sendPrompt(prompt) {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const targets = get().selectedAgentIds;
    const agentIds = [...targets];
    if (agentIds.length === 0) return;
    set({ sending: true });
    try {
      await sendPromptToTargets(trimmed, agentIds);
      await get().load();
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    } finally {
      set({ sending: false });
    }
  },
  async broadcastPrompt(prompt) {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const agentIds = get().agents.map((agent) => agent.session_id);
    if (agentIds.length === 0) return;
    set({ sending: true });
    try {
      await sendPromptToTargets(trimmed, agentIds);
      await get().load();
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    } finally {
      set({ sending: false });
    }
  },
  async runAgentAction(action, target) {
    try {
      await remoteClient.runAgentAction(action, target);
      await get().load();
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
  async runWorkflow(workflowId) {
    try {
      await remoteClient.runWorkflow(workflowId);
      await get().load();
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
}));
