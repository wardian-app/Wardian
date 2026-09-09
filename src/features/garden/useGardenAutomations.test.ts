import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { loadGardenAutomationInputs, useGardenAutomations } from "./useGardenAutomations";
import { GardenAutomationCache, GARDEN_BLUEPRINT_CACHE_MS, GARDEN_RUN_CACHE_MS, GARDEN_AUTOMATION_CACHE_LIMIT } from "./gardenAutomationCache";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const blueprint = { schema: 1, id: "build", name: "Build", nodes: [
  { id: "task", type: "task", fields: { agent: "role:worker" } },
], edges: [] };
const schedule = { id: "daily", blueprint_id: "build", name: "Daily", workspace: "/repo",
  assignments: { worker: { target_type: "agent", agent_id: "saved" } } };
const summary = { run_id: "r1", blueprint_id: "build", schedule_id: "daily", status: "running", path: "/runs/r1", node_count: 1 };
function responder(command: string): Promise<unknown> {
  if (command === "automation_list_blueprints") return Promise.resolve({ blueprints: [{ id: "build", path: "/build.md" }], truncated: false, next_offset: null });
  if (command === "automation_parse") return Promise.resolve({ blueprint });
  if (command === "automation_list_runs") return Promise.resolve({ runs: [summary], truncated: false, next_offset: null });
  if (command === "schedule_list") return Promise.resolve([schedule]);
  if (command === "automation_read_run") return Promise.resolve({ blueprint, events: [], state: { nodes: { task: "running" } } });
  if (command === "read_file_preview") return Promise.resolve(JSON.stringify({ workspace: "/live", assignments: { worker: { target_type: "agent", agent_id: "live" } } }));
  return Promise.reject(new Error(command));
}
beforeEach(() => { vi.clearAllMocks(); vi.mocked(invoke).mockImplementation(responder); vi.mocked(listen).mockResolvedValue(() => undefined); });

describe("bounded refresh caching", () => {
  const now = Date.parse("2026-09-08T00:00:00Z");
  const ended = { ...summary, status: "completed", updated_at: new Date(now).toISOString() };
  const completedResponder = async (command: string) => {
    if (command === "automation_list_runs") return { runs: [ended], truncated: false, next_offset: null };
    if (command === "automation_read_run") return { blueprint, events: [], state: { status: "completed", nodes: { task: "completed" } } };
    return responder(command);
  };
  it("reduces 46 terminal runs from 141 reads to the three fresh catalogs", async () => {
    const cache = new GardenAutomationCache();
    const invoker = vi.fn(async (command: string) => {
      if (command === "automation_list_blueprints") return { blueprints: Array.from({ length: 46 }, (_, i) => ({ id: `b${i}`, path: `/b${i}.md` })), truncated: false, next_offset: null };
      if (command === "automation_list_runs") return { runs: Array.from({ length: 46 }, (_, i) => ({ ...ended, run_id: `r${i}`, path: `/r${i}` })), truncated: false, next_offset: null };
      return completedResponder(command);
    });
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    expect(invoker).toHaveBeenCalledTimes(141);
    invoker.mockClear();
    await loadGardenAutomationInputs(invoker, 0, { cache, now: now + 15_000 });
    expect(invoker.mock.calls.map(([command]) => command).sort()).toEqual(["automation_list_blueprints", "automation_list_runs", "schedule_list"]);
  });
  it("expires same-path definitions and terminal evidence without extending TTL on hits", async () => {
    const cache = new GardenAutomationCache();
    const invoker = vi.fn(completedResponder);
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    invoker.mockClear();
    await loadGardenAutomationInputs(invoker, 0, { cache, now: now + GARDEN_BLUEPRINT_CACHE_MS });
    expect(invoker.mock.calls.map(([command]) => command)).toContain("automation_parse");
    expect(invoker.mock.calls.map(([command]) => command)).not.toContain("automation_read_run");
    invoker.mockClear();
    await loadGardenAutomationInputs(invoker, 0, { cache, now: now + GARDEN_RUN_CACHE_MS });
    expect(invoker.mock.calls.map(([command]) => command)).toContain("automation_read_run");
    expect(invoker.mock.calls.map(([command]) => command)).toContain("read_file_preview");
  });
  it("rereads live runs and invalidates terminal evidence when any summary field changes", async () => {
    const cache = new GardenAutomationCache();
    let run = { ...ended };
    const invoker = vi.fn(async (command: string) => command === "automation_list_runs"
      ? { runs: [run], truncated: false, next_offset: null } : completedResponder(command));
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    run = { ...run, node_count: 2 };
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    run = { ...run, status: "running" };
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    expect(invoker.mock.calls.filter(([command]) => command === "automation_read_run")).toHaveLength(4);
    expect(invoker.mock.calls.filter(([command]) => command === "read_file_preview")).toHaveLength(4);
    expect(invoker.mock.calls.filter(([command]) => command === "automation_parse")).toHaveLength(1);
  });
  it("does not cache failed or mismatched terminal evidence, and retries expired parse failures", async () => {
    const cache = new GardenAutomationCache();
    let fail = false;
    const invoker = vi.fn(async (command: string) => {
      if (fail && command === "automation_parse") throw new Error("parse offline");
      if (command === "automation_read_run") return responder(command); // running detail under terminal summary
      return completedResponder(command);
    });
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    fail = true;
    const failed = await loadGardenAutomationInputs(invoker, 0, { cache, now: now + GARDEN_BLUEPRINT_CACHE_MS });
    expect(failed.errors.join()).toContain("parse offline");
    fail = false;
    const recovered = await loadGardenAutomationInputs(invoker, 0, { cache, now: now + GARDEN_BLUEPRINT_CACHE_MS + 1 });
    expect(recovered.errors).toEqual([]);
    expect(invoker.mock.calls.filter(([command]) => command === "automation_parse")).toHaveLength(3);
    expect(invoker.mock.calls.filter(([command]) => command === "automation_read_run")).toHaveLength(3);
  });
  it("does not publish cache writes from an aborted refresh", async () => {
    const cache = new GardenAutomationCache();
    const controller = new AbortController();
    const invoker = vi.fn(async (command: string) => {
      if (command === "read_file_preview") controller.abort();
      return completedResponder(command);
    });
    await expect(loadGardenAutomationInputs(invoker, 0, { cache, now, signal: controller.signal })).rejects.toThrow();
    invoker.mockImplementation(completedResponder);
    invoker.mockClear();
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    expect(invoker).toHaveBeenCalledTimes(6);
  });
  it("retries incomplete invocation evidence rather than caching a partial success", async () => {
    const cache = new GardenAutomationCache();
    let fail = true;
    const invoker = vi.fn(async (command: string) => {
      if (command === "read_file_preview" && fail) throw new Error("invocation unavailable");
      return completedResponder(command);
    });
    const failed = await loadGardenAutomationInputs(invoker, 0, { cache, now });
    expect(failed.projectionErrors["schedule:daily"]).toEqual(["Run r1: invocation unavailable"]);
    fail = false;
    const recovered = await loadGardenAutomationInputs(invoker, 0, { cache, now });
    expect(recovered.errors).toEqual([]);
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    expect(invoker.mock.calls.filter(([command]) => command === "read_file_preview")).toHaveLength(2);
    expect(invoker.mock.calls.filter(([command]) => command === "automation_read_run")).toHaveLength(2);
  });
  it("caches retained historical evidence without changing population or paging, then prunes on deselection", async () => {
    const cache = new GardenAutomationCache();
    const old = { ...ended, schedule_id: null, updated_at: "2020-01-01T00:00:00Z" };
    const invoker = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "automation_list_runs") return args?.offset
        ? { runs: [old], truncated: false, next_offset: null }
        : { runs: [], truncated: true, next_offset: 200 };
      return completedResponder(command);
    });
    const options = { cache, now, retainedProjectionIds: ["run:r1"] };
    await loadGardenAutomationInputs(invoker, 0, options);
    invoker.mockClear();
    const second = await loadGardenAutomationInputs(invoker, 0, options);
    expect(second.retainedAutomations.map((item) => item.id)).toEqual(["run:r1"]);
    expect(second.automations.some((item) => item.id === "run:r1")).toBe(false);
    expect(second.runsNextOffset).toBe(200);
    expect(invoker.mock.calls.some(([command]) => command === "automation_read_run")).toBe(false);
    await loadGardenAutomationInputs(invoker, 0, { cache, now });
    invoker.mockClear();
    await loadGardenAutomationInputs(invoker, 0, options);
    expect(invoker.mock.calls.some(([command]) => command === "automation_read_run")).toBe(true);
  });
  it("bounds entries, prunes absent paths, and rejects invalidated in-flight transactions", () => {
    const cache = new GardenAutomationCache();
    const tx = cache.begin(now);
    const paths = new Set(Array.from({ length: GARDEN_AUTOMATION_CACHE_LIMIT + 1 }, (_, i) => String(i)));
    for (const path of paths) tx.putBlueprint(path, blueprint);
    tx.commit(paths, new Set());
    expect(cache.begin(now).blueprint("0")).toBeUndefined();
    expect(cache.begin(now).blueprint("1")).toEqual(blueprint);
    cache.begin(now).commit(new Set(["1"]), new Set());
    expect(cache.begin(now).blueprint("2")).toBeUndefined();
    const pending = cache.begin(now);
    pending.putBlueprint("old", blueprint);
    cache.invalidate();
    pending.commit(new Set(["old"]), new Set());
    expect(cache.begin(now).blueprint("old")).toBeUndefined();
  });
  it("keeps caches on ordinary events but invalidates on library events and manual refresh", async () => {
    const callbacks = new Map<string, () => void>();
    vi.mocked(listen).mockImplementation(async (name, callback) => {
      callbacks.set(name, () => callback({ event: name, id: 1, payload: null }));
      return () => undefined;
    });
    const { result, unmount } = renderHook(() => useGardenAutomations());
    await waitFor(() => expect(result.current.loading).toBe(false));
    const parses = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "automation_parse").length;
    expect(parses()).toBe(1);
    await act(async () => callbacks.get("schedules-updated")?.());
    expect(parses()).toBe(1);
    await act(async () => callbacks.get("library-changed")?.());
    expect(parses()).toBe(2);
    await act(async () => result.current.refresh());
    expect(parses()).toBe(3);
    unmount();
  });
});

describe("loadGardenAutomationInputs", () => {
  it("reads durable invocation assignments and stage state instead of pooling blueprint agents", async () => {
    const invoker = vi.fn(responder);
    const result = await loadGardenAutomationInputs(invoker);
    expect(result.errors).toEqual([]);
    expect(result.automationProjections).toBe(result.automations);
    expect(result.automations[0]).toMatchObject({ id: "schedule:daily", agentIds: ["live"], workspacePaths: ["/live"], stages: [{ nodeId: "task", status: "running" }] });
    expect(invoker).toHaveBeenCalledWith("read_file_preview", { path: "/runs/r1/invocation.json" });
  });
  it("reparses same-path definitions on refresh", async () => {
    const invoker = vi.fn(responder);
    await loadGardenAutomationInputs(invoker);
    await loadGardenAutomationInputs(invoker);
    expect(invoker.mock.calls.filter(([command]) => command === "automation_parse")).toHaveLength(2);
  });
  it("reports both paging cursors and loads additional catalog and run pages", async () => {
    const invoker = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "automation_list_blueprints") return { blueprints: [], truncated: !args?.offset, next_offset: args?.offset ? null : 10 };
      if (command === "automation_list_runs") return { runs: [], truncated: !args?.offset, next_offset: args?.offset ? null : 20 };
      return responder(command);
    });
    const first = await loadGardenAutomationInputs(invoker);
    expect(first).toMatchObject({ truncated: true, nextOffset: 10, runsNextOffset: 20 });
    const second = await loadGardenAutomationInputs(invoker, 0, { pageCount: 2 });
    expect(second.truncated).toBe(false);
    expect(invoker).toHaveBeenCalledWith("automation_list_runs", { offset: 20 });
    expect(invoker).toHaveBeenCalledWith("automation_list_blueprints", { offset: 10 });
  });
  it("reports a non-advancing page as a source failure while loading healthy sources", async () => {
    const result = await loadGardenAutomationInputs(async (command) => command === "automation_list_runs"
      ? { runs: [], truncated: true, next_offset: 0 } : responder(command));
    expect(result.sourceErrors.runs).toContain("Run paging");
    expect(result.automations[0]).toMatchObject({ id: "schedule:daily", stale: true });
  });
  it("exposes invocation read failures without borrowing saved live assignments", async () => {
    const result = await loadGardenAutomationInputs(async (command) => {
      if (command === "read_file_preview") throw new Error("unavailable");
      return responder(command);
    });
    expect(result.errors[0]).toContain("unavailable");
    expect(result.automations).toEqual([]);
  });
  it("cancels before invoking and after an outstanding request resolves", async () => {
    const controller = new AbortController();
    controller.abort();
    const invoker = vi.fn(responder);
    await expect(loadGardenAutomationInputs(invoker, 0, { signal: controller.signal })).rejects.toThrow();
    expect(invoker).not.toHaveBeenCalled();
  });
});

describe("useGardenAutomations", () => {
  it("refreshes events, retains the last scene on failure, and releases subscriptions", async () => {
    const callbacks = new Map<string, () => void>();
    const dispose = vi.fn();
    vi.mocked(listen).mockImplementation(async (name, callback) => {
      callbacks.set(name, () => callback({ event: name, id: 1, payload: null }));
      return dispose;
    });
    const { result, unmount } = renderHook(() => useGardenAutomations());
    await waitFor(() => expect(result.current.automations).toHaveLength(1));
    const previous = result.current.automations;
    vi.mocked(invoke).mockRejectedValue(new Error("offline"));
    act(() => callbacks.get("schedules-updated")?.());
    await waitFor(() => expect(result.current.error).toContain("offline"));
    expect(result.current.automations[0]).toMatchObject({ id: previous[0].id, agentIds: previous[0].agentIds, stale: true });
    vi.mocked(invoke).mockImplementation(responder);
    act(() => callbacks.get("automation-inbox-updated")?.());
    await waitFor(() => expect(result.current.error).toBeNull());
    unmount();
    expect(dispose).toHaveBeenCalledTimes(3);
  });
  it("does not load while disabled and discards completion after disable", async () => {
    const { result, rerender } = renderHook(({ enabled }) => useGardenAutomations(enabled), { initialProps: { enabled: false } });
    expect(invoke).not.toHaveBeenCalled();
    let resolve: (value: unknown) => void = () => undefined;
    vi.mocked(invoke).mockImplementation((command) => command === "automation_list_blueprints"
      ? new Promise((done) => { resolve = done; }) : responder(command));
    rerender({ enabled: true });
    rerender({ enabled: false });
    await act(async () => resolve({ blueprints: [], truncated: false, next_offset: null }));
    expect(result.current.automations).toEqual([]);
    expect(result.current.loading).toBe(false);
  });
  it("cleans up listeners that finish subscribing after unmount", async () => {
    const pending: ((dispose: () => void) => void)[] = [];
    vi.mocked(listen).mockImplementation(() => new Promise((resolve) => pending.push(resolve)));
    const { unmount } = renderHook(() => useGardenAutomations());
    unmount();
    const dispose = vi.fn();
    await act(async () => pending.forEach((resolve) => resolve(dispose)));
    expect(dispose).toHaveBeenCalledTimes(3);
  });
});

describe("independent source recovery", () => {
  it.each([
    ["schedules", "schedule_list"],
    ["blueprints", "automation_list_blueprints"],
    ["runs", "automation_list_runs"],
  ] as const)("refreshes healthy evidence when %s fails and clears staleness after recovery", async (source, failedCommand) => {
    let version = "old";
    let fail = false;
    const manual = { ...summary, schedule_id: null, run_id: "manual" };
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (fail && command === failedCommand) throw new Error("source offline");
      if (command === "automation_list_blueprints") return { blueprints: [
        { id: "build", path: "/build.md" }, { id: "direct", path: "/direct.md" },
      ], truncated: false, next_offset: null };
      if (command === "automation_parse" && (args as Record<string, unknown>)?.path === "/direct.md") return {
        blueprint: { ...blueprint, id: "direct", nodes: [{ id: "task", type: "task", fields: { agent: `binding-${version}` } }] },
      };
      if (command === "automation_list_runs") return { runs: [manual], truncated: false, next_offset: null };
      if (command === "schedule_list") return [{ ...schedule, name: `schedule-${version}` }];
      if (command === "read_file_preview") return JSON.stringify({ workspace: "/live", assignments: { worker: { target_type: "agent", agent_id: `manual-${version}` } } });
      return responder(command);
    });
    const { result, unmount } = renderHook(() => useGardenAutomations());
    await waitFor(() => expect(result.current.automations).toHaveLength(3));
    version = "new"; fail = true;
    await act(async () => result.current.refresh());
    expect(result.current.sourceErrors[source]).toContain("source offline");
    const byId = new Map(result.current.automations.map((item) => [item.id, item]));
    // A failed schedule or catalog cannot freeze a healthy manual run.
    expect(byId.get("run:manual")?.agentIds).toEqual(["manual-new"]);
    expect(byId.get("run:manual")?.stale).toBe(source === "runs" ? true : undefined);
    // Cached catalog paths may still be parsed, and run/schedule failures never
    // prevent the healthy blueprint definition from changing its direct binding.
    expect(byId.get("binding:direct")?.agentIds).toEqual(["binding-new"]);
    expect(byId.get("binding:direct")?.stale).toBe(source === "blueprints" ? true : undefined);
    expect(byId.get("schedule:daily")?.label).toBe(source === "schedules" ? "schedule-old" : "schedule-new");
    expect(byId.get("schedule:daily")?.stale).toBe(true);
    fail = false;
    await act(async () => result.current.refresh());
    expect(result.current.sourceErrors).toEqual({});
    expect(result.current.automations.every((item) => !item.stale)).toBe(true);
    expect(result.current.error).toBeNull();
    unmount();
  });
  it("loads healthy data on an initial source failure without a cache", async () => {
    const result = await loadGardenAutomationInputs(async (command) => {
      if (command === "schedule_list") throw new Error("schedule source offline");
      if (command === "automation_list_runs") return { runs: [{ ...summary, schedule_id: null }], truncated: false, next_offset: null };
      return responder(command);
    });
    expect(result.automations[0]).toMatchObject({ id: "run:r1", agentIds: ["live"] });
    expect(result.automations[0].stale).toBeUndefined();
    expect(result.sourceErrors.schedules).toContain("schedule source offline");
  });
});

describe("focused historical evidence", () => {
  const old = { ...summary, run_id: "old", schedule_id: null, status: "completed", updated_at: "2020-01-01T00:00:00Z", path: "/runs/old" };
  it("finds a retained expired run beyond the canvas window without reviving its trail", async () => {
    const invoker = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "automation_list_runs") return args?.offset
        ? { runs: [old, { ...old, run_id: "unselected" }], truncated: false, next_offset: null }
        : { runs: [], truncated: true, next_offset: 20 };
      return responder(command);
    });
    const result = await loadGardenAutomationInputs(invoker, 0, { retainedProjectionIds: ["run:old"] });
    expect(result.retainedAutomations.map((item) => item.id)).toEqual(["run:old"]);
    expect(result.automationProjections.map((item) => item.id)).toEqual(["schedule:daily"]);
    expect(result).toMatchObject({ truncated: true, runsNextOffset: 20 });
    expect(invoker).toHaveBeenCalledWith("automation_read_run", { blueprintId: "build", runId: "old" });
    expect(invoker).not.toHaveBeenCalledWith("automation_read_run", { blueprintId: "build", runId: "unselected" });
    expect(result.retainedAutomations[0].runEvidence[0].detail?.blueprint).toEqual(blueprint);
  });
  it("does not search historical pages without an explicit retained ID", async () => {
    const invoker = vi.fn(async (command: string) => command === "automation_list_runs"
      ? { runs: [old], truncated: true, next_offset: 20 } : responder(command));
    const result = await loadGardenAutomationInputs(invoker);
    expect(result.retainedAutomations).toEqual([]);
    expect(invoker.mock.calls.filter(([command]) => command === "automation_list_runs")).toHaveLength(1);
    expect(invoker.mock.calls.some(([command]) => command === "automation_read_run")).toBe(false);
  });
  it("keeps focused evidence across the expiry boundary while removing the canvas trail", async () => {
    const invoker = async (command: string) => command === "automation_list_runs"
      ? { runs: [old], truncated: false, next_offset: null } : responder(command);
    const endedAt = Date.parse(old.updated_at);
    const before = await loadGardenAutomationInputs(invoker, 0, { now: endedAt + 999, recentMs: 1000, retainedProjectionIds: ["run:old"] });
    const after = await loadGardenAutomationInputs(invoker, 0, { now: endedAt + 1001, recentMs: 1000, retainedProjectionIds: ["run:old"] });
    expect(before.automationProjections.some((item) => item.id === "run:old")).toBe(true);
    expect(after.automationProjections.some((item) => item.id === "run:old")).toBe(false);
    expect(after.retainedAutomations[0].runEvidence).toEqual(before.retainedAutomations[0].runEvidence);
  });
  it("clears retained records immediately on deselection and accepts inline arrays without reload loops", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "automation_list_runs"
      ? { runs: [old], truncated: false, next_offset: null } : responder(command));
    const { result, rerender, unmount } = renderHook(({ ids }) => useGardenAutomations(true, { retainedProjectionIds: [...ids] }),
      { initialProps: { ids: ["run:old"] } });
    await waitFor(() => expect(result.current.retainedAutomations).toHaveLength(1));
    expect(result.current.automationProjections.some((item) => item.id === "run:old")).toBe(false);
    const calls = vi.mocked(invoke).mock.calls.length;
    rerender({ ids: ["run:old"] });
    expect(vi.mocked(invoke).mock.calls).toHaveLength(calls);
    rerender({ ids: [] });
    expect(result.current.retainedAutomations).toEqual([]);
    unmount();
  });
  it("preserves only failed evidence while healthy projections update and recovers on retry", async () => {
    let broken = false;
    let healthyName = "Daily";
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "automation_list_runs") return { runs: [old], truncated: false, next_offset: null };
      if (command === "schedule_list") return [{ ...schedule, name: healthyName }];
      if (command === "read_file_preview" && (args as Record<string, unknown>)?.path === "/runs/old/invocation.json" && broken) throw new Error("old invocation unavailable");
      return responder(command);
    });
    const { result, unmount } = renderHook(() => useGardenAutomations(true, { retainedProjectionIds: ["run:old"] }));
    await waitFor(() => expect(result.current.retainedAutomations).toHaveLength(1));
    broken = true; healthyName = "Rebound daily";
    await act(async () => result.current.refresh());
    expect(result.current.automations[0].label).toBe("Rebound daily");
    expect(result.current.automations[0].stale).toBeUndefined();
    expect(result.current.automations.some((item) => item.id === "run:old")).toBe(false);
    expect(result.current.retainedAutomations[0]).toMatchObject({ stale: true, evidenceErrors: ["Run old: invocation unavailable"] });
    expect(result.current.error).toContain("old invocation unavailable");
    broken = false;
    await act(async () => result.current.refresh());
    expect(result.current.retainedAutomations[0].stale).toBeUndefined();
    expect(result.current.error).toBeNull();
    unmount();
  });
  it("keeps successful current data when the historical page lookup fails", async () => {
    const result = await loadGardenAutomationInputs(async (command, args) => {
      if (command === "automation_list_runs") {
        if (args?.offset) throw new Error("history unavailable");
        return { runs: [summary], truncated: true, next_offset: 20 };
      }
      return responder(command);
    }, 0, { retainedProjectionIds: ["run:old"] });
    expect(result.automations[0].agentIds).toEqual(["live"]);
    expect(result.projectionErrors["run:old"][0]).toContain("history unavailable");
    expect(result.retainedAutomations).toEqual([]);
  });
});
