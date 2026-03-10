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
import { WorkflowDefinition, NodeStatus, WorkflowTelemetryEvent } from '../types/workflow';

interface WorkflowState {
  nodes: Node[];
  edges: Edge[];
  activeWorkflowId: string | null;
  nodeStatuses: Record<string, NodeStatus>;
  availableWorkflows: WorkflowDefinition[];
  
  // Actions
  onNodesChange: OnNodesChange;
  onEdgesChange: OnEdgesChange;
  onConnect: (connection: Connection) => void;
  setNodes: (nodes: Node[]) => void;
  setEdges: (edges: Edge[]) => void;
  
  fetchWorkflows: () => Promise<void>;
  loadWorkflow: (workflow: WorkflowDefinition) => void;
  updateNodeStatus: (nodeId: string, status: NodeStatus) => void;
  handleTelemetry: (event: WorkflowTelemetryEvent) => void;
  runActiveWorkflow: () => Promise<void>;
  clearActiveWorkflow: () => void;
}

export const useWorkflowStore = create<WorkflowState>((set, get) => ({
  nodes: [],
  edges: [],
  activeWorkflowId: null,
  nodeStatuses: {},
  availableWorkflows: [],

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
      edges: addEdge(connection, get().edges),
    });
  },
  setNodes: (nodes) => set({ nodes }),
  setEdges: (edges) => set({ edges }),

  fetchWorkflows: async () => {
    try {
      const workflows = await invoke<WorkflowDefinition[]>("list_workflows");
      set({ availableWorkflows: workflows });
    } catch (err) {
      console.error("Failed to fetch workflows:", err);
    }
  },

  loadWorkflow: (workflow) => {
    const nodes: Node[] = workflow.nodes.map((node) => ({
      id: node.id,
      type: node.type,
      position: node.position || { x: Math.random() * 400, y: Math.random() * 400 },
      data: { 
        label: node.name || node.id, 
        config: node.config,
        type: node.type
      },
    }));

    const edges: Edge[] = [];
    workflow.nodes.forEach((node) => {
      if (node.depends_on) {
        node.depends_on.forEach((sourceId) => {
          edges.push({
            id: `e-${sourceId}-${node.id}`,
            source: sourceId,
            target: node.id,
            type: 'default',
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

  updateNodeStatus: (nodeId, status) => {
    set((state) => {
      const newNodeStatuses = {
        ...state.nodeStatuses,
        [nodeId]: status
      };

      // Update edge animations based on status
      const newEdges = state.edges.map(edge => {
        if (edge.target === nodeId) {
          return { ...edge, animated: status === 'processing' };
        }
        return edge;
      });

      return {
        nodeStatuses: newNodeStatuses,
        edges: newEdges
      };
    });
  },

  handleTelemetry: (event) => {
    const { node_id, status } = event;
    get().updateNodeStatus(node_id, status);
  },

  runActiveWorkflow: async () => {
    const workflowId = get().activeWorkflowId;
    if (!workflowId) return;

    // Reset statuses before running
    set({ nodeStatuses: {} });

    try {
      await invoke("run_workflow", { id: workflowId });
    } catch (err) {
      console.error("Failed to run workflow:", err);
    }
  },

  clearActiveWorkflow: () => {
    set({
      activeWorkflowId: null,
      nodes: [],
      edges: [],
      nodeStatuses: {}
    });
  }
}));
