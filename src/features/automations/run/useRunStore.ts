import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { nodeStatusesAt } from './replay';
import type { Blueprint } from '../builder/blueprintTypes';
import type { NodeStatusKind, RunEvent, RunReadResult, RunState, RunSummary, RunSummaryListResult, TemporaryWorker, TemporaryWorkerTelemetry } from './runTypes';

interface RunStoreState {
  runs: RunSummary[];
  runsTruncated: boolean;
  runsNextOffset: number | null;
  loadingMoreRuns: boolean;
  state: RunState | null;
  events: RunEvent[];
  blueprint: Blueprint | null;
  blueprintPath: string | null;
  workers: TemporaryWorker[];
  workerTelemetry: Record<string, TemporaryWorkerTelemetry>;
  scrubIndex: number;
  loadRuns: () => Promise<void>;
  loadMoreRuns: () => Promise<void>;
  openRun: (blueprintId: string, runId: string) => Promise<void>;
  clearOpenRun: () => void;
  setScrubIndex: (index: number) => void;
  currentNodeStatuses: () => Record<string, NodeStatusKind>;
  reset: () => void;
}

const initialState = {
  runs: [],
  runsTruncated: false,
  runsNextOffset: null,
  loadingMoreRuns: false,
  state: null,
  events: [],
  blueprint: null,
  blueprintPath: null,
  workers: [],
  workerTelemetry: {},
  scrubIndex: 0,
};

let loadRunsInFlight: Promise<void> | null = null;

export const useRunStore = create<RunStoreState>((set, get) => ({
  ...initialState,
  loadRuns() {
    if (loadRunsInFlight) return loadRunsInFlight;

    const request = (async () => {
      const result = await invoke<RunSummaryListResult>('automation_list_runs');
      const runs = result.runs;
      const runsTruncated = result.truncated;
      const runsNextOffset = result.next_offset ?? null;
      if (runSummariesEqual(get().runs, runs) && get().runsTruncated === runsTruncated && get().runsNextOffset === runsNextOffset) return;
      set({ runs, runsTruncated, runsNextOffset });
    })();
    loadRunsInFlight = request;
    void request.then(
      () => {
        if (loadRunsInFlight === request) loadRunsInFlight = null;
      },
      () => {
        if (loadRunsInFlight === request) loadRunsInFlight = null;
      },
    );
    return request;
  },
  async loadMoreRuns() {
    const offset = get().runsNextOffset;
    if (offset === null || get().loadingMoreRuns) return;
    set({ loadingMoreRuns: true });
    try {
      const result = await invoke<RunSummaryListResult>('automation_list_runs', { offset });
      set((state) => {
        const byKey = new Map(state.runs.map((run) => [`${run.blueprint_id}:${run.run_id}`, run]));
        for (const run of result.runs) byKey.set(`${run.blueprint_id}:${run.run_id}`, run);
        return {
          runs: [...byKey.values()],
          runsTruncated: result.truncated,
          runsNextOffset: result.next_offset ?? null,
        };
      });
    } finally {
      set({ loadingMoreRuns: false });
    }
  },
  async openRun(blueprintId, runId) {
    const result = await invoke<RunReadResult>('automation_read_run', { blueprintId, runId });
    const events = result.events ?? [];
    const summaryPath = get().runs.find((run) => run.blueprint_id === blueprintId && run.run_id === runId)?.blueprint_path;
    set({
      state: result.state,
      events,
      blueprint: result.blueprint,
      blueprintPath: result.blueprint_path ?? summaryPath ?? null,
      workers: result.workers ?? [],
      workerTelemetry: result.worker_telemetry ?? {},
      scrubIndex: Math.max(0, events.length - 1),
    });
  },
  clearOpenRun() {
    set({
      state: null,
      events: [],
      blueprint: null,
      blueprintPath: null,
      workers: [],
      workerTelemetry: {},
      scrubIndex: 0,
    });
  },
  setScrubIndex(index) {
    const last = Math.max(0, get().events.length - 1);
    const scrubIndex = Math.min(Math.max(0, index), last);
    set({ scrubIndex });
  },
  currentNodeStatuses() {
    return nodeStatusesAt(get().events, get().scrubIndex, get().blueprint);
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
    && left.schedule_id === right.schedule_id
    && left.worker_attention_count === right.worker_attention_count;
}
