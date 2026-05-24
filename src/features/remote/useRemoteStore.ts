import { create } from "zustand";
import type { AgentChatEvent, RemoteAgentSummary, RemoteTerminalSnapshot, RemoteWorkflowSummary } from "../../types";
import {
  clearStoredRemoteIdentity,
  createRemoteDeviceKeyPair,
  defaultRemoteDeviceLabel,
  loadStoredRemoteIdentity,
  saveStoredRemoteIdentity,
  signRemoteAuthChallenge,
  type StoredRemoteDeviceIdentity,
} from "./remoteIdentity";
import { remoteClient, RemoteRequestError } from "./remoteClient";

type RemoteStatus =
  | "loading"
  | "ready"
  | "unreachable"
  | "session_expired"
  | "pairing_pending"
  | "pairing_expired"
  | "gateway_identity_changed"
  | "device_revoked";

interface RemoteState {
  agents: RemoteAgentSummary[];
  workflows: RemoteWorkflowSummary[];
  status: RemoteStatus;
  activeAgentId: string | null;
  activeAgentViewMode: "terminal" | "chat";
  terminalSnapshot: RemoteTerminalSnapshot | null;
  terminalLoading: boolean;
  terminalError: string;
  chatEvents: AgentChatEvent[];
  chatLoading: boolean;
  chatError: string;
  sending: boolean;
  load: () => Promise<void>;
  disconnectStatusStream: () => void;
  openAgent: (id: string) => Promise<void>;
  closeAgent: () => void;
  setActiveAgentViewMode: (mode: "terminal" | "chat") => Promise<void>;
  refreshActiveAgentTerminal: (options?: { background?: boolean }) => Promise<void>;
  refreshActiveAgentChat: (options?: { background?: boolean }) => Promise<void>;
  sendPromptToActiveAgent: (prompt: string) => Promise<void>;
  broadcastPrompt: (prompt: string) => Promise<void>;
  runAgentAction: (action: string, target: string) => Promise<void>;
  runWorkflow: (workflowId: string) => Promise<void>;
}

type RemoteSet = (
  partial: Partial<RemoteState> | ((state: RemoteState) => Partial<RemoteState>),
) => void;
type RemoteGet = () => RemoteState;

const statusFromError = (error: unknown): RemoteStatus =>
  error instanceof RemoteRequestError && error.status === 401 ? "session_expired" : "unreachable";

const BACKGROUND_CHAT_REFRESH_MIN_INTERVAL_MS = 750;
const STATUS_STREAM_RECONNECT_BASE_DELAY_MS = 250;
const STATUS_STREAM_RECONNECT_MAX_DELAY_MS = 5_000;

const chatEventFingerprint = (event: AgentChatEvent) =>
  [
    event.id,
    event.kind,
    event.role ?? "",
    event.text ?? "",
    event.title ?? "",
    event.status ?? "",
    event.command ?? "",
    event.exit_code ?? "",
    event.path ?? "",
    event.language ?? "",
    event.sequence ?? "",
  ].join("\u0001");

const chatEventsEqual = (left: AgentChatEvent[], right: AgentChatEvent[]) => {
  if (left.length !== right.length) return false;
  return left.every((event, index) => chatEventFingerprint(event) === chatEventFingerprint(right[index]));
};

class RemotePairingExpiredError extends Error {}
type RemotePairingRejectedReason = "pairing_rejected" | "server_identity_mismatch";

class RemotePairingRejectedError extends Error {
  constructor(readonly reason: RemotePairingRejectedReason) {
    super(reason);
  }
}

const sendPromptToTargets = async (prompt: string, agentIds: string[]) => {
  const results = await Promise.allSettled(agentIds.map((target) => remoteClient.sendPrompt(target, prompt)));
  const rejected = results.find((result): result is PromiseRejectedResult => result.status === "rejected");
  if (!rejected) return;
  if (rejected.reason instanceof RemoteRequestError && rejected.reason.status === 401) {
    throw rejected.reason;
  }
  const acceptedCount = results.filter((result) => result.status === "fulfilled").length;
  throw new Error(`${acceptedCount}/${agentIds.length} prompts accepted.`);
};

let statusStreamSocket: WebSocket | null = null;
let backgroundChatRefreshTimer: number | null = null;
let backgroundChatRefreshInFlight = false;
let backgroundChatRefreshQueued = false;
let lastBackgroundChatRefreshStartedAt = 0;
let statusStreamReconnectTimer: number | null = null;
let statusStreamReconnectAttempts = 0;
let lastActiveAgentRefreshKey: string | null = null;
let terminalRefreshRequestSerial = 0;
let suppressNextStatusStreamReconnect = false;

const clearBackgroundChatRefresh = () => {
  if (backgroundChatRefreshTimer !== null) {
    window.clearTimeout(backgroundChatRefreshTimer);
    backgroundChatRefreshTimer = null;
  }
  backgroundChatRefreshQueued = false;
};

const clearStatusStreamReconnect = () => {
  if (statusStreamReconnectTimer !== null) {
    window.clearTimeout(statusStreamReconnectTimer);
    statusStreamReconnectTimer = null;
  }
};

const closeStatusStream = () => {
  if (statusStreamSocket) {
    suppressNextStatusStreamReconnect = true;
    statusStreamSocket.close();
  }
  statusStreamSocket = null;
  clearStatusStreamReconnect();
  clearBackgroundChatRefresh();
};

const runBackgroundActiveChatRefresh = async (set: RemoteSet, get: RemoteGet) => {
  if (backgroundChatRefreshInFlight) {
    backgroundChatRefreshQueued = true;
    return;
  }
  if (!get().activeAgentId) return;
  backgroundChatRefreshInFlight = true;
  lastBackgroundChatRefreshStartedAt = Date.now();
  try {
    if (get().activeAgentViewMode === "chat") {
      await get().refreshActiveAgentChat({ background: true });
    } else {
      await get().refreshActiveAgentTerminal({ background: true });
    }
  } finally {
    backgroundChatRefreshInFlight = false;
    if (backgroundChatRefreshQueued) {
      backgroundChatRefreshQueued = false;
      scheduleBackgroundActiveChatRefresh(set, get);
    }
  }
};

const scheduleBackgroundActiveChatRefresh = (set: RemoteSet, get: RemoteGet) => {
  if (!get().activeAgentId || backgroundChatRefreshTimer !== null) return;
  const elapsed = Date.now() - lastBackgroundChatRefreshStartedAt;
  const delay = Math.max(0, BACKGROUND_CHAT_REFRESH_MIN_INTERVAL_MS - elapsed);
  backgroundChatRefreshTimer = window.setTimeout(() => {
    backgroundChatRefreshTimer = null;
    void runBackgroundActiveChatRefresh(set, get);
  }, delay);
};

const activeAgentRefreshKey = (agent: RemoteAgentSummary) =>
  [agent.session_id, agent.status, agent.latest_text ?? ""].join("\0");

const scheduleStatusStreamReconnect = (set: RemoteSet, get: RemoteGet) => {
  if (statusStreamReconnectTimer !== null || statusStreamSocket || get().status === "session_expired") return;
  const delay = Math.min(
    STATUS_STREAM_RECONNECT_MAX_DELAY_MS,
    STATUS_STREAM_RECONNECT_BASE_DELAY_MS * 2 ** statusStreamReconnectAttempts,
  );
  statusStreamReconnectAttempts += 1;
  statusStreamReconnectTimer = window.setTimeout(() => {
    statusStreamReconnectTimer = null;
    void ensureStatusStream(set, get).catch((error) => {
      handleStatusStreamOpenFailure(set, error);
      if (!(error instanceof RemoteRequestError && error.status === 401)) {
        scheduleStatusStreamReconnect(set, get);
      }
    });
  }, delay);
};

const ensureStatusStream = async (set: RemoteSet, get: RemoteGet) => {
  if (statusStreamSocket) return;
  clearStatusStreamReconnect();
  suppressNextStatusStreamReconnect = false;
  statusStreamSocket = await remoteClient.openStatusStream({
    onAgents: (agents) => {
      const activeAgentId = get().activeAgentId;
      const activeAgent = activeAgentId ? agents.find((agent) => agent.session_id === activeAgentId) : null;
      set({
        agents,
        status: "ready",
        ...(activeAgent
          ? {}
          : {
              activeAgentId: null,
              terminalSnapshot: null,
              terminalLoading: false,
              terminalError: "",
              chatEvents: [],
              chatLoading: false,
              chatError: "",
            }),
      });
      if (activeAgent) {
        const nextRefreshKey = activeAgentRefreshKey(activeAgent);
        const refreshKeyChanged = nextRefreshKey !== lastActiveAgentRefreshKey;
        lastActiveAgentRefreshKey = nextRefreshKey;
        if (refreshKeyChanged || get().activeAgentViewMode === "terminal") {
          scheduleBackgroundActiveChatRefresh(set, get);
        }
      } else {
        lastActiveAgentRefreshKey = null;
      }
    },
    onSessionExpired: () => {
      closeStatusStream();
      set({ status: "session_expired" });
    },
    onError: () => {
      closeStatusStream();
    },
    onClose: () => {
      statusStreamSocket = null;
      if (suppressNextStatusStreamReconnect) {
        suppressNextStatusStreamReconnect = false;
        return;
      }
      scheduleStatusStreamReconnect(set, get);
    },
  });
  statusStreamReconnectAttempts = 0;
};

const handleStatusStreamOpenFailure = (set: RemoteSet, error: unknown) => {
  closeStatusStream();
  if (error instanceof RemoteRequestError && error.status === 401) {
    set({ status: "session_expired" });
  }
};

const pairingParamsFromLocation = () => {
  const search = new URLSearchParams(window.location.search);
  const pairing_offer_id = search.get("pairing_offer_id")?.trim() || "";
  const nonce = search.get("nonce")?.trim() || "";
  const server_identity_fingerprint =
    search.get("server_fingerprint")?.trim() ||
    search.get("server_identity_fingerprint")?.trim() ||
    "";
  if (!pairing_offer_id || !nonce || !server_identity_fingerprint) return null;
  return { pairing_offer_id, nonce, server_identity_fingerprint };
};

const clearPairingUrl = () => {
  if (!window.location.search) return;
  window.history.replaceState({}, "", `${window.location.pathname}${window.location.hash}`);
};

const delay = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));

const authenticateDevice = async (identity: StoredRemoteDeviceIdentity) => {
  const challenge = await remoteClient.createAuthChallenge(identity.device_id);
  if (challenge.server_identity_fingerprint !== identity.server_identity_fingerprint) {
    await clearStoredRemoteIdentity();
    throw new RemotePairingRejectedError("server_identity_mismatch");
  }
  const signature_der_base64 = await signRemoteAuthChallenge(identity.private_key, challenge);
  await remoteClient.createAuthSession({
    challenge_id: challenge.challenge_id,
    device_id: identity.device_id,
    signature_der_base64,
  });
};

const pollPairingApproval = async (
  identity: StoredRemoteDeviceIdentity,
  set: RemoteSet,
) => {
  const requestId = identity.pending_pairing_request_id;
  if (!requestId) throw new RemotePairingExpiredError("missing_pairing_request");

  set({ status: "pairing_pending" });
  while (true) {
    const status = await remoteClient.pairingStatus(requestId);
    if (status.status === "approved") {
      const approvedIdentity = {
        ...identity,
        paired_at: status.paired_at,
        pending_pairing_request_id: undefined,
      };
      await saveStoredRemoteIdentity(approvedIdentity);
      await authenticateDevice(approvedIdentity);
      clearPairingUrl();
      return;
    }
    if (status.status === "rejected") {
      await clearStoredRemoteIdentity();
      throw new RemotePairingRejectedError("pairing_rejected");
    }
    if (Date.parse(status.expires_at) <= Date.now()) {
      await clearStoredRemoteIdentity();
      throw new RemotePairingExpiredError("pairing_expired");
    }
    await delay(1_000);
  }
};

const pairFromUrl = async (set: RemoteSet) => {
  const params = pairingParamsFromLocation();
  if (!params) return false;

  const keyPair = await createRemoteDeviceKeyPair();
  const response = await remoteClient.submitPairing({
    pairing_offer_id: params.pairing_offer_id,
    nonce: params.nonce,
    device_label: defaultRemoteDeviceLabel(),
    public_key_spki_der_base64: keyPair.publicKeySpkiDerBase64,
  });
  const identity: StoredRemoteDeviceIdentity = {
    device_id: response.device_id,
    public_key_fingerprint: response.public_key_fingerprint,
    server_identity_fingerprint: params.server_identity_fingerprint,
    origin: window.location.origin,
    private_key: keyPair.privateKey,
    paired_at: response.paired_at,
    pending_pairing_request_id: response.pairing_request_id,
  };
  await saveStoredRemoteIdentity(identity);
  await pollPairingApproval(identity, set);
  return true;
};

const ensureAuthenticatedSession = async (set: RemoteSet) => {
  try {
    await remoteClient.loadSession();
    return;
  } catch (error) {
    if (!(error instanceof RemoteRequestError) || error.status !== 401) {
      throw error;
    }
  }
  const identity = await loadStoredRemoteIdentity();
  if (!identity) {
    throw new RemoteRequestError("Remote session expired", 401);
  }
  if (identity.pending_pairing_request_id) {
    await pollPairingApproval(identity, set);
    return;
  }
  await authenticateDevice(identity);
};

const loadRemoteShellData = async (set: RemoteSet, get: RemoteGet) => {
  const [agents, workflows] = await Promise.all([remoteClient.listAgents(), remoteClient.listWorkflows()]);
  set((state) => {
    const liveAgentIds = new Set(agents.map((agent) => agent.session_id));
    const activeAgentId = state.activeAgentId && liveAgentIds.has(state.activeAgentId) ? state.activeAgentId : null;
    return {
      agents,
      workflows,
      status: "ready",
      activeAgentId,
      ...(activeAgentId
        ? {}
        : {
            terminalSnapshot: null,
            terminalLoading: false,
            terminalError: "",
            chatEvents: [],
            chatLoading: false,
            chatError: "",
          }),
    };
  });
  void ensureStatusStream(set, get).catch((error: unknown) => handleStatusStreamOpenFailure(set, error));
};

export const useRemoteStore = create<RemoteState>((set, get) => ({
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
  async load() {
    set({ status: "loading" });
    try {
      await pairFromUrl(set);
      await ensureAuthenticatedSession(set);
      await loadRemoteShellData(set, get);
    } catch (error) {
      closeStatusStream();
      if (error instanceof RemotePairingRejectedError) {
        set({
          status:
            error.reason === "server_identity_mismatch"
              ? "gateway_identity_changed"
              : "device_revoked",
        });
        return;
      }
      if (error instanceof RemotePairingExpiredError) {
        set({ status: "pairing_expired" });
        return;
      }
      set({ status: statusFromError(error) });
    }
  },
  disconnectStatusStream() {
    closeStatusStream();
  },
  async openAgent(id) {
    clearBackgroundChatRefresh();
    const activeAgent = get().agents.find((agent) => agent.session_id === id);
    lastActiveAgentRefreshKey = activeAgent ? activeAgentRefreshKey(activeAgent) : null;
    set({
      activeAgentId: id,
      activeAgentViewMode: "terminal",
      terminalSnapshot: null,
      terminalLoading: true,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatError: "",
    });
    await get().refreshActiveAgentTerminal();
  },
  closeAgent() {
    clearBackgroundChatRefresh();
    lastActiveAgentRefreshKey = null;
    set({
      activeAgentId: null,
      activeAgentViewMode: "terminal",
      terminalSnapshot: null,
      terminalLoading: false,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatError: "",
    });
  },
  async setActiveAgentViewMode(mode) {
    set({ activeAgentViewMode: mode });
    if (mode === "chat" && get().chatEvents.length === 0) {
      await get().refreshActiveAgentChat();
      return;
    }
    if (mode === "terminal" && !get().terminalSnapshot) {
      await get().refreshActiveAgentTerminal();
    }
  },
  async refreshActiveAgentTerminal(options) {
    const activeAgentId = get().activeAgentId;
    if (!activeAgentId) return;
    const requestSerial = ++terminalRefreshRequestSerial;
    if (!options?.background) {
      set({ terminalLoading: true, terminalError: "" });
    }
    try {
      const terminalSnapshot = await remoteClient.loadAgentTerminal(activeAgentId);
      set((state) => {
        if (state.activeAgentId !== activeAgentId || requestSerial !== terminalRefreshRequestSerial) return {};
        return { terminalSnapshot, terminalLoading: false, terminalError: "" };
      });
    } catch (error) {
      if (error instanceof RemoteRequestError && error.status === 401) {
        set({ terminalLoading: false, status: "session_expired" });
        return;
      }
      set((state) => {
        if (state.activeAgentId !== activeAgentId || requestSerial !== terminalRefreshRequestSerial) return {};
        return {
          terminalLoading: false,
          terminalError: error instanceof Error ? error.message : String(error),
        };
      });
    }
  },
  async refreshActiveAgentChat(options) {
    const activeAgentId = get().activeAgentId;
    if (!activeAgentId) return;
    if (!options?.background) {
      set({ chatLoading: true, chatError: "" });
    }
    try {
      const chatEvents = await remoteClient.loadAgentChat(activeAgentId);
      set((state) => {
        if (state.activeAgentId !== activeAgentId) return { chatLoading: false };
        if (chatEventsEqual(state.chatEvents, chatEvents)) return { chatLoading: false, chatError: "" };
        return { chatEvents, chatLoading: false, chatError: "" };
      });
    } catch (error) {
      set({
        chatLoading: false,
        chatError: error instanceof Error ? error.message : String(error),
        status: statusFromError(error),
      });
    }
  },
  async sendPromptToActiveAgent(prompt) {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const activeAgentId = get().activeAgentId;
    if (!activeAgentId) return;
    set({ sending: true });
    try {
      await remoteClient.sendPrompt(activeAgentId, trimmed);
      if (get().activeAgentViewMode === "chat") {
        await get().refreshActiveAgentChat();
      } else {
        await get().refreshActiveAgentTerminal();
      }
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    } finally {
      set({ sending: false });
    }
  },
  async broadcastPrompt(prompt) {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const agentIds = get().agents.map((agent) => agent.session_id);
    if (agentIds.length === 0) return;
    set({ sending: true });
    try {
      await sendPromptToTargets(trimmed, agentIds);
      if (get().activeAgentViewMode === "chat") {
        await get().refreshActiveAgentChat({ background: true });
      } else {
        await get().refreshActiveAgentTerminal({ background: true });
      }
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    } finally {
      set({ sending: false });
    }
  },
  async runAgentAction(action, target) {
    try {
      await remoteClient.runAgentAction(action, target);
      if (get().activeAgentId === target) {
        if (get().activeAgentViewMode === "chat") {
          await get().refreshActiveAgentChat({ background: true });
        } else {
          await get().refreshActiveAgentTerminal({ background: true });
        }
      }
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
  async runWorkflow(workflowId) {
    try {
      await remoteClient.runWorkflow(workflowId);
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
}));
