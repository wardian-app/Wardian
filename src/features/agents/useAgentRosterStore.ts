import { create } from "zustand";

/**
 * Whether the roster this window holds can be trusted as the complete set of agents.
 *
 * - `unloaded`: no `list_agents` request has started in this session.
 * - `loading`: a request is in flight and none has succeeded since the last failure.
 * - `loaded`: the most recent completed request succeeded; `session_ids` is its full result.
 * - `failed`: the most recent completed request failed, so the roster may be stale.
 */
export type AgentRosterStatus = "unloaded" | "loading" | "loaded" | "failed";

/**
 * How authoritative the agent roster is, published for consumers that must not act on a
 * roster that has not loaded.
 *
 * `useAgentResourceController` owns the roster and keeps it in its own state for `App`.
 * This store carries only the provenance: whether the last `list_agents` call succeeded, and
 * the complete set of session ids it returned. That is the one fact a destructive consumer
 * needs and the rendered roster cannot answer. An empty `agents` array means both "nothing
 * has loaded yet" and "there are no agents", and a filtered view (a watchlist) means "these
 * are the agents shown", not "these are the agents that exist".
 *
 * A refresh of an already-loaded roster keeps `loaded`: the previous snapshot was complete
 * when it was taken and is replaced in one step when the next one succeeds. A failed
 * refresh drops to `failed`, because the roster may have changed since.
 */
export type AgentRosterState = {
  status: AgentRosterStatus;
  /** Session ids from the most recent successful `list_agents`; empty until one succeeds. */
  session_ids: readonly string[];
};

const INITIAL_STATE: AgentRosterState = { status: "unloaded", session_ids: [] };

export const useAgentRosterStore = create<AgentRosterState>(() => ({ ...INITIAL_STATE }));

/**
 * Returns the store to its unloaded state.
 *
 * The controller calls this when it mounts, so a remount never inherits a previous
 * session's roster as if it had been loaded in this one.
 */
export function resetAgentRosterStore(): void {
  useAgentRosterStore.setState({ ...INITIAL_STATE }, true);
}
