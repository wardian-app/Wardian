import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type {
  ChangeReviewBaseline,
  ChangeReviewFileEntry,
  ChangeReviewLoadResponse,
  ChangeReviewPrefs,
} from "../../types";
import { normalizeEntityPath } from "./entityRef";
import { ROOT_KEY_SEPARATOR } from "./useGardenTerrain";
import {
  buildTerrainPaint,
  joinWorkspacePath,
  type RootChangeSet,
  type TerrainPaint,
} from "./terrainPaint";

interface ExplorerChangedEvent {
  root_path: string;
  changed_paths: string[];
}

/** Change-review calls allowed in flight at once. */
export const MAX_CHANGE_CONCURRENCY = 4;

/**
 * Baselines the map can honestly render.
 *
 * `last_effective_turn`, `conversation_start`, and `unreviewed` all resolve
 * against one agent's snapshots and watermark. That is exactly right for the
 * Changes pane, which shows one selected agent — and meaningless for a map
 * showing every agent at once, where picking a representative would make the
 * whole colouring depend on an arbitrary choice.
 *
 * `head` and `branch_point` are computed from the repository and name no agent,
 * so the map adopts the pane's preference when it is one of these and falls
 * back to `head` when it is not. The legend states which baseline is in force,
 * so the two surfaces can differ but can never differ *silently*.
 */
const MAP_BASELINES: readonly ChangeReviewBaseline[] = ["head", "branch_point"];

/**
 * What the map falls back to when the pane's baseline is agent-scoped.
 *
 * `branch_point` rather than `head`, and the default matters more than it
 * looks: the stored preference defaults to `last_effective_turn`, which is
 * agent-scoped, so *every* install that has never changed the setting lands on
 * this fallback. With `head` that meant a ground showing only uncommitted work
 * — blank across a tidy roster, and blank for exactly the repositories whose
 * agents had finished and committed. The map-scale question is "what has this
 * branch done", which is what `branch_point` answers.
 */
export const DEFAULT_TERRAIN_BASELINE: ChangeReviewBaseline = "branch_point";

export function terrainBaselineFor(preference: ChangeReviewBaseline): ChangeReviewBaseline {
  return MAP_BASELINES.includes(preference) ? preference : DEFAULT_TERRAIN_BASELINE;
}

export interface TerrainChangesOptions {
  enabled: boolean;
  /** Normalized workspace roots whose ground is currently on screen. */
  roots: readonly string[];
}

/**
 * A changed path, with everything needed to open its comparison.
 *
 * The paint is an aggregate and cannot answer this: a folder's paint says its
 * subtree was modified, but opening a diff needs the entry's own `old_path`,
 * `change_kind`, and the root and revision it was computed against.
 */
export interface TerrainChangeEntry {
  entry: ChangeReviewFileEntry;
  /**
   * Normalized repository root the entry's path is relative to.
   *
   * The comparison opens with this as its `cwd`, so it must be the base the
   * path resolves against and not merely some directory inside the repository.
   */
  root: string;
  /** Revision the baseline resolved to, for the comparison. */
  baselineRef: string | null;
}

export interface TerrainChangesResult {
  /** Failed reads remain visible as a limitation, rather than an empty activity claim. */
  errors: ReadonlyMap<string, string>;
  /** Normalized absolute path -> paint, including every changed path's ancestors. */
  paint: ReadonlyMap<string, TerrainPaint>;
  /** Normalized absolute path -> the change entry itself, for changed files only. */
  entries: ReadonlyMap<string, TerrainChangeEntry>;
  baseline: ChangeReviewBaseline;
  /** Roots that reported no git repository, so the map can stay quiet about them. */
  withoutGit: ReadonlySet<string>;
}

const EMPTY_PAINT: ReadonlyMap<string, TerrainPaint> = new Map();
const EMPTY_ENTRIES: ReadonlyMap<string, TerrainChangeEntry> = new Map();
const EMPTY_ROOTS: ReadonlySet<string> = new Set();

/**
 * Fetch the change set for each visible workspace root and turn it into paint.
 *
 * One call per root, not one per agent: `load_change_review` attributes across
 * every conversation in the workspace, so the requesting agent affects only the
 * watermark and the snapshot baseline — neither of which a map-level baseline
 * uses. A 37-district install therefore costs a handful of `git status`
 * invocations rather than one per agent.
 *
 * Nothing polls. Recomputation is driven by `explorer-changed` for agent writes
 * and `git-changed` for the operator's own staging and commits, which is the
 * same pair the Changes pane subscribes to and for the same reasons:
 * `git_watch` observes only `.git/index` and `.git/HEAD`, so it can never see
 * the writes this paint exists to show.
 */
/**
 * A held summary, tagged with the baseline that produced it.
 *
 * Recorded rather than invalidated by a separate effect: an effect that clears
 * state on a baseline change has to run *before* the one that decides what is
 * missing, and relying on declaration order for that is a bug waiting for
 * someone to reorder two hooks. A tag makes staleness a property of the datum.
 */
interface HeldChangeSet extends RootChangeSet {
  baseline: ChangeReviewBaseline;
  baselineRef: string | null;
}

/**
 * Where an entry's path is rooted.
 *
 * Git reports paths relative to the *repository* root, not to the directory the
 * command ran in, so an agent working in a subdirectory of a repository would
 * otherwise have every path joined onto the wrong base — producing plausible
 * absolute paths that match no cell, and ground that is silently blank with no
 * error anywhere. The backend resolves it once; a workspace with no repository
 * falls back to the requested root, where its turn-record paths do belong.
 */
function joinBaseFor(response: ChangeReviewLoadResponse, root: string): string {
  return normalizeEntityPath(response.workspace_root) ?? root;
}

export function useTerrainChanges(options: TerrainChangesOptions): TerrainChangesResult {
  const { enabled, roots } = options;
  const [baseline, setBaseline] = useState<ChangeReviewBaseline>(DEFAULT_TERRAIN_BASELINE);
  const [baselineLoaded, setBaselineLoaded] = useState(false);
  const [changes, setChanges] = useState<ReadonlyMap<string, HeldChangeSet>>(
    () => new Map<string, HeldChangeSet>(),
  );
  const [withoutGit, setWithoutGit] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [errors, setErrors] = useState<ReadonlyMap<string, string>>(() => new Map());
  const inFlight = useRef(new Set<string>());

  const rootKey = useMemo(() => [...roots].sort().join(ROOT_KEY_SEPARATOR), [roots]);

  // The pane's habitual baseline, narrowed to what a map can render. Fetching
  // waits on this: starting with the default and correcting afterwards would
  // spend a `git status` per root on a question nobody asked.
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    void Promise.resolve(invoke<ChangeReviewPrefs>("load_change_review_prefs"))
      .then((prefs) => {
        if (!active) return;
        if (prefs) setBaseline(terrainBaselineFor(prefs.baseline));
        setBaselineLoaded(true);
      })
      .catch(() => {
        if (active) setBaselineLoaded(true);
      });
    return () => {
      active = false;
    };
  }, [enabled]);

  const load = useCallback(
    async (targets: readonly string[]) => {
      const queue = targets.filter((root) => !inFlight.current.has(root));
      if (queue.length === 0) return;
      for (const root of queue) inFlight.current.add(root);

      // A fixed pool rather than `Promise.all`: each call is a `git status` on a
      // real repository, and firing thirty-seven at once is a disk-bound stall
      // that delays the ones the user is actually looking at.
      let cursor = 0;
      const worker = async () => {
        for (;;) {
          const index = cursor++;
          if (index >= queue.length) return;
          const root = queue[index];
          try {
            const response = await invoke<ChangeReviewLoadResponse>("load_change_review", {
              request: { cwd: root, baseline, agent_id: null },
            });
            if (!response) continue;
            setErrors((current) => { if (!current.has(root)) return current; const next = new Map(current); next.delete(root); return next; });
            setWithoutGit((current) => {
              const next = new Set(current);
              if (response.git_available) next.delete(root);
              else next.add(root);
              return next;
            });
            setChanges((current) => {
              const next = new Map(current);
              // Keyed by the root that was requested — that is what `roots`
              // names — but rooted at the repository, which is what the paths
              // inside are relative to. The two differ for an agent scoped to a
              // subdirectory, and conflating them is the bug this separates.
              next.set(root, {
                root: joinBaseFor(response, root),
                baseline,
                baselineRef: response.summary.baseline_ref,
                entries: response.summary.files,
                toTurnIndex: response.summary.to_turn_index,
              });
              return next;
            });
          } catch (error) {
            setErrors((current) => new Map(current).set(root, String(error)));
            // A root that cannot be reviewed renders as unpainted ground. The
            // change-review spec is explicit that a failure here degrades the
            // view rather than becoming the view.
            setChanges((current) => {
              if (!current.has(root)) return current;
              const next = new Map(current);
              next.delete(root);
              return next;
            });
          } finally {
            inFlight.current.delete(root);
          }
        }
      };

      await Promise.all(
        Array.from({ length: Math.min(MAX_CHANGE_CONCURRENCY, queue.length) }, worker),
      );
    },
    [baseline],
  );

  // Load whatever is on screen and not held under the current baseline. Cached
  // results survive the root leaving the viewport, so panning back is free; a
  // baseline change makes every held set stale and reloads them.
  useEffect(() => {
    if (!enabled || !baselineLoaded || roots.length === 0) return;
    const missing = roots.filter((root) => changes.get(root)?.baseline !== baseline);
    if (missing.length === 0) return;
    void load(missing);
    // `changes` is read to find what is missing but must not retrigger this
    // effect on every arrival, or a slow root would restart the pass repeatedly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, baselineLoaded, baseline, rootKey, load]);

  useEffect(() => {
    if (!enabled || roots.length === 0) return;
    const watched = new Set(roots);
    let active = true;

    const refresh = (rootPath: string) => {
      const root = normalizeEntityPath(rootPath);
      if (!active || !root || !watched.has(root)) return;
      void load([root]);
    };

    const unlistenExplorer = listen<ExplorerChangedEvent>("explorer-changed", (event) =>
      refresh(event.payload.root_path),
    );
    const unlistenGit = listen<string>("git-changed", (event) => refresh(event.payload));

    return () => {
      active = false;
      void Promise.resolve(unlistenExplorer.then((unlisten) => unlisten?.())).catch(
        () => undefined,
      );
      void Promise.resolve(unlistenGit.then((unlisten) => unlisten?.())).catch(() => undefined);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, rootKey, load]);

  const paint = useMemo(() => {
    if (!enabled || changes.size === 0) return EMPTY_PAINT;
    // Only sets answering the current question. A set held under a superseded
    // baseline describes a comparison the legend is no longer claiming.
    const current = [...changes.values()].filter((held) => held.baseline === baseline);
    if (current.length === 0) return EMPTY_PAINT;
    return buildTerrainPaint(current);
  }, [enabled, changes, baseline]);

  const entries = useMemo(() => {
    if (!enabled || changes.size === 0) return EMPTY_ENTRIES;
    const index = new Map<string, TerrainChangeEntry>();
    for (const held of changes.values()) {
      if (held.baseline !== baseline) continue;
      for (const entry of held.entries) {
        const absolute = joinWorkspacePath(held.root, entry.path);
        if (!absolute) continue;
        index.set(absolute, { entry, root: held.root, baselineRef: held.baselineRef });
      }
    }
    return index;
  }, [enabled, changes, baseline]);

  return {
    errors,
    paint,
    entries,
    baseline,
    withoutGit: enabled ? withoutGit : EMPTY_ROOTS,
  };
}
