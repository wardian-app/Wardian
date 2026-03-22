import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useLibraryStore } from './useLibraryStore';
import { invoke } from '@tauri-apps/api/core';

// Mock Tauri invoke
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

describe('useLibraryStore', () => {
  beforeEach(() => {
    // Reset store state before each test
    useLibraryStore.setState({
      promptTree: null,
      skillTree: null,
      isLoading: false,
      error: null,
      activeTab: 'prompts',
    });
    vi.clearAllMocks();
  });

  it('should initialize with default state', () => {
    const state = useLibraryStore.getState();
    expect(state.promptTree).toBeNull();
    expect(state.skillTree).toBeNull();
    expect(state.isLoading).toBe(false);
    expect(state.error).toBeNull();
    expect(state.activeTab).toBe('prompts');
  });

  it('should update activeTab and fetch when setActiveTab is called', async () => {
    const mockTree = { type: 'Folder', path: '', name: 'Root', children: [] };
    vi.mocked(invoke).mockResolvedValueOnce(mockTree);
    
    useLibraryStore.getState().setActiveTab('skills');
    
    const state = useLibraryStore.getState();
    expect(state.activeTab).toBe('skills');
    expect(invoke).toHaveBeenCalledWith('get_library_tree', { libraryType: 'skills' });
    
    // Need to wait for the async fetch in setActiveTab to resolve
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(useLibraryStore.getState().skillTree).toEqual(mockTree);
    expect(useLibraryStore.getState().promptTree).toBeNull();
  });

  it('should fetch to promptTree when activeTab is prompts', async () => {
    const mockTree = { type: 'Folder', path: '', name: 'Root', children: [] };
    vi.mocked(invoke).mockResolvedValueOnce(mockTree);
    
    await useLibraryStore.getState().fetchLibraryTree();
    
    const state = useLibraryStore.getState();
    expect(invoke).toHaveBeenCalledWith('get_library_tree', { libraryType: 'prompts' });
    expect(state.promptTree).toEqual(mockTree);
    expect(state.skillTree).toBeNull();
  });
});
