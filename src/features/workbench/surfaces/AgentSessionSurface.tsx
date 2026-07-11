import { useCallback, useEffect, useState } from "react";

import type {
  AgentConfig,
  TerminalBrokerState,
  TerminalPresentationState,
  TerminalRenderState,
  TerminalRequestedInteraction,
  TerminalVisibility,
} from "../../../types";
import { AgentTerminal } from "../../terminal/AgentTerminal";

export interface AgentSessionSurfaceProps {
  /** Stable workbench presentation identity. Closing it must not affect the agent runtime. */
  surface_id: string;
  /** Stable agent/session UUID stored as the workbench surface resource key. */
  resource_key: string;
  /** The shared agent resource matching `resource_key`, when it still exists. */
  agent?: AgentConfig;
  theme: "dark" | "light" | "system";
  visibility?: TerminalVisibility;
  render_state?: TerminalRenderState;
  requested_interaction?: TerminalRequestedInteraction;
  is_maximized?: boolean;
  broker_state?: TerminalBrokerState | null;
  presentation_state?: TerminalPresentationState | null;
  on_title_change?: (agent_id: string, title: string) => void;
  on_terminal_focus?: (agent_id: string) => void;
  on_refresh_agents?: () => void;
  rebind_candidates?: AgentConfig[];
  on_rebind_agent?: (agent_id: string) => void;
  on_reset_surface?: () => void;
  on_close_surface?: () => void;
}

/** One renderer identity per workbench presentation of an agent runtime. */
export function agentSessionPresentationId(surfaceId: string, agentId: string): string {
  return `${surfaceId}:agent:${agentId}`;
}

type PresentationMode = "connecting" | "owner" | "mirror";

function presentationMode(
  agentId: string,
  presentationId: string,
  brokerState: TerminalBrokerState | null | undefined,
): PresentationMode {
  if (!brokerState || brokerState.session_id !== agentId) return "connecting";
  return brokerState.owner_presentation_id === presentationId ? "owner" : "mirror";
}

/**
 * Runtime-backed Agent Session presentation.
 *
 * This component owns only presentation state. Agent lifecycle operations are
 * intentionally absent from its API, so unmounting or closing a tab can detach
 * this renderer without pausing, clearing, or terminating the shared runtime.
 */
export function AgentSessionSurface({
  surface_id,
  resource_key,
  agent,
  theme,
  visibility = "visible",
  render_state = "mounted",
  requested_interaction = "interactive",
  is_maximized,
  broker_state,
  presentation_state,
  on_title_change,
  on_terminal_focus,
  on_refresh_agents,
  rebind_candidates = [],
  on_rebind_agent,
  on_reset_surface,
  on_close_surface,
}: AgentSessionSurfaceProps) {
  const presentationId = agentSessionPresentationId(surface_id, resource_key);
  const resolvedAgent = agent?.session_id === resource_key ? agent : undefined;
  const [observedBrokerState, setObservedBrokerState] = useState(broker_state ?? null);
  const [observedPresentationState, setObservedPresentationState] = useState(
    presentation_state ?? null,
  );
  const [rebindAgentId, setRebindAgentId] = useState(rebind_candidates[0]?.session_id ?? "");
  useEffect(() => { setObservedBrokerState(broker_state ?? null); }, [broker_state]);
  useEffect(() => { setObservedPresentationState(presentation_state ?? null); }, [presentation_state]);
  useEffect(() => {
    if (rebind_candidates.some((candidate) => candidate.session_id === rebindAgentId)) return;
    setRebindAgentId(rebind_candidates[0]?.session_id ?? "");
  }, [rebindAgentId, rebind_candidates]);

  const mode = presentationMode(resource_key, presentationId, observedBrokerState);
  const resolvedPresentation = observedPresentationState?.presentation_id === presentationId
    ? observedPresentationState
    : null;
  const isReadOnly = mode === "mirror"
    || requested_interaction === "read_only"
    || resolvedPresentation?.interaction_capability === "read_only";

  const handleTitleChange = useCallback((title: string) => {
    on_title_change?.(resource_key, title);
  }, [on_title_change, resource_key]);
  const handleTerminalFocus = useCallback(() => {
    on_terminal_focus?.(resource_key);
  }, [on_terminal_focus, resource_key]);

  if (!resolvedAgent) {
    return (
      <section
        className="flex h-full min-h-0 min-w-0 items-center justify-center bg-[var(--color-wardian-bg)] p-6"
        data-missing-agent="true"
        data-resource-key={resource_key}
        data-surface-id={surface_id}
        data-testid="agent-session-surface"
      >
        <div className="max-w-md rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] p-5 text-center shadow-sm">
          <h2 className="text-base font-semibold text-primary">Agent unavailable</h2>
          <p className="mt-2 text-sm text-muted-neutral">
            This surface references agent <code>{resource_key}</code>, but that agent is no longer available.
          </p>
          {on_refresh_agents ? (
            <button
              className="mt-4 rounded border border-wardian-border px-3 py-1.5 text-sm text-primary transition-colors hover:bg-[var(--color-wardian-card-bg-muted)]"
              onClick={on_refresh_agents}
              type="button"
            >
              Refresh agents
            </button>
          ) : null}
          {on_rebind_agent && rebind_candidates.length > 0 ? (
            <div className="mt-4 flex items-center justify-center gap-2">
              <label className="text-xs text-muted-neutral">
                Rebind to
                <select
                  aria-label="Rebind Agent Session"
                  className="ml-2 rounded border border-wardian-border bg-[var(--color-wardian-card)] px-2 py-1 text-primary"
                  value={rebindAgentId}
                  onChange={(event) => setRebindAgentId(event.target.value)}
                >
                  {rebind_candidates.map((candidate) => (
                    <option key={candidate.session_id} value={candidate.session_id}>
                      {candidate.session_name}
                    </option>
                  ))}
                </select>
              </label>
              <button
                className="rounded border border-wardian-border px-3 py-1.5 text-sm text-primary transition-colors hover:bg-[var(--color-wardian-card-bg-muted)]"
                disabled={!rebindAgentId}
                onClick={() => on_rebind_agent(rebindAgentId)}
                type="button"
              >
                Rebind
              </button>
            </div>
          ) : null}
          {(on_reset_surface || on_close_surface) ? (
            <div className="mt-4 flex justify-center gap-2">
              {on_reset_surface ? (
                <button
                  className="rounded border border-wardian-border px-3 py-1.5 text-sm text-primary"
                  onClick={on_reset_surface}
                  type="button"
                >
                  Reset Surface
                </button>
              ) : null}
              {on_close_surface ? (
                <button
                  className="rounded border border-wardian-border px-3 py-1.5 text-sm text-primary"
                  onClick={on_close_surface}
                  type="button"
                >
                  Close
                </button>
              ) : null}
            </div>
          ) : null}
        </div>
      </section>
    );
  }

  return (
    <section
      className="flex h-full min-h-0 min-w-0 flex-col bg-[var(--color-wardian-bg)]"
      data-presentation-id={presentationId}
      data-presentation-mode={mode}
      data-resource-key={resource_key}
      data-surface-id={surface_id}
      data-testid="agent-session-surface"
    >
      <header className="flex min-h-9 shrink-0 items-center gap-2 border-b border-wardian-border bg-[var(--color-wardian-sidebar-secondary)] px-3 py-1.5">
        <div className="min-w-0">
          <h2 className="truncate text-sm font-semibold text-primary">{resolvedAgent.session_name}</h2>
          <p className="truncate text-[11px] text-muted-neutral">
            {resolvedAgent.agent_class}{resolvedAgent.provider ? ` · ${resolvedAgent.provider}` : ""}
          </p>
        </div>
        <div aria-label="Terminal presentation status" className="ml-auto flex shrink-0 items-center gap-1.5">
          <span
            className="rounded-full border border-wardian-border bg-[var(--color-wardian-card)] px-2 py-0.5 text-[10px] font-medium text-muted-neutral"
            data-testid="agent-session-presentation-mode"
          >
            {mode === "owner" ? "Owner" : mode === "mirror" ? "Mirror" : "Connecting"}
          </span>
          {isReadOnly ? (
            <span
              className="rounded-full border border-wardian-border bg-[var(--color-wardian-card)] px-2 py-0.5 text-[10px] font-medium text-muted-neutral"
              data-testid="agent-session-read-only"
            >
              Read only
            </span>
          ) : null}
        </div>
      </header>
      <div className="min-h-0 min-w-0 flex-1 overflow-hidden p-2">
        <AgentTerminal
          sessionId={resource_key}
          presentationId={presentationId}
          visibility={visibility}
          renderState={render_state}
          requestedInteraction={requested_interaction}
          provider={resolvedAgent.provider}
          isMaximized={is_maximized}
          theme={theme}
          workspacePath={resolvedAgent.git_worktree && resolvedAgent.git_worktree_folder?.trim()
            ? resolvedAgent.git_worktree_folder
            : resolvedAgent.folder}
          onTitleChange={handleTitleChange}
          onTerminalFocus={handleTerminalFocus}
          onPresentationStateChange={(nextBrokerState, nextPresentationState) => {
            setObservedBrokerState(nextBrokerState);
            setObservedPresentationState(nextPresentationState);
          }}
        />
      </div>
    </section>
  );
}
