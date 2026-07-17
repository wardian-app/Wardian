import React, { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronRight, ChevronDown, File, FileText, Image, Code } from 'lucide-react';
import { normalizeExplorerPathForCompare } from './pathUtils';

export interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  extension: string | null;
}

export interface FileTreeProps {
  path: string;
  onSelect?: (path: string, is_dir: boolean) => void;
  onOpen?: (path: string, is_dir: boolean) => void;
  onContextMenu?: (e: React.MouseEvent, node: FileNode) => void;
  depth?: number;
  gitStatusMap?: Record<string, string>;
  changedDirectories?: Set<string>;
  explorerRoot?: string;
  refreshToken?: number;
  changedPaths?: string[];
}

const GIT_STATUS_COLORS: Record<string, string> = {
  M: 'var(--color-wardian-warning)',
  A: 'var(--color-wardian-success)',
  D: 'var(--color-wardian-error)',
  R: 'var(--color-wardian-warning)',
  C: 'var(--color-wardian-processing)',
  '?': 'var(--color-wardian-success)',
};

function toRelativePath(nodePath: string, root: string): string {
  const n = nodePath.replace(/\\/g, '/');
  const r = root.replace(/\\/g, '/').replace(/\/$/, '');
  return n.startsWith(r) ? n.slice(r.length).replace(/^\//, '') : n;
}

function pathAffectsDirectory(changedPath: string, directoryPath: string): boolean {
  const changed = normalizeExplorerPathForCompare(changedPath);
  const directory = normalizeExplorerPathForCompare(directoryPath);
  const changedParent = changed.includes('/') ? changed.slice(0, changed.lastIndexOf('/')) : '';
  return changed === directory || changedParent === directory;
}

const getFileIcon = (extension: string | null) => {
  if (!extension) return <File className="w-4 h-4 text-wardian-text-muted shrink-0" />;
  const ext = extension.toLowerCase();
  if (['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp'].includes(ext)) {
    return <Image className="w-4 h-4 text-wardian-processing shrink-0" />;
  }
  if (['ts', 'tsx', 'js', 'jsx', 'json', 'rs', 'py', 'html', 'css', 'md'].includes(ext)) {
    return <Code className="w-4 h-4 text-wardian-accent shrink-0" />;
  }
  return <FileText className="w-4 h-4 text-wardian-text-muted shrink-0" />;
}

export const FileTree: React.FC<FileTreeProps> = ({
  path,
  onSelect,
  onOpen,
  onContextMenu,
  depth = 0,
  gitStatusMap,
  changedDirectories,
  explorerRoot,
  refreshToken = 0,
  changedPaths = [],
}) => {
  const [nodes, setNodes] = useState<FileNode[]>([]);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pendingSelection = useRef<{
    path: string;
    timer: number;
  } | null>(null);

  const cancelPendingSelection = useCallback((path?: string) => {
    const pending = pendingSelection.current;
    if (!pending || (path !== undefined && pending.path !== path)) return;
    window.clearTimeout(pending.timer);
    pendingSelection.current = null;
  }, []);

  useEffect(() => () => cancelPendingSelection(), [cancelPendingSelection]);

  const fetchTree = useCallback(async (isMounted: () => boolean, showLoading: boolean) => {
    if (showLoading) {
      setLoading(true);
    }
    try {
      const result = await invoke<FileNode[]>('get_directory_tree', { path });
      if (isMounted()) {
        setNodes(result);
        setError(null);
      }
    } catch (err) {
      if (isMounted()) {
        setError(String(err));
        console.error("Failed to load directory tree for", path, err);
      }
    } finally {
      if (isMounted() && showLoading) setLoading(false);
    }
  }, [path]);

  useEffect(() => {
    let isMounted = true;
    void fetchTree(() => isMounted, true);
    return () => { isMounted = false; };
  }, [fetchTree]);

  useEffect(() => {
    if (refreshToken === 0) return;
    if (!changedPaths.some((changedPath) => pathAffectsDirectory(changedPath, path))) {
      return;
    }

    let isMounted = true;
    void fetchTree(() => isMounted, false);
    return () => { isMounted = false; };
  }, [changedPaths, fetchTree, path, refreshToken]);

  const toggleFolder = (nodePath: string) => {
    setExpanded(prev => ({ ...prev, [nodePath]: !prev[nodePath] }));
  };

  const handleClick = (e: React.MouseEvent<HTMLDivElement>, node: FileNode) => {
    e.stopPropagation();
    e.currentTarget.focus();
    if (node.is_dir) {
      if (e.detail > 1) return;
      toggleFolder(node.path);
      onSelect?.(node.path, true);
      return;
    }
    if (e.detail > 1) return;
    cancelPendingSelection();
    const timer = window.setTimeout(() => {
      if (pendingSelection.current?.timer !== timer) return;
      pendingSelection.current = null;
      onSelect?.(node.path, false);
    }, 200);
    pendingSelection.current = { path: node.path, timer };
  };

  const handleOpen = (e: React.SyntheticEvent, node: FileNode) => {
    e.stopPropagation();
    if (node.is_dir) return;
    cancelPendingSelection(node.path);
    onOpen?.(node.path, false);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>, node: FileNode) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    if (node.is_dir) {
      cancelPendingSelection();
      toggleFolder(node.path);
      onSelect?.(node.path, true);
      return;
    }
    handleOpen(e, node);
  };

  if (loading && depth === 0) {
    return <div className="text-sm text-wardian-text-muted p-2 animate-pulse">Loading workspace...</div>;
  }

  if (error && depth === 0) {
    return <div className="text-sm text-red-400 p-2 break-words">Error: {error}</div>;
  }

  return (
    <div
      className={`flex flex-col ${depth === 0 ? 'w-full h-full' : ''}`}
      role={depth === 0 ? 'tree' : 'group'}
      aria-label={depth === 0 ? 'Workspace files' : undefined}
    >
      {nodes.map(node => {
        const relPath = explorerRoot ? toRelativePath(node.path, explorerRoot) : '';
        const fileStatus = gitStatusMap?.[relPath];
        const dirHasChanges = node.is_dir && relPath !== '' && changedDirectories?.has(relPath);
        const gitColor = fileStatus
          ? GIT_STATUS_COLORS[fileStatus]
          : dirHasChanges ? GIT_STATUS_COLORS['M'] : undefined;

        return (
          <React.Fragment key={node.path}>
            <div
              role="treeitem"
              tabIndex={0}
              aria-expanded={node.is_dir ? Boolean(expanded[node.path]) : undefined}
              className="flex items-center shrink-0 gap-1.5 py-[2px] pr-2 hover:bg-wardian-card-bg-muted cursor-pointer rounded-md text-[13px] whitespace-nowrap overflow-hidden select-none group w-full"
              style={{ paddingLeft: `${(depth * 14) + 2}px` }}
              onClick={(e) => handleClick(e, node)}
              onDoubleClick={(e) => handleOpen(e, node)}
              onKeyDown={(e) => handleKeyDown(e, node)}
              onContextMenu={(e) => onContextMenu && onContextMenu(e, node)}
            >
              {node.is_dir ? (
                <span className="text-wardian-text-muted cursor-pointer hover:text-wardian-text shrink-0 flex items-center justify-center w-4 h-4" onClick={(e) => { e.stopPropagation(); toggleFolder(node.path); }}>
                  {expanded[node.path] ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
                </span>
              ) : (
                <span className="w-4 h-4 shrink-0 inline-block" />
              )}

              {!node.is_dir && (
                <span className="text-wardian-text-muted flex items-center shrink-0">
                  {getFileIcon(node.extension)}
                </span>
              )}

              <span
                className={`truncate flex-1 ${node.is_dir ? 'font-medium' : ''} ${!gitColor ? (node.is_dir ? 'text-wardian-text' : 'text-wardian-text-muted group-hover:text-wardian-text') : ''} transition-colors`}
                style={gitColor ? { color: gitColor } : undefined}
              >
                {node.name}
              </span>

              {fileStatus && (
                <span className="shrink-0 text-[10px] font-bold ml-1 opacity-80" style={{ color: gitColor }}>
                  {fileStatus}
                </span>
              )}
            </div>

            {node.is_dir && expanded[node.path] && (
              <FileTree
                path={node.path}
                depth={depth + 1}
                onSelect={onSelect}
                onOpen={onOpen}
                onContextMenu={onContextMenu}
                gitStatusMap={gitStatusMap}
                changedDirectories={changedDirectories}
                explorerRoot={explorerRoot}
                refreshToken={refreshToken}
                changedPaths={changedPaths}
              />
            )}
          </React.Fragment>
        );
      })}
    </div>
  );
};
