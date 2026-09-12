import { beforeEach, describe, expect, it } from 'vitest';

import { useAutomationsView } from './useAutomationsView';

describe('useAutomationsView', () => {
  beforeEach(() => useAutomationsView.getState().reset());

  it('defaults to monitor mode with nothing selected', () => {
    const s = useAutomationsView.getState();

    expect(s.mode).toBe('monitor');
    expect(s.agentScopeEnabled).toBe(true);
    expect(s.blueprintPath).toBeNull();
    expect(s.selectedRunId).toBeNull();
    expect(s.observedBlueprintId).toBeNull();
    expect(s.selectedRunIdsByBlueprint).toEqual({});
  });

  it('switches mode', () => {
    useAutomationsView.getState().setMode('observe');

    expect(useAutomationsView.getState().mode).toBe('observe');
  });

  it('toggles workflow scoping without changing the selected agents', () => {
    useAutomationsView.getState().setAgentScopeEnabled(false);

    expect(useAutomationsView.getState().agentScopeEnabled).toBe(false);

    useAutomationsView.getState().reset();

    expect(useAutomationsView.getState().agentScopeEnabled).toBe(true);
  });

  it('opening a run sets observe mode and remembers the run for that blueprint', () => {
    useAutomationsView.getState().observeRun('wf', 'run-1');

    expect(useAutomationsView.getState().mode).toBe('observe');
    expect(useAutomationsView.getState().selectedRunId).toBe('run-1');
    expect(useAutomationsView.getState().observedBlueprintId).toBe('wf');
    expect(useAutomationsView.getState().selectedRunIdsByBlueprint).toEqual({ wf: 'run-1' });
  });

  it('clears the active observe run without forgetting other blueprint selections', () => {
    useAutomationsView.getState().observeRun('wf', 'run-1');
    useAutomationsView.getState().observeRun('other', 'run-2');

    useAutomationsView.getState().clearObservedRun('wf');

    expect(useAutomationsView.getState().selectedRunId).toBeNull();
    expect(useAutomationsView.getState().observedBlueprintId).toBe('other');
    expect(useAutomationsView.getState().selectedRunIdsByBlueprint).toEqual({ other: 'run-2' });
  });

  it('sets the active blueprint path', () => {
    useAutomationsView.getState().setBlueprintPath('/x/wf.md');

    expect(useAutomationsView.getState().blueprintPath).toBe('/x/wf.md');
  });
});
