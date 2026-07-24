import React, { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Archive, Check, ChevronDown, Download, GitBranch, List, ListTree, Plus, RefreshCw, RotateCcw, Trash2, Upload, X } from "lucide-react";
import {
  AgentConfig,
  AgentWorktreeSummary,
  GitBranchSummary,
  GitCommitChangeEntry,
  GitFileEntry,
  GitLogEntry,
  GitStashEntry,
} from "../../types";
import { GitFileList, type ResourceSortMode } from "./GitFileList";
import { GitDiffView, type GitDiffAction, type GitDiffHunkAction } from "./GitDiffView";
import { GitHistoryGraph, loadRefFilter, saveRefFilter, type GraphRefFilter } from "./GitHistoryGraph";
import { useConfirm } from "../../components/ConfirmDialog";
import { formatGitStatusError, type SelectedAgentGitStatus } from "./useSelectedAgentGitStatus";
import { ContextMenu, type ContextMenuItem } from "../../components/ContextMenu";
import { CompactOverflowButton } from "../../components/CompactOverflowButton";
import { useSettingsStore } from "../../store/useSettingsStore";

const DEFAULT_GIT_ERROR = "Unable to load git status.";
const MERGE_CONFLICT_STATUSES = new Set(["DD", "AU", "UD", "UA", "DU", "AA", "UU"]);
const HISTORY_PAGE_SIZE = 50;
const COMMIT_SUMMARY_LIMIT = 50;
const COMMIT_BODY_LINE_LIMIT = 72;

const normalizeComparablePath = (path: string): string => {
  const normalized = path
    .trim()
    .replace(/\\/g, "/")
    .replace(/^\/\/\?\/UNC\//i, "//")
    .replace(/^\/\/\?\//, "")
    .replace(/\/+$/g, "");
  return /^[a-z]:\//i.test(normalized) ? normalized.toLowerCase() : normalized;
};

const isAbsoluteResourcePath = (path: string) =>
  /^[a-z]:[\\/]/i.test(path) || path.startsWith("/") || path.startsWith("\\\\");

const resolveGitResourcePath = (rootPath: string, resourcePath: string) => {
  if (isAbsoluteResourcePath(resourcePath)) {
    return resourcePath;
  }
  return `${rootPath.replace(/[\\/]+$/g, "")}/${resourcePath.replace(/^[\\/]+/g, "")}`;
};

interface GitPanelProps {
  selectedAgentIds: Set<string>;
  agents: AgentConfig[];
  onAgentsUpdated: () => void;
  telemetry: Record<string, import("../../types").AgentTelemetry>;
  sourceControlStatus: SelectedAgentGitStatus;
}

interface ActiveWorktreeName {
  agentId: string;
  folder: string;
  name: string;
}

type PrimaryActionKind = "commit" | "publish" | "sync";
type ResourceDisplayMode = "tree" | "list";
type CommitMode = "staged" | "all";
interface CommitMessageValidation {
  message: string;
  count: string;
}

const sourceControlStorageRoot = (rootPath: string) => rootPath.trim().replace(/\\/g, "/") || "unknown-root";
const resourceDisplayModeKey = "wardian:source-control:resources:display-mode";
const legacyResourceDisplayModeKey = (rootPath: string) =>
  `wardian:source-control:resources:${sourceControlStorageRoot(rootPath)}:display-mode`;
const resourceSortModeKey = (rootPath: string) =>
  `wardian:source-control:resources:${sourceControlStorageRoot(rootPath)}:sort-mode`;
const commitActionStorageKey = "wardian:source-control:commit:last-action";

const loadResourceDisplayMode = (rootPath: string): ResourceDisplayMode => {
  if (typeof window === "undefined") return "list";
  const stored = window.localStorage.getItem(resourceDisplayModeKey);
  if (stored === "list" || stored === "tree") return stored;

  const legacyStored = window.localStorage.getItem(legacyResourceDisplayModeKey(rootPath));
  return legacyStored === "list" || legacyStored === "tree" ? legacyStored : "list";
};

const saveResourceDisplayMode = (rootPath: string, mode: ResourceDisplayMode) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(resourceDisplayModeKey, mode);
  window.localStorage.removeItem(legacyResourceDisplayModeKey(rootPath));
};

const loadResourceSortMode = (rootPath: string): ResourceSortMode => {
  if (typeof window === "undefined") return "status";
  const stored = window.localStorage.getItem(resourceSortModeKey(rootPath));
  return stored === "path" || stored === "name" || stored === "status" ? stored : "status";
};

const saveResourceSortMode = (rootPath: string, mode: ResourceSortMode) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(resourceSortModeKey(rootPath), mode);
};

const loadLastCommitMode = (): CommitMode | null => {
  if (typeof window === "undefined") return null;
  const stored = window.localStorage.getItem(commitActionStorageKey);
  return stored === "all" || stored === "staged" ? stored : null;
};

const saveLastCommitMode = (mode: CommitMode) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(commitActionStorageKey, mode);
};

const validateCommitMessage = (message: string): CommitMessageValidation | null => {
  const lines = message.replace(/\r\n/g, "\n").split("\n");
  const summaryLength = lines[0]?.length ?? 0;

  if (summaryLength > COMMIT_SUMMARY_LIMIT) {
    return {
      message: `Summary line is ${summaryLength} characters; VS Code marks commit subjects past ${COMMIT_SUMMARY_LIMIT} and body lines past ${COMMIT_BODY_LINE_LIMIT} for review.`,
      count: `${summaryLength}/${COMMIT_SUMMARY_LIMIT}`,
    };
  }

  const longBodyLine = lines.slice(1).find((line) => line.length > COMMIT_BODY_LINE_LIMIT);
  if (longBodyLine) {
    return {
      message: `Commit body line is ${longBodyLine.length} characters; VS Code marks body lines past ${COMMIT_BODY_LINE_LIMIT} for review.`,
      count: `${longBodyLine.length}/${COMMIT_BODY_LINE_LIMIT}`,
    };
  }

  return null;
};

export const GitPanel: React.FC<GitPanelProps> = ({ selectedAgentIds, agents, onAgentsUpdated, sourceControlStatus }) => {
  const confirm = useConfirm();
  const externalEditor = useSettingsStore((state) => state.externalEditor);
  const externalEditorCustomExecutable = useSettingsStore((state) => state.externalEditorCustomExecutable);
  const {
    rootPath,
    status,
    error: statusError,
    loading: statusLoading,
    changeEventRevision,
    refreshStatus,
  } = sourceControlStatus;
  const [panelError, setPanelError] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [commitMsg, setCommitMsg] = useState("");
  const [diffContent, setDiffContent] = useState<string | null>(null);
  const [diffFilePath, setDiffFilePath] = useState<string>("");
  const [diffActions, setDiffActions] = useState<GitDiffAction[]>([]);
  const [diffHunkActions, setDiffHunkActions] = useState<GitDiffHunkAction[]>([]);
  const [loading, setLoading] = useState(false);
  const [manualRefreshing, setManualRefreshing] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [initializingRepository, setInitializingRepository] = useState(false);
  const [isCloneRepositoryFormOpen, setIsCloneRepositoryFormOpen] = useState(false);
  const [cloneRepositoryUrl, setCloneRepositoryUrl] = useState("");
  const [cloningRepository, setCloningRepository] = useState(false);
  const [worktreeLoading, setWorktreeLoading] = useState(false);
  const [availableWorktrees, setAvailableWorktrees] = useState<AgentWorktreeSummary[]>([]);
  const [currentWorktreeName, setCurrentWorktreeName] = useState<ActiveWorktreeName | null>(null);
  const [isNamingWorktree, setIsNamingWorktree] = useState(false);
  const [worktreeName, setWorktreeName] = useState("");
  const [groupContextMenu, setGroupContextMenu] = useState<{
    x: number;
    y: number;
    items: ContextMenuItem[];
  } | null>(null);
  const [commitActionMenu, setCommitActionMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
  const [branchMenu, setBranchMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
  const [sourceControlActionMenu, setSourceControlActionMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
  const [stashMenu, setStashMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
  const [isCreatingBranch, setIsCreatingBranch] = useState(false);
  const [branchName, setBranchName] = useState("");
  const createWorktreeButtonRef = useRef<HTMLButtonElement | null>(null);
  const branchNameInputRef = useRef<HTMLInputElement | null>(null);
  const worktreeNameInputRef = useRef<HTMLInputElement | null>(null);

  // Collapsible sections
  const [stagedOpen, setStagedOpen] = useState(true);
  const [mergeOpen, setMergeOpen] = useState(true);
  const [changesOpen, setChangesOpen] = useState(true);
  const [untrackedOpen, setUntrackedOpen] = useState(true);
  const [historyOpen, setHistoryOpen] = useState(true);
  const [resourceDisplayMode, setResourceDisplayMode] = useState<ResourceDisplayMode>(() => loadResourceDisplayMode(""));
  const [resourceSortMode, setResourceSortMode] = useState<ResourceSortMode>(() => loadResourceSortMode(""));
  const [lastCommitMode, setLastCommitMode] = useState<CommitMode | null>(() => loadLastCommitMode());

  // Commit history
  const [history, setHistory] = useState<GitLogEntry[]>([]);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [historyLimit, setHistoryLimit] = useState(HISTORY_PAGE_SIZE);
  const [historyLoadingMore, setHistoryLoadingMore] = useState(false);
  const [historyRefFilter, setHistoryRefFilter] = useState<GraphRefFilter>(() => loadRefFilter(""));

  const selectedAgentId = selectedAgentIds.size === 1 ? Array.from(selectedAgentIds)[0] : null;
  const selectedAgent = agents.find((a) => a.session_id === selectedAgentId) ?? null;
  const selectedWorkspaceRevision = [
    selectedAgent?.folder ?? "",
    selectedAgent?.git_worktree ? "worktree" : "main",
    selectedAgent?.git_worktree_source ?? "",
    selectedAgent?.git_worktree_folder ?? "",
  ].join("|");
  const error = panelError ?? statusError;
  const hasStatus = status !== null;
  const errorMessage = error === null ? "" : error.trim() || DEFAULT_GIT_ERROR;
  const errorWorkspacePath = rootPath ?? selectedAgent?.folder ?? "";
  const isNotGitRepoError =
    errorMessage.toLowerCase().includes("not a git repository") ||
    errorMessage.toLowerCase().includes("not a git directory");
  const isWorktreeActive = selectedAgent?.git_worktree === true || (status?.branch?.startsWith("wardian/") ?? false);
  const selectedSourceFolder = normalizeComparablePath(selectedAgent?.git_worktree_source ?? selectedAgent?.folder ?? "");
  const selectedRuntimeFolder = selectedAgent?.folder?.replace(/\\/g, "/") ?? rootPath ?? "";
  const selectedWorktreeFolder = selectedAgent?.git_worktree_folder?.replace(/\\/g, "/") ?? "";
  const worktreeFolderName = (folder: string) => {
    const normalized = folder.replace(/\\/g, "/").replace(/\/+$/g, "");
    return normalized.split("/").pop()?.trim() ?? "";
  };
  const currentSummaryName =
    currentWorktreeName?.agentId === selectedAgentId &&
    (!selectedWorktreeFolder || currentWorktreeName.folder === selectedWorktreeFolder)
      ? currentWorktreeName.name
      : null;
  const activeWorktreeName =
    currentSummaryName ||
    worktreeFolderName(selectedWorktreeFolder) ||
    worktreeFolderName(selectedRuntimeFolder) ||
    status?.branch ||
    "worktree";
  const hasStaleWorktreeAssignment =
    selectedAgent?.git_worktree === true &&
    selectedWorktreeFolder.length > 0 &&
    selectedRuntimeFolder.length > 0 &&
    selectedRuntimeFolder !== selectedWorktreeFolder;

  const formatError = formatGitStatusError;

  const handleRevealWorkspace = async () => {
    if (!errorWorkspacePath) return;
    try {
      await invoke("reveal_in_explorer", { path: errorWorkspacePath });
    } catch (err) {
      setPanelError(formatError(err));
    }
  };

  const handleInitializeRepository = async () => {
    if (!errorWorkspacePath) return;
    setInitializingRepository(true);
    setPanelError(null);
    setOperationError(null);
    try {
      await invoke("git_init", { cwd: errorWorkspacePath });
      await refreshStatus();
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      setInitializingRepository(false);
    }
  };

  const handleCloneRepository = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const repository = cloneRepositoryUrl.trim();
    if (!errorWorkspacePath || !repository) return;

    setCloningRepository(true);
    setPanelError(null);
    setOperationError(null);
    try {
      await invoke("git_clone_repository", { cwd: errorWorkspacePath, repository });
      setCloneRepositoryUrl("");
      setIsCloneRepositoryFormOpen(false);
      await refreshStatus();
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      setCloningRepository(false);
    }
  };

  const slugifyWorktreeName = (name: string) => {
    const slug = name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "");
    return slug || "agent";
  };

  const worktreeNameSlug = slugifyWorktreeName(worktreeName);
  const isWorktreeNameDuplicate = availableWorktrees.some(
    (worktree) => slugifyWorktreeName(worktree.name) === worktreeNameSlug,
  );
  const canCreateNamedWorktree = worktreeName.trim().length > 0 && !isWorktreeNameDuplicate && !worktreeLoading;

  useEffect(() => {
    setHistory([]);
    setHistoryError(null);
    setHistoryLimit(HISTORY_PAGE_SIZE);
    setHistoryLoadingMore(false);
    setOperationError(null);
    setPanelError(null);
    setDiffContent(null);
    setDiffActions([]);
    setDiffHunkActions([]);
    setCloneRepositoryUrl("");
    setIsCloneRepositoryFormOpen(false);
    setResourceDisplayMode(loadResourceDisplayMode(rootPath ?? ""));
    setResourceSortMode(loadResourceSortMode(rootPath ?? ""));
  }, [selectedAgentId, selectedWorkspaceRevision]);

  useEffect(() => {
    setResourceDisplayMode(loadResourceDisplayMode(rootPath ?? ""));
    setResourceSortMode(loadResourceSortMode(rootPath ?? ""));
    setHistoryRefFilter(loadRefFilter(rootPath ?? ""));
  }, [rootPath]);

  // Fetch commit history when root changes or after a commit
  const refreshHistory = useCallback(async () => {
    if (!rootPath) return;
    try {
      const args: {
        cwd: string;
        count: number;
        revision?: string;
        all?: boolean;
      } = { cwd: rootPath, count: historyLimit };
      if (historyRefFilter === "all") {
        args.all = true;
      } else if (historyRefFilter === "current" && status?.branch) {
        args.revision = status.branch;
      } else if (historyRefFilter === "upstream" && status?.upstream) {
        args.revision = status.upstream;
      } else if (historyRefFilter.startsWith("ref:")) {
        args.revision = historyRefFilter.slice("ref:".length);
      }

      const log = await invoke<GitLogEntry[]>("git_log", args);
      setHistory(log);
      setHistoryError(null);
    } catch (err) {
      setHistory([]);
      setHistoryError(formatError(err));
    } finally {
      setHistoryLoadingMore(false);
    }
  }, [historyLimit, historyRefFilter, rootPath, status?.branch, status?.upstream]);

  const handleHistoryRefFilterChange = useCallback((nextFilter: GraphRefFilter) => {
    saveRefFilter(rootPath ?? "", nextFilter);
    setHistory([]);
    setHistoryError(null);
    setHistoryLoadingMore(false);
    setHistoryLimit(HISTORY_PAGE_SIZE);
    setHistoryRefFilter(nextFilter);
  }, [rootPath]);

  const loadMoreHistory = () => {
    if (!rootPath || historyLoadingMore) return;
    setHistoryLoadingMore(true);
    setHistoryLimit((current) => current + HISTORY_PAGE_SIZE);
  };

  useEffect(() => {
    if (!rootPath || !hasStatus) {
      setHistory([]);
      setHistoryError(null);
      return;
    }
    void refreshHistory();
  }, [changeEventRevision, hasStatus, refreshHistory, rootPath]);

  useEffect(() => {
    let isMounted = true;
    const fetchWorktrees = async () => {
      if (!selectedAgentId || !selectedSourceFolder) {
        setAvailableWorktrees([]);
        setCurrentWorktreeName(null);
        return;
      }
      try {
        const summaries = await invoke<AgentWorktreeSummary[]>("list_agent_worktrees");
        if (!isMounted) return;
        const currentWorktree = selectedAgent?.git_worktree_folder?.replace(/\\/g, "/") ?? "";
        const currentSummary = summaries.find((worktree) => {
          const worktreeFolder = worktree.worktree_folder.replace(/\\/g, "/");
          return worktreeFolder === currentWorktree || worktree.member_agent_ids.includes(selectedAgentId);
        });
        setCurrentWorktreeName(
          currentSummary
            ? {
                agentId: selectedAgentId,
                folder: currentSummary.worktree_folder.replace(/\\/g, "/"),
                name: currentSummary.name,
              }
            : null,
        );
        setAvailableWorktrees(
          summaries.filter((worktree) => {
            const sameSource = normalizeComparablePath(worktree.source_folder) === selectedSourceFolder;
            const notCurrent = worktree.worktree_folder.replace(/\\/g, "/") !== currentWorktree;
            const notMember = !worktree.member_agent_ids.includes(selectedAgentId);
            return sameSource && notCurrent && notMember;
          }),
        );
      } catch {
        if (isMounted) {
          setAvailableWorktrees([]);
          setCurrentWorktreeName(null);
        }
      }
    };

    fetchWorktrees();
    return () => {
      isMounted = false;
    };
  }, [selectedAgentId, selectedSourceFolder, selectedWorkspaceRevision, selectedAgent?.git_worktree_folder]);

  useEffect(() => {
    if (!isNamingWorktree) return;
    const input = worktreeNameInputRef.current;
    if (!input) return;
    input.focus();
    input.select();
  }, [isNamingWorktree]);

  useEffect(() => {
    if (!isCreatingBranch) return;
    const input = branchNameInputRef.current;
    if (!input) return;
    input.focus();
    input.select();
  }, [isCreatingBranch]);

  // File operations
  const handleStage = async (path: string) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      await invoke("git_stage", { cwd: rootPath, paths: [path] });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleUnstage = async (path: string) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      await invoke("git_unstage", { cwd: rootPath, paths: [path] });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleDiscard = async (path: string) => {
    if (!rootPath) return;
    if (!(await confirm(`Discard changes to ${path}?`))) return;
    setOperationError(null);
    try {
      await invoke("git_discard_changes", { cwd: rootPath, paths: [path] });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleDiscardPaths = async (paths: string[], label?: string) => {
    if (!rootPath || paths.length === 0) return;
    const target = label ? `${label}/` : `${paths.length} files`;
    if (!(await confirm(`Discard changes in ${target}?`))) return;
    setOperationError(null);
    try {
      await invoke("git_discard_changes", { cwd: rootPath, paths });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleIgnorePaths = async (paths: string[]) => {
    if (!rootPath || paths.length === 0) return;
    setOperationError(null);
    try {
      await invoke("git_ignore", { cwd: rootPath, paths });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleDiff = async (path: string, staged: boolean) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      const diff = await invoke<string>("git_diff_file", { cwd: rootPath, path, staged });
      setDiffContent(diff);
      setDiffFilePath(path);
      setDiffActions([
        staged
          ? { label: "Unstage Changes", onClick: () => void handleUnstage(path) }
          : { label: "Stage Changes", onClick: () => void handleStage(path) },
      ]);
      setDiffHunkActions([
        staged
          ? { label: "Unstage Hunk", onClick: (patch) => void handleApplyDiffHunk(patch, true) }
          : { label: "Stage Hunk", onClick: (patch) => void handleApplyDiffHunk(patch, false) },
      ]);
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleApplyDiffHunk = async (patch: string, reverse: boolean) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      await invoke("git_apply_diff_hunk", { cwd: rootPath, patch, reverse });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleCompareWithWorkspace = async (path: string) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      const diff = await invoke<string>("git_diff_file_against_workspace", { cwd: rootPath, path });
      setDiffContent(diff);
      setDiffFilePath(`Workspace: ${path}`);
      setDiffActions([]);
      setDiffHunkActions([]);
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleOpenFile = async (path: string) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      await invoke("open_in_external_editor", {
        path: resolveGitResourcePath(rootPath, path),
        editor: {
          external_editor: externalEditor,
          external_editor_custom_executable: externalEditorCustomExecutable.trim() || null,
        },
      });
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleOpenHeadFile = async (path: string) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      const content = await invoke<string>("git_show_file_revision", {
        cwd: rootPath,
        path,
        revision: "HEAD",
      });
      setDiffContent(content);
      setDiffFilePath(`HEAD: ${path}`);
      setDiffActions([]);
      setDiffHunkActions([]);
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleOpenHistoryFile = async (entry: GitLogEntry, change: GitCommitChangeEntry) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      const content = await invoke<string>("git_show_file_revision", {
        cwd: rootPath,
        path: change.path,
        revision: entry.hash,
      });
      setDiffContent(content);
      setDiffFilePath(`${entry.hash.slice(0, 8)}: ${change.path}`);
      setDiffActions([]);
      setDiffHunkActions([]);
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleViewHistoryChanges = async (entry: GitLogEntry) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      const diff = await invoke<string>("git_commit_diff", {
        cwd: rootPath,
        hash: entry.hash,
        parentHash: entry.parent_hashes?.[0] ?? null,
      });
      setDiffContent(diff);
      setDiffFilePath(`${entry.hash.slice(0, 8)}: ${entry.message}`);
      setDiffActions([]);
      setDiffHunkActions([]);
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleRevealFile = async (path: string) => {
    if (!rootPath) return;
    setOperationError(null);
    try {
      await invoke("reveal_in_explorer", { path: resolveGitResourcePath(rootPath, path) });
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleDiffGroup = async (label: string, files: GitFileEntry[]) => {
    if (!rootPath || files.length === 0) return;
    setOperationError(null);
    try {
      const diffs = await Promise.all(
        files.map(async (file) => {
          const diff = await invoke<string>("git_diff_file", {
            cwd: rootPath,
            path: file.path,
            staged: file.is_staged,
          });
          return diff.trim();
        }),
      );
      setDiffContent(diffs.filter(Boolean).join("\n"));
      setDiffFilePath(label);
      setDiffActions([]);
      setDiffHunkActions([]);
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  // Stage all / Unstage all
  const stagePaths = async (paths: string[]) => {
    if (!rootPath || paths.length === 0) return;
    setOperationError(null);
    try {
      await invoke("git_stage", { cwd: rootPath, paths });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const unstagePaths = async (paths: string[]) => {
    if (!rootPath || paths.length === 0) return;
    setOperationError(null);
    try {
      await invoke("git_unstage", { cwd: rootPath, paths });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const handleStageTrackedAll = async () => {
    await stagePaths(unstagedTracked.map((f) => f.path));
  };

  const handleStageUntrackedAll = async () => {
    await stagePaths(untrackedFiles.map((f) => f.path));
  };

  const handleUnstageStagedAll = async () => {
    await unstagePaths(stagedFiles.map((f) => f.path));
  };

  const handleStageMergeAll = async () => {
    const paths = mergeFiles.map((f) => f.path);
    if (paths.length === 0) return;
    await stagePaths(paths);
  };

  const openResourceGroupContextMenu = (
    event: React.MouseEvent,
    items: ContextMenuItem[],
  ) => {
    event.preventDefault();
    event.stopPropagation();
    setGroupContextMenu({
      x: event.clientX,
      y: event.clientY,
      items,
    });
  };

  const openStagedGroupContextMenu = (event: React.MouseEvent) =>
    openResourceGroupContextMenu(event, [
      { label: "Open Staged Changes", onClick: () => void handleDiffGroup("Staged changes", stagedFiles) },
      { label: "Unstage All Changes", onClick: () => void handleUnstageStagedAll() },
      { label: stagedOpen ? "Collapse" : "Expand", onClick: () => setStagedOpen((open) => !open) },
    ]);

  const openMergeGroupContextMenu = (event: React.MouseEvent) => {
    openResourceGroupContextMenu(event, [
      { label: "Stage All Merge Changes", onClick: () => void handleStageMergeAll() },
      { label: mergeOpen ? "Collapse" : "Expand", onClick: () => setMergeOpen((open) => !open) },
    ]);
  };

  const openTrackedGroupContextMenu = (event: React.MouseEvent) =>
    openResourceGroupContextMenu(event, [
      { label: "Open Changes", onClick: () => void handleDiffGroup("Changes", unstagedTracked) },
      {
        label: "Discard All Tracked Changes",
        danger: true,
        onClick: () => void handleDiscardPaths(unstagedTracked.map((file) => file.path)),
      },
      { label: "Stage All Tracked Changes", onClick: () => void handleStageTrackedAll() },
      { label: changesOpen ? "Collapse" : "Expand", onClick: () => setChangesOpen((open) => !open) },
    ]);

  const openUntrackedGroupContextMenu = (event: React.MouseEvent) =>
    openResourceGroupContextMenu(event, [
      { label: "Open Untracked Changes", onClick: () => void handleDiffGroup("Untracked", untrackedFiles) },
      {
        label: "Discard All Untracked Changes",
        danger: true,
        onClick: () => void handleDiscardPaths(untrackedFiles.map((file) => file.path)),
      },
      { label: "Stage All Untracked Changes", onClick: () => void handleStageUntrackedAll() },
      { label: untrackedOpen ? "Collapse" : "Expand", onClick: () => setUntrackedOpen((open) => !open) },
    ]);

  // Commit
  const handleCommit = async (mode: CommitMode = "staged") => {
    if (!rootPath || !commitMsg.trim() || !status) return;
    const unstaged = status.files.filter((f) => !f.is_staged).map((f) => f.path);
    const shouldStageUnstaged = mode === "all" || (!hasStagedFiles && unstaged.length > 0);
    setLoading(true);
    setOperationError(null);
    try {
      if (shouldStageUnstaged && unstaged.length > 0) {
        await invoke("git_stage", { cwd: rootPath, paths: unstaged });
      }
      await invoke("git_commit", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAmend = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_amend", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAmendNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_amend_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitStagedAmendNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_staged_amend_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAllAmendNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_all_amend_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitStagedAmend = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_staged_amend", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAllAmend = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_all_amend", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitNoVerify = async () => {
    if (!rootPath || !commitMsg.trim() || !status) return;
    const unstaged = status.files.filter((f) => !f.is_staged).map((f) => f.path);
    const shouldStageUnstaged = !hasStagedFiles && unstaged.length > 0;
    setLoading(true);
    setOperationError(null);
    try {
      if (shouldStageUnstaged) {
        await invoke("git_stage", { cwd: rootPath, paths: unstaged });
      }
      await invoke("git_commit_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitSigned = async () => {
    if (!rootPath || !commitMsg.trim() || !status) return;
    const unstaged = status.files.filter((f) => !f.is_staged).map((f) => f.path);
    const shouldStageUnstaged = !hasStagedFiles && unstaged.length > 0;
    setLoading(true);
    setOperationError(null);
    try {
      if (shouldStageUnstaged) {
        await invoke("git_stage", { cwd: rootPath, paths: unstaged });
      }
      await invoke("git_commit_signed", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitStagedSigned = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_staged_signed", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAllSigned = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_all_signed", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitSignedNoVerify = async () => {
    if (!rootPath || !commitMsg.trim() || !status) return;
    const unstaged = status.files.filter((f) => !f.is_staged).map((f) => f.path);
    const shouldStageUnstaged = !hasStagedFiles && unstaged.length > 0;
    setLoading(true);
    setOperationError(null);
    try {
      if (shouldStageUnstaged) {
        await invoke("git_stage", { cwd: rootPath, paths: unstaged });
      }
      await invoke("git_commit_signed_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitStagedSignedNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_staged_signed_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAllSignedNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_all_signed_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitStagedNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_staged_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitAllNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_all_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitEmpty = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_empty", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleCommitEmptyNoVerify = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_commit_empty_no_verify", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleUndoLastCommit = async () => {
    if (!rootPath) return;
    if (!(await confirm("Undo last commit and keep its changes in the working tree?"))) return;
    setLoading(true);
    setOperationError(null);
    try {
      const message = await invoke<string>("git_undo_last_commit", { cwd: rootPath });
      setCommitMsg(message);
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  const handleAbortRebase = async () => {
    if (!rootPath) return;
    setLoading(true);
    setOperationError(null);
    try {
      await invoke("git_rebase_abort", { cwd: rootPath });
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setLoading(false);
    }
  };

  // Worktree action — moves the provider runtime by forcing a fresh session in the selected tree.
  const beginCreateWorktree = () => {
    if (!selectedAgent) return;
    setWorktreeName(selectedAgent.session_name || "worktree");
    setIsNamingWorktree(true);
  };

  const cancelCreateWorktree = () => {
    setIsNamingWorktree(false);
    setWorktreeName("");
    requestAnimationFrame(() => createWorktreeButtonRef.current?.focus());
  };

  const createNamedWorktree = async () => {
    if (!selectedAgent || !selectedAgentId || !canCreateNamedWorktree) return;
    setWorktreeLoading(true);
    try {
      await invoke("enable_agent_worktree", {
        sessionId: selectedAgentId,
        worktreeName: worktreeName.trim(),
      });
      setIsNamingWorktree(false);
      setWorktreeName("");
      onAgentsUpdated();
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      setWorktreeLoading(false);
    }
  };

  const handleWorktreeAction = async (enable: boolean) => {
    if (!selectedAgent || !selectedAgentId) return;
    if (enable) {
      beginCreateWorktree();
      return;
    }
    setWorktreeLoading(true);
    try {
      await invoke("disable_agent_worktree", {
        sessionId: selectedAgentId,
      });
      onAgentsUpdated();
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      setWorktreeLoading(false);
    }
  };

  const handleJoinWorktree = async (worktree: AgentWorktreeSummary) => {
    if (!selectedAgent || !selectedAgentId) return;
    setWorktreeLoading(true);
    try {
      await invoke("assign_agent_worktree", {
        sessionId: selectedAgentId,
        worktreeFolder: worktree.worktree_folder,
      });
      onAgentsUpdated();
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      setWorktreeLoading(false);
    }
  };

  const handleDeleteWorktree = async (worktree: AgentWorktreeSummary) => {
    if (worktree.member_agent_ids.length > 0) return;
    const confirmed = await confirm(
      `Delete worktree "${worktree.name}"?\n\nThis removes the Git worktree folder, discards any local changes, and keeps the branch. Reusing this name later reattaches that branch.`,
    );
    if (!confirmed) return;

    setWorktreeLoading(true);
    try {
      await invoke("delete_agent_worktree", {
        worktreeFolder: worktree.worktree_folder,
        sourceFolder: worktree.source_folder,
        force: true,
      });
      setAvailableWorktrees((current) =>
        current.filter((candidate) => candidate.worktree_folder !== worktree.worktree_folder),
      );
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      onAgentsUpdated();
      setWorktreeLoading(false);
    }
  };

  const handleActivateAssignedWorktree = async () => {
    if (!selectedAgent || !selectedAgentId || !selectedWorktreeFolder) return;
    setWorktreeLoading(true);
    try {
      await invoke("assign_agent_worktree", {
        sessionId: selectedAgentId,
        worktreeFolder: selectedWorktreeFolder,
      });
      onAgentsUpdated();
    } catch (err) {
      setPanelError(formatError(err));
    } finally {
      setWorktreeLoading(false);
    }
  };

  // Pull / Push
  const handlePull = async () => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_pull", { cwd: rootPath });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handlePush = async () => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_push", { cwd: rootPath });
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleFetch = async () => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_fetch", { cwd: rootPath });
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const refreshAfterSourceControlOperation = async () => {
    await refreshStatus();
    await refreshHistory();
  };

  const handleStashPush = async (includeUntracked: boolean) => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_push", { cwd: rootPath, includeUntracked });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashStaged = async () => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_staged", { cwd: rootPath });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashPopLatest = async () => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_pop_latest", { cwd: rootPath });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const checkoutBranch = async (branch: string) => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke("git_checkout_branch", { cwd: rootPath, branch });
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashApplyLatest = async () => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_apply_latest", { cwd: rootPath });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashApply = async (stash: GitStashEntry) => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_apply", { cwd: rootPath, stash: stash.selector });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashPop = async (stash: GitStashEntry) => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_pop", { cwd: rootPath, stash: stash.selector });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashDrop = async (stash: GitStashEntry) => {
    if (!rootPath) return;
    if (!(await confirm(`Drop stash ${stash.selector}? This cannot be undone.`))) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_drop", { cwd: rootPath, stash: stash.selector });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleStashDropAll = async () => {
    if (!rootPath) return;
    if (!(await confirm("Drop all stashes for this workspace? This cannot be undone."))) return;
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke<string>("git_stash_drop_all", { cwd: rootPath });
      await refreshAfterSourceControlOperation();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleShowStash = async (stash: GitStashEntry) => {
    if (!rootPath) return;
    setSyncing(true);
    setOperationError(null);
    try {
      const diff = await invoke<string>("git_show_stash", { cwd: rootPath, stash: stash.selector });
      setDiffContent(diff);
      setDiffFilePath(`Stash ${stash.selector}`);
      setDiffActions([]);
      setDiffHunkActions([]);
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const openStashPickerMenu = async (onSelect: (stash: GitStashEntry) => void) => {
    if (!rootPath) return;
    const menuX = sourceControlActionMenu?.x ?? 0;
    const menuY = sourceControlActionMenu?.y ?? 0;
    setSyncing(true);
    setOperationError(null);
    try {
      const stashes = await invoke<GitStashEntry[]>("git_list_stashes", { cwd: rootPath });
      if (stashes.length === 0) {
        setOperationError("No stashes found for this workspace.");
        return;
      }
      setStashMenu({
        x: menuX,
        y: menuY,
        items: stashes.map((stash) => ({
          label: `${stash.selector} ${stash.message}`.trim(),
          icon: <Archive className="h-3.5 w-3.5" />,
          onClick: () => onSelect(stash),
        })),
      });
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const openStashViewMenu = async () => {
    await openStashPickerMenu((stash) => void handleShowStash(stash));
  };

  const openStashApplyMenu = async () => {
    await openStashPickerMenu((stash) => void handleStashApply(stash));
  };

  const openStashPopMenu = async () => {
    await openStashPickerMenu((stash) => void handleStashPop(stash));
  };

  const openStashDropMenu = async () => {
    await openStashPickerMenu((stash) => void handleStashDrop(stash));
  };

  const openSourceControlActionMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const rect = event.currentTarget.getBoundingClientRect();
    const branchItems: ContextMenuItem[] = [
      {
        label: "Checkout to...",
        icon: <GitBranch className="h-3.5 w-3.5" />,
        onClick: () => void openCheckoutMenuFromOverflow(),
      },
      {
        label: "Create Branch...",
        icon: <GitBranch className="h-3.5 w-3.5" />,
        onClick: () => {
          setBranchName("");
          setIsCreatingBranch(true);
        },
      },
    ];
    const syncItems: ContextMenuItem[] = [
      {
        label: "Fetch",
        icon: <Download className="h-3.5 w-3.5" />,
        onClick: () => void handleFetch(),
      },
      {
        label: "Pull",
        icon: <Download className="h-3.5 w-3.5" />,
        onClick: () => void handlePull(),
      },
      {
        label: pushTitle,
        icon: <Upload className="h-3.5 w-3.5" />,
        onClick: () => void handlePush(),
      },
    ];
    const viewItems: ContextMenuItem[] = [
      {
        label: "Use Tree View",
        icon: resourceDisplayMode === "tree" ? <Check className="h-3.5 w-3.5" /> : <ListTree className="h-3.5 w-3.5" />,
        onClick: () => updateResourceDisplayMode("tree"),
      },
      {
        label: "Use List View",
        icon: resourceDisplayMode === "list" ? <Check className="h-3.5 w-3.5" /> : <List className="h-3.5 w-3.5" />,
        onClick: () => updateResourceDisplayMode("list"),
      },
      { divider: true },
      {
        label: "Sort by Path",
        icon: resourceSortMode === "path" ? <Check className="h-3.5 w-3.5" /> : <List className="h-3.5 w-3.5" />,
        onClick: () => updateResourceSortMode("path"),
      },
      {
        label: "Sort by Name",
        icon: resourceSortMode === "name" ? <Check className="h-3.5 w-3.5" /> : <List className="h-3.5 w-3.5" />,
        onClick: () => updateResourceSortMode("name"),
      },
      {
        label: "Sort by Status",
        icon: resourceSortMode === "status" ? <Check className="h-3.5 w-3.5" /> : <List className="h-3.5 w-3.5" />,
        onClick: () => updateResourceSortMode("status"),
      },
    ];
    const stashItems: ContextMenuItem[] = [
      {
        label: "Stash Changes",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void handleStashPush(false),
      },
      {
        label: "Stash Changes Including Untracked",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void handleStashPush(true),
      },
      {
        label: "Stash Staged",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void handleStashStaged(),
      },
      { divider: true },
      {
        label: "Apply Latest Stash",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void handleStashApplyLatest(),
      },
      {
        label: "Apply Stash...",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void openStashApplyMenu(),
      },
      {
        label: "Pop Latest Stash",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void handleStashPopLatest(),
      },
      {
        label: "Pop Stash...",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void openStashPopMenu(),
      },
      {
        label: "View Stash...",
        icon: <Archive className="h-3.5 w-3.5" />,
        onClick: () => void openStashViewMenu(),
      },
      { divider: true },
      {
        label: "Drop Stash...",
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: () => void openStashDropMenu(),
      },
      {
        label: "Drop All Stashes...",
        icon: <Trash2 className="h-3.5 w-3.5" />,
        danger: true,
        onClick: () => void handleStashDropAll(),
      },
    ];

    setSourceControlActionMenu({
      x: rect.right - 220,
      y: rect.bottom + 4,
      items: [
        {
          label: "Branch",
          icon: <GitBranch className="h-3.5 w-3.5" />,
          subItems: branchItems,
        },
        {
          label: "Sync",
          icon: <RefreshCw className="h-3.5 w-3.5" />,
          subItems: syncItems,
        },
        {
          label: "View",
          icon: <ListTree className="h-3.5 w-3.5" />,
          subItems: viewItems,
        },
        {
          label: "Stash",
          icon: <Archive className="h-3.5 w-3.5" />,
          subItems: stashItems,
        },
      ],
    });
  };

  const createBranch = async () => {
    if (!rootPath) return;
    const branch = branchName.trim();
    if (!branch) {
      setOperationError("Branch name is required.");
      return;
    }
    setSyncing(true);
    setOperationError(null);
    try {
      await invoke("git_create_branch", { cwd: rootPath, branch });
      setBranchName("");
      setIsCreatingBranch(false);
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  const openCheckoutMenuAt = async (x: number, y: number) => {
    if (!rootPath) return;

    setOperationError(null);
    try {
      const branches = await invoke<GitBranchSummary[]>("git_list_branches", { cwd: rootPath });
      setBranchMenu({
        x,
        y,
        items: [
          {
            label: "Create Branch...",
            icon: <GitBranch className="h-3.5 w-3.5" />,
            onClick: () => {
              setBranchName("");
              setIsCreatingBranch(true);
            },
          },
          { divider: true },
          ...branches.map((branch) => ({
            label: branch.name,
            icon: branch.current ? <Check className="h-3.5 w-3.5" /> : undefined,
            onClick: () => {
              if (!branch.current) void checkoutBranch(branch.name);
            },
          })),
        ],
      });
    } catch (err) {
      setOperationError(formatError(err));
    }
  };

  const openCheckoutMenuFromOverflow = async () => {
    const menuX = sourceControlActionMenu?.x ?? 0;
    const menuY = sourceControlActionMenu?.y ?? 0;
    await openCheckoutMenuAt(menuX, menuY);
  };

  const handleSync = async () => {
    if (!rootPath || !status) return;
    setSyncing(true);
    setOperationError(null);
    try {
      if (status.behind > 0) {
        await invoke<string>("git_pull", { cwd: rootPath });
      }
      if (status.ahead > 0) {
        await invoke<string>("git_push", { cwd: rootPath });
      }
      await refreshStatus();
    } catch (err) {
      setOperationError(formatError(err));
    } finally {
      setSyncing(false);
    }
  };

  // No agent selected
  if (!selectedAgentId) {
    return (
      <div className="flex flex-col h-full w-full">
        <h2 className="text-sm font-bold text-primary tracking-tight mb-4">Source Control</h2>
        <div className="flex flex-col items-center justify-center flex-1 text-center p-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
          <div className="w-16 h-16 mb-4 text-gray-700/40">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="7" cy="5" r="2" style={{fill:'none'}} /><circle cx="7" cy="19" r="2" style={{fill:'none'}} /><circle cx="17" cy="12" r="2" style={{fill:'none'}} />
              <line x1="7" y1="7" x2="7" y2="17" /><path style={{fill:'none'}} d="M7 17 C7 13 17 13 17 12" />
            </svg>
          </div>
          <p className="text-xs text-muted italic">Select an agent to view source control.</p>
        </div>
      </div>
    );
  }

  // Not a git repo or error
  if (error !== null) {
    return (
      <div className="flex flex-col h-full w-full">
        <h2 className="text-sm font-bold text-primary tracking-tight mb-4">Source Control</h2>
        <div className="flex flex-col items-center justify-center flex-1 text-center p-6 animate-in fade-in slide-in-from-bottom-4 duration-500">
          <div className="w-16 h-16 mb-4 text-[color-mix(in_srgb,var(--color-wardian-text-muted),transparent_55%)]">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="7" cy="5" r="2" style={{fill:'none'}} /><circle cx="7" cy="19" r="2" style={{fill:'none'}} /><circle cx="17" cy="12" r="2" style={{fill:'none'}} />
              <line x1="7" y1="7" x2="7" y2="17" /><path style={{fill:'none'}} d="M7 17 C7 13 17 13 17 12" />
            </svg>
          </div>
          <h3 className="text-sm font-bold text-primary mb-2 tracking-wide">
            {isNotGitRepoError ? "Not a Git Repository" : "Unable to Load Source Control"}
          </h3>
          <p className="text-xs text-muted italic px-4">
            {isNotGitRepoError
              ? "The agent's workspace is not initialized as a git repository."
              : errorMessage}
          </p>
          {errorWorkspacePath && (
            <p className="mt-3 max-w-full px-2 text-[10px] text-[var(--color-wardian-text-muted)]">
              Workspace: <span className="font-mono text-primary break-all">{errorWorkspacePath}</span>
            </p>
          )}
          {errorWorkspacePath && (
            <div className="mt-4 flex flex-wrap justify-center gap-2">
              {isNotGitRepoError && (
                <button
                  type="button"
                  onClick={() => void handleInitializeRepository()}
                  disabled={initializingRepository}
                  className="inline-flex items-center gap-1.5 rounded border border-wardian-border px-2 py-1 text-[11px] text-[var(--color-wardian-text-muted)] transition-colors hover:bg-wardian-card-bg-muted hover:text-primary disabled:cursor-not-allowed disabled:opacity-60"
                >
                  <Plus size={12} aria-hidden="true" />
                  {initializingRepository ? "Initializing..." : "Initialize Repository"}
                </button>
              )}
              {isNotGitRepoError && (
                <button
                  type="button"
                  onClick={() => setIsCloneRepositoryFormOpen(true)}
                  disabled={cloningRepository}
                  className="inline-flex items-center gap-1.5 rounded border border-wardian-border px-2 py-1 text-[11px] text-[var(--color-wardian-text-muted)] transition-colors hover:bg-wardian-card-bg-muted hover:text-primary disabled:cursor-not-allowed disabled:opacity-60"
                >
                  <Download size={12} aria-hidden="true" />
                  Clone Repository...
                </button>
              )}
              <button
                type="button"
                onClick={() => void handleRevealWorkspace()}
                className="rounded border border-wardian-border px-2 py-1 text-[11px] text-[var(--color-wardian-text-muted)] transition-colors hover:bg-wardian-card-bg-muted hover:text-primary"
              >
                Reveal Workspace
              </button>
            </div>
          )}
          {isCloneRepositoryFormOpen && isNotGitRepoError && errorWorkspacePath && (
            <form
              onSubmit={(event) => void handleCloneRepository(event)}
              className="mt-3 flex w-full max-w-xs flex-col gap-2 rounded border border-wardian-border bg-wardian-card-bg-muted/40 p-2 text-left"
            >
              <label htmlFor="source-control-clone-url" className="text-[10px] font-semibold uppercase tracking-wide text-[var(--color-wardian-text-muted)]">
                Repository URL
              </label>
              <input
                id="source-control-clone-url"
                type="text"
                value={cloneRepositoryUrl}
                onChange={(event) => setCloneRepositoryUrl(event.target.value)}
                disabled={cloningRepository}
                className="w-full rounded border border-wardian-border bg-wardian-input-bg px-2 py-1 text-xs text-primary outline-none focus:border-[var(--color-wardian-accent)] disabled:opacity-60"
              />
              <div className="flex justify-end gap-2">
                <button
                  type="button"
                  onClick={() => {
                    setCloneRepositoryUrl("");
                    setIsCloneRepositoryFormOpen(false);
                  }}
                  disabled={cloningRepository}
                  className="rounded border border-wardian-border px-2 py-1 text-[11px] text-[var(--color-wardian-text-muted)] transition-colors hover:bg-wardian-card-bg-muted hover:text-primary disabled:cursor-not-allowed disabled:opacity-60"
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={cloningRepository || cloneRepositoryUrl.trim().length === 0}
                  className="rounded border border-[color-mix(in_srgb,var(--color-wardian-accent),transparent_35%)] px-2 py-1 text-[11px] text-[var(--color-wardian-accent)] transition-colors hover:bg-wardian-card-bg-muted disabled:cursor-not-allowed disabled:opacity-60"
                >
                  {cloningRepository ? "Cloning..." : "Clone"}
                </button>
              </div>
            </form>
          )}
        </div>
      </div>
    );
  }

  // Loading initial status
  if (!status) {
    return (
      <div className="flex flex-col h-full w-full">
        <h2 className="text-sm font-bold text-primary tracking-tight mb-4">Source Control</h2>
        <div role="status" aria-live="polite" className="text-sm text-[var(--color-wardian-text-muted)] animate-pulse px-1">
          {statusLoading ? "Loading source control..." : "Loading git status..."}
        </div>
      </div>
    );
  }

  const mergeFiles = status.files.filter((f) => !f.is_staged && MERGE_CONFLICT_STATUSES.has(f.status));
  const stagedFiles = status.files.filter((f) => f.is_staged && !MERGE_CONFLICT_STATUSES.has(f.status));
  const unstagedTracked = status.files.filter(
    (f) => !f.is_staged && f.status !== "?" && !MERGE_CONFLICT_STATUSES.has(f.status),
  );
  const untrackedFiles = status.files.filter((f) => !f.is_staged && f.status === "?");
  const hasStagedFiles = stagedFiles.length > 0;
  const hasUnstagedFiles = mergeFiles.length > 0 || unstagedTracked.length > 0 || untrackedFiles.length > 0;
  const canCommit = commitMsg.trim().length > 0 && (hasStagedFiles || hasUnstagedFiles);
  const commitValidation = validateCommitMessage(commitMsg);
  const hasPendingFiles = status.files.length > 0;
  const canCommitEmpty = commitMsg.trim().length > 0 && !hasPendingFiles && !loading;
  const canUndoLastCommit = history.length > 0 && !loading;
  const canAbortRebase = Boolean(status.rebase_in_progress) && !loading;
  const isCleanUnpublishedBranch = !hasPendingFiles && status.has_upstream === false;
  const isCleanDivergedBranch = !hasPendingFiles && status.has_upstream && (status.ahead > 0 || status.behind > 0);
  const primaryActionKind: PrimaryActionKind = isCleanUnpublishedBranch
    ? "publish"
    : isCleanDivergedBranch
      ? "sync"
      : "commit";
  const effectiveCommitMode: CommitMode = lastCommitMode === "all" || (lastCommitMode === "staged" && hasStagedFiles) ? lastCommitMode : "staged";
  const syncCountLabel = `${status.behind > 0 ? ` ↓${status.behind}` : ""}${status.ahead > 0 ? ` ↑${status.ahead}` : ""}`;
  const commitActionLabel =
    lastCommitMode === "all" ? "Commit All" : lastCommitMode === "staged" && hasStagedFiles ? "Commit Staged" : "Commit";
  const primaryActionLabel =
    primaryActionKind === "publish" ? "Publish Branch" : primaryActionKind === "sync" ? `Sync Changes${syncCountLabel}` : commitActionLabel;
  const primaryActionBusyLabel =
    primaryActionKind === "publish" ? "Publishing..." : primaryActionKind === "sync" ? "Syncing..." : "Committing...";
  const primaryActionDisabled = primaryActionKind === "commit" ? loading || !canCommit : syncing;
  const primaryActionTitle =
    primaryActionKind === "publish"
      ? `Publish ${status.branch}`
      : primaryActionKind === "sync"
        ? `Sync ${status.branch} with ${status.upstream ?? "upstream"}`
        : commitActionLabel;
  const isPrimaryActionBusy = primaryActionKind === "commit" ? loading : syncing;
  const pushTitle = status.has_upstream === false ? "Publish Branch" : "Push";
  const progressMessage = loading
    ? "Committing..."
    : syncing
      ? "Syncing source control..."
      : worktreeLoading
        ? "Updating worktree..."
        : manualRefreshing
          ? "Refreshing source control..."
          : statusLoading
            ? "Loading source control..."
            : null;

  const handlePrimaryAction = () => {
    if (primaryActionKind === "publish") {
      void handlePush();
      return;
    }
    if (primaryActionKind === "sync") {
      void handleSync();
      return;
    }
    void handleCommit(effectiveCommitMode);
  };

  const runCommitAction = (mode: CommitMode) => {
    setLastCommitMode(mode);
    saveLastCommitMode(mode);
    void handleCommit(mode);
  };

  const openCommitActionMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    if (primaryActionKind !== "commit" || (!canCommit && !canCommitEmpty && !canUndoLastCommit && !canAbortRebase)) return;

    const items: ContextMenuItem[] = [];
    if (canCommit && hasStagedFiles) {
      items.push({
        label: "Commit Staged",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => runCommitAction("staged"),
      });
    }
    if (canCommit) {
      items.push({
        label: "Commit All",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => runCommitAction("all"),
      });
    }
    if (canCommit) {
      items.push({
        label: "Commit (No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitNoVerify(),
      });
      items.push({
        label: "Commit Staged (No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitStagedNoVerify(),
      });
      items.push({
        label: "Commit All (No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAllNoVerify(),
      });
      items.push({
        label: "Commit (Signed Off)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitSigned(),
      });
      items.push({
        label: "Commit Staged (Signed Off)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitStagedSigned(),
      });
      items.push({
        label: "Commit All (Signed Off)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAllSigned(),
      });
      items.push({
        label: "Commit (Signed Off, No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitSignedNoVerify(),
      });
      items.push({
        label: "Commit Staged (Signed Off, No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitStagedSignedNoVerify(),
      });
      items.push({
        label: "Commit All (Signed Off, No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAllSignedNoVerify(),
      });
    }
    if (canCommitEmpty) {
      items.push({
        label: "Commit Empty",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitEmpty(),
      });
      items.push({
        label: "Commit Empty (No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitEmptyNoVerify(),
      });
    }
    if (canCommit && history.length > 0) {
      items.push({
        label: "Commit (Amend)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAmend(),
      });
      items.push({
        label: "Commit Staged (Amend)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitStagedAmend(),
      });
      items.push({
        label: "Commit All (Amend)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAllAmend(),
      });
      items.push({
        label: "Commit (Amend, No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAmendNoVerify(),
      });
      items.push({
        label: "Commit Staged (Amend, No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitStagedAmendNoVerify(),
      });
      items.push({
        label: "Commit All (Amend, No Verify)",
        icon: <Check className="h-3.5 w-3.5" />,
        onClick: () => void handleCommitAllAmendNoVerify(),
      });
    }
    if (canUndoLastCommit) {
      items.push({
        label: "Undo Last Commit",
        icon: <RotateCcw className="h-3.5 w-3.5" />,
        onClick: () => void handleUndoLastCommit(),
      });
    }
    if (canAbortRebase) {
      items.push({
        label: "Abort Rebase",
        icon: <RotateCcw className="h-3.5 w-3.5" />,
        onClick: () => void handleAbortRebase(),
      });
    }

    const rect = event.currentTarget.getBoundingClientRect();
    setCommitActionMenu({
      x: rect.right - 200,
      y: rect.bottom + 4,
      items,
    });
  };

  const updateResourceDisplayMode = (mode: ResourceDisplayMode) => {
    setResourceDisplayMode(mode);
    saveResourceDisplayMode(rootPath ?? "", mode);
  };

  const updateResourceSortMode = (mode: ResourceSortMode) => {
    setResourceSortMode(mode);
    saveResourceSortMode(rootPath ?? "", mode);
  };

  const handleRefreshSourceControl = async () => {
    setOperationError(null);
    setManualRefreshing(true);
    try {
      const refreshed = await refreshStatus();
      if (refreshed) {
        await refreshHistory();
      }
    } finally {
      setManualRefreshing(false);
    }
  };

  return (
    <div className="flex flex-col h-full w-full relative">
      <div className="mb-2 flex min-h-7 items-center gap-1">
        <h2 className="min-w-0 flex-1 truncate text-sm font-bold text-primary tracking-tight">Source Control</h2>
        <button
          type="button"
          onClick={handlePrimaryAction}
          disabled={primaryActionDisabled}
          aria-label="Run primary source control action"
          title={primaryActionTitle}
          className="p-1 rounded hover:bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors disabled:opacity-40"
        >
          {primaryActionKind === "publish" ? (
            <Upload className="h-3.5 w-3.5" aria-hidden="true" />
          ) : primaryActionKind === "sync" ? (
            <RefreshCw className={`h-3.5 w-3.5 ${syncing ? "animate-spin" : ""}`} aria-hidden="true" />
          ) : (
            <Check className="h-3.5 w-3.5" aria-hidden="true" />
          )}
        </button>
        <button
          type="button"
          onClick={() => void handleRefreshSourceControl()}
          disabled={statusLoading || manualRefreshing}
          aria-label="Refresh Source Control"
          title="Refresh Source Control"
          className="p-1 rounded hover:bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors disabled:opacity-40"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${manualRefreshing ? "animate-spin" : ""}`} aria-hidden="true" />
        </button>
        <CompactOverflowButton
          onClick={openSourceControlActionMenu}
          disabled={syncing}
          aria-label="More Source Control Actions"
          title="More Source Control Actions"
        />
      </div>
      {progressMessage && (
        <div
          role="status"
          aria-live="polite"
          className="mb-2 flex items-center gap-1.5 rounded border border-[color-mix(in_srgb,var(--color-wardian-processing),transparent_72%)] bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_90%)] px-2 py-1 text-[11px] text-[var(--color-wardian-processing)]"
        >
          <RefreshCw className="h-3 w-3 animate-spin" aria-hidden="true" />
          {progressMessage}
        </div>
      )}

      {isCreatingBranch && (
        <div className="mb-2 flex items-center gap-1 rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] px-1.5 py-1">
          <GitBranch className="h-3 w-3 shrink-0 text-[var(--color-wardian-accent)]" aria-hidden="true" />
          <input
            ref={branchNameInputRef}
            value={branchName}
            onChange={(event) => setBranchName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void createBranch();
              }
              if (event.key === "Escape") {
                event.preventDefault();
                setBranchName("");
                setIsCreatingBranch(false);
              }
            }}
            readOnly={syncing}
            placeholder="branch-name"
            className="min-w-0 flex-1 bg-transparent text-[11px] text-primary outline-none placeholder:text-[var(--color-wardian-text-muted)]"
          />
        </div>
      )}

      {operationError && (
        <div className="mb-3 px-2 py-1.5 rounded border border-[color-mix(in_srgb,var(--color-wardian-error),transparent_60%)] bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_88%)] text-[11px] text-[var(--color-wardian-error)]">
          {operationError}
        </div>
      )}

      {/* Worktree action row */}
      <div className="mb-3">
        {isWorktreeActive ? (
          <div className="flex items-center gap-2 px-2 py-1.5 rounded-lg bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_88%)] border border-[color-mix(in_srgb,var(--color-wardian-processing),transparent_70%)]">
            <svg className="w-3.5 h-3.5 text-[var(--color-wardian-processing)] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="7" cy="5" r="2" fill="none" /><circle cx="7" cy="19" r="2" fill="none" /><circle cx="17" cy="12" r="2" fill="none" />
              <line x1="7" y1="7" x2="7" y2="17" /><path fill="none" d="M7 17 C7 13 17 13 17 12" />
            </svg>
            <div className="min-w-0 flex-1">
                <div className="text-[11px] font-semibold text-[var(--color-wardian-processing)] truncate">Worktree: {activeWorktreeName}</div>
                <div className="text-[10px] font-mono text-muted truncate" title={selectedRuntimeFolder}>
                  {selectedRuntimeFolder || status.branch}
                </div>
              </div>
            {hasStaleWorktreeAssignment && (
              <button
                onClick={handleActivateAssignedWorktree}
                disabled={worktreeLoading}
                className="text-[10px] px-1.5 py-0.5 rounded border border-[color-mix(in_srgb,var(--color-wardian-processing),transparent_55%)] text-[var(--color-wardian-processing)] hover:bg-[color-mix(in_srgb,var(--color-wardian-processing),transparent_88%)] transition-colors disabled:opacity-40 shrink-0"
                title={selectedWorktreeFolder}
              >
                Start Fresh Here
              </button>
            )}
            <button
              onClick={() => handleWorktreeAction(false)}
              disabled={worktreeLoading}
              className="text-[var(--color-wardian-processing)] hover:text-[var(--color-wardian-error)] transition-colors disabled:opacity-40 shrink-0"
              title="Remove worktree assignment"
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2.5" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {isNamingWorktree ? (
              <div className={`w-full flex items-center gap-2 px-2 py-1.5 rounded-lg border bg-[var(--color-wardian-input-bg)] ${isWorktreeNameDuplicate ? "border-[var(--color-wardian-warning)]" : "border-wardian-border"}`}>
                <GitBranch className="w-3.5 h-3.5 shrink-0 text-[var(--color-wardian-accent)]" />
                <input
                  ref={worktreeNameInputRef}
                  value={worktreeName}
                  onChange={(e) => setWorktreeName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      createNamedWorktree();
                    }
                    if (e.key === "Escape") {
                      e.preventDefault();
                      cancelCreateWorktree();
                    }
                  }}
                  readOnly={worktreeLoading}
                  placeholder="worktree-name"
                  className={`min-w-0 flex-1 bg-transparent text-[11px] text-primary outline-none placeholder:text-[var(--color-wardian-text-muted)] ${isWorktreeNameDuplicate ? "text-[var(--color-wardian-warning)]" : ""}`}
                />
                <button
                  onClick={createNamedWorktree}
                  disabled={!canCreateNamedWorktree}
                  className="p-0.5 rounded text-[var(--color-wardian-success)] hover:bg-wardian-card-bg-muted transition-colors disabled:opacity-30 disabled:cursor-not-allowed shrink-0"
                  title="Create and start fresh"
                >
                  <Check className="w-3.5 h-3.5" />
                </button>
                <button
                  onClick={cancelCreateWorktree}
                  disabled={worktreeLoading}
                  className="p-0.5 rounded text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-error)] hover:bg-wardian-card-bg-muted transition-colors disabled:opacity-30 shrink-0"
                  title="Cancel"
                >
                  <X className="w-3.5 h-3.5" />
                </button>
              </div>
            ) : (
              <button
                ref={createWorktreeButtonRef}
                onClick={() => handleWorktreeAction(true)}
                disabled={worktreeLoading}
                className="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg border border-dashed border-wardian-border text-[var(--color-wardian-text-muted)] hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)] transition-colors group disabled:opacity-40"
                title="Create isolated worktree"
              >
                <GitBranch className="w-3.5 h-3.5 shrink-0" />
                <span className="text-[11px]">{worktreeLoading ? "Creating..." : "Create Worktree"}</span>
              </button>
            )}
            {availableWorktrees.length > 0 && (
              <div className="flex flex-col gap-1">
                <div className="text-[10px] uppercase tracking-wide text-muted px-1">Available Worktrees</div>
                {availableWorktrees.map((worktree) => (
                  <div
                    key={worktree.id}
                    className="w-full flex items-stretch rounded-lg border border-wardian-border overflow-hidden"
                    title={worktree.worktree_folder}
                  >
                    <button
                      onClick={() => handleJoinWorktree(worktree)}
                      disabled={worktreeLoading}
                      className="min-w-0 flex-1 flex items-center gap-2 px-2 py-1.5 text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-accent)] hover:bg-wardian-card-bg-muted transition-colors disabled:opacity-40"
                    >
                      <span className="text-[11px] truncate">Move to {worktree.name}</span>
                      <span className="ml-auto text-[10px] font-mono text-muted">{worktree.member_agent_ids.length}</span>
                    </button>
                    {worktree.can_delete && worktree.member_agent_ids.length === 0 && (
                      <button
                        onClick={() => handleDeleteWorktree(worktree)}
                        disabled={worktreeLoading}
                        className="w-8 shrink-0 flex items-center justify-center border-l border-wardian-border text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-error)] hover:bg-wardian-card-bg-muted transition-colors disabled:opacity-40"
                        title={`Delete ${worktree.name} worktree`}
                        aria-label={`Delete ${worktree.name} worktree`}
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Commit box — at top, matching VS Code layout */}
      <div className="pb-3 mb-1 border-b border-wardian-border/30 flex flex-col gap-2">
        <textarea
          className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-colors h-16 resize-none placeholder:text-[var(--color-wardian-text-muted)]"
          placeholder={`Message on ${status.branch} (Ctrl+Enter to commit)`}
          value={commitMsg}
          aria-describedby={commitValidation ? "commit-message-validation" : undefined}
          onChange={(e) => setCommitMsg(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              handleCommit();
            }
          }}
        />
        {commitValidation && (
          <div
            id="commit-message-validation"
            role="status"
            aria-label="Commit message validation"
            className="flex items-center gap-2 rounded border border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_55%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] px-2 py-1 text-[11px] text-[var(--color-wardian-warning)]"
          >
            <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
            <span className="min-w-0 flex-1">{commitValidation.message}</span>
            <span className="shrink-0 rounded bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_82%)] px-1.5 py-0.5 font-mono text-[10px]">
              {commitValidation.count}
            </span>
          </div>
        )}
        <div className="flex w-full items-stretch gap-1">
          <button
            onClick={handlePrimaryAction}
            disabled={primaryActionDisabled}
            title={primaryActionTitle}
            className="min-w-0 flex-1 py-1.5 rounded text-xs font-bold transition-colors bg-[var(--color-wardian-accent)] text-black hover:opacity-90 disabled:opacity-30 disabled:cursor-not-allowed flex items-center justify-center gap-1.5"
          >
            {primaryActionKind === "publish" ? (
              <Upload className="w-3.5 h-3.5" />
            ) : primaryActionKind === "sync" ? (
              <RefreshCw className={`w-3.5 h-3.5 ${syncing ? "animate-spin" : ""}`} />
            ) : (
              <Check className="w-3.5 h-3.5" />
            )}
            {isPrimaryActionBusy ? primaryActionBusyLabel : primaryActionLabel}
          </button>
          <button
            type="button"
            onClick={openCommitActionMenu}
            disabled={primaryActionKind !== "commit" || (!canCommit && !canCommitEmpty && !canUndoLastCommit && !canAbortRebase)}
            aria-label="More Actions"
            aria-haspopup="menu"
            aria-expanded={commitActionMenu !== null}
            title="More Commit Actions"
            className="flex w-8 shrink-0 items-center justify-center rounded border border-[color-mix(in_srgb,var(--color-wardian-accent),transparent_35%)] bg-[var(--color-wardian-accent)] text-black transition-colors hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-30"
          >
            <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
      </div>

      {/* Scrollable content */}
      <div className="flex-1 overflow-y-auto no-scrollbar min-h-0 flex flex-col gap-2">
        {/* Staged Changes */}
        {stagedFiles.length > 0 && (
          <section>
            <div
              className="flex items-center gap-1.5 w-full py-1 group"
              onContextMenu={openStagedGroupContextMenu}
            >
              <button
                onClick={() => setStagedOpen(!stagedOpen)}
                className="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              >
                <svg className={`w-3 h-3 text-[var(--color-wardian-text-muted)] transition-transform ${stagedOpen ? "rotate-90" : ""}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
                </svg>
                <span className="text-[11px] font-bold text-[var(--color-wardian-text-muted)] tracking-wide">Staged changes</span>
              </button>
              <div className="flex-1" />
              <button
                onClick={(e) => { e.stopPropagation(); void handleUnstageStagedAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Unstage All Changes"
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M20 12H4" />
                </svg>
              </button>
              <span className="min-w-[18px] h-[18px] px-1 rounded bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] text-[10px] font-mono flex items-center justify-center ml-1">
                {stagedFiles.length}
              </span>
            </div>
            {stagedOpen && (
              <GitFileList
                files={stagedFiles}
                displayMode={resourceDisplayMode}
                sortMode={resourceSortMode}
                onUnstage={handleUnstage}
                onUnstagePaths={unstagePaths}
                onDiff={handleDiff}
                onCompareWithWorkspace={handleCompareWithWorkspace}
                onOpenFile={handleOpenFile}
                onOpenHeadFile={handleOpenHeadFile}
                onRevealFile={handleRevealFile}
              />
            )}
          </section>
        )}

        {/* Merge Changes */}
        {mergeFiles.length > 0 && (
          <section>
            <div
              className="flex items-center gap-1.5 w-full py-1 group"
              onContextMenu={openMergeGroupContextMenu}
            >
              <button
                onClick={() => setMergeOpen(!mergeOpen)}
                className="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              >
                <svg className={`w-3 h-3 text-[var(--color-wardian-text-muted)] transition-transform ${mergeOpen ? "rotate-90" : ""}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
                </svg>
                <span className="text-[11px] font-bold text-[var(--color-wardian-warning)] tracking-wide">Merge Changes</span>
              </button>
              <div className="flex-1" />
              <button
                onClick={(e) => { e.stopPropagation(); void handleStageMergeAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Stage All Merge Changes"
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
                </svg>
              </button>
              <span className="min-w-[18px] h-[18px] px-1 rounded bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] text-[10px] font-mono flex items-center justify-center ml-1">
                {mergeFiles.length}
              </span>
            </div>
            {mergeOpen && (
              <GitFileList
                files={mergeFiles}
                displayMode={resourceDisplayMode}
                sortMode={resourceSortMode}
                onStage={handleStage}
                onStagePaths={stagePaths}
                onDiscard={handleDiscard}
                onDiscardPaths={handleDiscardPaths}
                onIgnorePaths={handleIgnorePaths}
                onDiff={handleDiff}
                onOpenFile={handleOpenFile}
                onOpenHeadFile={handleOpenHeadFile}
                onRevealFile={handleRevealFile}
              />
            )}
          </section>
        )}

        {/* Changes (unstaged tracked) */}
        {unstagedTracked.length > 0 && (
          <section>
            <div
              className="flex items-center gap-1.5 w-full py-1 group"
              onContextMenu={openTrackedGroupContextMenu}
            >
              <button
                onClick={() => setChangesOpen(!changesOpen)}
                className="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              >
                <svg className={`w-3 h-3 text-[var(--color-wardian-text-muted)] transition-transform ${changesOpen ? "rotate-90" : ""}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
                </svg>
                <span className="text-[11px] font-bold text-[var(--color-wardian-text-muted)] tracking-wide">Changes</span>
              </button>
              <div className="flex-1" />
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  void handleDiscardPaths(unstagedTracked.map((file) => file.path));
                }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-error)] transition-all"
                title="Discard All Tracked Changes"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
              <button
                onClick={(e) => { e.stopPropagation(); void handleStageTrackedAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Stage All Tracked Changes"
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
                </svg>
              </button>
              <span className="min-w-[18px] h-[18px] px-1 rounded bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] text-[10px] font-mono flex items-center justify-center ml-1">
                {unstagedTracked.length}
              </span>
            </div>
            {changesOpen && (
              <GitFileList
                files={unstagedTracked}
                displayMode={resourceDisplayMode}
                sortMode={resourceSortMode}
                onStage={handleStage}
                onStagePaths={stagePaths}
                onDiscard={handleDiscard}
                onDiscardPaths={handleDiscardPaths}
                onIgnorePaths={handleIgnorePaths}
                onDiff={handleDiff}
                onOpenFile={handleOpenFile}
                onOpenHeadFile={handleOpenHeadFile}
                onRevealFile={handleRevealFile}
              />
            )}
          </section>
        )}

        {/* Untracked Files */}
        {untrackedFiles.length > 0 && (
          <section>
            <div
              className="flex items-center gap-1.5 w-full py-1 group"
              onContextMenu={openUntrackedGroupContextMenu}
            >
              <button
                onClick={() => setUntrackedOpen(!untrackedOpen)}
                className="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              >
                <svg className={`w-3 h-3 text-[var(--color-wardian-text-muted)] transition-transform ${untrackedOpen ? "rotate-90" : ""}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
                </svg>
                <span className="text-[11px] font-bold text-[var(--color-wardian-text-muted)] tracking-wide">Untracked</span>
              </button>
              <div className="flex-1" />
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  void handleDiscardPaths(untrackedFiles.map((file) => file.path));
                }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-[var(--color-wardian-error)] transition-all"
                title="Discard All Untracked Changes"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
              <button
                onClick={(e) => { e.stopPropagation(); void handleStageUntrackedAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Stage All Untracked Changes"
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
                </svg>
              </button>
              <span className="min-w-[18px] h-[18px] px-1 rounded bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] text-[10px] font-mono flex items-center justify-center ml-1">
                {untrackedFiles.length}
              </span>
            </div>
            {untrackedOpen && (
              <GitFileList
                files={untrackedFiles}
                displayMode={resourceDisplayMode}
                sortMode={resourceSortMode}
                onStage={handleStage}
                onStagePaths={stagePaths}
                onDiscard={handleDiscard}
                onDiscardPaths={handleDiscardPaths}
                onIgnorePaths={handleIgnorePaths}
                onDiff={handleDiff}
                onOpenFile={handleOpenFile}
                onOpenHeadFile={handleOpenHeadFile}
                onRevealFile={handleRevealFile}
              />
            )}
          </section>
        )}

        {/* Clean state */}
        {status.files.length === 0 && (
          <div className="flex flex-col items-center justify-center py-8 text-center">
            <svg className="w-10 h-10 mb-3 text-[var(--color-wardian-success)]/40" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M5 13l4 4L19 7" />
            </svg>
            <p className="text-xs text-muted italic">Working tree clean</p>
          </div>
        )}

        {/* Commit History */}
        {(history.length > 0 || historyError) && (
          <section className="mt-1 border-t border-wardian-border/30 pt-2">
            <button
              onClick={() => setHistoryOpen(!historyOpen)}
              className="flex items-center gap-1.5 w-full text-left py-1"
            >
              <svg className={`w-3 h-3 text-[var(--color-wardian-text-muted)] transition-transform ${historyOpen ? "rotate-90" : ""}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
              </svg>
              <span className="text-[11px] font-bold text-[var(--color-wardian-text-muted)] tracking-wide">History</span>
              <span className="min-w-[18px] h-[18px] px-1 rounded bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] text-[10px] font-mono flex items-center justify-center ml-1">
                {historyError ? "!" : history.length}
              </span>
            </button>
            {historyOpen && (
              <div className="flex flex-col">
                {historyError ? (
                  <div className="px-1 py-2 text-[11px] text-[var(--color-wardian-text-muted)]">
                    History unavailable
                  </div>
                ) : (
                  <GitHistoryGraph
                    entries={history}
                    branch={status.branch}
                    rootPath={rootPath ?? ""}
                    upstream={status.upstream}
                    ahead={status.ahead}
                    behind={status.behind}
                    selectedRefFilter={historyRefFilter}
                    hasMoreHistory={history.length >= historyLimit}
                    isLoadingMoreHistory={historyLoadingMore}
                    onRefFilterChange={handleHistoryRefFilterChange}
                    onLoadMoreHistory={loadMoreHistory}
                    onOpenHistoryFile={handleOpenHistoryFile}
                    onViewHistoryChanges={handleViewHistoryChanges}
                  />
                )}
              </div>
            )}
          </section>
        )}
      </div>

      {/* Diff overlay */}
      {diffContent !== null && (
        <GitDiffView
          diff={diffContent}
          filePath={diffFilePath}
          actions={diffActions}
          hunkActions={diffHunkActions}
          onClose={() => {
            setDiffContent(null);
            setDiffActions([]);
            setDiffHunkActions([]);
          }}
        />
      )}
      {groupContextMenu && (
        <ContextMenu
          x={groupContextMenu.x}
          y={groupContextMenu.y}
          items={groupContextMenu.items}
          onClose={() => setGroupContextMenu(null)}
        />
      )}
      {commitActionMenu && (
        <ContextMenu
          x={commitActionMenu.x}
          y={commitActionMenu.y}
          items={commitActionMenu.items}
          onClose={() => setCommitActionMenu(null)}
        />
      )}
      {sourceControlActionMenu && (
        <ContextMenu
          x={sourceControlActionMenu.x}
          y={sourceControlActionMenu.y}
          items={sourceControlActionMenu.items}
          onClose={() => setSourceControlActionMenu(null)}
        />
      )}
      {stashMenu && (
        <ContextMenu
          x={stashMenu.x}
          y={stashMenu.y}
          items={stashMenu.items}
          onClose={() => setStashMenu(null)}
        />
      )}
      {branchMenu && (
        <ContextMenu
          x={branchMenu.x}
          y={branchMenu.y}
          items={branchMenu.items}
          onClose={() => setBranchMenu(null)}
        />
      )}
    </div>
  );
};
