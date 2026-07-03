import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { GitPanel } from "./GitPanel";
import { ConfirmProvider } from "../../components/ConfirmDialog";
import type { AgentConfig, AgentTelemetry, GitStatusResult } from "../../types";
import { useSelectedAgentGitStatus, type SelectedAgentGitStatus } from "./useSelectedAgentGitStatus";
import { useSettingsStore } from "../../store/useSettingsStore";

const mockInvoke = vi.mocked(invoke);

const agent: AgentConfig = {
  session_id: "agent-1",
  session_name: "Repo Agent",
  agent_class: "Coder",
  folder: "C:/repo",
  is_off: false,
};

const telemetry: Record<string, AgentTelemetry> = {
  "agent-1": {
    session_id: "agent-1",
    cpu_usage: 0,
    memory_mb: 0,
    uptime_seconds: 0,
    query_count: 0,
    init_timestamp: null,
    current_status: "Idle",
    log_path: null,
  },
};

function createSourceControlStatus(overrides?: Partial<SelectedAgentGitStatus>): SelectedAgentGitStatus {
  const status: GitStatusResult = overrides?.status ?? {
    branch: "main",
    upstream: "origin/main",
    has_upstream: true,
    ahead: 0,
    behind: 0,
    files: [],
  };
  return {
    rootPath: "C:/repo",
    status,
    error: null,
    loading: false,
    refreshing: false,
    statusRevision: 1,
    changeEventRevision: 0,
    changeCount: status.files.length,
    refreshStatus: vi.fn(async () => true),
    ...overrides,
  };
}

function ObservedGitPanelHarness({
  selectedAgentIds,
  agents,
  onAgentsUpdated,
  telemetry,
}: {
  selectedAgentIds: Set<string>;
  agents: AgentConfig[];
  onAgentsUpdated: () => void;
  telemetry: Record<string, AgentTelemetry>;
}) {
  const observedStatus = useSelectedAgentGitStatus(selectedAgentIds, agents);
  return (
    <GitPanel
      selectedAgentIds={selectedAgentIds}
      agents={agents}
      onAgentsUpdated={onAgentsUpdated}
      telemetry={telemetry}
      sourceControlStatus={observedStatus}
    />
  );
}

function GitPanelHarness({
  selectedAgentIds,
  agents,
  onAgentsUpdated,
  telemetry,
  sourceControlStatus,
}: {
  selectedAgentIds: Set<string>;
  agents: AgentConfig[];
  onAgentsUpdated: () => void;
  telemetry: Record<string, AgentTelemetry>;
  sourceControlStatus?: SelectedAgentGitStatus;
}) {
  if (!sourceControlStatus) {
    return (
      <ObservedGitPanelHarness
        selectedAgentIds={selectedAgentIds}
        agents={agents}
        onAgentsUpdated={onAgentsUpdated}
        telemetry={telemetry}
      />
    );
  }
  return (
    <GitPanel
      selectedAgentIds={selectedAgentIds}
      agents={agents}
      onAgentsUpdated={onAgentsUpdated}
      telemetry={telemetry}
      sourceControlStatus={sourceControlStatus}
    />
  );
}

function renderGitPanel(options?: {
  agentOverride?: Partial<AgentConfig>;
  telemetryOverride?: Record<string, AgentTelemetry>;
  sourceControlStatus?: SelectedAgentGitStatus;
  onAgentsUpdated?: () => void;
}) {
  const renderedAgent = { ...agent, ...options?.agentOverride };
  const renderedTelemetry = options?.telemetryOverride ?? telemetry;

  return render(
    <ConfirmProvider>
      <GitPanelHarness
        selectedAgentIds={new Set(["agent-1"])}
        agents={[renderedAgent]}
        onAgentsUpdated={options?.onAgentsUpdated ?? vi.fn()}
        telemetry={renderedTelemetry}
        sourceControlStatus={options?.sourceControlStatus}
      />
    </ConfirmProvider>,
  );
}

function mockLoadedRepository(statusBranch = "main") {
  mockInvoke.mockImplementation(async (command) => {
    if (command === "get_explorer_root") return "C:/repo";
    if (command === "list_agent_worktrees") return [];
    if (command === "git_status") {
      return {
        branch: statusBranch,
        ahead: 0,
        behind: 0,
        files: [],
      };
    }
    if (command === "git_log") return [];
    return null;
  });
}

describe("GitPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    useSettingsStore.setState({
      externalEditor: "system",
      externalEditorCustomExecutable: "",
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows an error instead of loading forever when the workspace cannot be resolved", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") throw new Error("Agent not found");
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("Unable to Load Source Control")).toBeInTheDocument();
    expect(screen.getByText("Agent not found")).toBeInTheDocument();
    expect(screen.queryByText("Loading git status...")).not.toBeInTheDocument();
  });

  it("shows a not-a-repository state when git status reports a non-git workspace", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/not-a-repo";
      if (command === "git_status") throw new Error("fatal: not a git repository (or any of the parent directories): .git");
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("Not a Git Repository")).toBeInTheDocument();
    expect(screen.getByText("The agent's workspace is not initialized as a git repository.")).toBeInTheDocument();
    expect(screen.queryByText("Loading git status...")).not.toBeInTheDocument();
  });

  it("does not poll git status after the initial status load fails", async () => {
    const setIntervalSpy = vi.spyOn(window, "setInterval");
    let statusCalls = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/not-a-repo";
      if (command === "git_status") {
        statusCalls += 1;
        throw new Error("fatal: not a git repository (or any of the parent directories): .git");
      }
      if (command === "git_log") return [];
      return null;
    });

    try {
      renderGitPanel();

      expect(await screen.findByText("Not a Git Repository")).toBeInTheDocument();
      expect(statusCalls).toBe(1);
      expect(setIntervalSpy).not.toHaveBeenCalledWith(expect.any(Function), 3000);
      expect(mockInvoke).not.toHaveBeenCalledWith("git_watch", { cwd: "C:/not-a-repo" });
    } finally {
      setIntervalSpy.mockRestore();
    }
  });

  it("shows a fallback error instead of loading forever when git status returns an empty error", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") throw "";
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("Unable to Load Source Control")).toBeInTheDocument();
    expect(screen.getByText("Unable to load git status.")).toBeInTheDocument();
    expect(screen.queryByText("Loading git status...")).not.toBeInTheDocument();
  });

  it("renders changed files and commit history for a loaded repository", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          ahead: 1,
          behind: 0,
          files: [
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "README.md", status: "A", is_staged: true },
          ],
        };
      }
      if (command === "git_log") {
        return [
          {
            hash: "1234567890abcdef",
            message: "Initial commit",
            author: "Tester",
            date: "2026-04-30 00:00:00 -0400",
          },
        ];
      }
      return null;
    });

    renderGitPanel();

    expect(await screen.findByRole("heading", { name: "Source Control", level: 2 })).toHaveClass("text-sm");
    expect(await screen.findByText("main")).toBeInTheDocument();
    expect(screen.getByText("Staged changes")).toBeInTheDocument();
    expect(screen.getByText("Changes")).toBeInTheDocument();
    expect(screen.getByText("changed.ts")).toBeInTheDocument();
    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(await screen.findByText("Initial commit")).toBeInTheDocument();
  });

  it("keeps the source control header compact and moves secondary actions into overflow", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "feature/compact-header",
          upstream: "origin/feature/compact-header",
          has_upstream: true,
          ahead: 2,
          behind: 1,
          files: [{ path: "src/changed.ts", status: "M", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    expect(await screen.findByRole("heading", { name: "Source Control", level: 2 })).toBeInTheDocument();
    expect(screen.getByTitle("Refresh Source Control")).toBeInTheDocument();
    expect(screen.getByTitle("More Source Control Actions")).toBeInTheDocument();

    expect(screen.queryByTitle("Use source control tree view")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Use source control list view")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Checkout to...")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Fetch")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Pull")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Push")).not.toBeInTheDocument();
    expect(screen.queryByText("↑2")).not.toBeInTheDocument();
    expect(screen.queryByText("↓1")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTitle("More Source Control Actions"));

    expect(await screen.findByRole("button", { name: "Checkout to..." })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fetch" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pull" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Push" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Use Tree View" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Use List View" })).toBeInTheDocument();
  });

  it("uses a supplied source-control observer without resolving or watching git itself", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/shared.ts", status: "M", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    expect(await screen.findByText("shared.ts")).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("get_explorer_root", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("git_status", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("git_watch", expect.anything());
  });

  it("shows source-control refresh progress without hiding loaded files", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshing: true,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/shared.ts", status: "M", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    expect(await screen.findByText("shared.ts")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Refreshing source control...");
  });

  it("refreshes status and history from the source-control header", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ refreshStatus }),
    });

    expect(await screen.findByText("Working tree clean")).toBeInTheDocument();
    mockInvoke.mockClear();

    fireEvent.click(screen.getByRole("button", { name: "Refresh Source Control" }));

    await waitFor(() => {
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
  });

  it("switches source-control resources between tree and list mode per root", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });
    const status = {
      branch: "main",
      upstream: "origin/main",
      has_upstream: true,
      ahead: 0,
      behind: 0,
      files: [{ path: "src/features/git/GitPanel.tsx", status: "M", is_staged: false }],
    };

    const { unmount } = renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ status, changeCount: 1 }),
    });

    expect(await screen.findByRole("button", { name: "src" })).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(screen.getByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Use List View" }));

    expect(screen.queryByRole("button", { name: "src" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View diff for src/features/git/GitPanel.tsx" })).toBeInTheDocument();

    unmount();

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ status, changeCount: 1 }),
    });

    expect(screen.queryByRole("button", { name: "src" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View diff for src/features/git/GitPanel.tsx" })).toBeInTheDocument();
  });

  it("shows the non-git workspace path and reveal action", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "reveal_in_explorer") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        rootPath: "C:/not-a-repo",
        status: null,
        error: "fatal: not a git repository (or any of the parent directories): .git",
        changeCount: 0,
      }),
    });

    expect(await screen.findByText("Not a Git Repository")).toBeInTheDocument();
    expect(screen.getByText("C:/not-a-repo")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Reveal Workspace" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("reveal_in_explorer", { path: "C:/not-a-repo" });
    });
  });

  it("initializes a non-git workspace and refreshes source control", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_init") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        rootPath: "C:/not-a-repo",
        status: null,
        error: "fatal: not a git repository (or any of the parent directories): .git",
        changeCount: 0,
        refreshStatus,
      }),
    });

    expect(await screen.findByText("Not a Git Repository")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Initialize Repository" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_init", { cwd: "C:/not-a-repo" });
    });
    expect(refreshStatus).toHaveBeenCalledTimes(1);
  });

  it("clones a repository into a non-git workspace and refreshes source control", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_clone_repository") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        rootPath: "C:/not-a-repo",
        status: null,
        error: "fatal: not a git repository (or any of the parent directories): .git",
        changeCount: 0,
        refreshStatus,
      }),
    });

    expect(await screen.findByText("Not a Git Repository")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Clone Repository..." }));
    fireEvent.change(screen.getByLabelText("Repository URL"), {
      target: { value: "https://example.com/team/project.git" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_clone_repository", {
        cwd: "C:/not-a-repo",
        repository: "https://example.com/team/project.git",
      });
    });
    expect(refreshStatus).toHaveBeenCalledTimes(1);
  });

  it("separates unresolved merge changes from ordinary working tree changes", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "src/conflicted.ts", status: "UU", is_staged: false },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "src/staged.ts", status: "A", is_staged: true },
            { path: "notes.txt", status: "?", is_staged: false },
          ],
        },
        changeCount: 4,
      }),
    });

    expect(await screen.findByText("Merge Changes")).toBeInTheDocument();
    const mergeSection = screen.getByText("Merge Changes").closest("section");
    expect(mergeSection).not.toBeNull();
    expect(mergeSection).toHaveTextContent("conflicted.ts");
    expect(mergeSection).toHaveTextContent("UU");

    const changesSection = screen.getByText("Changes").closest("section");
    expect(changesSection).not.toBeNull();
    expect(changesSection).toHaveTextContent("changed.ts");
    expect(changesSection).not.toHaveTextContent("conflicted.ts");
    expect(screen.getByText("Staged changes")).toBeInTheDocument();
    expect(screen.getByText("Untracked")).toBeInTheDocument();
  });

  it("opens merge group context actions from the group header", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/conflicted.ts", status: "UU", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.contextMenu(await screen.findByText("Merge Changes"));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Stage All Merge Changes" })).toHaveLength(2);
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Stage All Merge Changes" })[1]);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["src/conflicted.ts"],
      });
    });
  });

  it("opens scoped resource group context actions for staged, tracked, and untracked files", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.txt", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.contextMenu(await screen.findByText("Staged changes"));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Unstage All Changes" })).toHaveLength(2);
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Unstage All Changes" })[1]);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_unstage", {
        cwd: "C:/repo",
        paths: ["README.md"],
      });
    });

    fireEvent.contextMenu(screen.getByText("Changes"));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Stage All Tracked Changes" })).toHaveLength(2);
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Stage All Tracked Changes" })[1]);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["src/changed.ts"],
      });
    });

    fireEvent.contextMenu(screen.getByText("Untracked"));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Stage All Untracked Changes" })).toHaveLength(2);
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Stage All Untracked Changes" })[1]);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["notes.txt"],
      });
    });
  });

  it("discards all untracked resources from the untracked group context menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_discard_changes") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "scratch.txt", status: "?", is_staged: false },
            { path: "logs/debug.txt", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.contextMenu(await screen.findByText("Untracked"));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Discard All Untracked Changes" })).toHaveLength(2);
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Discard All Untracked Changes" })[1]);
    fireEvent.click(await screen.findByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_discard_changes", {
        cwd: "C:/repo",
        paths: ["scratch.txt", "logs/debug.txt"],
      });
    });
    expect(refreshStatus).toHaveBeenCalledTimes(1);
  });

  it("discards all tracked resources from the changes group context menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_discard_changes") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "src/deleted.ts", status: "D", is_staged: false },
            { path: "scratch.txt", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.contextMenu(await screen.findByText("Changes"));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: "Discard All Tracked Changes" })).toHaveLength(2);
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Discard All Tracked Changes" })[1]);
    fireEvent.click(await screen.findByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_discard_changes", {
        cwd: "C:/repo",
        paths: ["src/changed.ts", "src/deleted.ts"],
      });
    });
    expect(refreshStatus).toHaveBeenCalledTimes(1);
  });

  it("exposes inline discard actions on tracked and untracked group headers", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_discard_changes") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "scratch.txt", status: "?", is_staged: false },
          ],
        },
        changeCount: 2,
      }),
    });

    fireEvent.click(await screen.findByTitle("Discard All Tracked Changes"));
    fireEvent.click(await screen.findByRole("button", { name: "Confirm" }));
    fireEvent.click(await screen.findByTitle("Discard All Untracked Changes"));
    fireEvent.click(await screen.findByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_discard_changes", {
        cwd: "C:/repo",
        paths: ["src/changed.ts"],
      });
      expect(mockInvoke).toHaveBeenCalledWith("git_discard_changes", {
        cwd: "C:/repo",
        paths: ["scratch.txt"],
      });
    });
    expect(refreshStatus).toHaveBeenCalledTimes(2);
  });

  it("opens a scoped diff for tracked changes from the resource group context menu", async () => {
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_diff_file") {
        const path = (args as { path: string }).path;
        return `diff --git a/${path} b/${path}\n+${path}`;
      }
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "src/other.ts", status: "M", is_staged: false },
            { path: "notes.txt", status: "?", is_staged: false },
          ],
        },
        changeCount: 4,
      }),
    });

    fireEvent.contextMenu(await screen.findByText("Changes"));
    fireEvent.click(await screen.findByRole("button", { name: "Open Changes" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_diff_file", {
        cwd: "C:/repo",
        path: "src/changed.ts",
        staged: false,
      });
      expect(mockInvoke).toHaveBeenCalledWith("git_diff_file", {
        cwd: "C:/repo",
        path: "src/other.ts",
        staged: false,
      });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("git_diff_file", {
      cwd: "C:/repo",
      path: "README.md",
      staged: true,
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("git_diff_file", {
      cwd: "C:/repo",
      path: "notes.txt",
      staged: false,
    });
    expect(await screen.findByText("+src/changed.ts")).toBeInTheDocument();
    expect(screen.getByText("+src/other.ts")).toBeInTheDocument();
  });

  it("opens and reveals file resources from the file context menu", async () => {
    useSettingsStore.setState({
      externalEditor: "vscode",
      externalEditorCustomExecutable: "",
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "open_in_external_editor") return null;
      if (command === "reveal_in_explorer") return null;
      if (command === "git_show_file_revision") return "committed version\n";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/app.tsx", status: "M", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    const fileRow = await screen.findByRole("button", { name: "View diff for src/app.tsx" });
    fireEvent.contextMenu(fileRow);
    fireEvent.click(await screen.findByRole("button", { name: "Open File" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("open_in_external_editor", {
        path: "C:/repo/src/app.tsx",
        editor: {
          external_editor: "vscode",
          external_editor_custom_executable: null,
        },
      });
    });

    fireEvent.contextMenu(fileRow);
    fireEvent.click(await screen.findByRole("button", { name: "Reveal in Explorer View" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("reveal_in_explorer", {
        path: "C:/repo/src/app.tsx",
      });
    });

    fireEvent.contextMenu(fileRow);
    fireEvent.click(await screen.findByRole("button", { name: "Open File (HEAD)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_show_file_revision", {
        cwd: "C:/repo",
        path: "src/app.tsx",
        revision: "HEAD",
      });
    });
    expect(await screen.findByText("HEAD: src/app.tsx")).toBeInTheDocument();
    expect(screen.getByText("committed version")).toBeInTheDocument();
  });

  it("compares a staged resource with the workspace from the file context menu", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_diff_file_against_workspace") {
        return "diff --git a/src/app.tsx b/src/app.tsx\n-staged version\n+workspace version\n";
      }
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "src/app.tsx", status: "M", is_staged: true },
            { path: "src/app.tsx", status: "M", is_staged: false },
          ],
        },
        changeCount: 2,
      }),
    });

    const fileRows = await screen.findAllByRole("button", { name: "View diff for src/app.tsx" });
    fireEvent.contextMenu(fileRows[0]);
    fireEvent.click(await screen.findByRole("button", { name: "Compare with Workspace" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_diff_file_against_workspace", {
        cwd: "C:/repo",
        path: "src/app.tsx",
      });
    });
    expect(await screen.findByText("Workspace: src/app.tsx")).toBeInTheDocument();
    expect(screen.getByText("-staged version")).toBeInTheDocument();
    expect(screen.getByText("+workspace version")).toBeInTheDocument();
  });

  it("discards an untracked resource from the file context menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_discard_changes") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "scratch.txt", status: "?", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.contextMenu(await screen.findByRole("button", { name: "View diff for scratch.txt" }));
    fireEvent.click(await screen.findByRole("button", { name: "Discard Changes" }));
    fireEvent.click(await screen.findByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_discard_changes", {
        cwd: "C:/repo",
        paths: ["scratch.txt"],
      });
    });
    expect(refreshStatus).toHaveBeenCalledTimes(1);
  });

  it("opens folder context actions that only affect files under that tree folder", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "src/app.tsx", status: "M", is_staged: false },
            { path: "src/components/Button.tsx", status: "M", is_staged: false },
            { path: "README.md", status: "M", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.contextMenu(await screen.findByRole("button", { name: "src" }));
    fireEvent.click(await screen.findByRole("button", { name: "Stage Changes" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["src/app.tsx", "src/components/Button.tsx"],
      });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("git_stage", {
      cwd: "C:/repo",
      paths: expect.arrayContaining(["README.md"]),
    });
  });

  it("adds source-control tree folders to gitignore from the folder context menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_ignore") return "";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "logs/debug/output.log", status: "?", is_staged: false },
            { path: "logs/raw/input.log", status: "?", is_staged: false },
            { path: "src/app.tsx", status: "M", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.contextMenu(await screen.findByRole("button", { name: "logs" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add to .gitignore" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_ignore", {
        cwd: "C:/repo",
        paths: ["logs/"],
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
  });

  it("adds source-control file resources to gitignore from the file context menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_ignore") return "";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "logs/debug/output.log", status: "?", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.contextMenu(await screen.findByRole("button", { name: "View diff for logs/debug/output.log" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add to .gitignore" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_ignore", {
        cwd: "C:/repo",
        paths: ["logs/debug/output.log"],
      });
      expect(refreshStatus).toHaveBeenCalled();
    });
  });

  it("polls git status so working tree edits appear without a git-changed event", async () => {
    vi.useFakeTimers();
    let statusCalls = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        statusCalls += 1;
        return statusCalls === 1
          ? {
              branch: "main",
              upstream: "origin/main",
              has_upstream: true,
              ahead: 0,
              behind: 0,
              files: [],
            }
          : {
              branch: "main",
              upstream: "origin/main",
              has_upstream: true,
              ahead: 0,
              behind: 0,
              files: [{ path: "src/changed.ts", status: "M", is_staged: false }],
            };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel();

    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByText("Working tree clean")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });

    expect(screen.getByText("changed.ts")).toBeInTheDocument();
  });

  it("commits unstaged changes by staging them first", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/changed.ts", status: "M", is_staged: false }],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });
    renderGitPanel();

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "save changes" },
    });
    fireEvent.click(screen.getByRole("button", { name: /commit/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["src/changed.ts"],
      });
    });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit", {
        cwd: "C:/repo",
        message: "save changes",
      });
    });
  });

  it("shows a non-blocking SCM input warning for long commit summaries", async () => {
    const longSummary = "A".repeat(73);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/changed.ts", status: "M", is_staged: false }],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });
    renderGitPanel();

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: longSummary },
    });

    expect(screen.getByRole("status", { name: "Commit message validation" })).toHaveTextContent(
      "Summary line is 73 characters; VS Code marks commit subjects past 50 and body lines past 72 for review.",
    );
    expect(screen.getByText("73/50")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /commit/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit", {
        cwd: "C:/repo",
        message: longSummary,
      });
    });
  });

  it("offers an SCM input action menu for committing all pending files", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
          ],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });
    renderGitPanel();

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "ship all changes" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));

    expect(screen.getByRole("button", { name: "Commit Staged" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Commit All" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["src/changed.ts"],
      });
    });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit", {
        cwd: "C:/repo",
        message: "ship all changes",
      });
    });
  });

  it("amends the last commit from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "previous subject",
            author: "Wardian",
            date: "2026-06-26 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_amend") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "README.md", status: "M", is_staged: true }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "amended subject" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit (Amend)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_amend", {
        cwd: "C:/repo",
        message: "amended subject",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("amends the last commit with all pending changes from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "previous subject",
            author: "Wardian",
            date: "2026-06-27 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_all_amend") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "amend everything" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit All (Amend)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_all_amend", {
        cwd: "C:/repo",
        message: "amend everything",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("amends the last commit with staged changes from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "previous subject",
            author: "Wardian",
            date: "2026-06-27 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_staged_amend") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "amend staged work" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Staged (Amend)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_staged_amend", {
        cwd: "C:/repo",
        message: "amend staged work",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("amends the last commit with no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "previous subject",
            author: "Wardian",
            date: "2026-06-27 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_amend_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "README.md", status: "M", is_staged: true }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "amend bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit (Amend, No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_amend_no_verify", {
        cwd: "C:/repo",
        message: "amend bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("amends the last commit with staged changes and no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "previous subject",
            author: "Wardian",
            date: "2026-06-27 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_staged_amend_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "amend staged bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Staged (Amend, No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_staged_amend_no_verify", {
        cwd: "C:/repo",
        message: "amend staged bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("amends the last commit with all changes and no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "previous subject",
            author: "Wardian",
            date: "2026-06-27 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_all_amend_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "amend all bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit All (Amend, No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_all_amend_no_verify", {
        cwd: "C:/repo",
        message: "amend all bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits with no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "README.md", status: "M", is_staged: true }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "bypass local hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit (No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_no_verify", {
        cwd: "C:/repo",
        message: "bypass local hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits with signoff from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_stage") return null;
      if (command === "git_commit_signed") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "README.md", status: "M", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "signed off work" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit (Signed Off)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["README.md"],
      });
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_signed", {
        cwd: "C:/repo",
        message: "signed off work",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits staged changes with signoff from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_staged_signed") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "signed off staged work" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Staged (Signed Off)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_staged_signed", {
        cwd: "C:/repo",
        message: "signed off staged work",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("git_stage", expect.anything());
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits all pending changes with signoff from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_all_signed") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "signed off all work" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit All (Signed Off)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_all_signed", {
        cwd: "C:/repo",
        message: "signed off all work",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits with signoff and no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_stage") return null;
      if (command === "git_commit_signed_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "README.md", status: "M", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "signed off bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit (Signed Off, No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["README.md"],
      });
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_signed_no_verify", {
        cwd: "C:/repo",
        message: "signed off bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits staged changes with signoff and no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_staged_signed_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "signed off staged bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Staged (Signed Off, No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_staged_signed_no_verify", {
        cwd: "C:/repo",
        message: "signed off staged bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("git_stage", expect.anything());
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits all pending changes with signoff and no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_all_signed_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "signed off all bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit All (Signed Off, No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_all_signed_no_verify", {
        cwd: "C:/repo",
        message: "signed off all bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("aborts an in-progress rebase from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_rebase_abort") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "feature",
          upstream: "origin/feature",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          rebase_in_progress: true,
          files: [{ path: "README.md", status: "U", is_staged: false }],
        },
        changeCount: 1,
      }),
    });

    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Abort Rebase" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_rebase_abort", { cwd: "C:/repo" });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
  });

  it("commits staged changes with no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_staged_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "bypass hook for staged work" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Staged (No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_staged_no_verify", {
        cwd: "C:/repo",
        message: "bypass hook for staged work",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("commits all pending changes with no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_all_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [
            { path: "README.md", status: "M", is_staged: true },
            { path: "src/changed.ts", status: "M", is_staged: false },
            { path: "notes.md", status: "?", is_staged: false },
          ],
        },
        changeCount: 3,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "bypass hook for everything" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit All (No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_all_no_verify", {
        cwd: "C:/repo",
        message: "bypass hook for everything",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("creates an empty commit from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_empty") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [],
        },
        changeCount: 0,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "empty marker" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Empty" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_empty", {
        cwd: "C:/repo",
        message: "empty marker",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("creates an empty commit with no verification from the SCM input action menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit_empty_no_verify") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [],
        },
        changeCount: 0,
      }),
    });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "empty bypass hook" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Commit Empty (No Verify)" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_empty_no_verify", {
        cwd: "C:/repo",
        message: "empty bypass hook",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(screen.getByPlaceholderText("Message on main (Ctrl+Enter to commit)")).toHaveValue("");
  });

  it("remembers the last SCM input commit action as the next primary action", async () => {
    const status: GitStatusResult = {
      branch: "main",
      upstream: "origin/main",
      has_upstream: true,
      ahead: 0,
      behind: 0,
      files: [
        { path: "README.md", status: "M", is_staged: true },
        { path: "src/changed.ts", status: "M", is_staged: false },
      ],
    };
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    const firstRender = renderGitPanel({ sourceControlStatus: createSourceControlStatus({ status, changeCount: 2 }) });

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "ship all changes" },
    });
    fireEvent.click(screen.getByRole("button", { name: "More Actions" }));
    fireEvent.click(screen.getByRole("button", { name: "Commit All" }));

    await waitFor(() => {
      expect(window.localStorage.getItem("wardian:source-control:commit:last-action")).toBe("all");
    });

    firstRender.unmount();
    vi.clearAllMocks();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    renderGitPanel({ sourceControlStatus: createSourceControlStatus({ status, changeCount: 2 }) });
    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "ship all changes again" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Commit All" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stage", {
        cwd: "C:/repo",
        paths: ["src/changed.ts"],
      });
    });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit", {
        cwd: "C:/repo",
        message: "ship all changes again",
      });
    });
  });

  it("undoes the last commit from the commit action menu and restores its message", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") {
        return [
          {
            hash: "abc123",
            parent_hashes: ["def456"],
            refs: ["HEAD", "main"],
            message: "undo me",
            author: "Wardian",
            date: "2026-06-26 00:00:00 +0000",
          },
        ];
      }
      if (command === "list_agent_worktrees") return [];
      if (command === "git_undo_last_commit") return "undo me";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ refreshStatus }),
    });

    const commitInput = await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)");
    const actionRow = commitInput.closest("div");
    if (!actionRow) throw new Error("Expected commit action row");

    fireEvent.click(within(actionRow).getByRole("button", { name: "More Actions" }));
    fireEvent.click(await screen.findByRole("button", { name: "Undo Last Commit" }));
    expect(await screen.findByText("Undo last commit and keep its changes in the working tree?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_undo_last_commit", { cwd: "C:/repo" });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
    expect(commitInput).toHaveValue("undo me");
  });

  it("surfaces commit failures in the panel", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "README.md", status: "M", is_staged: true }],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_commit") throw new Error("Author identity unknown");
      return null;
    });
    renderGitPanel();

    fireEvent.change(await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)"), {
      target: { value: "save changes" },
    });
    fireEvent.click(screen.getByRole("button", { name: /commit/i }));

    expect(await screen.findByText("Author identity unknown")).toBeInTheDocument();
  });

  it("labels unpublished branch pushes as publish branch", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "feature/unpublished",
          upstream: null,
          has_upstream: false,
          ahead: 0,
          behind: 0,
          files: [],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });
    renderGitPanel();

    expect(await screen.findByRole("button", { name: "Publish Branch" })).toBeInTheDocument();
  });

  it("shows publish branch as the primary action for clean branches without upstream", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_push") return "published";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "feature/unpublished",
          upstream: null,
          has_upstream: false,
          ahead: 0,
          behind: 0,
          files: [],
        },
      }),
    });

    const actionRow = (await screen.findByPlaceholderText("Message on feature/unpublished (Ctrl+Enter to commit)"))
      .closest("div");
    if (!actionRow) throw new Error("Expected commit action row");

    fireEvent.click(within(actionRow).getByRole("button", { name: "Publish Branch" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_push", { cwd: "C:/repo" });
    });
  });

  it("shows sync changes as the primary action for clean diverged branches", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_pull") return "pulled";
      if (command === "git_push") return "pushed";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 2,
          behind: 1,
          files: [],
        },
      }),
    });

    const actionRow = (await screen.findByPlaceholderText("Message on main (Ctrl+Enter to commit)")).closest("div");
    if (!actionRow) throw new Error("Expected commit action row");

    fireEvent.click(within(actionRow).getByRole("button", { name: "Sync Changes ↓1 ↑2" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_pull", { cwd: "C:/repo" });
      expect(mockInvoke).toHaveBeenCalledWith("git_push", { cwd: "C:/repo" });
    });
  });

  it("surfaces push failures in the panel", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_push") throw new Error("remote rejected");
      return null;
    });
    renderGitPanel();

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Push" }));

    expect(await screen.findByText("remote rejected")).toBeInTheDocument();
  });

  it("runs pull from the selected source control root", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 1,
          files: [],
        };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_pull") return "Already up to date.";
      return null;
    });
    renderGitPanel();

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Pull" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_pull", { cwd: "C:/repo" });
    });
  });

  it("fetches remote updates from the selected source control root", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_fetch") return "fetched";
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ refreshStatus }),
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Fetch" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_fetch", { cwd: "C:/repo" });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
  });

  it("checks out a local branch from the source control overflow", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_list_branches") {
        return [
          { name: "main", current: true },
          { name: "feature/source-control", current: false },
        ];
      }
      if (command === "git_checkout_branch") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ refreshStatus }),
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Checkout to..." }));
    fireEvent.click(await screen.findByRole("button", { name: "feature/source-control" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_list_branches", { cwd: "C:/repo" });
      expect(mockInvoke).toHaveBeenCalledWith("git_checkout_branch", {
        cwd: "C:/repo",
        branch: "feature/source-control",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
  });

  it("creates a local branch from the source control overflow", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_list_branches") {
        return [{ name: "main", current: true }];
      }
      if (command === "git_create_branch") return null;
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({ refreshStatus }),
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Checkout to..." }));
    fireEvent.click(await screen.findByRole("button", { name: "Create Branch..." }));
    const input = await screen.findByPlaceholderText("branch-name");
    fireEvent.change(input, { target: { value: "feature/new-branch" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_create_branch", {
        cwd: "C:/repo",
        branch: "feature/new-branch",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });
  });

  it("runs stash actions from the source control header menu", async () => {
    const refreshStatus = vi.fn(async () => true);
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      if (command === "git_stash_push") return "";
      if (command === "git_stash_staged") return "";
      if (command === "git_stash_apply_latest") return "";
      if (command === "git_stash_apply") return "";
      if (command === "git_stash_pop_latest") return "";
      if (command === "git_stash_pop") return "";
      if (command === "git_stash_drop") return "";
      if (command === "git_stash_drop_all") return "";
      if (command === "git_list_stashes") {
        return [
          { selector: "stash@{0}", message: "WIP on main: second stash" },
          { selector: "stash@{1}", message: "WIP on main: first stash" },
        ];
      }
      if (command === "git_show_stash") {
        expect(args).toEqual({ cwd: "C:/repo", stash: "stash@{0}" });
        return "diff --git a/src/App.tsx b/src/App.tsx\n+stash preview\n";
      }
      return null;
    });

    renderGitPanel({
      sourceControlStatus: createSourceControlStatus({
        refreshStatus,
        status: {
          branch: "main",
          upstream: "origin/main",
          has_upstream: true,
          ahead: 0,
          behind: 0,
          files: [{ path: "src/App.tsx", status: "M", is_staged: false }],
        },
      }),
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Stash Changes Including Untracked" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_push", {
        cwd: "C:/repo",
        includeUntracked: true,
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Stash Staged" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_staged", { cwd: "C:/repo" });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Apply Latest Stash" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_apply_latest", { cwd: "C:/repo" });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Apply Stash..." }));
    fireEvent.click(await screen.findByRole("button", { name: "stash@{1} WIP on main: first stash" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_apply", {
        cwd: "C:/repo",
        stash: "stash@{1}",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Pop Latest Stash" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_pop_latest", { cwd: "C:/repo" });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Pop Stash..." }));
    fireEvent.click(await screen.findByRole("button", { name: "stash@{0} WIP on main: second stash" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_pop", {
        cwd: "C:/repo",
        stash: "stash@{0}",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "View Stash..." }));
    fireEvent.click(await screen.findByRole("button", { name: "stash@{0} WIP on main: second stash" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_show_stash", {
        cwd: "C:/repo",
        stash: "stash@{0}",
      });
      expect(screen.getByText("Stash stash@{0}")).toBeInTheDocument();
      expect(screen.getByText("+stash preview")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTitle("Close diff"));

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Drop Stash..." }));
    fireEvent.click(await screen.findByRole("button", { name: "stash@{1} WIP on main: first stash" }));
    expect(await screen.findByText("Drop stash stash@{1}? This cannot be undone.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_drop", {
        cwd: "C:/repo",
        stash: "stash@{1}",
      });
      expect(refreshStatus).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith("git_log", { cwd: "C:/repo", count: 50 });
    });

    fireEvent.click(await screen.findByTitle("More Source Control Actions"));
    fireEvent.click(await screen.findByRole("button", { name: "Drop All Stashes..." }));
    expect(await screen.findByText("Drop all stashes for this workspace? This cannot be undone.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_stash_drop_all", { cwd: "C:/repo" });
    });
  }, 10000);

  it("keeps file status visible when commit history cannot be loaded", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        return {
          branch: "main",
          ahead: 0,
          behind: 0,
          files: [{ path: "src/changed.ts", status: "M", is_staged: false }],
        };
      }
      if (command === "git_log") throw new Error("fatal: your current branch does not have any commits yet");
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("changed.ts")).toBeInTheDocument();
    expect(await screen.findByText("History unavailable")).toBeInTheDocument();
  });

  it("re-resolves source control root when the selected agent worktree assignment changes", async () => {
    let rootCalls = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") {
        rootCalls += 1;
        return rootCalls === 1 ? "C:/repo" : "C:/repo-worktree";
      }
      if (command === "git_status") {
        return { branch: "wardian/repo-agent", ahead: 0, behind: 0, files: [] };
      }
      if (command === "git_log") return [];
      if (command === "list_agent_worktrees") return [];
      return null;
    });

    const { rerender } = render(
      <ConfirmProvider>
        <GitPanelHarness
          selectedAgentIds={new Set(["agent-1"])}
          agents={[agent]}
          onAgentsUpdated={vi.fn()}
          telemetry={telemetry}
        />
      </ConfirmProvider>,
    );

    await screen.findByPlaceholderText("Message on wardian/repo-agent (Ctrl+Enter to commit)");

    rerender(
      <ConfirmProvider>
        <GitPanelHarness
          selectedAgentIds={new Set(["agent-1"])}
          agents={[{ ...agent, git_worktree: true, git_worktree_folder: "C:/repo-worktree" }]}
          onAgentsUpdated={vi.fn()}
          telemetry={telemetry}
        />
      </ConfirmProvider>,
    );

    await waitFor(() => {
      expect(rootCalls).toBeGreaterThanOrEqual(2);
      expect(mockInvoke).toHaveBeenCalledWith("git_status", { cwd: "C:/repo-worktree" });
    });
  });

  it("enables a named real agent worktree without resuming a running agent", async () => {
    mockLoadedRepository();
    renderGitPanel();

    fireEvent.click(await screen.findByText("Create Worktree"));
    const input = await screen.findByPlaceholderText("worktree-name");
    expect(input).toHaveValue("Repo Agent");
    fireEvent.change(input, { target: { value: "review fixes" } });
    fireEvent.click(screen.getByTitle("Create and start fresh"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("enable_agent_worktree", {
        sessionId: "agent-1",
        worktreeName: "review fixes",
      });
    });
    await waitFor(() => {
      expect(screen.getByText("Create Worktree")).toBeInTheDocument();
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("resume_agent", { sessionId: "agent-1" });
  });

  it("does not create a worktree when inline naming is cancelled", async () => {
    mockLoadedRepository();
    renderGitPanel();

    fireEvent.click(await screen.findByText("Create Worktree"));
    const input = await screen.findByPlaceholderText("worktree-name");
    fireEvent.change(input, { target: { value: "review fixes" } });
    fireEvent.click(screen.getByTitle("Cancel"));

    await waitFor(() => {
      expect(screen.getByText("Create Worktree")).toBeInTheDocument();
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("enable_agent_worktree", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("clear_agent_session", expect.anything());
  });

  it("creates a named worktree from the inline input with Enter", async () => {
    mockLoadedRepository();
    renderGitPanel();

    fireEvent.click(await screen.findByText("Create Worktree"));
    const input = await screen.findByPlaceholderText("worktree-name");
    fireEvent.change(input, { target: { value: "keyboard branch" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("enable_agent_worktree", {
        sessionId: "agent-1",
        worktreeName: "keyboard branch",
      });
    });
  });

  it("joins an existing shared worktree without resuming a running agent", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "list_agent_worktrees") {
        return [
          {
            id: "C:/repo-worktree",
            name: "repo-worktree",
            source_folder: "C:/repo",
            worktree_folder: "C:/repo-worktree",
            member_agent_ids: ["agent-2"],
            can_delete: false,
          },
        ];
      }
      if (command === "git_status") return { branch: "main", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });
    renderGitPanel();

    fireEvent.click(await screen.findByText("Move to repo-worktree"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("assign_agent_worktree", {
        sessionId: "agent-1",
        worktreeFolder: "C:/repo-worktree",
      });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("resume_agent", { sessionId: "agent-1" });
  });

  it("shows join options when source paths differ only by Windows spelling", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "list_agent_worktrees") {
        return [
          {
            id: "C:/repo-worktree",
            name: "repo-worktree",
            source_folder: "c:/repo/",
            worktree_folder: "C:/repo-worktree",
            member_agent_ids: [],
            can_delete: true,
          },
        ];
      }
      if (command === "git_status") return { branch: "main", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel({ agentOverride: { folder: "C:\\repo" } });

    expect(await screen.findByText("Move to repo-worktree")).toBeInTheDocument();
  });

  it("deletes an unassigned available worktree after confirmation", async () => {
    const onAgentsUpdated = vi.fn();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "list_agent_worktrees") {
        return [
          {
            id: "C:/repo-worktree",
            name: "repo-worktree",
            source_folder: "C:/repo",
            worktree_folder: "C:/repo-worktree",
            member_agent_ids: [],
            can_delete: true,
          },
        ];
      }
      if (command === "git_status") return { branch: "main", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      if (command === "delete_agent_worktree") return null;
      return null;
    });

    render(
      <ConfirmProvider>
        <GitPanelHarness
          selectedAgentIds={new Set(["agent-1"])}
          agents={[agent]}
          onAgentsUpdated={onAgentsUpdated}
          telemetry={telemetry}
        />
      </ConfirmProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Delete repo-worktree worktree" }));
    fireEvent.click(await screen.findByText("Confirm"));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("delete_agent_worktree", {
        worktreeFolder: "C:/repo-worktree",
      }),
    );
    await waitFor(() => {
      expect(screen.queryByText("Move to repo-worktree")).not.toBeInTheDocument();
    });
    expect(onAgentsUpdated).toHaveBeenCalled();
  });

  it("does not show delete controls for worktrees assigned to another agent", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "list_agent_worktrees") {
        return [
          {
            id: "C:/repo-worktree",
            name: "repo-worktree",
            source_folder: "C:/repo",
            worktree_folder: "C:/repo-worktree",
            member_agent_ids: ["agent-2"],
            can_delete: false,
          },
        ];
      }
      if (command === "git_status") return { branch: "main", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("Move to repo-worktree")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete repo-worktree worktree" })).not.toBeInTheDocument();
  });

  it("does not show delete controls for external unassigned worktrees", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "list_agent_worktrees") {
        return [
          {
            id: "C:/external-worktree",
            name: "external-worktree",
            source_folder: "C:/repo",
            worktree_folder: "C:/external-worktree",
            member_agent_ids: [],
            can_delete: false,
          },
        ];
      }
      if (command === "git_status") return { branch: "main", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("Move to external-worktree")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete external-worktree worktree" })).not.toBeInTheDocument();
  });

  it("shows discovered unassigned worktrees as join options", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "list_agent_worktrees") {
        return [
          {
            id: "C:/wardian/agents/agent-1/worktrees/manual-review",
            name: "manual-review",
            source_folder: "C:/repo",
            worktree_folder: "C:/wardian/agents/agent-1/worktrees/manual-review",
            member_agent_ids: [],
            can_delete: true,
          },
        ];
      }
      if (command === "git_status") return { branch: "main", upstream: "origin/main", has_upstream: true, ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel();

    expect(await screen.findByText("Move to manual-review")).toBeInTheDocument();
  });

  it("repairs a stale worktree assignment by moving the runtime fresh", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo-worktree";
      if (command === "list_agent_worktrees") return [
        {
          id: "C:/repo-worktree",
          name: "repo-worktree",
          source_folder: "C:/repo",
          worktree_folder: "C:/repo-worktree",
          member_agent_ids: ["agent-1"],
          can_delete: false,
        },
      ];
      if (command === "git_status") return { branch: "wardian/repo-agent", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });
    renderGitPanel({
      agentOverride: {
        folder: "C:/repo",
        git_worktree: true,
        git_worktree_source: "C:/repo",
        git_worktree_folder: "C:/repo-worktree",
      },
    });

    fireEvent.click(await screen.findByText("Start Fresh Here"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("assign_agent_worktree", {
        sessionId: "agent-1",
        worktreeFolder: "C:/repo-worktree",
      });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("resume_agent", { sessionId: "agent-1" });
  });

  it("shows the active worktree name in the source control worktree chip", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo-worktree";
      if (command === "list_agent_worktrees") return [
        {
          id: "C:/repo-worktree",
          name: "repo-worktree",
          source_folder: "C:/repo",
          worktree_folder: "C:/repo-worktree",
          member_agent_ids: ["agent-1"],
          can_delete: false,
        },
      ];
      if (command === "git_status") return { branch: "wardian/repo-agent", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      return null;
    });

    renderGitPanel({
      agentOverride: {
        folder: "C:/repo-worktree",
        git_worktree: true,
        git_worktree_source: "C:/repo",
        git_worktree_folder: "C:/repo-worktree",
      },
    });

    expect(await screen.findByText("Worktree: repo-worktree")).toBeInTheDocument();
    expect(screen.queryByText("Worktree runtime")).not.toBeInTheDocument();
  });

  it("disables the agent worktree without resuming a running agent", async () => {
    mockLoadedRepository("wardian/repo-agent");
    renderGitPanel();

    fireEvent.click(await screen.findByTitle("Remove worktree assignment"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("disable_agent_worktree", { sessionId: "agent-1" });
    });
    await waitFor(() => {
      expect(screen.getByTitle("Remove worktree assignment")).toBeInTheDocument();
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("resume_agent", { sessionId: "agent-1" });
  });

  it("surfaces worktree disable failures instead of appearing inert", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo-worktree";
      if (command === "list_agent_worktrees") return [];
      if (command === "git_status") return { branch: "wardian/repo-agent", ahead: 0, behind: 0, files: [] };
      if (command === "git_log") return [];
      if (command === "disable_agent_worktree") throw new Error("worktree is locked");
      return null;
    });
    renderGitPanel({
      agentOverride: {
        folder: "C:/repo-worktree",
        git_worktree: true,
        git_worktree_source: "C:/repo",
        git_worktree_folder: "C:/repo-worktree",
      },
    });

    fireEvent.click(await screen.findByTitle("Remove worktree assignment"));

    expect(await screen.findByText("Unable to Load Source Control")).toBeInTheDocument();
    expect(screen.getByText("worktree is locked")).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith("clear_agent_session", {
      sessionId: "agent-1",
      reason: "worktree_switch",
    });
  });
});
