import { create } from "zustand";
import type {
  AgentChatEvent,
  AgentChatPage,
  AgentChatDetail,
  QueueItem,
  RemoteAgentInputMode,
  RemoteAgentSummary,
  RemoteTerminalSnapshot,
  RemoteAutomationSummary,
} from "../../types";
import { addChatSubmission, applyChatPage, canAdmitOlderChatPage, submittedChatEvent } from "../chat/chatReadState";
import {
  DEFAULT_WATCHLIST_PREFS,
  type AgentTeam,
  type Watchlist,
  type WatchlistPrefs,
} from "../../layout/watchlist/types";
import { normalizeWatchlistState } from "../../layout/watchlist/watchlistUtils";
import { normalizedRemoteAgentStatus } from "./remoteAgentStatus";
import {
  clearStoredRemoteIdentity,
  createRemoteDeviceKeyPair,
  defaultRemoteDeviceLabel,
  loadStoredRemoteIdentity,
  saveStoredRemoteIdentity,
  signRemoteAuthChallenge,
  type StoredRemoteDeviceIdentity,
} from "./remoteIdentity";
import { remoteClient, RemoteChatBodyError, RemoteChatTimeoutError, RemoteRequestError } from "./remoteClient";

type RemoteStatus =
  | "loading"
  | "ready"
  | "unreachable"
  | "session_expired"
  | "pairing_pending"
  | "pairing_expired"
  | "gateway_identity_changed"
  | "device_revoked";

type ActiveRemoteTab = "watchlist" | "automations" | "queue" | "garden" | "library";
type RemoteAgentViewMode = "terminal" | "chat";

export const MIN_REMOTE_TERMINAL_FONT_SIZE = 10;
export const MAX_REMOTE_TERMINAL_FONT_SIZE = 20;
export const DEFAULT_REMOTE_TERMINAL_FONT_SIZE = 11;

interface RemoteState {
  agents: RemoteAgentSummary[];
  automations: RemoteAutomationSummary[];
  remoteQueueItems: QueueItem[];
  providerChoiceRecoveryByItem: Record<string, string>;
  remoteQueueError: string;
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
  activeAgentViewModesById: Record<string, RemoteAgentViewMode>;
  terminalSnapshot: RemoteTerminalSnapshot | null;
  terminalLoading: boolean;
  terminalError: string;
  chatEvents: AgentChatEvent[];
  chatLoading: boolean;
  chatLoadingOlder: boolean;
  chatBrowsingOlder: boolean;
  chatHasOlder: boolean;
  chatNextBefore: string | null;
  chatPage: AgentChatPage | null;
  chatError: string;
  sending: boolean;
  load: () => Promise<void>;
  refresh: () => Promise<void>;
  refreshInbox: () => Promise<boolean>;
  runInboxAction: (action: string, itemId?: string, choice?: string) => Promise<void>;
  recordProviderChoiceRecovery: (itemId: string, choice: string) => void;
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
  loadOlderActiveAgentChat: () => Promise<void>;
  jumpToLatestActiveAgentChat: () => void;
  loadActiveAgentChatDetail: (reference: string) => Promise<AgentChatDetail>;
  sendPromptToActiveAgent: (prompt: string, inputMode?: RemoteAgentInputMode) => Promise<void>;
  sendPromptToAgent: (sessionId: string, prompt: string, inboxItemId?: string) => Promise<void>;
  runAgentAction: (action: string, target: string) => Promise<void>;
  runAutomation: (automationId: string) => Promise<void>;
}

type RemoteSet = (
  partial: Partial<RemoteState> | ((state: RemoteState) => Partial<RemoteState>),
) => void;
type RemoteGet = () => RemoteState;

const PAIRING_EXPIRED_ERROR_CODES = new Set([
  "pairing_offer_not_found",
  "pairing_offer_used",
  "pairing_offer_expired",
  "pairing_offer_invalid",
  "pending_pairing_not_found",
  "pending_pairing_not_active",
]);

const statusFromError = (error: unknown): RemoteStatus => {
  if (!(error instanceof RemoteRequestError)) return "unreachable";
  if (error.status === 401) return "session_expired";
  if (error.code && PAIRING_EXPIRED_ERROR_CODES.has(error.code)) return "pairing_expired";
  if (error.code === "device_not_found" || error.code === "device_revoked") return "device_revoked";
  return "unreachable";
};

const chatConnectionStatusFromError = (error: unknown): RemoteStatus | null => {
  if (error instanceof RemoteChatTimeoutError || error instanceof RemoteChatBodyError) return null;
  const status = statusFromError(error);
  // Chat deadlines and application failures do not establish connectivity loss.
  return error instanceof RemoteRequestError
    && status === "unreachable"
    && error.status >= 400
    && error.status < 500 ? null : status;
};

const chatErrorMessage = (error: unknown): string => {
  if (error instanceof RemoteChatTimeoutError || error instanceof RemoteChatBodyError) return error.message;
  if (error instanceof RemoteRequestError) {
    const stages: Record<string, string> = {
      agent_chat_snapshot_failed: "Agent state could not be read",
      agent_chat_provider_capture_failed: "Provider history could not be captured",
      agent_chat_archive_write_failed: "Chat history could not be saved",
      agent_chat_projection_failed: "Chat history could not be prepared",
      agent_chat_projection_unavailable: "Chat history could not be prepared",
      agent_chat_provenance_failed: "Chat history ownership could not be verified",
    };
    const stage = error.code && Object.prototype.hasOwnProperty.call(stages, error.code) ? stages[error.code] : undefined;
    return stage ? `${stage}. Retry to load Chat again.` : `Remote request failed: ${error.status}`;
  }
  return "Chat could not be loaded. Retry when the desktop is available.";
};

const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);

const REMOTE_ACTIVE_WATCHLIST_STORAGE_KEY = "wardian.remote.activeWatchlistId";
const REMOTE_AGENT_DEFAULT_VIEW_STORAGE_KEY = "wardian.remote.agentDefaultViewMode";
const REMOTE_TERMINAL_FONT_SIZE_STORAGE_KEY = "wardian.remote.terminalFontSize";
const REMOTE_HISTORY_DETAIL_VIEW = "agent_detail";
const BACKGROUND_CHAT_REFRESH_MIN_INTERVAL_MS = 750;
const STATUS_STREAM_RECONNECT_BASE_DELAY_MS = 250;
const STATUS_STREAM_RECONNECT_MAX_DELAY_MS = 5_000;

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

const chatEventFingerprint = (event: AgentChatEvent) => JSON.stringify(event);

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

let statusStreamSocket: WebSocket | null = null;
let backgroundChatRefreshTimer: number | null = null;
let backgroundChatRefreshInFlight = false;
let backgroundChatRefreshQueued = false;
let lastBackgroundChatRefreshStartedAt = 0;
let statusStreamReconnectTimer: number | null = null;
let statusStreamReconnectAttempts = 0;
let lastActiveAgentRefreshKey: string | null = null;
let terminalRefreshRequestSerial = 0;
let chatRefreshRequestSerial = 0;
interface ChatReadFlight {
  agentId: string;
  serial: number;
  window: number;
  controller: AbortController;
  promise: Promise<void>;
}
interface OlderChatReadFlight extends ChatReadFlight {
  physical: Promise<void> | null;
  recentDue: boolean;
  recentTurn: boolean;
  resume: () => Promise<void>;
  settle: () => void;
}
let chatReadInFlight: ChatReadFlight | null = null;
let chatOlderReadInFlight: OlderChatReadFlight | null = null;
let chatOlderCursor: { serial: number; generation: string | null; before: string | null } | null = null;
let chatWindowRequestSerial = 0;
let chatWindowRefreshQueued = false;
let chatForceRecentRead = false;
let queueRefreshRequestSerial = 0;

/** Enrich verified loaded members without replacing the user's history window. */
function patchLoadedRemoteChatMembers(current: AgentChatEvent[], next: AgentChatPage): AgentChatEvent[] {
  const loaded = new Map(current.map((event) => [event.id, event]));
  const aliases = next.aliases.filter((alias) => loaded.has(alias.observation_id));
  const observations = new Map(aliases.map((alias) => [alias.canonical_id, alias.observation_id]));
  const events = next.events.flatMap((event) => {
    const old = loaded.get(event.id) ?? loaded.get(observations.get(event.id) ?? "");
    if (!old || event.session_id !== next.session_id) return [];
    const oldBinding = old.metadata.chat_body_binding;
    const binding = event.metadata.chat_body_binding;
    if (oldBinding !== undefined && binding !== undefined && oldBinding !== binding) return [];
    const metadata = { ...old.metadata, ...event.metadata };
    if (old.metadata.chat_body_pending === false) metadata.chat_body_pending = false;
    if (old.metadata.chat_detail_ref && !event.metadata.chat_detail_ref) metadata.chat_detail_ref = old.metadata.chat_detail_ref;
    const text = old.text && (!event.text || !event.text.startsWith(old.text)) ? old.text : event.text;
    return [{ ...event, text, metadata }];
  });
  const result = applyChatPage(current, { ...next, reset: false, events, aliases, removed_ids: [] }, "recent", "older");
  const retained = new Set(result.map((event) => event.id));
  const canonical = new Map(aliases.map((alias) => [alias.observation_id, alias.canonical_id]));
  if (current.some((event) => !retained.has(event.id) && !retained.has(canonical.get(event.id) ?? ""))) {
    throw new Error("Loaded metadata patch exceeds the visible window. Retry without advancing history.");
  }
  return result;
}

const retireActiveChatReads = () => {
  chatReadInFlight?.controller.abort();
  chatOlderReadInFlight?.controller.abort();
  chatOlderReadInFlight?.settle();
};

interface StatusStreamAttempt {
  generation: number;
  id: number;
}

let statusStreamLifecycleGeneration = 0;
let statusStreamAttemptId = 0;
let statusStreamOpenInFlight: StatusStreamAttempt | null = null;
let statusStreamActiveAttempt: StatusStreamAttempt | null = null;

const refreshRemoteQueue = async (set: RemoteSet): Promise<boolean> => {
  const requestSerial = ++queueRefreshRequestSerial;
  try {
    const remoteQueueItems = await remoteClient.loadQueueItems();
    if (requestSerial === queueRefreshRequestSerial) {
      set({ remoteQueueItems, remoteQueueError: "" });
      return true;
    }
    return false;
  } catch (error) {
    if (requestSerial === queueRefreshRequestSerial) {
      set({ remoteQueueError: errorMessage(error) });
    }
    return false;
  }
};

const clearBackgroundChatRefresh = (resetInterval = false) => {
  if (backgroundChatRefreshTimer !== null) {
    window.clearTimeout(backgroundChatRefreshTimer);
    backgroundChatRefreshTimer = null;
  }
  backgroundChatRefreshQueued = false;
  // A transport pause retains the interval; a retired agent starts a new cadence.
  if (resetInterval) lastBackgroundChatRefreshStartedAt = 0;
};

const clearStatusStreamReconnect = () => {
  if (statusStreamReconnectTimer !== null) {
    window.clearTimeout(statusStreamReconnectTimer);
    statusStreamReconnectTimer = null;
  }
};

const closeStatusStream = () => {
  const socket = statusStreamSocket;
  statusStreamSocket = null;
  statusStreamActiveAttempt = null;
  statusStreamOpenInFlight = null;
  statusStreamLifecycleGeneration += 1;
  clearStatusStreamReconnect();
  clearBackgroundChatRefresh();
  socket?.close();
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
      if (chatOlderReadInFlight) {
        if (get().status !== "ready") return;
        if (document.visibilityState === "hidden") {
          scheduleBackgroundActiveChatRefresh(set, get);
          return;
        }
        const olderRead = chatOlderReadInFlight;
        if (olderRead.recentDue && !olderRead.physical && !chatReadInFlight) {
          // One existing interval serves recent metadata before the original older demand resumes.
          olderRead.recentDue = false;
          olderRead.recentTurn = true;
          chatWindowRefreshQueued = false;
          try { await get().refreshActiveAgentChat({ background: true }); }
          finally {
            olderRead.recentTurn = false;
            if (chatOlderReadInFlight === olderRead && get().status === "ready") scheduleBackgroundActiveChatRefresh(set, get);
          }
        } else await olderRead.resume();
        if (!chatOlderReadInFlight && chatWindowRefreshQueued && get().status === "ready") {
          chatWindowRefreshQueued = false;
          await get().refreshActiveAgentChat({ background: true });
        }
      } else await get().refreshActiveAgentChat({ background: true });
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

const pruneActiveAgentViewModes = (
  modesById: Record<string, RemoteAgentViewMode>,
  liveAgentIds: Set<string>,
) => Object.fromEntries(Object.entries(modesById).filter(([agentId]) => liveAgentIds.has(agentId)));

const scheduleStatusStreamReconnect = (set: RemoteSet, get: RemoteGet) => {
  if (
    statusStreamReconnectTimer !== null ||
    statusStreamSocket ||
    statusStreamOpenInFlight ||
    get().status === "session_expired"
  ) return;
  const delay = Math.min(
    STATUS_STREAM_RECONNECT_MAX_DELAY_MS,
    STATUS_STREAM_RECONNECT_BASE_DELAY_MS * 2 ** statusStreamReconnectAttempts,
  );
  statusStreamReconnectAttempts += 1;
  statusStreamReconnectTimer = window.setTimeout(() => {
    statusStreamReconnectTimer = null;
    void ensureStatusStream(set, get).catch((error) => handleStatusStreamOpenFailure(set, get, error));
  }, delay);
};

const retryStatusStreamAfterError = (set: RemoteSet, get: RemoteGet) => {
  closeStatusStream();
  scheduleStatusStreamReconnect(set, get);
};

const isMatchingStatusStreamAttempt = (current: StatusStreamAttempt | null, expected: StatusStreamAttempt) =>
  current?.generation === expected.generation && current.id === expected.id;

const isCurrentStatusStreamAttempt = (attempt: StatusStreamAttempt) =>
  attempt.generation === statusStreamLifecycleGeneration &&
  (isMatchingStatusStreamAttempt(statusStreamOpenInFlight, attempt) || isMatchingStatusStreamAttempt(statusStreamActiveAttempt, attempt));

const ensureStatusStream = async (set: RemoteSet, get: RemoteGet) => {
  if (statusStreamSocket || statusStreamOpenInFlight) return;
  clearStatusStreamReconnect();
  const attempt: StatusStreamAttempt = {
    generation: statusStreamLifecycleGeneration,
    id: ++statusStreamAttemptId,
  };
  statusStreamOpenInFlight = attempt;

  try {
    const socket = await remoteClient.openStatusStream({
      onAgents: (agents) => {
        if (!isCurrentStatusStreamAttempt(attempt)) return;
        statusStreamReconnectAttempts = 0;
        const activeAgentId = get().activeAgentId;
        const liveAgentIds = new Set(agents.map((agent) => agent.session_id));
        const activeAgent = activeAgentId ? agents.find((agent) => agent.session_id === activeAgentId) : null;
        if (activeAgentId && !activeAgent) {
          retireActiveChatReads();
          chatRefreshRequestSerial += 1;
        }
        set((state) => ({
          agents,
          status: "ready",
          activeAgentViewModesById: pruneActiveAgentViewModes(state.activeAgentViewModesById, liveAgentIds),
          ...(activeAgent
            ? {}
            : {
                activeAgentId: null,
                activeAgentViewMode: "terminal",
                terminalSnapshot: null,
                terminalLoading: false,
                terminalError: "",
                chatEvents: [],
                chatLoading: false,
                chatLoadingOlder: false,
                chatHasOlder: false,
                chatNextBefore: null,
                chatError: "",
              }),
        }));
        void refreshRemoteQueue(set);
        if (activeAgent) {
          const nextRefreshKey = activeAgentRefreshKey(activeAgent);
          const refreshKeyChanged = nextRefreshKey !== lastActiveAgentRefreshKey;
          lastActiveAgentRefreshKey = nextRefreshKey;
          if ((refreshKeyChanged || activeAgentStatusShouldRefreshChat(activeAgent.status) || chatOlderReadInFlight)
            && get().activeAgentViewMode === "chat") {
            scheduleBackgroundActiveChatRefresh(set, get);
          }
        } else {
          lastActiveAgentRefreshKey = null;
        }
      },
      onSessionExpired: () => {
        if (!isCurrentStatusStreamAttempt(attempt)) return;
        closeStatusStream();
        set({ status: "session_expired" });
      },
      onError: () => {
        if (!isCurrentStatusStreamAttempt(attempt)) return;
        retryStatusStreamAfterError(set, get);
      },
      onClose: () => {
        if (!isCurrentStatusStreamAttempt(attempt)) return;
        statusStreamSocket = null;
        statusStreamActiveAttempt = null;
        statusStreamOpenInFlight = null;
        statusStreamLifecycleGeneration += 1;
        clearBackgroundChatRefresh();
        scheduleStatusStreamReconnect(set, get);
      },
    });

    if (!isCurrentStatusStreamAttempt(attempt)) {
      socket.close();
      return;
    }
    statusStreamSocket = socket;
    statusStreamActiveAttempt = attempt;
  } catch (error) {
    if (isCurrentStatusStreamAttempt(attempt)) throw error;
  } finally {
    if (statusStreamOpenInFlight?.id === attempt.id) statusStreamOpenInFlight = null;
  }
};

const handleStatusStreamOpenFailure = (set: RemoteSet, get: RemoteGet, error: unknown) => {
  closeStatusStream();
  if (error instanceof RemoteRequestError && error.status === 401) {
    set({ status: "session_expired" });
    return;
  }
  scheduleStatusStreamReconnect(set, get);
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
  const [agents, remoteWatchlists] = await Promise.all([
    remoteClient.listAgents(),
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
      remoteQueueError: "",
      activeAgentViewModesById: pruneActiveAgentViewModes(state.activeAgentViewModesById, liveAgentIds),
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
            activeAgentViewMode: "terminal",
            terminalSnapshot: null,
            terminalLoading: false,
            terminalError: "",
            chatEvents: [],
            chatLoading: false,
            chatLoadingOlder: false,
            chatHasOlder: false,
            chatNextBefore: null,
            chatError: "",
          }),
    };
  });

  // Neither surface is needed to render the watchlist. Keep a slow or
  // temporarily unavailable optional endpoint from holding the whole remote
  // shell in its initial loading state.
  void remoteClient.listAutomations()
    .catch(() => [])
    .then((automations) => set({ automations }));
  void refreshRemoteQueue(set);
  void ensureStatusStream(set, get).catch((error: unknown) => handleStatusStreamOpenFailure(set, get, error));
};

export const useRemoteStore = create<RemoteState>((set, get) => ({
  agents: [],
  automations: [],
  remoteQueueItems: [],
  providerChoiceRecoveryByItem: {},
  remoteQueueError: "",
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
  activeAgentViewModesById: {},
  terminalSnapshot: null,
  terminalLoading: false,
  terminalError: "",
  chatEvents: [],
  chatLoading: false,
  chatLoadingOlder: false,
  chatBrowsingOlder: false,
  chatHasOlder: false,
  chatNextBefore: null,
  chatPage: null,
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
        clearPairingUrl();
        set({ status: "pairing_expired" });
        return;
      }
      const nextStatus = statusFromError(error);
      if (nextStatus === "pairing_expired") clearPairingUrl();
      if (nextStatus === "device_revoked") await clearStoredRemoteIdentity();
      set({ status: nextStatus });
    }
  },
  async refresh() {
    try {
      await ensureAuthenticatedSession(set);
      await loadRemoteShellData(set, get);
    } catch (error) {
      closeStatusStream();
      const nextStatus = statusFromError(error);
      if (nextStatus === "pairing_expired") clearPairingUrl();
      if (nextStatus === "device_revoked") await clearStoredRemoteIdentity();
      set({ status: nextStatus });
    }
  },
  async refreshInbox() {
    return refreshRemoteQueue(set);
  },
  async runInboxAction(action, itemId, choice) {
    await remoteClient.runInboxAction(action, itemId, choice);
    await refreshRemoteQueue(set);
  },
  recordProviderChoiceRecovery(itemId, choice) {
    set((state) => ({
      providerChoiceRecoveryByItem: {
        ...state.providerChoiceRecoveryByItem,
        [itemId]: choice,
      },
    }));
  },
  disconnectStatusStream() {
    closeStatusStream();
    statusStreamReconnectAttempts = 0;
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
    retireActiveChatReads();
    chatRefreshRequestSerial += 1;
    chatWindowRefreshQueued = false; chatForceRecentRead = false;
    set({ chatPage: null, sending: false, chatBrowsingOlder: false });
    clearBackgroundChatRefresh(true);
    const activeAgent = get().agents.find((agent) => agent.session_id === id);
    lastActiveAgentRefreshKey = activeAgent ? activeAgentRefreshKey(activeAgent) : null;
    pushRemoteAgentDetailHistory(id);
    const activeAgentViewMode = get().activeAgentViewModesById[id] ?? get().remoteAgentDefaultViewMode;
    set({
      activeAgentId: id,
      activeAgentViewMode,
      terminalSnapshot: null,
      terminalLoading: false,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatLoadingOlder: false,
      chatHasOlder: false,
      chatNextBefore: null,
      chatError: "",
    });
    if (activeAgentViewMode === "chat") {
      await get().refreshActiveAgentChat();
    }
  },
  closeAgent(options) {
    retireActiveChatReads();
    chatRefreshRequestSerial += 1;
    chatWindowRefreshQueued = false; chatForceRecentRead = false;
    set({ chatPage: null, chatBrowsingOlder: false });
    if (options?.syncHistory !== false && isRemoteAgentDetailHistoryState()) {
      try {
        window.history.back();
      } catch {
        // If browser history cannot move, still close the in-app detail view.
      }
    }
    clearBackgroundChatRefresh(true);
    lastActiveAgentRefreshKey = null;
    set({
      activeAgentId: null,
      activeAgentViewMode: "terminal",
      terminalSnapshot: null,
      terminalLoading: false,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatLoadingOlder: false,
      chatHasOlder: false,
      chatNextBefore: null,
      chatError: "",
    });
  },
  async setActiveAgentViewMode(mode) {
    retireActiveChatReads();
    chatRefreshRequestSerial += 1;
    chatWindowRefreshQueued = false; chatForceRecentRead = false;
    set({ chatPage: null, chatBrowsingOlder: false, chatLoading: false, chatLoadingOlder: false });
    set((state) => ({
      activeAgentViewMode: mode,
      activeAgentViewModesById: state.activeAgentId
        ? { ...state.activeAgentViewModesById, [state.activeAgentId]: mode }
        : state.activeAgentViewModesById,
    }));
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
    const olderDemand = chatOlderReadInFlight;
    const interleavedRecent = olderDemand?.recentTurn === true;
    if (get().chatLoadingOlder && !interleavedRecent) {
      chatWindowRefreshQueued = true;
      return chatOlderReadInFlight?.promise;
    }
    const requestSerial = chatRefreshRequestSerial;
    const requestedWindow = chatWindowRequestSerial;
    if (chatReadInFlight?.agentId === activeAgentId && chatReadInFlight.serial === requestSerial) {
      // Any refresh demand needs one read after the pending snapshot settles.
      chatWindowRefreshQueued = true;
      return chatReadInFlight.promise;
    }
    const controller = new AbortController();
    const read = async () => {
    if (!options?.background) {
      set({ chatLoading: true, chatError: "" });
    }
    try {
      const previous = get().chatPage;
      const page = await remoteClient.loadAgentChatPage(activeAgentId, undefined,
        !chatForceRecentRead && previous?.session_id === activeAgentId && get().chatEvents.length > 0 ? previous.revision : undefined, undefined, controller.signal);
      if (page.reset && requestSerial === chatRefreshRequestSerial && requestedWindow === chatWindowRequestSerial
        && get().activeAgentId === activeAgentId && page.session_id === activeAgentId
        && (page.conversation_id !== get().chatPage?.conversation_id || page.generation !== get().chatPage?.generation
          || page.source_epoch !== get().chatPage?.source_epoch)) olderDemand?.settle();
      set((state) => {
        if (requestSerial !== chatRefreshRequestSerial || requestedWindow !== chatWindowRequestSerial) return {};
        if (state.activeAgentId !== activeAgentId) return { chatLoading: false };
        if (page.session_id !== activeAgentId) return {};
        const compatible = page.conversation_id === state.chatPage?.conversation_id
          && page.generation === state.chatPage?.generation && page.source_epoch === state.chatPage?.source_epoch;
        if (compatible && (interleavedRecent || (page.reset && state.chatBrowsingOlder))) {
          if (page.reset && page.events.length > 80) throw new Error("Recent snapshot exceeds the metadata page limit");
          const chatEvents = patchLoadedRemoteChatMembers(state.chatEvents, page);
          return { chatEvents, chatLoading: false, chatError: "",
            chatPage: state.chatPage ? { ...state.chatPage, revision: page.revision, progress: page.progress, next_before: state.chatNextBefore } : state.chatPage };
        }
        if (interleavedRecent && !page.reset) throw new Error("Conversation changed during recent read");
        if (!page.unchanged && (page.reset || !chatOlderCursor || chatOlderCursor.serial !== requestSerial || chatOlderCursor.generation !== page.generation)) chatOlderCursor = null;
        const nextBefore = chatOlderCursor ? chatOlderCursor.before : page.next_before;
        const nextPage = page.unchanged ? state.chatPage : { ...page, next_before: nextBefore };
        chatForceRecentRead = false;
        const chatBrowsingOlder = page.reset ? false : state.chatBrowsingOlder;
        const mergedChatEvents = applyChatPage(state.chatEvents, page, "recent", chatBrowsingOlder ? "older" : "recent");
        if (chatEventsEqual(state.chatEvents, mergedChatEvents)) {
          return {
            chatLoading: false,
            chatLoadingOlder: false,
            chatHasOlder: page.unchanged ? state.chatHasOlder : nextBefore !== null,
            chatNextBefore: page.unchanged ? state.chatNextBefore : nextBefore,
            chatPage: nextPage,
            chatBrowsingOlder,
            chatError: "",
          };
        }
        return {
          chatEvents: mergedChatEvents,
          chatLoading: false,
          chatLoadingOlder: false,
          chatHasOlder: nextBefore !== null,
          chatNextBefore: nextBefore,
          chatPage: nextPage,
          chatBrowsingOlder,
          chatError: "",
        };
      });
    } catch (error) {
      if (requestSerial !== chatRefreshRequestSerial || requestedWindow !== chatWindowRequestSerial) return;
      if (get().activeAgentId !== activeAgentId) return;
      const connectionStatus = chatConnectionStatusFromError(error);
      if (connectionStatus && connectionStatus !== "unreachable") {
        closeStatusStream();
        clearBackgroundChatRefresh();
        chatWindowRefreshQueued = false;
      }
      set({
        chatLoading: false,
        chatError: chatErrorMessage(error),
        ...(connectionStatus ? { status: connectionStatus } : {}),
      });
    } finally {
      if (chatReadInFlight?.serial === requestSerial && chatReadInFlight.agentId === activeAgentId
        && chatReadInFlight.window === requestedWindow) chatReadInFlight = null;
      if (chatWindowRefreshQueued && requestSerial === chatRefreshRequestSerial && get().activeAgentId === activeAgentId) {
        chatWindowRefreshQueued = false;
        if (get().status === "ready" && get().activeAgentViewMode === "chat") {
          if (chatForceRecentRead) void get().refreshActiveAgentChat();
          else scheduleBackgroundActiveChatRefresh(set, get);
        }
      }
    }
    };
    const promise = read();
    chatReadInFlight = { agentId: activeAgentId, serial: requestSerial, window: requestedWindow, controller, promise };
    return promise;
  },
  async loadOlderActiveAgentChat() {
    const pendingRecent = chatReadInFlight;
    if (pendingRecent?.agentId === get().activeAgentId && pendingRecent.serial === chatRefreshRequestSerial) {
      await pendingRecent.promise;
      if (pendingRecent.serial !== chatRefreshRequestSerial || pendingRecent.window !== chatWindowRequestSerial
        || pendingRecent.agentId !== get().activeAgentId) return;
    }
    const { activeAgentId, chatNextBefore, chatLoadingOlder } = get();
    if (chatOlderReadInFlight?.agentId === activeAgentId
      && chatOlderReadInFlight.serial === chatRefreshRequestSerial
      && chatOlderReadInFlight.window === chatWindowRequestSerial) return chatOlderReadInFlight.promise;
    if (!activeAgentId || chatNextBefore === null || chatLoadingOlder || chatReadInFlight?.serial === chatRefreshRequestSerial) return;
    const requestSerial = chatRefreshRequestSerial;
    const requestedWindow = chatWindowRequestSerial;
    const generation = get().chatPage?.generation;
    const conversation = get().chatPage?.conversation_id;
    const source = get().chatPage?.source_epoch;
    let complete = () => {};
    let settled = false;
    const read: OlderChatReadFlight = { agentId: activeAgentId, serial: requestSerial,
      window: requestedWindow, controller: new AbortController(),
      promise: new Promise<void>((resolve) => { complete = resolve; }), physical: null, recentDue: false, recentTurn: false,
      resume: async () => {}, settle: () => {} };
    const ownsDemand = () => requestSerial === chatRefreshRequestSerial
      && requestedWindow === chatWindowRequestSerial && get().activeAgentId === activeAgentId
      && get().chatPage?.generation === generation && get().chatPage?.conversation_id === conversation
      && get().chatPage?.source_epoch === source;
    read.settle = () => {
      if (settled) return;
      settled = true;
      if (chatOlderReadInFlight === read) chatOlderReadInFlight = null;
      if (requestSerial === chatRefreshRequestSerial && get().activeAgentId === activeAgentId) set({ chatLoadingOlder: false });
      complete();
    };
    chatOlderReadInFlight = read;
    set({ chatLoadingOlder: true, chatError: "" });
    read.resume = () => {
      if (read.physical) return read.physical;
      if (chatReadInFlight?.serial === requestSerial && chatReadInFlight.agentId === activeAgentId) return chatReadInFlight.promise;
      if (settled || read.controller.signal.aborted || !ownsDemand()) {
        read.settle(); return Promise.resolve();
      }
      lastBackgroundChatRefreshStartedAt = Date.now();
      read.physical = (async () => {
        try {
          const page = await remoteClient.loadAgentChatPage(activeAgentId, chatNextBefore, undefined, undefined, read.controller.signal);
          if (!ownsDemand() || page.session_id !== activeAgentId
            || (!page.reset && (page.generation !== generation || page.conversation_id !== conversation || page.source_epoch !== source))) {
            read.settle(); return;
          }
          if (!page.reset && !page.unchanged && page.events.length === 0
            && page.progress === "indexing" && page.next_before === chatNextBefore) {
            // Preserve the user's demand until the cold index has older rows, not just a successful response.
            read.recentDue = true;
            set((state) => ({ chatLoadingOlder: true, chatError: "",
              chatPage: state.chatPage ? { ...state.chatPage, progress: page.progress } : state.chatPage }));
            return;
          }
          set((state) => {
            if (requestSerial !== chatRefreshRequestSerial || requestedWindow !== chatWindowRequestSerial || state.activeAgentId !== activeAgentId || state.chatPage?.generation !== generation || page.session_id !== activeAgentId) return {};
            if (page.generation !== generation && !page.reset) return { chatLoadingOlder: false };
            if (!canAdmitOlderChatPage(page)) return { chatLoadingOlder: false,
              chatError: "Older history page exceeds the visible window. Retry without advancing history." };
            chatOlderCursor = page.reset ? null : { serial: requestSerial, generation: page.generation, before: page.next_before };
            return {
              chatEvents: applyChatPage(state.chatEvents, page, "older"),
              chatLoadingOlder: false,
              chatBrowsingOlder: !page.reset,
              chatHasOlder: page.next_before !== null,
              chatNextBefore: page.next_before,
              chatPage: page.reset ? page : state.chatPage,
              chatError: "",
            };
          });
          read.settle();
        } catch (error) {
          if (requestSerial !== chatRefreshRequestSerial || requestedWindow !== chatWindowRequestSerial) return;
          if (get().activeAgentId !== activeAgentId) return;
          const connectionStatus = chatConnectionStatusFromError(error);
          if (connectionStatus && connectionStatus !== "unreachable") {
            closeStatusStream();
            clearBackgroundChatRefresh();
            chatWindowRefreshQueued = false;
          }
          set({
            chatLoadingOlder: false,
            chatError: chatErrorMessage(error),
            ...(connectionStatus ? { status: connectionStatus } : {}),
          });
          read.settle();
        } finally {
          read.physical = null;
          if (!settled) {
            const status = get().status;
            if (ownsDemand() && get().activeAgentViewMode === "chat"
              && (status === "ready" || status === "loading" || status === "unreachable")) {
              if (status === "ready") scheduleBackgroundActiveChatRefresh(set, get);
            } else read.settle();
          }
          if (requestSerial === chatRefreshRequestSerial && get().activeAgentId === activeAgentId) {
            if (settled && chatWindowRefreshQueued) {
              chatWindowRefreshQueued = false;
              if (get().status === "ready" && get().activeAgentViewMode === "chat") {
                if (chatForceRecentRead) void get().refreshActiveAgentChat();
                else scheduleBackgroundActiveChatRefresh(set, get);
              }
            }
          }
        }
      })();
      return read.physical;
    };
    void read.resume();
    return read.promise;
  },
  jumpToLatestActiveAgentChat() {
    const state = get();
    if (!state.activeAgentId) return;
    const olderRead = chatOlderReadInFlight;
    chatWindowRequestSerial += 1; chatForceRecentRead = true; chatWindowRefreshQueued = true; chatOlderCursor = null;
    olderRead?.controller.abort(); olderRead?.settle();
    set({ chatEvents: state.chatEvents.filter((event) => event.metadata.optimistic === true),
      chatPage: state.chatPage ? { ...state.chatPage, events: [], next_before: null } : null,
      chatBrowsingOlder: false, chatHasOlder: false, chatNextBefore: null, chatLoading: true, chatError: "" });
    if (!chatReadInFlight && !olderRead?.physical) {
      chatWindowRefreshQueued = false; void get().refreshActiveAgentChat();
    }
  },
  async loadActiveAgentChatDetail(reference) {
    const agentId = get().activeAgentId;
    const serial = chatRefreshRequestSerial;
    const requestedWindow = chatWindowRequestSerial;
    const generation = get().chatPage?.generation;
    if (!agentId) throw new Error("Conversation is closed");
    const page = await remoteClient.loadAgentChatPage(agentId, undefined, undefined, reference);
    if (serial !== chatRefreshRequestSerial || requestedWindow !== chatWindowRequestSerial || get().activeAgentId !== agentId || get().chatPage?.generation !== generation
      || page.generation !== generation || page.session_id !== agentId || !page.detail) throw new Error("Conversation changed during detail read");
    return page.detail;
  },
  async sendPromptToActiveAgent(prompt, inputMode = "message") {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const activeAgentId = get().activeAgentId;
    if (!activeAgentId) return;
    if (get().chatBrowsingOlder) get().jumpToLatestActiveAgentChat();
    const submissionScope = chatRefreshRequestSerial;
    const submissionPage = get().chatPage;
    const isCurrentSubmission = (state: RemoteState) => submissionScope === chatRefreshRequestSerial
      && state.activeAgentId === activeAgentId
      && (!submissionPage?.conversation_id || !state.chatPage?.conversation_id
        || submissionPage.conversation_id === state.chatPage.conversation_id)
      && (!submissionPage?.source_epoch || !state.chatPage?.source_epoch
        || submissionPage.source_epoch === state.chatPage.source_epoch);
    set({ sending: true });
    try {
      const acknowledgement = await remoteClient.sendPrompt(activeAgentId, trimmed, inputMode);
      if (!isCurrentSubmission(get())) return;
      if (get().activeAgentViewMode === "chat") {
        if (inputMode === "message") {
          set((state) => {
            if (!isCurrentSubmission(state)) return {};
            const activeAgent = state.agents.find((agent) => agent.session_id === activeAgentId);
            return {
              chatEvents: addChatSubmission(state.chatEvents, state.chatPage,
                submittedChatEvent(activeAgentId, activeAgent?.provider ?? "unknown", trimmed, acknowledgement)),
            };
          });
        }
        await get().refreshActiveAgentChat();
      } else {
        await get().refreshActiveAgentTerminal();
      }
    } catch (error) {
      if (isCurrentSubmission(get())) set({ status: statusFromError(error) });
      throw error;
    } finally {
      if (submissionScope === chatRefreshRequestSerial) set({ sending: false });
    }
  },
  async sendPromptToAgent(sessionId, prompt, inboxItemId) {
    await remoteClient.sendPrompt(sessionId, prompt, "message", inboxItemId);
  },
  async runAgentAction(action, target) {
    try {
      await remoteClient.runAgentAction(action, target);
      if (get().activeAgentId === target) {
        if (action === "clear") {
          retireActiveChatReads();
          chatRefreshRequestSerial += 1;
          chatWindowRequestSerial += 1; chatForceRecentRead = true; chatWindowRefreshQueued = false;
          chatOlderCursor = null;
          set({
            chatPage: null,
            chatBrowsingOlder: false,
            sending: false,
            terminalSnapshot: null,
            terminalLoading: false,
            terminalError: "",
            chatEvents: [],
            chatLoading: false,
            chatLoadingOlder: false,
            chatHasOlder: false,
            chatNextBefore: null,
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
  async runAutomation(automationId) {
    try {
      await remoteClient.runAutomation(automationId);
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
}));
