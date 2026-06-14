import React, { useState, useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { FolderOpen } from 'lucide-react';
import { FileTree, FileNode } from './FileTree';
import { useConfirm } from '../../components/ConfirmDialog';
import { AgentConfig, GitStatusResult } from '../../types';
import { useSettingsStore } from '../../store/useSettingsStore';
import { normalizeExplorerPathForCompare } from './pathUtils';

interface ExplorerPanelProps {
  selectedAgentIds: Set<string>;
  agents: AgentConfig[];
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

export const ExplorerPanel: React.FC<ExplorerPanelProps> = ({ selectedAgentIds, agents }) => {
  const confirm = useConfirm();
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
  
  // Preview Modal State
  const [previewContent, setPreviewContent] = useState<string | null>(null);
  const [previewTitle, setPreviewTitle] = useState<string | null>(null);
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

  const openExternalEditor = async (node: FileNode) => {
    try {
      await invoke('open_in_external_editor', {
        path: node.path,
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

  const openPreview = async (node: FileNode) => {
    if (node.is_dir) return;
    try {
      const content = await invoke<string>('read_file_preview', { path: node.path });
      setPreviewTitle(node.name);
      setPreviewContent(content);
    } catch (err) {
      console.error("Preview failed:", err);
      setPreviewTitle("Error reading " + node.name);
      setPreviewContent(String(err));
    }
  };

  const handlePreview = async () => {
    if (activeNode) {
      await openPreview(activeNode);
    }
    setMenuPos(null);
  };

  const handleFileSelect = async (path: string, isDir: boolean) => {
    if (isDir) return;
    const normalizedPath = path.replace(/\\/g, '/');
    const name = normalizedPath.split('/').filter(Boolean).pop() ?? path;
    const node: FileNode = {
      path,
      name,
      is_dir: false,
      extension: name.includes('.') ? name.split('.').pop() ?? null : null,
    };

    if (explorerFileClickAction === 'external') {
      await openExternalEditor(node);
    } else {
      await openPreview(node);
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

  return (
    <div data-testid="explorer-panel" className="flex flex-col h-full w-full relative">
      <div className="flex flex-col mb-2">
        <h2 className="text-sm font-bold text-primary tracking-tight">Explorer</h2>
      </div>
      
      {rootPath && (
        <div className="flex items-center gap-1.5 py-1 mb-2 border-b border-wardian-border/30 w-full group">
          <span className="label-small text-[12px] font-mono text-muted-neutral group-hover:text-primary select-all truncate transition-colors flex-1 min-w-0" title={rootPath}>
            {rootPath}
          </span>
          <button
            type="button"
            aria-label="Open in local file system"
            title="Open in local file system"
            onClick={handleOpenRoot}
            className="shrink-0 rounded-md border border-wardian-border p-1 text-muted hover:text-primary hover:bg-wardian-card-bg-muted transition-colors"
          >
            <FolderOpen aria-hidden="true" size={14} strokeWidth={2} />
          </button>
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
            <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-primary" onClick={handlePreview}>
              Open Preview
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

      {/* Preview Modal */}
      {previewContent !== null && (
        <div className="fixed inset-0 z-[100] bg-black/60 flex items-center justify-center p-8 backdrop-blur-sm animate-in fade-in" onClick={() => setPreviewContent(null)}>
          <div 
            className="bg-wardian-card border border-wardian-border shadow-2xl rounded-2xl w-full max-w-4xl max-h-[90vh] flex flex-col font-mono text-sm overflow-hidden animate-in zoom-in-95 duration-200"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex justify-between items-center p-4 border-b border-wardian-border shrink-0 bg-wardian-bg/50">
              <h3 className="font-bold text-lg text-wardian-accent truncate flex-1 mr-4">{previewTitle}</h3>
              <button onClick={() => setPreviewContent(null)} className="text-wardian-text-muted hover:text-wardian-error font-bold transition-colors w-8 h-8 flex items-center justify-center rounded-md hover:bg-wardian-error/10">✕</button>
            </div>
            <div className="p-0 overflow-y-auto flex-1 bg-wardian-bg/30 cursor-text">
              <pre className="p-6 text-primary whitespace-pre-wrap break-words">{previewContent}</pre>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
