import type {
  AuthChallengeResponse,
  AuthSessionResponse,
  AgentChatEvent,
  AgentChatPage,
  ChatReceiptFields,
  QueueItem,
  PairingSubmitResponse,
  RemoteAgentActionRequest,
  RemoteAgentInputMode,
  RemoteAgentSummary,
  RemoteTerminalSnapshot,
  RemoteTerminalStreamMessage,
  RemoteWebSocketTicketResponse,
  RemoteWatchlistResponse,
  RemoteAutomationRunRequest,
  RemoteAutomationStopRequest,
  RemoteAutomationSummary,
  RemoteAutomationMonitorSnapshot,
} from "../../types";

export type RemoteAgentChatPage = AgentChatPage;

const REMOTE_CSRF_HEADER_NAME = "x-wardian-csrf";
const REMOTE_STATUS_STREAM_PATH = "/remote/api/status-stream";
const REMOTE_REQUEST_TIMEOUT_MS = 15_000;
const REMOTE_CHAT_READ_TIMEOUT_MS = 60_000;

/** A Chat read deadline does not establish that the desktop is unreachable. */
export class RemoteChatTimeoutError extends Error {
  constructor() {
    super("Chat history did not finish loading within 60 seconds. Retry to request it again; the desktop may still be loading it.");
    this.name = "RemoteChatTimeoutError";
  }
}

/** A failed successful response body leaves the desktop connection usable. */
export class RemoteChatBodyError extends Error {
  constructor() {
    super("Chat could not be loaded. Retry when the desktop is available.");
    this.name = "RemoteChatBodyError";
  }
}

export class RemoteRequestError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code?: string,
    readonly detail?: string,
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

const isReadOnlyRequest = (method: string) => method === "GET" || method === "HEAD";
const isMutatingRequest = (method: string) => !isReadOnlyRequest(method);

async function remoteJson<T>(path: string, init?: RequestInit, chatRead = false): Promise<T> {
  const method = (init?.method ?? "GET").toUpperCase();
  const headers = {
    "Content-Type": "application/json",
    ...normalizeHeaders(init?.headers),
    ...(csrfNonce && isMutatingRequest(method) ? { [REMOTE_CSRF_HEADER_NAME]: csrfNonce } : {}),
  };
  const controller = new AbortController();
  let timedOut = false;
  const timeout = isReadOnlyRequest(method)
    ? setTimeout(() => { timedOut = true; controller.abort(); }, chatRead ? REMOTE_CHAT_READ_TIMEOUT_MS : REMOTE_REQUEST_TIMEOUT_MS)
    : undefined;
  const requestSignal = init?.signal;
  const abortRequest = () => controller.abort(requestSignal?.reason);
  if (requestSignal) {
    if (requestSignal.aborted) {
      abortRequest();
    } else {
      requestSignal.addEventListener("abort", abortRequest, { once: true });
    }
  }

  try {
    const response = await fetch(path, {
      ...init,
      method,
      credentials: "same-origin",
      headers,
      signal: controller.signal,
    });
    if (!response.ok) {
      let code: string | undefined;
      let detail: string | undefined;
      try {
        const body = (await response.json()) as { code?: unknown; detail?: unknown };
        if (typeof body.code === "string") code = body.code;
        if (typeof body.detail === "string" && body.detail.trim()) detail = body.detail;
      } catch {
        // An error body is optional; the status code alone still surfaces.
      }
      throw new RemoteRequestError(
        detail ? `Remote request failed: ${detail}` : `Remote request failed: ${response.status}`,
        response.status,
        code,
        detail,
      );
    }
    try {
      return await response.json() as T;
    } catch (error) {
      // Headers already succeeded; preserve deadline and caller cancellation.
      if (chatRead && !timedOut && !requestSignal?.aborted) throw new RemoteChatBodyError();
      throw error;
    }
  } catch (error) {
    // A received HTTP status survives failure to parse its optional body.
    if (chatRead && timedOut && !(error instanceof RemoteRequestError)) throw new RemoteChatTimeoutError();
    throw error;
  } finally {
    if (timeout !== undefined) clearTimeout(timeout);
    requestSignal?.removeEventListener("abort", abortRequest);
  }
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
  async loadQueueItems() {
    const result = await remoteJson<{ items: QueueItem[] }>("/remote/api/queue");
    return result.items;
  },
  async runInboxAction(action: string, itemId?: string, choice?: string) {
    await remoteJson<{ ok: true }>("/remote/api/queue/action", {
      method: "POST",
      body: JSON.stringify({ action, item_id: itemId, choice }),
    });
  },
  async loadAgentChat(sessionId: string, signal?: AbortSignal) {
    const result = await remoteJson<{ events: AgentChatEvent[] }>(
      `/remote/api/agents/${encodeURIComponent(sessionId)}/chat`,
      { signal }, true,
    );
    return result.events;
  },
  async loadAgentChatPage(sessionId: string, before?: string, revision?: string, detail?: string, signal?: AbortSignal): Promise<RemoteAgentChatPage> {
    const query = new URLSearchParams();
    if (before) query.set("before", before);
    if (revision) query.set("revision", revision);
    if (detail) query.set("detail", detail);
    return remoteJson<RemoteAgentChatPage>(`/remote/api/agents/${encodeURIComponent(sessionId)}/chat${query.size ? `?${query}` : ""}`, { signal }, true);
  },
  async loadAgentTerminal(sessionId: string) {
    const result = await remoteJson<{ snapshot: RemoteTerminalSnapshot }>(
      `/remote/api/agents/${encodeURIComponent(sessionId)}/terminal`,
    );
    return result.snapshot;
  },
  async sendPrompt(target: string, prompt: string, inputMode: RemoteAgentInputMode = "message", inboxItemId?: string) {
    const request: RemoteAgentActionRequest =
      inputMode === "command"
        ? { action: "send_prompt", target, prompt, input_mode: "command", ...(inboxItemId ? { inbox_item_id: inboxItemId } : {}) }
        : { action: "send_prompt", target, prompt, ...(inboxItemId ? { inbox_item_id: inboxItemId } : {}) };
    return remoteJson<{ ok: true } & ChatReceiptFields>("/remote/api/agents/action", {
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
  async listAutomations() {
    const result = await remoteJson<{ automations: RemoteAutomationSummary[] }>("/remote/api/automations");
    return result.automations;
  },
  async loadAutomationMonitor(offsets: {
    active_offset?: number;
    recent_offset?: number;
    schedule_offset?: number;
  } = {}) {
    const search = new URLSearchParams();
    for (const [key, value] of Object.entries(offsets)) {
      if (typeof value === "number" && Number.isInteger(value) && value >= 0) {
        search.set(key, String(value));
      }
    }
    const suffix = search.size > 0 ? `?${search.toString()}` : "";
    return remoteJson<RemoteAutomationMonitorSnapshot>(`/remote/api/automations/monitor${suffix}`);
  },
  async loadWatchlists() {
    return remoteJson<RemoteWatchlistResponse>("/remote/api/watchlists");
  },
  async runAutomation(automation_id: string, payload?: unknown) {
    const request: RemoteAutomationRunRequest = { automation_id, payload };
    await remoteJson<{ ok: true }>("/remote/api/automations/run", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },
  async stopAutomation(run_instance_id: string) {
    const request: RemoteAutomationStopRequest = { run_instance_id };
    await remoteJson<{ ok: true }>("/remote/api/automations/stop", {
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
      onOpen?: () => void;
      onSocket?: (socket: WebSocket) => void;
    },
  ) {
    const ticket = await this.createTerminalStreamTicket();
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(
      `${protocol}//${window.location.host}/remote/api/agents/${encodeURIComponent(sessionId)}/terminal-stream`,
    );
    handlers.onSocket?.(socket);

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ protocol_version: 2, ticket: ticket.ticket, cols, rows }));
      handlers.onOpen?.();
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
        if (data.fatal) {
          handlers.onError?.(data.code);
          return;
        }
      }
      handlers.onMessage(data);
    });
    socket.addEventListener("error", () => handlers.onError?.("terminal_stream_error"));
    socket.addEventListener("close", () => handlers.onClose?.());
    return socket;
  },
};
