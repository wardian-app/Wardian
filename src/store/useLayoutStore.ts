import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { GridLayout } from '../types';

const DEFAULT_LEFT_SIDEBAR_WIDTH = 260;
const DEFAULT_RIGHT_SIDEBAR_WIDTH = 240;
const MIN_SIDEBAR_WIDTH = 200;
const MAX_SIDEBAR_FRACTION = 0.4;
const DEFAULT_USER_TERMINAL_HEIGHT = 360;
const MIN_USER_TERMINAL_HEIGHT = 180;
const MAX_USER_TERMINAL_FRACTION = 0.7;

const DEFAULT_LAYOUT: GridLayout = {
  column_tracks: [0.5, 0.5],
  row_height: 450,
};

const clampSidebarWidth = (px: number): number => {
  const max = Math.max(MIN_SIDEBAR_WIDTH, Math.floor(window.innerWidth * MAX_SIDEBAR_FRACTION));
  return Math.max(MIN_SIDEBAR_WIDTH, Math.min(max, Math.round(px)));
};

const clampUserTerminalHeight = (px: number): number => {
  const max = Math.max(MIN_USER_TERMINAL_HEIGHT, Math.floor(window.innerHeight * MAX_USER_TERMINAL_FRACTION));
  return Math.max(MIN_USER_TERMINAL_HEIGHT, Math.min(max, Math.round(px)));
};

interface LayoutState {
  layout: GridLayout;
  leftSidebarWidth: number;
  rightSidebarWidth: number;
  userTerminalOpen: boolean;
  userTerminalHeight: number;
  gridStacked: boolean;
  previousColumnTracks: number[] | null;
  setColumnTracks: (tracks: number[]) => void;
  setRowHeight: (height: number) => void;
  setLeftSidebarWidth: (px: number) => void;
  setRightSidebarWidth: (px: number) => void;
  setUserTerminalOpen: (open: boolean) => void;
  setUserTerminalHeight: (px: number) => void;
  toggleUserTerminal: () => void;
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
      userTerminalOpen: false,
      userTerminalHeight: DEFAULT_USER_TERMINAL_HEIGHT,
      gridStacked: false,
      previousColumnTracks: null,
      setColumnTracks: (column_tracks) => set((state) => ({ layout: { ...state.layout, column_tracks } })),
      setRowHeight: (row_height) => set((state) => ({ layout: { ...state.layout, row_height } })),
      setLeftSidebarWidth: (px) => set({ leftSidebarWidth: clampSidebarWidth(px) }),
      setRightSidebarWidth: (px) => set({ rightSidebarWidth: clampSidebarWidth(px) }),
      setUserTerminalOpen: (userTerminalOpen) => set({ userTerminalOpen }),
      setUserTerminalHeight: (px) => set({ userTerminalHeight: clampUserTerminalHeight(px) }),
      toggleUserTerminal: () => set((state) => ({ userTerminalOpen: !state.userTerminalOpen })),
      setGridStacked: (gridStacked) => set({ gridStacked }),
      setPreviousColumnTracks: (previousColumnTracks) => set({ previousColumnTracks }),
      resetLayout: () => set({
        layout: DEFAULT_LAYOUT,
        leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
        rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
        userTerminalOpen: false,
        userTerminalHeight: DEFAULT_USER_TERMINAL_HEIGHT,
        gridStacked: false,
        previousColumnTracks: null,
      }),
    }),
    {
      name: 'wardian-layout',
      onRehydrateStorage: () => (state) => {
        if (!state) return;
        // localStorage may hold values outside the current viewport's clamp range
        // (e.g. user resized the window since last session). Re-clamp on load.
        state.leftSidebarWidth = clampSidebarWidth(state.leftSidebarWidth);
        state.rightSidebarWidth = clampSidebarWidth(state.rightSidebarWidth);
        state.userTerminalHeight = clampUserTerminalHeight(state.userTerminalHeight);
      },
    }
  )
);
