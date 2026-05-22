import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
  static instances: MockWebSocket[] = [];

  sent: string[] = [];
  listeners: Record<string, Array<(event: any) => void>> = {};

  constructor(readonly url: string) {
    MockWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: any) => void) {
    this.listeners[type] = [...(this.listeners[type] ?? []), listener];
  }

  send(payload: string) {
    this.sent.push(payload);
  }

  close() {}

  emit(type: string, event: any = {}) {
    for (const listener of this.listeners[type] ?? []) listener(event);
  }
}

describe("RemoteMobileApp", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    scrollIntoViewMock = vi.fn();
    Object.defineProperty(Element.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoViewMock,
    });
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
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

  it("opens a selected agent conversation, loads chat events, and sends through the chat composer", async () => {
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

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder conversation/i }));

    expect(await screen.findByText("I can see the selected conversation.")).toBeVisible();
    expect(screen.getByTestId("remote-agent-conversation")).toHaveClass("h-dvh", "overflow-hidden");
    expect(screen.getByRole("region", { name: "Coder conversation" })).toHaveClass("min-h-0", "overflow-y-auto");
    expect(scrollIntoViewMock).toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText("Message Coder"), "what changed?");
    await userEvent.click(screen.getByRole("button", { name: "Send message" }));

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

  it("refreshes the selected conversation when the status stream updates that agent", async () => {
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
                  id: `agent-1:${chatCalls}`,
                  session_id: "agent-1",
                  provider: "codex",
                  kind: "message",
                  role: "assistant",
                  text: chatCalls === 1 ? "First transcript chunk." : "Second transcript chunk.",
                  title: null,
                  status: null,
                  turn_id: `turn-${chatCalls}`,
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

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder conversation/i }));
    expect(await screen.findByText("First transcript chunk.")).toBeVisible();

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

    expect(await screen.findByText("Second transcript chunk.")).toBeVisible();
    expect(chatCalls).toBeGreaterThanOrEqual(2);
  });

  it("coalesces bursty status updates into one selected conversation refresh", async () => {
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
                  text: "Stable transcript.",
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

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder conversation/i }));
    expect(await screen.findByText("Stable transcript.")).toBeVisible();
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

    await waitFor(() => expect(chatCalls).toBe(2));
    expect(scrollIntoViewMock).toHaveBeenCalledTimes(scrollCallsAfterOpen);
  });

  it("does not refresh the selected conversation for unchanged active-agent status frames", async () => {
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
                  text: "Stable transcript.",
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

    await userEvent.click(await screen.findByRole("button", { name: /Open Coder conversation/i }));
    expect(await screen.findByText("Stable transcript.")).toBeVisible();

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

    await new Promise((resolve) => window.setTimeout(resolve, 25));
    expect(chatCalls).toBe(1);
  });

  it("shows pause and resume as mutually exclusive lifecycle actions", async () => {
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

    const runningCard = await screen.findByRole("article", { name: /Running Coder/i });
    expect(within(runningCard).getByRole("button", { name: "Pause" })).toBeVisible();
    expect(within(runningCard).queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();

    const offlineCard = screen.getByRole("article", { name: /Offline Coder/i });
    expect(within(offlineCard).getByRole("button", { name: "Resume" })).toBeVisible();
    expect(within(offlineCard).queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
  });

  it("runs card lifecycle actions without opening the conversation or reloading the roster", async () => {
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

    const card = await screen.findByRole("article", { name: /Coder/i });
    await userEvent.click(within(card).getByRole("button", { name: "Pause" }));

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
    expect(screen.queryByTestId("remote-agent-conversation")).not.toBeInTheDocument();
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
    await userEvent.click(screen.getByRole("button", { name: "Kill" }));

    expect(window.confirm).toHaveBeenCalledWith("Kill Coder?");
    expect(fetchMock.mock.calls.some(([url]) => url === "/remote/api/agents/action")).toBe(false);
  });
});
