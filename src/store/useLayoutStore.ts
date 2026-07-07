import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { GridLayout } from '../types';

const DEFAULT_LEFT_SIDEBAR_WIDTH = 240;
const DEFAULT_RIGHT_SIDEBAR_WIDTH = 240;
const MIN_SIDEBAR_WIDTH = 200;
const MAX_SIDEBAR_FRACTION = 0.4;
const DEFAULT_USER_TERMINAL_HEIGHT = 360;
const MIN_USER_TERMINAL_HEIGHT = 180;
const MAX_USER_TERMINAL_FRACTION = 0.7;
const DEFAULT_LIBRARY_DETAIL_WIDTH = 480;
const MIN_LIBRARY_DETAIL_WIDTH = 360;
const MAX_LIBRARY_DETAIL_FRACTION = 0.7;

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

const clampLibraryDetailWidth = (px: number): number => {
  const max = Math.max(MIN_LIBRARY_DETAIL_WIDTH, Math.floor(window.innerWidth * MAX_LIBRARY_DETAIL_FRACTION));
  return Math.max(MIN_LIBRARY_DETAIL_WIDTH, Math.min(max, Math.round(px)));
};

interface LayoutState {
  layout: GridLayout;
  leftSidebarWidth: number;
  rightSidebarWidth: number;
  userTerminalOpen: boolean;
  settingsOpen: boolean;
  userTerminalHeight: number;
  gridStacked: boolean;
  previousColumnTracks: number[] | null;
  libraryDetailWidth: number;
  setColumnTracks: (tracks: number[]) => void;
  setRowHeight: (height: number) => void;
  setLeftSidebarWidth: (px: number) => void;
  setRightSidebarWidth: (px: number) => void;
  setUserTerminalOpen: (open: boolean) => void;
  setSettingsOpen: (open: boolean) => void;
  setUserTerminalHeight: (px: number) => void;
  setLibraryDetailWidth: (px: number) => void;
  toggleUserTerminal: () => void;
  toggleSettings: () => void;
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
      settingsOpen: false,
      userTerminalHeight: DEFAULT_USER_TERMINAL_HEIGHT,
      gridStacked: false,
      previousColumnTracks: null,
      libraryDetailWidth: DEFAULT_LIBRARY_DETAIL_WIDTH,
      setColumnTracks: (column_tracks) => set((state) => ({ layout: { ...state.layout, column_tracks } })),
      setRowHeight: (row_height) => set((state) => ({ layout: { ...state.layout, row_height } })),
      setLeftSidebarWidth: (px) => set({ leftSidebarWidth: clampSidebarWidth(px) }),
      setRightSidebarWidth: (px) => set({ rightSidebarWidth: clampSidebarWidth(px) }),
      setUserTerminalOpen: (userTerminalOpen) => set({ userTerminalOpen }),
      setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
      setUserTerminalHeight: (px) => set({ userTerminalHeight: clampUserTerminalHeight(px) }),
      setLibraryDetailWidth: (px) => set({ libraryDetailWidth: clampLibraryDetailWidth(px) }),
      toggleUserTerminal: () => set((state) => ({ userTerminalOpen: !state.userTerminalOpen })),
      toggleSettings: () => set((state) => ({ settingsOpen: !state.settingsOpen })),
      setGridStacked: (gridStacked) => set({ gridStacked }),
      setPreviousColumnTracks: (previousColumnTracks) => set({ previousColumnTracks }),
      resetLayout: () => set({
        layout: DEFAULT_LAYOUT,
        leftSidebarWidth: DEFAULT_LEFT_SIDEBAR_WIDTH,
        rightSidebarWidth: DEFAULT_RIGHT_SIDEBAR_WIDTH,
        userTerminalOpen: false,
        settingsOpen: false,
        userTerminalHeight: DEFAULT_USER_TERMINAL_HEIGHT,
        gridStacked: false,
        previousColumnTracks: null,
        libraryDetailWidth: DEFAULT_LIBRARY_DETAIL_WIDTH,
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
        state.libraryDetailWidth = clampLibraryDetailWidth(state.libraryDetailWidth);
      },
    }
  )
);
