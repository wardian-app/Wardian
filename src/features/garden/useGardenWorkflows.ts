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

type GardenInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

let cachedBlueprintKey: string | null = null;
let cachedBlueprints: ParsedBlueprint[] = [];

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

export async function loadGardenWorkflowInputs(invoker: GardenInvoke = invoke as GardenInvoke): Promise<GardenWorkflowInput[]> {
  // `invoke` can resolve to null (not just reject), so coalesce to [] before mapping.
  const refs = ((await invoker("workflow_list_blueprints").catch(() => [])) ?? []) as BlueprintRef[];
  const nextBlueprintKey = blueprintRefsKey(refs);
  let blueprints = cachedBlueprintKey === nextBlueprintKey ? cachedBlueprints : null;

  if (!blueprints) {
    const parsedRaw = await Promise.all(
      refs.map(async (ref) => {
        const result = await invoker("workflow_parse", { path: ref.path }).catch(() => null) as { blueprint?: Blueprint } | null;
        if (!result?.blueprint) return null;
        return {
          id: result.blueprint.id,
          name: result.blueprint.name,
          nodeCount: result.blueprint.nodes.length,
        } satisfies ParsedBlueprint;
      }),
    );
    blueprints = parsedRaw.filter((bp): bp is ParsedBlueprint => bp !== null);
    cachedBlueprintKey = nextBlueprintKey;
    cachedBlueprints = blueprints;
  }

  const runs = ((await invoker("workflow_list_runs").catch(() => [])) ?? []) as RunSummary[];
  return mergeWorkflowRunStatus(blueprints, runs);
}

export function resetGardenWorkflowCacheForTests() {
  cachedBlueprintKey = null;
  cachedBlueprints = [];
}

function blueprintRefsKey(refs: BlueprintRef[]) {
  return JSON.stringify(refs.map((ref) => [ref.id, ref.path]));
}

/** Loads the blueprint catalog (list + parse, mirroring WorkflowsView) and merges run status. */
export function useGardenWorkflows(): GardenWorkflowInput[] {
  const [workflows, setWorkflows] = useState<GardenWorkflowInput[]>([]);

  const load = useCallback(async () => {
    setWorkflows(await loadGardenWorkflowInputs());
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return workflows;
}
