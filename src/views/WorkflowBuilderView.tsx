import React, { useMemo, useEffect } from 'react';
import { 
  ReactFlow, 
  Background, 
  Controls, 
  MiniMap, 
  BackgroundVariant 
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { useWorkflowStore } from '../store/useWorkflowStore';
import { WorkflowNode } from '../features/workflows/WorkflowNode';

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
    fetchWorkflows,
    loadWorkflow,
    runActiveWorkflow,
    setNodes
  } = useWorkflowStore();

  const flowColorMode = useMemo(() => {
    if (theme === "system") {
      return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
    }
    return theme;
  }, [theme]);

  const [selectedNodeId, setSelectedNodeId] = React.useState<string | null>(null);
  const [isLibraryOpen, setIsBlockLibraryOpen] = React.useState(false);
  const [libraryPos, setLibraryPos] = React.useState({ x: 0, y: 0 });
  const [searchQuery, setSearchQuery] = React.useState("");

  useEffect(() => {
    fetchWorkflows();
  }, [fetchWorkflows]);

  const onNodeClick = (_: any, node: any) => setSelectedNodeId(node.id);
  const onPaneClick = () => {
    setSelectedNodeId(null);
    setIsBlockLibraryOpen(false);
  };

  const onPaneContextMenu = (event: React.MouseEvent) => {
    event.preventDefault();
    setLibraryPos({ x: event.clientX, y: event.clientY });
    setIsBlockLibraryOpen(true);
  };

  const addNode = (type: NodeType) => {
    const id = `${type}-${Date.now()}`;
    const newNode = {
      id,
      type,
      position: { x: libraryPos.x - 400, y: libraryPos.y - 200 }, // Rough offset
      data: { label: `New ${type}`, type },
    };
    setNodes([...nodes, newNode]);
    setIsBlockLibraryOpen(false);
    setSearchQuery("");
  };

  const selectedNode = useMemo(() => nodes.find(n => n.id === selectedNodeId), [nodes, selectedNodeId]);

  // Enhance nodes with status from store
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

  const blockTypes: NodeType[] = ['agent', 'command', 'script', 'tool', 'logic', 'loop', 'wait', 'parallel', 'subflow', 'governance', 'memory', 'communication'];
  const filteredBlocks = blockTypes.filter(b => b.includes(searchQuery.toLowerCase()));

  return (
    <div className="flex-1 h-full w-full bg-[var(--color-wardian-bg)] relative overflow-hidden rounded-2xl border border-wardian-border shadow-2xl flex flex-col">
      {/* Action Bar */}
      <div className="h-14 border-b border-wardian-border bg-[var(--color-wardian-card)] flex items-center justify-between px-6 z-10">
        <div className="flex items-center gap-4">
          <select 
            className="bg-[var(--color-wardian-bg)] border border-wardian-border text-[var(--color-wardian-text)] text-[10px] font-bold uppercase tracking-widest px-3 py-1.5 rounded-md focus:ring-1 focus:ring-[var(--color-wardian-accent)] outline-none"
            value={activeWorkflowId || ''}
            onChange={(e) => {
              const wf = availableWorkflows.find(w => w.id === e.target.value);
              if (wf) loadWorkflow(wf);
            }}
          >
            <option value="" disabled>Select Workflow</option>
            {availableWorkflows.map(wf => (
              <option key={wf.id} value={wf.id}>{wf.name}</option>
            ))}
          </select>
          
          <button 
            onClick={() => fetchWorkflows()}
            className="p-1.5 text-muted-neutral hover:text-primary transition-colors"
            title="Refresh List"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"></path></svg>
          </button>
        </div>

        <div className="flex items-center gap-3">
          <button 
            onClick={() => { setLibraryPos({ x: window.innerWidth/2, y: window.innerHeight/2 }); setIsBlockLibraryOpen(true); }}
            className="p-2 bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_90%)] text-[var(--color-wardian-accent)] rounded-lg hover:bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_80%)] transition-all border border-[color-mix(in_srgb,var(--color-wardian-accent),transparent_80%)]"
          >
            <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4"></path></svg>
          </button>
          <button 
            onClick={() => runActiveWorkflow()}
            disabled={!activeWorkflowId}
            className={`px-6 py-1.5 rounded-md text-[10px] font-bold uppercase tracking-widest transition-all ${activeWorkflowId ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] hover:bg-[var(--color-wardian-accent-hover)] hover:scale-105 active:scale-95' : 'bg-[var(--color-wardian-card-bg-muted)] text-[var(--color-wardian-text-muted-neutral)] cursor-not-allowed border border-wardian-border'}`}
          >
            RUN WORKFLOW
          </button>
        </div>
      </div>

      <div className="flex-1 relative bg-[var(--color-wardian-bg)]">
        <ReactFlow
          nodes={styledNodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onNodeClick={onNodeClick}
          onPaneClick={onPaneClick}
          onPaneContextMenu={onPaneContextMenu}
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
        </ReactFlow>

        {/* Block Library Popup */}
        {isLibraryOpen && (
          <div 
            className="absolute bg-[var(--color-wardian-card)] border border-wardian-border-heavy rounded-xl shadow-2xl z-50 w-64 overflow-hidden animate-in fade-in zoom-in duration-200"
            style={{ left: Math.min(libraryPos.x - 300, window.innerWidth - 700), top: Math.min(libraryPos.y - 100, window.innerHeight - 400) }}
          >
            <div className="p-3 border-b border-wardian-border">
              <input 
                autoFocus
                placeholder="Search blocks..."
                className="w-full bg-[var(--color-wardian-bg)] border border-wardian-border rounded px-2 py-1 text-xs text-primary outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
              />
            </div>
            <div className="max-h-64 overflow-y-auto p-1">
              {filteredBlocks.map(type => (
                <button
                  key={type}
                  onClick={() => addNode(type)}
                  className="w-full text-left px-3 py-2 text-xs font-bold uppercase tracking-widest text-muted-neutral hover:bg-[var(--color-wardian-accent)]/10 hover:text-[var(--color-wardian-accent)] rounded transition-colors"
                >
                  {type}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
      
      {/* Side Drawer Editor */}
      <div className={`absolute top-20 right-4 bottom-4 w-80 bg-[color-mix(in_srgb,var(--color-wardian-sidebar-primary),transparent_10%)] backdrop-blur-md border border-wardian-border rounded-xl p-6 shadow-2xl z-10 transition-transform duration-300 ${selectedNodeId ? 'translate-x-0' : 'translate-x-[calc(100%+2rem)]'}`}>
        <div className="flex justify-between items-center mb-6">
          <h3 className="text-lg font-bold text-[var(--color-wardian-text)] uppercase tracking-tighter">Node Editor</h3>
          <button onClick={() => setSelectedNodeId(null)} className="text-muted-neutral hover:text-white">&times;</button>
        </div>
        
        <div className="flex flex-col gap-6">
            <div className="flex flex-col gap-2">
              <label className="text-[10px] font-bold text-muted-neutral uppercase tracking-widest">Name</label>
              <input 
                className="bg-[var(--color-wardian-bg)] border border-wardian-border rounded px-3 py-2 text-sm text-primary outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
                value={selectedNode?.data.label || ""}
                onChange={(e) => {
                  const newNodes = nodes.map(n => n.id === selectedNodeId ? { ...n, data: { ...n.data, label: e.target.value } } : n);
                  setNodes(newNodes);
                }}
              />
            </div>

            <div className="p-4 bg-[var(--color-wardian-card-bg-muted)] rounded-xl border border-wardian-border-heavy">
                <span className="text-[10px] font-bold text-[var(--color-wardian-accent)] uppercase mb-3 block tracking-widest">Variable Assistant</span>
                <p className="text-[10px] text-muted-neutral italic mb-4">Click to insert at cursor:</p>
                <div className="flex flex-wrap gap-2">
                  {nodes.filter(n => n.id !== selectedNodeId).map(n => (
                    <button
                      key={n.id}
                      onClick={() => {
                        const activeEl = document.activeElement as HTMLTextAreaElement | HTMLInputElement;
                        if (activeEl && (activeEl.tagName === 'TEXTAREA' || activeEl.tagName === 'INPUT')) {
                          const start = activeEl.selectionStart || 0;
                          const end = activeEl.selectionEnd || 0;
                          const val = activeEl.value;
                          const tag = `{{nodes.${n.id}.output}}`;
                          activeEl.value = val.substring(0, start) + tag + val.substring(end);
                          activeEl.dispatchEvent(new Event('input', { bubbles: true }));
                        }
                      }}
                      className="px-2 py-1 bg-[var(--color-wardian-bg)] border border-wardian-border rounded text-[9px] font-mono text-[var(--color-wardian-processing)] hover:border-[var(--color-wardian-processing)] transition-colors"
                    >
                      {n.id}
                    </button>
                  ))}
                </div>
            </div>
        </div>
      </div>
    </div>
  );
};
