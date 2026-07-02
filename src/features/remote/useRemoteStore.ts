import { create } from "zustand";
import type {
  AgentChatEvent,
  QueueItem,
  RemoteAgentInputMode,
  RemoteAgentSummary,
  RemoteTerminalSnapshot,
  RemoteWorkflowSummary,
} from "../../types";
import {
  DEFAULT_WATCHLIST_PREFS,
  type AgentTeam,
  type Watchlist,
  type WatchlistPrefs,
} from "../../layout/watchlist/types";
import { normalizeWatchlistState } from "../../layout/watchlist/watchlistUtils";
import { normalizedRemoteAgentStatus } from "./remoteAgentStatus";
import { extractTerminalQueueContent } from "../../utils/statusUtils";
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

type ActiveRemoteTab = "watchlist" | "workflows" | "queue" | "garden" | "library";
type RemoteAgentViewMode = "terminal" | "chat";

export const MIN_REMOTE_TERMINAL_FONT_SIZE = 10;
export const MAX_REMOTE_TERMINAL_FONT_SIZE = 20;
export const DEFAULT_REMOTE_TERMINAL_FONT_SIZE = 11;

interface RemoteState {
  agents: RemoteAgentSummary[];
  workflows: RemoteWorkflowSummary[];
  remoteQueueItems: QueueItem[];
  remoteQueueBuffers: Record<string, string>;
  remoteAgentStatuses: Record<string, string>;
  watchlists: Watchlist[];
  teams: AgentTeam[];
  watchlistPrefs: WatchlistPrefs;
  activeWatchlistId: string;
  activeRemoteTab: ActiveRemoteTab;
  mobileCollapsedTeamIds: string[];
  mobileCollapsedTeamIdsByList: Record<string, string[]>;
  status: RemoteStatus;
  activeAgentId: string | null;
  activeAgentViewMode: RemoteAgentViewMode;
  remoteAgentDefaultViewMode: RemoteAgentViewMode;
  remoteTerminalFontSize: number;
  terminalSnapshot: RemoteTerminalSnapshot | null;
  terminalLoading: boolean;
  terminalError: string;
  chatEvents: AgentChatEvent[];
  chatLoading: boolean;
  chatError: string;
  sending: boolean;
  load: () => Promise<void>;
  refresh: () => Promise<void>;
  disconnectStatusStream: () => void;
  setActiveWatchlistId: (id: string) => void;
  setActiveRemoteTab: (tab: ActiveRemoteTab) => void;
  setRemoteAgentDefaultViewMode: (mode: RemoteAgentViewMode) => void;
  setRemoteTerminalFontSize: (value: number) => void;
  toggleMobileTeamCollapsed: (teamId: string) => void;
  openAgent: (id: string) => Promise<void>;
  closeAgent: (options?: { syncHistory?: boolean }) => void;
  setActiveAgentViewMode: (mode: RemoteAgentViewMode) => Promise<void>;
  refreshActiveAgentTerminal: (options?: { background?: boolean }) => Promise<void>;
  refreshActiveAgentChat: (options?: { background?: boolean }) => Promise<void>;
  appendRemoteTerminalQueueOutput: (sessionId: string, data: string, provider?: string) => void;
  sendPromptToActiveAgent: (prompt: string, inputMode?: RemoteAgentInputMode) => Promise<void>;
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

const REMOTE_ACTIVE_WATCHLIST_STORAGE_KEY = "wardian.remote.activeWatchlistId";
const REMOTE_AGENT_DEFAULT_VIEW_STORAGE_KEY = "wardian.remote.agentDefaultViewMode";
const REMOTE_TERMINAL_FONT_SIZE_STORAGE_KEY = "wardian.remote.terminalFontSize";
const REMOTE_HISTORY_DETAIL_VIEW = "agent_detail";
const BACKGROUND_CHAT_REFRESH_MIN_INTERVAL_MS = 750;
const STATUS_STREAM_RECONNECT_BASE_DELAY_MS = 250;
const STATUS_STREAM_RECONNECT_MAX_DELAY_MS = 5_000;
const REMOTE_QUEUE_SUMMARY_MAX_CHARS = 500;
const REMOTE_QUEUE_MAX_ITEMS = 100;

const storedActiveWatchlistId = () => {
  try {
    return window.localStorage.getItem(REMOTE_ACTIVE_WATCHLIST_STORAGE_KEY) || "all";
  } catch {
    return "all";
  }
};

const normalizeRemoteAgentViewMode = (value: string | null | undefined): RemoteAgentViewMode =>
  value === "chat" ? "chat" : "terminal";

const storedRemoteAgentDefaultViewMode = () => {
  try {
    return normalizeRemoteAgentViewMode(window.localStorage.getItem(REMOTE_AGENT_DEFAULT_VIEW_STORAGE_KEY));
  } catch {
    return "terminal";
  }
};

export const normalizeRemoteTerminalFontSize = (value: number) => {
  if (!Number.isFinite(value)) return DEFAULT_REMOTE_TERMINAL_FONT_SIZE;
  return Math.min(MAX_REMOTE_TERMINAL_FONT_SIZE, Math.max(MIN_REMOTE_TERMINAL_FONT_SIZE, Math.round(value)));
};

const storedRemoteTerminalFontSize = () => {
  try {
    const stored = window.localStorage.getItem(REMOTE_TERMINAL_FONT_SIZE_STORAGE_KEY);
    return stored === null ? DEFAULT_REMOTE_TERMINAL_FONT_SIZE : normalizeRemoteTerminalFontSize(Number(stored));
  } catch {
    return DEFAULT_REMOTE_TERMINAL_FONT_SIZE;
  }
};

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

const normalizePromptText = (value: string) => value.replace(/\s+/g, " ").trim();

const matchingUserMessageCount = (events: AgentChatEvent[], text: string) => {
  const normalized = normalizePromptText(text);
  if (!normalized) return 0;
  return events.filter((event) => event.kind === "message" && event.role === "user" && normalizePromptText(event.text ?? "") === normalized)
    .length;
};

const maxSequence = (events: AgentChatEvent[]) =>
  events.reduce((max, event) => (typeof event.sequence === "number" ? Math.max(max, event.sequence) : max), 0);

const optimisticChatEvents = (events: AgentChatEvent[]) =>
  events.filter((event) => event.kind === "message" && event.role === "user" && event.metadata?.optimistic === true);

const pendingConfirmAfterMatchingUserCount = (event: AgentChatEvent) => {
  const value = event.metadata?.confirm_after_matching_user_count;
  return typeof value === "number" ? value : null;
};

const unconfirmedOptimisticChatEvents = (transcript: AgentChatEvent[], currentEvents: AgentChatEvent[]) =>
  optimisticChatEvents(currentEvents).filter((event) => {
    const pendingText = normalizePromptText(event.text ?? "");
    if (!pendingText) return false;
    const confirmAfterCount = pendingConfirmAfterMatchingUserCount(event);
    if (confirmAfterCount === null) return matchingUserMessageCount(transcript, pendingText) === 0;
    return matchingUserMessageCount(transcript, pendingText) <= confirmAfterCount;
  });

const mergeOptimisticChatEvents = (transcript: AgentChatEvent[], currentEvents: AgentChatEvent[]) => {
  const pending = unconfirmedOptimisticChatEvents(transcript, currentEvents);
  if (pending.length === 0) return transcript;
  const baseSequence = maxSequence(transcript);
  return [
    ...transcript,
    ...pending.map((event, index) => ({
      ...event,
      sequence: baseSequence + index + 1,
    })),
  ];
};

const createOptimisticUserMessage = (
  sessionId: string,
  provider: string,
  prompt: string,
  currentEvents: AgentChatEvent[],
): AgentChatEvent => {
  const createdAt = new Date().toISOString();
  return {
    id: `pending-user-${sessionId}-${createdAt}`,
    session_id: sessionId,
    provider,
    kind: "message",
    role: "user",
    text: prompt,
    title: null,
    status: "succeeded",
    turn_id: null,
    source: "chat_input",
    command: null,
    exit_code: null,
    path: null,
    language: null,
    created_at: createdAt,
    sequence: maxSequence(currentEvents) + 1,
    metadata: {
      optimistic: true,
      confirm_after_matching_user_count: matchingUserMessageCount(currentEvents, prompt),
    },
  };
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

const activeAgentStatusShouldRefreshChat = (status: string) => {
  const normalized = normalizedRemoteAgentStatus(status);
  return normalized === "processing" || normalized === "running" || normalized === "action_required" || normalized === "action_needed";
};

const statusCanFlushRemoteCompletion = (previousStatus: string | undefined, nextStatus: string) => {
  const previous = previousStatus ? normalizedRemoteAgentStatus(previousStatus) : "";
  const next = normalizedRemoteAgentStatus(nextStatus);
  return (previous === "processing" || previous === "running") && next === "idle";
};

const boundRemoteQueueSummary = (text: string): string => {
  if (text.length <= REMOTE_QUEUE_SUMMARY_MAX_CHARS) return text;
  const marker = "\n...\n";
  const available = REMOTE_QUEUE_SUMMARY_MAX_CHARS - marker.length;
  const headLength = Math.ceil(available * 0.72);
  const tailLength = available - headLength;
  return `${text.slice(0, headLength)}${marker}${text.slice(-tailLength)}`;
};

const newRemoteQueueItemId = () => {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `remote-queue-${Date.now()}-${Math.random().toString(36).slice(2)}`;
};

const remoteAgentStatusMap = (agents: RemoteAgentSummary[]) =>
  Object.fromEntries(agents.map((agent) => [agent.session_id, agent.status]));

const remoteQueuePatchForAgents = (state: RemoteState, agents: RemoteAgentSummary[]): Partial<RemoteState> => {
  let items = state.remoteQueueItems;
  let buffers = state.remoteQueueBuffers;
  let changed = false;

  for (const agent of agents) {
    if (!statusCanFlushRemoteCompletion(state.remoteAgentStatuses[agent.session_id], agent.status)) continue;
    const summary = (buffers[agent.session_id] ?? "").trim();
    if (!summary) continue;

    const item: QueueItem = {
      id: newRemoteQueueItemId(),
      type: "agent_completed",
      timestamp: Date.now(),
      read: false,
      agent_session_id: agent.session_id,
      agent_name: agent.session_name,
      summary: boundRemoteQueueSummary(summary),
      evidence_source: "live_runtime",
    };
    items = [item, ...items].slice(0, REMOTE_QUEUE_MAX_ITEMS);
    buffers = { ...buffers, [agent.session_id]: "" };
    changed = true;
  }

  return {
    remoteAgentStatuses: remoteAgentStatusMap(agents),
    ...(changed ? { remoteQueueItems: items, remoteQueueBuffers: buffers } : {}),
  };
};

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
      set((state) => ({
        agents,
        status: "ready",
        ...remoteQueuePatchForAgents(state, agents),
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
      }));
      if (activeAgent) {
        const nextRefreshKey = activeAgentRefreshKey(activeAgent);
        const refreshKeyChanged = nextRefreshKey !== lastActiveAgentRefreshKey;
        lastActiveAgentRefreshKey = nextRefreshKey;
        if ((refreshKeyChanged || activeAgentStatusShouldRefreshChat(activeAgent.status)) && get().activeAgentViewMode === "chat") {
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

const currentHistoryStateObject = () =>
  typeof window.history.state === "object" && window.history.state !== null && !Array.isArray(window.history.state)
    ? window.history.state
    : {};

const isRemoteAgentDetailHistoryState = (state = window.history.state) =>
  typeof state === "object" &&
  state !== null &&
  !Array.isArray(state) &&
  (state as { wardian_remote_view?: unknown }).wardian_remote_view === REMOTE_HISTORY_DETAIL_VIEW;

const pushRemoteAgentDetailHistory = (agentId: string) => {
  try {
    const currentState = currentHistoryStateObject();
    if (
      isRemoteAgentDetailHistoryState(currentState) &&
      (currentState as { wardian_remote_agent_id?: unknown }).wardian_remote_agent_id === agentId
    ) {
      return;
    }
    window.history.pushState(
      {
        ...currentState,
        wardian_remote_view: REMOTE_HISTORY_DETAIL_VIEW,
        wardian_remote_agent_id: agentId,
      },
      "",
      `${window.location.pathname}${window.location.search}${window.location.hash}`,
    );
  } catch {
    // Some embedded browsers restrict history writes; explicit in-app back remains available.
  }
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
  const [agents, workflows, remoteWatchlists] = await Promise.all([
    remoteClient.listAgents(),
    remoteClient.listWorkflows().catch((error: unknown) => {
      if (error instanceof RemoteRequestError && error.status === 404) return [];
      throw error;
    }),
    remoteClient.loadWatchlists().catch((error: unknown) => {
      if (error instanceof RemoteRequestError && error.status === 404) {
        return { watchlists: [], teams: [], prefs: null };
      }
      throw error;
    }),
  ]);
  const watchlistState = normalizeWatchlistState({
    version: 2,
    watchlists: remoteWatchlists.watchlists,
    teams: remoteWatchlists.teams,
  });
  const watchlistIds = new Set(watchlistState.watchlists.map((list) => list.id));
  const storedId = storedActiveWatchlistId();
  const activeWatchlistId = storedId === "all" || watchlistIds.has(storedId) ? storedId : "all";
  const watchlistPrefs = {
    ...DEFAULT_WATCHLIST_PREFS,
    ...(remoteWatchlists.prefs ?? {}),
    collapsed_team_ids: Array.isArray(remoteWatchlists.prefs?.collapsed_team_ids)
      ? remoteWatchlists.prefs.collapsed_team_ids
      : [],
  };
  set((state) => {
    const liveAgentIds = new Set(agents.map((agent) => agent.session_id));
    const activeAgentId = state.activeAgentId && liveAgentIds.has(state.activeAgentId) ? state.activeAgentId : null;
    return {
      agents,
      workflows,
      remoteAgentStatuses: remoteAgentStatusMap(agents),
      watchlists: watchlistState.watchlists,
      teams: watchlistState.teams,
      watchlistPrefs,
      activeWatchlistId,
      mobileCollapsedTeamIdsByList: { all: watchlistPrefs.collapsed_team_ids },
      mobileCollapsedTeamIds: activeWatchlistId === "all" ? watchlistPrefs.collapsed_team_ids : [],
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
  remoteQueueItems: [],
  remoteQueueBuffers: {},
  remoteAgentStatuses: {},
  watchlists: [],
  teams: [],
  watchlistPrefs: DEFAULT_WATCHLIST_PREFS,
  activeWatchlistId: "all",
  activeRemoteTab: "watchlist",
  mobileCollapsedTeamIds: [],
  mobileCollapsedTeamIdsByList: {},
  status: "loading",
  activeAgentId: null,
  activeAgentViewMode: "terminal",
  remoteAgentDefaultViewMode: storedRemoteAgentDefaultViewMode(),
  remoteTerminalFontSize: storedRemoteTerminalFontSize(),
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
  async refresh() {
    try {
      await ensureAuthenticatedSession(set);
      await loadRemoteShellData(set, get);
    } catch (error) {
      closeStatusStream();
      set({ status: statusFromError(error) });
    }
  },
  disconnectStatusStream() {
    closeStatusStream();
  },
  setActiveWatchlistId(id) {
    set((state) => ({
      activeWatchlistId: id,
      mobileCollapsedTeamIds: state.mobileCollapsedTeamIdsByList[id] ?? [],
    }));
    try {
      window.localStorage.setItem(REMOTE_ACTIVE_WATCHLIST_STORAGE_KEY, id);
    } catch {
      // Browser storage may be unavailable in locked-down contexts.
    }
  },
  setActiveRemoteTab(tab) {
    set({ activeRemoteTab: tab });
  },
  setRemoteAgentDefaultViewMode(mode) {
    const normalized = normalizeRemoteAgentViewMode(mode);
    set({ remoteAgentDefaultViewMode: normalized });
    try {
      window.localStorage.setItem(REMOTE_AGENT_DEFAULT_VIEW_STORAGE_KEY, normalized);
    } catch {
      // Browser storage may be unavailable in locked-down contexts.
    }
  },
  setRemoteTerminalFontSize(value) {
    const normalized = normalizeRemoteTerminalFontSize(value);
    set({ remoteTerminalFontSize: normalized });
    try {
      window.localStorage.setItem(REMOTE_TERMINAL_FONT_SIZE_STORAGE_KEY, String(normalized));
    } catch {
      // Browser storage may be unavailable in locked-down contexts.
    }
  },
  toggleMobileTeamCollapsed(teamId) {
    set((state) => ({
      ...(() => {
        const scopeId = state.activeWatchlistId;
        const current = state.mobileCollapsedTeamIdsByList[scopeId] ?? [];
        const next = current.includes(teamId)
          ? current.filter((id) => id !== teamId)
          : [...current, teamId];
        return {
          mobileCollapsedTeamIdsByList: {
            ...state.mobileCollapsedTeamIdsByList,
            [scopeId]: next,
          },
          mobileCollapsedTeamIds: next,
        };
      })(),
    }));
  },
  async openAgent(id) {
    clearBackgroundChatRefresh();
    const activeAgent = get().agents.find((agent) => agent.session_id === id);
    lastActiveAgentRefreshKey = activeAgent ? activeAgentRefreshKey(activeAgent) : null;
    pushRemoteAgentDetailHistory(id);
    const activeAgentViewMode = get().remoteAgentDefaultViewMode;
    set({
      activeAgentId: id,
      activeAgentViewMode,
      terminalSnapshot: null,
      terminalLoading: false,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatError: "",
    });
    if (activeAgentViewMode === "chat") {
      await get().refreshActiveAgentChat();
    }
  },
  closeAgent(options) {
    if (options?.syncHistory !== false && isRemoteAgentDetailHistoryState()) {
      try {
        window.history.back();
      } catch {
        // If browser history cannot move, still close the in-app detail view.
      }
    }
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
    if (mode === "terminal") set({ terminalLoading: false, terminalError: "" });
  },
  async refreshActiveAgentTerminal(options) {
    const activeAgentId = get().activeAgentId;
    if (!activeAgentId) return;
    terminalRefreshRequestSerial += 1;
    if (!options?.background) set({ terminalLoading: false, terminalError: "" });
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
        const mergedChatEvents = mergeOptimisticChatEvents(chatEvents, state.chatEvents);
        if (chatEventsEqual(state.chatEvents, mergedChatEvents)) return { chatLoading: false, chatError: "" };
        return { chatEvents: mergedChatEvents, chatLoading: false, chatError: "" };
      });
    } catch (error) {
      set({
        chatLoading: false,
        chatError: error instanceof Error ? error.message : String(error),
        status: statusFromError(error),
      });
    }
  },
  appendRemoteTerminalQueueOutput(sessionId, data, provider) {
    if (provider && provider !== "opencode") return;
    const text = extractTerminalQueueContent(data);
    if (!text) return;
    set((state) => ({
      remoteQueueBuffers: {
        ...state.remoteQueueBuffers,
        [sessionId]: boundRemoteQueueSummary(
          `${state.remoteQueueBuffers[sessionId] ? `${state.remoteQueueBuffers[sessionId]}\n` : ""}${text}`,
        ),
      },
    }));
  },
  async sendPromptToActiveAgent(prompt, inputMode = "message") {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const activeAgentId = get().activeAgentId;
    if (!activeAgentId) return;
    set({ sending: true });
    try {
      await remoteClient.sendPrompt(activeAgentId, trimmed, inputMode);
      if (get().activeAgentViewMode === "chat") {
        if (inputMode === "message") {
          set((state) => {
            if (state.activeAgentId !== activeAgentId) return {};
            const activeAgent = state.agents.find((agent) => agent.session_id === activeAgentId);
            return {
              chatEvents: [
                ...state.chatEvents,
                createOptimisticUserMessage(activeAgentId, activeAgent?.provider ?? "unknown", trimmed, state.chatEvents),
              ],
            };
          });
        }
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
        if (action === "clear") {
          set({
            terminalSnapshot: null,
            terminalLoading: false,
            terminalError: "",
            chatEvents: [],
            chatLoading: false,
            chatError: "",
          });
        }
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
