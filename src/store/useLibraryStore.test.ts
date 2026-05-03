import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useLibraryStore } from './useLibraryStore';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

// Mock Tauri invoke
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

const mockListen = vi.mocked(listen);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flushPromises() {
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

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
    mockListen.mockResolvedValue(vi.fn());
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

  it('ignores stale successful skill fetches that resolve after a newer request', async () => {
    const oldTree = { type: 'Folder', path: '', name: 'Old', children: [] };
    const newTree = { type: 'Folder', path: '', name: 'New', children: [] };
    const first = deferred<typeof oldTree>();
    const second = deferred<typeof newTree>();
    vi.mocked(invoke)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const firstFetch = useLibraryStore.getState().fetchLibraryTree('skills');
    const secondFetch = useLibraryStore.getState().fetchLibraryTree('skills');

    second.resolve(newTree);
    await secondFetch;
    first.resolve(oldTree);
    await firstFetch;

    expect(useLibraryStore.getState().skillTree).toEqual(newTree);
    expect(useLibraryStore.getState().error).toBeNull();
    expect(useLibraryStore.getState().isLoading).toBe(false);
  });

  it('ignores stale failed skill fetches that resolve after a newer success', async () => {
    const newTree = { type: 'Folder', path: '', name: 'New', children: [] };
    const first = deferred<never>();
    const second = deferred<typeof newTree>();
    vi.mocked(invoke)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const firstFetch = useLibraryStore.getState().fetchLibraryTree('skills');
    const secondFetch = useLibraryStore.getState().fetchLibraryTree('skills');

    second.resolve(newTree);
    await secondFetch;
    first.reject(new Error('old failure'));
    await firstFetch;

    expect(useLibraryStore.getState().skillTree).toEqual(newTree);
    expect(useLibraryStore.getState().error).toBeNull();
    expect(useLibraryStore.getState().isLoading).toBe(false);
  });

  it('shares one backend watch and one listener across multiple skill subscribers', async () => {
    const unlisten = vi.fn();
    let handler: ((event: { payload: { library_type: string } }) => void) | undefined;
    mockListen.mockImplementation(async (_event, cb) => {
      handler = cb as typeof handler;
      return unlisten;
    });
    const mockTree = { type: 'Folder', path: '', name: 'Skills', children: [] };
    vi.mocked(invoke).mockResolvedValue(mockTree);

    const cleanupOne = useLibraryStore.getState().subscribeToLibraryChanges('skills');
    const cleanupTwo = useLibraryStore.getState().subscribeToLibraryChanges('skills');
    await flushPromises();

    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenCalledWith('library_watch', { libraryType: 'skills' });
    expect(invoke).toHaveBeenCalledWith('get_library_tree', { libraryType: 'skills' });
    expect(mockListen).toHaveBeenCalledTimes(1);

    handler?.({ payload: { library_type: 'prompts' } });
    await flushPromises();
    expect(invoke).toHaveBeenCalledTimes(2);

    handler?.({ payload: { library_type: 'skills' } });
    await flushPromises();
    expect(invoke).toHaveBeenCalledTimes(3);
    expect(invoke).toHaveBeenLastCalledWith('get_library_tree', { libraryType: 'skills' });

    cleanupOne();
    await flushPromises();
    expect(unlisten).not.toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledTimes(3);

    cleanupTwo();
    await flushPromises();
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledTimes(4);
    expect(invoke).toHaveBeenLastCalledWith('library_unwatch', { libraryType: 'skills' });
  });

  it('establishes the library event listener before starting the backend watch', async () => {
    const unlisten = vi.fn();
    const listenReady = deferred<typeof unlisten>();
    mockListen.mockReturnValue(listenReady.promise);
    vi.mocked(invoke).mockResolvedValue({ type: 'Folder', path: '', name: 'Skills', children: [] });

    const cleanup = useLibraryStore.getState().subscribeToLibraryChanges('skills');
    await flushPromises();

    expect(mockListen).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalledWith('library_watch', { libraryType: 'skills' });

    listenReady.resolve(unlisten);
    await flushPromises();

    expect(invoke).toHaveBeenNthCalledWith(1, 'library_watch', { libraryType: 'skills' });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_tree', { libraryType: 'skills' });

    cleanup();
  });
});
