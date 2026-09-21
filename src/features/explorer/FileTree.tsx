import React, { createContext, useContext, useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronRight, ChevronDown, File, FileText, Image, Code } from 'lucide-react';
import { normalizeExplorerPathForCompare } from './pathUtils';
import { gitStatusColor } from '../git/gitStatusPresentation';
import { setWardianFileDragData } from '../../utils/fileDrop';

export interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  extension: string | null;
}

export interface DirectoryTreeResult {
  nodes: FileNode[];
  truncated: boolean;
  next_offset?: number | null;
}

export interface FileTreeProps {
  path: string;
  /** `open_in_new_tab` is set for the platform-standard Ctrl/Cmd-click gesture. */
  onSelect?: (path: string, is_dir: boolean, open_in_new_tab?: boolean) => void;
  onOpen?: (path: string, is_dir: boolean) => void;
  onContextMenu?: (e: React.MouseEvent, node: FileNode) => void;
  depth?: number;
  gitStatusMap?: Record<string, string>;
  changedDirectories?: Set<string>;
  explorerRoot?: string;
  refreshToken?: number;
  changedPaths?: string[];
}

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

interface FileTreeInteractionController {
  activePath: string | null;
  setActivePath: React.Dispatch<React.SetStateAction<string | null>>;
  treeRef: React.RefObject<HTMLDivElement | null>;
  cancelPendingSelection: () => void;
  scheduleSelection: (path: string, select: () => void) => void;
}

const FileTreeInteractionContext = createContext<FileTreeInteractionController | null>(null);

function useFileTreeInteraction(): FileTreeInteractionController {
  const controller = useContext(FileTreeInteractionContext);
  if (!controller) throw new Error('FileTree branches require a shared interaction controller');
  return controller;
}

type FileTreeBranchProps = Omit<FileTreeProps, 'depth'> & { depth: number };

const FileTreeBranch: React.FC<FileTreeBranchProps> = ({
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
  const interaction = useFileTreeInteraction();
  const [nodes, setNodes] = useState<FileNode[]>([]);
  const [listingTruncated, setListingTruncated] = useState(false);
  const [listingNextOffset, setListingNextOffset] = useState<number | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(false);
  const [completingListing, setCompletingListing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestGeneration = useRef(0);
  const mounted = useRef(true);
  const completionFrame = useRef<number | null>(null);
  const completionTimer = useRef<number | null>(null);
  const initialRefresh = useRef({ path, token: refreshToken });
  if (initialRefresh.current.path !== path) {
    initialRefresh.current = { path, token: refreshToken };
  }

  const cancelDeferredCompletion = useCallback(() => {
    if (completionFrame.current !== null) {
      window.cancelAnimationFrame(completionFrame.current);
      completionFrame.current = null;
    }
    if (completionTimer.current !== null) {
      window.clearTimeout(completionTimer.current);
      completionTimer.current = null;
    }
  }, []);

  const beginRequest = useCallback(() => {
    cancelDeferredCompletion();
    requestGeneration.current += 1;
    return requestGeneration.current;
  }, [cancelDeferredCompletion]);

  const isCurrentRequest = useCallback((generation: number) => (
    mounted.current && requestGeneration.current === generation
  ), []);

  const fetchTree = useCallback(async (
    generation: number,
    showLoading: boolean,
    offset = 0,
    append = false,
  ) => {
    if (showLoading) {
      setLoading(true);
    }
    try {
      const result = await invoke<DirectoryTreeResult>(
        'get_directory_tree',
        offset > 0 ? { path, offset } : { path },
      );
      if (isCurrentRequest(generation)) {
        const page = result.nodes;
        setNodes((current) => {
          if (!append) return page;
          const byPath = new Map(current.map((node) => [node.path, node]));
          for (const node of page) byPath.set(node.path, node);
          return [...byPath.values()].sort((left, right) =>
            Number(right.is_dir) - Number(left.is_dir) || left.name.localeCompare(right.name),
          );
        });
        setListingTruncated(result.truncated);
        setListingNextOffset(result.next_offset ?? null);
        setCompletingListing(false);
        setError(null);
      }
    } catch (err) {
      if (isCurrentRequest(generation)) {
        setCompletingListing(false);
        setError(String(err));
        console.error("Failed to load directory tree for", path, err);
      }
    } finally {
      if (isCurrentRequest(generation) && showLoading) setLoading(false);
    }
  }, [isCurrentRequest, path]);

  const loadMore = () => {
    if (listingNextOffset === null || loading) return;
    const generation = beginRequest();
    void fetchTree(generation, true, listingNextOffset, true);
  };

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      requestGeneration.current += 1;
      cancelDeferredCompletion();
    };
  }, [cancelDeferredCompletion]);

  useEffect(() => {
    const generation = beginRequest();
    const fetchPreview = async () => {
      setLoading(true);
      try {
        const result = await invoke<DirectoryTreeResult>('get_directory_preview', { path });
        if (!isCurrentRequest(generation)) return;
        setNodes(result.nodes);
        setListingTruncated(false);
        setListingNextOffset(null);
        setError(null);
        setLoading(false);
        if (result.truncated) {
          setCompletingListing(true);
          completionFrame.current = window.requestAnimationFrame(() => {
            completionFrame.current = null;
            completionTimer.current = window.setTimeout(() => {
              completionTimer.current = null;
              if (!isCurrentRequest(generation)) return;
              void fetchTree(generation, false);
            }, 0);
          });
        } else {
          setCompletingListing(false);
        }
      } catch (err) {
        if (!isCurrentRequest(generation)) return;
        setLoading(false);
        setError(String(err));
        console.error("Failed to load directory preview for", path, err);
      }
    };
    void fetchPreview();
    return () => {
      if (requestGeneration.current === generation) {
        requestGeneration.current += 1;
      }
      cancelDeferredCompletion();
    };
  }, [beginRequest, cancelDeferredCompletion, fetchTree, isCurrentRequest, path]);

  useEffect(() => {
    if (refreshToken === initialRefresh.current.token) return;
    if (!changedPaths.some((changedPath) => pathAffectsDirectory(changedPath, path))) {
      return;
    }

    const generation = beginRequest();
    setCompletingListing(false);
    void fetchTree(generation, true);
  }, [beginRequest, changedPaths, fetchTree, path, refreshToken]);

  useLayoutEffect(() => {
    const visibleItems = Array.from(
      interaction.treeRef.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? [],
    );
    const activeIsVisible = interaction.activePath !== null && visibleItems.some(
      (item) => item.dataset.fileTreePath === interaction.activePath,
    );
    const nextActivePath = activeIsVisible
      ? interaction.activePath
      : visibleItems[0]?.dataset.fileTreePath ?? null;
    if (nextActivePath !== interaction.activePath) {
      interaction.setActivePath(nextActivePath);
    }
  }, [error, expanded, interaction, loading, nodes]);

  const toggleFolder = (nodePath: string) => {
    setExpanded(prev => ({ ...prev, [nodePath]: !prev[nodePath] }));
  };

  const focusNode = (nodePath: string, target: Element) => {
    const treeItem = target.closest<HTMLElement>('[role="treeitem"]');
    interaction.setActivePath(nodePath);
    treeItem?.focus();
  };

  const handleClick = (e: React.MouseEvent<HTMLDivElement>, node: FileNode) => {
    e.stopPropagation();
    focusNode(node.path, e.currentTarget);
    if (node.is_dir) {
      if (e.detail > 1) return;
      interaction.cancelPendingSelection();
      toggleFolder(node.path);
      onSelect?.(node.path, true);
      return;
    }
    if (e.detail > 1) return;
    if (e.ctrlKey || e.metaKey) {
      interaction.cancelPendingSelection();
      onSelect?.(node.path, false, true);
      return;
    }
    interaction.scheduleSelection(node.path, () => onSelect?.(node.path, false));
  };

  const handleOpen = (e: React.SyntheticEvent, node: FileNode) => {
    e.stopPropagation();
    if (node.is_dir) return;
    interaction.cancelPendingSelection();
    interaction.setActivePath(node.path);
    onOpen?.(node.path, false);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>, node: FileNode) => {
    e.stopPropagation();
    const treeItems = Array.from(
      interaction.treeRef.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? [],
    );
    const currentIndex = treeItems.indexOf(e.currentTarget);
    const focusItem = (item: HTMLElement | undefined) => {
      if (!item) return;
      interaction.setActivePath(item.dataset.fileTreePath ?? null);
      item.focus();
    };

    switch (e.key) {
      case 'Enter':
      case ' ':
        e.preventDefault();
        interaction.cancelPendingSelection();
        if (node.is_dir) {
          toggleFolder(node.path);
          onSelect?.(node.path, true);
        } else {
          onOpen?.(node.path, false);
        }
        return;
      case 'ArrowDown':
        e.preventDefault();
        focusItem(treeItems[currentIndex + 1]);
        return;
      case 'ArrowUp':
        e.preventDefault();
        focusItem(treeItems[currentIndex - 1]);
        return;
      case 'Home':
        e.preventDefault();
        focusItem(treeItems[0]);
        return;
      case 'End':
        e.preventDefault();
        focusItem(treeItems[treeItems.length - 1]);
        return;
      case 'ArrowRight':
        if (!node.is_dir) return;
        e.preventDefault();
        if (!expanded[node.path]) {
          interaction.cancelPendingSelection();
          toggleFolder(node.path);
          onSelect?.(node.path, true);
        } else {
          focusItem(e.currentTarget.querySelector<HTMLElement>('[role="group"] [role="treeitem"]') ?? undefined);
        }
        return;
      case 'ArrowLeft': {
        e.preventDefault();
        if (node.is_dir && expanded[node.path]) {
          toggleFolder(node.path);
          return;
        }
        const parentGroup = e.currentTarget.parentElement;
        focusItem(parentGroup?.getAttribute('role') === 'group'
          ? parentGroup.parentElement ?? undefined
          : undefined);
        return;
      }
      default:
        return;
    }
  };

  if (loading && depth === 0 && nodes.length === 0) {
    return <div className="text-sm text-wardian-text-muted p-2 animate-pulse">Loading workspace...</div>;
  }

  if (error && depth === 0 && nodes.length === 0) {
    return <div className="p-2 text-sm text-wardian-error break-words">Error: {error}</div>;
  }

  return (
    <>
      {nodes.map(node => {
        const relPath = explorerRoot ? toRelativePath(node.path, explorerRoot) : '';
        const fileStatus = gitStatusMap?.[relPath];
        const dirHasChanges = node.is_dir && relPath !== '' && changedDirectories?.has(relPath);
        const gitColor = fileStatus
          ? gitStatusColor(fileStatus)
          : dirHasChanges ? gitStatusColor('M') : undefined;

        return (
          <div
            key={node.path}
            role="treeitem"
            aria-label={node.name}
            aria-expanded={node.is_dir ? Boolean(expanded[node.path]) : undefined}
            data-file-tree-path={node.path}
            draggable={!node.is_dir}
            tabIndex={(
              interaction.activePath ?? (depth === 0 ? nodes[0]?.path : null)
            ) === node.path ? 0 : -1}
            onDragStart={(event) => {
              setWardianFileDragData(event.dataTransfer, [node.path]);
            }}
            onClick={(e) => {
              if ((e.target as Element).closest('[role="treeitem"]') === e.currentTarget) {
                handleClick(e, node);
              }
            }}
            onDoubleClick={(e) => {
              if ((e.target as Element).closest('[role="treeitem"]') === e.currentTarget) {
                handleOpen(e, node);
              }
            }}
            onContextMenu={(e) => {
              if ((e.target as Element).closest('[role="treeitem"]') === e.currentTarget) {
                onContextMenu?.(e, node);
              }
            }}
            onFocus={(e) => {
              if (e.currentTarget === e.target) interaction.setActivePath(node.path);
            }}
            onKeyDown={(e) => handleKeyDown(e, node)}
          >
            <div
              className="flex items-center shrink-0 gap-1.5 py-[2px] pr-2 hover:bg-wardian-card-bg-muted cursor-pointer rounded-md text-[13px] whitespace-nowrap overflow-hidden select-none group w-full"
              style={{ paddingLeft: `${(depth * 14) + 2}px` }}
            >
              {node.is_dir ? (
                <span
                  className="text-wardian-text-muted cursor-pointer hover:text-wardian-text shrink-0 flex items-center justify-center w-4 h-4"
                  onClick={(e) => {
                    e.stopPropagation();
                    interaction.cancelPendingSelection();
                    focusNode(node.path, e.currentTarget);
                    toggleFolder(node.path);
                  }}
                >
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
              <div role="group">
                <FileTreeBranch
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
              </div>
            )}
          </div>
        );
      })}
      {completingListing && (
        <div className="px-2 py-1 text-[11px] text-wardian-text-muted italic" role="status">
          Loading remaining files…
        </div>
      )}
      {error && nodes.length > 0 && (
        <div className="px-2 py-1 text-[11px] text-wardian-error" role="status">
          Couldn’t finish loading this folder.
        </div>
      )}
      {listingTruncated && (
        <div className="flex items-center gap-2 px-2 py-1 text-[11px] text-wardian-text-muted italic" role="status">
          <span>Showing the first 500 items in this folder; pages are capped at 500.</span>
          {listingNextOffset !== null && (
            <button
              type="button"
              className="not-italic text-[var(--color-wardian-accent)] hover:underline disabled:opacity-50"
              onClick={loadMore}
              disabled={loading}
            >
              {loading ? 'Loading…' : 'Load next 500'}
            </button>
          )}
        </div>
      )}
    </>
  );
};

export const FileTree: React.FC<FileTreeProps> = ({ depth: _depth, ...props }) => {
  const treeRef = useRef<HTMLDivElement>(null);
  const pendingSelection = useRef<{ path: string; timer: number } | null>(null);
  const [activePath, setActivePath] = useState<string | null>(null);

  const cancelPendingSelection = useCallback(() => {
    if (!pendingSelection.current) return;
    window.clearTimeout(pendingSelection.current.timer);
    pendingSelection.current = null;
  }, []);

  const scheduleSelection = useCallback((path: string, select: () => void) => {
    cancelPendingSelection();
    const timer = window.setTimeout(() => {
      if (pendingSelection.current?.timer !== timer) return;
      pendingSelection.current = null;
      select();
    }, 200);
    pendingSelection.current = { path, timer };
  }, [cancelPendingSelection]);

  useEffect(() => {
    cancelPendingSelection();
    setActivePath(null);
    return cancelPendingSelection;
  }, [cancelPendingSelection, props.explorerRoot, props.path]);

  const interaction = React.useMemo<FileTreeInteractionController>(() => ({
    activePath,
    setActivePath,
    treeRef,
    cancelPendingSelection,
    scheduleSelection,
  }), [activePath, cancelPendingSelection, scheduleSelection]);

  return (
    <FileTreeInteractionContext.Provider value={interaction}>
      <div
        ref={treeRef}
        className="flex h-full w-full flex-col"
        role="tree"
        aria-label="Workspace files"
      >
        <FileTreeBranch key={props.path} {...props} depth={0} />
      </div>
    </FileTreeInteractionContext.Provider>
  );
};
