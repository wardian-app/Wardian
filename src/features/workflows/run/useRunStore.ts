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
  blueprintPath: string | null;
  scrubIndex: number;
  loadRuns: () => Promise<void>;
  openRun: (blueprintId: string, runId: string) => Promise<void>;
  clearOpenRun: () => void;
  setScrubIndex: (index: number) => void;
  currentNodeStatuses: () => Record<string, NodeStatusKind>;
  reset: () => void;
}

const initialState = {
  runs: [],
  state: null,
  events: [],
  blueprint: null,
  blueprintPath: null,
  scrubIndex: 0,
};

export const useRunStore = create<RunStoreState>((set, get) => ({
  ...initialState,
  async loadRuns() {
    const runs = await invoke<RunSummary[]>('workflow_list_runs');
    if (runSummariesEqual(get().runs, runs)) return;
    set({ runs });
  },
  async openRun(blueprintId, runId) {
    const result = await invoke<RunReadResult>('workflow_read_run', { blueprintId, runId });
    const events = result.events ?? [];
    const summaryPath = get().runs.find((run) => run.blueprint_id === blueprintId && run.run_id === runId)?.blueprint_path;
    set({
      state: result.state,
      events,
      blueprint: result.blueprint,
      blueprintPath: result.blueprint_path ?? summaryPath ?? null,
      scrubIndex: Math.max(0, events.length - 1),
    });
  },
  clearOpenRun() {
    set({
      state: null,
      events: [],
      blueprint: null,
      blueprintPath: null,
      scrubIndex: 0,
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

function runSummariesEqual(left: RunSummary[], right: RunSummary[]) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (!runSummaryEqual(left[index], right[index])) return false;
  }
  return true;
}

function runSummaryEqual(left: RunSummary, right: RunSummary) {
  return left.run_id === right.run_id
    && left.blueprint_id === right.blueprint_id
    && left.status === right.status
    && left.node_count === right.node_count
    && left.path === right.path
    && left.blueprint_path === right.blueprint_path
    && left.started_at === right.started_at
    && left.updated_at === right.updated_at
    && left.completed_at === right.completed_at
    && left.failure === right.failure
    && left.schedule_id === right.schedule_id;
}
