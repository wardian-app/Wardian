import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { GitPanel } from "./GitPanel";
import { ConfirmProvider } from "../../components/ConfirmDialog";
import type { AgentConfig, AgentTelemetry } from "../../types";

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

function renderGitPanel(options?: {
  agentOverride?: Partial<AgentConfig>;
  telemetryOverride?: Record<string, AgentTelemetry>;
}) {
  const renderedAgent = { ...agent, ...options?.agentOverride };
  const renderedTelemetry = options?.telemetryOverride ?? telemetry;

  return render(
    <ConfirmProvider>
      <GitPanel
        selectedAgentIds={new Set(["agent-1"])}
        agents={[renderedAgent]}
        onAgentsUpdated={vi.fn()}
        telemetry={renderedTelemetry}
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
    expect(screen.getByText("Initial commit")).toBeInTheDocument();
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

    fireEvent.change(await screen.findByPlaceholderText("Message (Ctrl+Enter to commit)"), {
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

    fireEvent.change(await screen.findByPlaceholderText("Message (Ctrl+Enter to commit)"), {
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

    expect(await screen.findByTitle("Publish Branch")).toBeInTheDocument();
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

    fireEvent.click(await screen.findByTitle("Push"));

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

    fireEvent.click(await screen.findByTitle("Pull"));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_pull", { cwd: "C:/repo" });
    });
  });

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
    expect(screen.getByText("History unavailable")).toBeInTheDocument();
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
        <GitPanel
          selectedAgentIds={new Set(["agent-1"])}
          agents={[agent]}
          onAgentsUpdated={vi.fn()}
          telemetry={telemetry}
        />
      </ConfirmProvider>,
    );

    await waitFor(() => {
      expect(screen.getAllByText("wardian/repo-agent").length).toBeGreaterThan(0);
    });

    rerender(
      <ConfirmProvider>
        <GitPanel
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
      expect(mockInvoke).toHaveBeenCalledWith("clear_agent_session", {
        sessionId: "agent-1",
        reason: "worktree_switch",
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
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("clear_agent_session", {
        sessionId: "agent-1",
        reason: "worktree_switch",
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
        <GitPanel
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
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("clear_agent_session", {
        sessionId: "agent-1",
        reason: "worktree_switch",
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
      expect(mockInvoke).toHaveBeenCalledWith("clear_agent_session", {
        sessionId: "agent-1",
        reason: "worktree_switch",
      });
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
