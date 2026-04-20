import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { GridLayout } from '../types';

const DEFAULT_LEFT_SIDEBAR_WIDTH = 260;
const DEFAULT_RIGHT_SIDEBAR_WIDTH = 240;
const MIN_SIDEBAR_WIDTH = 200;
const MAX_SIDEBAR_FRACTION = 0.4;

const DEFAULT_LAYOUT: GridLayout = {
  column_tracks: [0.5, 0.5],
  row_height: 450,
};

const clampSidebarWidth = (px: number): number => {
  const max = Math.max(MIN_SIDEBAR_WIDTH, Math.floor(window.innerWidth * MAX_SIDEBAR_FRACTION));
  return Math.max(MIN_SIDEBAR_WIDTH, Math.min(max, Math.round(px)));
};

interface LayoutState {
  layout: GridLayout;
  leftSidebarWidth: number;
  rightSidebarWidth: number;
  gridStacked: boolean;
  previousColumnTracks: number[] | null;
  setColumnTracks: (tracks: number[]) => void;
  setRowHeight: (height: number) => void;
  setLeftSidebarWidth: (px: number) => void;
  setRightSidebarWidth: (px: number) => void;
  setGridStacked: (v: boolean) => void;
  setPreviousColumnTracks: (tracks: number[] | null) => void;
  resetLayout: () => void;
}

export const useLayoutStore = create<LayoutState>()(
  persist(
    (set) => ({
      layout: DEFAULT_LAYOUT,
      leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
      rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
      gridStacked: false,
      previousColumnTracks: null,
      setColumnTracks: (column_tracks) => set((state) => ({ layout: { ...state.layout, column_tracks } })),
      setRowHeight: (row_height) => set((state) => ({ layout: { ...state.layout, row_height } })),
      setLeftSidebarWidth: (px) => set({ leftSidebarWidth: clampSidebarWidth(px) }),
      setRightSidebarWidth: (px) => set({ rightSidebarWidth: clampSidebarWidth(px) }),
      setGridStacked: (gridStacked) => set({ gridStacked }),
      setPreviousColumnTracks: (previousColumnTracks) => set({ previousColumnTracks }),
      resetLayout: () => set({
        layout: DEFAULT_LAYOUT,
        leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
        rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
        gridStacked: false,
        previousColumnTracks: null,
      }),
    }),
    { name: 'wardian-layout' }
  )
);
