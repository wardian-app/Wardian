import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import React from 'react';
import { useGridResize } from './useGridResize';
import { useLayoutStore } from '../../store/useLayoutStore';

const makeContainer = (width = 1000) => {
  const el = document.createElement('div');
  Object.defineProperty(el, 'getBoundingClientRect', {
    value: () => ({ left: 0, top: 0, right: width, bottom: 600, width, height: 600, x: 0, y: 0, toJSON: () => '' }),
  });
  Object.defineProperty(el, 'clientWidth', { value: width });
  return el;
};

const makeOffsetContainer = (width = 1000, left = 24, top = 120) => {
  const el = document.createElement('div');
  Object.defineProperty(el, 'getBoundingClientRect', {
    value: () => ({
      left,
      top,
      right: left + width,
      bottom: top + 600,
      width,
      height: 600,
      x: left,
      y: top,
      toJSON: () => '',
    }),
  });
  Object.defineProperty(el, 'clientWidth', { value: width });
  return el;
};

describe('useLayoutStore — gridStacked basics', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('gridStacked defaults to false and previousColumnTracks to null', () => {
    expect(useLayoutStore.getState().gridStacked).toBe(false);
    expect(useLayoutStore.getState().previousColumnTracks).toBeNull();
  });

  it('resetLayout clears gridStacked and previousColumnTracks', () => {
    act(() => {
      useLayoutStore.getState().setGridStacked(true);
      useLayoutStore.getState().setPreviousColumnTracks([0.7, 0.3]);
    });
    act(() => useLayoutStore.getState().resetLayout());
    expect(useLayoutStore.getState().gridStacked).toBe(false);
    expect(useLayoutStore.getState().previousColumnTracks).toBeNull();
  });
});

describe('useGridResize — stacked entry', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('enters stacked mode when a horizontal drag releases past 2/3 of container', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('h', 0));
    // Move mouse to x=800 → globalWeight 0.8 → first track exceeds 2/3
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 800, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(true);
  });

  it('saves prior column_tracks as previousColumnTracks on entry and restores tracks', () => {
    act(() => useLayoutStore.getState().setColumnTracks([0.4, 0.6]));
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('h', 0));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 850, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(true);
    expect(useLayoutStore.getState().previousColumnTracks).toEqual([0.4, 0.6]);
    expect(useLayoutStore.getState().layout.column_tracks).toEqual([0.4, 0.6]);
  });

  it('does not enter stacked when drag stays below 2/3', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('h', 0));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 500, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(false);
  });

  it('enters stacked when neighbor is squeezed below 1/3 (active drag leftward)', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('h', 0));
    // Drag gutter way to the left → first track tiny, second track exceeds 2/3.
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 100, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(true);
  });
});

describe('useGridResize — stacked exit', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
    act(() => {
      useLayoutStore.getState().setGridStacked(true);
      useLayoutStore.getState().setPreviousColumnTracks([0.5, 0.5]);
    });
  });

  it('exits stacked when a stack-exit drag releases below 2/3 and restores prior tracks', () => {
    act(() => useLayoutStore.getState().setColumnTracks([1]));
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('stack-exit', 0));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 400, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(false);
    expect(useLayoutStore.getState().layout.column_tracks).toEqual([0.5, 0.5]);
    expect(useLayoutStore.getState().previousColumnTracks).toBeNull();
  });

  it('stays stacked when a stack-exit drag releases at or above 2/3', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('stack-exit', 0));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 800, clientY: 0 })));
    act(() => window.dispatchEvent(new MouseEvent('mouseup')));

    expect(useLayoutStore.getState().gridStacked).toBe(true);
  });
});

describe('useGridResize — row resizing', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('reports horizontal guide positions relative to the grid container', () => {
    const ref = { current: makeOffsetContainer(1000, 24, 120) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('v', 0));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 0, clientY: 570 })));

    expect(result.current.guidePos).toBe(450);
  });

  it('treats vertical resize indexes as row boundaries rather than agent indexes', () => {
    const ref = { current: makeContainer(1000) } as React.RefObject<HTMLDivElement>;
    const { result } = renderHook(() => useGridResize(ref));

    act(() => result.current.startResize('v', 1));
    act(() => window.dispatchEvent(new MouseEvent('mousemove', { clientX: 0, clientY: 900 })));

    expect(useLayoutStore.getState().layout.row_height).toBe(450);
  });
});
