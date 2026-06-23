import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { GardenPosition } from "../features/garden/garden.types";

interface GardenStoreState {
  positions: Record<string, GardenPosition>;
  pins: Record<string, boolean>;
  setPosition: (key: string, position: GardenPosition) => void;
  togglePin: (key: string) => void;
  reset: () => void;
}

export const useGardenStore = create<GardenStoreState>()(
  persist(
    (set) => ({
      positions: {},
      pins: {},
      setPosition: (key, position) =>
        set((state) => ({ positions: { ...state.positions, [key]: position } })),
      togglePin: (key) =>
        set((state) => ({ pins: { ...state.pins, [key]: !state.pins[key] } })),
      reset: () => set({ positions: {}, pins: {} }),
    }),
    { name: "wardian-garden" },
  ),
);
