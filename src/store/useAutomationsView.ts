import { create } from 'zustand';

export type AutomationsMode = 'edit' | 'observe' | 'monitor';

interface AutomationsViewState {
  mode: AutomationsMode;
  agentScopeEnabled: boolean;
  blueprintPath: string | null;
  selectedRunId: string | null;
  observedBlueprintId: string | null;
  selectedRunIdsByBlueprint: Record<string, string>;
  setMode: (mode: AutomationsMode) => void;
  setAgentScopeEnabled: (enabled: boolean) => void;
  setBlueprintPath: (path: string | null) => void;
  observeRun: (blueprintId: string, runId: string) => void;
  clearObservedRun: (blueprintId?: string) => void;
  reset: () => void;
}

export const useAutomationsView = create<AutomationsViewState>((set) => ({
  mode: 'monitor',
  agentScopeEnabled: true,
  blueprintPath: null,
  selectedRunId: null,
  observedBlueprintId: null,
  selectedRunIdsByBlueprint: {},
  setMode: (mode) => set({ mode }),
  setAgentScopeEnabled: (agentScopeEnabled) => set({ agentScopeEnabled }),
  setBlueprintPath: (blueprintPath) => set({ blueprintPath }),
  observeRun: (blueprintId, runId) => set((state) => ({
    mode: 'observe',
    selectedRunId: runId,
    observedBlueprintId: blueprintId,
    selectedRunIdsByBlueprint: {
      ...state.selectedRunIdsByBlueprint,
      [blueprintId]: runId,
    },
  })),
  clearObservedRun: (blueprintId) => set((state) => {
    if (!blueprintId) {
      return { selectedRunId: null, observedBlueprintId: null };
    }
    const { [blueprintId]: _removed, ...selectedRunIdsByBlueprint } = state.selectedRunIdsByBlueprint;
    return {
      selectedRunId: null,
      observedBlueprintId: state.observedBlueprintId === blueprintId ? null : state.observedBlueprintId,
      selectedRunIdsByBlueprint,
    };
  }),
  reset: () => set({
    mode: 'monitor',
    agentScopeEnabled: true,
    blueprintPath: null,
    selectedRunId: null,
    observedBlueprintId: null,
    selectedRunIdsByBlueprint: {},
  }),
}));
