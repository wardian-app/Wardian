import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { LibraryFolder, LibraryItemMetadata } from '../types';

interface LibraryState {
  promptTree: LibraryFolder | null;
  skillTree: LibraryFolder | null;
  isLoading: boolean;
  error: string | null;
  activeTab: 'prompts' | 'skills';
  setActiveTab: (tab: 'prompts' | 'skills') => void;
  fetchLibraryTree: (type?: 'prompts' | 'skills') => Promise<void>;
  saveLibraryItem: (path: string, content: string, metadata: LibraryItemMetadata) => Promise<void>;
  updateLibraryMetadata: (path: string, metadata: LibraryItemMetadata) => Promise<void>;
  openLibraryFolder: (path?: string) => Promise<void>;
  deploySkill: (sourcePath: string, targetType: "agent" | "class" | "user", targetId: string) => Promise<void>;
  removeDeployedSkill: (targetType: "agent" | "class" | "user", targetId: string, skillName: string) => Promise<void>;
  listDeployedSkills: (targetType: "agent" | "class" | "user", targetId: string) => Promise<string[]>;
  listSkillDeployments: (skillName: string) => Promise<{ target_type: string; target_id: string }[]>;
}

export const useLibraryStore = create<LibraryState>((set, get) => ({
  promptTree: null,
  skillTree: null,
  isLoading: false,
  error: null,
  activeTab: 'prompts',

  setActiveTab: (tab) => {
    set({ activeTab: tab });
    get().fetchLibraryTree(tab);
  },

  fetchLibraryTree: async (type) => {
    const targetType = type || get().activeTab;
    set({ isLoading: true, error: null });
    try {
      const tree = await invoke<LibraryFolder>('get_library_tree', { libraryType: targetType });
      if (targetType === 'prompts') {
        set({ promptTree: tree, isLoading: false });
      } else {
        set({ skillTree: tree, isLoading: false });
      }
    } catch (e: any) {
      set({ error: e.toString(), isLoading: false });
    }
  },

  saveLibraryItem: async (path: string, content: string, metadata: LibraryItemMetadata) => {
    try {
      await invoke('save_library_item', { libraryType: get().activeTab, path, content, metadata });
      await get().fetchLibraryTree();
    } catch (e: any) {
      console.error("Failed to save item:", e);
      throw e;
    }
  },

  updateLibraryMetadata: async (path: string, metadata: LibraryItemMetadata) => {
    try {
      await invoke('update_library_metadata', { path, metadata });
      await get().fetchLibraryTree();
    } catch (e: any) {
      console.error("Failed to update metadata:", e);
      throw e;
    }
  },

  openLibraryFolder: async (path?: string) => {
    try {
      await invoke('open_library_folder', { libraryType: get().activeTab, path });
    } catch (e: any) {
      console.error("Failed to open folder:", e);
    }
  },

  deploySkill: async (sourcePath, targetType, targetId) => {
    await invoke('deploy_skill', { sourcePath, targetType, targetId });
  },

  removeDeployedSkill: async (targetType, targetId, skillName) => {
    await invoke('remove_deployed_skill', { targetType, targetId, skillName });
  },

  listDeployedSkills: async (targetType, targetId) => {
    return await invoke<string[]>('list_deployed_skills', { targetType, targetId });
  },

  listSkillDeployments: async (skillName) => {
    return await invoke<{ target_type: string; target_id: string }[]>('list_skill_deployments', { skillName });
  }
}));
