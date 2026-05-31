import { useEffect, useMemo, useState } from 'react';
import { BlueprintSelector } from '../features/workflows/BlueprintSelector';
import { RunLaunchDialog, type RunInputParam } from '../features/workflows/RunLaunchDialog';
import { BuilderCanvas } from '../features/workflows/builder/BuilderCanvas';
import { BuilderToolbar } from '../features/workflows/builder/BuilderToolbar';
import { DiagnosticsPanel } from '../features/workflows/builder/DiagnosticsPanel';
import { NodeConfigForm } from '../features/workflows/builder/NodeConfigForm';
import { NodeLibrary } from '../features/workflows/builder/NodeLibrary';
import { VariableAssistantV2 } from '../features/workflows/builder/VariableAssistantV2';
import type { Blueprint, BlueprintNode, NodeTypeDef } from '../features/workflows/builder/blueprintTypes';
import { WorkflowMonitor } from '../features/workflows/monitor/WorkflowMonitor';
import { RunList } from '../features/workflows/run/RunList';
import { WorkflowObserveMode } from '../features/workflows/run/WorkflowObserveMode';
import { useRunStore } from '../features/workflows/run/useRunStore';
import { useBuilderStore } from '../store/useBuilderStore';
import { useSchedulesStore } from '../store/useSchedulesStore';
import { useWorkflowsView } from '../store/useWorkflowsView';
import type { WorkflowSchedule } from '../types/workflow';

interface WorkflowsViewProps {
  theme: 'dark' | 'light' | 'system';
}

const INITIAL_BLUEPRINT: Blueprint = {
  schema: 2,
  id: 'new-workflow',
  name: 'New Workflow',
  nodes: [],
  edges: [],
};

const EMPTY_INPUT_PARAMS: RunInputParam[] = [];

export function WorkflowsView({ theme }: WorkflowsViewProps) {
  const mode = useWorkflowsView((state) => state.mode);
  const blueprintPath = useWorkflowsView((state) => state.blueprintPath);
  const selectedRunId = useWorkflowsView((state) => state.selectedRunId);
  const setMode = useWorkflowsView((state) => state.setMode);
  const setBlueprintPath = useWorkflowsView((state) => state.setBlueprintPath);
  const observeRun = useWorkflowsView((state) => state.observeRun);

  const blueprint = useBuilderStore((state) => state.blueprint);
  const loadBlueprint = useBuilderStore((state) => state.load);
  const resetBuilder = useBuilderStore((state) => state.reset);
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const hasErrors = useBuilderStore((state) => state.hasErrors);

  const runs = useRunStore((state) => state.runs);
  const runState = useRunStore((state) => state.state);
  const runBlueprint = useRunStore((state) => state.blueprint);
  const loadRuns = useRunStore((state) => state.loadRuns);
  const openRun = useRunStore((state) => state.openRun);
  const subscribeSchedules = useSchedulesStore((state) => state.subscribe);
  const loadSchedules = useSchedulesStore((state) => state.load);

  const [launchOpen, setLaunchOpen] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editSchedule, setEditSchedule] = useState<WorkflowSchedule | null>(null);

  const activeBlueprintId = blueprint?.id ?? runState?.blueprint_id ?? runBlueprint?.id ?? null;
  const filteredRuns = useMemo(
    () => (activeBlueprintId ? runs.filter((run) => run.blueprint_id === activeBlueprintId) : runs),
    [activeBlueprintId, runs],
  );
  const inputParams = useMemo(() => inputParamsFromBlueprint(blueprint), [blueprint]);
  const runDisabled = !blueprintPath || hasErrors();

  useEffect(() => {
    void loadRuns();
  }, [loadRuns]);

  useEffect(() => {
    void loadSchedules();
    let unlisten: (() => void) | undefined;
    void subscribeSchedules().then((listener) => {
      unlisten = listener;
    });
    return () => unlisten?.();
  }, [loadSchedules, subscribeSchedules]);

  useEffect(() => {
    if (!blueprint) {
      setBlueprint(INITIAL_BLUEPRINT);
    }
  }, [blueprint, setBlueprint]);

  useEffect(() => {
    setDrawerOpen(mode === 'monitor');
  }, [mode]);

  useEffect(() => {
    if (mode !== 'observe' || !selectedRunId || !activeBlueprintId) return;
    if (runState?.run_id === selectedRunId && runState.blueprint_id === activeBlueprintId) return;

    void openRun(activeBlueprintId, selectedRunId);
  }, [activeBlueprintId, mode, openRun, runState?.blueprint_id, runState?.run_id, selectedRunId]);

  useEffect(() => {
    if (mode !== 'observe' || runState?.status !== 'running') return;

    const timer = window.setInterval(() => {
      void openRun(runState.blueprint_id, runState.run_id);
    }, 1500);

    return () => window.clearInterval(timer);
  }, [mode, openRun, runState?.blueprint_id, runState?.run_id, runState?.status]);

  const openBlueprint = async (path: string) => {
    setBlueprintPath(path);
    await loadBlueprint(path);
    setMode('edit');
  };

  const newBlueprint = () => {
    resetBuilder();
    setBlueprint(INITIAL_BLUEPRINT);
    setBlueprintPath(null);
    setMode('edit');
  };

  const openRunForObserve = async (blueprintId: string, runId: string) => {
    await openRun(blueprintId, runId);
    observeRun(runId);
  };

  const handleLaunched = async (runId: string) => {
    setLaunchOpen(false);
    await loadRuns();
    const blueprintId = activeBlueprintId ?? blueprint?.id;
    if (!blueprintId) return;
    await openRun(blueprintId, runId);
    observeRun(runId);
  };

  return (
    <div data-testid="workflows-view" className="relative flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)] text-primary">
      <div className="flex h-14 shrink-0 items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-4">
        <div className="flex min-w-0 items-center gap-3">
          <BlueprintSelector onOpen={(path) => void openBlueprint(path)} onNew={newBlueprint} />
          <div className="flex rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-0.5" aria-label="Workflow mode">
            {(['edit', 'observe', 'monitor'] as const).map((value) => (
              <button
                key={value}
                type="button"
                className={`rounded px-3 py-1 text-xs font-bold capitalize transition-colors ${
                  mode === value
                    ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]'
                    : 'text-muted hover:text-[var(--color-wardian-text)]'
                }`}
                onClick={() => setMode(value)}
              >
                {value}
              </button>
            ))}
          </div>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="rounded border border-wardian-border px-3 py-1.5 text-xs font-bold text-muted transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
            onClick={() => setDrawerOpen((open) => !open)}
          >
            {drawerOpen ? 'Hide Runs' : 'Show Runs'}
          </button>
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-accent)] px-4 py-1.5 text-xs font-bold text-[var(--color-wardian-bg)] transition-opacity disabled:cursor-not-allowed disabled:opacity-40"
            disabled={runDisabled}
            onClick={() => setLaunchOpen(true)}
          >
            Run
          </button>
        </div>
      </div>

      <div className={`grid min-h-0 flex-1 ${drawerOpen ? 'grid-cols-[minmax(0,1fr)_280px]' : 'grid-cols-[minmax(0,1fr)]'}`}>
        <main className="h-full min-h-0 overflow-hidden p-3">
          {mode === 'edit' ? (
            <WorkflowEditMode theme={theme} />
          ) : mode === 'observe' ? (
            <WorkflowObserveMode theme={theme} />
          ) : (
            <WorkflowMonitor
              onOpenRun={(blueprintId, runId) => void openRunForObserve(blueprintId, runId)}
              onEditSchedule={(schedule) => {
                setEditSchedule(schedule);
                setLaunchOpen(true);
              }}
            />
          )}
        </main>
        {drawerOpen && (
          <aside className="min-h-0 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div>
                <h2 className="text-sm font-bold text-[var(--color-wardian-text)]">Runs</h2>
                <p className="mt-0.5 text-[10px] text-muted">{activeBlueprintId ?? 'All blueprints'}</p>
              </div>
              <button
                type="button"
                onClick={() => void loadRuns()}
                className="rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
              >
                Refresh
              </button>
            </div>
            <RunList
              runs={filteredRuns}
              selectedRunId={runState?.run_id ?? selectedRunId}
              onOpen={(blueprintId, runId) => void openRunForObserve(blueprintId, runId)}
            />
          </aside>
        )}
      </div>

      {launchOpen && (blueprintPath || editSchedule) && (
        <div className="absolute inset-0 z-20 flex items-start justify-center bg-[color-mix(in_srgb,var(--color-wardian-bg),transparent_25%)] p-8">
          <RunLaunchDialog
            path={blueprintPath ?? ''}
            blueprintId={editSchedule?.blueprint_id ?? activeBlueprintId ?? undefined}
            inputParams={editSchedule && blueprint?.id !== editSchedule.blueprint_id ? EMPTY_INPUT_PARAMS : inputParams}
            editSchedule={editSchedule ?? undefined}
            onLaunched={(runId) => void handleLaunched(runId)}
            onScheduled={() => {
              setLaunchOpen(false);
              setEditSchedule(null);
              void loadSchedules();
            }}
            onCancel={() => {
              setLaunchOpen(false);
              setEditSchedule(null);
            }}
          />
        </div>
      )}
    </div>
  );
}

function WorkflowEditMode({ theme }: WorkflowsViewProps) {
  const blueprint = useBuilderStore((state) => state.blueprint);
  const diagnostics = useBuilderStore((state) => state.diagnostics);
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const validate = useBuilderStore((state) => state.validate);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [libraryOpen, setLibraryOpen] = useState(false);

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
    const node: BlueprintNode = {
      id: nextId,
      type: def.id,
      name: def.label,
      fields: defaultFields(def),
    };
    setBlueprint({ ...activeBlueprint, nodes: [...activeBlueprint.nodes, node] });
    setSelectedNodeId(node.id);
  };

  const addNodeFromLibrary = (def: NodeTypeDef) => {
    addNode(def);
    setLibraryOpen(false);
  };

  return (
    <div data-testid="workflows-edit-mode" className="relative flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <BuilderToolbar />
      <div className="flex min-h-[44px] shrink-0 items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-3">
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-1.5 text-xs font-bold text-[var(--color-wardian-text)] transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
            onClick={() => setLibraryOpen(true)}
          >
            Add node
          </button>
        </div>
        <div className="truncate text-[10px] font-mono text-muted">
          {activeBlueprint.nodes.length} nodes / {activeBlueprint.edges.length} edges
        </div>
      </div>
      <div className={`grid h-full min-h-0 flex-1 ${selectedNode ? 'grid-cols-[minmax(0,1fr)_320px]' : 'grid-cols-[minmax(0,1fr)]'}`}>
        <section className="relative min-h-0">
          <BuilderCanvas
            blueprint={activeBlueprint}
            diagnostics={diagnostics}
            selectedNodeId={selectedNodeId}
            onSelectNode={setSelectedNodeId}
            theme={theme}
          />
          {activeBlueprint.nodes.length === 0 ? (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-6">
              <div className="pointer-events-auto max-w-sm rounded-lg border border-dashed border-wardian-border bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_8%)] p-4 text-center shadow-lg">
                <div className="text-sm font-bold text-[var(--color-wardian-text)]">Start from the registry</div>
                <div className="mt-1 text-xs leading-relaxed text-muted">Add a trigger, agent, task decision, or control node. Layout is applied automatically.</div>
                <button
                  type="button"
                  className="mt-3 rounded bg-[var(--color-wardian-accent)] px-3 py-1.5 text-xs font-bold text-[var(--color-wardian-bg)]"
                  onClick={() => setLibraryOpen(true)}
                >
                  Browse registry
                </button>
              </div>
            </div>
          ) : null}
        </section>
        {selectedNode ? (
          <aside className="flex min-h-0 flex-col gap-3 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
            <div className="flex items-center justify-between gap-3">
              <div>
                <div className="text-xs font-bold text-[var(--color-wardian-text)]">Inspector</div>
                <div className="mt-0.5 truncate text-[10px] font-mono text-muted">{selectedNode.id}</div>
              </div>
              <button
                type="button"
                className="rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                onClick={() => setSelectedNodeId(null)}
              >
                Close
              </button>
            </div>
            <div className="grid gap-3">
              <NodeConfigForm node={selectedNode} onChange={updateNodeField} />
              <VariableAssistantV2 blueprint={activeBlueprint} selectedNodeId={selectedNode.id} />
            </div>
          </aside>
        ) : null}
      </div>
      <DiagnosticsPanel diagnostics={diagnostics} onFocusNode={setSelectedNodeId} />
      {libraryOpen ? (
        <div className="absolute inset-0 z-30 flex items-start justify-center bg-[color-mix(in_srgb,var(--color-wardian-bg),transparent_20%)] p-8">
          <NodeLibrary mode="popover" onAdd={addNodeFromLibrary} onClose={() => setLibraryOpen(false)} />
        </div>
      ) : null}
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

function inputParamsFromBlueprint(blueprint: Blueprint | null): RunInputParam[] {
  const entry = blueprint?.nodes.find((node) => node.type === 'manual_trigger');
  if (!entry) return [];
  return inputParamsFromSchema(entry.fields?.input_schema);
}

function inputParamsFromSchema(inputSchema: unknown): RunInputParam[] {
  const schema = normalizeInputSchema(inputSchema);
  if (!isRecord(schema)) return [];

  if (isRecord(schema.properties)) {
    return Object.entries(schema.properties)
      .map(([name, value]) => paramFromSchemaEntry(name, value))
      .filter((param): param is RunInputParam => Boolean(param));
  }

  return Object.entries(schema)
    .filter(([name]) => name !== 'type' && name !== 'required' && name !== 'properties')
    .map(([name, value]) => paramFromSchemaEntry(name, value))
    .filter((param): param is RunInputParam => Boolean(param));
}

function normalizeInputSchema(inputSchema: unknown): unknown {
  if (!inputSchema) return null;
  if (typeof inputSchema !== 'string') return inputSchema;
  const trimmed = inputSchema.trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return null;
  }
}

function paramFromSchemaEntry(name: string, value: unknown): RunInputParam | null {
  const rawType = typeof value === 'string'
    ? value
    : isRecord(value) && typeof value.type === 'string'
      ? value.type
      : 'string';
  const type = normalizeParamType(rawType);
  if (!type) return null;
  return { name, type };
}

function normalizeParamType(type: string): RunInputParam['type'] | null {
  if (type === 'string') return 'string';
  if (type === 'number' || type === 'integer') return 'number';
  if (type === 'boolean' || type === 'bool') return 'boolean';
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
