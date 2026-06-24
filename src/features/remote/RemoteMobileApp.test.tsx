import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent } from "../../types";
import {
  clearStoredRemoteIdentity,
  createRemoteDeviceKeyPair,
  defaultRemoteDeviceLabel,
  loadStoredRemoteIdentity,
  saveStoredRemoteIdentity,
  signRemoteAuthChallenge,
} from "./remoteIdentity";
import { remoteClient } from "./remoteClient";
import { RemoteBottomNav } from "./RemoteBottomNav";
import { RemoteMobileApp } from "./RemoteMobileApp";
import { RemoteWatchlistView } from "./RemoteWatchlistView";
import { useRemoteStore } from "./useRemoteStore";

vi.mock("./remoteIdentity", () => ({
  createRemoteDeviceKeyPair: vi.fn(),
  defaultRemoteDeviceLabel: vi.fn(),
  loadStoredRemoteIdentity: vi.fn(),
  saveStoredRemoteIdentity: vi.fn(),
  signRemoteAuthChallenge: vi.fn(),
  clearStoredRemoteIdentity: vi.fn(),
}));

const fetchMock = vi.fn();
globalThis.fetch = fetchMock;
let scrollIntoViewMock: ReturnType<typeof vi.fn>;
let clipboardWriteTextMock: ReturnType<typeof vi.fn>;

class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 3;
  static instances: MockWebSocket[] = [];

  sent: string[] = [];
  listeners: Record<string, Array<(event: any) => void>> = {};
  readyState = MockWebSocket.CONNECTING;

  constructor(readonly url: string) {
    MockWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: any) => void) {
    this.listeners[type] = [...(this.listeners[type] ?? []), listener];
  }

  send(payload: string) {
    this.sent.push(payload);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
  }

  emit(type: string, event: any = {}) {
    if (type === "open") this.readyState = MockWebSocket.OPEN;
    if (type === "close") this.readyState = MockWebSocket.CLOSED;
    for (const listener of this.listeners[type] ?? []) listener(event);
  }
}

function bytesToBase64(bytes: number[]) {
  return btoa(String.fromCharCode(...bytes));
}

function mockRemoteAgentDetailFetch(
  provider: string,
  options: {
    chatEvents?: AgentChatEvent[];
    status?: string;
  } = {},
) {
  const { chatEvents = [], status = "Idle" } = options;
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    if (url === "/remote/api/session") {
      return Promise.resolve(
        new Response(
          JSON.stringify({
            csrf_nonce: "csrf-1",
            expires_at: "2026-05-21T08:05:00.000Z",
            absolute_expires_at: "2026-05-21T20:00:00.000Z",
          }),
          { status: 200 },
        ),
      );
    }
    if (url === "/remote/api/agents") {
      return Promise.resolve(
        new Response(
          JSON.stringify({
            agents: [
              {
                session_id: "agent-1",
                session_name: "Coder",
                agent_class: "Coder",
                provider,
                workspace: "<absolute-workspace-path>",
                status,
                latest_text: null,
              },
            ],
          }),
          { status: 200 },
        ),
      );
    }
    if (url === "/remote/api/workflows") {
      return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
    }
    if (url === "/remote/api/agents/agent-1/chat") {
      return Promise.resolve(new Response(JSON.stringify({ events: chatEvents }), { status: 200 }));
    }
    if (url === "/remote/api/agents/action" && init?.method === "POST") {
      return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    }
    if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
      return Promise.resolve(
        new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
          status: 200,
        }),
      );
    }
    return Promise.resolve(new Response("{}", { status: 404 }));
  });
}

const initialRemoteStoreState = useRemoteStore.getState();

describe("RemoteMobileApp", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clipboardWriteTextMock = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: clipboardWriteTextMock },
    });
    vi.mocked(Terminal).mockImplementation(function MockTerminal(options) {
      return {
        open: vi.fn(),
        write: vi.fn(),
        resize: vi.fn(),
        clear: vi.fn(),
        onData: vi.fn(),
        onBinary: vi.fn(),
        onTitleChange: vi.fn(),
        onResize: vi.fn(),
        onScroll: vi.fn(),
        reset: vi.fn(),
        dispose: vi.fn(),
        focus: vi.fn(),
        attachCustomKeyEventHandler: vi.fn(),
        selectAll: vi.fn(),
        loadAddon: vi.fn(),
        scrollLines: vi.fn(),
        scrollToBottom: vi.fn(),
        scrollToTop: vi.fn(),
        options: { ...(options ?? {}) },
        cols: 80,
        rows: 24,
      } as unknown as Terminal;
    });
    scrollIntoViewMock = vi.fn();
    Object.defineProperty(Element.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoViewMock,
    });
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
    document.documentElement.removeAttribute("style");
    window.history.pushState({}, "", "/remote");
    vi.mocked(createRemoteDeviceKeyPair).mockResolvedValue({
      privateKey: { type: "private" } as CryptoKey,
      publicKeySpkiDerBase64: "phone-spki",
    });
    vi.mocked(defaultRemoteDeviceLabel).mockReturnValue("Pixel phone");
    vi.mocked(loadStoredRemoteIdentity).mockResolvedValue(null);
    vi.mocked(saveStoredRemoteIdentity).mockResolvedValue(undefined);
    vi.mocked(clearStoredRemoteIdentity).mockResolvedValue(undefined);
    vi.mocked(signRemoteAuthChallenge).mockResolvedValue("signature-der");
    useRemoteStore.setState({
      agents: [],
      workflows: [],
      remoteQueueItems: [],
      remoteQueueBuffers: {},
      remoteAgentStatuses: {},
      watchlists: [],
      teams: [],
      watchlistPrefs: { columns: [], sort: null, preserve_team_grouping_when_sorted: false, collapsed_team_ids: [] },
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      mobileCollapsedTeamIds: [],
      status: "loading",
      activeAgentId: null,
      activeAgentViewMode: "terminal",
      terminalSnapshot: null,
      terminalLoading: false,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatError: "",
      sending: false,
      load: initialRemoteStoreState.load,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    remoteClient.setCsrfNonce(null);
  });

  it("collapses team members locally from the mobile watchlist chevron", async () => {
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
        {
          session_id: "agent-2",
          session_name: "Beta",
          agent_class: "Reviewer",
          provider: "claude",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      teams: [{ id: "team-1", name: "Core Team", agentIds: ["agent-1", "agent-2"] }],
      watchlists: [],
      watchlistPrefs: { columns: [], sort: null, preserve_team_grouping_when_sorted: false, collapsed_team_ids: [] },
      mobileCollapsedTeamIds: [],
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      status: "ready",
    });

    render(<RemoteWatchlistView />);

    expect(screen.getByText("Alpha")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Collapse Core Team" }));
    expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
    expect(screen.queryByText("Beta")).not.toBeInTheDocument();
    expect(screen.getByText("Core Team")).toBeVisible();
  });

  it("opens agent detail when a mobile watchlist row is tapped", async () => {
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      teams: [],
      watchlists: [],
      watchlistPrefs: { columns: [], sort: null, preserve_team_grouping_when_sorted: false, collapsed_team_ids: [] },
      mobileCollapsedTeamIds: [],
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      status: "ready",
    });

    render(<RemoteWatchlistView />);

    await userEvent.click(screen.getByRole("button", { name: "Open Alpha details" }));
    expect(useRemoteStore.getState().activeAgentId).toBe("agent-1");
  });

  it("returns from agent detail to the watchlist when swiping from the left edge", async () => {
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      teams: [],
      watchlists: [],
      watchlistPrefs: { columns: [], sort: null, preserve_team_grouping_when_sorted: false, collapsed_team_ids: [] },
      mobileCollapsedTeamIds: [],
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      activeAgentId: "agent-1",
      activeAgentViewMode: "chat",
      status: "ready",
      load: vi.fn(async () => {}),
    });

    render(<RemoteMobileApp />);

    const detail = screen.getByTestId("remote-agent-detail");
    fireEvent.touchStart(detail, { touches: [{ clientX: 12, clientY: 240 }] });
    fireEvent.touchMove(detail, { touches: [{ clientX: 104, clientY: 246 }] });
    fireEvent.touchEnd(detail, { changedTouches: [{ clientX: 104, clientY: 246 }], touches: [] });

    expect(useRemoteStore.getState().activeAgentId).toBeNull();
    expect(screen.getByTestId("remote-watchlist-view")).toBeVisible();
  });

  it("updates the active remote tab from the compact mobile bottom nav", async () => {
    useRemoteStore.setState({
      activeRemoteTab: "watchlist",
      status: "ready",
    });

    render(<RemoteBottomNav />);

    expect(screen.getByRole("button", { name: "Watchlist" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("button", { name: "Graph" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Garden" }));
    expect(useRemoteStore.getState().activeRemoteTab).toBe("garden");
  });

  it("shows bottom navigation placeholders without exposing watchlist actions", async () => {
    useRemoteStore.setState({
      agents: [
        {
          session_id: "agent-1",
          session_name: "Alpha",
          agent_class: "Coder",
          provider: "codex",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: null,
        },
      ],
      workflows: [],
      teams: [],
      watchlists: [],
      watchlistPrefs: { columns: [], sort: null, preserve_team_grouping_when_sorted: false, collapsed_team_ids: [] },
      activeWatchlistId: "all",
      activeRemoteTab: "watchlist",
      mobileCollapsedTeamIds: [],
      status: "ready",
      load: vi.fn(async () => {}),
    });

    render(<RemoteMobileApp />);

    expect(screen.getByRole("navigation", { name: "Remote sections" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Watchlist" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByTestId("remote-watchlist-view")).toBeVisible();
    expect(screen.getByRole("button", { name: "Open Alpha details" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Broadcast" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Prompt")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Garden" }));
    expect(screen.getByText("Garden is not available in the mobile PWA yet.")).toBeVisible();
  });

  it("broadcasts a prompt from an explicit mobile watchlist action", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    mockRemoteAgentDetailFetch("codex");

    render(<RemoteMobileApp />);

    expect(await screen.findByRole("button", { name: /Open Coder details/i })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Broadcast" })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Open broadcast prompt" }));
    await userEvent.type(screen.getByLabelText("Broadcast prompt"), "status please");
    await userEvent.click(screen.getByRole("button", { name: "Broadcast" }));

    await waitFor(() => {
      expect(window.confirm).toHaveBeenCalledWith("Broadcast to 1 agent?");
      const actionCalls = fetchMock.mock.calls.filter(([url, init]) => url === "/remote/api/agents/action" && init?.method === "POST");
      expect(actionCalls).toHaveLength(1);
      expect(JSON.parse(actionCalls[0][1]?.body as string)).toEqual({
        action: "send_prompt",
        target: "agent-1",
        prompt: "status please",
      });
    });
  });

  it("pairs from a QR URL, waits for desktop approval, then authenticates the phone", async () => {
    window.history.pushState(
      {},
      "",
      "/remote?pairing_offer_id=offer-1&nonce=nonce-1&server_fingerprint=desktop-fp",
    );
    let pairingStatusCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/pairing/submit" && init?.method === "POST") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              status: "pending",
              pairing_request_id: "pairing-request-1",
              device_id: "dev-1",
              public_key_fingerprint: "phone-fp",
              paired_at: null,
              expires_at: "2099-05-21T08:01:00.000Z",
            }),
            { status: 202 },
          ),
        );
      }
      if (url === "/remote/api/pairing/pairing-request-1") {
        pairingStatusCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              status: pairingStatusCalls === 1 ? "pending" : "approved",
              pairing_request_id: "pairing-request-1",
              device_id: "dev-1",
              public_key_fingerprint: "phone-fp",
              paired_at: pairingStatusCalls === 1 ? null : "2026-05-21T08:00:30.000Z",
              expires_at: "2099-05-21T08:01:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/auth/challenge" && init?.method === "POST") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              challenge_id: "challenge-1",
              device_id: "dev-1",
              origin: "https://wardian.tailnet.ts.net",
              server_identity_fingerprint: "desktop-fp",
              nonce: "auth-nonce",
              expires_at: "2026-05-21T08:02:00.000Z",
              audience: "wardian_remote_pwa",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/auth/session" && init?.method === "POST") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:15:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:15:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Waiting for desktop approval.")).toBeVisible();
    expect(await screen.findByText("Coder")).toBeVisible();
    expect(saveStoredRemoteIdentity).toHaveBeenCalledWith(
      expect.objectContaining({
        device_id: "dev-1",
        public_key_fingerprint: "phone-fp",
        server_identity_fingerprint: "desktop-fp",
        origin: window.location.origin,
        private_key: { type: "private" },
      }),
    );
    expect(signRemoteAuthChallenge).toHaveBeenCalledWith(
      { type: "private" },
      expect.objectContaining({ challenge_id: "challenge-1" }),
    );
    const pairingSubmitCall = fetchMock.mock.calls.find(([url]) => url === "/remote/api/pairing/submit");
    expect(JSON.parse(pairingSubmitCall?.[1]?.body as string)).toEqual({
      pairing_offer_id: "offer-1",
      nonce: "nonce-1",
      device_label: "Pixel phone",
      public_key_spki_der_base64: "phone-spki",
    });
    const sessionCall = fetchMock.mock.calls.find(([url]) => url === "/remote/api/auth/session");
    expect(JSON.parse(sessionCall?.[1]?.body as string)).toEqual({
      challenge_id: "challenge-1",
      device_id: "dev-1",
      signature_der_base64: "signature-der",
    });
  });

  it("bootstraps the session nonce, shows the watchlist, and opens the status stream", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await screen.findByText("Coder");
    expect(screen.getByTestId("remote-mobile-app")).toHaveClass("h-dvh", "overflow-hidden");
    expect(screen.getByTestId("remote-watchlist-view")).toBeVisible();
    expect(screen.getByTestId("remote-scroll-region")).toHaveClass("min-h-0", "overflow-y-auto");
    expect(screen.getByTestId("remote-agent-list")).not.toHaveClass("grid-cols-1");
    expect(screen.getByTestId("remote-watchlist-agent-row")).toHaveTextContent("Coder");
    expect(screen.queryByRole("article", { name: /Coder/i })).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledWith("/remote/api/session", expect.objectContaining({ method: "GET" }));
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/ws-ticket",
        expect.objectContaining({ method: "POST" }),
      );
    });
    expect(MockWebSocket.instances[0]?.url).toContain("/remote/api/status-stream");
    MockWebSocket.instances[0]?.emit("open");
    expect(JSON.parse(MockWebSocket.instances[0]?.sent[0] ?? "{}")).toEqual({ ticket: "ws-ticket-1" });

    expect(screen.queryByLabelText("Prompt")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Broadcast" })).not.toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([url]) => url === "/remote/api/agents/action")).toBe(false);
  });

  it("keeps the remote shell usable when workflow listing is unavailable", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response("{}", { status: 404 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Coder")).toBeVisible();
    expect(screen.queryByText("Desktop unreachable.")).not.toBeInTheDocument();
    expect(screen.getByTestId("remote-watchlist-view")).toBeVisible();
    expect(screen.getByRole("navigation", { name: "Remote sections" })).toBeVisible();
  });

  it("keeps the loaded roster visible when the live status stream is unavailable", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(new Response("{}", { status: 503 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Coder")).toBeVisible();
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/ws-ticket",
        expect.objectContaining({ method: "POST" }),
      );
    });
    expect(screen.queryByText("Desktop unreachable.")).not.toBeInTheDocument();
  });

  it("refreshes stale agent status when the mobile PWA regains focus", async () => {
    let agentListCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        agentListCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: agentListCalls === 1 ? "Processing..." : "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Processing...")).toBeVisible();

    window.dispatchEvent(new Event("focus"));

    await waitFor(() => expect(screen.getByText("Idle")).toBeVisible());
    expect(agentListCalls).toBeGreaterThanOrEqual(2);
  });

  it("opens a selected agent detail view with terminal attach selected by default, keeps chat one tap away, and sends prompts", async () => {
    let chatCalls = 0;
    let terminalCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        chatCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              events: [
                {
                  id: "agent-1:1",
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "message",
                  role: "assistant",
                  text: "I can see the selected conversation.",
                  title: null,
                  status: null,
                  turn_id: "turn-1",
                  source: "provider_log",
                  command: null,
                  exit_code: null,
                  path: null,
                  language: null,
                  created_at: "2026-05-21T08:00:00.000Z",
                  sequence: 1,
                  metadata: {},
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/terminal") {
        terminalCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              snapshot: {
                cursor: `agent-1:${terminalCalls}`,
                text: "terminal says ready",
                truncated: false,
                omitted_bytes: 0,
              },
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    document.documentElement.style.setProperty("--color-wardian-card", "rgb(250, 251, 252)");
    document.documentElement.style.setProperty("--color-wardian-text", "rgb(17, 24, 39)");
    document.documentElement.style.setProperty("--color-wardian-accent", "rgb(146, 106, 9)");
    document.documentElement.style.setProperty("--color-wardian-border", "rgb(229, 231, 235)");

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    expect(MockWebSocket.instances[1]?.url).toContain("/remote/api/agents/agent-1/terminal-stream");
    act(() => {
      MockWebSocket.instances[1]?.emit("open");
    });
    expect(JSON.parse(MockWebSocket.instances[1]?.sent[0] ?? "{}")).toEqual({
      ticket: "ws-ticket-1",
      cols: 80,
      rows: 24,
    });
    const terminalTicketCall = fetchMock.mock.calls
      .filter(([url]) => url === "/remote/api/ws-ticket")
      .map(([, init]) => JSON.parse(init?.body as string))
      .find((body) => body.stream === "terminal_attach");
    expect(terminalTicketCall).toEqual({ stream: "terminal_attach" });
    expect(terminalCalls).toBe(0);
    const terminalHost = screen.getByTestId("remote-terminal-attach");
    expect(terminalHost).toBeVisible();
    expect(terminalHost).toHaveClass("remote-terminal-hide-composition");
    expect(screen.getByTestId("remote-agent-detail")).toHaveClass("h-dvh", "overflow-hidden");
    expect(screen.getByRole("region", { name: "Coder terminal" })).toHaveClass("flex", "min-h-0", "flex-col", "overflow-hidden");
    expect(screen.getByTestId("remote-terminal-scroll-surface")).toHaveClass("min-h-0", "flex-1", "overflow-hidden");
    expect(screen.getByTestId("remote-terminal-scroll-surface")).not.toHaveClass("h-full");
    expect(terminalHost).not.toHaveClass("overflow-hidden");
    expect(terminalHost).not.toHaveClass("min-h-[280px]");
    const terminalCallsForOptions = vi.mocked(Terminal).mock.calls;
    const terminalOptions = terminalCallsForOptions[terminalCallsForOptions.length - 1]?.[0] as Record<string, unknown> | undefined;
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminalInstance = terminalResults[terminalResults.length - 1]?.value as {
      attachCustomKeyEventHandler?: ReturnType<typeof vi.fn>;
    };
    expect(terminalInstance.attachCustomKeyEventHandler).toHaveBeenCalledTimes(1);
    expect(terminalOptions?.theme).toEqual(
      expect.objectContaining({
        background: "rgb(250, 251, 252)",
        cursor: "rgb(146, 106, 9)",
        foreground: "rgb(17, 24, 39)",
        selectionBackground: "rgb(229, 231, 235)",
      }),
    );
    expect(terminalOptions?.cursorStyle).toBe("bar");
    expect(screen.getByRole("button", { name: "Terminal" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Chat" })).toHaveAttribute("aria-pressed", "false");
    expect(chatCalls).toBe(0);
    expect(scrollIntoViewMock).toHaveBeenCalled();
    expect(screen.queryByLabelText("Prompt Coder")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send prompt" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Chat" }));
    expect(await screen.findByText("I can see the selected conversation.")).toBeVisible();
    expect(chatCalls).toBe(1);
    expect(screen.getByLabelText("Prompt Coder")).toBeVisible();
    await userEvent.type(screen.getByLabelText("Prompt Coder"), "what changed?");
    await userEvent.click(screen.getByRole("button", { name: "Send prompt" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/action",
        expect.objectContaining({
          credentials: "same-origin",
          method: "POST",
          headers: expect.objectContaining({ "x-wardian-csrf": "csrf-1" }),
        }),
      );
    });
    const actionCall = fetchMock.mock.calls.find(([url]) => url === "/remote/api/agents/action");
    expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
      action: "send_prompt",
      target: "agent-1",
      prompt: "what changed?",
    });
  });

  it("keeps the mobile chat composer editable while the selected agent is processing", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Processing...",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        return Promise.resolve(new Response(JSON.stringify({ events: [] }), { status: 200 }));
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    const input = await screen.findByLabelText("Prompt Coder");
    expect(input).not.toBeDisabled();

    await userEvent.type(input, "continue while running");
    await userEvent.click(screen.getByRole("button", { name: "Send prompt" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/action",
        expect.objectContaining({
          credentials: "same-origin",
          method: "POST",
        }),
      );
    });
    const actionCall = fetchMock.mock.calls.find(([url]) => url === "/remote/api/agents/action");
    expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
      action: "send_prompt",
      target: "agent-1",
      prompt: "continue while running",
    });
  });

  it("sends typed slash commands through command input mode without a composer toggle", async () => {
    mockRemoteAgentDetailFetch("codex");

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    expect(screen.queryByRole("button", { name: "Command mode" })).not.toBeInTheDocument();
    await userEvent.type(await screen.findByLabelText("Prompt Coder"), "/status");
    await userEvent.click(screen.getByRole("button", { name: "Send prompt" }));

    await waitFor(() => {
      const actionCall = fetchMock.mock.calls.find(([url, init]) => url === "/remote/api/agents/action" && init?.method === "POST");
      expect(actionCall).toBeTruthy();
      expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
        action: "send_prompt",
        target: "agent-1",
        prompt: "/status",
        input_mode: "command",
      });
    });
  });

  it("renders remote chat messages with desktop markdown and copy behavior", async () => {
    mockRemoteAgentDetailFetch("codex", {
      chatEvents: [
        {
          id: "assistant-markdown",
          session_id: "agent-1",
          provider: "codex",
          kind: "message",
          role: "assistant",
          text: "Use **bold** and `npm test`.",
          title: null,
          status: null,
          turn_id: "turn-1",
          source: "provider_log",
          command: null,
          exit_code: null,
          path: null,
          language: null,
          created_at: "2026-05-21T08:00:00.000Z",
          sequence: 1,
          metadata: {},
        },
      ],
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    const article = await screen.findByLabelText("assistant message");
    expect(within(article).getByText("bold").tagName).toBe("STRONG");
    expect(within(article).getByText("npm test").tagName).toBe("CODE");

    await userEvent.click(within(article).getByRole("button", { name: "Copy message" }));
    await waitFor(() => expect(clipboardWriteTextMock).toHaveBeenCalledWith("Use **bold** and `npm test`."));
  });

  it("submits remote approval choices through the PWA prompt action", async () => {
    mockRemoteAgentDetailFetch("codex", {
      status: "Action Required",
      chatEvents: [
        {
          id: "approval-required",
          session_id: "agent-1",
          provider: "codex",
          kind: "approval",
          role: null,
          text: "Do you want to proceed?\n\n1. Yes, and always allow\n2. No",
          title: "Approval required",
          status: "action_required",
          turn_id: "turn-1",
          source: "provider_log",
          command: null,
          exit_code: null,
          path: null,
          language: null,
          created_at: "2026-05-21T08:00:00.000Z",
          sequence: 1,
          metadata: {},
        },
      ],
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    expect(await screen.findByTestId("remote-activity-row-approval")).toHaveTextContent(
      "Action required. Choose a response or type below.",
    );
    await userEvent.click(await screen.findByRole("button", { name: "Send approval response 1: Yes, and always allow" }));

    await waitFor(() => {
      const actionCall = fetchMock.mock.calls.find(([url, init]) => url === "/remote/api/agents/action" && init?.method === "POST");
      expect(actionCall).toBeTruthy();
      expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
        action: "send_prompt",
        target: "agent-1",
        prompt: "1",
      });
    });
  });

  it("disables the remote chat composer when the selected agent cannot accept chat input", async () => {
    mockRemoteAgentDetailFetch("codex", { status: "Headless" });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    const input = await screen.findByLabelText("Prompt Coder");
    expect(input).toBeDisabled();
    expect(input).toHaveAttribute("placeholder", "Agent is headless");
    expect(screen.getByRole("button", { name: "Send prompt" })).toBeDisabled();
  });

  it("shows an optimistic remote user message while the post-send chat refresh is pending", async () => {
    let chatCalls = 0;
    let resolveRefresh: ((response: Response) => void) | null = null;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        chatCalls += 1;
        if (chatCalls === 1) {
          return Promise.resolve(new Response(JSON.stringify({ events: [] }), { status: 200 }));
        }
        return new Promise<Response>((resolve) => {
          resolveRefresh = resolve;
        });
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));
    await userEvent.type(await screen.findByLabelText("Prompt Coder"), "show status");
    await userEvent.click(screen.getByRole("button", { name: "Send prompt" }));

    expect(await screen.findByLabelText("user message")).toHaveTextContent("show status");

    act(() => {
      resolveRefresh?.(
        new Response(
          JSON.stringify({
            events: [
              {
                id: "confirmed-user",
                session_id: "agent-1",
                provider: "codex",
                kind: "message",
                role: "user",
                text: "show status",
                title: null,
                status: "succeeded",
                turn_id: "turn-1",
                source: "provider_log",
                command: null,
                exit_code: null,
                path: null,
                language: null,
                created_at: "2026-05-21T08:00:00.000Z",
                sequence: 1,
                metadata: {},
              },
            ],
          }),
          { status: 200 },
        ),
      );
    });

    await waitFor(() => expect(screen.getAllByLabelText("user message")).toHaveLength(1));
  });

  it("clears the active remote chat immediately after a clear action succeeds", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    let chatCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        chatCalls += 1;
        if (chatCalls === 1) {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                events: [
                  {
                    id: "message-before-clear",
                    session_id: "agent-1",
                    provider: "codex",
                    kind: "message",
                    role: "assistant",
                    text: "Clear me from the PWA chat.",
                    title: null,
                    status: null,
                    turn_id: "turn-1",
                    source: "provider_log",
                    command: null,
                    exit_code: null,
                    path: null,
                    language: null,
                    created_at: "2026-05-21T08:00:00.000Z",
                    sequence: 1,
                    metadata: {},
                  },
                ],
              }),
              { status: 200 },
            ),
          );
        }
        return new Promise<Response>(() => {});
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));
    expect(await screen.findByText("Clear me from the PWA chat.")).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    await waitFor(() => {
      const actionCall = fetchMock.mock.calls.find(([url, init]) => url === "/remote/api/agents/action" && init?.method === "POST");
      expect(actionCall).toBeTruthy();
      expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
        action: "clear",
        target: "agent-1",
      });
    });

    expect(screen.queryByText("Clear me from the PWA chat.")).not.toBeInTheDocument();
    expect(screen.getByText("No chat transcript yet.")).toBeVisible();
  });

  it("lazy loads older remote chat rows from the latest transcript window", async () => {
    const chatEvents: AgentChatEvent[] = Array.from({ length: 85 }, (_, index) => ({
      id: `message-${index + 1}`,
      session_id: "agent-1",
      provider: "codex",
      kind: "message",
      role: "assistant",
      text: `Message ${index + 1}`,
      title: null,
      status: null,
      turn_id: "turn-1",
      source: "provider_log",
      command: null,
      exit_code: null,
      path: null,
      language: null,
      created_at: `2026-05-21T08:${String(index).padStart(2, "0")}:00.000Z`,
      sequence: index + 1,
      metadata: {},
    }));
    mockRemoteAgentDetailFetch("codex", { chatEvents });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    expect(await screen.findByText("Message 85")).toBeVisible();
    expect(screen.queryByText("Message 1")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Load 5 earlier transcript rows" }));
    expect(await screen.findByText("Message 1")).toBeVisible();
  });

  it("keeps long remote terminal fallback rows compact until expanded", async () => {
    const terminalOutput = Array.from({ length: 45 }, (_, index) => `terminal line ${index + 1}`).join("\n");
    mockRemoteAgentDetailFetch("codex", {
      chatEvents: [
        {
          id: "terminal-fallback",
          session_id: "agent-1",
          provider: "codex",
          kind: "terminal_output",
          role: null,
          text: terminalOutput,
          title: "Terminal output",
          status: null,
          turn_id: "turn-1",
          source: "provider_log",
          command: null,
          exit_code: null,
          path: null,
          language: "terminal",
          created_at: "2026-05-21T08:00:00.000Z",
          sequence: 1,
          metadata: {},
        },
      ],
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    const row = await screen.findByTestId("remote-activity-row-terminal-fallback");
    expect(row).toHaveTextContent("terminal line 1");
    expect(row).not.toHaveTextContent("terminal line 45");

    await userEvent.click(within(row).getByRole("button", { name: "Copy activity output" }));
    await waitFor(() => expect(clipboardWriteTextMock).toHaveBeenCalledWith(terminalOutput));

    await userEvent.click(within(row).getByRole("button", { name: "Show output" }));
    expect(row).toHaveTextContent("terminal line 45");
  });

  it("renders remote chat work logs with concrete tool call details", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Processing...",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              events: [
                {
                  id: "shell-call-1",
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "tool_call",
                  role: null,
                  text: null,
                  title: "shell_command",
                  status: "running",
                  turn_id: null,
                  source: "provider_log",
                  command: "Get-ChildItem src/features/grid",
                  exit_code: null,
                  path: null,
                  language: "shell",
                  created_at: "2026-05-21T08:00:00.000Z",
                  sequence: 1,
                  metadata: { raw_type: "custom_tool_call", tool_name: "shell_command" },
                },
                {
                  id: "shell-call-2",
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "tool_call",
                  role: null,
                  text: null,
                  title: "shell_command",
                  status: "running",
                  turn_id: null,
                  source: "provider_log",
                  command: "cargo test -p Wardian commands::terminal::tests",
                  exit_code: null,
                  path: null,
                  language: "shell",
                  created_at: "2026-05-21T08:00:01.000Z",
                  sequence: 2,
                  metadata: { raw_type: "custom_tool_call", tool_name: "shell_command" },
                },
                {
                  id: "shell-result-1",
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "tool_result",
                  role: "tool",
                  text: "commands::terminal::tests passed",
                  title: "Tool result",
                  status: "succeeded",
                  turn_id: null,
                  source: "provider_log",
                  command: null,
                  exit_code: 0,
                  path: null,
                  language: null,
                  created_at: "2026-05-21T08:00:02.000Z",
                  sequence: 3,
                  metadata: {},
                },
                {
                  id: "shell-call-3",
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "tool_call",
                  role: null,
                  text: null,
                  title: "shell_command",
                  status: "running",
                  turn_id: null,
                  source: "provider_log",
                  command: "npm run test -- src/features/grid/AgentChatView.test.tsx",
                  exit_code: null,
                  path: null,
                  language: "shell",
                  created_at: "2026-05-21T08:00:03.000Z",
                  sequence: 4,
                  metadata: { raw_type: "custom_tool_call", tool_name: "shell_command" },
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    const group = await screen.findByText("Work log");
    const article = group.closest("article") as HTMLElement;

    expect(article).toHaveTextContent("4 events");
    expect(article).toHaveTextContent("Get-ChildItem src/features/grid");
    expect(article).toHaveTextContent("cargo test -p Wardian commands::terminal::tests");
    expect(article).toHaveTextContent("npm run test -- src/features/grid/AgentChatView.test.tsx");
    expect(article).not.toHaveTextContent("running");

    await userEvent.click(within(article).getByRole("button", { name: "Copy work log" }));
    await waitFor(() => {
      expect(clipboardWriteTextMock).toHaveBeenCalledWith(expect.stringContaining("Get-ChildItem src/features/grid"));
      expect(clipboardWriteTextMock).toHaveBeenCalledWith(expect.stringContaining("commands::terminal::tests passed"));
      expect(clipboardWriteTextMock).toHaveBeenCalledWith(expect.stringContaining("npm run test -- src/features/grid/AgentChatView.test.tsx"));
    });
  });

  it("renders remote diff and todo tool activity with desktop-style structure", async () => {
    mockRemoteAgentDetailFetch("codex", {
      chatEvents: [
        {
          id: "diff-output",
          session_id: "agent-1",
          provider: "codex",
          kind: "tool_result",
          role: "tool",
          text: "diff --git a/src/App.tsx b/src/App.tsx\n+added\n-removed",
          title: "Tool result",
          status: "succeeded",
          turn_id: "turn-1",
          source: "provider_log",
          command: null,
          exit_code: 0,
          path: null,
          language: "diff",
          created_at: "2026-05-21T08:00:00.000Z",
          sequence: 1,
          metadata: {},
        },
        {
          id: "todo-tool",
          session_id: "agent-1",
          provider: "codex",
          kind: "tool_call",
          role: null,
          text: "- [x] Inspect transcript\n- [ ] Add mobile parity",
          title: "todowrite",
          status: "succeeded",
          turn_id: "turn-1",
          source: "provider_log",
          command: null,
          exit_code: null,
          path: null,
          language: "markdown",
          created_at: "2026-05-21T08:00:01.000Z",
          sequence: 2,
          metadata: { tool_name: "todowrite" },
        },
      ],
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    expect(await screen.findByTestId("remote-tool-diff-panel")).toHaveTextContent("1 file");
    expect(screen.getByTestId("remote-tool-diff-panel")).toHaveTextContent("+1");
    expect(screen.getByTestId("remote-tool-diff-panel")).toHaveTextContent("-1");
    expect(screen.getByTestId("remote-tool-todo-list")).toHaveTextContent("Inspect transcript");
    expect(screen.getByTestId("remote-tool-todo-list")).toHaveTextContent("Add mobile parity");
  });

  it("surfaces changed files inside remote grouped work logs", async () => {
    mockRemoteAgentDetailFetch("codex", {
      chatEvents: [
        {
          id: "shell-call-1",
          session_id: "agent-1",
          provider: "codex",
          kind: "tool_call",
          role: null,
          text: null,
          title: "shell_command",
          status: "running",
          turn_id: null,
          source: "provider_log",
          command: "git status --short",
          exit_code: null,
          path: null,
          language: "shell",
          created_at: "2026-05-21T08:00:00.000Z",
          sequence: 1,
          metadata: { tool_name: "shell_command" },
        },
        {
          id: "diff-result",
          session_id: "agent-1",
          provider: "codex",
          kind: "tool_result",
          role: "tool",
          text: "diff --git a/src/remote.ts b/src/remote.ts\n+added",
          title: "Tool result",
          status: "succeeded",
          turn_id: null,
          source: "provider_log",
          command: null,
          exit_code: 0,
          path: null,
          language: "diff",
          created_at: "2026-05-21T08:00:01.000Z",
          sequence: 2,
          metadata: { changed_files: ["src/remote.ts"] },
        },
        {
          id: "shell-call-2",
          session_id: "agent-1",
          provider: "codex",
          kind: "tool_call",
          role: null,
          text: null,
          title: "shell_command",
          status: "running",
          turn_id: null,
          source: "provider_log",
          command: "npm test",
          exit_code: null,
          path: null,
          language: "shell",
          created_at: "2026-05-21T08:00:02.000Z",
          sequence: 3,
          metadata: { tool_name: "shell_command" },
        },
        {
          id: "shell-call-3",
          session_id: "agent-1",
          provider: "codex",
          kind: "tool_call",
          role: null,
          text: null,
          title: "shell_command",
          status: "running",
          turn_id: null,
          source: "provider_log",
          command: "npm run build",
          exit_code: null,
          path: null,
          language: "shell",
          created_at: "2026-05-21T08:00:03.000Z",
          sequence: 4,
          metadata: { tool_name: "shell_command" },
        },
      ],
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));

    const group = (await screen.findByText("Work log")).closest("article") as HTMLElement;
    expect(group).toHaveTextContent("Changed files");
    expect(group).toHaveTextContent("src/remote.ts");

    await userEvent.click(within(group).getByRole("button", { name: "Copy changed file paths" }));
    await waitFor(() => expect(clipboardWriteTextMock).toHaveBeenCalledWith("src/remote.ts"));
  });

  it("refreshes remote chat on repeated active status frames", async () => {
    let chatCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Processing...",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        chatCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              events: [
                {
                  id: `agent-1:${chatCalls}`,
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "message",
                  role: "assistant",
                  text: chatCalls === 1 ? "Initial active message." : "New active message.",
                  title: null,
                  status: null,
                  turn_id: "turn-1",
                  source: "provider_log",
                  command: null,
                  exit_code: null,
                  path: null,
                  language: null,
                  created_at: "2026-05-21T08:00:00.000Z",
                  sequence: chatCalls,
                  metadata: {},
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(await screen.findByRole("button", { name: "Chat" }));
    expect(await screen.findByText("Initial active message.")).toBeVisible();
    expect(chatCalls).toBe(1);

    act(() => {
      MockWebSocket.instances[0]?.emit("message", {
        data: JSON.stringify({
          type: "agent_status",
          agents: [
            {
              session_id: "agent-1",
              session_name: "Coder",
              agent_class: "Coder",
              provider: "codex",
              workspace: "<absolute-workspace-path>",
              status: "Processing...",
              latest_text: null,
            },
          ],
        }),
      });
    });

    expect(await screen.findByText("New active message.")).toBeVisible();
    expect(chatCalls).toBeGreaterThanOrEqual(2);
  });

  it("maps touch drag gestures in the terminal pane to xterm scrollback", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    const terminalHost = await screen.findByTestId("remote-terminal-attach");
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminalInstance = terminalResults[terminalResults.length - 1]?.value as { scrollLines?: ReturnType<typeof vi.fn> };
    terminalInstance.scrollLines = vi.fn();

    fireEvent.touchStart(terminalHost, { touches: [{ clientY: 220 }] });
    fireEvent.touchMove(terminalHost, { touches: [{ clientY: 184 }] });

    expect(terminalInstance.scrollLines).toHaveBeenCalledWith(2);
  });

  it("maps captured terminal child touch drags to xterm scrollback", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    const terminalHost = await screen.findByTestId("remote-terminal-attach");
    const terminalChild = document.createElement("div");
    terminalChild.addEventListener("touchstart", (event) => event.stopPropagation());
    terminalChild.addEventListener("touchmove", (event) => event.stopPropagation());
    terminalHost.appendChild(terminalChild);
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminalInstance = terminalResults[terminalResults.length - 1]?.value as { scrollLines?: ReturnType<typeof vi.fn> };
    terminalInstance.scrollLines = vi.fn();

    fireEvent.touchStart(terminalChild, { touches: [{ clientY: 220 }] });
    fireEvent.touchMove(terminalChild, { touches: [{ clientY: 184 }] });

    expect(terminalInstance.scrollLines).toHaveBeenCalledWith(2);
  });

  it("maps terminal pane drags outside the xterm host to xterm scrollback", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    const terminalSurface = await screen.findByTestId("remote-terminal-scroll-surface");
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminalInstance = terminalResults[terminalResults.length - 1]?.value as { scrollLines?: ReturnType<typeof vi.fn> };
    terminalInstance.scrollLines = vi.fn();

    fireEvent.touchStart(terminalSurface, { touches: [{ clientY: 220 }] });
    fireEvent.touchMove(terminalSurface, { touches: [{ clientY: 184 }] });

    expect(terminalInstance.scrollLines).toHaveBeenCalledWith(2);
  });

  it("sends terminal resize messages after mobile layout changes", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;
    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    try {
      render(<RemoteMobileApp />);

      await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
      await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
      act(() => {
        MockWebSocket.instances[1]?.emit("open");
      });

      const terminalResults = vi.mocked(Terminal).mock.results;
      const terminalInstance = terminalResults[terminalResults.length - 1]?.value as { cols: number; rows: number };
      terminalInstance.cols = 112;
      terminalInstance.rows = 31;

      act(() => {
        resizeCallback?.([], {} as ResizeObserver);
      });

      expect(MockWebSocket.instances[1]?.sent.map((payload) => JSON.parse(payload))).toContainEqual({
        type: "resize",
        cols: 112,
        rows: 31,
      });
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("does not clear xterm scrollback when a later terminal snapshot arrives", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.reset?.mock && value?.write?.mock) as {
      reset: ReturnType<typeof vi.fn>;
      write: ReturnType<typeof vi.fn>;
    };

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("history line 1\r\nhistory line 2\r\n"),
        }),
      });
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 100,
          rows: 30,
          state_base64: btoa("current viewport"),
        }),
      });
    });

    expect(terminalInstance.reset).toHaveBeenCalledTimes(1);
    expect(terminalInstance.write).toHaveBeenCalledTimes(2);
  });

  it("renders Codex repaint frames natively without journaling in the remote terminal", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.write?.mock) as {
      write: ReturnType<typeof vi.fn>;
    };
    const firstFrame = "\u001b[?2026h\u001b[?2026l\u001b[?25l\u001b[H  60\u001b[K\r\n  61\u001b[K\r\n  62\u001b[K";
    const secondFrame = "\u001b[?2026h\u001b[?2026l\u001b[?25l\u001b[H  61\u001b[K\r\n  62\u001b[K\r\n  63\u001b[K";

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa(firstFrame + secondFrame),
        }),
      });
    });

    const written = terminalInstance.write.mock.calls[0]?.[0];
        expect(written).not.toContain("\u001b[999;1H");
        expect(written).toContain("  63");
  });

  it("renders Codex's dark composer background as a light fill in the remote mobile terminal", async () => {
    document.documentElement.style.setProperty("--color-wardian-card", "#fcfaf5");

    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.write?.mock) as {
      write: ReturnType<typeof vi.fn>;
    };
    const codexComposerFrame = "\u001b[48;2;41;41;41m\n\u001b[K";

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa(codexComposerFrame),
        }),
      });
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "update",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          state_base64: btoa(codexComposerFrame),
        }),
      });
    });

    const writes = terminalInstance.write.mock.calls.map(([data]) => String(data));
    expect(writes).toEqual([
      "\u001b[48;2;242;240;235m\n\u001b[K",
      "\u001b[48;2;242;240;235m\n\u001b[K",
    ]);
    expect(writes.join("")).not.toContain("\u001b[48;2;41;41;41m");
  });

  it("does not reconstruct scrollback from live terminal update frames", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.write?.mock) as {
      write: ReturnType<typeof vi.fn>;
    };
    const firstFrame = "\u001b[?2026h\u001b[H  60\u001b[K\r\n  61\u001b[K\r\n  62\u001b[K\u001b[?2026l";
    const secondFrame = "\u001b[?2026h\u001b[H  61\u001b[K\r\n  62\u001b[K\r\n  63\u001b[K\u001b[?2026l";

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa(firstFrame),
        }),
      });
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "update",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          state_base64: btoa(secondFrame),
        }),
      });
    });

    expect(terminalInstance.write.mock.calls[1]?.[0]).toBe(secondFrame);
    expect(terminalInstance.write.mock.calls[1]?.[0]).not.toContain("\u001b[999;1H");
  });

  it("strips provider cursor-shape sequences from remote terminal writes", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.write?.mock) as {
      write: ReturnType<typeof vi.fn>;
    };

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("\u001b[0 qprompt\u001b[1 q"),
        }),
      });
    });

    const written = String(terminalInstance.write.mock.calls[0]?.[0] ?? "");
    expect(written).toContain("prompt");
    expect(written).not.toContain("\u001b[0 q");
    expect(written).not.toContain("\u001b[1 q");
  });

  it("strips Codex terminal color-report replies before forwarding remote terminal input", async () => {
    mockRemoteAgentDetailFetch("codex");

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalSocket = MockWebSocket.instances[1];
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.onData?.mock) as {
      onData: ReturnType<typeof vi.fn>;
    };
    const onData = terminalInstance.onData.mock.calls[0]?.[0] as ((data: string) => void) | undefined;
    const colorReply = "\u001b]11;rgb:fc/fa/f5\u001b\\";

    act(() => {
      terminalSocket?.emit("open");
      terminalSocket?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("ready"),
        }),
      });
    });
    await waitFor(() => expect(terminalSocket?.sent[0]).toContain("ws-ticket-1"));

    const sentBeforeColorReply = terminalSocket?.sent.length ?? 0;
    act(() => {
      onData?.(colorReply);
      onData?.(`${colorReply}ls -la\r`);
    });

    const sentAfterColorReply = terminalSocket?.sent.slice(sentBeforeColorReply).map((payload) => JSON.parse(payload));
    expect(sentAfterColorReply).toEqual([{ type: "input", data: "ls -la\r" }]);
  });

  it("normalizes OpenCode synchronized-output noise from remote terminal live updates", async () => {
    mockRemoteAgentDetailFetch("opencode");

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.write?.mock) as {
      write: ReturnType<typeof vi.fn>;
    };

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("ready"),
        }),
      });
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "update",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          state_base64: btoa("\u001b[?2026hhello\u001b[?1016$ptest\u001b[?2026l"),
        }),
      });
    });

    expect(terminalInstance.write).toHaveBeenLastCalledWith("hellotest");
  });

  it("answers OpenCode terminal capability probes on the remote terminal stream", async () => {
    mockRemoteAgentDetailFetch("opencode");

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalSocket = MockWebSocket.instances[1];

    act(() => {
      terminalSocket?.emit("open");
      terminalSocket?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("\u001b[6n\u001b[14t"),
        }),
      });
    });
    await waitFor(() => expect(terminalSocket?.sent[0]).toContain("ws-ticket-1"));

    const inputFrames = terminalSocket?.sent.map((payload) => JSON.parse(payload)).filter((frame) => frame.type === "input");
    expect(inputFrames).toEqual([
      { type: "input", data: "\u001b[1;1R" },
      { type: "input", data: "\u001b[4;1;1t" },
    ]);
  });

  it("does not send terminal input or detach while the attach socket is not open", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    const { unmount } = render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalSocket = MockWebSocket.instances[1];
    expect(terminalSocket?.readyState).toBe(MockWebSocket.CONNECTING);
    const sendSpy = vi.spyOn(terminalSocket!, "send").mockImplementation(() => {
      throw new Error("socket is not open");
    });
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.onData?.mock && value?.onBinary?.mock) as {
      onData: ReturnType<typeof vi.fn>;
      onBinary: ReturnType<typeof vi.fn>;
      options: { disableStdin?: boolean };
    };
    const onData = terminalInstance.onData.mock.calls[0]?.[0] as ((data: string) => void) | undefined;
    const onBinary = terminalInstance.onBinary.mock.calls[0]?.[0] as ((data: string) => void) | undefined;

    expect(terminalInstance.options.disableStdin).toBe(true);
    expect(() => onData?.("a")).not.toThrow();
    expect(() => onBinary?.("b")).not.toThrow();
    expect(() => unmount()).not.toThrow();
    expect(sendSpy).not.toHaveBeenCalled();
  });

  it("enables remote terminal stdin only while the attach stream is accepted", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.options) as { options: { disableStdin?: boolean } };
    expect(terminalInstance.options.disableStdin).toBe(true);

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("ready"),
        }),
      });
    });
    expect(terminalInstance.options.disableStdin).toBe(false);

    act(() => {
      MockWebSocket.instances[1]?.emit("close");
    });
    expect(terminalInstance.options.disableStdin).toBe(true);
  });

  it("disables remote terminal stdin when another attachment becomes owner", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.options) as { options: { disableStdin?: boolean } };

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("ready"),
        }),
      });
    });
    expect(terminalInstance.options.disableStdin).toBe(false);

    act(() => {
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "ownership",
          owner_attachment_id: "attach-2",
          cols: 80,
          rows: 24,
        }),
      });
    });
    expect(terminalInstance.options.disableStdin).toBe(true);
  });

  it("streams live terminal UTF-8 across split update frames", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const terminalInstance = [...vi.mocked(Terminal).mock.results]
      .reverse()
      .map((result) => result.value)
      .find((value) => value?.write?.mock) as { write: ReturnType<typeof vi.fn> };

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("ready"),
        }),
      });
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "update",
          attachment_id: null,
          owner_attachment_id: "attach-1",
          state_base64: bytesToBase64([0xe2]),
        }),
      });
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "update",
          attachment_id: null,
          owner_attachment_id: "attach-1",
          state_base64: bytesToBase64([0x82, 0xac]),
        }),
      });
    });

    expect(terminalInstance.write).toHaveBeenCalledTimes(2);
    expect(terminalInstance.write).toHaveBeenLastCalledWith("\u20ac");
  });

  it("adds streamed remote terminal output to the mobile queue when the agent returns idle", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "opencode",
                  workspace: "<absolute-workspace-path>",
                  status: "Processing...",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));

    act(() => {
      MockWebSocket.instances[1]?.emit("open");
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "update",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          state_base64: btoa("Finished the requested update."),
        }),
      });
      MockWebSocket.instances[0]?.emit("message", {
        data: JSON.stringify({
          type: "agent_status",
          agents: [
            {
              session_id: "agent-1",
              session_name: "Coder",
              agent_class: "Coder",
              provider: "opencode",
              workspace: "<absolute-workspace-path>",
              status: "Idle",
              latest_text: "Ready",
            },
          ],
        }),
      });
    });

    await userEvent.click(screen.getByRole("button", { name: "Back to remote agents" }));
    await userEvent.click(screen.getByRole("button", { name: "Queue" }));

    expect(screen.getByText("Agent task completed")).toBeVisible();
    expect(screen.getByText("Coder")).toBeVisible();
    expect(screen.getByText("Finished the requested update.")).toBeVisible();
  });

  it("does not poll terminal snapshots when the status stream updates the attached terminal", async () => {
    let terminalCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/terminal") {
        terminalCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              snapshot: {
                cursor: `agent-1:${terminalCalls}`,
                text: terminalCalls === 1 ? "First terminal chunk." : "Second terminal chunk.",
                truncated: false,
                omitted_bytes: 0,
              },
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    expect(terminalCalls).toBe(0);

    act(() => {
      MockWebSocket.instances[0]?.emit("message", {
        data: JSON.stringify({
          type: "agent_status",
          agents: [
            {
              session_id: "agent-1",
              session_name: "Coder",
              agent_class: "Coder",
              provider: "codex",
              workspace: "<absolute-workspace-path>",
              status: "Processing...",
              latest_text: null,
            },
          ],
        }),
      });
    });

    await waitFor(() => expect(screen.getByText("Processing...")).toBeVisible());
    expect(terminalCalls).toBe(0);
  });

  it("ignores bursty status updates for terminal snapshot polling while attached", async () => {
    let terminalCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/terminal") {
        terminalCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              snapshot: {
                cursor: `agent-1:${terminalCalls}`,
                text: "Stable terminal.",
                truncated: false,
                omitted_bytes: 0,
              },
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    const scrollCallsAfterOpen = scrollIntoViewMock.mock.calls.length;

    act(() => {
      for (const status of ["Processing...", "Processing...", "Idle"]) {
        MockWebSocket.instances[0]?.emit("message", {
          data: JSON.stringify({
            type: "agent_status",
            agents: [
              {
                session_id: "agent-1",
                session_name: "Coder",
                agent_class: "Coder",
                provider: "codex",
                workspace: "<absolute-workspace-path>",
                status,
                latest_text: null,
              },
            ],
          }),
        });
      }
    });

    await waitFor(() => expect(screen.getByText("Idle")).toBeVisible());
    expect(terminalCalls).toBe(0);
    expect(scrollIntoViewMock).toHaveBeenCalledTimes(scrollCallsAfterOpen);
  });

  it("does not refresh terminal snapshots for unchanged active-agent status frames", async () => {
    let terminalCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/terminal") {
        terminalCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              snapshot: {
                cursor: `agent-1:${terminalCalls}`,
                text: terminalCalls === 1 ? "Stable terminal." : "Updated terminal.",
                truncated: false,
                omitted_bytes: 0,
              },
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));

    act(() => {
      MockWebSocket.instances[0]?.emit("message", {
        data: JSON.stringify({
          type: "agent_status",
          agents: [
            {
              session_id: "agent-1",
              session_name: "Coder",
              agent_class: "Coder",
              provider: "codex",
              workspace: "<absolute-workspace-path>",
              status: "Idle",
              latest_text: "Ready",
            },
          ],
        }),
      });
    });

    await waitFor(() => expect(screen.getByText("Idle")).toBeVisible());
    expect(terminalCalls).toBe(0);
  });

  it("keeps terminal rendering on the terminal stream instead of snapshot refresh ordering", async () => {
    let terminalCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/terminal") {
        terminalCalls += 1;
        return Promise.resolve(new Response("{}", { status: 500 }));
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    act(() => {
      MockWebSocket.instances[1]?.emit("open");
    });
    act(() => {
      MockWebSocket.instances[1]?.emit("message", {
        data: JSON.stringify({
          type: "snapshot",
          attachment_id: "attach-1",
          owner_attachment_id: "attach-1",
          cols: 80,
          rows: 24,
          state_base64: btoa("new terminal"),
        }),
      });
    });

    await act(async () => {
      void useRemoteStore.getState().refreshActiveAgentTerminal();
    });

    expect(terminalCalls).toBe(0);
    expect(MockWebSocket.instances[1]?.sent[0]).toContain("ws-ticket-1");
  });

  it("keeps terminal attach failures local to the active agent detail view", async () => {
    let wsTicketCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents/agent-1/terminal") {
        return Promise.resolve(new Response(JSON.stringify({ ok: false, code: "agent_terminal_failed" }), { status: 400 }));
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        wsTicketCalls += 1;
        if (wsTicketCalls === 2) {
          return Promise.resolve(new Response(JSON.stringify({ ok: false, code: "terminal_attach_failed" }), { status: 400 }));
        }
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));

    expect(await screen.findByTestId("remote-agent-detail")).toBeVisible();
    expect(await screen.findByText("Remote request failed: 400")).toBeVisible();
    expect(wsTicketCalls).toBe(2);
    expect(screen.queryByText("Desktop unreachable.")).not.toBeInTheDocument();
  });

  it("shows lifecycle actions in agent detail while keeping watchlist rows action-free", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Running Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Processing...",
                  latest_text: null,
                },
                {
                  session_id: "agent-2",
                  session_name: "Offline Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Off",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      if (url === "/remote/api/agents/agent-1/chat" || url === "/remote/api/agents/agent-2/chat") {
        return Promise.resolve(new Response(JSON.stringify({ events: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    const runningRow = await screen.findByRole("button", { name: /Open Running Coder details/i });
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Clear" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Kill" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Clone" })).not.toBeInTheDocument();
    await userEvent.click(runningRow);
    expect(await screen.findByTestId("remote-agent-detail")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Clone" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Back to remote agents/i }));

    const offlineRow = screen.getByRole("button", { name: /Open Offline Coder details/i });
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    await userEvent.click(offlineRow);
    expect(await screen.findByTestId("remote-agent-detail")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Clone" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Resume" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
  });

  it("does not expose selected-agent clone in the mobile roster", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    expect(await screen.findByTestId("remote-agent-detail")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Clone" })).not.toBeInTheDocument();
  });

  it("runs detail lifecycle actions without reloading the roster", async () => {
    let agentListCalls = 0;
    let chatCalls = 0;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        agentListCalls += 1;
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Processing...",
                  latest_text: null,
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      if (url === "/remote/api/agents/agent-1/chat") {
        chatCalls += 1;
        return Promise.resolve(new Response(JSON.stringify({ events: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    const row = await screen.findByRole("button", { name: /Open Coder details/i });
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    await userEvent.click(row);
    await userEvent.click(await screen.findByRole("button", { name: "Pause" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/action",
        expect.objectContaining({ method: "POST" }),
      );
    });
    expect(JSON.parse(fetchMock.mock.calls.find(([url]) => url === "/remote/api/agents/action")?.[1]?.body as string)).toEqual({
      action: "pause",
      target: "agent-1",
    });
    expect(screen.getByTestId("remote-agent-detail")).toBeVisible();
    expect(agentListCalls).toBe(1);
    expect(chatCalls).toBe(0);
  });

  it("reconnects the status stream after a transient close", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: `ws-ticket-${MockWebSocket.instances.length + 1}`, expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await screen.findByText("Coder");
    expect(MockWebSocket.instances).toHaveLength(1);

    act(() => {
      MockWebSocket.instances[0]?.emit("close");
    });

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
  });

  it("shows a re-authentication action when the remote session expires", async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(new Response("{}", { status: 401 }));
      }
      return Promise.resolve(new Response("{}", { status: 500 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Session expired. Re-authentication is required.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Re-authenticate" })).toBeVisible();
  });

  it("clears the stored phone identity and asks for a fresh QR code when the desktop fingerprint changes", async () => {
    vi.mocked(loadStoredRemoteIdentity).mockResolvedValue({
      device_id: "dev-1",
      public_key_fingerprint: "phone-fp",
      server_identity_fingerprint: "desktop-fp",
      origin: "https://wardian.tailnet.ts.net",
      private_key: { type: "private" } as CryptoKey,
      paired_at: "2026-05-21T08:00:30.000Z",
    });
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(new Response("{}", { status: 401 }));
      }
      if (url === "/remote/api/auth/challenge" && init?.method === "POST") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              challenge_id: "challenge-1",
              device_id: "dev-1",
              origin: "https://wardian.tailnet.ts.net",
              server_identity_fingerprint: "changed-desktop-fp",
              nonce: "auth-nonce",
              expires_at: "2026-05-21T08:02:00.000Z",
              audience: "wardian_remote_pwa",
            }),
            { status: 200 },
          ),
        );
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    expect(await screen.findByText("Gateway identity changed. Scan a fresh QR code to re-pair.")).toBeVisible();
    expect(screen.queryByText("This device has been revoked.")).not.toBeInTheDocument();
    expect(clearStoredRemoteIdentity).toHaveBeenCalled();
    expect(signRemoteAuthChallenge).not.toHaveBeenCalled();
    expect(fetchMock.mock.calls.some(([url]) => url === "/remote/api/auth/session")).toBe(false);
  });

  it("requires confirmation before killing an agent", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(false);
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              csrf_nonce: "csrf-1",
              expires_at: "2026-05-21T08:05:00.000Z",
              absolute_expires_at: "2026-05-21T20:00:00.000Z",
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/agents") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              agents: [
                {
                  session_id: "agent-1",
                  session_name: "Coder",
                  agent_class: "Coder",
                  provider: "codex",
                  workspace: "<absolute-workspace-path>",
                  status: "Idle",
                  latest_text: "Ready",
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (url === "/remote/api/workflows") {
        return Promise.resolve(new Response(JSON.stringify({ workflows: [] }), { status: 200 }));
      }
      if (url === "/remote/api/ws-ticket" && init?.method === "POST") {
        return Promise.resolve(
          new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: "2026-05-21T08:01:00.000Z" }), {
            status: 200,
          }),
        );
      }
      if (url === "/remote/api/agents/action" && init?.method === "POST") {
        return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await screen.findByText("Coder");
    await userEvent.click(screen.getByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(screen.getByRole("button", { name: "Kill" }));

    expect(window.confirm).toHaveBeenCalledWith("Kill Coder?");
    expect(fetchMock.mock.calls.some(([url]) => url === "/remote/api/agents/action")).toBe(false);
  });
});
