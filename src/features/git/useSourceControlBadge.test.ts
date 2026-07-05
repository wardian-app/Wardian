import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig } from "../../types";
import { useSourceControlBadge } from "./useSourceControlBadge";

const mockInvoke = vi.mocked(invoke);

const agent: AgentConfig = {
  session_id: "agent-1",
  session_name: "Repo Agent",
  agent_class: "Coder",
  folder: "C:/repo",
  is_off: false,
};

describe("useSourceControlBadge", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("counts pending changes for the single selected agent workspace", async () => {
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
            { path: "src/app.tsx", status: "M", is_staged: false },
            { path: "README.md", status: "A", is_staged: true },
            { path: "new.txt", status: "?", is_staged: false },
          ],
        };
      }
      return null;
    });

    const { result } = renderHook(() => useSourceControlBadge(new Set(["agent-1"]), [agent]));

    await waitFor(() => expect(result.current.changeCount).toBe(3));
    expect(mockInvoke).toHaveBeenCalledWith("get_explorer_root", { sessionId: "agent-1" });
    expect(mockInvoke).toHaveBeenCalledWith("git_status", { cwd: "C:/repo" });
  });

  it("returns zero when the source control selection is ambiguous", async () => {
    const { result } = renderHook(() => useSourceControlBadge(new Set(["agent-1", "agent-2"]), [agent]));

    expect(result.current.changeCount).toBe(0);
    expect(mockInvoke).not.toHaveBeenCalledWith("git_status", expect.anything());
  });
});
