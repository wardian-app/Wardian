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

export type WorkflowAgentMode = 'ephemeral' | 'inherit_fresh' | 'inherit_resume';

export interface NodeDependency {
  node_id: string;
  port: string;
}

export interface WorkflowNode {
  id: string;
  type: NodeType;
  name?: string;
  config: Record<string, any>;
  parameter_schema?: Record<string, any>;
  dependencies?: NodeDependency[];
  // For UI state tracking
  position?: { x: number; y: number };
}

export interface WorkflowAgentNodeConfig {
  mode?: WorkflowAgentMode;
  /** Legacy field retained for migration from older saved workflows. */
  session_type?: 'temporary' | 'persistent';
  agent_id?: string;
  agent_class?: string;
  folder?: string;
  prompt?: string;
  output_format?: 'text' | 'json';
  json_schema?: Record<string, unknown>;
  timeout_ms?: number | string;
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
  schedule_type: "interval" | "daily" | "weekly" | "monthly" | "specific_dates" | "one_time";
  interval_minutes?: number;
  time_of_day?: string;           // "HH:MM"
  days_of_week?: string[];        // ["Mon","Tue","Fri"]
  repeat_every?: number;          // every N weeks (default 1)
  days_of_month?: number[];       // [1, 15] for monthly
  specific_dates?: string[];      // ["2026-05-01"] for specific_dates
  run_at?: string;                // ISO datetime for one_time
  end_condition?: "never" | "on_date" | "after_occurrences";
  end_date?: string;              // YYYY-MM-DD
  max_occurrences?: number;
  occurrence_count?: number;
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
  paused_remaining_ms?: number | null;
  is_paused: boolean;
}

export interface ActiveRunTracker {
  run_instance_id: string;
  scheduled_run_id?: string | null;
  workflow_id: string;
  workflow_name: string;
  current_step: number;
  total_steps: number;
  active_node_name: string;
}
