import { create } from 'zustand';

export type WorkflowsMode = 'edit' | 'observe';

interface WorkflowsViewState {
  mode: WorkflowsMode;
  blueprintPath: string | null;
  selectedRunId: string | null;
  setMode: (mode: WorkflowsMode) => void;
  setBlueprintPath: (path: string | null) => void;
  observeRun: (runId: string) => void;
  reset: () => void;
}

export const useWorkflowsView = create<WorkflowsViewState>((set) => ({
  mode: 'edit',
  blueprintPath: null,
  selectedRunId: null,
  setMode: (mode) => set({ mode }),
  setBlueprintPath: (blueprintPath) => set({ blueprintPath }),
  observeRun: (runId) => set({ mode: 'observe', selectedRunId: runId }),
  reset: () => set({ mode: 'edit', blueprintPath: null, selectedRunId: null }),
}));
