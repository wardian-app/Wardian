import { useEffect, useMemo, useState } from 'react';
import { BuilderCanvas } from '../features/workflows/builder/BuilderCanvas';
import { BuilderToolbar } from '../features/workflows/builder/BuilderToolbar';
import { DiagnosticsPanel } from '../features/workflows/builder/DiagnosticsPanel';
import { NodeConfigForm } from '../features/workflows/builder/NodeConfigForm';
import { NodePalette } from '../features/workflows/builder/NodePalette';
import { VariableAssistant } from '../features/workflows/builder/VariableAssistant';
import type { Blueprint, BlueprintNode, NodeTypeDef } from '../features/workflows/builder/blueprintTypes';
import { useBuilderStore } from '../store/useBuilderStore';

interface BuilderViewProps {
  theme: 'dark' | 'light' | 'system';
}

const INITIAL_BLUEPRINT: Blueprint = {
  schema: 2,
  id: 'new-workflow',
  name: 'New Workflow',
  nodes: [],
  edges: [],
};

export function BuilderView({ theme }: BuilderViewProps) {
  const blueprint = useBuilderStore((state) => state.blueprint);
  const diagnostics = useBuilderStore((state) => state.diagnostics);
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const validate = useBuilderStore((state) => state.validate);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);

  useEffect(() => {
    if (!blueprint) setBlueprint(INITIAL_BLUEPRINT);
  }, [blueprint, setBlueprint]);

  useEffect(() => {
    if (!blueprint) return;
    const timer = window.setTimeout(() => {
      void validate();
    }, 400);
    return () => window.clearTimeout(timer);
  }, [blueprint, validate]);

  const activeBlueprint = blueprint ?? INITIAL_BLUEPRINT;
  const selectedNode = useMemo(
    () => activeBlueprint.nodes.find((node) => node.id === selectedNodeId) ?? null,
    [activeBlueprint.nodes, selectedNodeId],
  );

  const updateNodeField = (field: string, value: unknown) => {
    if (!selectedNode) return;
    setBlueprint({
      ...activeBlueprint,
      nodes: activeBlueprint.nodes.map((node) => (
        node.id === selectedNode.id
          ? { ...node, fields: { ...node.fields, [field]: value } }
          : node
      )),
    });
  };

  const addNode = (def: NodeTypeDef) => {
    const nextId = nextNodeId(activeBlueprint.nodes, def.id);
    const index = activeBlueprint.nodes.length;
    const node: BlueprintNode = {
      id: nextId,
      type: def.id,
      name: def.label,
      fields: defaultFields(def),
      position: { x: 120 + index * 80, y: 120 + index * 40 },
    };
    setBlueprint({ ...activeBlueprint, nodes: [...activeBlueprint.nodes, node] });
    setSelectedNodeId(node.id);
  };

  return (
    <div data-testid="workflow-builder" className="flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <BuilderToolbar />
      <div className="grid min-h-0 flex-1 grid-cols-[220px_minmax(0,1fr)_320px]">
        <aside className="min-h-0 overflow-y-auto border-r border-wardian-border bg-[var(--color-wardian-card)] p-3">
          <NodePalette onAdd={addNode} />
        </aside>
        <section className="min-h-0">
          <BuilderCanvas
            blueprint={activeBlueprint}
            diagnostics={diagnostics}
            selectedNodeId={selectedNodeId}
            onSelectNode={setSelectedNodeId}
            theme={theme}
          />
        </section>
        <aside className="flex min-h-0 flex-col gap-3 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
          {selectedNode ? (
            <>
              <NodeConfigForm node={selectedNode} onChange={updateNodeField} />
              <VariableAssistant blueprint={activeBlueprint} selectedNodeId={selectedNode.id} />
            </>
          ) : (
            <div className="rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)] p-3 text-xs text-muted">
              Select a node to edit its fields.
            </div>
          )}
        </aside>
      </div>
      <DiagnosticsPanel diagnostics={diagnostics} onFocusNode={setSelectedNodeId} />
    </div>
  );
}

function nextNodeId(nodes: BlueprintNode[], type: string) {
  let index = nodes.filter((node) => node.type === type).length + 1;
  let id = `${type}-${index}`;
  while (nodes.some((node) => node.id === id)) {
    index += 1;
    id = `${type}-${index}`;
  }
  return id;
}

function defaultFields(def: NodeTypeDef) {
  return Object.fromEntries(
    def.fields
      .filter((field) => field.default !== undefined)
      .map((field) => [field.id, field.default]),
  );
}
