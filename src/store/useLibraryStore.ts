import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  LibraryIndex,
  LibraryItemMetadata,
  LibrarySectionId,
  OrphanDeployment,
  SkillDeployment,
} from '../types';

// The backend only exposes a single logical watch type, `"library"`, which
// covers everything under `library/` (skills, prompts, workflows) plus
// `classes/` (see src-tauri/src/commands/library.rs). The `library_type`
// param is kept on the wire for interface stability, but only `"library"` is
// ever sent or expected here.
const LIBRARY_WATCH_TYPE = 'library';

interface LibraryChangedEvent {
  library_type: string;
}

interface LibrarySubscription {
  refCount: number;
  disposed: boolean;
  unlisten?: () => void;
  listenPromise?: Promise<() => void>;
}

let librarySubscription: LibrarySubscription | null = null;

function releaseLibrarySubscription() {
  const current = librarySubscription;
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
  librarySubscription = null;
  void invoke('library_unwatch', { libraryType: LIBRARY_WATCH_TYPE });
}

function parseEntryRef(entryRef: string): { section: LibrarySectionId; path: string } {
  const parts = entryRef.split('/');
  const section = parts[0] as LibrarySectionId;
  const path = parts.slice(1).join('/');
  return { section, path };
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

interface LibraryState {
  index: LibraryIndex | null;
  isLoading: boolean;
  error: string | null;
  activeSection: LibrarySectionId;
  selection: { section: LibrarySectionId; entryRef: string } | null;
  expandedFolders: Set<string>;
  searchQuery: string;
  showStarredOnly: boolean;
  selectedContent: string | null;
  contentStale: boolean;
  /** Tracks the dirty flag from the most recent `select` call, so a
   * `library-changed` event knows whether it's safe to silently reload
   * `selectedContent` or whether it must instead flag `contentStale` and
   * let the editor decide when to reload. Internal to this store. */
  _editorDirty: boolean;
  setActiveSection: (s: LibrarySectionId) => void;
  select: (entryRef: string | null, opts?: { editorDirty?: boolean }) => Promise<void>;
  /** Cheap way for a future editor to flag dirty state without forcing a
   * redundant `read_library_item` round-trip via `select()`. */
  markEditorDirty: (dirty: boolean) => void;
  /** Reverts `selection` back to a previously-tracked entry WITHOUT
   * re-reading its content from disk (unlike `select()`), and marks the
   * editor dirty again. Used when the caller declines a discard-changes
   * prompt: the in-progress draft for that entry must survive untouched, so
   * `selectedContent` is intentionally left alone. */
  revertSelection: (entryRef: string) => void;
  /** Clears `contentStale` without touching `selectedContent`/the draft.
   * Used when the user resolves a stale-content conflict in favor of their
   * local draft ("Keep mine"), and again after a successful save so the
   * store doesn't linger in a stale state once the draft matches disk. */
  resolveStale: () => void;
  toggleFolder: (key: string) => void;
  setSearchQuery: (q: string) => void;
  setShowStarredOnly: (v: boolean) => void;
  fetchIndex: () => Promise<void>;
  subscribeToLibraryChanges: () => () => void;
  reloadSelectedContent: () => Promise<void>;
  saveItem: (section: LibrarySectionId, path: string, content: string) => Promise<void>;
  updateMetadata: (entryRef: string, metadata: LibraryItemMetadata) => Promise<void>;
  createFolder: (section: LibrarySectionId, path: string) => Promise<void>;
  renameEntry: (section: LibrarySectionId, fromPath: string, toPath: string) => Promise<void>;
  deleteEntry: (section: LibrarySectionId, path: string) => Promise<void>;
  setSkillDeployments: (sourcePath: string, targets: SkillDeployment[]) => Promise<void>;
  removeOrphan: (o: OrphanDeployment) => Promise<void>;
  openLibraryFolder: (section: LibrarySectionId, path?: string) => Promise<void>;
}

export const useLibraryStore = create<LibraryState>((set, get) => ({
  index: null,
  isLoading: false,
  error: null,
  activeSection: 'skills',
  selection: null,
  expandedFolders: new Set<string>(),
  searchQuery: '',
  showStarredOnly: false,
  selectedContent: null,
  contentStale: false,
  _editorDirty: false,

  setActiveSection: (s) => set({ activeSection: s }),

  select: async (entryRef, opts) => {
    const editorDirty = opts?.editorDirty ?? false;
    set({ _editorDirty: editorDirty });

    if (entryRef === null) {
      set({ selection: null, selectedContent: null, contentStale: false });
      return;
    }

    const { section, path } = parseEntryRef(entryRef);
    set({ selection: { section, entryRef }, contentStale: false });
    try {
      const content = await invoke<string>('read_library_item', { section, path });
      set({ selectedContent: content });
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  markEditorDirty: (dirty) => set({ _editorDirty: dirty }),

  revertSelection: (entryRef) => {
    const { section } = parseEntryRef(entryRef);
    set({ selection: { section, entryRef }, _editorDirty: true });
  },

  resolveStale: () => set({ contentStale: false }),

  toggleFolder: (key) => {
    set((s) => {
      const next = new Set(s.expandedFolders);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return { expandedFolders: next };
    });
  },

  setSearchQuery: (q) => set({ searchQuery: q }),

  setShowStarredOnly: (v) => set({ showStarredOnly: v }),

  fetchIndex: async () => {
    set({ isLoading: true, error: null });
    try {
      const index = await invoke<LibraryIndex>('get_library_index');
      set({ index, isLoading: false });
    } catch (e) {
      set({ error: errorMessage(e), isLoading: false });
    }
  },

  subscribeToLibraryChanges: () => {
    const existing = librarySubscription;
    if (existing && !existing.disposed) {
      existing.refCount += 1;
      return () => releaseLibrarySubscription();
    }

    const subscription: LibrarySubscription = {
      refCount: 1,
      disposed: false,
    };
    librarySubscription = subscription;

    subscription.listenPromise = listen<LibraryChangedEvent>('library-changed', (event) => {
      if (event.payload.library_type !== LIBRARY_WATCH_TYPE) return;

      void get().fetchIndex();

      if (!get().selection) return;
      if (get()._editorDirty) {
        set({ contentStale: true });
      } else {
        void get().reloadSelectedContent();
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
        return invoke('library_watch', { libraryType: LIBRARY_WATCH_TYPE });
      })
      .catch((error) => {
        console.error('Failed to watch library:', error);
      })
      .finally(() => {
        if (!subscription.disposed) {
          void get().fetchIndex();
        }
      });

    return () => releaseLibrarySubscription();
  },

  reloadSelectedContent: async () => {
    const selection = get().selection;
    if (!selection) return;
    const { section, path } = parseEntryRef(selection.entryRef);
    try {
      const content = await invoke<string>('read_library_item', { section, path });
      set({ selectedContent: content, contentStale: false });
    } catch (e) {
      set({ error: errorMessage(e) });
    }
  },

  saveItem: async (section, path, content) => {
    try {
      await invoke('save_library_item', { section, path, content });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to save library item:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  updateMetadata: async (entryRef, metadata) => {
    try {
      await invoke('update_library_metadata', { entryRef, metadata });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to update library metadata:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  createFolder: async (section, path) => {
    try {
      await invoke('create_library_folder', { section, path });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to create library folder:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  renameEntry: async (section, fromPath, toPath) => {
    try {
      await invoke('rename_library_entry', { section, fromPath, toPath });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to rename library entry:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  deleteEntry: async (section, path) => {
    try {
      await invoke('delete_library_entry', { section, path });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to delete library entry:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  setSkillDeployments: async (sourcePath, targets) => {
    try {
      await invoke('set_skill_deployments', { sourcePath, targets });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to set skill deployments:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  removeOrphan: async (o) => {
    try {
      await invoke('remove_orphan_deployment', {
        targetType: o.target_type,
        targetId: o.target_id,
        skillName: o.skill_name,
      });
      await get().fetchIndex();
    } catch (e) {
      console.error('Failed to remove orphan deployment:', e);
      set({ error: errorMessage(e) });
      throw e;
    }
  },

  openLibraryFolder: async (section, path) => {
    try {
      await invoke('open_library_folder', { section, path });
    } catch (e) {
      console.error('Failed to open folder:', e);
    }
  },
}));
