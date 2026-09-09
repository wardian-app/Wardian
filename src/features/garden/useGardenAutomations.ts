import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Blueprint } from "../automations/builder/blueprintTypes";
import type { RunReadResult, RunSummaryListResult } from "../automations/run/runTypes";
import type { BlueprintListResult } from "../automations/automationTypes";
import type { AutomationSchedule } from "../../types/automation";
import { GardenAutomationCache } from "./gardenAutomationCache";
import {
  projectSituatedAutomations, isVisibleGardenRun, GARDEN_AUTOMATION_RECENT_MS,
  type GardenBlueprintEvidence, type GardenRunEvidence, type GardenRunInvocation, type SituatedAutomationInput,
} from "./automationProjection";

type GardenInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
type AutomationSource = "blueprints" | "runs" | "schedules";
interface AutomationSourceEvidence {
  blueprints: { refs: BlueprintListResult["blueprints"]; nextOffset: number | null };
  runs: { runs: RunSummaryListResult["runs"]; nextOffset: number | null };
  schedules: AutomationSchedule[];
}
const emptySources: AutomationSourceEvidence = {
  blueprints: { refs: [], nextOffset: null }, runs: { runs: [], nextOffset: null }, schedules: [],
};

export interface GardenAutomationInputsResult {
  sourceErrors: Partial<Record<AutomationSource, string>>;
  /** Per-hook source cache; only completed, non-cancelled refreshes publish it. */
  sourceEvidence: AutomationSourceEvidence;
  automations: SituatedAutomationInput[];
  automationProjections: SituatedAutomationInput[];
  /** Selected evidence, including expired runs; never feed this array to the canvas. */
  retainedAutomations: SituatedAutomationInput[];
  projectionErrors: Record<string, string[]>;
  truncated: boolean;
  nextOffset: number | null;
  runsNextOffset: number | null;
  errors: string[];
}
export interface GardenAutomationHookResult extends GardenAutomationInputsResult {
  loading: boolean;
  error: string | null;
  loadMore: () => Promise<void>;
  refresh: () => Promise<void>;
}
interface LoadOptions {
  cache?: GardenAutomationCache;
  previousSources?: AutomationSourceEvidence;
  retainedProjectionIds?: readonly string[];
  signal?: AbortSignal;
  /** Reload this many pages from each catalog, preserving the expanded window on events. */
  pageCount?: number;
  now?: number;
  recentMs?: number;
  knownAgentIds?: ReadonlySet<string>;
}

/** Lists stay fresh; successful stable evidence may use a bounded per-hook cache. */
export async function loadGardenAutomationInputs(
  invoker: GardenInvoke = invoke as GardenInvoke, blueprintOffset = 0, options: LoadOptions = {},
): Promise<GardenAutomationInputsResult> {
  const check = () => options.signal?.throwIfAborted();
  const cache = options.cache?.begin(options.now ?? Date.now());
  const errors: string[] = [];
  const sourceErrors: Partial<Record<AutomationSource, string>> = {};
  const projectionErrors: Record<string, string[]> = {};
  const retainedIds = new Set(options.retainedProjectionIds);
  const failProjection = (id: string, message: string) => {
    (projectionErrors[id] ??= []).push(message);
  };
  const read = async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
    check();
    const value = await invoker(command, args);
    check();
    return value as T;
  };
  const pageCount = Math.max(1, options.pageCount ?? 1);
  const recoverSource = async <K extends AutomationSource>(source: K, request: Promise<AutomationSourceEvidence[K]>): Promise<AutomationSourceEvidence[K]> => {
    try { return await request; }
    catch (failure) {
      check();
      const message = `${source}: ${String(failure)}`;
      sourceErrors[source] = message;
      errors.push(message);
      return options.previousSources?.[source] ?? emptySources[source];
    }
  };
  const [blueprintPages, runPages, schedules] = await Promise.all([
    recoverSource("blueprints", (async () => {
      const refs: BlueprintListResult["blueprints"] = [];
      let offset: number | null = blueprintOffset;
      for (let page = 0; page < pageCount && offset !== null; page++) {
        const result: BlueprintListResult = await read("automation_list_blueprints", offset ? { offset } : undefined);
        refs.push(...result.blueprints);
        const next: number | null = result.truncated ? result.next_offset : null;
        if (result.truncated && (next === null || next <= offset)) throw new Error("Blueprint paging did not advance");
        offset = next;
      }
      return { refs, nextOffset: offset };
    })()),
    recoverSource("runs", (async () => {
      const runs: RunSummaryListResult["runs"] = [];
      let offset: number | null = 0;
      for (let page = 0; page < pageCount && offset !== null; page++) {
        const result: RunSummaryListResult = await read("automation_list_runs", offset ? { offset } : undefined);
        runs.push(...result.runs);
        const next: number | null = result.truncated ? result.next_offset ?? null : null;
        if (result.truncated && (next === null || next <= offset)) throw new Error("Run paging did not advance");
        offset = next;
      }
      return { runs, nextOffset: offset };
    })()),
    recoverSource("schedules", read<AutomationSchedule[]>("schedule_list")),
  ]);
  // Search beyond the visible page window only for explicitly retained run IDs.
  // These pages do not expand the Habitat population or consume its paging cursor.
  const retainedSummaries = [...runPages.runs];
  const missing = new Set([...retainedIds].filter((id) => id.startsWith("run:")
    && !retainedSummaries.some((run) => id === `run:${run.run_id}`)));
  let searchOffset = runPages.nextOffset;
  while (missing.size && searchOffset !== null) {
    try {
      const page = await read<RunSummaryListResult>("automation_list_runs", { offset: searchOffset });
      for (const run of page.runs) if (missing.delete(`run:${run.run_id}`)) retainedSummaries.push(run);
      const next = page.truncated ? page.next_offset ?? null : null;
      if (page.truncated && (next === null || next <= searchOffset)) throw new Error("Retained run paging did not advance");
      searchOffset = next;
    } catch (failure) {
      check();
      const message = `Retained run lookup: ${String(failure)}`;
      errors.push(message);
      for (const id of missing) failProjection(id, message);
      break;
    }
  }
  const blueprints: GardenBlueprintEvidence[] = [];
  const paths = new Set(blueprintPages.refs.map((ref) => ref.path));
  const retained = [...new Map(retainedSummaries.map((run) => [run.run_id, run])).values()].filter((run) =>
    isVisibleGardenRun(run, options.now ?? Date.now(), options.recentMs ?? GARDEN_AUTOMATION_RECENT_MS)
    || retainedIds.has(`run:${run.run_id}`) || retainedIds.has(`schedule:${run.schedule_id}`));
  for (const run of retained) if (run.blueprint_path) paths.add(run.blueprint_path);
  await Promise.all([...paths].map(async (path) => {
    try {
      const cached = cache?.blueprint(path);
      const parsed = cached ? { blueprint: cached } : await read<{ blueprint?: Blueprint }>("automation_parse", { path });
      if (!parsed.blueprint) throw new Error("Blueprint unavailable");
      if (!cached) cache?.putBlueprint(path, parsed.blueprint);
      blueprints.push({ blueprint: parsed.blueprint, path });
    } catch (failure) {
      check();
      const message = `Blueprint: ${String(failure)}`;
      errors.push(message);
      const blueprintId = blueprintPages.refs.find((ref) => ref.path === path)?.id;
      if (blueprintId) {
        failProjection(`binding:${blueprintId}`, message);
        for (const schedule of schedules) if (schedule.blueprint_id === blueprintId) failProjection(`schedule:${schedule.id}`, message);
      }
    }
  }));
  const runIdentities = new Set<string>();
  const runs = await Promise.all(retained.map(async (summary): Promise<GardenRunEvidence> => {
    // Include every summary field, including timestamps, path and assignments' schedule ID.
    const identity = JSON.stringify(Object.entries(summary).sort(([a], [b]) => a.localeCompare(b)));
    const terminal = summary.status === "completed" || summary.status === "failed";
    if (terminal) runIdentities.add(identity);
    const cached = terminal ? cache?.run(identity) : undefined;
    if (cached) return { ...cached, summary };
    const [detail, invocation] = await Promise.all([
      read<RunReadResult>("automation_read_run", { blueprintId: summary.blueprint_id, runId: summary.run_id })
        .catch((failure) => { check(); errors.push(`Run ${summary.run_id}: ${String(failure)}`); return null; }),
      read<string>("read_file_preview", { path: summary.path.replace(/[\\/]+$/, "") + "/invocation.json" })
        .then((body): GardenRunInvocation => {
          const value: unknown = JSON.parse(body);
          if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid invocation record");
          return value as GardenRunInvocation;
        }).catch((failure) => { check(); errors.push(`Run assignments ${summary.run_id}: ${String(failure)}`); return null; }),
    ]);
    if (!detail || !invocation) {
      const id = summary.schedule_id ? `schedule:${summary.schedule_id}` : `run:${summary.run_id}`;
      failProjection(id, `Run ${summary.run_id}: ${!detail ? "detail unavailable" : "invocation unavailable"}`);
    }
    const evidence = { summary, detail, invocation };
    // Invocation is written at launch, but files remain editable. Cache it only with
    // terminal evidence, bounded by TTL/manual invalidation; never cache partial reads.
    if (terminal && detail?.state?.status === summary.status && invocation) cache?.putRun(identity, evidence);
    return evidence;
  }));
  check();
  blueprints.sort((a, b) => a.blueprint.id.localeCompare(b.blueprint.id));
  const automations = projectSituatedAutomations(blueprints, schedules, runs, { ...options, retainedProjectionIds: [] });
  const retainedAutomations = projectSituatedAutomations(blueprints, schedules, runs, options).filter((item) => retainedIds.has(item.id));
  // Recompute from the independently recovered sources. Do not replace a whole
  // schedule snapshot merely because its metadata source is stale: live runs may
  // still carry fresh assignments and stage state.
  for (const item of [...automations, ...retainedAutomations]) {
    const affectedErrors = [
      item.projectionKind === "schedule" ? sourceErrors.schedules : undefined,
      item.projectionKind !== "binding" ? sourceErrors.runs : undefined,
      item.projectionKind !== "run" ? sourceErrors.blueprints : undefined,
    ].filter((message): message is string => !!message);
    if (affectedErrors.length) { item.stale = true; item.evidenceErrors = affectedErrors; }
  }
  check();
  cache?.commit(paths, runIdentities);
  return { automations, automationProjections: automations,
    sourceErrors, sourceEvidence: { blueprints: blueprintPages, runs: runPages, schedules },
    retainedAutomations, projectionErrors,
    truncated: blueprintPages.nextOffset !== null || runPages.nextOffset !== null,
    nextOffset: blueprintPages.nextOffset, runsNextOffset: runPages.nextOffset, errors };
}

const emptyResult: GardenAutomationInputsResult = {
  sourceErrors: {}, sourceEvidence: emptySources,
  retainedAutomations: [], projectionErrors: {},
  automations: [], automationProjections: [], truncated: false, nextOffset: null, runsNextOffset: null, errors: [],
};

/** Canonical events refresh assignments; quiet polling refreshes stages and expires recent trails. */
export function useGardenAutomations(enabled = true, options: { retainedProjectionIds?: readonly string[] } = {}): GardenAutomationHookResult {
  const [result, setResult] = useState(emptyResult);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pages = useRef(1);
  const sources = useRef(emptySources);
  const cache = useRef(new GardenAutomationCache());
  const controller = useRef<AbortController | null>(null);
  const inFlight = useRef(false);
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;
  // Value-based dependency allows callers to pass an inline array without a reload loop.
  const retainedKey = JSON.stringify([...new Set(options.retainedProjectionIds)].sort());
  const reload = useCallback(async (invalidate = false) => {
    if (!enabledRef.current) return;
    if (invalidate) cache.current.invalidate();
    controller.current?.abort();
    const request = new AbortController();
    controller.current = request;
    inFlight.current = true;
    setLoading(true);
    try {
      const retainedProjectionIds = JSON.parse(retainedKey) as string[];
      const next = await loadGardenAutomationInputs(invoke as GardenInvoke, 0, { cache: cache.current, signal: request.signal, pageCount: pages.current, retainedProjectionIds, previousSources: sources.current });
      if (request.signal.aborted || !enabledRef.current) return;
      sources.current = next.sourceEvidence;
      setError(next.errors.length ? next.errors.join("; ") : null);
      setResult((current) => {
        const previous = new Map([...current.automations, ...current.retainedAutomations].map((item) => [item.id, item]));
        const preserve = (items: SituatedAutomationInput[], retainedOnly: boolean) => {
          const merged = new Map(items.map((item) => [item.id, item]));
          for (const [id, evidenceErrors] of Object.entries(next.projectionErrors)) {
            const old = previous.get(id);
            if (!old || (retainedOnly && !retainedProjectionIds.includes(id))) continue;
            // Expiry continues to apply to stale map trails, even while their record is selected.
            if (!retainedOnly && old.projectionKind === "run" && !old.runs.some((run) => isVisibleGardenRun(run, Date.now(), GARDEN_AUTOMATION_RECENT_MS))) continue;
            merged.set(id, { ...old, stale: true, evidenceErrors });
          }
          return [...merged.values()];
        };
        const automations = preserve(next.automations, false);
        return { ...next, automations, automationProjections: automations, retainedAutomations: preserve(next.retainedAutomations, true) };
      });
    } catch (failure) {
      if (!request.signal.aborted && enabledRef.current) setError(String(failure));
    } finally {
      if (!request.signal.aborted) { inFlight.current = false; setLoading(false); }
    }
  }, [retainedKey]);
  const refresh = useCallback(() => reload(true), [reload]);
  const loadMore = useCallback(async () => {
    if (!enabledRef.current || inFlight.current || !result.truncated) return;
    pages.current += 1;
    await reload();
  }, [result.truncated, reload]);
  useEffect(() => {
    if (!enabled) { controller.current?.abort(); cache.current.invalidate(); inFlight.current = false; setLoading(false); return; }
    let disposed = false;
    const disposers: (() => void)[] = [];
    for (const event of ["schedules-updated", "automation-inbox-updated", "library-changed"]) {
      void listen(event, () => { if (!disposed) void reload(event === "library-changed"); }).then((unlisten) => {
        if (disposed) unlisten(); else disposers.push(unlisten);
      }).catch((failure) => { if (!disposed) setError(`Automation events: ${String(failure)}`); });
    }
    void reload();
    const interval = setInterval(() => { if (!inFlight.current) void reload(); }, 15_000);
    return () => { disposed = true; controller.current?.abort(); inFlight.current = false; clearInterval(interval); disposers.forEach((dispose) => dispose()); };
  }, [enabled, reload]);
  const selectedIds = new Set(options.retainedProjectionIds);
  return { ...result, retainedAutomations: result.retainedAutomations.filter((item) => selectedIds.has(item.id)), loading, error, loadMore, refresh };
}
