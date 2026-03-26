export type NodeType = 
  | 'trigger' 
  | 'agent' 
  | 'command' 
  | 'script' 
  | 'tool' 
  | 'logic' 
  | 'loop' 
  | 'wait' 
  | 'parallel' 
  | 'subflow' 
  | 'governance' 
  | 'memory' 
  | 'communication';

export type NodeStatus = 'idle' | 'processing' | 'completed' | 'failed' | 'blocked';

export interface NodeDependency {
  node_id: string;
  port: string;
}

export interface WorkflowNode {
  id: string;
  type: NodeType;
  name?: string;
  config: Record<string, any>;
  dependencies?: NodeDependency[];
  // For UI state tracking
  position?: { x: number; y: number };
}

export interface WorkflowSettings {
  max_iterations: number;
  on_limit_reached: 'pause' | 'terminate';
}

export interface WorkflowDefinition {
  id: string;
  name: string;
  settings: WorkflowSettings;
  nodes: WorkflowNode[];
  /** Maps role names to agent session IDs. Set before execution. */
  role_mappings?: Record<string, string>;
}

export interface WorkflowExecutionState {
  workflow_id: string;
  active_node_ids: string[];
  node_statuses: Record<string, NodeStatus>;
  node_outputs: Record<string, any>;
  start_timestamp: number;
  end_timestamp?: number;
}

export interface WorkflowTelemetryEvent {
  workflow_id: string;
  node_id: string;
  status: NodeStatus;
  output?: any;
  error?: string;
}

export type WorkflowTriggerStatus = 'active' | 'muted' | 'off';
export type WorkflowTriggerType = 'scheduled' | 'webhook' | 'watcher' | 'manual';

export interface WorkflowSummary {
  id: string;
  name: string;
  trigger_type: WorkflowTriggerType;
  trigger_status: WorkflowTriggerStatus;
}

export interface ScheduleDefinition {
  schedule_type: "one_time" | "minutes" | "hours" | "daily" | "weekly";
  value: string;
  active: boolean;
}

export interface ScheduledRun {
  id: string;
  workflow_id: string;
  workflow_name: string;
  schedule: ScheduleDefinition;
  role_mappings: Record<string, string>;
  /** Human-readable description (e.g. "Every 5m", "Daily at 09:00") */
  description?: string;
  next_run_epoch_ms: number | null;
  is_paused: boolean;
}

export interface ActiveRunTracker {
  run_id: string;
  workflow_id: string;
  workflow_name: string;
  current_step: number;
  total_steps: number;
  active_node_name: string;
}
