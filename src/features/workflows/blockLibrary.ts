import { NodeType } from '../../types/workflow';

export interface BlockField {
  name: string;
  label: string;
  type: 'text' | 'textarea' | 'select' | 'code' | 'schema';
  options?: string[];
  placeholder?: string;
}

export interface BlockPorts {
  inputs: number;
  outputs: string[]; // Names/IDs for source handles (e.g. ['default'] or ['true', 'false'])
}

export interface BlockDefinition {
  type: NodeType;
  name: string;
  description: string;
  inputs: string;
  outputs: string;
  category: string;
  fields?: BlockField[];
  advancedFields?: BlockField[];
  ports?: BlockPorts;
}

const DEFAULT_PORTS: BlockPorts = { inputs: 1, outputs: ['default'] };
const EXECUTION_ADVANCED: BlockField[] = [
  { name: 'pre_hooks', label: 'Pre-Execution Hooks', type: 'schema', placeholder: '[]' },
  { name: 'post_hooks', label: 'Post-Execution Hooks', type: 'schema', placeholder: '[]' }
];

const SIMPLE_ADVANCED: BlockField[] = [
  { name: 'timeout_ms', label: 'Timeout', type: 'text', placeholder: '30000' }
];

export const BLOCK_LIBRARY: BlockDefinition[] = [
  // ... (Triggers remain the same)
  { 
    type: 'trigger', 
    name: 'Manual Trigger', 
    category: 'TRIGGER', 
    description: 'Fires the initial sequence pulse.', 
    inputs: 'Provided Payload', 
    outputs: 'Trigger Context', 
    ports: { inputs: 0, outputs: ['default'] },
    fields: [
      { name: 'input_schema', label: 'Input Schema', type: 'schema', placeholder: '{ "type": "object" }' }
    ]
  },
  { 
    type: 'trigger', 
    name: 'File Watcher', 
    category: 'TRIGGER', 
    description: 'Fires on file system events.', 
    inputs: 'Glob Pattern', 
    outputs: 'Trigger Context', 
    ports: { inputs: 0, outputs: ['default'] },
    fields: [
      { name: 'pattern', label: 'Glob Pattern', type: 'text', placeholder: 'src/**/*.rs' },
      { name: 'events', label: 'Events', type: 'select', options: ['create', 'modify', 'delete', 'all'] },
      { name: 'debounce_ms', label: 'Debounce', type: 'text', placeholder: '500' }
    ]
  },
  {
    type: 'trigger',
    name: 'Scheduled Trigger',
    category: 'TRIGGER',
    description: 'Runs on a scheduled interval or at a specific time.',
    inputs: 'Schedule Config',
    outputs: 'Trigger Context',
    ports: { inputs: 0, outputs: ['default'] },
    fields: [
      { name: 'schedule_type', label: 'Frequency', type: 'select', options: ['Minutes', 'Hours', 'Daily', 'Weekly', 'One-Time'] },
      { name: 'interval', label: 'Interval Value', type: 'text', placeholder: '5' },
      { name: 'time', label: 'Time', type: 'text', placeholder: '09:00' },
      { name: 'days', label: 'Days', type: 'text', placeholder: 'Mon,Tue,Wed' },
      { name: 'datetime', label: 'Date & Time', type: 'text', placeholder: '2026-03-27T09:00' }
    ]
  },
  
  // EXECUTION
  { 
    type: 'agent', 
    name: 'Agent', 
    category: 'EXECUTION', 
    description: 'Structured prompt execution via LLM.', 
    inputs: 'Registry Context', 
    outputs: 'Agent Result', 
    ports: DEFAULT_PORTS,
    fields: [
      { name: 'agent_id', label: 'Target Agent', type: 'select', placeholder: 'Select Agent' },
      { name: 'agent_class', label: 'Agent Class', type: 'select', placeholder: 'Select Class' },
      { name: 'prompt', label: 'Prompt Template', type: 'textarea', placeholder: 'Analyze {{nodes.step1.output}}' },
      { name: 'session_type', label: 'Session Type', type: 'select', options: ['persistent', 'temporary'] },
      { name: 'folder', label: 'Workspace Folder', type: 'text', placeholder: 'C:\\path\\to\\project' },
      { name: 'output_format', label: 'Output Format', type: 'select', options: ['text', 'json'] },
      { name: 'json_schema', label: 'JSON Schema', type: 'schema', placeholder: '{ "type": "object" }' }
    ],
    advancedFields: [
      ...EXECUTION_ADVANCED,
      ...SIMPLE_ADVANCED
    ]
  },
  { 
    type: 'command', 
    name: 'Shell Command', 
    category: 'EXECUTION', 
    description: 'Native PTY execution.', 
    inputs: 'Registry Context', 
    outputs: 'Cmd Result', 
    ports: DEFAULT_PORTS,
    fields: [
      { name: 'cmd', label: 'Command String', type: 'code', placeholder: 'npm run build' },
      { name: 'folder', label: 'Execution Directory', type: 'text', placeholder: 'C:\\path\\to\\project' },
      { name: 'env', label: 'Environment Variables', type: 'schema', placeholder: '{ "NODE_ENV": "production" }' }
    ],
    advancedFields: [
      ...EXECUTION_ADVANCED,
      ...SIMPLE_ADVANCED
    ]
  },
  { 
    type: 'script', 
    name: 'Script', 
    category: 'EXECUTION', 
    description: 'Isolated local file execution.', 
    inputs: 'Registry Context', 
    outputs: 'Script Result', 
    ports: DEFAULT_PORTS,
    fields: [
      { name: 'runtime', label: 'Runtime', type: 'select', options: ['python', 'node', 'sh'] },
      { name: 'file_path', label: 'File Path', type: 'text', placeholder: './scripts/test.py' },
      { name: 'args', label: 'Arguments', type: 'text', placeholder: '--verbose' },
      { name: 'folder', label: 'Execution Directory', type: 'text', placeholder: 'C:\\path\\to\\project' },
      { name: 'env', label: 'Environment Variables', type: 'schema', placeholder: '{ "DEBUG": "*" }' }
    ],
    advancedFields: SIMPLE_ADVANCED
  },
  { 
    type: 'tool', 
    name: 'Tool Call', 
    category: 'EXECUTION', 
    description: 'Direct MCP tool invocation.', 
    inputs: 'Registry Context', 
    outputs: 'Tool Result', 
    ports: DEFAULT_PORTS,
    fields: [{ name: 'tool_name', label: 'Tool Name', type: 'text', placeholder: 'google_web_search' }],
    advancedFields: SIMPLE_ADVANCED
  },

  // FLOW CONTROL
  { 
    type: 'logic', 
    name: 'Branch', 
    category: 'FLOW CONTROL', 
    description: 'Branching based on conditions.', 
    inputs: 'Registry Context', 
    outputs: 'True/False Path',
    ports: { inputs: 1, outputs: ['on_true', 'on_false'] },
    fields: [{ name: 'condition', label: 'JS Condition', type: 'text', placeholder: 'nodes.step1.output.ok' }]
  },
  { 
    type: 'loop', 
    name: 'Loop', 
    category: 'FLOW CONTROL', 
    description: 'Iterative execution pulse.', 
    inputs: 'Registry Context', 
    outputs: 'Body / Done', 
    ports: { inputs: 1, outputs: ['body', 'done'] },
    fields: [
      { name: 'mode', label: 'Loop Mode', type: 'select', options: ['count', 'conditional'] },
      { name: 'max_iterations', label: 'Max Iterations', type: 'text', placeholder: '10' },
      { name: 'condition', label: 'Condition', type: 'text', placeholder: 'nodes.step.output.iter < 5' },
      { name: 'iterator_name', label: 'Iterator Name', type: 'text', placeholder: 'iter' }
    ]
  },
  { 
    type: 'wait', 
    name: 'Wait', 
    category: 'FLOW CONTROL', 
    description: 'Synchronization barrier.', 
    inputs: 'Multiple Temporal signals', 
    outputs: 'Sync Stamp', 
    ports: { inputs: 1, outputs: ['default'] }, // Input is implicitly multi-wired
    fields: [{ name: 'timeout_ms', label: 'Timeout', type: 'text', placeholder: '30000' }]
  },
  { 
    type: 'subflow', 
    name: 'Sub-Flow', 
    category: 'FLOW CONTROL', 
    description: 'Invoke an existing workflow.', 
    inputs: 'Explicit Args', 
    outputs: 'Final Registry State', 
    ports: DEFAULT_PORTS,
    fields: [
      { name: 'workflow_id', label: 'Workflow ID', type: 'select', placeholder: 'Select Workflow' },
      { name: 'args', label: 'Arguments', type: 'schema', placeholder: '{ "key": "{{nodes.id.output.key}}" }' }
    ]
  },

  // PERSISTENCE
  { 
    type: 'memory', 
    name: 'KV Storage', 
    category: 'PERSISTENCE', 
    description: 'Read/Write to shared storage.', 
    inputs: 'Registry Context', 
    outputs: 'Result/Status', 
    ports: DEFAULT_PORTS,
    fields: [
      { name: 'operation', label: 'Operation', type: 'select', options: ['get', 'set', 'delete'] },
      { name: 'key', label: 'Key Path', type: 'text' },
      { name: 'value', label: 'Value', type: 'textarea', placeholder: '{{nodes.id.output}}' },
      { name: 'scope', label: 'Scope', type: 'select', options: ['workspace', 'run'] }
    ]
  },
  
  // COMMUNICATION
  { 
    type: 'communication', 
    name: 'Notify', 
    category: 'COMMUNICATION', 
    description: 'Send UI toast or message.', 
    inputs: 'Registry Context', 
    outputs: 'Delivery Status', 
    ports: DEFAULT_PORTS,
    fields: [{ name: 'message', label: 'Message Template', type: 'text' }]
  },
  { 
    type: 'communication', 
    name: 'Broadcast', 
    category: 'COMMUNICATION', 
    description: 'Unified message to all agents.', 
    inputs: 'Registry Context', 
    outputs: 'Status', 
    ports: DEFAULT_PORTS,
    fields: [{ name: 'prompt', label: 'Broadcast Prompt', type: 'textarea' }]
  },
];
