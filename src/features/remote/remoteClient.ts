import type {
  AuthChallengeResponse,
  AuthSessionResponse,
  AgentChatEvent,
  PairingSubmitResponse,
  RemoteAgentActionRequest,
  RemoteAgentSummary,
  RemoteTerminalSnapshot,
  RemoteTerminalStreamMessage,
  RemoteWebSocketTicketResponse,
  RemoteWorkflowRunRequest,
  RemoteWorkflowStopRequest,
  RemoteWorkflowSummary,
} from "../../types";

const REMOTE_CSRF_HEADER_NAME = "x-wardian-csrf";
const REMOTE_STATUS_STREAM_PATH = "/remote/api/status-stream";

export class RemoteRequestError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "RemoteRequestError";
  }
}

let csrfNonce: string | null = null;

const normalizeHeaders = (headers?: HeadersInit): Record<string, string> => {
  if (!headers) return {};
  if (headers instanceof Headers) {
    return Object.fromEntries(headers.entries());
  }
  if (Array.isArray(headers)) {
    return Object.fromEntries(headers);
  }
  return { ...headers };
};

const isMutatingRequest = (method: string) => method !== "GET" && method !== "HEAD";

async function remoteJson<T>(path: string, init?: RequestInit): Promise<T> {
  const method = (init?.method ?? "GET").toUpperCase();
  const headers = {
    "Content-Type": "application/json",
    ...normalizeHeaders(init?.headers),
    ...(csrfNonce && isMutatingRequest(method) ? { [REMOTE_CSRF_HEADER_NAME]: csrfNonce } : {}),
  };
  const response = await fetch(path, {
    ...init,
    method,
    credentials: "same-origin",
    headers,
  });
  if (!response.ok) {
    throw new RemoteRequestError(`Remote request failed: ${response.status}`, response.status);
  }
  return response.json() as Promise<T>;
}

export const remoteClient = {
  setCsrfNonce(nextNonce: string | null) {
    csrfNonce = nextNonce?.trim() || null;
  },
  getCsrfNonce() {
    return csrfNonce;
  },
  async loadSession() {
    const session = await remoteJson<AuthSessionResponse>("/remote/api/session");
    this.setCsrfNonce(session.csrf_nonce);
    return session;
  },
  async submitPairing(request: {
    pairing_offer_id: string;
    nonce: string;
    device_label: string;
    public_key_spki_der_base64: string;
  }) {
    return remoteJson<PairingSubmitResponse>("/remote/api/pairing/submit", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },
  async pairingStatus(pairingRequestId: string) {
    return remoteJson<PairingSubmitResponse>(
      `/remote/api/pairing/${encodeURIComponent(pairingRequestId)}`,
    );
  },
  async createAuthChallenge(deviceId: string) {
    return remoteJson<AuthChallengeResponse>("/remote/api/auth/challenge", {
      method: "POST",
      body: JSON.stringify({ device_id: deviceId }),
    });
  },
  async createAuthSession(request: {
    challenge_id: string;
    device_id: string;
    signature_der_base64: string;
  }) {
    const session = await remoteJson<AuthSessionResponse>("/remote/api/auth/session", {
      method: "POST",
      body: JSON.stringify(request),
    });
    this.setCsrfNonce(session.csrf_nonce);
    return session;
  },
  async listAgents() {
    const result = await remoteJson<{ agents: RemoteAgentSummary[] }>("/remote/api/agents");
    return result.agents;
  },
  async loadAgentChat(sessionId: string) {
    const result = await remoteJson<{ events: AgentChatEvent[] }>(
      `/remote/api/agents/${encodeURIComponent(sessionId)}/chat`,
    );
    return result.events;
  },
  async loadAgentTerminal(sessionId: string) {
    const result = await remoteJson<{ snapshot: RemoteTerminalSnapshot }>(
      `/remote/api/agents/${encodeURIComponent(sessionId)}/terminal`,
    );
    return result.snapshot;
  },
  async sendPrompt(target: string, prompt: string) {
    const request: RemoteAgentActionRequest = { action: "send_prompt", target, prompt };
    await remoteJson<{ ok: true }>("/remote/api/agents/action", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },
  async runAgentAction(action: string, target: string) {
    const request: RemoteAgentActionRequest = { action, target };
    await remoteJson<{ ok: true }>("/remote/api/agents/action", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },
  async listWorkflows() {
    const result = await remoteJson<{ workflows: RemoteWorkflowSummary[] }>("/remote/api/workflows");
    return result.workflows;
  },
  async runWorkflow(workflow_id: string, payload?: unknown) {
    const request: RemoteWorkflowRunRequest = { workflow_id, payload };
    await remoteJson<{ ok: true }>("/remote/api/workflows/run", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },
  async stopWorkflow(run_instance_id: string) {
    const request: RemoteWorkflowStopRequest = { run_instance_id };
    await remoteJson<{ ok: true }>("/remote/api/workflows/stop", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },
  async createStatusStreamTicket() {
    return this.createWebSocketTicket("agent_status");
  },
  async createTerminalStreamTicket() {
    return this.createWebSocketTicket("terminal_attach");
  },
  async createWebSocketTicket(stream: "agent_status" | "terminal_attach") {
    return remoteJson<RemoteWebSocketTicketResponse>("/remote/api/ws-ticket", {
      method: "POST",
      body: JSON.stringify({ stream }),
    });
  },
  async openStatusStream(handlers: {
    onAgents: (agents: RemoteAgentSummary[]) => void;
    onSessionExpired: () => void;
    onError?: () => void;
    onClose?: () => void;
  }) {
    const ticket = await this.createStatusStreamTicket();
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${window.location.host}${REMOTE_STATUS_STREAM_PATH}`);

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ ticket: ticket.ticket }));
    });
    socket.addEventListener("message", (event) => {
      let data:
        | { type: "agent_status"; agents: RemoteAgentSummary[] }
        | { type: "error"; code: string };
      try {
        data = JSON.parse(String(event.data));
      } catch {
        handlers.onError?.();
        return;
      }
      if (data.type === "agent_status") {
        handlers.onAgents(data.agents);
        return;
      }
      if (data.code === "session_expired" || data.code === "invalid_websocket_ticket") {
        handlers.onSessionExpired();
        return;
      }
      handlers.onError?.();
    });
    socket.addEventListener("error", () => handlers.onError?.());
    socket.addEventListener("close", () => handlers.onClose?.());
    return socket;
  },
  async openTerminalStream(
    sessionId: string,
    cols: number,
    rows: number,
    handlers: {
      onMessage: (message: RemoteTerminalStreamMessage) => void;
      onSessionExpired?: () => void;
      onError?: (message: string) => void;
      onClose?: () => void;
    },
  ) {
    const ticket = await this.createTerminalStreamTicket();
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(
      `${protocol}//${window.location.host}/remote/api/agents/${encodeURIComponent(sessionId)}/terminal-stream`,
    );

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ ticket: ticket.ticket, cols, rows }));
    });
    socket.addEventListener("message", (event) => {
      let data: RemoteTerminalStreamMessage;
      try {
        data = JSON.parse(String(event.data)) as RemoteTerminalStreamMessage;
      } catch {
        handlers.onError?.("invalid_terminal_stream_message");
        return;
      }
      if (data.type === "error") {
        if (data.code === "session_expired" || data.code === "invalid_websocket_ticket") {
          handlers.onSessionExpired?.();
          return;
        }
        handlers.onError?.(data.code);
        return;
      }
      handlers.onMessage(data);
    });
    socket.addEventListener("error", () => handlers.onError?.("terminal_stream_error"));
    socket.addEventListener("close", () => handlers.onClose?.());
    return socket;
  },
};
