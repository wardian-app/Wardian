import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { LibraryFolder, LibraryItemMetadata } from '../types';

interface LibraryState {
  libraryTree: LibraryFolder | null;
  isLoading: boolean;
  error: string | null;
  fetchLibraryTree: () => Promise<void>;
  savePrompt: (path: string, content: string, metadata: LibraryItemMetadata) => Promise<void>;
  updatePromptMetadata: (path: string, metadata: LibraryItemMetadata) => Promise<void>;
}

export const useLibraryStore = create<LibraryState>((set, get) => ({
  libraryTree: null,
  isLoading: false,
  error: null,

  fetchLibraryTree: async () => {
    set({ isLoading: true, error: null });
    try {
      const tree = await invoke<LibraryFolder>('get_library_tree');
      set({ libraryTree: tree, isLoading: false });
    } catch (e: any) {
      set({ error: e.toString(), isLoading: false });
    }
  },

  savePrompt: async (path: string, content: string, metadata: LibraryItemMetadata) => {
    try {
      await invoke('save_prompt', { path, content, metadata });
      await get().fetchLibraryTree();
    } catch (e: any) {
      console.error("Failed to save prompt:", e);
      throw e;
    }
  },

  updatePromptMetadata: async (path: string, metadata: LibraryItemMetadata) => {
    try {
      await invoke('update_prompt_metadata', { path, metadata });
      await get().fetchLibraryTree();
    } catch (e: any) {
      console.error("Failed to update metadata:", e);
      throw e;
    }
  }
}));
