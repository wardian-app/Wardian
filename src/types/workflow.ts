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

export type NodeStatus = 'idle' | 'processing' | 'completed' | 'failed';

export interface WorkflowNode {
  id: string;
  type: NodeType;
  name?: string;
  config: Record<string, any>;
  depends_on?: string[];
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
