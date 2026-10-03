import type { AgentRosterState } from "../agents/useAgentRosterStore";
import { authoritativeLiveKeys } from "./gardenScene";

/**
 * Keys of every entity the Garden scene may keep derived state for, or null
 * while that set is not yet known with authority.
 *
 * The layout places agents only — `GardenView` passes no automations to it —
 * so the agent roster is the one source scene positions come from, and the
 * only one that must have loaded. A derived entry under any other key (an
 * automation laid out before that changed) is never read and is pruned with
 * the rest. Any entity kind added to the layout must be added here as a
 * source, or its warm-start positions would be pruned on every pass.
 *
 * The roster is the full one from the agent resource controller, never the
 * watchlist-filtered agents the map draws: an agent outside the active list
 * still exists, and its saved position must survive the filter.
 */
export function gardenLiveKeys(roster: AgentRosterState): ReadonlySet<string> | null {
  return authoritativeLiveKeys([
    {
      kind: "agent",
      status: roster.status,
      ids: roster.session_ids,
      // `list_agents` returns the whole roster in one unpaginated answer once
      // the backend reports restoration complete, which `status` reflects.
      complete: true,
    },
  ]);
}
