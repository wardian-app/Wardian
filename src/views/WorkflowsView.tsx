import { useEffect, useMemo, useState } from 'react';
import { BlueprintSelector } from '../features/workflows/BlueprintSelector';
import { RunControls, type RunControlStatus } from '../features/workflows/RunControls';
import { RunLaunchDialog, type RunInputParam } from '../features/workflows/RunLaunchDialog';
import { BuilderCanvas } from '../features/workflows/builder/BuilderCanvas';
import { BuilderToolbar } from '../features/workflows/builder/BuilderToolbar';
import { DiagnosticsPanel } from '../features/workflows/builder/DiagnosticsPanel';
import { NodeConfigForm } from '../features/workflows/builder/NodeConfigForm';
import { NodePalette } from '../features/workflows/builder/NodePalette';
import { VariableAssistantV2 } from '../features/workflows/builder/VariableAssistantV2';
import type { Blueprint, BlueprintNode, NodeTypeDef } from '../features/workflows/builder/blueprintTypes';
import { EventTimeline } from '../features/workflows/run/EventTimeline';
import { WorkflowMonitor } from '../features/workflows/monitor/WorkflowMonitor';
import { NodeInspector } from '../features/workflows/run/NodeInspector';
import { RunDag } from '../features/workflows/run/RunDag';
import { RunList } from '../features/workflows/run/RunList';
import type { RunStatusKind } from '../features/workflows/run/runTypes';
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
  const [drawerOpen, setDrawerOpen] = useState(true);
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
    <div data-testid="workflows-edit-mode" className="flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <BuilderToolbar />
      <div className="grid h-full min-h-0 flex-1 grid-cols-[220px_minmax(0,1fr)_320px]">
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
              <VariableAssistantV2 blueprint={activeBlueprint} selectedNodeId={selectedNode.id} />
            </>
          ) : (
            <div className="rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-3 text-xs text-muted">
              Select a node to edit its fields.
            </div>
          )}
        </aside>
      </div>
      <DiagnosticsPanel diagnostics={diagnostics} onFocusNode={setSelectedNodeId} />
    </div>
  );
}

function WorkflowObserveMode({ theme }: WorkflowsViewProps) {
  const state = useRunStore((store) => store.state);
  const runs = useRunStore((store) => store.runs);
  const events = useRunStore((store) => store.events);
  const blueprint = useRunStore((store) => store.blueprint);
  const scrubIndex = useRunStore((store) => store.scrubIndex);
  const setScrubIndex = useRunStore((store) => store.setScrubIndex);
  const currentNodeStatuses = useRunStore((store) => store.currentNodeStatuses);
  const openRun = useRunStore((store) => store.openRun);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);

  const statuses = currentNodeStatuses();
  const awaitingNode = useMemo(() => {
    if (state?.status !== 'awaiting_approval') return null;
    return [...events].reverse().find((event) => event.kind === 'awaiting_approval')?.node ?? null;
  }, [events, state?.status]);

  const controlsStatus = toRunControlStatus(state?.status);
  const activeRunPath = state
    ? runs.find((run) => run.blueprint_id === state.blueprint_id && run.run_id === state.run_id)?.path ?? ''
    : '';

  return (
    <div data-testid="workflows-observe-mode" className="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)] overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <div className="flex min-h-[48px] items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-4">
        <div className="min-w-0">
          <div className="truncate text-xs font-bold text-[var(--color-wardian-text)]">{state?.run_id ?? 'No run selected'}</div>
          <div className="mt-0.5 truncate text-[10px] font-mono text-muted">{state?.blueprint_id ?? 'Open a run from the drawer'}</div>
        </div>
        {state && controlsStatus ? (
          <RunControls
            blueprintId={state.blueprint_id}
            runId={state.run_id}
            blueprintPath={activeRunPath}
            status={controlsStatus}
            awaitingNode={awaitingNode}
            onChanged={() => void openRun(state.blueprint_id, state.run_id)}
          />
        ) : null}
      </div>
      <div className="grid min-h-0 grid-cols-[minmax(0,1fr)_320px]">
        <section className="grid min-h-0 grid-rows-[minmax(0,1fr)_220px]">
          <div className="min-h-0 p-3">
            <RunDag
              blueprint={blueprint}
              currentStatuses={statuses}
              selectedNodeId={selectedNodeId}
              onSelectNode={setSelectedNodeId}
              theme={theme}
            />
          </div>
          <div className="min-h-0 border-t border-wardian-border p-3">
            <EventTimeline events={events} scrubIndex={scrubIndex} onScrub={setScrubIndex} />
          </div>
        </section>
        <aside className="min-h-0 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
          <NodeInspector
            selectedNodeId={selectedNodeId}
            state={state}
            currentStatuses={statuses}
            events={events}
          />
        </aside>
      </div>
    </div>
  );
}

function toRunControlStatus(status: RunStatusKind | 'interrupted' | undefined): RunControlStatus | null {
  if (!status) return null;
  if (status === 'running' || status === 'awaiting_approval' || status === 'completed' || status === 'failed' || status === 'interrupted') {
    return status;
  }
  return null;
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
  return inputParamsFromSchema(entry.fields.input_schema);
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
