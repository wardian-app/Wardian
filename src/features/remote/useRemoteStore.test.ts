import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_WATCHLIST_PREFS } from "../../layout/watchlist/types";
import { RemoteRequestError, remoteClient } from "./remoteClient";
import { useRemoteStore } from "./useRemoteStore";

vi.mock("./remoteClient", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./remoteClient")>();
  return {
    ...actual,
    remoteClient: {
      ...actual.remoteClient,
      loadSession: vi.fn(),
      listAgents: vi.fn(),
      listWorkflows: vi.fn(),
      loadWatchlists: vi.fn(),
      openStatusStream: vi.fn(),
    },
  };
});

const session = {
  csrf_nonce: "csrf-1",
  expires_at: "2026-05-21T08:05:00.000Z",
  absolute_expires_at: "2026-05-21T20:00:00.000Z",
};

describe("useRemoteStore watchlists", () => {
  beforeEach(() => {
    vi.mocked(remoteClient.loadSession).mockResolvedValue(session);
    vi.mocked(remoteClient.listAgents).mockResolvedValue([]);
    vi.mocked(remoteClient.listWorkflows).mockResolvedValue([]);
    vi.mocked(remoteClient.loadWatchlists).mockReset();
    vi.mocked(remoteClient.openStatusStream).mockResolvedValue({ close: vi.fn() } as unknown as WebSocket);
    localStorage.clear();
    useRemoteStore.getState().disconnectStatusStream();
    useRemoteStore.setState({
      agents: [],
      workflows: [],
      watchlists: [],
      teams: [],
      watchlistPrefs: DEFAULT_WATCHLIST_PREFS,
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      mobileCollapsedTeamIds: [],
      activeAgentId: null,
      status: "loading",
    });
  });

  it("loads and normalizes remote watchlists and team state", async () => {
    localStorage.setItem("wardian.remote.activeWatchlistId", "main");
    vi.mocked(remoteClient.loadWatchlists).mockResolvedValue({
      watchlists: [{ id: "main", name: "Main", entries: [{ type: "team", teamId: "team-1" }] }],
      teams: [{ id: "team-1", name: "Core Team", agentIds: ["agent-2", "agent-1"] }],
      prefs: {
        columns: [],
        sort: null,
        preserve_team_grouping_when_sorted: false,
        collapsed_team_ids: ["team-1"],
      },
    });

    await useRemoteStore.getState().load();

    expect(useRemoteStore.getState().watchlists[0]?.id).toBe("main");
    expect(useRemoteStore.getState().teams[0]?.agentIds).toEqual(["agent-2", "agent-1"]);
    expect(useRemoteStore.getState().activeWatchlistId).toBe("main");
    expect(useRemoteStore.getState().mobileCollapsedTeamIds).toEqual(["team-1"]);
  });

  it("falls back to all agents when the remote watchlist endpoint is unavailable", async () => {
    vi.mocked(remoteClient.loadWatchlists).mockRejectedValue(new RemoteRequestError("not found", 404));

    await useRemoteStore.getState().load();

    expect(useRemoteStore.getState().watchlists).toEqual([]);
    expect(useRemoteStore.getState().teams).toEqual([]);
    expect(useRemoteStore.getState().activeWatchlistId).toBe("all");
  });
});
