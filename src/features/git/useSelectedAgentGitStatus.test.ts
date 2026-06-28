import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig } from "../../types";
import { useSelectedAgentGitStatus } from "./useSelectedAgentGitStatus";

const mockInvoke = vi.mocked(invoke);

const agent: AgentConfig = {
  session_id: "agent-1",
  session_name: "Repo Agent",
  agent_class: "Coder",
  folder: "C:/repo",
  is_off: false,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("useSelectedAgentGitStatus", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("resolves the selected agent root and counts pending changes", async () => {
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

    const { result } = renderHook(() => useSelectedAgentGitStatus(new Set(["agent-1"]), [agent]));

    await waitFor(() => expect(result.current.changeCount).toBe(3));
    expect(result.current.rootPath).toBe("C:/repo");
    expect(result.current.error).toBeNull();
    expect(result.current.statusRevision).toBe(1);
    expect(mockInvoke).toHaveBeenCalledWith("get_explorer_root", { sessionId: "agent-1" });
    expect(mockInvoke).toHaveBeenCalledWith("git_status", { cwd: "C:/repo" });
  });

  it("polls git status from the shared observer", async () => {
    let statusCalls = 0;
    const setIntervalSpy = vi.spyOn(window, "setInterval");
    try {
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
        return null;
      });

      const { result, unmount } = renderHook(() => useSelectedAgentGitStatus(new Set(["agent-1"]), [agent]));

      await waitFor(() => expect(result.current.statusRevision).toBe(1));
      expect(result.current.changeCount).toBe(0);
      expect(setIntervalSpy).toHaveBeenCalledWith(expect.any(Function), 3000);
      const pollCall = setIntervalSpy.mock.calls.find(([, timeout]) => timeout === 3000);
      const pollCallback = pollCall?.[0];
      expect(typeof pollCallback).toBe("function");

      await act(async () => {
        if (typeof pollCallback === "function") {
          pollCallback();
        }
        await Promise.resolve();
      });

      expect(result.current.changeCount).toBe(1);
      expect(result.current.statusRevision).toBe(2);
      unmount();
    } finally {
      setIntervalSpy.mockRestore();
    }
  });

  it("reports background refresh progress without clearing the previous status", async () => {
    const refresh = deferred<{
      branch: string;
      upstream: string;
      has_upstream: boolean;
      ahead: number;
      behind: number;
      files: { path: string; status: string; is_staged: boolean }[];
    }>();
    let statusCalls = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === "get_explorer_root") return "C:/repo";
      if (command === "git_status") {
        statusCalls += 1;
        if (statusCalls === 1) {
          return {
            branch: "main",
            upstream: "origin/main",
            has_upstream: true,
            ahead: 0,
            behind: 0,
            files: [{ path: "README.md", status: "M", is_staged: false }],
          };
        }
        return refresh.promise;
      }
      return null;
    });

    const { result } = renderHook(() => useSelectedAgentGitStatus(new Set(["agent-1"]), [agent]));

    await waitFor(() => expect(result.current.changeCount).toBe(1));

    let refreshPromise!: Promise<boolean>;
    act(() => {
      refreshPromise = result.current.refreshStatus();
    });

    await waitFor(() => expect(result.current.refreshing).toBe(true));
    expect(result.current.changeCount).toBe(1);

    await act(async () => {
      refresh.resolve({
        branch: "main",
        upstream: "origin/main",
        has_upstream: true,
        ahead: 0,
        behind: 0,
        files: [{ path: "src/changed.ts", status: "M", is_staged: false }],
      });
      await refreshPromise;
    });

    expect(result.current.refreshing).toBe(false);
    expect(result.current.status?.files[0]?.path).toBe("src/changed.ts");
  });

  it("does not resolve git state when the source control selection is ambiguous", () => {
    const { result } = renderHook(() => useSelectedAgentGitStatus(new Set(["agent-1", "agent-2"]), [agent]));

    expect(result.current.changeCount).toBe(0);
    expect(result.current.loading).toBe(false);
    expect(result.current.status).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalledWith("get_explorer_root", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("git_status", expect.anything());
  });
});
