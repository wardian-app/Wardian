import type { AgentTeam, Watchlist, WatchlistPrefs } from "../layout/watchlist/types";
import type {
  TerminalActivationAckResult,
  TerminalActivationBeginResult,
  TerminalBrokerEvent,
  TerminalBrokerState,
  TerminalEventBatch,
  TerminalEventAckResult,
  TerminalGeometryCommitResult,
  TerminalLeaseDecision,
  TerminalOwnerResyncAckResult,
  TerminalOwnerResyncBeginResult,
  TerminalPresentationRegistrationResult,
  TerminalPresentationState,
  TerminalPresentationUpdateResult,
  TerminalSnapshot,
} from "./index";

export type RemoteAccessStatus = "disabled" | "enabled" | "needs_repair";
export type RemoteCredentialKind = "pwa" | "client";
export type RemoteCredentialAudience = "wardian_remote_pwa" | "wardian_remote_client_v1";
export type RemoteReadGrant = "agents:read" | "chat:read";
export type RemoteSendGrant = "chat:send";
export type RemoteClientScope = RemoteReadGrant | RemoteSendGrant;

export interface RemoteGatewayConfig {
  schema_version: 1;
  enabled: boolean;
  canonical_origin: string;
  client_origin_allowlist: string[];
  loopback_host: string;
  loopback_port: number;
  gateway_identity_public_key: string;
  gateway_identity_fingerprint: string;
}

export type RemoteSetupOverallStatus = "disabled" | "needs_action" | "ready";

export type RemoteSetupCheckStatus = "ok" | "warning" | "error";

export interface RemoteSetupCheck {
  id: string;
  label: string;
  status: RemoteSetupCheckStatus;
  message: string;
  details: string | null;
}

export interface RemoteSetupCommandHint {
  label: string;
  command: string;
}

export interface RemoteSetupCheckResult {
  overall_status: RemoteSetupOverallStatus;
  checks: RemoteSetupCheck[];
  inferred_origin: string | null;
  serve_target: string | null;
  setup_command: RemoteSetupCommandHint | null;
}

export interface PairingQrPayload {
  gateway_origin: string;
  pairing_offer_id: string;
  expires_at: string;
  nonce: string;
  server_identity_fingerprint: string;
}

export interface RemoteClientPairingQrPayload extends PairingQrPayload {
  credential_kind: "client";
  client_origin: string;
  read_grants: RemoteReadGrant[];
}

export interface PairingSubmitResponse {
  status: "pending" | "approved" | "rejected";
  pairing_request_id: string;
  device_id: string;
  public_key_fingerprint: string;
  paired_at: string | null;
  expires_at: string;
}

/**
 * @ipcContract Rust struct RemoteClientPairingSubmitResponse; remote-client pairing-submit HTTP response.
 */
export interface RemoteClientPairingSubmitResponse {
  status: "pending" | "approved" | "rejected";
  pairing_request_id: string;
  device_id: string;
  public_key_fingerprint: string;
  expires_at: string;
}

export interface RemoteDeviceRecord {
  device_id: string;
  label: string;
  public_key_spki_der_base64: string;
  public_key_fingerprint: string;
  credential_kind: RemoteCredentialKind;
  audience: RemoteCredentialAudience;
  client_origin: string | null;
  read_grants: RemoteReadGrant[];
  send_grants: RemoteSendGrant[];
  created_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
}

export interface RemotePendingPairingRequest {
  request_id: string;
  device_label: string;
  public_key_fingerprint: string;
  canonical_origin: string;
  credential_kind: RemoteCredentialKind;
  audience: string;
  client_origin: string | null;
  read_grants: RemoteReadGrant[];
  send_grants: RemoteSendGrant[];
  submitted_at: string;
  expires_at: string;
}

export interface RemoteAgentSummary {
  session_id: string;
  session_name: string;
  agent_class: string;
  provider: string;
  workspace: string;
  status: string;
  latest_text: string | null;
}

export interface RemoteTerminalSnapshot {
  cursor: string;
  text: string;
  truncated: boolean;
  omitted_bytes: number;
}

export interface RemoteTerminalErrorMessage {
  type: "error";
  code: string;
  fatal?: boolean;
  decision?: TerminalLeaseDecision;
}

export type RemoteTerminalBrokerEvent = Exclude<TerminalBrokerEvent, { type: "output" }> | {
  type: "output";
  sequence: number;
  runtime_generation: number;
  bytes_base64: string;
};

export type RemoteTerminalEventBatch = Omit<TerminalEventBatch, "events"> & {
  events: RemoteTerminalBrokerEvent[];
};

export type RemoteTerminalRegisteredMessage = {
  type: "registered";
  protocol_version: 2;
} & TerminalPresentationRegistrationResult;

export type RemoteTerminalPresentationStateMessage = {
  type: "presentation_state";
  presentation: TerminalPresentationState;
  broker_state?: TerminalPresentationUpdateResult["broker_state"];
};

export type RemoteTerminalActivationBeginMessage = {
  type: "activation_begin";
  result: TerminalActivationBeginResult;
};

export type RemoteTerminalActivationAckMessage = {
  type: "activation_ack";
  result: TerminalActivationAckResult;
};

export type RemoteTerminalOwnerResyncBeginMessage = {
  type: "owner_resync_begin";
  result: TerminalOwnerResyncBeginResult;
};

export type RemoteTerminalOwnerResyncAckMessage = {
  type: "owner_resync_ack";
  result: TerminalOwnerResyncAckResult;
};

export type RemoteTerminalInputResultMessage = {
  type: "input_result";
  decision: TerminalLeaseDecision;
};

export type RemoteTerminalResizeResultMessage = {
  type: "resize_result";
  result: TerminalGeometryCommitResult;
};

export type RemoteTerminalBrokerSnapshotMessage = {
  type: "snapshot";
  snapshot: TerminalSnapshot;
};

export type RemoteTerminalEventsMessage = {
  type: "events";
  batch: RemoteTerminalEventBatch;
};

export type RemoteTerminalEventsAckMessage = {
  type: "events_ack";
  result: TerminalEventAckResult;
};

export type RemoteTerminalDetachedMessage = { type: "detached" };

export type RemoteTerminalStreamMessage =
  | RemoteTerminalErrorMessage
  | RemoteTerminalRegisteredMessage
  | RemoteTerminalPresentationStateMessage
  | RemoteTerminalActivationBeginMessage
  | RemoteTerminalActivationAckMessage
  | RemoteTerminalOwnerResyncBeginMessage
  | RemoteTerminalOwnerResyncAckMessage
  | RemoteTerminalInputResultMessage
  | RemoteTerminalResizeResultMessage
  | RemoteTerminalBrokerSnapshotMessage
  | RemoteTerminalEventsMessage
  | RemoteTerminalEventsAckMessage
  | RemoteTerminalDetachedMessage;

export type RemoteTerminalPresentationMode = "owner" | "mirror" | "connecting";

export type RemoteTerminalV2ClientMessage =
  | { type: "report_viewport"; runtime_generation: number; cols: number; rows: number }
  | {
      type: "set_presentation_state";
      runtime_generation: number;
      observed_lease_epoch: number;
      visibility: "visible" | "hidden";
      render_state: "mounted" | "suspended";
      requested_interaction: "interactive" | "read_only";
      cols?: number;
      rows?: number;
    }
  | { type: "begin_activation"; runtime_generation: number; observed_lease_epoch: number }
  | {
      type: "ack_activation";
      runtime_generation: number;
      lease_epoch: number;
      activation_id: string;
    }
  | { type: "begin_owner_resync"; runtime_generation: number; lease_epoch: number }
  | {
      type: "ack_owner_resync";
      runtime_generation: number;
      lease_epoch: number;
      resync_id: string;
    }
  | { type: "input"; runtime_generation: number; lease_epoch: number; data: string }
  | { type: "binary"; runtime_generation: number; lease_epoch: number; data_base64: string }
  | {
      type: "resize";
      runtime_generation: number;
      lease_epoch: number;
      geometry_sequence: number;
      cols: number;
      rows: number;
    }
  | { type: "request_snapshot" }
  | {
      type: "request_events";
      runtime_generation: number;
      after_sequence: number;
    }
  | { type: "ack_events"; runtime_generation: number; applied_sequence: number }
  | { type: "detach" };

export type RemoteTerminalV2State = {
  presentation: TerminalPresentationState | null;
  broker_state: TerminalBrokerState | null;
  mode: RemoteTerminalPresentationMode;
  applied_sequence: number;
};

export type RemoteAgentInputMode = "message" | "command";

export interface RemoteAgentActionRequest {
  action: string;
  target: string;
  prompt?: string;
  input_mode?: RemoteAgentInputMode;
  inbox_item_id?: string;
}

export interface RemoteAutomationSummary {
  id: string;
  name: string;
  node_count: number;
}

export type RemoteAutomationRunStatus = "running" | "awaiting_approval" | "completed" | "failed";

export interface RemoteAutomationMonitorRun {
  run_id: string;
  blueprint_id: string;
  automation_name: string;
  schedule_id: string | null;
  status: RemoteAutomationRunStatus;
  node_count: number;
  completed_node_count: number | null;
  failure: string | null;
  started_at: string | null;
  updated_at: string | null;
  completed_at: string | null;
}

export interface RemoteAutomationMonitorSchedule {
  id: string;
  blueprint_id: string;
  automation_name: string;
  schedule: import("./automation").ScheduleDefinition;
  next_run_epoch_ms: number | null;
  is_paused: boolean;
  last_run_status: RemoteAutomationRunStatus | null;
  last_run_error: string | null;
  last_run_epoch_ms: number | null;
  target_labels: string[];
}

export interface RemoteAutomationMonitorSnapshot {
  schema_version: 1;
  generated_at: string;
  active_runs: RemoteAutomationMonitorRun[];
  active_runs_truncated: boolean;
  active_runs_next_offset: number | null;
  recent_runs: RemoteAutomationMonitorRun[];
  recent_runs_truncated: boolean;
  recent_runs_next_offset: number | null;
  schedules: RemoteAutomationMonitorSchedule[];
  schedules_truncated: boolean;
  schedules_next_offset: number | null;
}

export interface RemoteWatchlistResponse {
  watchlists: Watchlist[];
  teams: AgentTeam[];
  prefs: WatchlistPrefs | null;
}

export interface RemoteAutomationRunRequest {
  automation_id: string;
  payload?: unknown;
}

export interface RemoteAutomationStopRequest {
  run_instance_id: string;
}

export interface AuthSessionResponse {
  csrf_nonce: string;
  expires_at: string;
  absolute_expires_at: string;
}

export interface AuthChallengeResponse {
  challenge_id: string;
  device_id: string;
  origin: string;
  server_identity_fingerprint: string;
  nonce: string;
  expires_at: string;
  audience: string;
}

/**
 * @ipcContract Rust struct RemoteClientAuthChallengeResponse; remote-client auth-challenge HTTP response.
 */
export interface RemoteClientAuthChallengeResponse {
  challenge_id: string;
  device_id: string;
  origin: string;
  client_origin: string;
  server_identity_fingerprint: string;
  nonce: string;
  expires_at: string;
  audience: "wardian_remote_client_v1";
  signing_payload: string;
}

/**
 * @ipcContract Rust struct RemoteClientAuthSessionResponse; remote-client auth-session HTTP response.
 */
export interface RemoteClientAuthSessionResponse {
  token_type: "Bearer";
  access_token: string;
  expires_at: string;
  device_id: string;
  scopes: RemoteClientScope[];
  idempotency_namespace?: string;
  idempotency_expires_at?: string;
}

/**
 * @ipcContract Rust struct RemoteClientSharingStatus; remote-client sharing-status HTTP response.
 */
export interface RemoteClientSharingStatus {
  status: "active";
  device_id: string;
  client_origin: string;
  audience: "wardian_remote_client_v1";
  read_grants: RemoteReadGrant[];
  send_grants: RemoteSendGrant[];
  expires_at: string;
}

/**
 * @ipcContract Rust struct RemoteClientMessageSubmitRequest; remote-client message-submit HTTP request.
 */
export interface RemoteClientMessageSubmitRequest {
  text: string;
  input_mode: "message";
}

/**
 * @ipcContract Rust struct RemoteClientMessageOperation; remote-client message-operation HTTP response.
 */
export interface RemoteClientMessageOperation {
  operation_id: string;
  request_id: string;
  session_id: string;
  delivery_state: string;
  accepted_at: string;
  updated_at: string;
  idempotency_expires_at: string;
  error_code: string | null;
}

export interface RemoteWebSocketTicketResponse {
  ticket: string;
  expires_at: string;
}
