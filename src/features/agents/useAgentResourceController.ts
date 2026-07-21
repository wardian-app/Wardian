import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentConfig,
  AgentJsonEvent,
  AgentStatusUpdate,
  AgentTelemetry,
  AppTelemetry,
  CloneMode,
} from "../../types";
import { useSettingsStore } from "../../store/useSettingsStore";
import { classifyJsonEvent } from "../../utils/statusUtils";
import { normalizeAgentConfig, normalizeAgentConfigs } from "./configUtils";

export type AgentStatusTransitionSource = "metrics" | "status_event";

export type AgentStatusTransition = {
  session_id: string;
  current_status: string;
  previous_status: string | undefined;
  source: AgentStatusTransitionSource;
  agent: AgentConfig | undefined;
};

export type AgentResourceControllerOptions = {
  /** Receives the provider event after the resource controller updates its thought projection. */
  on_agent_json_event?: (session_id: string, data: Record<string, unknown>) => void;
  /** Receives status transitions so queue policy can remain outside this resource owner. */
  on_agent_status_transition?: (transition: AgentStatusTransition) => void;
  /** Receives one coalesced interaction update per telemetry payload. */
  on_agent_interactions?: (updates: Readonly<Record<string, string>>) => void;
  on_error?: (operation: string, error: unknown) => void;
  now?: () => string;
};

export type AgentResourceController = {
  agents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  app_telemetry: AppTelemetry;
  terminal_titles: Record<string, string>;
  current_thoughts: Record<string, string>;
  agent_statuses: Record<string, string>;
  off_agent_ids: Set<string>;
  refresh_agents: (spawned_agent?: AgentConfig) => Promise<readonly AgentConfig[]>;
  set_terminal_title: (session_id: string, title: string) => void;
  rename_agent: (session_id: string, new_name: string) => Promise<void>;
  pause_agent: (session_id: string) => Promise<void>;
  resume_agent: (session_id: string) => Promise<void>;
  clear_agent: (session_id: string) => Promise<void>;
  clone_agent: (
    session_id: string,
    mode: Exclude<CloneMode, "custom">,
  ) => Promise<AgentConfig>;
  delete_agents: (session_ids: readonly string[]) => Promise<readonly string[]>;
  reorder_agents: (session_ids: readonly string[]) => Promise<void>;
};

const EMPTY_APP_TELEMETRY: AppTelemetry = { cpu_usage: 0, memory_mb: 0 };

function makeStatusTelemetry(
  session_id: string,
  current_status: string,
  previous: AgentTelemetry | undefined,
): AgentTelemetry {
  return {
    session_id,
    cpu_usage: previous?.cpu_usage ?? 0,
    memory_mb: previous?.memory_mb ?? 0,
    uptime_seconds: previous?.uptime_seconds ?? 0,
    query_count: previous?.query_count ?? 0,
    init_timestamp: previous?.init_timestamp ?? null,
    current_status,
    log_path: previous?.log_path ?? null,
  };
}

/**
 * Owns the single desktop subscription and load path for shared agent resources.
 * Inbox, watchlist, confirmation, and interaction persistence policies enter only
 * through callbacks or returned operation results; they are not mirrored here.
 */
export function useAgentResourceController(
  options: AgentResourceControllerOptions = {},
): AgentResourceController {
  const options_ref = useRef(options);
  options_ref.current = options;

  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const agents_ref = useRef(agents);
  const [telemetry, setTelemetry] = useState<Record<string, AgentTelemetry>>({});
  const telemetry_ref = useRef(telemetry);
  const [appTelemetry, setAppTelemetry] = useState<AppTelemetry>(EMPTY_APP_TELEMETRY);
  const [terminalTitles, setTerminalTitles] = useState<Record<string, string>>({});
  const [currentThoughts, setCurrentThoughts] = useState<Record<string, string>>({});
  const [offAgentIds, setOffAgentIds] = useState<Set<string>>(new Set());
  const agent_status_ref = useRef<Record<string, string>>({});
  const fetch_request_ref = useRef(0);
  const mounted_ref = useRef(true);

  const reportError = useCallback((operation: string, error: unknown) => {
    const handler = options_ref.current.on_error;
    if (handler) {
      handler(operation, error);
      return;
    }
    console.error(`Agent resource operation ${operation} failed:`, error);
  }, []);

  const commitAgents = useCallback((next_agents: AgentConfig[]) => {
    agents_ref.current = next_agents;
    setAgents(next_agents);
    setOffAgentIds(new Set(
      next_agents
        .filter((agent) => agent.is_off)
        .map((agent) => agent.session_id),
    ));
  }, []);

  const refreshAgents = useCallback(async (
    spawned_agent?: AgentConfig,
  ): Promise<readonly AgentConfig[]> => {
    const request_id = ++fetch_request_ref.current;
    try {
      const listed_agents = await invoke<AgentConfig[]>("list_agents");
      if (!mounted_ref.current || request_id !== fetch_request_ref.current) {
        return agents_ref.current;
      }

      const normalized = normalizeAgentConfigs(listed_agents);
      const spawned_agent_id = spawned_agent?.session_id;
      const should_place_new_agent = Boolean(spawned_agent_id);
      const new_agent_position = useSettingsStore.getState().watchlistNewAgentPosition;
      const next_agents = should_place_new_agent
        ? new_agent_position === "bottom"
          ? [
              ...normalized.filter((agent) => agent.session_id !== spawned_agent_id),
              ...normalized.filter((agent) => agent.session_id === spawned_agent_id),
            ]
          : [
              ...normalized.filter((agent) => agent.session_id === spawned_agent_id),
              ...normalized.filter((agent) => agent.session_id !== spawned_agent_id),
            ]
        : normalized;

      commitAgents(next_agents);

      const order_changed = next_agents.some(
        (agent, index) => agent.session_id !== normalized[index]?.session_id,
      );
      if (
        should_place_new_agent
        && order_changed
        && next_agents.some((agent) => agent.session_id === spawned_agent_id)
      ) {
        try {
          await invoke("reorder_agents", {
            sessionIds: next_agents.map((agent) => agent.session_id),
          });
        } catch (error) {
          reportError("reorder_spawned_agent", error);
        }
      }
      return next_agents;
    } catch (error) {
      if (request_id === fetch_request_ref.current) {
        reportError("list_agents", error);
      }
      return agents_ref.current;
    }
  }, [commitAgents, reportError]);

  const applyStatus = useCallback((
    session_id: string,
    current_status: string,
    source: AgentStatusTransitionSource,
    notify_initial: boolean,
  ) => {
    const previous_status = agent_status_ref.current[session_id];
    if (notify_initial || previous_status !== undefined) {
      options_ref.current.on_agent_status_transition?.({
        session_id,
        current_status,
        previous_status,
        source,
        agent: agents_ref.current.find((agent) => agent.session_id === session_id),
      });
    }
    agent_status_ref.current[session_id] = current_status;
  }, []);

  useEffect(() => {
    mounted_ref.current = true;
    void refreshAgents();

    const subscriptions = [
      listen<AgentJsonEvent>("agent-json-event", (event) => {
        const { session_id, data } = event.payload;
        const json_data = data as Record<string, unknown>;
        options_ref.current.on_agent_json_event?.(session_id, json_data);
        const effect = classifyJsonEvent(json_data);
        if (effect.type === "progress") {
          setCurrentThoughts((previous) => ({
            ...previous,
            [session_id]: effect.thought,
          }));
        } else if (effect.type === "clear_thought") {
          setCurrentThoughts((previous) => ({ ...previous, [session_id]: "" }));
        }
      }),
      listen("agents-updated", () => {
        void refreshAgents();
      }),
      listen<AgentTelemetry[]>("agent-metrics", (event) => {
        const previous_telemetry = telemetry_ref.current;
        const next_telemetry = { ...previous_telemetry };
        const interaction_updates: Record<string, string> = {};
        for (const metric of event.payload) {
          applyStatus(metric.session_id, metric.current_status, "metrics", false);

          const previous_metric = previous_telemetry[metric.session_id];
          const previous_query_count = previous_metric?.query_count ?? 0;
          const current_query_count = metric.query_count ?? 0;
          const is_transcript_hydration = Boolean(
            previous_metric
            && previous_query_count === 0
            && current_query_count > 0
            && metric.current_status === "Idle"
            && metric.log_path,
          );
          if (
            previous_metric
            && current_query_count > previous_query_count
            && !is_transcript_hydration
          ) {
            const occurred_at = options_ref.current.now?.() ?? new Date().toISOString();
            interaction_updates[metric.session_id] = occurred_at;
          }
          next_telemetry[metric.session_id] = metric;
        }
        if (Object.keys(interaction_updates).length > 0) {
          options_ref.current.on_agent_interactions?.(interaction_updates);
        }
        telemetry_ref.current = next_telemetry;
        setTelemetry(next_telemetry);
      }),
      listen<AppTelemetry>("app-metrics", (event) => {
        setAppTelemetry(event.payload);
      }),
      listen<AgentStatusUpdate>("agent-status-updated", (event) => {
        const { session_id, current_status } = event.payload;
        if (
          current_status === "Idle"
          || current_status === "Off"
          || current_status === "Action Needed"
        ) {
          setCurrentThoughts((previous) => ({ ...previous, [session_id]: "" }));
        }
        applyStatus(session_id, current_status, "status_event", true);
        const next_telemetry = {
          ...telemetry_ref.current,
          [session_id]: makeStatusTelemetry(
            session_id,
            current_status,
            telemetry_ref.current[session_id],
          ),
        };
        telemetry_ref.current = next_telemetry;
        setTelemetry(next_telemetry);
      }),
    ];

    for (const subscription of subscriptions) {
      void subscription.catch((error) => reportError("listen_agent_resources", error));
    }

    return () => {
      mounted_ref.current = false;
      fetch_request_ref.current += 1;
      for (const subscription of subscriptions) {
        void subscription.then((unlisten) => unlisten()).catch(() => undefined);
      }
    };
  }, [applyStatus, refreshAgents, reportError]);

  const setTerminalTitle = useCallback((session_id: string, title: string) => {
    setTerminalTitles((previous) => ({ ...previous, [session_id]: title }));
  }, []);

  const renameAgent = useCallback(async (session_id: string, new_name: string) => {
    await invoke("rename_agent", { sessionId: session_id, newName: new_name });
    commitAgents(agents_ref.current.map((agent) => agent.session_id === session_id
      ? { ...agent, session_name: new_name }
      : agent));
  }, [commitAgents]);

  const pauseAgent = useCallback(async (session_id: string) => {
    await invoke("pause_agent", { sessionId: session_id });
    setOffAgentIds((previous) => new Set(previous).add(session_id));
    await refreshAgents();
  }, [refreshAgents]);

  const resumeAgent = useCallback(async (session_id: string) => {
    await invoke("resume_agent", { sessionId: session_id });
    setOffAgentIds((previous) => {
      const next = new Set(previous);
      next.delete(session_id);
      return next;
    });
    await refreshAgents();
  }, [refreshAgents]);

  const clearAgent = useCallback(async (session_id: string) => {
    await invoke("clear_agent_session", { sessionId: session_id });
    setCurrentThoughts((previous) => ({ ...previous, [session_id]: "" }));
    setTerminalTitles((previous) => ({ ...previous, [session_id]: "" }));
    setOffAgentIds((previous) => {
      const next = new Set(previous);
      next.delete(session_id);
      return next;
    });
    await refreshAgents();
  }, [refreshAgents]);

  const cloneAgent = useCallback(async (
    session_id: string,
    mode: Exclude<CloneMode, "custom">,
  ) => {
    const cloned_agent = normalizeAgentConfig(await invoke<AgentConfig>("clone_agent", {
      req: {
        source_session_id: session_id,
        mode,
      },
    }));
    await refreshAgents();
    return cloned_agent;
  }, [refreshAgents]);

  const deleteAgents = useCallback(async (session_ids: readonly string[]) => {
    const deleted_ids: string[] = [];
    for (const session_id of session_ids) {
      try {
        await invoke("kill_agent", { sessionId: session_id });
        deleted_ids.push(session_id);
      } catch (error) {
        reportError("kill_agent", error);
      }
    }
    if (deleted_ids.length > 0) {
      await refreshAgents();
    }
    return deleted_ids;
  }, [refreshAgents, reportError]);

  const reorderAgents = useCallback(async (session_ids: readonly string[]) => {
    const by_id = new Map(agents_ref.current.map((agent) => [agent.session_id, agent]));
    const explicitly_ordered = session_ids.flatMap((session_id) => {
      const agent = by_id.get(session_id);
      if (!agent) return [];
      by_id.delete(session_id);
      return [agent];
    });
    const next_agents = [...explicitly_ordered, ...by_id.values()];
    commitAgents(next_agents);
    try {
      await invoke("reorder_agents", {
        sessionIds: next_agents.map((agent) => agent.session_id),
      });
    } catch (error) {
      await refreshAgents();
      throw error;
    }
  }, [commitAgents, refreshAgents]);

  const agentStatuses = useMemo(() => {
    const statuses: Record<string, string> = {};
    for (const agent of agents) {
      statuses[agent.session_id] = offAgentIds.has(agent.session_id)
        ? "Off"
        : telemetry[agent.session_id]?.current_status ?? "Idle";
    }
    for (const [session_id, metric] of Object.entries(telemetry)) {
      statuses[session_id] = offAgentIds.has(session_id) ? "Off" : metric.current_status;
    }
    return statuses;
  }, [agents, offAgentIds, telemetry]);

  return useMemo(() => ({
    agents,
    telemetry,
    app_telemetry: appTelemetry,
    terminal_titles: terminalTitles,
    current_thoughts: currentThoughts,
    agent_statuses: agentStatuses,
    off_agent_ids: offAgentIds,
    refresh_agents: refreshAgents,
    set_terminal_title: setTerminalTitle,
    rename_agent: renameAgent,
    pause_agent: pauseAgent,
    resume_agent: resumeAgent,
    clear_agent: clearAgent,
    clone_agent: cloneAgent,
    delete_agents: deleteAgents,
    reorder_agents: reorderAgents,
  }), [
    agentStatuses,
    agents,
    appTelemetry,
    clearAgent,
    cloneAgent,
    currentThoughts,
    deleteAgents,
    offAgentIds,
    pauseAgent,
    refreshAgents,
    renameAgent,
    reorderAgents,
    resumeAgent,
    telemetry,
    terminalTitles,
    setTerminalTitle,
  ]);
}
