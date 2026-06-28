import React, { useState } from "react";
import { GitFileEntry } from "../../types";
import { ContextMenu, type ContextMenuItem } from "../../components/ContextMenu";

interface GitFileListProps {
  files: GitFileEntry[];
  displayMode?: "list" | "tree";
  onStage?: (path: string) => void;
  onStagePaths?: (paths: string[]) => void;
  onUnstage?: (path: string) => void;
  onUnstagePaths?: (paths: string[]) => void;
  onDiscard?: (path: string) => void;
  onDiscardPaths?: (paths: string[], label?: string) => void;
  onIgnorePaths?: (paths: string[]) => void;
  onDiff?: (path: string, staged: boolean) => void;
  onCompareWithWorkspace?: (path: string) => void;
  onOpenFile?: (path: string) => void;
  onOpenHeadFile?: (path: string) => void;
  onRevealFile?: (path: string) => void;
}

interface FileTreeDirectory {
  type: "directory";
  name: string;
  path: string;
  children: FileTreeNode[];
}

interface FileTreeFile {
  type: "file";
  file: GitFileEntry;
}

type FileTreeNode = FileTreeDirectory | FileTreeFile;

const STATUS_COLORS: Record<string, string> = {
  M: "text-[var(--color-wardian-warning)]",
  A: "text-[var(--color-wardian-success)]",
  D: "text-[var(--color-wardian-error)]",
  R: "text-[var(--color-wardian-processing)]",
  C: "text-[var(--color-wardian-processing)]",
  U: "text-[var(--color-wardian-warning)]",
  AA: "text-[var(--color-wardian-warning)]",
  AU: "text-[var(--color-wardian-warning)]",
  DD: "text-[var(--color-wardian-error)]",
  DU: "text-[var(--color-wardian-error)]",
  UA: "text-[var(--color-wardian-warning)]",
  UD: "text-[var(--color-wardian-error)]",
  UU: "text-[var(--color-wardian-warning)]",
  "?": "text-[var(--color-wardian-text-muted)]",
};

const STATUS_LABELS: Record<string, string> = {
  M: "Modified",
  A: "Added",
  D: "Deleted",
  R: "Renamed",
  C: "Copied",
  U: "Unmerged",
  AA: "Both Added",
  AU: "Added By Us",
  DD: "Both Deleted",
  DU: "Deleted By Us",
  UA: "Added By Them",
  UD: "Deleted By Them",
  UU: "Both Modified",
  "?": "Untracked",
};

const isDeletedStatus = (status: string) => status === "D" || status.includes("D");

const splitPath = (path: string) => path.replace(/\\/g, "/").split("/").filter(Boolean);
const lastPathPart = (path: string) => {
  const parts = splitPath(path);
  return parts.length > 0 ? parts[parts.length - 1] : path;
};

const resourcePriority = (status: string) => {
  if (["U", "AA", "AU", "DD", "DU", "UA", "UD", "UU"].includes(status)) return 4;
  if (["M", "C", "T"].includes(status)) return 2;
  return 1;
};

const compareFilePaths = (a: string, b: string) => a.replace(/\\/g, "/").localeCompare(b.replace(/\\/g, "/"));

const sortFilesByResourcePriority = (files: GitFileEntry[]) =>
  [...files].sort((a, b) => {
    const priorityDiff = resourcePriority(b.status) - resourcePriority(a.status);
    if (priorityDiff !== 0) return priorityDiff;
    return compareFilePaths(a.path, b.path);
  });

const buildFileTree = (files: GitFileEntry[]): FileTreeNode[] => {
  const root: FileTreeDirectory = { type: "directory", name: "", path: "", children: [] };

  for (const file of sortFilesByResourcePriority(files)) {
    const parts = splitPath(file.path);
    let current = root;
    parts.slice(0, -1).forEach((part, index) => {
      const directoryPath = parts.slice(0, index + 1).join("/");
      let directory = current.children.find(
        (child): child is FileTreeDirectory => child.type === "directory" && child.path === directoryPath,
      );
      if (!directory) {
        directory = { type: "directory", name: part, path: directoryPath, children: [] };
        current.children.push(directory);
      }
      current = directory;
    });
    current.children.push({ type: "file", file });
  }

  const sortNodes = (nodes: FileTreeNode[]) => {
    nodes.sort((a, b) => {
      if (a.type !== b.type) return a.type === "directory" ? -1 : 1;
      if (a.type === "file" && b.type === "file") {
        const priorityDiff = resourcePriority(b.file.status) - resourcePriority(a.file.status);
        if (priorityDiff !== 0) return priorityDiff;
        return compareFilePaths(a.file.path, b.file.path);
      }
      const aName = a.type === "directory" ? a.name : lastPathPart(a.file.path);
      const bName = b.type === "directory" ? b.name : lastPathPart(b.file.path);
      return aName.localeCompare(bName);
    });
    nodes.forEach((node) => {
      if (node.type === "directory") sortNodes(node.children);
    });
  };

  sortNodes(root.children);
  return root.children;
};

export const GitFileList: React.FC<GitFileListProps> = ({
  files,
  displayMode = "list",
  onStage,
  onStagePaths,
  onUnstage,
  onUnstagePaths,
  onDiscard,
  onDiscardPaths,
  onIgnorePaths,
  onDiff,
  onCompareWithWorkspace,
  onOpenFile,
  onOpenHeadFile,
  onRevealFile,
}) => {
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
  } | null>(null);
  const [collapsedDirectories, setCollapsedDirectories] = useState<Set<string>>(() => new Set());
  const sortedFiles = sortFilesByResourcePriority(files);

  if (files.length === 0) return null;

  const openFileContextMenu = (event: React.MouseEvent, file: GitFileEntry) => {
    event.preventDefault();
    event.stopPropagation();

    const items: ContextMenuItem[] = [];
    if (onDiff) {
      items.push({ label: "View Changes", onClick: () => onDiff(file.path, file.is_staged) });
    }
    if (file.is_staged && onCompareWithWorkspace) {
      items.push({ label: "Compare with Workspace", onClick: () => onCompareWithWorkspace(file.path) });
    }
    if (onOpenFile) {
      items.push({ label: "Open File", onClick: () => onOpenFile(file.path) });
    }
    if (onOpenHeadFile) {
      items.push({ label: "Open File (HEAD)", onClick: () => onOpenHeadFile(file.path) });
    }
    if (onRevealFile) {
      items.push({ label: "Reveal in Explorer View", onClick: () => onRevealFile(file.path) });
    }
    if (file.is_staged && onUnstage) {
      items.push({ label: "Unstage", onClick: () => onUnstage(file.path) });
    }
    if (!file.is_staged && onStage) {
      items.push({ label: "Stage", onClick: () => onStage(file.path) });
    }
    if (!file.is_staged && onDiscard) {
      items.push({ label: "Discard Changes", danger: true, onClick: () => onDiscard(file.path) });
    }
    if (!file.is_staged && onIgnorePaths) {
      items.push({ label: "Add to .gitignore", onClick: () => onIgnorePaths([file.path.replace(/\\/g, "/")]) });
    }

    if (items.length > 0) {
      setContextMenu({ x: event.clientX, y: event.clientY, items });
    }
  };

  const folderFiles = (folderPath: string) => {
    const normalizedFolder = folderPath.replace(/\\/g, "/");
    return sortedFiles.filter((file) => {
      const normalizedPath = file.path.replace(/\\/g, "/");
      return normalizedPath.startsWith(`${normalizedFolder}/`);
    });
  };

  const openFolderContextMenu = (event: React.MouseEvent, directory: FileTreeDirectory) => {
    event.preventDefault();
    event.stopPropagation();

    const descendants = folderFiles(directory.path);
    const items: ContextMenuItem[] = [];
    const staged = descendants.filter((file) => file.is_staged);
    const unstaged = descendants.filter((file) => !file.is_staged);
    const discardable = unstaged;

    if (unstaged.length > 0 && (onStagePaths || onStage)) {
      items.push({
        label: "Stage Changes",
        onClick: () => {
          const paths = unstaged.map((file) => file.path);
          if (onStagePaths) {
            onStagePaths(paths);
          } else {
            paths.forEach((path) => onStage?.(path));
          }
        },
      });
    }

    if (staged.length > 0 && (onUnstagePaths || onUnstage)) {
      items.push({
        label: "Unstage Changes",
        onClick: () => {
          const paths = staged.map((file) => file.path);
          if (onUnstagePaths) {
            onUnstagePaths(paths);
          } else {
            paths.forEach((path) => onUnstage?.(path));
          }
        },
      });
    }

    if (discardable.length > 0 && (onDiscardPaths || onDiscard)) {
      items.push({
        label: "Discard Changes",
        danger: true,
        onClick: () => {
          const paths = discardable.map((file) => file.path);
          if (onDiscardPaths) {
            onDiscardPaths(paths, directory.path);
          } else {
            paths.forEach((path) => onDiscard?.(path));
          }
        },
      });
    }

    if (unstaged.length > 0 && onIgnorePaths) {
      items.push({
        label: "Add to .gitignore",
        onClick: () => onIgnorePaths([`${directory.path.replace(/\\/g, "/").replace(/\/+$/g, "")}/`]),
      });
    }

    if (items.length > 0) {
      setContextMenu({ x: event.clientX, y: event.clientY, items });
    }
  };

  const renderFileRow = (file: GitFileEntry, index: number, depth: number, showDirectory: boolean) => {
    const colorClass = STATUS_COLORS[file.status] || "text-primary";
    const statusLabel = STATUS_LABELS[file.status] ?? file.status;
    const deleted = isDeletedStatus(file.status);
    const filename = lastPathPart(file.path);
    const dir = file.path.includes("/")
      ? file.path.substring(0, file.path.lastIndexOf("/"))
      : "";

    return (
      <li
        key={`${file.path}-${file.is_staged}-${index}`}
        className="flex items-center gap-1.5 px-1.5 py-1 rounded hover:bg-wardian-card-bg-muted group text-xs"
        style={{ paddingLeft: `${6 + depth * 14}px` }}
        onContextMenu={(event) => openFileContextMenu(event, file)}
      >
        <button
          className={`flex-1 min-w-0 text-left truncate text-primary hover:underline cursor-pointer ${deleted ? "text-[var(--color-wardian-text-muted)]" : ""}`}
          aria-label={`View diff for ${file.path}`}
          title={file.path}
          onClick={() => onDiff?.(file.path, file.is_staged)}
        >
          <span className={deleted ? "line-through" : ""}>{filename}</span>
          {showDirectory && dir && (
            <span className="ml-1.5 text-[var(--color-wardian-text-muted)]">{dir}</span>
          )}
        </button>
        <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
          {file.is_staged && onUnstage && (
            <button
              className="p-0.5 rounded hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors"
              aria-label={`Unstage ${file.path}`}
              title="Unstage"
              onClick={() => onUnstage(file.path)}
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M20 12H4" />
              </svg>
            </button>
          )}
          {!file.is_staged && onStage && (
            <button
              className="p-0.5 rounded hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors"
              aria-label={`Stage ${file.path}`}
              title="Stage"
              onClick={() => onStage(file.path)}
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
              </svg>
            </button>
          )}
          {!file.is_staged && onDiscard && (
            <button
              className="p-0.5 rounded hover:bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_80%)] text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-error)] transition-colors"
              aria-label={`Discard changes to ${file.path}`}
              title="Discard Changes"
              onClick={() => onDiscard(file.path)}
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M3 10h10a8 8 0 018 8v2M3 10l6 6m-6-6l6-6" />
              </svg>
            </button>
          )}
        </div>
        <span
          aria-label={statusLabel}
          title={statusLabel}
          className={`font-mono font-bold min-w-4 text-center shrink-0 ${colorClass}`}
        >
          {file.status}
        </span>
      </li>
    );
  };

  const renderTreeNodes = (nodes: FileTreeNode[], depth = 0): React.ReactNode =>
    nodes.map((node, index) => {
      if (node.type === "file") {
        return renderFileRow(node.file, index, depth, false);
      }

      const isOpen = !collapsedDirectories.has(node.path);
      return (
        <li key={node.path}>
          <button
            type="button"
            aria-expanded={isOpen}
            onContextMenu={(event) => openFolderContextMenu(event, node)}
            onClick={() =>
              setCollapsedDirectories((current) => {
                const next = new Set(current);
                if (next.has(node.path)) {
                  next.delete(node.path);
                } else {
                  next.add(node.path);
                }
                return next;
              })
            }
            className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-xs text-[var(--color-wardian-text-muted)] hover:bg-wardian-card-bg-muted hover:text-primary"
            style={{ paddingLeft: `${6 + depth * 14}px` }}
          >
            <svg
              className={`h-3 w-3 shrink-0 transition-transform ${isOpen ? "rotate-90" : ""}`}
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
            </svg>
            <span className="truncate">{node.name}</span>
          </button>
          {isOpen && <ul className="flex flex-col gap-0.5">{renderTreeNodes(node.children, depth + 1)}</ul>}
        </li>
      );
    });

  return (
    <>
      <ul className="flex flex-col gap-0.5">
        {displayMode === "tree"
          ? renderTreeNodes(buildFileTree(sortedFiles))
          : sortedFiles.map((file, index) => renderFileRow(file, index, 0, true))}
      </ul>
      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          items={contextMenu.items}
          onClose={() => setContextMenu(null)}
        />
      )}
    </>
  );
};
