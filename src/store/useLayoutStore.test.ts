import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { act } from '@testing-library/react';
import { useLayoutStore } from './useLayoutStore';

describe('useLayoutStore — sidebar widths', () => {
  const originalInnerWidth = window.innerWidth;

  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  afterEach(() => {
    Object.defineProperty(window, 'innerWidth', { value: originalInnerWidth, configurable: true });
  });

  it('exposes default sidebar widths', () => {
    const s = useLayoutStore.getState();
    expect(s.leftSidebarWidth).toBe(240);
    expect(s.rightSidebarWidth).toBe(240);
  });

  it('setLeftSidebarWidth clamps below 200px to 200', () => {
    act(() => useLayoutStore.getState().setLeftSidebarWidth(50));
    expect(useLayoutStore.getState().leftSidebarWidth).toBe(200);
  });

  it('setLeftSidebarWidth clamps above 40% of window width', () => {
    Object.defineProperty(window, 'innerWidth', { value: 1000, configurable: true });
    act(() => useLayoutStore.getState().setLeftSidebarWidth(900));
    expect(useLayoutStore.getState().leftSidebarWidth).toBe(400);
  });

  it('setRightSidebarWidth applies the same clamps', () => {
    Object.defineProperty(window, 'innerWidth', { value: 1000, configurable: true });
    act(() => useLayoutStore.getState().setRightSidebarWidth(50));
    expect(useLayoutStore.getState().rightSidebarWidth).toBe(200);
    act(() => useLayoutStore.getState().setRightSidebarWidth(900));
    expect(useLayoutStore.getState().rightSidebarWidth).toBe(400);
  });

  it('resetLayout restores sidebar defaults', () => {
    act(() => {
      useLayoutStore.getState().setLeftSidebarWidth(320);
      useLayoutStore.getState().setRightSidebarWidth(320);
    });
    act(() => useLayoutStore.getState().resetLayout());
    expect(useLayoutStore.getState().leftSidebarWidth).toBe(240);
    expect(useLayoutStore.getState().rightSidebarWidth).toBe(240);
  });

  it('keeps layout state runtime-only with no wardian-layout shadow writer', () => {
    const storageWrite = vi.spyOn(Storage.prototype, 'setItem');
    useLayoutStore.getState().setLeftSidebarWidth(300);
    useLayoutStore.getState().setUserTerminalOpen(true);

    expect(storageWrite).not.toHaveBeenCalled();
    expect(localStorage.getItem('wardian-layout')).toBeNull();
    expect('settingsOpen' in useLayoutStore.getState()).toBe(false);
    expect('libraryDetailWidth' in useLayoutStore.getState()).toBe(false);
    storageWrite.mockRestore();
  });
});
