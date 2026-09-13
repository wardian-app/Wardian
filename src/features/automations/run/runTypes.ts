import type { Blueprint } from '../builder/blueprintTypes';

export type NodeStatusKind = 'pending' | 'running' | 'completed' | 'failed' | 'skipped';
export type RunStatusKind = 'running' | 'awaiting_approval' | 'completed' | 'failed';

export type RunEvent = { seq: number; ts: string } & (
  | { kind: 'run_started'; run_id?: string; blueprint_hash?: string; blueprint_id: string; schema: number; trigger: unknown }
  | { kind: 'node_started'; node: string }
  | { kind: 'node_completed'; node: string; output: unknown }
  | { kind: 'decision_completed'; node: string; output: unknown; port: string }
  | { kind: 'state_updated'; node: string; op: string; entries: unknown }
  | { kind: 'notification'; node: string; message: string }
  | { kind: 'node_failed'; node: string; error: string }
  | { kind: 'branch_taken'; node: string; port: string }
  | { kind: 'decision_made'; node: string; port: string }
  | { kind: 'loop_iteration'; node: string; iteration: number }
  | { kind: 'loop_completed'; node: string }
  | { kind: 'node_skipped'; node: string }
  | { kind: 'awaiting_approval'; node: string }
  | { kind: 'approval_granted'; node: string; actor: string; note?: string | null }
  | { kind: 'approval_rejected'; node: string; actor: string; note?: string | null }
  | { kind: 'run_completed' }
  | { kind: 'run_failed'; error: string }
);

export interface RunState {
  run_id: string;
  blueprint_id: string;
  blueprint_hash?: string | null;
  status: RunStatusKind;
  nodes: Record<string, NodeStatusKind>;
  registry?: unknown;
  loop_iter?: Record<string, number>;
  delivered?: Record<string, number[]>;
  skipped_edges?: number[];
  next_seq?: number;
  failure?: string | null;
}

export interface RunSummary {
  run_id: string;
  blueprint_id: string;
  schedule_id?: string | null;
  status: RunStatusKind;
  node_count: number;
  failure?: string | null;
  path: string;
  blueprint_path?: string | null;
  started_at?: string | null;
  updated_at?: string | null;
  completed_at?: string | null;
  worker_attention_count?: number;
}

export type TemporaryWorkerState = 'requested' | 'running' | 'waiting' | 'succeeded' | 'failed' | 'cancelled' | 'unknown';

export interface TemporaryWorker {
  worker_id: string;
  kind: 'automation' | 'provider_child';
  provider: string;
  workspace: string;
  root_agent_id?: string | null;
  parent_worker_id?: string | null;
  parent_provider_session_id?: string | null;
  blueprint_id?: string | null;
  run_id?: string | null;
  node_id?: string | null;
  attempt?: number | null;
  provider_session_id?: string | null;
  runtime_session_id: string;
  runtime_generation?: number | null;
  state: TemporaryWorkerState;
  outcome?: string | null;
  capabilities: {
    inspection: boolean;
    follow_up: boolean;
    interruption: boolean;
    resume: boolean;
    source: string;
  };
  coverage: string;
  source_key?: string | null;
  source_path?: string | null;
  requested_at: string;
  started_at?: string | null;
  terminal_at?: string | null;
  last_observed_at?: string;
  last_follow_up_accepted_at?: string | null;
  resumable_until?: string | null;
  detail_retained_until?: string | null;
  error?: string | null;
}

export interface TemporaryWorkerTelemetry {
  worker_id: string;
  turns: number;
  tokens: {
    input_tokens?: number | null;
    cached_input_tokens?: number | null;
    cache_write_tokens?: number | null;
    output_tokens?: number | null;
    reasoning_tokens?: number | null;
  };
  models: string[];
  efforts: string[];
}

export interface RunSummaryListResult {
  runs: RunSummary[];
  truncated: boolean;
  next_offset?: number | null;
}

export interface RunReadResult {
  state: RunState | null;
  events: RunEvent[];
  blueprint: Blueprint | null;
  blueprint_path?: string | null;
  workers: TemporaryWorker[];
  worker_telemetry: Record<string, TemporaryWorkerTelemetry>;
}
