import { create } from 'zustand';
import { GridLayout } from '../types';

const DEFAULT_LEFT_SIDEBAR_WIDTH = 240;
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
  leftSidebarCollapsed: boolean;
  rightSidebarCollapsed: boolean;
  leftSidebarWidth: number;
  rightSidebarWidth: number;
  userTerminalOpen: boolean;
  userTerminalHeight: number;
  gridStacked: boolean;
  previousColumnTracks: number[] | null;
  setColumnTracks: (tracks: number[]) => void;
  setRowHeight: (height: number) => void;
  setLeftSidebarCollapsed: (collapsed: boolean) => void;
  setRightSidebarCollapsed: (collapsed: boolean) => void;
  setLeftSidebarWidth: (px: number) => void;
  setRightSidebarWidth: (px: number) => void;
  setUserTerminalOpen: (open: boolean) => void;
  setUserTerminalHeight: (px: number) => void;
  toggleUserTerminal: () => void;
  setGridStacked: (v: boolean) => void;
  setPreviousColumnTracks: (tracks: number[] | null) => void;
  resetGridLayout: () => void;
  resetLayout: () => void;
}

export const useLayoutStore = create<LayoutState>()(
    (set) => ({
      layout: DEFAULT_LAYOUT,
      leftSidebarCollapsed: false,
      rightSidebarCollapsed: false,
      leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
      rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
      userTerminalOpen: false,
      userTerminalHeight: DEFAULT_USER_TERMINAL_HEIGHT,
      gridStacked: false,
      previousColumnTracks: null,
      setColumnTracks: (column_tracks) => set((state) => ({ layout: { ...state.layout, column_tracks } })),
      setRowHeight: (row_height) => set((state) => ({ layout: { ...state.layout, row_height } })),
      setLeftSidebarCollapsed: (leftSidebarCollapsed) => set({ leftSidebarCollapsed }),
      setRightSidebarCollapsed: (rightSidebarCollapsed) => set({ rightSidebarCollapsed }),
      setLeftSidebarWidth: (px) => set({ leftSidebarWidth: clampSidebarWidth(px) }),
      setRightSidebarWidth: (px) => set({ rightSidebarWidth: clampSidebarWidth(px) }),
      setUserTerminalOpen: (userTerminalOpen) => set({ userTerminalOpen }),
      setUserTerminalHeight: (px) => set({ userTerminalHeight: clampUserTerminalHeight(px) }),
      toggleUserTerminal: () => set((state) => ({ userTerminalOpen: !state.userTerminalOpen })),
      setGridStacked: (gridStacked) => set({ gridStacked }),
      setPreviousColumnTracks: (previousColumnTracks) => set({ previousColumnTracks }),
      resetGridLayout: () => set({
        layout: DEFAULT_LAYOUT,
        gridStacked: false,
        previousColumnTracks: null,
      }),
      resetLayout: () => set({
        layout: DEFAULT_LAYOUT,
        leftSidebarCollapsed: false,
        rightSidebarCollapsed: false,
        leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
        rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
        userTerminalOpen: false,
        userTerminalHeight: DEFAULT_USER_TERMINAL_HEIGHT,
        gridStacked: false,
        previousColumnTracks: null,
      }),
    })
);
