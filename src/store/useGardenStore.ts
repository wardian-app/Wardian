import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { GardenPosition } from "../features/garden/garden.types";
import {
  createScene,
  excludeFromDistrict,
  markVisited,
  pinEntity,
  pruneScene,
  reviveScene,
  scenesConverged,
  unpinEntity,
  type GardenScene,
} from "../features/garden/gardenScene";

interface GardenStoreState {
  scene: GardenScene;
  /** True when stored geometry predates the current metric version. */
  needsRederive: boolean;
  /**
   * Incremented whenever the scene is discarded rather than advanced.
   *
   * The view carries the layout's own scene forward through a ref, deliberately
   * outside the reactive chain, so that settled positions cannot re-trigger the
   * layout that produced them. That makes a reset invisible: the store's scene
   * is emptied, and the next pass warm-starts from the carried copy and puts
   * everything back exactly where it was. A counter is the smallest thing that
   * distinguishes "the scene moved on" from "the scene was thrown away".
   */
  generation: number;
  /**
   * Pin an entity at an absolute point, stored relative to its district origin
   * so the placement survives the district moving.
   */
  pin: (
    entityKey: string,
    districtId: string,
    absolute: GardenPosition,
    districtOrigin: GardenPosition,
  ) => void;
  unpin: (entityKey: string) => void;
  exclude: (entityKey: string, districtId: string) => void;
  visit: (entityKey: string) => void;
  /**
   * Adopt the scene a layout pass returned (district cells, warm-start seeds).
   *
   * With `liveKeys`, derived state for every other entity is pruned as it is
   * written. Pass only a set from `authoritativeLiveKeys`, and null otherwise:
   * a partial set would discard the saved positions of everything it omits.
   */
  adoptScene: (scene: GardenScene, liveKeys?: ReadonlySet<string> | null) => void;
  reset: () => void;
}

/**
 * Garden scene store.
 *
 * The malleable-garden spec calls for scenes to live as inspectable files under
 * the active Wardian home. They still persist through browser storage here; the
 * scene is a plain serializable object with a versioned schema and a tolerant
 * reviver precisely so that becomes a storage swap rather than a rewrite.
 *
 * Layout output is written back through `adoptScene` because the drift penalty
 * needs last-known positions to warm-start from. Without them every reload would
 * re-derive from scratch and the map would rearrange itself in front of the user.
 */
export const useGardenStore = create<GardenStoreState>()(
  persist(
    (set) => ({
      scene: createScene(),
      needsRederive: false,
      generation: 0,
      pin: (entityKey, districtId, absolute, districtOrigin) =>
        set((state) => ({
          scene: pinEntity(state.scene, entityKey, districtId, absolute, districtOrigin),
        })),
      unpin: (entityKey) => set((state) => ({ scene: unpinEntity(state.scene, entityKey) })),
      exclude: (entityKey, districtId) =>
        set((state) => ({ scene: excludeFromDistrict(state.scene, entityKey, districtId) })),
      visit: (entityKey) => set((state) => ({ scene: markVisited(state.scene, entityKey) })),
      // A layout pass always returns a fresh scene object even when nothing
      // moved. Committing it unconditionally would publish a new `scene`
      // reference on every pass, re-rendering every subscriber and re-writing
      // storage for sub-pixel changes. Keeping the existing reference when the
      // two scenes are materially the same makes the write-back idempotent,
      // which is what stops a relayout from provoking another one.
      //
      // Pruning rides on the same write rather than a timer: this is where the
      // scene is next saved anyway, and the caller only supplies `liveKeys` once
      // every entity source has loaded. It touches no pin or exclusion, so it
      // cannot provoke a relayout either.
      adoptScene: (scene, liveKeys = null) =>
        set((state) => {
          const incoming = liveKeys ? pruneScene(scene, liveKeys) : scene;
          if (!scenesConverged(state.scene, incoming)) return { scene: incoming };
          // A converged pass still owes the stored scene its pruning: `visited`
          // is outside the convergence test, so a dead key's timestamp would
          // otherwise survive every pass.
          const stored = liveKeys ? pruneScene(state.scene, liveKeys) : state.scene;
          return stored === state.scene ? state : { scene: stored };
        }),
      reset: () =>
        set((state) => ({
          scene: createScene(),
          needsRederive: false,
          generation: state.generation + 1,
        })),
    }),
    {
      name: "wardian-garden",
      // Only the scene is persisted; `needsRederive` is derived on load.
      partialize: (state) => ({ scene: state.scene }),
      version: 2,
      // v1 stored absolute positions keyed by unitKey plus a boolean pin map.
      // Those coordinates came from the phyllotaxis seeding and carry no meaning
      // under the metric layout, so they are dropped rather than migrated into
      // pins the user never actually placed.
      migrate: () => ({ scene: createScene() }),
      merge: (persisted, current) => {
        const revived = reviveScene((persisted as { scene?: unknown } | undefined)?.scene);
        return { ...current, scene: revived.scene, needsRederive: revived.needsRederive };
      },
    },
  ),
);
