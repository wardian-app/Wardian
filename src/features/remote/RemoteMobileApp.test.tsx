import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearStoredRemoteIdentity,
  createRemoteDeviceKeyPair,
  defaultRemoteDeviceLabel,
  loadStoredRemoteIdentity,
  saveStoredRemoteIdentity,
  signRemoteAuthChallenge,
} from "./remoteIdentity";
import { remoteClient } from "./remoteClient";
import { RemoteMobileApp } from "./RemoteMobileApp";
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

describe("RemoteMobileApp", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(Terminal).mockImplementation((options) => ({
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
    }) as unknown as Terminal);
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
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    remoteClient.setCsrfNonce(null);
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

  it("bootstraps the session nonce, shows the roster, and opens the status stream", async () => {
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
    expect(screen.getByTestId("remote-scroll-region")).toHaveClass("min-h-0", "overflow-y-auto");
    expect(screen.getByTestId("remote-agent-list")).toHaveClass("grid-cols-1");
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

    await userEvent.type(screen.getByLabelText("Prompt"), "status please");
    expect(screen.getByRole("button", { name: "Broadcast" })).toBeEnabled();
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
    expect(screen.getByText("No workflows available.")).toBeVisible();
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
      if (url.startsWith("/remote/api/agents/agent-1/chat")) {
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
    expect(screen.getByTestId("remote-terminal-attach")).toBeVisible();
    expect(screen.getByTestId("remote-agent-detail")).toHaveClass("h-dvh", "overflow-hidden");
    expect(screen.getByRole("region", { name: "Coder terminal" })).toHaveClass("flex", "min-h-0", "flex-col", "overflow-hidden");
    expect(screen.getByTestId("remote-terminal-scroll-surface")).toHaveClass("min-h-0", "flex-1", "overflow-hidden");
    expect(screen.getByTestId("remote-terminal-scroll-surface")).not.toHaveClass("h-full");
    expect(screen.getByTestId("remote-terminal-attach")).not.toHaveClass("overflow-hidden");
    expect(screen.getByTestId("remote-terminal-attach")).not.toHaveClass("min-h-[280px]");
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
    expect(chatCalls).toBe(2);
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

  it("loads remote chat with a small tail first and automatically backfills retained history", async () => {
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url === "/remote/api/session") {
        return Promise.resolve(
          new Response(JSON.stringify({ csrf_nonce: "csrf-1", expires_at: null, absolute_expires_at: null }), { status: 200 }),
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
      if (url.startsWith("/remote/api/agents/agent-1/chat")) {
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
                  text: "Recent remote answer",
                  title: null,
                  status: null,
                  turn_id: "turn-1",
                  source: "provider_log",
                  command: null,
                  exit_code: null,
                  path: null,
                  language: null,
                  created_at: null,
                  sequence: 1,
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
        return Promise.resolve(new Response(JSON.stringify({ ticket: "ws-ticket-1", expires_at: null }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder details/i }));
    await userEvent.click(screen.getByRole("button", { name: "Chat" }));
    expect(await screen.findByText("Recent remote answer")).toBeVisible();

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/agent-1/chat?tail_bytes=131072",
        expect.objectContaining({ method: "GET" }),
      );
    });
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/agent-1/chat?tail_bytes=2097152",
        expect.objectContaining({ method: "GET" }),
      );
    });
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

  it("normalizes flattened Codex repaint history before writing to the remote terminal", async () => {
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

    expect(terminalInstance.write.mock.calls[0]?.[0]).toContain("\u001b[999;1H  60\r\n");
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

  it("shows lifecycle actions in agent detail while keeping roster cards action-free", async () => {
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
      if (url.startsWith("/remote/api/agents/agent-1/chat") || url.startsWith("/remote/api/agents/agent-2/chat")) {
        return Promise.resolve(new Response(JSON.stringify({ events: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    const runningCard = await screen.findByRole("article", { name: /Running Coder/i });
    expect(within(runningCard).queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(within(runningCard).queryByRole("button", { name: "Clear" })).not.toBeInTheDocument();
    expect(within(runningCard).queryByRole("button", { name: "Kill" })).not.toBeInTheDocument();
    expect(within(runningCard).queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
    await userEvent.click(within(runningCard).getByRole("button", { name: /Open Running Coder details/i }));
    expect(await screen.findByTestId("remote-agent-detail")).toBeVisible();
    expect(screen.getByRole("button", { name: "Pause" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Back to remote agents/i }));

    const offlineCard = screen.getByRole("article", { name: /Offline Coder/i });
    expect(within(offlineCard).queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
    expect(within(offlineCard).queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    await userEvent.click(within(offlineCard).getByRole("button", { name: /Open Offline Coder details/i }));
    expect(await screen.findByTestId("remote-agent-detail")).toBeVisible();
    expect(screen.getByRole("button", { name: "Resume" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
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
      if (url.startsWith("/remote/api/agents/agent-1/chat")) {
        chatCalls += 1;
        return Promise.resolve(new Response(JSON.stringify({ events: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 404 }));
    });

    render(<RemoteMobileApp />);

    const card = await screen.findByRole("article", { name: /Coder/i });
    expect(within(card).queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    await userEvent.click(within(card).getByRole("button", { name: /Open Coder details/i }));
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
