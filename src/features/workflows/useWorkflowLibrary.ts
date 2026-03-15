import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface WorkflowFolder {
  id: string;
  name: string;
  workflowIds: string[];
  isCollapsed: boolean;
}

export interface WorkflowLibraryState {
  folders: WorkflowFolder[];
  rootWorkflowIds: string[];
}

const DEFAULT_STATE: WorkflowLibraryState = {
  folders: [],
  rootWorkflowIds: [],
};

export function useWorkflowLibrary() {
  const [state, setState] = useState<WorkflowLibraryState>(DEFAULT_STATE);
  const [loading, setLoading] = useState(true);

  const loadLibrary = useCallback(async () => {
    try {
      setLoading(true);
      // We expect the backend to provide the library structure or we derive it from workflows.json
      // For now, we'll try to invoke a command that might exist or we'll manage it via a shared state file
      const data = await invoke<WorkflowLibraryState>('load_workflow_library').catch(() => {
        console.warn('load_workflow_library not implemented, using local defaults');
        return DEFAULT_STATE;
      });
      setState(data);
    } catch (err) {
      console.error('Failed to load workflow library:', err);
    } finally {
      setLoading(false);
    }
  }, []);

  const saveLibrary = useCallback(async (newState: WorkflowLibraryState) => {
    try {
      await invoke('save_workflow_library', { state: newState });
    } catch (err) {
      console.error('Failed to save workflow library:', err);
    }
  }, []);

  useEffect(() => {
    loadLibrary();
  }, [loadLibrary]);

  const addFolder = async (name: string) => {
    const newFolder: WorkflowFolder = {
      id: `folder-${Date.now()}`,
      name,
      workflowIds: [],
      isCollapsed: false,
    };
    const nextState = {
      ...state,
      folders: [...state.folders, newFolder],
    };
    setState(nextState);
    await saveLibrary(nextState);
  };

  const toggleFolderCollapse = async (folderId: string) => {
    const nextState = {
      ...state,
      folders: state.folders.map(f => 
        f.id === folderId ? { ...f, isCollapsed: !f.isCollapsed } : f
      ),
    };
    setState(nextState);
    await saveLibrary(nextState);
  };

  const moveWorkflowToFolder = async (workflowId: string, folderId: string | null, index?: number) => {
    // Remove from everywhere first
    let nextFolders = state.folders.map(f => ({
      ...f,
      workflowIds: f.workflowIds.filter(id => id !== workflowId)
    }));
    let nextRootIds = state.rootWorkflowIds.filter(id => id !== workflowId);

    if (folderId) {
      nextFolders = nextFolders.map(f => {
        if (f.id === folderId) {
          const newIds = [...f.workflowIds];
          if (typeof index === 'number' && index >= 0) {
            newIds.splice(Math.min(index, newIds.length), 0, workflowId);
          } else {
            newIds.push(workflowId);
          }
          return { ...f, workflowIds: newIds };
        }
        return f;
      });
    } else {
      if (typeof index === 'number' && index >= 0) {
        nextRootIds.splice(Math.min(index, nextRootIds.length), 0, workflowId);
      } else {
        nextRootIds.push(workflowId);
      }
    }

    const nextState = {
      folders: nextFolders,
      rootWorkflowIds: nextRootIds,
    };
    setState(nextState);
    await saveLibrary(nextState);
  };

  const renameFolder = async (folderId: string, name: string) => {
    const nextState = {
      ...state,
      folders: state.folders.map(f => 
        f.id === folderId ? { ...f, name } : f
      ),
    };
    setState(nextState);
    await saveLibrary(nextState);
  };

  const deleteFolder = async (folderId: string) => {
    const folder = state.folders.find(f => f.id === folderId);
    if (!folder) return;

    // Move all workflows in this folder back to root
    const nextState = {
      folders: state.folders.filter(f => f.id !== folderId),
      rootWorkflowIds: [...state.rootWorkflowIds, ...folder.workflowIds],
    };
    setState(nextState);
    await saveLibrary(nextState);
  };

  return {
    ...state,
    loading,
    addFolder,
    renameFolder,
    deleteFolder,
    toggleFolderCollapse,
    moveWorkflowToFolder,
    refresh: loadLibrary,
  };
}
