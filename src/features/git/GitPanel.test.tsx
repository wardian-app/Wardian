import { render, screen } from "@testing-library/react";
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

function renderGitPanel() {
  render(
    <ConfirmProvider>
      <GitPanel
        selectedAgentIds={new Set(["agent-1"])}
        agents={[agent]}
        onAgentsUpdated={vi.fn()}
        telemetry={telemetry}
      />
    </ConfirmProvider>,
  );
}

describe("GitPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
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

    expect(await screen.findByText("main")).toBeInTheDocument();
    expect(screen.getByText("Staged changes")).toBeInTheDocument();
    expect(screen.getByText("Changes")).toBeInTheDocument();
    expect(screen.getByText("changed.ts")).toBeInTheDocument();
    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(screen.getByText("Initial commit")).toBeInTheDocument();
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
});
