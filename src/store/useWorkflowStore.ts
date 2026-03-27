import { create } from 'zustand';
import { 
  Node, 
  Edge, 
  OnNodesChange, 
  OnEdgesChange, 
  applyNodeChanges, 
  applyEdgeChanges,
  Connection,
  addEdge,
} from '@xyflow/react';
import { invoke } from '@tauri-apps/api/core';
import { WorkflowDefinition, NodeStatus, WorkflowTelemetryEvent, ScheduledRun, ActiveRunTracker } from '../types/workflow';
import { AgentConfig, AgentClassDefinition } from '../types';
import { BLOCK_LIBRARY } from '../features/workflows/blockLibrary';

interface WorkflowState {
  nodes: Node[];
  edges: Edge[];
  activeWorkflowId: string | null;
  nodeStatuses: Record<string, NodeStatus>;
  availableWorkflows: WorkflowDefinition[];
  agents: AgentConfig[];
  agentClasses: AgentClassDefinition[];
  isSaving: boolean;
  activeRuns: ActiveRunTracker[];
  scheduledRuns: ScheduledRun[];
  
  // Actions
  onNodesChange: OnNodesChange;
  onEdgesChange: OnEdgesChange;
  onConnect: (connection: Connection) => void;
  setNodes: (nodes: Node[]) => void;
  setEdges: (edges: Edge[]) => void;
  setAgents: (agents: AgentConfig[]) => void;
  setAgentClasses: (classes: AgentClassDefinition[]) => void;
  
  fetchWorkflows: () => Promise<void>;
  loadWorkflow: (workflow: WorkflowDefinition) => void;
  saveWorkflow: (workflow: WorkflowDefinition) => Promise<void>;
  deleteWorkflow: (id: string) => Promise<void>;
  updateNodeStatus: (nodeId: string, status: NodeStatus, firedPorts?: string[]) => void;
  updateNodeConfig: (nodeId: string, key: string, value: any) => void;
  updateActiveWorkflowName: (name: string) => void;
  duplicateNode: (nodeId: string) => void;
  handleTelemetry: (event: WorkflowTelemetryEvent) => void;
  runActiveWorkflow: (payload?: any) => Promise<void>;
  runWorkflowById: (id: string, payload?: any) => Promise<void>;
  stopAllTriggers: () => Promise<void>;
  stopWorkflowTriggers: (workflowId: string) => Promise<void>;
  stopWorkflowRun: (workflowId: string) => Promise<void>;
  pauseAllTriggers: () => Promise<void>;
  resumeAllTriggers: () => Promise<void>;
  clearActiveWorkflow: () => void;
  handleProgress: (event: any) => void;
  handleStatusUpdate: (event: any) => void;
  loadScheduledRuns: () => Promise<void>;
  createScheduledRun: (run: ScheduledRun) => Promise<void>;
  deleteScheduledRun: (runId: string) => Promise<void>;
  toggleScheduledRun: (runId: string) => Promise<void>;
}

export const useWorkflowStore = create<WorkflowState>((set, get) => ({
  nodes: [],
  edges: [],
  activeWorkflowId: null,
  nodeStatuses: {},
  availableWorkflows: [],
  agents: [],
  agentClasses: [],
  isSaving: false,
  activeRuns: [],
  scheduledRuns: [],

  onNodesChange: (changes) => {
    set({
      nodes: applyNodeChanges(changes, get().nodes),
    });
  },
  onEdgesChange: (changes) => {
    set({
      edges: applyEdgeChanges(changes, get().edges),
    });
  },
  onConnect: (connection) => {
    set({
      edges: addEdge({ 
        ...connection, 
        style: { stroke: '#4b5563', strokeWidth: 2 },
        animated: false 
      }, get().edges),
    });
  },
  setNodes: (nodes) => set({ nodes }),
  setEdges: (edges) => set({ edges }),
  setAgents: (agents) => set({ agents }),
  setAgentClasses: (agentClasses) => set({ agentClasses }),

  fetchWorkflows: async () => {
    try {
      const workflows = await invoke<WorkflowDefinition[]>("list_workflows");
      set({ availableWorkflows: workflows });
    } catch (err) {
      console.error("Failed to fetch workflows:", err);
    }
  },

  loadWorkflow: (workflow) => {
    const nodes: Node[] = workflow.nodes.map((node, index) => {
      const blockDef = BLOCK_LIBRARY.find(b => b.type === node.type && (node.name ? b.name === node.name : true));
      return {
        id: node.id,
        type: node.type,
        position: node.position || { x: 100 + (index * 250), y: 100 + (index % 2 * 50) },
        data: { 
          label: node.name || node.id, 
          config: node.config,
          type: node.type,
          blockName: blockDef?.name,
          inputs: blockDef?.inputs || 'None',
          outputs: blockDef?.outputs || 'JSON'
        },
      };
    });

    const edges: Edge[] = [];
    workflow.nodes.forEach((node) => {
      // Support new dependencies structure
      if (node.dependencies) {
        node.dependencies.forEach((dep) => {
          edges.push({
            id: `e-${dep.node_id}-${node.id}-${dep.port}`,
            source: dep.node_id,
            sourceHandle: dep.port,
            target: node.id,
            style: { stroke: '#4b5563', strokeWidth: 2 },
            animated: false,
          });
        });
      }
      
      // Fallback for any legacy files that might still have depends_on
      const legacyDeps = (node as any).depends_on;
      if (legacyDeps && Array.isArray(legacyDeps)) {
        legacyDeps.forEach((sourceId) => {
          edges.push({
            id: `e-${sourceId}-${node.id}-default`,
            source: sourceId,
            sourceHandle: 'default',
            target: node.id,
            style: { stroke: '#4b5563', strokeWidth: 2 },
            animated: false,
          });
        });
      }
    });

    set({ 
      nodes, 
      edges, 
      activeWorkflowId: workflow.id,
      nodeStatuses: {} 
    });
  },

  saveWorkflow: async (workflow) => {
    set({ isSaving: true });
    try {
      await invoke("save_workflow", { workflow });
      await get().fetchWorkflows();
      // Optional: show success state for a moment
      setTimeout(() => set({ isSaving: false }), 500);
    } catch (err) {
      console.error("Failed to save workflow:", err);
      set({ isSaving: false });
    }
  },

  deleteWorkflow: async (id) => {
    try {
      await invoke("delete_workflow", { id }); 
      if (get().activeWorkflowId === id) {
        get().clearActiveWorkflow();
      }
      await get().fetchWorkflows();
    } catch (err) {
      console.error("Failed to delete workflow:", err);
    }
  },

  updateNodeStatus: (nodeId, status, firedPorts) => {
    set((state) => {
      const newNodeStatuses = {
        ...state.nodeStatuses,
        [nodeId]: status
      };

      // Update edges:
      // 1. If a node is 'processing', its INCOMING edges should be animated.
      // 2. If a node is 'completed', its OUTGOING edges (if in firedPorts) should be colored and animated.
      const newEdges = state.edges.map(edge => {
        let updated = { ...edge };
        
        // Incoming to processing node
        if (edge.target === nodeId) {
          updated.animated = status === 'processing';
        }

        // Outgoing from completed node
        if (edge.source === nodeId && status === 'completed') {
          if (firedPorts && firedPorts.includes(edge.sourceHandle || 'default')) {
            const isLoopBody = edge.sourceHandle === 'body';
            const isLoopDone = edge.sourceHandle === 'done';
            
            const color = edge.sourceHandle === 'on_true' ? '#10b981' : 
                          edge.sourceHandle === 'on_false' ? '#EF4444' : 
                          isLoopBody ? '#10b981' :
                          isLoopDone ? '#EF4444' : '#22d3ee';
            
            updated.style = { ...edge.style, stroke: color, strokeWidth: isLoopBody ? 4 : 3 };
            updated.animated = true;
            
            // Apply strobe effect to the loop backlink (body port)
            updated.className = isLoopBody ? 'edge-strobe' : '';
          } else {
            // Port not fired: Reset animation and style
            updated.animated = false;
            updated.className = '';
            updated.style = { ...edge.style, stroke: '#4b5563', strokeWidth: 2 };
          }
        }

        return updated;
      });

      return {
        nodeStatuses: newNodeStatuses,
        edges: newEdges
      };
    });
  },

  updateNodeConfig: (nodeId, key, value) => {
    set((state) => ({
      nodes: state.nodes.map(n => 
        n.id === nodeId 
          ? { ...n, data: { ...n.data, config: { ...(typeof n.data.config === 'object' && n.data.config !== null ? n.data.config : {}), [key]: value } } }
          : n
      )
    }));
  },

  updateActiveWorkflowName: (name) => {
    set((state) => ({
      availableWorkflows: state.availableWorkflows.map(w => 
        w.id === state.activeWorkflowId ? { ...w, name } : w
      )
    }));
  },

  duplicateNode: (nodeId) => {
    const { nodes } = get();
    const node = nodes.find(n => n.id === nodeId);
    if (!node) return;

    const id = `${node.type}-${Date.now()}`;
    const newNode: Node = {
      ...node,
      id,
      position: { x: node.position.x + 40, y: node.position.y + 40 },
      selected: false,
    };

    set({ nodes: [...nodes, newNode] });
  },

  handleTelemetry: (event) => {
    const { workflow_id, node_id, status, output } = event;
    if (workflow_id !== get().activeWorkflowId) return;
    const firedPorts = output?.fired_ports;
    get().updateNodeStatus(node_id, status, firedPorts);
  },

  handleProgress: (event) => {
    const { workflow_id, current_step, total_steps, active_node_name, workflow_name } = event;
    set((state) => {
      const existingIdx = state.activeRuns.findIndex(r => r.workflow_id === workflow_id);
      const newRun = {
        run_id: workflow_id, // Simple mapping for now
        workflow_id,
        workflow_name: workflow_name || state.availableWorkflows.find(w => w.id === workflow_id)?.name || 'Unknown',
        current_step,
        total_steps,
        active_node_name
      };

      if (existingIdx !== -1) {
        const nextRuns = [...state.activeRuns];
        nextRuns[existingIdx] = newRun;
        return { activeRuns: nextRuns };
      } else {
        return { activeRuns: [...state.activeRuns, newRun] };
      }
    });
  },

  handleStatusUpdate: (event) => {
    const { workflow_id, status } = event;
    if (status === 'completed' || status === 'failed') {
      set((state) => ({
        activeRuns: state.activeRuns.filter(r => r.workflow_id !== workflow_id)
      }));
    }
  },

  runActiveWorkflow: async (payload?: any) => {
    const workflowId = get().activeWorkflowId;
    if (!workflowId) return;

    // Reset statuses before running
    set({ nodeStatuses: {} });

    try {
      await invoke("run_workflow", { id: workflowId, payload: payload || null });
    } catch (err) {
      console.error("Failed to run workflow:", err);
    }
  },

  runWorkflowById: async (id: string, payload?: any) => {
    // Reset statuses if it's the active one, or just fire and let telemetry handle it
    if (get().activeWorkflowId === id) {
      set({ nodeStatuses: {} });
    }

    try {
      await invoke("run_workflow", { id, payload: payload || null });
    } catch (err) {
      console.error("Failed to run workflow by id:", err);
    }
  },

  stopAllTriggers: async () => {
    try {
      await invoke("stop_all_triggers");
    } catch (err) {
      console.error("Failed to stop all triggers:", err);
    }
  },

  stopWorkflowTriggers: async (workflowId: string) => {
    try {
      await invoke("stop_workflow_triggers", { workflowId });
    } catch (err) {
      console.error("Failed to stop workflow triggers:", err);
    }
  },

  stopWorkflowRun: async (workflowId: string) => {
    try {
      await invoke("stop_workflow_run", { workflowId });
      set((state) => ({
        activeRuns: state.activeRuns.filter(r => r.workflow_id !== workflowId)
      }));
    } catch (err) {
      console.error("Failed to stop workflow run:", err);
    }
  },

  pauseAllTriggers: async () => {
    try {
      await invoke("pause_all_triggers");
    } catch (err) {
      console.error("Failed to pause all triggers:", err);
    }
  },

  resumeAllTriggers: async () => {
    try {
      await invoke("resume_all_triggers");
    } catch (err) {
      console.error("Failed to resume all triggers:", err);
    }
  },

  clearActiveWorkflow: () => {
    set({
      activeWorkflowId: null,
      nodes: [],
      edges: [],
      nodeStatuses: {}
    });
  },

  loadScheduledRuns: async () => {
    try {
      const runs = await invoke<ScheduledRun[]>("list_scheduled_runs");
      set({ scheduledRuns: runs });
    } catch (err) {
      console.error("Failed to load scheduled runs:", err);
    }
  },

  createScheduledRun: async (run) => {
    try {
      await invoke("create_scheduled_run", { run });
      await get().loadScheduledRuns();
    } catch (err) {
      console.error("Failed to create scheduled run:", err);
    }
  },

  deleteScheduledRun: async (runId) => {
    try {
      await invoke("delete_scheduled_run", { run_id: runId });
      await get().loadScheduledRuns();
    } catch (err) {
      console.error("Failed to delete scheduled run:", err);
    }
  },

  toggleScheduledRun: async (runId) => {
    try {
      await invoke("toggle_scheduled_run", { run_id: runId });
      await get().loadScheduledRuns();
    } catch (err) {
      console.error("Failed to toggle scheduled run:", err);
    }
  },
}));
