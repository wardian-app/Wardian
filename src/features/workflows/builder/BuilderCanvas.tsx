import { memo, useCallback, useEffect, useMemo } from 'react';
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type ColorMode,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { toReactFlow, fromReactFlow } from './blueprintGraph';
import { findNodeType } from './registry';
import { describeNodeFields } from './nodeSummary';
import { useBuilderStore } from '../../../store/useBuilderStore';
import type { Blueprint, BlueprintNode, Diagnostic, PortDef } from './blueprintTypes';

interface BuilderCanvasProps {
  blueprint: Blueprint;
  diagnostics: Diagnostic[];
  selectedNodeId: string | null;
  onSelectNode: (id: string | null) => void;
  theme: 'dark' | 'light' | 'system';
}

type BuilderNodeData = { node: BlueprintNode; diagnostics?: Diagnostic[] };

export function BuilderCanvas({ blueprint, diagnostics, selectedNodeId, onSelectNode, theme }: BuilderCanvasProps) {
  return (
    <ReactFlowProvider>
      <BuilderCanvasInner
        blueprint={blueprint}
        diagnostics={diagnostics}
        selectedNodeId={selectedNodeId}
        onSelectNode={onSelectNode}
        theme={theme}
      />
    </ReactFlowProvider>
  );
}

function BuilderCanvasInner({ blueprint, diagnostics, selectedNodeId, onSelectNode, theme }: BuilderCanvasProps) {
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const { fitView } = useReactFlow();

  const graph = useMemo(() => {
    const rf = toReactFlow(blueprint);
    return {
      nodes: rf.nodes.map((node) => ({
        ...node,
        selected: node.id === selectedNodeId,
        data: {
          ...(node.data as BuilderNodeData),
          diagnostics: diagnostics.filter((d) => d.node === node.id),
        },
      })),
      edges: rf.edges,
    };
  }, [blueprint, diagnostics, selectedNodeId]);

  const flowColorMode: ColorMode = useMemo(() => {
    if (theme === 'system') {
      return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }
    return theme;
  }, [theme]);

  const commitGraph = useCallback((nodes: Node[], edges: Edge[]) => {
    setBlueprint(fromReactFlow(nodes, edges, {
      schema: blueprint.schema,
      id: blueprint.id,
      name: blueprint.name,
    }));
  }, [blueprint.id, blueprint.name, blueprint.schema, setBlueprint]);

  const handleNodesChange = useCallback((changes: NodeChange[]) => {
    commitGraph(applyNodeChanges(changes, graph.nodes), graph.edges);
  }, [commitGraph, graph.edges, graph.nodes]);

  const handleEdgesChange = useCallback((changes: EdgeChange[]) => {
    commitGraph(graph.nodes, applyEdgeChanges(changes, graph.edges));
  }, [commitGraph, graph.edges, graph.nodes]);

  const handleConnect = useCallback((connection: Connection) => {
    const edgeId = `e${graph.edges.length}`;
    commitGraph(graph.nodes, addEdge({ ...connection, id: edgeId }, graph.edges));
  }, [commitGraph, graph.edges, graph.nodes]);

  useEffect(() => {
    if (graph.nodes.length === 0) return;
    const frame = window.requestAnimationFrame(() => {
      void fitView({ padding: 0.2, duration: 120 });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [blueprint.id, graph.nodes.length, fitView]);

  return (
    <ReactFlow
      nodes={graph.nodes}
      edges={graph.edges}
      nodeTypes={nodeTypes}
      onNodesChange={handleNodesChange}
      onEdgesChange={handleEdgesChange}
      onConnect={handleConnect}
      onNodeClick={(_, node) => onSelectNode(node.id)}
      onPaneClick={() => onSelectNode(null)}
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
  );
}

const BuilderNode = memo(({ data, selected }: NodeProps<Node<BuilderNodeData>>) => {
  const node = data.node;
  const def = findNodeType(node.type);
  const hasDiagnostics = Boolean(data.diagnostics?.some((d) => d.severity === 'error'));
  const summaries = describeNodeFields(node, def);

  return (
    <div data-testid={`builder-node-${node.id}`} className={`w-[260px] rounded-lg border-2 bg-[var(--color-wardian-card)] px-3 py-3 shadow-md transition-all ${
      hasDiagnostics
        ? 'border-[var(--color-wardian-error)]'
        : selected
          ? 'border-[var(--color-wardian-accent)]'
          : 'border-wardian-border'
    }`}>
      {renderHandles('target', portsFor(def?.inputs ?? []), Position.Left)}
      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[9px] font-bold tracking-wide text-muted">{def?.label ?? node.type}</span>
          {hasDiagnostics && (
            <span className="text-[9px] font-bold text-[var(--color-wardian-error)]">Error</span>
          )}
        </div>
        <div className="truncate text-sm font-bold text-[var(--color-wardian-text)]">{node.name ?? node.id}</div>
        {def?.description && (
          <div className="line-clamp-2 text-[10px] leading-snug text-muted">{def.description}</div>
        )}
        {summaries.length > 0 ? (
          <div className="grid gap-1 border-t border-wardian-border pt-2">
            {summaries.map((summary) => (
              <div key={summary.label} className="grid grid-cols-[74px_minmax(0,1fr)] gap-2 text-[10px] leading-tight">
                <span className="truncate font-bold text-[var(--color-wardian-text-muted)]">{summary.label}</span>
                <span className={`truncate ${summary.state === 'missing' ? 'text-[var(--color-wardian-error)]' : 'text-[var(--color-wardian-text)]'}`}>
                  {summary.value}
                </span>
              </div>
            ))}
          </div>
        ) : null}
      </div>
      {renderHandles('source', outputPorts(node, def?.outputs ?? [], def?.outputs_from_field), Position.Right)}
    </div>
  );
});

const LoopGroupNode = memo(({ data, selected }: NodeProps<Node<BuilderNodeData>>) => {
  const node = data.node;
  const def = findNodeType(node.type);
  const hasDiagnostics = Boolean(data.diagnostics?.some((d) => d.severity === 'error'));

  return (
    <div data-testid={`builder-node-${node.id}`} className={`h-full min-h-[220px] w-full min-w-[360px] rounded-lg border-2 bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_20%)] px-4 py-3 ${
      hasDiagnostics
        ? 'border-[var(--color-wardian-error)]'
        : selected
          ? 'border-[var(--color-wardian-accent)]'
          : 'border-wardian-border'
    }`}>
      {renderHandles('target', portsFor(def?.inputs ?? []), Position.Left)}
      <div className="text-[10px] font-bold tracking-wide text-muted">{def?.label ?? 'Loop'}</div>
      <div className="mt-1 text-sm font-bold text-[var(--color-wardian-text)]">{node.name ?? node.id}</div>
      {renderHandles('source', outputPorts(node, def?.outputs ?? [], def?.outputs_from_field), Position.Right)}
    </div>
  );
});

const nodeTypes = {
  wardian: BuilderNode,
  group: LoopGroupNode,
};

function portsFor(ports: PortDef[]) {
  return ports.map((port) => ({ id: port.id, label: port.label }));
}

function outputPorts(node: BlueprintNode, ports: PortDef[], dynamicField?: string) {
  if (!dynamicField) return portsFor(ports);
  const dynamic = node.fields?.[dynamicField];
  if (!Array.isArray(dynamic)) return portsFor(ports);
  return dynamic.map((port) => ({ id: String(port), label: String(port) }));
}

function renderHandles(type: 'source' | 'target', ports: { id: string; label: string }[], position: Position) {
  return ports.map((port, index) => (
    <div
      key={`${type}-${port.id}`}
      className="absolute"
      style={{
        top: `${(index + 1) * (100 / (ports.length + 1))}%`,
        [position === Position.Left ? 'left' : 'right']: '-4px',
      }}
    >
      <Handle
        id={port.id}
        type={type}
        position={position}
        className="h-2 w-2 border-none !bg-[var(--color-wardian-border-heavy)]"
        style={{ top: 0, transform: 'translateY(-50%)' }}
      />
      {type === 'source' && ports.length > 1 && (
        <div className="pointer-events-none absolute left-4 top-0 -translate-y-1/2 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-1.5 py-0.5 text-[8px] font-bold text-muted shadow-md">
          {port.label}
        </div>
      )}
    </div>
  ));
}
