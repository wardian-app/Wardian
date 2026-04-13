import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { GridLayout } from '../types';

interface LayoutState {
  layout: GridLayout;
  setColumnTracks: (tracks: number[]) => void;
  setRowHeight: (height: number) => void;
  resetLayout: () => void;
}

const DEFAULT_LAYOUT: GridLayout = {
  column_tracks: [1, 1], // Default 2 equal columns
  row_height: 450,
};

export const useLayoutStore = create<LayoutState>()(
  persist(
    (set) => ({
      layout: DEFAULT_LAYOUT,
      setColumnTracks: (column_tracks) => set((state) => ({ layout: { ...state.layout, column_tracks } })),
      setRowHeight: (row_height) => set((state) => ({ layout: { ...state.layout, row_height } })),
      resetLayout: () => set({ layout: DEFAULT_LAYOUT }),
    }),
    { name: 'wardian-layout' }
  )
);
