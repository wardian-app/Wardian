import React, { useState, useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { ExternalLink, FolderOpen } from 'lucide-react';
import { FileTree, FileNode } from './FileTree';
import { useConfirm } from '../../components/ConfirmDialog';
import { AgentConfig, GitStatusResult } from '../../types';
import { useSettingsStore } from '../../store/useSettingsStore';
import { normalizeExplorerPathForCompare } from './pathUtils';
import { createFileSurfaceState, fileResourceKey } from '../files/fileResourceKey';
import type { WorkbenchNavigationService } from '../workbench/navigationService';
import { useAppShellWorkbenchNavigation } from '../../layout/AppShell';

interface ExplorerPanelProps {
  selectedAgentIds: Set<string>;
  agents: AgentConfig[];
  navigation?: WorkbenchNavigationService;
}

interface ExplorerChangedEvent {
  root_path: string;
  changed_paths: string[];
}

const externalEditorLabel = (editor: string) => {
  switch (editor) {
    case 'vscode':
      return 'VS Code';
    case 'custom':
      return 'Custom executable';
    default:
      return 'System default app';
  }
};

export const ExplorerPanel: React.FC<ExplorerPanelProps> = ({ selectedAgentIds, agents, navigation }) => {
  const confirm = useConfirm();
  const appShellNavigation = useAppShellWorkbenchNavigation();
  const workbenchNavigation = navigation ?? appShellNavigation;
  const externalEditor = useSettingsStore((state) => state.externalEditor);
  const externalEditorCustomExecutable = useSettingsStore((state) => state.externalEditorCustomExecutable);
  const explorerFileClickAction = useSettingsStore((state) => state.explorerFileClickAction);
  const [rootPath, setRootPath] = useState<string | null>(null);
  const [gitStatusMap, setGitStatusMap] = useState<Record<string, string>>({});
  const changedDirectories = useMemo(() => {
    const directories = new Set<string>();
    for (const filePath of Object.keys(gitStatusMap)) {
      const segments = filePath.split('/').filter(Boolean);
      if (segments.length <= 1) {
        continue;
      }
      let prefix = '';
      for (let i = 0; i < segments.length - 1; i++) {
        prefix = prefix ? `${prefix}/${segments[i]}` : segments[i];
        directories.add(prefix);
      }
    }
    return directories;
  }, [gitStatusMap]);

  // Context Menu State
  const [menuPos, setMenuPos] = useState<{x: number, y: number} | null>(null);
  const [activeNode, setActiveNode] = useState<FileNode | null>(null);
  
  const [refreshToken, setRefreshToken] = useState(0);
  const [changedPaths, setChangedPaths] = useState<string[]>([]);
  const [externalOpenError, setExternalOpenError] = useState<string | null>(null);

  const selectedAgentId = selectedAgentIds.size === 1 ? Array.from(selectedAgentIds)[0] : null;
  const selectedAgent = agents.find((agent) => agent.session_id === selectedAgentId) ?? null;
  const selectedWorkspaceRevision = [
    selectedAgent?.folder ?? "",
    selectedAgent?.git_worktree ? "worktree" : "main",
    selectedAgent?.git_worktree_source ?? "",
    selectedAgent?.git_worktree_folder ?? "",
  ].join("|");

  useEffect(() => {
    const fetchPath = async () => {
      try {
        const path = await invoke<string>('get_explorer_root', { sessionId: selectedAgentId });
        setRootPath(path);
      } catch (err) {
        console.error("Failed to fetch root path", err);
      }
    };
    fetchPath();
  }, [selectedAgentId, selectedWorkspaceRevision]);

  useEffect(() => {
    if (!rootPath) return;
    let isMounted = true;
    const poll = async () => {
      try {
        const result = await invoke<GitStatusResult>('git_status', { cwd: rootPath });
        if (!isMounted) return;
        const map: Record<string, string> = {};
        for (const f of result.files) {
          const key = f.path.replace(/\\/g, '/');
          if (!map[key] || f.is_staged) map[key] = f.status;
        }
        setGitStatusMap(map);
      } catch {
        if (isMounted) setGitStatusMap({});
      }
    };
    poll();
    const id = setInterval(poll, 3000);
    return () => { isMounted = false; clearInterval(id); };
  }, [rootPath]);

  useEffect(() => {
    if (!rootPath) return;

    let disposed = false;
    let watchActive = false;
    const unlistenPromise = listen<ExplorerChangedEvent>('explorer-changed', (event) => {
      if (
        normalizeExplorerPathForCompare(event.payload.root_path) !==
        normalizeExplorerPathForCompare(rootPath)
      ) {
        return;
      }
      setChangedPaths(event.payload.changed_paths);
      setRefreshToken((current) => current + 1);
    });

    void unlistenPromise
      .then(async () => {
        if (disposed) return;
        await invoke('explorer_watch', { rootPath });
        watchActive = true;
        if (disposed) {
          watchActive = false;
          invoke('explorer_unwatch', { rootPath }).catch(() => {});
        }
      })
      .catch((err) => {
        console.error('Failed to watch explorer root:', err);
      });

    return () => {
      disposed = true;
      if (watchActive) {
        watchActive = false;
        invoke('explorer_unwatch', { rootPath }).catch(() => {});
      }
      unlistenPromise.then((fn) => fn()).catch(() => {});
    };
  }, [rootPath]);

  useEffect(() => {
    const handleClick = () => setMenuPos(null);
    window.addEventListener('click', handleClick);
    return () => window.removeEventListener('click', handleClick);
  }, []);

  const handleContextMenu = (e: React.MouseEvent, node: FileNode) => {
    e.preventDefault();
    setMenuPos({ x: e.clientX, y: e.clientY });
    setActiveNode(node);
  };

  const handleCopyPath = async () => {
    if (activeNode) {
      await writeText(activeNode.path);
    }
    setMenuPos(null);
  };

  const handleReveal = async () => {
    if (activeNode) {
      try {
        await invoke('reveal_in_explorer', { path: activeNode.path });
      } catch (err) {
        console.error("Reveal failed:", err);
      }
    }
    setMenuPos(null);
  };

  const openExternalPath = async (path: string) => {
    try {
      await invoke('open_in_external_editor', {
        path,
        editor: {
          external_editor: externalEditor,
          external_editor_custom_executable: externalEditorCustomExecutable.trim() || null,
        },
      });
      setExternalOpenError(null);
    } catch (err) {
      console.error("External editor open failed:", err);
      setExternalOpenError(
        `External app open failed for ${externalEditorLabel(externalEditor)}: ${String(err)}`,
      );
    }
  };

  const openExternalEditor = async (node: FileNode) => {
    await openExternalPath(node.path);
  };

  const handleOpenExternalEditor = async () => {
    if (activeNode) {
      await openExternalEditor(activeNode);
    }
    setMenuPos(null);
  };

  const handleOpenRoot = async () => {
    if (!rootPath) return;
    try {
      await invoke('reveal_in_explorer', { path: rootPath });
    } catch (err) {
      console.error("Open explorer root failed:", err);
    }
  };

  const handleOpenRootExternal = async () => {
    if (!rootPath) return;
    await openExternalPath(rootPath);
  };

  const requireNavigation = () => {
    if (!workbenchNavigation) {
      throw new Error('ExplorerPanel requires AppShell Workbench navigation');
    }
    return workbenchNavigation;
  };

  const fileSurfaceRequest = (path: string, transientPreview: boolean) => ({
    surface_type: 'files' as const,
    resource_key: fileResourceKey(path),
    state: createFileSurfaceState(transientPreview),
  });

  const openPermanent = (path: string) => {
    const currentNavigation = requireNavigation();
    const surfaceId = currentNavigation.open(fileSurfaceRequest(path, false));
    currentNavigation.pin_transient(surfaceId);
  };

  const handleOpen = () => {
    if (activeNode && !activeNode.is_dir) openPermanent(activeNode.path);
    setMenuPos(null);
  };

  const handleOpenToSide = () => {
    if (activeNode && !activeNode.is_dir) {
      requireNavigation().open_to_side(fileSurfaceRequest(activeNode.path, false), 'horizontal');
    }
    setMenuPos(null);
  };

  const fileNode = (path: string): FileNode => {
    const normalizedPath = path.replace(/\\/g, '/');
    const name = normalizedPath.split('/').filter(Boolean).pop() ?? path;
    return {
      path,
      name,
      is_dir: false,
      extension: name.includes('.') ? name.split('.').pop() ?? null : null,
    };
  };

  const handleFileSelect = async (path: string, isDir: boolean) => {
    if (isDir) return;

    if (explorerFileClickAction === 'external') {
      await openExternalEditor(fileNode(path));
    } else {
      requireNavigation().open_transient(fileSurfaceRequest(path, true));
    }
  };

  const handleFileOpen = async (path: string, isDir: boolean) => {
    if (isDir) return;
    if (explorerFileClickAction === 'external') {
      await openExternalEditor(fileNode(path));
    } else {
      openPermanent(path);
    }
  };

  const handleDelete = async () => {
    if (activeNode) {
      if (await confirm(`Are you sure you want to delete ${activeNode.name}?`)) {
        try {
          await invoke('delete_file', { path: activeNode.path });
          setChangedPaths([activeNode.path]);
          setRefreshToken((current) => current + 1);
        } catch (err) {
          console.error("Delete failed:", err);
          alert(`Failed to delete: ${err}`);
        }
      }
    }
    setMenuPos(null);
  };

  const showRootExternalAction = externalEditor !== 'system';
  const rootExternalLabel = `Open root in ${externalEditorLabel(externalEditor)}`;

  return (
    <div data-testid="explorer-panel" className="flex flex-col h-full w-full relative">
      <div className="mb-2 flex min-h-7 items-center gap-1">
        <h2 className="min-w-0 flex-1 truncate text-sm font-bold text-primary tracking-tight">Explorer</h2>
        {rootPath && (
          <>
            <button
              type="button"
              aria-label="Open in local file system"
              title="Open in local file system"
              onClick={handleOpenRoot}
              className="rounded p-1 text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted hover:text-primary transition-colors"
            >
              <FolderOpen aria-hidden="true" className="h-3.5 w-3.5" />
            </button>
            {showRootExternalAction && (
              <button
                type="button"
                aria-label={rootExternalLabel}
                title={rootExternalLabel}
                onClick={handleOpenRootExternal}
                className="rounded p-1 text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted hover:text-primary transition-colors"
              >
                <ExternalLink aria-hidden="true" className="h-3.5 w-3.5" />
              </button>
            )}
          </>
        )}
      </div>
      
      {rootPath && (
        <div className="py-1 mb-2 border-b border-wardian-border/30 w-full group">
          <span className="block label-small text-[12px] font-mono text-muted-neutral group-hover:text-primary select-all truncate transition-colors" title={rootPath}>
            {rootPath}
          </span>
        </div>
      )}

      {externalOpenError && (
        <div
          role="alert"
          className="mb-2 rounded-md border border-wardian-error/40 bg-wardian-error/10 px-3 py-2 text-xs leading-relaxed text-wardian-error"
        >
          {externalOpenError}
        </div>
      )}
      
      <div className="flex-1 overflow-auto w-full relative min-h-0 -mx-3 px-3">
        {rootPath ? (
          <FileTree
            path={rootPath}
            onContextMenu={handleContextMenu}
            onSelect={handleFileSelect}
            onOpen={handleFileOpen}
            gitStatusMap={gitStatusMap}
            changedDirectories={changedDirectories}
            explorerRoot={rootPath}
            refreshToken={refreshToken}
            changedPaths={changedPaths}
          />
        ) : (
          <div className="text-sm text-wardian-text-muted animate-pulse border border-transparent">Mapping directory...</div>
        )}
      </div>

      {menuPos && activeNode && (
        <div 
          className="fixed bg-wardian-card border border-wardian-border shadow-2xl rounded-lg py-1 z-50 min-w-40 text-sm font-medium animate-in fade-in zoom-in-95 duration-100"
          style={{ 
            top: Math.min(menuPos.y, window.innerHeight - 200),
            left: Math.min(menuPos.x, window.innerWidth - 200) 
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <div className="px-4 py-2 label-small truncate border-b border-wardian-border mb-1 max-w-64">
            {activeNode.name}
          </div>
          {!activeNode.is_dir && (
            <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-primary" onClick={handleOpen}>
              Open
            </button>
          )}
          {!activeNode.is_dir && (
            <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-primary" onClick={handleOpenToSide}>
              Open to Side
            </button>
          )}
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-primary" onClick={handleOpenExternalEditor}>
            Open in External App
          </button>
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-primary" onClick={handleReveal}>
            Reveal in OS
          </button>
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-primary" onClick={handleCopyPath}>
            Copy Absolute Path
          </button>
          <div className="h-px bg-wardian-border my-1 w-full" />
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-wardian-error group flex items-center justify-between" onClick={handleDelete}>
            Delete
          </button>
        </div>
      )}
    </div>
  );
};
