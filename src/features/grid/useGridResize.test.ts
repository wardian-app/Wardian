import { describe, it, expect, beforeEach } from 'vitest';
import { useLayoutStore } from '../../store/useLayoutStore';

describe('useGridResize — stacked mode', () => {
  beforeEach(() => {
    localStorage.clear();
    useLayoutStore.getState().resetLayout();
  });

  it('gridStacked defaults to false', () => {
    expect(useLayoutStore.getState().gridStacked).toBe(false);
  });

  it('setGridStacked updates state', () => {
    useLayoutStore.getState().setGridStacked(true);
    expect(useLayoutStore.getState().gridStacked).toBe(true);

    useLayoutStore.getState().setGridStacked(false);
    expect(useLayoutStore.getState().gridStacked).toBe(false);
  });

  it('resetLayout clears gridStacked', () => {
    useLayoutStore.getState().setGridStacked(true);
    expect(useLayoutStore.getState().gridStacked).toBe(true);

    useLayoutStore.getState().resetLayout();
    expect(useLayoutStore.getState().gridStacked).toBe(false);
  });
});
