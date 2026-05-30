import { beforeEach, describe, expect, it } from 'vitest';

import { useWorkflowsView } from './useWorkflowsView';

describe('useWorkflowsView', () => {
  beforeEach(() => useWorkflowsView.getState().reset());

  it('defaults to edit mode with nothing selected', () => {
    const s = useWorkflowsView.getState();

    expect(s.mode).toBe('edit');
    expect(s.blueprintPath).toBeNull();
    expect(s.selectedRunId).toBeNull();
  });

  it('switches mode', () => {
    useWorkflowsView.getState().setMode('observe');

    expect(useWorkflowsView.getState().mode).toBe('observe');
  });

  it('opening a run sets observe mode and the selected run', () => {
    useWorkflowsView.getState().observeRun('run-1');

    expect(useWorkflowsView.getState().mode).toBe('observe');
    expect(useWorkflowsView.getState().selectedRunId).toBe('run-1');
  });

  it('sets the active blueprint path', () => {
    useWorkflowsView.getState().setBlueprintPath('/x/wf.md');

    expect(useWorkflowsView.getState().blueprintPath).toBe('/x/wf.md');
  });
});
