import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentConfig, GitStatusResult } from "../../types";

const GIT_STATUS_POLL_INTERVAL_MS = 3000;
const DEFAULT_GIT_ERROR = "Unable to load git status.";

export interface SelectedAgentGitStatus {
  rootPath: string | null;
  status: GitStatusResult | null;
  error: string | null;
  loading: boolean;
  refreshing: boolean;
  statusRevision: number;
  changeEventRevision: number;
  changeCount: number;
  refreshStatus: () => Promise<boolean>;
}

export const formatGitStatusError = (err: unknown) => {
  const message = err instanceof Error ? err.message : String(err);
  return message.trim() || DEFAULT_GIT_ERROR;
};

export function useSelectedAgentGitStatus(
  selectedAgentIds: Set<string>,
  agents: AgentConfig[],
): SelectedAgentGitStatus {
  const [rootPath, setRootPath] = useState<string | null>(null);
  const [status, setStatus] = useState<GitStatusResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [statusRevision, setStatusRevision] = useState(0);
  const [changeEventRevision, setChangeEventRevision] = useState(0);

  const selectedAgentId = selectedAgentIds.size === 1 ? Array.from(selectedAgentIds)[0] : null;
  const selectedAgent = selectedAgentId
    ? agents.find((agent) => agent.session_id === selectedAgentId) ?? null
    : null;
  const selectedWorkspaceRevision = useMemo(
    () => [
      selectedAgent?.session_id ?? "",
      selectedAgent?.folder ?? "",
      selectedAgent?.git_worktree ? "worktree" : "main",
      selectedAgent?.git_worktree_source ?? "",
      selectedAgent?.git_worktree_folder ?? "",
    ].join("|"),
    [
      selectedAgent?.session_id,
      selectedAgent?.folder,
      selectedAgent?.git_worktree,
      selectedAgent?.git_worktree_source,
      selectedAgent?.git_worktree_folder,
    ],
  );

  const loadStatusForRoot = useCallback(async (cwd: string) => {
    setRefreshing(true);
    try {
      const result = await invoke<GitStatusResult>("git_status", { cwd });
      setStatus(result);
      setError(null);
      setStatusRevision((current) => current + 1);
      return true;
    } catch (err) {
      setStatus(null);
      setError(formatGitStatusError(err));
      return false;
    } finally {
      setRefreshing(false);
    }
  }, []);

  const refreshStatus = useCallback(async () => {
    if (!rootPath) return false;
    return loadStatusForRoot(rootPath);
  }, [loadStatusForRoot, rootPath]);

  useEffect(() => {
    let disposed = false;
    let resolvedRoot: string | null = null;
    let pollId: number | null = null;
    let watching = false;
    let hasLoadedInitialStatus = false;

    setRootPath(null);
    setStatus(null);
    setError(null);
    setStatusRevision(0);
    setChangeEventRevision(0);
    setRefreshing(false);

    if (!selectedAgentId) {
      setLoading(false);
      return;
    }

    setLoading(true);

    const refreshResolvedStatus = async () => {
      if (!resolvedRoot) return false;
      if (hasLoadedInitialStatus && !disposed) {
        setRefreshing(true);
      }
      try {
        const result = await invoke<GitStatusResult>("git_status", { cwd: resolvedRoot });
        if (!disposed) {
          setStatus(result);
          setError(null);
          setStatusRevision((current) => current + 1);
          hasLoadedInitialStatus = true;
        }
        return true;
      } catch (err) {
        if (!disposed) {
          setStatus(null);
          setError(formatGitStatusError(err));
        }
        return false;
      } finally {
        if (!disposed) {
          setRefreshing(false);
        }
      }
    };

    const unlistenPromise = listen<string>("git-changed", (event) => {
      if (event.payload === resolvedRoot) {
        setChangeEventRevision((current) => current + 1);
        void refreshResolvedStatus();
      }
    });

    void (async () => {
      try {
        const path = await invoke<string>("get_explorer_root", { sessionId: selectedAgentId });
        if (disposed) return;
        if (!path.trim()) {
          setError("Agent workspace is not configured.");
          setLoading(false);
          return;
        }

        resolvedRoot = path;
        setRootPath(path);
        const statusLoaded = await refreshResolvedStatus();
        if (disposed) return;
        setLoading(false);
        if (!statusLoaded) return;

        pollId = window.setInterval(() => {
          void refreshResolvedStatus();
        }, GIT_STATUS_POLL_INTERVAL_MS);
        watching = true;
        invoke("git_watch", { cwd: resolvedRoot }).catch(() => {});
      } catch (err) {
        if (!disposed) {
          setError(formatGitStatusError(err));
          setLoading(false);
        }
      }
    })();

    return () => {
      disposed = true;
      if (pollId !== null) {
        window.clearInterval(pollId);
      }
      if (watching && resolvedRoot) {
        invoke("git_unwatch", { cwd: resolvedRoot }).catch(() => {});
      }
      unlistenPromise.then((fn) => fn());
    };
  }, [selectedAgentId, selectedWorkspaceRevision]);

  return {
    rootPath,
    status,
    error,
    loading,
    refreshing,
    statusRevision,
    changeEventRevision,
    changeCount: status?.files.length ?? 0,
    refreshStatus,
  };
}
