import React, { useMemo, useEffect } from 'react';
import { useConfirm } from '../components/ConfirmDialog';
import { 
  ReactFlow, 
  Background, 
  Controls, 
  MiniMap, 
  BackgroundVariant,
  ReactFlowProvider,
  useReactFlow,
  useNodesInitialized
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { useWorkflowStore } from '../store/useWorkflowStore';
import { WorkflowNode } from '../features/workflows/WorkflowNode';
import { BLOCK_LIBRARY, BlockDefinition } from '../features/workflows/blockLibrary';
import { SchemaEditor } from '../components/SchemaEditor';
import { RenderableInput } from '../components/RenderableInput';
import { VariableAssistant } from '../features/workflows/VariableAssistant';
import { RunPayloadModal, getManualTriggerSchema, getWorkflowRoles } from '../features/workflows/RunPayloadModal';

const nodeTypes = {
  trigger: WorkflowNode,
  agent: WorkflowNode,
  command: WorkflowNode,
  script: WorkflowNode,
  tool: WorkflowNode,
  logic: WorkflowNode,
  loop: WorkflowNode,
  wait: WorkflowNode,
  parallel: WorkflowNode,
  subflow: WorkflowNode,
  governance: WorkflowNode,
  memory: WorkflowNode,
  communication: WorkflowNode,
};

const FitViewHandler = ({ activeWorkflowId }: { activeWorkflowId: string | null }) => {
  const { fitView } = useReactFlow();
  const nodes = useWorkflowStore(s => s.nodes);
  const nodesInitialized = useNodesInitialized();

  useEffect(() => {
    if (activeWorkflowId && nodes.length > 0 && nodesInitialized) {
      // Multiple attempts to ensure the canvas is stable, especially on first launch
      const timer1 = setTimeout(() => fitView({ padding: 0.2 }), 50);
      const timer2 = setTimeout(() => fitView({ padding: 0.2 }), 400);
      return () => {
        clearTimeout(timer1);
        clearTimeout(timer2);
      };
    }
  }, [activeWorkflowId, nodes.length, nodesInitialized, fitView]);
  return null;
};

interface WorkflowBuilderViewProps {
  theme: "dark" | "light" | "system";
}

export const WorkflowBuilderView: React.FC<WorkflowBuilderViewProps> = ({ theme }) => {
  const { 
    nodes, 
    edges, 
    onNodesChange, 
    onEdgesChange, 
    onConnect,
    nodeStatuses,
    availableWorkflows,
    activeWorkflowId,
    agents,
    agentClasses,
    fetchWorkflows,
    loadWorkflow,
    runActiveWorkflow,
    setNodes,
    setEdges,
    saveWorkflow,
    deleteWorkflow,
    updateActiveWorkflowName,
    duplicateNode,
    isSaving,
    loadScheduledRuns
  } = useWorkflowStore();

  const confirm = useConfirm();
  const [selectedNodeId, setSelectedNodeId] = React.useState<string | null>(null);
  const [isLibraryOpen, setIsBlockLibraryOpen] = React.useState(false);
  const [activeCategory, setActiveCategory] = React.useState("ALL");
  const [searchQuery, setSearchQuery] = React.useState("");
  
  const [isRenamingWf, setIsRenamingWf] = React.useState(false);
  const [tempWfName, setTempWfName] = React.useState("");

  const [isRunModalOpen, setIsRunModalOpen] = React.useState(false);

  const [contextMenu, setContextMenu] = React.useState<{
    visible: boolean;
    x: number;
    y: number;
    type: 'node' | 'edge';
    targetId: string;
  } | null>(null);

  useEffect(() => {
    fetchWorkflows();
  }, [fetchWorkflows]);

  // Auto-load first workflow if none active
  useEffect(() => {
    if (!activeWorkflowId && availableWorkflows.length > 0) {
      loadWorkflow(availableWorkflows[0]);
    }
  }, [activeWorkflowId, availableWorkflows, loadWorkflow]);

  const activeWorkflow = useMemo(() => availableWorkflows.find(w => w.id === activeWorkflowId), [availableWorkflows, activeWorkflowId]);

  useEffect(() => {
    if (activeWorkflow) setTempWfName(activeWorkflow.name);
  }, [activeWorkflow]);

  const commitRename = () => {
    if (tempWfName.trim()) {
      updateActiveWorkflowName(tempWfName);
    }
    setIsRenamingWf(false);
  };

  const flowColorMode = useMemo(() => {
    if (theme === "system") {
      return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
    }
    return theme;
  }, [theme]);

  const onNodeClick = (_: any, node: any) => setSelectedNodeId(node.id);
  const onPaneClick = () => {
    setSelectedNodeId(null);
    setContextMenu(null);
  };

  const onPaneContextMenu = (event: React.MouseEvent | MouseEvent) => {
    event.preventDefault();
    setIsBlockLibraryOpen(true);
    setContextMenu(null);
  };

  const onNodeContextMenu = (event: React.MouseEvent, node: any) => {
    event.preventDefault();
    event.stopPropagation();
    setContextMenu({
      visible: true,
      x: event.clientX,
      y: event.clientY,
      type: 'node',
      targetId: node.id
    });
  };

  const onEdgeContextMenu = (event: React.MouseEvent, edge: any) => {
    event.preventDefault();
    event.stopPropagation();
    setContextMenu({
      visible: true,
      x: event.clientX,
      y: event.clientY,
      type: 'edge',
      targetId: edge.id
    });
  };



  const handleSave = () => {
    if (activeWorkflowId && activeWorkflow) {
      const updatedNodes = nodes.map(n => ({
        id: n.id,
        type: n.type,
        name: n.data.label,
        config: n.data.config || {},
        position: n.position,
        dependencies: edges
          .filter(e => e.target === n.id)
          .map(e => ({
            node_id: e.source,
            port: e.sourceHandle || 'default'
          }))
      }));
      saveWorkflow({ ...activeWorkflow, name: tempWfName, nodes: updatedNodes as any });
      return true;
    }
    return false;
  };

  const handleRun = async () => {
    handleSave();

    // Scheduled triggers: save registers the schedule; don't execute immediately
    const triggerNode = activeWorkflow?.nodes.find(n => n.type === 'trigger');
    if (triggerNode?.name === 'Scheduled Trigger') {
      loadScheduledRuns();
      return;
    }

    if (activeWorkflow && (getManualTriggerSchema(activeWorkflow) || getWorkflowRoles(activeWorkflow).length > 0)) {
      setIsRunModalOpen(true);
      return;
    }

    runActiveWorkflow();
  };

  const deleteNode = (id: string) => {
    setNodes(nodes.filter(n => n.id !== id));
    setEdges(edges.filter(e => e.source !== id && e.target !== id));
    setContextMenu(null);
  };

  const deleteEdge = (id: string) => {
    setEdges(edges.filter(e => e.id !== id));
    setContextMenu(null);
  };

  const copyNodeId = (id: string) => {
    navigator.clipboard.writeText(id);
    setContextMenu(null);
  };

  const getNextNodeId = (type: string) => {
    const sameTypeNodes = nodes.filter(n => n.type === type);
    if (sameTypeNodes.length === 0) return `${type}-1`;
    
    const ids = sameTypeNodes.map(n => {
      const parts = n.id.split('-');
      return parseInt(parts[parts.length - 1]);
    }).filter(n => !isNaN(n));
    
    const maxId = ids.length > 0 ? Math.max(...ids) : 0;
    return `${type}-${maxId + 1}`;
  };

  const addNode = (block: BlockDefinition) => {
    const id = getNextNodeId(block.type);
    
    // Initialize config with defaults
    const config: Record<string, any> = {};
    if (block.type === 'agent') {
      config.session_type = 'persistent';
      config.output_format = 'text';
    }
    if (block.type === 'loop') {
      config.mode = 'count';
      config.max_iterations = '10';
      config.iterator_name = 'i';
    }
    if (block.name === 'Scheduled Trigger') {
      config.schedule_type = 'Minutes';
      config.interval = '5';
    }

    const newNode = {
      id,
      type: block.type,
      position: { x: 400, y: 300 }, // Centralized for large modal
      data: { 
        label: block.name, 
        type: block.type,
        blockName: block.name,
        inputs: block.inputs,
        outputs: block.outputs,
        config
      },
    };
    setNodes([...nodes, newNode]);
    setIsBlockLibraryOpen(false);
    setSearchQuery("");
  };

  const selectedNode = useMemo(() => nodes.find(n => n.id === selectedNodeId), [nodes, selectedNodeId]);

  const styledNodes = useMemo(() => 
    nodes.map(node => ({
      ...node,
      data: {
        ...node.data,
        status: nodeStatuses[node.id] || 'idle'
      }
    })),
    [nodes, nodeStatuses]
  );

  const categories = ["ALL", ...Array.from(new Set(BLOCK_LIBRARY.map(b => b.category)))];
  const filteredBlocks = BLOCK_LIBRARY.filter(b => 
    (activeCategory === "ALL" || b.category === activeCategory) &&
    (b.name.toLowerCase().includes(searchQuery.toLowerCase()) || b.description.toLowerCase().includes(searchQuery.toLowerCase()))
  );

  const hasGraphErrors = useMemo(() => {
    // Check for multiple triggers
    const triggerCount = nodes.filter(n => n.type === 'trigger').length;
    if (triggerCount > 1) return true;

    return nodes.some(n => {
      const incoming = edges.filter(e => e.target === n.id);
      if (n.type === 'loop') return incoming.length < 2;
      return false;
    });
  }, [nodes, edges]);

  return (
    <div className="flex-1 h-full w-full bg-[var(--color-wardian-bg)] relative overflow-hidden rounded-2xl border border-wardian-border shadow-2xl flex flex-col">
      {/* Action Bar */}
      <div className="h-14 border-b border-wardian-border bg-[var(--color-wardian-card)] grid grid-cols-3 items-center px-6 z-10">
        
        {/* Left: Registry Management */}
        <div className="flex items-center gap-2">
          <select 
            className="bg-[var(--color-wardian-bg)] border border-wardian-border text-[var(--color-wardian-text)] text-[10px] font-bold tracking-wide px-3 py-1.5 rounded-md focus:ring-1 focus:ring-[var(--color-wardian-accent)] outline-none cursor-pointer"
            value={activeWorkflowId || ''}
            onChange={(e) => {
              const wf = availableWorkflows.find(w => w.id === e.target.value);
              if (wf) {
                loadWorkflow(wf);
                setIsRenamingWf(false);
              }
            }}
          >
            {availableWorkflows.map(wf => (
              <option key={wf.id} value={wf.id}>{wf.name}</option>
            ))}
          </select>

          <button 
            onClick={() => {
              const id = `wf-${Date.now()}`;
              const newWf = { id, name: "New Workflow", settings: { max_iterations: 10, on_limit_reached: "terminate" }, nodes: [] };
              saveWorkflow(newWf as any);
            }}
            className="p-1.5 text-muted-neutral hover:text-[var(--color-wardian-accent)] transition-colors cursor-pointer"
            title="New Workflow"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4"></path></svg>
          </button>

          <button 
            onClick={() => fetchWorkflows()}
            className="p-1.5 text-muted-neutral hover:text-primary transition-colors cursor-pointer"
            title="Refresh Registry"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"></path></svg>
          </button>
        </div>

        {/* Center: Active Context (Hero) */}
        <div className="flex items-center justify-center gap-3">
          {activeWorkflow ? (
            <>
              <div className={`flex items-center gap-2 px-4 py-1 bg-[var(--color-wardian-bg)] border border-wardian-border rounded-lg shadow-inner cursor-pointer hover:border-[var(--color-wardian-accent)]`}>
                {isRenamingWf ? (
                  <input
                    autoFocus
                    className="bg-transparent text-[var(--color-wardian-text)] text-sm font-bold outline-none text-center min-w-[120px]"
                    value={tempWfName}
                    onChange={(e) => setTempWfName(e.target.value)}
                    onBlur={commitRename}
                    onKeyDown={(e) => e.key === 'Enter' && commitRename()}
                  />
                ) : (
                  <h3 
                    onClick={() => setIsRenamingWf(true)}
                    className={`text-sm font-bold text-[var(--color-wardian-text)] transition-colors tracking-tight hover:text-[var(--color-wardian-accent)]`}
                  >
                    {tempWfName || activeWorkflow.name}
                  </h3>
                )}
              </div>
              
              <div className="flex items-center gap-1">
                <button 
                  onClick={() => {
                    if (activeWorkflow) {
                      const id = `wf-${Date.now()}`;
                      const clonedWf = { 
                        ...activeWorkflow, 
                        id, 
                        name: `Copy of ${activeWorkflow.name}`,
                        nodes: nodes.map(n => ({
                          id: n.id,
                          type: n.type,
                          name: n.data.label,
                          config: n.data.config || {},
                          position: n.position,
                          depends_on: edges.filter(e => e.target === n.id).map(e => e.source)
                        }))
                      };
                      saveWorkflow(clonedWf as any);
                    }
                  }}
                  className="p-1.5 text-muted-neutral hover:text-[var(--color-wardian-accent)] transition-colors cursor-pointer"
                  title="Clone Workflow"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 7v8a2 2 0 002 2h6M8 7V5a2 2 0 012-2h4.586a1 1 0 01.707.293l4.414 4.414a1 1 0 01.293.707V15a2 2 0 01-2 2h-2M8 7H6a2 2 0 00-2 2v10a2 2 0 002 2h8a2 2 0 002-2v-2"></path></svg>
                </button>

                <button
                  onClick={async () => {
                    if (activeWorkflowId && await confirm("Delete this workflow?")) {
                      deleteWorkflow(activeWorkflowId);
                    }
                  }}
                  className={`p-1.5 transition-colors text-muted-neutral hover:text-wardian-error cursor-pointer`}
                  title={"Delete This Workflow"}
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                </button>
              </div>
            </>
          ) : (
            <span className="label-small opacity-30 select-none">No Workflow Active</span>
          )}
        </div>

        {/* Right: Execution Actions */}
        <div className="flex items-center justify-end gap-3">
          <button 
            disabled={!activeWorkflowId}
            onClick={() => {
              if (activeWorkflowId) {
                const savedVersion = availableWorkflows.find(w => w.id === activeWorkflowId);
                if (savedVersion) {
                  loadWorkflow(savedVersion);
                }
              }
            }}
            className={`px-4 py-1.5 rounded-md text-[10px] font-bold uppercase tracking-widest transition-all ${activeWorkflowId ? 'bg-[var(--color-wardian-card-bg-muted)] text-[var(--color-wardian-text-muted)] hover:bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),var(--color-wardian-text)_10%)] hover:text-[var(--color-wardian-text)] border border-wardian-border cursor-pointer' : 'hidden'}`}
          >
            Reset
          </button>
          <button 
            disabled={!activeWorkflowId}
            onClick={handleSave}
            className={`px-4 py-1.5 rounded-md text-[10px] font-bold uppercase tracking-widest transition-all ${activeWorkflowId ? 'bg-[var(--color-wardian-card-bg-muted)] text-[var(--color-wardian-text-muted)] hover:bg-[color-mix(in_srgb,var(--color-wardian-card-bg-muted),var(--color-wardian-text)_10%)] hover:text-[var(--color-wardian-text)] border border-wardian-border cursor-pointer' : 'bg-[var(--color-wardian-card-bg-muted)] text-[var(--color-wardian-text-muted-neutral)] cursor-not-allowed hidden'}`}
          >
            {isSaving ? 'Saving...' : 'Save Changes'}
          </button>
          <button 
            onClick={handleRun}
            disabled={!activeWorkflowId || hasGraphErrors}
            className={`px-6 py-1.5 rounded-md text-[10px] font-bold uppercase tracking-widest transition-all ${activeWorkflowId && !hasGraphErrors ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] hover:bg-[var(--color-wardian-accent-hover)] hover:scale-105 active:scale-95 cursor-pointer shadow-wardian-accent' : 'bg-wardian-error/10 text-wardian-error/50 cursor-not-allowed border border-wardian-error/20 shadow-none scale-100'}`}
            title={hasGraphErrors ? "Cannot run: Fix node errors first" : undefined}
          >
            {hasGraphErrors ? 'Graph Error' : 'Run Workflow'}
          </button>
        </div>
      </div>

      <div className="flex-1 relative bg-[var(--color-wardian-bg)]">
        {/* Floating Add Block Button */}
        <div className="absolute top-6 left-6 z-20">
          <button 
            onClick={() => { setIsBlockLibraryOpen(true); }}
            className="group flex items-center gap-3 bg-[var(--color-wardian-card)] border border-wardian-border-heavy p-2 pr-4 rounded-xl hover:border-[var(--color-wardian-accent)] transition-all shadow-wardian-accent cursor-pointer"
          >
            <div className="p-2 bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_90%)] text-[var(--color-wardian-accent)] rounded-lg group-hover:bg-[var(--color-wardian-accent)] transition-all group-hover:text-[var(--color-wardian-bg)]">
              <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4"></path></svg>
            </div>
            <span className="label-small group-hover:text-[var(--color-wardian-text)]">Add Block</span>
          </button>
        </div>

        <ReactFlowProvider>
          <ReactFlow
            nodes={styledNodes}
            edges={edges}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            onNodeClick={onNodeClick}
            onPaneClick={onPaneClick}
            onPaneContextMenu={onPaneContextMenu}
            onNodeContextMenu={onNodeContextMenu}
            onEdgeContextMenu={onEdgeContextMenu}
            onNodesDelete={(deleted) => deleted.forEach(n => deleteNode(n.id))}
            onEdgesDelete={(deleted) => deleted.forEach(e => deleteEdge(e.id))}
            nodeTypes={nodeTypes}
            fitView
            colorMode={flowColorMode}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={12} size={1} color="var(--color-wardian-border-heavy)" />
            <Controls className="!bg-[var(--color-wardian-card)] !border-wardian-border !fill-[var(--color-wardian-text)]" />
            <MiniMap 
              className="!bg-[var(--color-wardian-card)] !border-wardian-border" 
              maskColor="color-mix(in srgb, var(--color-wardian-bg), transparent 50%)"
              nodeStrokeWidth={3}
            />
            <FitViewHandler activeWorkflowId={activeWorkflowId} />
          </ReactFlow>
        </ReactFlowProvider>

        {/* --- Context Menu --- */}
        {contextMenu && (
          <div 
            className="fixed z-[100] min-w-[160px] bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_10%)] backdrop-blur-xl border border-wardian-border shadow-2xl rounded-xl p-1 overflow-hidden animate-in fade-in zoom-in-95 duration-100"
            style={{ left: contextMenu.x, top: contextMenu.y }}
          >
            {contextMenu.type === 'node' ? (
              <>
                <button 
                  onClick={() => { duplicateNode(contextMenu.targetId); setContextMenu(null); }}
                  className="w-full text-left px-3 py-2 text-xs font-bold text-[var(--color-wardian-text)] hover:bg-[var(--color-wardian-accent)]/10 hover:text-[var(--color-wardian-accent)] rounded-lg transition-colors flex items-center gap-2"
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 7v8a2 2 0 002 2h6M8 7V5a2 2 0 012-2h4.586a1 1 0 01.707.293l4.414 4.414a1 1 0 01.293.707V15a2 2 0 01-2 2h-2M8 7H6a2 2 0 00-2 2v10a2 2 0 002 2h8a2 2 0 002-2v-2"></path></svg>
                  Duplicate Node
                </button>
                <button 
                  onClick={() => copyNodeId(contextMenu.targetId)}
                  className="w-full text-left px-3 py-2 text-xs font-bold text-[var(--color-wardian-text)] hover:bg-[var(--color-wardian-accent)]/10 hover:text-[var(--color-wardian-accent)] rounded-lg transition-colors flex items-center gap-2"
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3"></path></svg>
                  Copy Node ID
                </button>
                <div className="h-px bg-wardian-border my-1 mx-1" />
                <button 
                  onClick={() => deleteNode(contextMenu.targetId)}
                  className="w-full text-left px-3 py-2 text-xs font-bold text-red-500 hover:bg-red-500/10 rounded-lg transition-colors flex items-center gap-2"
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                  Delete Node
                </button>
              </>
            ) : (
              <button 
                onClick={() => deleteEdge(contextMenu.targetId)}
                className="w-full text-left px-3 py-2 text-xs font-bold text-red-500 hover:bg-red-500/10 rounded-lg transition-colors flex items-center gap-2"
              >
                <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                Delete Connection
              </button>
            )}
          </div>
        )}

        {/* --- Block Library Modal --- */}
        {isLibraryOpen && (
          <div className="absolute inset-0 z-50 flex items-center justify-center p-8 bg-black/40 backdrop-blur-sm animate-in fade-in duration-200">
            <div className="bg-[var(--color-wardian-card)] border border-wardian-border-heavy w-full max-w-5xl h-[80vh] rounded-3xl shadow-[0_32px_64px_rgba(0,0,0,0.5)] flex overflow-hidden animate-in zoom-in-95 duration-300">
              
              {/* Sidebar */}
              <div className="w-64 border-r border-wardian-border flex flex-col bg-[color-mix(in_srgb,var(--color-wardian-card),black_10%)]">
                <div className="p-8">
                  <h2 className="text-2xl font-bold text-[var(--color-wardian-text)] tracking-tighter mb-6">Block Library</h2>
                  <div className="relative">
                    <input 
                      autoFocus
                      placeholder="Search..."
                      className="w-full bg-[var(--color-wardian-bg)] border border-wardian-border rounded-xl px-4 py-2.5 text-sm text-primary outline-none focus:ring-2 focus:ring-[var(--color-wardian-accent)] transition-all"
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                    />
                  </div>
                </div>
                
                <nav className="flex-1 overflow-y-auto px-4 pb-8 space-y-1">
                  {categories.map(cat => (
                    <button
                      key={cat}
                      onClick={() => setActiveCategory(cat)}
                      className={`w-full text-left px-4 py-3 rounded-xl text-xs font-bold tracking-wide transition-all cursor-pointer ${activeCategory === cat ? 'bg-[var(--color-wardian-accent)]/10 text-[var(--color-wardian-accent)] shadow-sm' : 'text-muted-neutral hover:bg-[var(--color-wardian-card-bg-muted)] hover:text-[var(--color-wardian-text)]'}`}
                    >
                      {cat === "ALL" ? "✦ All Blocks" : cat.charAt(0).toUpperCase() + cat.slice(1).toLowerCase()}
                    </button>
                  ))}
                </nav>
              </div>

              {/* Main Content */}
              <div className="flex-1 flex flex-col min-w-0">
                <div className="h-16 border-b border-wardian-border flex items-center justify-between px-8 bg-[var(--color-wardian-card)]">
                  <span className="text-sm font-mono font-bold text-muted-neutral tracking-wide">{activeCategory.charAt(0).toUpperCase() + activeCategory.slice(1).toLowerCase()} Blocks ({filteredBlocks.length})</span>
                  <button 
                    onClick={() => setIsBlockLibraryOpen(false)}
                    className="p-2 hover:bg-[var(--color-wardian-card-bg-muted)] rounded-full text-muted-neutral hover:text-[var(--color-wardian-text)] transition-all cursor-pointer"
                  >
                    <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                  </button>
                </div>

                <div className="flex-1 overflow-y-auto p-6 no-scrollbar">
                  <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                    {filteredBlocks.map(block => (
                      <button
                        key={block.name}
                        onClick={() => addNode(block)}
                        className="group flex flex-col text-left p-5 rounded-2xl bg-[var(--color-wardian-bg)] border border-wardian-border hover:border-[var(--color-wardian-accent)]/50 hover:shadow-2xl hover:shadow-[var(--color-wardian-accent)]/5 transition-all duration-300 relative overflow-hidden cursor-pointer h-full"
                      >
                        <div className="absolute top-0 right-0 p-4 opacity-0 group-hover:opacity-100 transition-opacity">
                          <svg className="w-4 h-4 text-[var(--color-wardian-accent)]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="3" d="M12 4v16m8-8H4"></path></svg>
                        </div>
                        <h3 className="text-lg font-bold text-[var(--color-wardian-text)] mb-1 group-hover:text-[var(--color-wardian-accent)] transition-colors">{block.name}</h3>
                        <p className="text-sm text-muted-neutral leading-snug mb-1">{block.description}</p>
                        
                        <div className="space-y-3 mt-auto pt-2 border-t border-white/5">
                          <div className="flex flex-col gap-1">
                            <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-[var(--color-wardian-text-muted)] opacity-80">Input</span>
                            <span className="text-sm font-mono text-[var(--color-wardian-text)]/80 break-all leading-tight">{block.inputs}</span>
                          </div>
                          <div className="flex flex-col gap-1">
                            <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-[var(--color-wardian-text-muted)] opacity-80">Output</span>
                            <span className="text-sm font-mono text-[var(--color-wardian-processing)] break-all leading-tight">{block.outputs}</span>
                          </div>
                        </div>
                      </button>
                    ))}
                  </div>
                </div>
              </div>

            </div>
          </div>
        )}
      </div>
      
      {/* Side Drawer Editor */}
      <div 
        onContextMenu={(e) => e.preventDefault()}
        className={`absolute top-20 right-4 bottom-4 w-[400px] bg-[color-mix(in_srgb,var(--color-wardian-sidebar-primary),transparent_5%)] backdrop-blur-xl border border-wardian-border rounded-2xl p-0 shadow-[0_32px_64px_rgba(0,0,0,0.5)] z-10 transition-transform duration-300 flex flex-col overflow-hidden ${selectedNodeId ? 'translate-x-0' : 'translate-x-[calc(100%+2rem)]'}`}
      >
        <div className="flex justify-between items-center p-6 border-b border-wardian-border bg-white/5">
          <div className="flex flex-col">
            <h3 className="text-lg font-bold text-[var(--color-wardian-text)] tracking-tight">Node Settings</h3>
            <span className="text-[10px] font-bold text-muted-neutral tracking-wide">
              {BLOCK_LIBRARY.find(b => b.type === selectedNode?.type && (selectedNode?.data.blockName ? b.name === selectedNode.data.blockName : true))?.name || selectedNode?.type}
            </span>
          </div>
          <button onClick={() => setSelectedNodeId(null)} className="p-2 hover:bg-white/10 rounded-full text-muted-neutral hover:text-[var(--color-wardian-text)] cursor-pointer transition-all">&times;</button>
        </div>
        
        <div className="flex-1 overflow-y-auto p-6 space-y-8 no-scrollbar">
            {/* Core Fields (Shared with Canvas) */}
            <div className="space-y-6">
              <div className="flex flex-col gap-2">
                <label className="text-[10px] font-bold text-[var(--color-wardian-accent)] tracking-wide">Display Name</label>
                <RenderableInput 
                  className="!rounded-xl"
                  value={(selectedNode?.data.label as string) || ""}
                  nodeId={selectedNodeId!}
                  onChange={(newVal) => {
                    const newNodes = nodes.map(n => n.id === selectedNodeId ? { ...n, data: { ...n.data, label: newVal } } : n);
                    setNodes(newNodes);
                  }}
                />
              </div>


              {(() => {
                const blockDef = BLOCK_LIBRARY.find(b => b.type === selectedNode?.type && (selectedNode?.data.blockName ? b.name === selectedNode.data.blockName : true));
                if (!blockDef) return null;

                const renderField = (field: any) => {
                  const val = (selectedNode?.data.config as any)?.[field.name] || '';
                  const sessionType = (selectedNode?.data.config as any)?.session_type || 'persistent';

                  // Conditional visibility for Agent fields
                  if (selectedNode?.type === 'agent') {
                    if (field.name === 'agent_id' && sessionType === 'temporary') return null;
                    if (field.name === 'agent_class' && sessionType === 'persistent') return null;
                    if (field.name === 'folder' && sessionType === 'persistent') return null;

                    const outputFormat = (selectedNode?.data.config as any)?.output_format || 'text';
                    if (field.name === 'json_schema' && outputFormat !== 'json') return null;
                  }

                  // Conditional visibility for Schedule fields
                  if (selectedNode?.data.blockName === 'Scheduled Trigger') {
                    const st = (selectedNode?.data.config as any)?.schedule_type || 'Minutes';
                    if (st === 'Minutes' && ['time', 'days', 'datetime'].includes(field.name)) return null;
                    if (st === 'Hours' && ['time', 'days', 'datetime'].includes(field.name)) return null;
                    if (st === 'Daily' && ['interval', 'days', 'datetime'].includes(field.name)) return null;
                    if (st === 'Weekly' && ['interval', 'datetime'].includes(field.name)) return null;
                    if (st === 'One-Time' && ['interval', 'time', 'days'].includes(field.name)) return null;
                  }

                  // Conditional visibility for Loop fields
                  if (selectedNode?.type === 'loop') {
                    const loopMode = (selectedNode?.data.config as any)?.mode || 'count';
                    if (field.name === 'condition' && loopMode === 'count') return null;
                    if (field.name === 'max_iterations' && loopMode === 'conditional') return null;
                  }

                  // Dynamic Options Override
                  let dynamicOptions = field.options;
                  if (field.name === 'agent_id') {
                    return (
                      <div key={field.name} className="flex flex-col gap-2">
                        <label className="text-[10px] font-bold text-[var(--color-wardian-accent)] uppercase tracking-[0.2em]">{field.label}</label>
                        <select
                          className="p-3 rounded-xl bg-[var(--color-wardian-bg)] border border-wardian-border text-sm text-[var(--color-wardian-text)] w-full outline-none focus:ring-2 focus:ring-[var(--color-wardian-accent)] cursor-pointer"
                          value={val}
                          onChange={(e) => useWorkflowStore.getState().updateNodeConfig(selectedNodeId!, field.name, e.target.value)}
                        >
                          {val === '' && <option value="" disabled>Select {field.label}</option>}
                          {(agents || []).map(a => <option key={a.session_id} value={a.session_id}>{a.session_name}</option>)}
                        </select>
                      </div>
                    );
                  }
                  if (field.name === 'agent_class') dynamicOptions = (agentClasses || []).map(c => c.name);
                  const isSelect = field.type === 'select' || (dynamicOptions && dynamicOptions.length > 0);

                  return (
                    <div key={field.name} className="flex flex-col gap-2">
                      <label className="text-[10px] font-bold text-[var(--color-wardian-accent)] tracking-wide">{field.label}</label>
                      {field.type === 'schema' ? (
                        <SchemaEditor 
                          value={val}
                          nodeId={selectedNodeId!}
                          onChange={(newVal) => useWorkflowStore.getState().updateNodeConfig(selectedNodeId!, field.name, newVal)}
                        />
                      ) : isSelect ? (
                        <select
                          className="p-3 rounded-xl bg-[var(--color-wardian-bg)] border border-wardian-border text-sm text-[var(--color-wardian-text)] w-full outline-none focus:ring-2 focus:ring-[var(--color-wardian-accent)] cursor-pointer"
                          value={val}
                          onChange={(e) => useWorkflowStore.getState().updateNodeConfig(selectedNodeId!, field.name, e.target.value)}
                        >
                          {val === '' && <option value="" disabled>Select {field.label}</option>}
                          {dynamicOptions?.map((opt: string) => <option key={opt} value={opt}>{opt}</option>)}
                        </select>
                      ) : (
                        <RenderableInput
                          multiline={true}
                          placeholder={field.placeholder}
                          value={val}
                          nodeId={selectedNodeId!}
                          onChange={(newVal) => useWorkflowStore.getState().updateNodeConfig(selectedNodeId!, field.name, newVal)}
                        />
                      )}
                    </div>
                  );
                };

                return (
                  <>
                    {blockDef.fields?.map(f => renderField(f))}
                    
                    {blockDef.advancedFields && blockDef.advancedFields.length > 0 && (
                      <div className="pt-8 border-t border-wardian-border space-y-6">
                        <h4 className="text-[10px] font-bold text-muted-neutral uppercase tracking-[0.3em]">Advanced Configuration</h4>
                        {blockDef.advancedFields.map(f => renderField(f))}
                      </div>
                    )}
                  </>
                );
              })()}
            </div>

            {/* Variable Assistant */}
            <div className="pt-2">
              <VariableAssistant selectedNodeId={selectedNodeId!} />
            </div>
        </div>
      </div>
      {activeWorkflow && (
        <RunPayloadModal
          workflow={activeWorkflow}
          isOpen={isRunModalOpen}
          agents={agents.map(a => ({ session_id: a.session_id, session_name: a.session_name }))}
          onRun={(payload) => {
            runActiveWorkflow(payload);
            setIsRunModalOpen(false);
          }}
          onCancel={() => setIsRunModalOpen(false)}
        />
      )}
    </div>
  );
};
