import { render, screen, waitFor } from "@testing-library/react";
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
      selectedAgentIds: new Set(),
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

  it("bootstraps the session nonce, requires an explicit target, and sends a CSRF-protected prompt", async () => {
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
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: /Coder/ }));
    await userEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/remote/api/agents/action",
        expect.objectContaining({
          credentials: "same-origin",
          method: "POST",
          headers: expect.objectContaining({
            "Content-Type": "application/json",
            "x-wardian-csrf": "csrf-1",
          }),
        }),
      );
    });

    const actionCall = fetchMock.mock.calls.find(([url]) => url === "/remote/api/agents/action");
    expect(JSON.parse(actionCall?.[1]?.body as string)).toEqual({
      action: "send_prompt",
      target: "agent-1",
      prompt: "status please",
    });
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
