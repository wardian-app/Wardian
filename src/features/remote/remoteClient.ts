import type {
  AuthSessionResponse,
  RemoteAgentActionRequest,
  RemoteAgentSummary,
  RemoteWebSocketTicketResponse,
  RemoteWorkflowRunRequest,
  RemoteWorkflowStopRequest,
  RemoteWorkflowSummary,
} from "../../types";

const REMOTE_CSRF_HEADER_NAME = "x-wardian-csrf";

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
  async listAgents() {
    const result = await remoteJson<{ agents: RemoteAgentSummary[] }>("/remote/api/agents");
    return result.agents;
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
    return remoteJson<RemoteWebSocketTicketResponse>("/remote/api/ws-ticket", {
      method: "POST",
      body: JSON.stringify({ stream: "agent_status" }),
    });
  },
};
