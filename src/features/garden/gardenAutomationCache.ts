import type { Blueprint } from "../automations/builder/blueprintTypes";
import type { GardenRunEvidence } from "./automationProjection";

export const GARDEN_BLUEPRINT_CACHE_MS = 60_000;
export const GARDEN_RUN_CACHE_MS = 300_000;
export const GARDEN_AUTOMATION_CACHE_LIMIT = 512;
interface Entry<T> { value: T; expires: number }

/** Owned by one hook. Refreshes stage writes privately so cancellation cannot warm the cache. */
export class GardenAutomationCache {
  private generation = 0;
  private blueprints = new Map<string, Entry<Blueprint>>();
  private runs = new Map<string, Entry<GardenRunEvidence>>();

  invalidate() {
    this.generation++;
    this.blueprints.clear();
    this.runs.clear();
  }

  begin(now: number) {
    const generation = this.generation;
    const blueprints = new Map(this.blueprints);
    const runs = new Map(this.runs);
    const get = <T,>(map: Map<string, Entry<T>>, key: string) => {
      const entry = map.get(key);
      return entry && entry.expires > now ? entry.value : undefined;
    };
    const put = <T,>(map: Map<string, Entry<T>>, key: string, value: T, ttl: number) => {
      map.delete(key);
      map.set(key, { value, expires: now + ttl });
    };
    const prune = <T,>(map: Map<string, Entry<T>>, keep: ReadonlySet<string>) => {
      for (const [key, entry] of map) if (!keep.has(key) || entry.expires <= now) map.delete(key);
      while (map.size > GARDEN_AUTOMATION_CACHE_LIMIT) map.delete(map.keys().next().value!);
    };
    return {
      blueprint: (key: string) => get(blueprints, key),
      run: (key: string) => get(runs, key),
      putBlueprint: (key: string, value: Blueprint) => put(blueprints, key, value, GARDEN_BLUEPRINT_CACHE_MS),
      putRun: (key: string, value: GardenRunEvidence) => put(runs, key, value, GARDEN_RUN_CACHE_MS),
      commit: (paths: ReadonlySet<string>, identities: ReadonlySet<string>) => {
        if (generation !== this.generation) return;
        prune(blueprints, paths);
        prune(runs, identities);
        this.blueprints = blueprints;
        this.runs = runs;
        this.generation++;
      },
    };
  }
}
