import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Event, EventCallback } from "@tauri-apps/api/event";
import type {
  AgentConfig,
  AgentJsonEvent,
  AgentStatusUpdate,
  AgentTelemetry,
  AppTelemetry,
} from "../../types";
import { useAgentResourceController } from "./useAgentResourceController";
import { useAgentTelemetryStore } from "./useAgentTelemetryStore";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

const alpha: AgentConfig = {
  session_id: "agent-1",
  session_name: "Alpha",
  agent_class: "Coder",
  folder: "C:/workspace",
  is_off: false,
};

const beta: AgentConfig = {
  session_id: "agent-2",
  session_name: "Beta",
  agent_class: "Reviewer",
  folder: "C:/workspace",
  is_off: true,
};

type ListenerPayloads = {
  "agent-json-event": AgentJsonEvent;
  "agents-updated": void;
  "agent-metrics": AgentTelemetry[];
  "app-metrics": AppTelemetry;
  "agent-status-updated": AgentStatusUpdate;
  "agent-turn-completed": { session_id: string };
};

let agents: AgentConfig[];
let listeners: Map<keyof ListenerPayloads, EventCallback<unknown>>;
let unlisteners: Map<keyof ListenerPayloads, ReturnType<typeof vi.fn>>;

function emit<K extends keyof ListenerPayloads>(event: K, payload: ListenerPayloads[K]) {
  const callback = listeners.get(event) as EventCallback<ListenerPayloads[K]> | undefined;
  callback?.({ event, id: 1, payload } as Event<ListenerPayloads[K]>);
}

function metric(
  session_id: string,
  current_status: string,
  query_count = 0,
  log_path: string | null = null,
  last_query_timestamp?: string,
): AgentTelemetry {
  return {
    session_id,
    cpu_usage: 1,
    memory_mb: 2,
    uptime_seconds: 3,
    query_count,
    init_timestamp: null,
    last_query_timestamp,
    current_status,
    log_path,
  };
}

beforeEach(() => {
  mockInvoke.mockReset();
  mockListen.mockReset();
  agents = [alpha, beta];
  listeners = new Map();
  unlisteners = new Map();
  mockListen.mockImplementation((event, callback) => {
    const event_name = event as keyof ListenerPayloads;
    const unlisten = vi.fn();
    listeners.set(event_name, callback as EventCallback<unknown>);
    unlisteners.set(event_name, unlisten);
    return Promise.resolve(unlisten);
  });
  mockInvoke.mockImplementation(async (command, args) => {
    const command_args = args as Record<string, unknown> | undefined;
    switch (command) {
      case "list_agents":
        return agents;
      case "rename_agent":
        agents = agents.map((agent) => agent.session_id === command_args?.sessionId
          ? { ...agent, session_name: String(command_args.newName) }
          : agent);
        return null;
      case "pause_agent":
        agents = agents.map((agent) => agent.session_id === command_args?.sessionId
          ? { ...agent, is_off: true }
          : agent);
        return null;
      case "resume_agent":
      case "clear_agent_session":
        agents = agents.map((agent) => agent.session_id === command_args?.sessionId
          ? { ...agent, is_off: false }
          : agent);
        return null;
      case "clone_agent": {
        const clone = { ...alpha, session_id: "agent-clone", session_name: "Alpha-copy" };
        agents = [clone, ...agents];
        return clone;
      }
      case "kill_agent":
        agents = agents.filter((agent) => agent.session_id !== command_args?.sessionId);
        return null;
      case "reorder_agents": {
        const order = command_args?.sessionIds as string[];
        const by_id = new Map(agents.map((agent) => [agent.session_id, agent]));
        agents = order.flatMap((id) => {
          const agent = by_id.get(id);
          return agent ? [agent] : [];
        });
        return null;
      }
      default:
        return null;
    }
  });
});

/** Telemetry, titles and thoughts live in a store now, not on the controller. */
function projections() {
  return useAgentTelemetryStore.getState();
}

describe("useAgentResourceController", () => {
  it("owns one load and subscription path across consumer rerenders", async () => {
    const on_agent_json_event = vi.fn();
    const { result, rerender, unmount } = renderHook(
      ({ callback }) => useAgentResourceController({ on_agent_json_event: callback }),
      { initialProps: { callback: on_agent_json_event } },
    );

    await waitFor(() => expect(result.current.agents).toHaveLength(2));
    expect(mockInvoke).toHaveBeenCalledWith("list_agents");
    expect(mockListen.mock.calls.map(([event]) => event)).toEqual([
      "agent-json-event",
      "agents-updated",
      "agent-metrics",
      "app-metrics",
      "agent-status-updated",
      "agent-turn-completed",
    ]);
    expect(result.current.off_agent_ids).toEqual(new Set(["agent-2"]));

    const replacement_callback = vi.fn();
    rerender({ callback: replacement_callback });
    expect(mockListen).toHaveBeenCalledTimes(6);
    act(() => emit("agent-json-event", {
      session_id: "agent-1",
      data: { type: "progress", content: "Updated callback" },
    }));
    expect(replacement_callback).toHaveBeenCalledOnce();
    expect(on_agent_json_event).not.toHaveBeenCalled();

    agents = [alpha];
    act(() => {
      emit("agents-updated", undefined);
      emit("agents-updated", undefined);
      emit("agents-updated", undefined);
    });
    await waitFor(() => expect(result.current.agents).toHaveLength(1));
    expect(mockInvoke.mock.calls.filter(([command]) => command === "list_agents")).toHaveLength(2);

    unmount();
    await waitFor(() => {
      for (const unlisten of unlisteners.values()) {
        expect(unlisten).toHaveBeenCalledOnce();
      }
    });
  });

  it("does not re-render its host when telemetry, thoughts or titles change", async () => {
    let renders = 0;
    const { result } = renderHook(() => {
      renders += 1;
      return useAgentResourceController({});
    });
    await waitFor(() => expect(result.current.agents).toHaveLength(2));
    const settled = renders;

    // This hook runs in App, which builds the whole tree in one JSX expression.
    // A five-second tick, or one line of provider output, must not cost a
    // whole-application render — the components that display these subscribe to
    // the store themselves.
    act(() => emit("agent-metrics", [{
      session_id: "agent-1",
      cpu_usage: 7,
      memory_mb: 256,
      uptime_seconds: 120,
      query_count: 3,
      init_timestamp: null,
      current_status: "Processing",
      log_path: null,
    }]));
    act(() => emit("app-metrics", { cpu_usage: 41, memory_mb: 900 }));
    act(() => emit("agent-json-event", {
      session_id: "agent-1",
      data: { type: "progress", content: "Reading files" },
    }));
    act(() => result.current.set_terminal_title("agent-1", "npm test"));

    expect(renders).toBe(settled);

    // The values still landed; they just landed somewhere App does not watch.
    expect(projections().telemetry["agent-1"].current_status).toBe("Processing");
    expect(projections().app_telemetry).toEqual({ cpu_usage: 41, memory_mb: 900 });
    expect(projections().current_thoughts["agent-1"]).toBe("Reading files");
    expect(projections().terminal_titles["agent-1"]).toBe("npm test");
  });

  it("keeps projection identity when a repeated event carries no new value", async () => {
    const { result } = renderHook(() => useAgentResourceController({}));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    const metric = {
      session_id: "agent-1",
      cpu_usage: 4,
      memory_mb: 128,
      uptime_seconds: 90,
      query_count: 2,
      init_timestamp: null,
      current_status: "Idle",
      log_path: null,
    };
    act(() => emit("agent-metrics", [metric]));
    const settled = projections().telemetry;
    const settledAgent = settled["agent-1"];
    expect(settledAgent).toBeDefined();

    // The backend re-serializes every metric on each 5s tick, so payload
    // identity always differs even when no value did. Rebuilding the map here
    // re-rendered the whole application on every tick.
    act(() => emit("agent-metrics", [{ ...metric }]));
    expect(projections().telemetry).toBe(settled);
    expect(projections().telemetry["agent-1"]).toBe(settledAgent);

    act(() => emit("agent-metrics", [{ ...metric, uptime_seconds: 95 }]));
    expect(projections().telemetry).not.toBe(settled);
    expect(projections().telemetry["agent-1"].uptime_seconds).toBe(95);

    // One agent-json-event per line of provider output; an unchanged thought
    // must not re-render.
    act(() => emit("agent-json-event", {
      session_id: "agent-1",
      data: { type: "progress", content: "Indexing files" },
    }));
    const thoughts = projections().current_thoughts;
    expect(thoughts["agent-1"]).toBe("Indexing files");
    act(() => emit("agent-json-event", {
      session_id: "agent-1",
      data: { type: "progress", content: "Indexing files" },
    }));
    expect(projections().current_thoughts).toBe(thoughts);

    const appTelemetry = projections().app_telemetry;
    act(() => emit("app-metrics", {
      cpu_usage: appTelemetry.cpu_usage,
      memory_mb: appTelemetry.memory_mb,
    }));
    expect(projections().app_telemetry).toBe(appTelemetry);
  });

  it("projects JSON thoughts, terminal titles, status events, and app telemetry", async () => {
    const on_agent_json_event = vi.fn();
    const on_agent_status_transition = vi.fn();
    const { result } = renderHook(() => useAgentResourceController({
      on_agent_json_event,
      on_agent_status_transition,
    }));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    act(() => {
      result.current.set_terminal_title("agent-1", "Running tests");
      emit("agent-json-event", {
        session_id: "agent-1",
        data: { type: "progress", content: "Indexing files" },
      });
      emit("app-metrics", { cpu_usage: 12, memory_mb: 34 });
    });

    expect(projections().terminal_titles["agent-1"]).toBe("Running tests");
    expect(projections().current_thoughts["agent-1"]).toBe("Indexing files");
    expect(projections().app_telemetry).toEqual({ cpu_usage: 12, memory_mb: 34 });
    expect(on_agent_json_event).toHaveBeenCalledWith(
      "agent-1",
      { type: "progress", content: "Indexing files" },
    );

    act(() => {
      emit("agent-status-updated", { session_id: "agent-1", current_status: "Idle" });
    });

    expect(projections().current_thoughts["agent-1"]).toBe("");
    expect(projections().telemetry["agent-1"].current_status).toBe("Idle");
    expect(on_agent_status_transition).toHaveBeenLastCalledWith({
      session_id: "agent-1",
      current_status: "Idle",
      previous_status: undefined,
      source: "status_event",
      agent: expect.objectContaining({ session_id: "agent-1", session_name: "Alpha" }),
    });
  });

  it("keeps a headless status visible for an agent configured off", async () => {
    const { result } = renderHook(() => useAgentResourceController());
    await waitFor(() => expect(result.current.agents).toHaveLength(2));
    expect(result.current.off_agent_ids.has("agent-2")).toBe(true);

    act(() => {
      emit("agent-status-updated", { session_id: "agent-2", current_status: "Headless" });
    });

    expect(projections().telemetry["agent-2"].current_status).toBe("Headless");
    expect(result.current.off_agent_ids.has("agent-2")).toBe(true);
  });

  it.each(["status_event", "metrics"] as const)("projects backend startup through %s while the roster stays Off", async (source) => {
    const { result } = renderHook(() => useAgentResourceController());
    await waitFor(() => expect(result.current.agents).toHaveLength(2));
    act(() => {
      if (source === "status_event") {
        emit("agent-status-updated", { session_id: "agent-2", current_status: "Processing..." });
      } else {
        emit("agent-metrics", [metric("agent-2", "Processing...")]);
      }
    });
    await act(async () => { await result.current.refresh_agents(); });
    expect(result.current.off_agent_ids.has("agent-2")).toBe(true);
    expect(result.current.agents.find((agent) => agent.session_id === "agent-2")?.is_off).toBe(true);
    expect(projections().telemetry["agent-2"].current_status).toBe("Processing...");
    expect(mockInvoke.mock.calls.some(([command]) => command === "resume_agent")).toBe(false);

    act(() => emit("agent-status-updated", { session_id: "agent-2", current_status: "Off" }));
    expect(projections().telemetry["agent-2"].current_status).toBe("Off");
    expect(result.current.off_agent_ids.has("agent-2")).toBe(true);
  });

  it("projects pending UI resume and rollback before the invoke settles", async () => {
    const { result } = renderHook(() => useAgentResourceController());
    await waitFor(() => expect(result.current.agents).toHaveLength(2));
    let rejectResume!: (error: Error) => void;
    const pending = new Promise<never>((_resolve, reject) => { rejectResume = reject; });
    mockInvoke.mockReturnValueOnce(pending);
    let settled = false;
    const resume = result.current.resume_agent("agent-2").catch((error: unknown) => {
      settled = true;
      return error;
    });
    act(() => emit("agent-status-updated", { session_id: "agent-2", current_status: "Processing..." }));
    expect(projections().telemetry["agent-2"].current_status).toBe("Processing...");
    expect(result.current.off_agent_ids.has("agent-2")).toBe(true);
    expect(settled).toBe(false);

    const failure = new Error("attachment failed");
    await act(async () => {
      emit("agent-status-updated", { session_id: "agent-2", current_status: "Off" });
      rejectResume(failure);
      expect(await resume).toBe(failure);
    });
    expect(projections().telemetry["agent-2"].current_status).toBe("Off");
    expect(result.current.off_agent_ids.has("agent-2")).toBe(true);
  });

  it("reports explicit provider turn completions separately from status changes", async () => {
    const on_agent_turn_completed = vi.fn();
    const { result } = renderHook(() => useAgentResourceController({ on_agent_turn_completed }));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    act(() => emit("agent-turn-completed", { session_id: "agent-1" }));

    expect(on_agent_turn_completed).toHaveBeenCalledWith({
      session_id: "agent-1",
      agent: expect.objectContaining({ session_name: "Alpha" }),
    });
  });

  it("preserves metric transition and transcript-hydration interaction semantics", async () => {
    const on_agent_status_transition = vi.fn();
    const on_agent_interactions = vi.fn();
    const { result } = renderHook(() => useAgentResourceController({
      on_agent_status_transition,
      on_agent_interactions,
      now: () => "2026-07-10T12:00:00.000Z",
    }));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    act(() => emit("agent-metrics", [metric("agent-1", "Processing", 0)]));
    expect(on_agent_status_transition).not.toHaveBeenCalled();

    act(() => emit("agent-metrics", [metric("agent-1", "Processing", 1)]));
    expect(on_agent_status_transition).toHaveBeenLastCalledWith({
      session_id: "agent-1",
      current_status: "Processing",
      previous_status: "Processing",
      source: "metrics",
      agent: expect.objectContaining({ session_id: "agent-1", session_name: "Alpha" }),
    });
    expect(on_agent_interactions).toHaveBeenCalledWith({
      "agent-1": "2026-07-10T12:00:00.000Z",
    });

    on_agent_interactions.mockClear();
    act(() => emit("agent-metrics", [metric("agent-2", "Processing", 0)]));
    act(() => emit("agent-metrics", [metric("agent-2", "Idle", 3, "agent-2.jsonl")]));
    expect(on_agent_interactions).not.toHaveBeenCalled();
  });

  it("uses the provider timestamp when telemetry is hydrated after launch", async () => {
    const on_agent_interactions = vi.fn();
    const { result } = renderHook(() => useAgentResourceController({ on_agent_interactions }));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    act(() => emit("agent-metrics", [
      metric("agent-1", "Idle", 4, "agent-1.jsonl", "2026-08-20T15:30:00.000Z"),
    ]));

    expect(useAgentTelemetryStore.getState().telemetry["agent-1"].last_query_timestamp)
      .toBe("2026-08-20T15:30:00.000Z");
    expect(on_agent_interactions).toHaveBeenCalledWith({
      "agent-1": "2026-08-20T15:30:00.000Z",
    });
  });

  it("coalesces all interaction changes from one metrics payload", async () => {
    const on_agent_interactions = vi.fn();
    const { result } = renderHook(() => useAgentResourceController({
      on_agent_interactions,
      now: () => "2026-07-10T12:00:00.000Z",
    }));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    act(() => emit("agent-metrics", [
      metric("agent-1", "Processing", 0),
      metric("agent-2", "Processing", 0),
    ]));
    act(() => emit("agent-metrics", [
      metric("agent-1", "Processing", 1),
      metric("agent-2", "Processing", 1),
    ]));

    expect(on_agent_interactions).toHaveBeenCalledTimes(1);
    expect(on_agent_interactions).toHaveBeenCalledWith({
      "agent-1": "2026-07-10T12:00:00.000Z",
      "agent-2": "2026-07-10T12:00:00.000Z",
    });
  });

  it("coalesces concurrent agent loads and performs one follow-up refresh", async () => {
    let resolve_first: ((value: AgentConfig[]) => void) | undefined;
    const first = new Promise<AgentConfig[]>((resolve) => {
      resolve_first = resolve;
    });
    mockInvoke.mockImplementationOnce(() => first).mockResolvedValueOnce([beta]);

    const { result } = renderHook(() => useAgentResourceController());
    let first_refresh!: Promise<readonly AgentConfig[]>;
    let second_refresh!: Promise<readonly AgentConfig[]>;
    act(() => {
      first_refresh = result.current.refresh_agents();
      second_refresh = result.current.refresh_agents();
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === "list_agents")).toHaveLength(1);

    await act(async () => {
      resolve_first?.([alpha]);
      await first_refresh;
      await second_refresh;
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === "list_agents")).toHaveLength(2);
    expect(result.current.agents.map((agent) => agent.session_id)).toEqual(["agent-2"]);
  });

  it("merges an authoritative agent config without reordering the shared roster", async () => {
    const { result } = renderHook(() => useAgentResourceController());
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    let resolveStaleLoad!: (value: AgentConfig[]) => void;
    const staleLoad = new Promise<AgentConfig[]>((resolve) => {
      resolveStaleLoad = resolve;
    });
    mockInvoke.mockImplementationOnce(() => staleLoad);
    let staleRefresh!: Promise<readonly AgentConfig[]>;
    act(() => {
      staleRefresh = result.current.refresh_agents();
    });

    act(() => result.current.apply_agent_config({
      ...alpha,
      provider: "codex",
      model: "gpt-5.6-luna",
      provider_config: { type: "codex", reasoning_effort: "high" },
    }));

    await act(async () => {
      resolveStaleLoad([alpha, beta]);
      await staleRefresh;
    });

    expect(result.current.agents.map((agent) => agent.session_id)).toEqual(["agent-1", "agent-2"]);
    expect(result.current.agents[0]).toEqual(expect.objectContaining({
      model: "gpt-5.6-luna",
      provider_config: { type: "codex", reasoning_effort: "high" },
    }));
  });

  it("exposes lifecycle operations without moving confirmation or roster ownership into the controller", async () => {
    const { result } = renderHook(() => useAgentResourceController());
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    await act(async () => result.current.rename_agent("agent-1", "Renamed"));
    expect(mockInvoke).toHaveBeenCalledWith("rename_agent", {
      sessionId: "agent-1",
      newName: "Renamed",
    });
    expect(result.current.agents[0].session_name).toBe("Renamed");

    await act(async () => result.current.pause_agent("agent-1"));
    expect(result.current.off_agent_ids.has("agent-1")).toBe(true);

    await act(async () => result.current.resume_agent("agent-1"));
    expect(result.current.off_agent_ids.has("agent-1")).toBe(false);

    act(() => {
      result.current.set_terminal_title("agent-1", "Old title");
      emit("agent-json-event", {
        session_id: "agent-1",
        data: { type: "progress", content: "Old thought" },
      });
    });
    await act(async () => result.current.clear_agent("agent-1"));
    expect(projections().terminal_titles["agent-1"]).toBe("");
    expect(projections().current_thoughts["agent-1"]).toBe("");

    let cloned: AgentConfig | undefined;
    await act(async () => {
      cloned = await result.current.clone_agent("agent-1", "fresh");
    });
    expect(cloned?.session_id).toBe("agent-clone");
    expect(result.current.agents[0].session_id).toBe("agent-clone");

    let deleted: readonly string[] = [];
    await act(async () => {
      deleted = await result.current.delete_agents(["agent-1", "missing"]);
    });
    expect(deleted).toEqual(["agent-1", "missing"]);
    expect(result.current.agents.some((agent) => agent.session_id === "agent-1")).toBe(false);

    await act(async () => result.current.reorder_agents(["agent-2", "agent-clone"]));
    expect(result.current.agents.map((agent) => agent.session_id)).toEqual([
      "agent-2",
      "agent-clone",
    ]);
  });

  it("reports partial deletion success and continues deleting remaining agents", async () => {
    const on_error = vi.fn();
    mockInvoke.mockImplementation(async (command, args) => {
      const command_args = args as Record<string, unknown> | undefined;
      if (command === "list_agents") return agents;
      if (command === "kill_agent" && command_args?.sessionId === "agent-1") {
        throw new Error("locked");
      }
      if (command === "kill_agent") {
        agents = agents.filter((agent) => agent.session_id !== command_args?.sessionId);
      }
      return null;
    });

    const { result } = renderHook(() => useAgentResourceController({ on_error }));
    await waitFor(() => expect(result.current.agents).toHaveLength(2));

    let deleted: readonly string[] = [];
    await act(async () => {
      deleted = await result.current.delete_agents(["agent-1", "agent-2"]);
    });

    expect(deleted).toEqual(["agent-2"]);
    expect(on_error).toHaveBeenCalledWith("kill_agent", expect.any(Error));
    expect(result.current.agents.map((agent) => agent.session_id)).toEqual(["agent-1"]);
  });
});
