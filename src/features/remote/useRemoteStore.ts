import { create } from "zustand";
import type { RemoteAgentSummary, RemoteWorkflowSummary } from "../../types";
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
  | "device_revoked";

interface RemoteState {
  agents: RemoteAgentSummary[];
  workflows: RemoteWorkflowSummary[];
  status: RemoteStatus;
  selectedAgentIds: Set<string>;
  sending: boolean;
  load: () => Promise<void>;
  disconnectStatusStream: () => void;
  toggleAgent: (id: string) => void;
  sendPrompt: (prompt: string) => Promise<void>;
  broadcastPrompt: (prompt: string) => Promise<void>;
  runAgentAction: (action: string, target: string) => Promise<void>;
  runWorkflow: (workflowId: string) => Promise<void>;
}

type RemoteSet = (
  partial: Partial<RemoteState> | ((state: RemoteState) => Partial<RemoteState>),
) => void;

const statusFromError = (error: unknown): RemoteStatus =>
  error instanceof RemoteRequestError && error.status === 401 ? "session_expired" : "unreachable";

class RemotePairingExpiredError extends Error {}
class RemotePairingRejectedError extends Error {}

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

const closeStatusStream = () => {
  statusStreamSocket?.close();
  statusStreamSocket = null;
};

const ensureStatusStream = async (set: RemoteSet) => {
  if (statusStreamSocket) return;
  statusStreamSocket = await remoteClient.openStatusStream({
    onAgents: (agents) => set({ agents, status: "ready" }),
    onSessionExpired: () => {
      closeStatusStream();
      set({ status: "session_expired" });
    },
    onError: () => {
      closeStatusStream();
    },
    onClose: () => {
      statusStreamSocket = null;
    },
  });
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

const loadRemoteShellData = async (set: RemoteSet) => {
  const [agents, workflows] = await Promise.all([remoteClient.listAgents(), remoteClient.listWorkflows()]);
  set((state) => {
    const liveAgentIds = new Set(agents.map((agent) => agent.session_id));
    const selectedAgentIds = new Set([...state.selectedAgentIds].filter((id) => liveAgentIds.has(id)));
    return { agents, workflows, status: "ready", selectedAgentIds };
  });
  void ensureStatusStream(set).catch((error: unknown) => handleStatusStreamOpenFailure(set, error));
};

export const useRemoteStore = create<RemoteState>((set, get) => ({
  agents: [],
  workflows: [],
  status: "loading",
  selectedAgentIds: new Set(),
  sending: false,
  async load() {
    set({ status: "loading" });
    try {
      await pairFromUrl(set);
      await ensureAuthenticatedSession(set);
      await loadRemoteShellData(set);
    } catch (error) {
      closeStatusStream();
      if (error instanceof RemotePairingRejectedError) {
        set({ status: "device_revoked" });
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
  toggleAgent(id) {
    set((state) => {
      const selectedAgentIds = new Set(state.selectedAgentIds);
      if (selectedAgentIds.has(id)) selectedAgentIds.delete(id);
      else selectedAgentIds.add(id);
      return { selectedAgentIds };
    });
  },
  async sendPrompt(prompt) {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    const targets = get().selectedAgentIds;
    const agentIds = [...targets];
    if (agentIds.length === 0) return;
    set({ sending: true });
    try {
      await sendPromptToTargets(trimmed, agentIds);
      await get().load();
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
      await get().load();
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
      await get().load();
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
  async runWorkflow(workflowId) {
    try {
      await remoteClient.runWorkflow(workflowId);
      await get().load();
    } catch (error) {
      set({ status: statusFromError(error) });
      throw error;
    }
  },
}));
