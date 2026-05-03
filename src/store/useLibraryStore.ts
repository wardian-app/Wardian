import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { DeployedSkillRef, LibraryFolder, LibraryItemMetadata } from '../types';

type LibraryType = 'prompts' | 'skills';
type WatchableLibraryType = 'skills';

interface LibraryChangedEvent {
  library_type: WatchableLibraryType;
}

interface LibrarySubscription {
  refCount: number;
  disposed: boolean;
  unlisten?: () => void;
  listenPromise?: Promise<() => void>;
}

const fetchRequestIds: Record<LibraryType, number> = { prompts: 0, skills: 0 };
const librarySubscriptions: Partial<Record<WatchableLibraryType, LibrarySubscription>> = {};

function releaseLibrarySubscription(type: WatchableLibraryType) {
  const current = librarySubscriptions[type];
  if (!current) return;
  current.refCount = Math.max(0, current.refCount - 1);
  if (current.refCount > 0) return;

  current.disposed = true;
  const unlisten = current.unlisten;
  current.unlisten = undefined;
  if (unlisten) {
    unlisten();
  } else {
    current.listenPromise
      ?.then((lateUnlisten) => lateUnlisten())
      .catch((error) => {
        console.error('Failed to release library listener:', error);
      });
  }
  delete librarySubscriptions[type];
  void invoke('library_unwatch', { libraryType: type });
}

interface LibraryState {
  promptTree: LibraryFolder | null;
  skillTree: LibraryFolder | null;
  isLoading: boolean;
  error: string | null;
  activeTab: LibraryType;
  setActiveTab: (tab: LibraryType) => void;
  fetchLibraryTree: (type?: LibraryType) => Promise<void>;
  subscribeToLibraryChanges: (type: WatchableLibraryType) => () => void;
  saveLibraryItem: (path: string, content: string, metadata: LibraryItemMetadata) => Promise<void>;
  updateLibraryMetadata: (path: string, metadata: LibraryItemMetadata) => Promise<void>;
  openLibraryFolder: (path?: string) => Promise<void>;
  deploySkill: (sourcePath: string, targetType: "agent" | "class" | "user", targetId: string) => Promise<void>;
  removeDeployedSkill: (targetType: "agent" | "class" | "user", targetId: string, skillName: string) => Promise<void>;
  listDeployedSkills: (targetType: "agent" | "class" | "user", targetId: string) => Promise<string[]>;
  listDeployedSkillRefs: (targetType: "agent" | "class" | "user", targetId: string) => Promise<DeployedSkillRef[]>;
  listSkillDeployments: (skillName: string, sourcePath?: string) => Promise<{ target_type: string; target_id: string }[]>;
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
    const requestId = fetchRequestIds[targetType] + 1;
    fetchRequestIds[targetType] = requestId;
    set({ isLoading: true, error: null });
    try {
      const tree = await invoke<LibraryFolder>('get_library_tree', { libraryType: targetType });
      if (fetchRequestIds[targetType] !== requestId) {
        return;
      }
      if (targetType === 'prompts') {
        set({ promptTree: tree, isLoading: false });
      } else {
        set({ skillTree: tree, isLoading: false });
      }
    } catch (e: any) {
      if (fetchRequestIds[targetType] !== requestId) {
        return;
      }
      set({ error: e.toString(), isLoading: false });
    }
  },

  subscribeToLibraryChanges: (type) => {
    const existing = librarySubscriptions[type];
    if (existing && !existing.disposed) {
      existing.refCount += 1;
      return () => releaseLibrarySubscription(type);
    }

    const subscription: LibrarySubscription = {
      refCount: 1,
      disposed: false,
    };
    librarySubscriptions[type] = subscription;

    subscription.listenPromise = listen<LibraryChangedEvent>('library-changed', (event) => {
      if (event.payload.library_type === type) {
        void get().fetchLibraryTree(type);
      }
    }).then((unlisten) => {
      if (subscription.disposed) {
        unlisten();
      } else {
        subscription.unlisten = unlisten;
      }
      return unlisten;
    });

    void subscription.listenPromise
      .then(() => {
        if (subscription.disposed) return;
        return invoke('library_watch', { libraryType: type });
      })
      .catch((error) => {
        console.error('Failed to watch library:', error);
      })
      .finally(() => {
        if (!subscription.disposed) {
          void get().fetchLibraryTree(type);
        }
      });

    return () => releaseLibrarySubscription(type);
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
    const deployedSkills = await invoke<string[]>('list_deployed_skills', { targetType, targetId });
    return Array.isArray(deployedSkills) ? deployedSkills : [];
  },

  listDeployedSkillRefs: async (targetType, targetId) => {
    const deployedSkillRefs = await invoke<DeployedSkillRef[]>('list_deployed_skill_refs', { targetType, targetId });
    return Array.isArray(deployedSkillRefs) ? deployedSkillRefs : [];
  },

  listSkillDeployments: async (skillName, sourcePath) => {
    return await invoke<{ target_type: string; target_id: string }[]>('list_skill_deployments', { skillName, sourcePath });
  }
}));
