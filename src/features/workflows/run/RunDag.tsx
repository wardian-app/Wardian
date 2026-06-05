import { memo, useEffect, useMemo } from 'react';
import {
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
  type Node,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { findNodeType } from '../builder/registry';
import { toReactFlow } from '../builder/blueprintGraph';
import { nodeStatusColor } from './statusColors';
import { formatNodeStatus } from './statusLabels';
import type { Blueprint, BlueprintNode, PortDef } from '../builder/blueprintTypes';
import type { NodeStatusKind } from './runTypes';

interface RunDagProps {
  blueprint: Blueprint | null;
  currentStatuses: Record<string, NodeStatusKind>;
  selectedNodeId: string | null;
  onSelectNode: (id: string | null) => void;
  theme: 'dark' | 'light' | 'system';
}

type RunNodeData = {
  node: BlueprintNode;
  status: NodeStatusKind;
  statusColor: string;
};

export function RunDag({ blueprint, currentStatuses, selectedNodeId, onSelectNode, theme }: RunDagProps) {
  return (
    <ReactFlowProvider>
      <RunDagInner
        blueprint={blueprint}
        currentStatuses={currentStatuses}
        selectedNodeId={selectedNodeId}
        onSelectNode={onSelectNode}
        theme={theme}
      />
    </ReactFlowProvider>
  );
}

function RunDagInner({ blueprint, currentStatuses, selectedNodeId, onSelectNode, theme }: RunDagProps) {
  const { fitView } = useReactFlow();

  const graph = useMemo(() => {
    if (!blueprint) return null;
    const rf = toReactFlow(blueprint);
    return {
      nodes: rf.nodes.map((node) => {
        const status = currentStatuses[node.id] ?? 'pending';
        return {
          ...node,
          selected: node.id === selectedNodeId,
          data: {
            ...(node.data as { node: BlueprintNode }),
            status,
            statusColor: nodeStatusColor(status),
          },
        };
      }),
      edges: rf.edges,
    };
  }, [blueprint, currentStatuses, selectedNodeId]);

  const flowColorMode: ColorMode = useMemo(() => {
    if (theme === 'system') {
      return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }
    return theme;
  }, [theme]);

  useEffect(() => {
    if (!graph || graph.nodes.length === 0) return;
    const frame = window.requestAnimationFrame(() => {
      void fitView({ padding: 0.2, duration: 120 });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [blueprint?.id, graph?.nodes.length, fitView]);

  if (!blueprint || !graph) {
    return (
      <div className="flex h-full items-center justify-center rounded-lg border border-dashed border-wardian-border bg-[var(--color-wardian-card)] p-6 text-center text-sm text-[var(--color-wardian-text-muted)]">
        Blueprint source not found — showing event trace only
      </div>
    );
  }

  return (
    <ReactFlow
      nodes={graph.nodes}
      edges={graph.edges}
      nodeTypes={nodeTypes}
      nodesDraggable={false}
      nodesConnectable={false}
      elementsSelectable
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

const RunNode = memo(({ data, selected }: NodeProps<Node<RunNodeData>>) => {
  const node = data.node;
  const def = findNodeType(node.type);
  return (
    <div
      data-status={data.status}
      data-testid={`run-dag-node-${node.id}`}
      className="w-[260px] rounded-lg border-2 bg-[var(--color-wardian-card)] px-3 py-3 shadow-md transition-all"
      style={{
        borderColor: selected ? 'var(--color-wardian-accent)' : data.statusColor,
        boxShadow: `0 0 0 1px color-mix(in srgb, ${data.statusColor}, transparent 55%)`,
      }}
    >
      {renderHandles('target', portsFor(def?.inputs ?? []), Position.Left)}
      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[9px] font-bold text-muted">{def?.label ?? node.type}</span>
          <span className="rounded border px-1.5 py-0.5 text-[9px] font-bold" style={{ borderColor: data.statusColor, color: data.statusColor }}>
            {formatNodeStatus(data.status)}
          </span>
        </div>
        <div className="truncate text-sm font-bold text-[var(--color-wardian-text)]">{node.name ?? node.id}</div>
        {def?.description ? <div className="line-clamp-2 text-[10px] leading-snug text-muted">{def.description}</div> : null}
      </div>
      {renderHandles('source', outputPorts(node, def?.outputs ?? [], def?.outputs_from_field), Position.Right)}
    </div>
  );
});

const RunGroupNode = memo(({ data, selected }: NodeProps<Node<RunNodeData>>) => {
  const node = data.node;
  const def = findNodeType(node.type);
  return (
    <div
      data-status={data.status}
      data-testid={`run-dag-node-${node.id}`}
      className="h-full min-h-[220px] w-full min-w-[360px] rounded-lg border-2 bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_20%)] px-4 py-3"
      style={{ borderColor: selected ? 'var(--color-wardian-accent)' : data.statusColor }}
    >
      {renderHandles('target', portsFor(def?.inputs ?? []), Position.Left)}
      <div className="flex items-center justify-between gap-3">
        <div>
          <div className="text-[10px] font-bold text-muted">{def?.label ?? 'Loop'}</div>
          <div className="mt-1 text-sm font-bold text-[var(--color-wardian-text)]">{node.name ?? node.id}</div>
        </div>
        <span className="rounded border px-1.5 py-0.5 text-[9px] font-bold" style={{ borderColor: data.statusColor, color: data.statusColor }}>
          {formatNodeStatus(data.status)}
        </span>
      </div>
      {renderHandles('source', outputPorts(node, def?.outputs ?? [], def?.outputs_from_field), Position.Right)}
    </div>
  );
});

const nodeTypes = {
  wardian: RunNode,
  group: RunGroupNode,
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
      {type === 'source' && ports.length > 1 ? (
        <div className="pointer-events-none absolute left-4 top-0 -translate-y-1/2 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-1.5 py-0.5 text-[8px] font-bold text-muted shadow-md">
          {port.label}
        </div>
      ) : null}
    </div>
  ));
}
