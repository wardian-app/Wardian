import { beforeEach, describe, expect, it } from 'vitest';

import { useWorkflowsView } from './useWorkflowsView';

describe('useWorkflowsView', () => {
  beforeEach(() => useWorkflowsView.getState().reset());

  it('defaults to edit mode with nothing selected', () => {
    const s = useWorkflowsView.getState();

    expect(s.mode).toBe('edit');
    expect(s.blueprintPath).toBeNull();
    expect(s.selectedRunId).toBeNull();
    expect(s.observedBlueprintId).toBeNull();
    expect(s.selectedRunIdsByBlueprint).toEqual({});
  });

  it('switches mode', () => {
    useWorkflowsView.getState().setMode('observe');

    expect(useWorkflowsView.getState().mode).toBe('observe');
  });

  it('opening a run sets observe mode and remembers the run for that blueprint', () => {
    useWorkflowsView.getState().observeRun('wf', 'run-1');

    expect(useWorkflowsView.getState().mode).toBe('observe');
    expect(useWorkflowsView.getState().selectedRunId).toBe('run-1');
    expect(useWorkflowsView.getState().observedBlueprintId).toBe('wf');
    expect(useWorkflowsView.getState().selectedRunIdsByBlueprint).toEqual({ wf: 'run-1' });
  });

  it('clears the active observe run without forgetting other blueprint selections', () => {
    useWorkflowsView.getState().observeRun('wf', 'run-1');
    useWorkflowsView.getState().observeRun('other', 'run-2');

    useWorkflowsView.getState().clearObservedRun('wf');

    expect(useWorkflowsView.getState().selectedRunId).toBeNull();
    expect(useWorkflowsView.getState().observedBlueprintId).toBe('other');
    expect(useWorkflowsView.getState().selectedRunIdsByBlueprint).toEqual({ other: 'run-2' });
  });

  it('sets the active blueprint path', () => {
    useWorkflowsView.getState().setBlueprintPath('/x/wf.md');

    expect(useWorkflowsView.getState().blueprintPath).toBe('/x/wf.md');
  });
});
