import { isUnderPath, parentPath } from "./entityRef";
import type { TerrainChangeEntry } from "./useTerrainChanges";
import type { TerrainPaint } from "./terrainPaint";
import type { GardenTimeLens } from "./gardenNavigation";

/** Unknown recency is retained: absence of attribution is not evidence of inactivity. */
export function activityInLens(paint: TerrainPaint | undefined, lens: GardenTimeLens): boolean {
  if (!paint || lens === "branch" || paint.evidence === "inferred" || paint.recencyKnown === false) return true;
  return paint.recency > 2 ** (-(lens === "now" ? 2 : 16) / 8);
}

export interface GardenActivityGroup {
  path: string;
  isDirectory: boolean;
  count: number;
  agents: string[];
}

/** Immediate active children, including deleted files absent from filesystem listings. */
export function activityChildren(
  root: string,
  entries: ReadonlyMap<string, TerrainChangeEntry>,
  paint: ReadonlyMap<string, TerrainPaint>,
  lens: GardenTimeLens,
): GardenActivityGroup[] {
  const groups = new Map<string, GardenActivityGroup>();
  for (const [path, change] of entries) {
    if (path === root || !isUnderPath(root, path) || !activityInLens(paint.get(path), lens)) continue;
    let child = path;
    for (;;) {
      const parent = parentPath(child);
      if (!parent || parent === root) break;
      child = parent;
    }
    const current = groups.get(child) ?? { path: child, isDirectory: child !== path, count: 0, agents: [] };
    current.count += 1;
    current.agents = [...new Set([...current.agents, ...change.entry.agent_ids])];
    groups.set(child, current);
  }
  return [...groups.values()].sort((a, b) => Number(b.isDirectory) - Number(a.isDirectory) || a.path.localeCompare(b.path));
}
