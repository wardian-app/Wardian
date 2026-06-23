import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Blueprint } from "../workflows/builder/blueprintTypes";
import type { RunSummary } from "../workflows/run/runTypes";
import type { GardenWorkflowInput } from "./gardenProjection";

interface BlueprintRef {
  id: string;
  path: string;
}

interface ParsedBlueprint {
  id: string;
  name: string;
  nodeCount: number;
}

/** Pure: attach each blueprint's most-recent run status (by updated_at). */
export function mergeWorkflowRunStatus(
  blueprints: ParsedBlueprint[],
  runs: RunSummary[],
): GardenWorkflowInput[] {
  const latest = new Map<string, RunSummary>();
  for (const run of runs) {
    const existing = latest.get(run.blueprint_id);
    if (!existing || (run.updated_at ?? "") > (existing.updated_at ?? "")) {
      latest.set(run.blueprint_id, run);
    }
  }
  return blueprints.map((bp) => ({
    id: bp.id,
    label: bp.name,
    runStatus: latest.get(bp.id)?.status ?? "none",
    nodeCount: bp.nodeCount,
  }));
}

/** Loads the blueprint catalog (list + parse, mirroring WorkflowsView) and merges run status. */
export function useGardenWorkflows(): GardenWorkflowInput[] {
  const [workflows, setWorkflows] = useState<GardenWorkflowInput[]>([]);

  const load = useCallback(async () => {
    const refs = await invoke<BlueprintRef[]>("workflow_list_blueprints").catch(() => []);
    const parsedRaw = await Promise.all(
      refs.map(async (ref) => {
        const result = await invoke<{ blueprint: Blueprint }>("workflow_parse", { path: ref.path }).catch(() => null);
        if (!result?.blueprint) return null;
        return {
          id: result.blueprint.id,
          name: result.blueprint.name,
          nodeCount: result.blueprint.nodes.length,
        } satisfies ParsedBlueprint;
      }),
    );
    const blueprints = parsedRaw.filter((bp): bp is ParsedBlueprint => bp !== null);
    const runs = await invoke<RunSummary[]>("workflow_list_runs").catch(() => []);
    setWorkflows(mergeWorkflowRunStatus(blueprints, runs));
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return workflows;
}
