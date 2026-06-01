import type { Blueprint } from '../builder/blueprintTypes';

export type NodeStatusKind = 'pending' | 'running' | 'completed' | 'failed' | 'skipped';
export type RunStatusKind = 'running' | 'awaiting_approval' | 'completed' | 'failed';

export type RunEvent = { seq: number; ts: string } & (
  | { kind: 'run_started'; blueprint_id: string; schema: number; trigger: unknown }
  | { kind: 'node_started'; node: string }
  | { kind: 'node_completed'; node: string; output: unknown }
  | { kind: 'node_failed'; node: string; error: string }
  | { kind: 'branch_taken'; node: string; port: string }
  | { kind: 'decision_made'; node: string; port: string }
  | { kind: 'loop_iteration'; node: string; iteration: number }
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
  status: RunStatusKind;
  node_count: number;
  failure?: string | null;
  path: string;
  started_at?: string | null;
  updated_at?: string | null;
  completed_at?: string | null;
}

export interface RunReadResult {
  state: RunState | null;
  events: RunEvent[];
  blueprint: Blueprint | null;
}
