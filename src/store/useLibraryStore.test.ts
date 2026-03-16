import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useLibraryStore } from './useLibraryStore';

// Mock Tauri invoke
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

describe('useLibraryStore', () => {
  beforeEach(() => {
    // Reset store state before each test
    useLibraryStore.setState({
      libraryTree: null,
      isLoading: false,
      error: null,
    });
    vi.clearAllMocks();
  });

  it('should initialize with default state', () => {
    const state = useLibraryStore.getState();
    expect(state.libraryTree).toBeNull();
    expect(state.isLoading).toBe(false);
    expect(state.error).toBeNull();
  });
});
