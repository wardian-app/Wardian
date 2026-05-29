import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { nodeStatusesAt } from './replay';
import type { Blueprint } from '../builder/blueprintTypes';
import type { NodeStatusKind, RunEvent, RunReadResult, RunState, RunSummary } from './runTypes';

interface RunStoreState {
  runs: RunSummary[];
  state: RunState | null;
  events: RunEvent[];
  blueprint: Blueprint | null;
  scrubIndex: number;
  loadRuns: () => Promise<void>;
  openRun: (blueprintId: string, runId: string) => Promise<void>;
  setScrubIndex: (index: number) => void;
  currentNodeStatuses: () => Record<string, NodeStatusKind>;
  reset: () => void;
}

const initialState = {
  runs: [],
  state: null,
  events: [],
  blueprint: null,
  scrubIndex: 0,
};

export const useRunStore = create<RunStoreState>((set, get) => ({
  ...initialState,
  async loadRuns() {
    const runs = await invoke<RunSummary[]>('workflow_list_runs');
    set({ runs });
  },
  async openRun(blueprintId, runId) {
    const result = await invoke<RunReadResult>('workflow_read_run', { blueprintId, runId });
    const events = result.events ?? [];
    set({
      state: result.state,
      events,
      blueprint: result.blueprint,
      scrubIndex: Math.max(0, events.length - 1),
    });
  },
  setScrubIndex(index) {
    const last = Math.max(0, get().events.length - 1);
    const scrubIndex = Math.min(Math.max(0, index), last);
    set({ scrubIndex });
  },
  currentNodeStatuses() {
    return nodeStatusesAt(get().events, get().scrubIndex);
  },
  reset() {
    set(initialState);
  },
}));
