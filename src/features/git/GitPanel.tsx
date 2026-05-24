import React, { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, GitBranch, X } from "lucide-react";
import { AgentConfig, AgentWorktreeSummary, GitStatusResult, GitLogEntry } from "../../types";
import { GitFileList } from "./GitFileList";
import { GitDiffView } from "./GitDiffView";
import { useConfirm } from "../../components/ConfirmDialog";

const DEFAULT_GIT_ERROR = "Unable to load git status.";

interface GitPanelProps {
  selectedAgentIds: Set<string>;
  agents: AgentConfig[];
  onAgentsUpdated: () => void;
  telemetry: Record<string, import("../../types").AgentTelemetry>;
}

interface ActiveWorktreeName {
  agentId: string;
  folder: string;
  name: string;
}

export const GitPanel: React.FC<GitPanelProps> = ({ selectedAgentIds, agents, onAgentsUpdated }) => {
  const confirm = useConfirm();
  const [rootPath, setRootPath] = useState<string | null>(null);
  const [status, setStatus] = useState<GitStatusResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [commitMsg, setCommitMsg] = useState("");
  const [diffContent, setDiffContent] = useState<string | null>(null);
  const [diffFilePath, setDiffFilePath] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [worktreeLoading, setWorktreeLoading] = useState(false);
  const [availableWorktrees, setAvailableWorktrees] = useState<AgentWorktreeSummary[]>([]);
  const [currentWorktreeName, setCurrentWorktreeName] = useState<ActiveWorktreeName | null>(null);
  const [isNamingWorktree, setIsNamingWorktree] = useState(false);
  const [worktreeName, setWorktreeName] = useState("");
  const createWorktreeButtonRef = useRef<HTMLButtonElement | null>(null);
  const worktreeNameInputRef = useRef<HTMLInputElement | null>(null);

  // Collapsible sections
  const [stagedOpen, setStagedOpen] = useState(true);
  const [changesOpen, setChangesOpen] = useState(true);
  const [untrackedOpen, setUntrackedOpen] = useState(true);
  const [historyOpen, setHistoryOpen] = useState(true);

  // Commit history
  const [history, setHistory] = useState<GitLogEntry[]>([]);
  const [historyError, setHistoryError] = useState<string | null>(null);

  const selectedAgentId = selectedAgentIds.size === 1 ? Array.from(selectedAgentIds)[0] : null;
  const selectedAgent = agents.find((a) => a.session_id === selectedAgentId) ?? null;
  const selectedWorkspaceRevision = [
    selectedAgent?.folder ?? "",
    selectedAgent?.git_worktree ? "worktree" : "main",
    selectedAgent?.git_worktree_source ?? "",
    selectedAgent?.git_worktree_folder ?? "",
  ].join("|");
  const errorMessage = error === null ? "" : error.trim() || DEFAULT_GIT_ERROR;
  const isNotGitRepoError =
    errorMessage.toLowerCase().includes("not a git repository") ||
    errorMessage.toLowerCase().includes("not a git directory");
  const isWorktreeActive = selectedAgent?.git_worktree === true || (status?.branch?.startsWith("wardian/") ?? false);
  const selectedSourceFolder = (selectedAgent?.git_worktree_source ?? selectedAgent?.folder ?? "").replace(/\\/g, "/");
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

  const formatError = (err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    return message.trim() || DEFAULT_GIT_ERROR;
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

  // Resolve the agent's working directory
  useEffect(() => {
    const fetchPath = async () => {
      setStatus(null);
      setHistory([]);
      setHistoryError(null);
      setError(null);
      setRootPath(null);

      if (!selectedAgentId) {
        return;
      }
      try {
        const path = await invoke<string>("get_explorer_root", { sessionId: selectedAgentId });
        if (!path.trim()) {
          setError("Agent workspace is not configured.");
          return;
        }
        setRootPath(path);
      } catch (err) {
        setError(formatError(err));
      }
    };
    fetchPath();
  }, [selectedAgentId, selectedWorkspaceRevision]);

  // Fetch git status
  const refreshStatus = useCallback(async () => {
    if (!rootPath) return;
    try {
      const result = await invoke<GitStatusResult>("git_status", { cwd: rootPath });
      setStatus(result);
      setError(null);
    } catch (err) {
      setStatus(null);
      setError(formatError(err));
    }
  }, [rootPath]);

  // Fetch commit history when root changes or after a commit
  const refreshHistory = useCallback(async () => {
    if (!rootPath) return;
    try {
      const log = await invoke<GitLogEntry[]>("git_log", { cwd: rootPath, count: 50 });
      setHistory(log);
      setHistoryError(null);
    } catch (err) {
      setHistory([]);
      setHistoryError(formatError(err));
    }
  }, [rootPath]);

  // Watch .git/index + .git/HEAD via FSWatcher; refresh on change event
  useEffect(() => {
    if (!rootPath) return;

    refreshStatus();
    refreshHistory();

    invoke("git_watch", { cwd: rootPath }).catch(() => {});

    const unlistenPromise = listen<string>("git-changed", (event) => {
      if (event.payload === rootPath) {
        refreshStatus();
        refreshHistory();
      }
    });

    return () => {
      invoke("git_unwatch", { cwd: rootPath }).catch(() => {});
      unlistenPromise.then((fn) => fn());
    };
  }, [rootPath, refreshStatus, refreshHistory]);

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
            const sameSource = worktree.source_folder.replace(/\\/g, "/") === selectedSourceFolder;
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

  // File operations
  const handleStage = async (path: string) => {
    if (!rootPath) return;
    try {
      await invoke("git_stage", { cwd: rootPath, paths: [path] });
      await refreshStatus();
    } catch (err) {
      console.error("Stage failed:", err);
    }
  };

  const handleUnstage = async (path: string) => {
    if (!rootPath) return;
    try {
      await invoke("git_unstage", { cwd: rootPath, paths: [path] });
      await refreshStatus();
    } catch (err) {
      console.error("Unstage failed:", err);
    }
  };

  const handleDiscard = async (path: string) => {
    if (!rootPath) return;
    if (!(await confirm(`Discard changes to ${path}?`))) return;
    try {
      await invoke("git_discard_changes", { cwd: rootPath, paths: [path] });
      await refreshStatus();
    } catch (err) {
      console.error("Discard failed:", err);
    }
  };

  const handleDiff = async (path: string, staged: boolean) => {
    if (!rootPath) return;
    try {
      const diff = await invoke<string>("git_diff_file", { cwd: rootPath, path, staged });
      setDiffContent(diff);
      setDiffFilePath(path);
    } catch (err) {
      console.error("Diff failed:", err);
    }
  };

  // Stage all / Unstage all
  const handleStageAll = async () => {
    if (!rootPath || !status) return;
    const unstaged = status.files.filter((f) => !f.is_staged).map((f) => f.path);
    if (unstaged.length === 0) return;
    try {
      await invoke("git_stage", { cwd: rootPath, paths: unstaged });
      await refreshStatus();
    } catch (err) {
      console.error("Stage all failed:", err);
    }
  };

  const handleUnstageAll = async () => {
    if (!rootPath || !status) return;
    const staged = status.files.filter((f) => f.is_staged).map((f) => f.path);
    if (staged.length === 0) return;
    try {
      await invoke("git_unstage", { cwd: rootPath, paths: staged });
      await refreshStatus();
    } catch (err) {
      console.error("Unstage all failed:", err);
    }
  };

  // Commit
  const handleCommit = async () => {
    if (!rootPath || !commitMsg.trim()) return;
    setLoading(true);
    try {
      await invoke("git_commit", { cwd: rootPath, message: commitMsg.trim() });
      setCommitMsg("");
      await refreshStatus();
      await refreshHistory();
    } catch (err) {
      console.error("Commit failed:", err);
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
      await invoke("clear_agent_session", { sessionId: selectedAgentId });
      setIsNamingWorktree(false);
      setWorktreeName("");
      onAgentsUpdated();
    } catch (err) {
      setError(formatError(err));
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
      await invoke("clear_agent_session", { sessionId: selectedAgentId });
      onAgentsUpdated();
    } catch (err) {
      setError(formatError(err));
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
      await invoke("clear_agent_session", { sessionId: selectedAgentId });
      onAgentsUpdated();
    } catch (err) {
      setError(formatError(err));
    } finally {
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
      await invoke("clear_agent_session", { sessionId: selectedAgentId });
      onAgentsUpdated();
    } catch (err) {
      setError(formatError(err));
    } finally {
      setWorktreeLoading(false);
    }
  };

  // Pull / Push
  const handlePull = async () => {
    if (!rootPath) return;
    setSyncing(true);
    try {
      await invoke<string>("git_pull", { cwd: rootPath });
      await refreshStatus();
    } catch (err) {
      console.error("Pull failed:", err);
    } finally {
      setSyncing(false);
    }
  };

  const handlePush = async () => {
    if (!rootPath) return;
    setSyncing(true);
    try {
      await invoke<string>("git_push", { cwd: rootPath });
      await refreshStatus();
    } catch (err) {
      console.error("Push failed:", err);
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
          <div className="w-16 h-16 mb-4 text-gray-700/40">
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
        </div>
      </div>
    );
  }

  // Loading initial status
  if (!status) {
    return (
      <div className="flex flex-col h-full w-full">
        <h2 className="text-sm font-bold text-primary tracking-tight mb-4">Source Control</h2>
        <div className="text-sm text-[var(--color-wardian-text-muted)] animate-pulse px-1">Loading git status...</div>
      </div>
    );
  }

  const stagedFiles = status.files.filter((f) => f.is_staged);
  const unstagedTracked = status.files.filter((f) => !f.is_staged && f.status !== "?");
  const untrackedFiles = status.files.filter((f) => !f.is_staged && f.status === "?");
  const hasStagedFiles = stagedFiles.length > 0;

  return (
    <div className="flex flex-col h-full w-full relative">
      <h2 className="text-sm font-bold text-primary tracking-tight mb-2">Source Control</h2>

      {/* Branch bar */}
      <div className="flex items-center gap-2 py-1.5 mb-3 border-b border-wardian-border/30">
        <svg className="w-4 h-4 text-[var(--color-wardian-accent)] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="7" cy="5" r="2" style={{fill:'none'}} /><circle cx="7" cy="19" r="2" style={{fill:'none'}} /><circle cx="17" cy="12" r="2" style={{fill:'none'}} />
          <line x1="7" y1="7" x2="7" y2="17" /><path style={{fill:'none'}} d="M7 17 C7 13 17 13 17 12" />
        </svg>
        <span className="text-xs font-semibold text-primary truncate">{status.branch}</span>
        {status.ahead > 0 && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-[color-mix(in_srgb,var(--color-wardian-success),transparent_80%)] text-[var(--color-wardian-success)] font-mono">↑{status.ahead}</span>
        )}
        {status.behind > 0 && (
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_80%)] text-[var(--color-wardian-warning)] font-mono">↓{status.behind}</span>
        )}
        <div className="flex-1" />
        <button
          onClick={handlePull}
          disabled={syncing}
          className="p-1 rounded hover:bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors disabled:opacity-40"
          title="Pull"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
          </svg>
        </button>
        <button
          onClick={handlePush}
          disabled={syncing}
          className="p-1 rounded hover:bg-wardian-card-bg-muted text-[var(--color-wardian-text-muted)] hover:text-primary transition-colors disabled:opacity-40"
          title="Push"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" />
          </svg>
        </button>
      </div>

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
                  <button
                    key={worktree.id}
                    onClick={() => handleJoinWorktree(worktree)}
                    disabled={worktreeLoading}
                    className="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg border border-wardian-border text-[var(--color-wardian-text-muted)] hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)] transition-colors disabled:opacity-40"
                    title={worktree.worktree_folder}
                  >
                    <span className="text-[11px] truncate">Move to {worktree.name}</span>
                    <span className="ml-auto text-[10px] font-mono text-muted">{worktree.member_agent_ids.length}</span>
                  </button>
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
          placeholder="Message (Ctrl+Enter to commit)"
          value={commitMsg}
          onChange={(e) => setCommitMsg(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              handleCommit();
            }
          }}
        />
        <button
          onClick={handleCommit}
          disabled={loading || !commitMsg.trim() || !hasStagedFiles}
          className="w-full py-1.5 rounded text-xs font-bold transition-colors bg-[var(--color-wardian-accent)] text-black hover:opacity-90 disabled:opacity-30 disabled:cursor-not-allowed flex items-center justify-center gap-1.5"
        >
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2.5" d="M5 13l4 4L19 7" />
          </svg>
          {loading ? "Committing..." : "Commit"}
        </button>
      </div>

      {/* Scrollable content */}
      <div className="flex-1 overflow-y-auto no-scrollbar min-h-0 flex flex-col gap-2">
        {/* Staged Changes */}
        {stagedFiles.length > 0 && (
          <section>
            <div className="flex items-center gap-1.5 w-full py-1 group">
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
                onClick={(e) => { e.stopPropagation(); handleUnstageAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Unstage All"
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
                onUnstage={handleUnstage}
                onDiff={handleDiff}
              />
            )}
          </section>
        )}

        {/* Changes (unstaged tracked) */}
        {unstagedTracked.length > 0 && (
          <section>
            <div className="flex items-center gap-1.5 w-full py-1 group">
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
                onClick={(e) => { e.stopPropagation(); handleStageAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Stage All"
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
                onStage={handleStage}
                onDiscard={handleDiscard}
                onDiff={handleDiff}
              />
            )}
          </section>
        )}

        {/* Untracked Files */}
        {untrackedFiles.length > 0 && (
          <section>
            <div className="flex items-center gap-1.5 w-full py-1 group">
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
                onClick={(e) => { e.stopPropagation(); handleStageAll(); }}
                className="p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-wardian-card text-[var(--color-wardian-text-muted)] hover:text-primary transition-all"
                title="Stage All Untracked"
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
                onStage={handleStage}
                onDiff={handleDiff}
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
                  history.map((entry, i) => (
                    <div key={entry.hash} className="flex items-center gap-2 py-[3px] px-1 hover:bg-wardian-card-bg-muted rounded group cursor-default">
                      <div className="relative flex flex-col items-center shrink-0" style={{ width: 12 }}>
                        {i > 0 && <div className="absolute top-0 left-1/2 -translate-x-1/2 w-px bg-wardian-border/50" style={{ height: '50%' }} />}
                        <div className={`w-2 h-2 rounded-full border shrink-0 z-10 ${i === 0 ? 'bg-[var(--color-wardian-accent)] border-[var(--color-wardian-accent)]' : 'bg-transparent border-[var(--color-wardian-text-muted)]'}`} />
                        {i < history.length - 1 && <div className="absolute bottom-0 left-1/2 -translate-x-1/2 w-px bg-wardian-border/50" style={{ height: '50%' }} />}
                      </div>
                      <span className="text-[11px] text-primary truncate flex-1 leading-snug">{entry.message}</span>
                      <span className="text-[9px] font-mono text-[var(--color-wardian-text-muted)] shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">{entry.hash.slice(0, 7)}</span>
                    </div>
                  ))
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
          onClose={() => setDiffContent(null)}
        />
      )}
    </div>
  );
};
