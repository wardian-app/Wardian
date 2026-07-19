import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { RefreshCw, X } from 'lucide-react';
import { BlueprintSelector } from '../features/workflows/BlueprintSelector';
import { RunLaunchDialog, type RunInputParam } from '../features/workflows/RunLaunchDialog';
import { BuilderCanvas } from '../features/workflows/builder/BuilderCanvas';
import { DiagnosticsPanel } from '../features/workflows/builder/DiagnosticsPanel';
import { NodeConfigForm } from '../features/workflows/builder/NodeConfigForm';
import { NodeLibrary } from '../features/workflows/builder/NodeLibrary';
import { VariableAssistant } from '../features/workflows/builder/VariableAssistant';
import type { Blueprint, BlueprintNode, NodeTypeDef } from '../features/workflows/builder/blueprintTypes';
import { findNodeType } from '../features/workflows/builder/registry';
import { WorkflowMonitor } from '../features/workflows/monitor/WorkflowMonitor';
import { RunList } from '../features/workflows/run/RunList';
import { WorkflowObserveMode } from '../features/workflows/run/WorkflowObserveMode';
import { useRunStore } from '../features/workflows/run/useRunStore';
import type { RunSummary } from '../features/workflows/run/runTypes';
import { ContextMenu, type ContextMenuItem } from '../components/ContextMenu';
import { useBuilderStore } from '../store/useBuilderStore';
import { useSchedulesStore } from '../store/useSchedulesStore';
import { useWorkflowsView } from '../store/useWorkflowsView';
import type { WorkflowSchedule } from '../types/workflow';

export interface WorkflowsViewProps {
  theme: 'dark' | 'light' | 'system';
}

const INITIAL_BLUEPRINT: Blueprint = {
  schema: 2,
  id: 'new-workflow',
  name: 'New Workflow',
  nodes: [],
  edges: [],
};

export function WorkflowsView({ theme }: WorkflowsViewProps) {
  const mode = useWorkflowsView((state) => state.mode);
  const blueprintPath = useWorkflowsView((state) => state.blueprintPath);
  const selectedRunId = useWorkflowsView((state) => state.selectedRunId);
  const observedBlueprintId = useWorkflowsView((state) => state.observedBlueprintId);
  const selectedRunIdsByBlueprint = useWorkflowsView((state) => state.selectedRunIdsByBlueprint);
  const setMode = useWorkflowsView((state) => state.setMode);
  const setBlueprintPath = useWorkflowsView((state) => state.setBlueprintPath);
  const observeRun = useWorkflowsView((state) => state.observeRun);
  const clearObservedRun = useWorkflowsView((state) => state.clearObservedRun);

  const blueprint = useBuilderStore((state) => state.blueprint);
  const loadBlueprint = useBuilderStore((state) => state.load);
  const resetBuilder = useBuilderStore((state) => state.reset);
  const initializeBlueprint = useBuilderStore((state) => state.initialize);
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const dirty = useBuilderStore((state) => state.dirty);
  const saveBlueprint = useBuilderStore((state) => state.save);
  const diagnostics = useBuilderStore((state) => state.diagnostics);

  const runs = useRunStore((state) => state.runs);
  const runState = useRunStore((state) => state.state);
  const runBlueprint = useRunStore((state) => state.blueprint);
  const loadRuns = useRunStore((state) => state.loadRuns);
  const openRun = useRunStore((state) => state.openRun);
  const clearOpenRun = useRunStore((state) => state.clearOpenRun);
  const subscribeSchedules = useSchedulesStore((state) => state.subscribe);
  const loadSchedules = useSchedulesStore((state) => state.load);

  const [launchOpen, setLaunchOpen] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editSchedule, setEditSchedule] = useState<WorkflowSchedule | null>(null);
  const [editScheduleBlueprint, setEditScheduleBlueprint] = useState<Blueprint | null>(null);
  const [editSchedulePath, setEditSchedulePath] = useState<string | null>(null);
  const [addNodeRequest, setAddNodeRequest] = useState(0);
  const pendingObserveRef = useRef<{ blueprintId: string; runId: string } | null>(null);

  const activeBlueprintId = mode === 'observe'
    ? observedBlueprintId ?? runState?.blueprint_id ?? runBlueprint?.id ?? blueprint?.id ?? null
    : blueprint?.id ?? runState?.blueprint_id ?? runBlueprint?.id ?? null;
  const filteredRuns = useMemo(
    () => (activeBlueprintId ? runs.filter((run) => run.blueprint_id === activeBlueprintId) : runs),
    [activeBlueprintId, runs],
  );
  const inputParams = useMemo(() => inputParamsFromBlueprint(blueprint), [blueprint]);
  const invalid = diagnostics.some((diagnostic) => diagnostic.severity === 'error');
  const runDisabled = !blueprintPath || invalid;

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
      initializeBlueprint(INITIAL_BLUEPRINT);
    }
  }, [blueprint, initializeBlueprint]);

  useEffect(() => {
    if (mode === 'monitor') {
      setDrawerOpen(false);
    }
  }, [mode]);

  useEffect(() => {
    if (mode !== 'observe' || !activeBlueprintId) return;

    const rememberedRunId = selectedRunIdsByBlueprint[activeBlueprintId];
    const targetRun = chooseRunForObserve(activeBlueprintId, runs, rememberedRunId);
    if (!targetRun) {
      if (runState && runState.blueprint_id !== activeBlueprintId) {
        clearOpenRun();
        clearObservedRun(activeBlueprintId);
      }
      return;
    }
    if (runState?.run_id === targetRun.run_id && runState.blueprint_id === activeBlueprintId) return;
    const pending = pendingObserveRef.current;
    if (pending?.blueprintId === activeBlueprintId && pending.runId === targetRun.run_id) return;

    observeRun(activeBlueprintId, targetRun.run_id);
    pendingObserveRef.current = { blueprintId: activeBlueprintId, runId: targetRun.run_id };
    void openRun(activeBlueprintId, targetRun.run_id).finally(() => {
      const current = pendingObserveRef.current;
      if (current?.blueprintId === activeBlueprintId && current.runId === targetRun.run_id) {
        pendingObserveRef.current = null;
      }
    });
  }, [
    activeBlueprintId,
    clearObservedRun,
    clearOpenRun,
    mode,
    observeRun,
    openRun,
    runState,
    runState?.blueprint_id,
    runState?.run_id,
    runs,
    selectedRunIdsByBlueprint,
  ]);

  useEffect(() => {
    if (mode !== 'observe' || runState?.status !== 'running') return;

    const timer = window.setInterval(() => {
      void openRun(runState.blueprint_id, runState.run_id);
    }, 1500);

    return () => window.clearInterval(timer);
  }, [mode, openRun, runState?.blueprint_id, runState?.run_id, runState?.status]);

  const openBlueprint = async (path: string) => {
    const currentMode = mode;
    setBlueprintPath(path);
    await loadBlueprint(path);
    const loadedBlueprint = useBuilderStore.getState().blueprint;

    if (currentMode === 'observe' && loadedBlueprint?.id) {
      await loadRuns();
      const freshRuns = useRunStore.getState().runs;
      const rememberedRunId = useWorkflowsView.getState().selectedRunIdsByBlueprint[loadedBlueprint.id];
      const targetRun = chooseRunForObserve(loadedBlueprint.id, freshRuns, rememberedRunId);
      if (targetRun) {
        await openRunForObserve(loadedBlueprint.id, targetRun.run_id);
      } else {
        clearOpenRun();
        clearObservedRun();
      }
      return;
    }

    if (currentMode === 'monitor') {
      return;
    }

    setMode('edit');
  };

  const newBlueprint = () => {
    resetBuilder();
    setBlueprint(INITIAL_BLUEPRINT);
    setBlueprintPath(null);
    setMode('edit');
  };

  const openRunForObserve = async (blueprintId: string, runId: string) => {
    pendingObserveRef.current = { blueprintId, runId };
    observeRun(blueprintId, runId);
    try {
      await openRun(blueprintId, runId);
    } finally {
      const current = pendingObserveRef.current;
      if (current?.blueprintId === blueprintId && current.runId === runId) {
        pendingObserveRef.current = null;
      }
    }
  };

  const handleLaunched = async (runId: string) => {
    setLaunchOpen(false);
    await loadRuns();
    const blueprintId = activeBlueprintId ?? blueprint?.id;
    if (!blueprintId) return;
    await openRunForObserve(blueprintId, runId);
  };

  const openScheduleEditor = async (schedule: WorkflowSchedule) => {
    const resolved = await resolveScheduleBlueprint(schedule, blueprint);
    setEditSchedule(schedule);
    setEditScheduleBlueprint(resolved.blueprint);
    setEditSchedulePath(resolved.path);
    setLaunchOpen(true);
  };

  const refreshCurrentMode = async () => {
    await loadRuns();
    if (mode === 'monitor') {
      await loadSchedules();
      return;
    }
    if (mode === 'observe' && activeBlueprintId) {
      const targetRunId = selectedRunIdsByBlueprint[activeBlueprintId] ?? selectedRunId;
      if (targetRunId) {
        await openRun(activeBlueprintId, targetRunId);
      }
    }
  };

  return (
    <div data-testid="workflows-view" className="relative flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)] text-primary">
      <div className={`workflows-toolbar ${mode === 'monitor' ? 'workflows-toolbar--monitor' : ''} flex min-h-14 shrink-0 items-center justify-between gap-3 border-b border-wardian-border bg-[var(--color-wardian-card)] px-3`}>
        <div className={`workflows-toolbar__primary flex min-w-0 flex-1 items-center gap-2 ${mode === 'monitor' ? 'flex-nowrap' : ''}`}>
          <BlueprintSelector selectedPath={blueprintPath} onOpen={(path) => void openBlueprint(path)} onNew={newBlueprint} />
          <div className="flex rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-0.5" aria-label="Workflow mode">
            {(['edit', 'observe', 'monitor'] as const).map((value) => (
              <button
                key={value}
                type="button"
                className={`rounded px-3 py-1 text-xs font-bold capitalize transition-colors ${
                  mode === value
                    ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]'
                    : 'text-muted hover:text-[var(--color-wardian-text)]'
                } cursor-pointer select-none`}
                onClick={() => setMode(value)}
              >
                {value}
              </button>
            ))}
          </div>
          {mode === 'edit' ? (
            <input
              aria-label="Workflow name"
              value={blueprint?.name ?? ''}
              disabled={!blueprint}
              onChange={(event) => blueprint && setBlueprint({ ...blueprint, name: event.target.value })}
              className="min-w-[180px] max-w-[260px] flex-1 rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-1.5 text-sm font-bold text-[var(--color-wardian-text)] outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)] disabled:opacity-50"
            />
          ) : (
            <div className="min-w-0 truncate px-2 text-sm font-bold text-[var(--color-wardian-text)]">
              {runBlueprint?.name ?? blueprint?.name ?? activeBlueprintId ?? 'Workflows'}
            </div>
          )}
        </div>
        <div className={`workflows-toolbar__actions flex shrink-0 items-center gap-2 ${mode === 'monitor' ? 'flex-nowrap' : ''}`}>
          {mode === 'edit' ? (
            <button
              type="button"
              className="cursor-pointer select-none rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-3 py-1.5 text-xs font-bold text-[var(--color-wardian-text)] transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
              onClick={() => setAddNodeRequest((request) => request + 1)}
            >
              Add node
            </button>
          ) : null}
          {mode !== 'monitor' ? (
            <button
              type="button"
              className="cursor-pointer select-none rounded border border-wardian-border px-3 py-1.5 text-xs font-bold text-muted transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
              onClick={() => setDrawerOpen((open) => !open)}
            >
              {drawerOpen ? 'Hide Runs' : 'Show Runs'}
            </button>
          ) : null}
          {mode !== 'edit' ? (
            <button
              type="button"
              className="inline-flex h-8 w-8 cursor-pointer select-none items-center justify-center rounded border border-wardian-border text-muted transition-colors hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
              onClick={() => void refreshCurrentMode()}
              aria-label="Refresh workflow view"
              title="Refresh"
            >
              <RefreshCw className="h-3.5 w-3.5" aria-hidden />
            </button>
          ) : null}
          <button
            type="button"
            className="cursor-pointer select-none rounded bg-[var(--color-wardian-accent)] px-4 py-1.5 text-xs font-bold text-[var(--color-wardian-bg)] transition-opacity disabled:cursor-not-allowed disabled:opacity-40"
            disabled={runDisabled}
            onClick={() => setLaunchOpen(true)}
          >
            Run
          </button>
          {mode === 'edit' ? (
            <button
              type="button"
              disabled={!blueprint || invalid || !dirty}
              onClick={() => void saveBlueprint()}
              className="cursor-pointer select-none rounded bg-[var(--color-wardian-accent)] px-4 py-1.5 text-xs font-bold text-[var(--color-wardian-bg)] transition-opacity disabled:cursor-not-allowed disabled:opacity-40"
            >
              {dirty ? 'Save' : 'Saved'}
            </button>
          ) : null}
        </div>
      </div>

      <div className="workflows-body grid min-h-0 flex-1" data-drawer-open={drawerOpen ? 'true' : 'false'}>
        <main className="workflows-main h-full min-h-0 overflow-hidden p-3">
          {mode === 'edit' ? (
            <WorkflowEditMode theme={theme} addNodeRequest={addNodeRequest} />
          ) : mode === 'observe' ? (
            <WorkflowObserveMode theme={theme} />
          ) : (
            <WorkflowMonitor
              onOpenRun={(blueprintId, runId) => void openRunForObserve(blueprintId, runId)}
              onEditSchedule={(schedule) => void openScheduleEditor(schedule)}
            />
          )}
        </main>
        {drawerOpen && (
          <aside className="workflows-run-drawer min-h-0 overflow-y-auto border-l border-wardian-border bg-[var(--color-wardian-card)] p-3">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div>
                <h2 className="text-sm font-bold text-[var(--color-wardian-text)]">Runs</h2>
                <p className="mt-0.5 text-[10px] text-muted">{activeBlueprintId ?? 'All blueprints'}</p>
              </div>
              <button
                type="button"
                onClick={() => void loadRuns()}
                className="inline-flex h-7 w-7 cursor-pointer select-none items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                aria-label="Refresh runs"
                title="Refresh"
              >
                <RefreshCw className="h-3.5 w-3.5" aria-hidden />
              </button>
            </div>
            <RunList
              runs={filteredRuns}
              selectedRunId={activeBlueprintId
                ? selectedRunIdsByBlueprint[activeBlueprintId] ?? (runState?.blueprint_id === activeBlueprintId ? runState.run_id : null)
                : selectedRunId ?? runState?.run_id ?? null}
              onOpen={(blueprintId, runId) => void openRunForObserve(blueprintId, runId)}
            />
          </aside>
        )}
      </div>

      {launchOpen && (blueprintPath || editSchedule) && (
        <div className="workflows-launch-overlay absolute inset-0 z-20 flex items-start justify-center overflow-hidden bg-[color-mix(in_srgb,var(--color-wardian-bg),transparent_25%)] p-8">
          <RunLaunchDialog
            path={editSchedulePath ?? blueprintPath ?? ''}
            blueprintId={editSchedule?.blueprint_id ?? activeBlueprintId ?? undefined}
            blueprint={editSchedule ? editScheduleBlueprint : blueprint}
            inputParams={editSchedule ? inputParamsFromBlueprint(editScheduleBlueprint) : inputParams}
            editSchedule={editSchedule ?? undefined}
            onLaunched={(runId) => void handleLaunched(runId)}
            onScheduled={() => {
              setLaunchOpen(false);
              setEditSchedule(null);
              setEditScheduleBlueprint(null);
              setEditSchedulePath(null);
              void loadSchedules();
            }}
            onCancel={() => {
              setLaunchOpen(false);
              setEditSchedule(null);
              setEditScheduleBlueprint(null);
              setEditSchedulePath(null);
            }}
          />
        </div>
      )}
    </div>
  );
}

interface WorkflowEditModeProps extends WorkflowsViewProps {
  addNodeRequest: number;
}

function WorkflowEditMode({ theme, addNodeRequest }: WorkflowEditModeProps) {
  const blueprint = useBuilderStore((state) => state.blueprint);
  const diagnostics = useBuilderStore((state) => state.diagnostics);
  const setBlueprint = useBuilderStore((state) => state.setBlueprint);
  const validate = useBuilderStore((state) => state.validate);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [contextMenu, setContextMenu] = useState<{
    type: 'node' | 'edge';
    targetId: string;
    x: number;
    y: number;
  } | null>(null);
  const lastAddNodeRequest = useRef(addNodeRequest);

  useEffect(() => {
    if (!blueprint) return;
    const timer = window.setTimeout(() => {
      void validate();
    }, 400);
    return () => window.clearTimeout(timer);
  }, [blueprint, validate]);

  useEffect(() => {
    if (lastAddNodeRequest.current === addNodeRequest) return;
    lastAddNodeRequest.current = addNodeRequest;
    setContextMenu(null);
    setLibraryOpen(true);
  }, [addNodeRequest]);

  const activeBlueprint = blueprint ?? INITIAL_BLUEPRINT;
  const selectedNode = useMemo(
    () => activeBlueprint.nodes.find((node) => node.id === selectedNodeId) ?? null,
    [activeBlueprint.nodes, selectedNodeId],
  );
  const selectedNodeDef = useMemo(
    () => selectedNode ? findNodeType(selectedNode.type) : undefined,
    [selectedNode],
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

  const duplicateNode = (id: string) => {
    const sourceNode = activeBlueprint.nodes.find((node) => node.id === id);
    if (!sourceNode) return;

    const nextId = nextNodeId(activeBlueprint.nodes, sourceNode.type);
    const clonedNode: BlueprintNode = {
      ...sourceNode,
      id: nextId,
      fields: { ...sourceNode.fields },
      ...(sourceNode.position
        ? { position: { x: sourceNode.position.x + 40, y: sourceNode.position.y + 40 } }
        : {}),
    };

    setBlueprint({ ...activeBlueprint, nodes: [...activeBlueprint.nodes, clonedNode] });
    setSelectedNodeId(nextId);
    setContextMenu(null);
  };

  const copyNodeId = (id: string) => {
    void navigator.clipboard?.writeText(id);
    setContextMenu(null);
  };

  const deleteNode = (id: string) => {
    setBlueprint({
      ...activeBlueprint,
      nodes: activeBlueprint.nodes.filter((node) => node.id !== id),
      edges: activeBlueprint.edges.filter((edge) => edge.from !== id && edge.to !== id),
    });
    if (selectedNodeId === id) {
      setSelectedNodeId(null);
    }
    setContextMenu(null);
  };

  const deleteEdge = (id: string) => {
    const match = /^e(\d+)$/.exec(id);
    const edgeIndex = match ? Number(match[1]) : -1;
    if (edgeIndex < 0) return;

    setBlueprint({
      ...activeBlueprint,
      edges: activeBlueprint.edges.filter((_, index) => index !== edgeIndex),
    });
    setContextMenu(null);
  };

  const contextMenuItems: ContextMenuItem[] = contextMenu?.type === 'node'
    ? [
        { label: 'Duplicate node', onClick: () => duplicateNode(contextMenu.targetId) },
        { label: 'Copy node ID', onClick: () => copyNodeId(contextMenu.targetId) },
        { divider: true },
        { label: 'Delete node', danger: true, onClick: () => deleteNode(contextMenu.targetId) },
      ]
    : [
        { label: 'Delete connection', danger: true, onClick: () => deleteEdge(contextMenu?.targetId ?? '') },
      ];

  return (
    <div data-testid="workflows-edit-mode" className="relative flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-bg)]">
      <div className="workflows-edit-body grid h-full min-h-0 flex-1" data-inspector-open={selectedNode ? 'true' : 'false'}>
        <section className="workflows-canvas relative min-h-0">
          <BuilderCanvas
            blueprint={activeBlueprint}
            diagnostics={diagnostics}
            selectedNodeId={selectedNodeId}
            onSelectNode={(id) => {
              setSelectedNodeId(id);
              if (id === null) {
                setContextMenu(null);
              }
            }}
            onRequestAddNode={() => {
              setContextMenu(null);
              setLibraryOpen(true);
            }}
            onNodeContextMenu={(targetId, x, y) => {
              setContextMenu({ type: 'node', targetId, x, y });
            }}
            onEdgeContextMenu={(targetId, x, y) => {
              setContextMenu({ type: 'edge', targetId, x, y });
            }}
            theme={theme}
          />
          <div
            data-testid="workflow-canvas-meta"
            className="pointer-events-none absolute right-3 top-3 rounded border border-wardian-border bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_10%)] px-2 py-1 text-[10px] font-mono text-muted shadow-sm"
          >
            {activeBlueprint.nodes.length} nodes / {activeBlueprint.edges.length} edges
          </div>
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
          <aside data-testid="workflow-inspector" className="workflow-node-inspector flex min-h-0 flex-col overflow-hidden border-l border-wardian-border bg-[var(--color-wardian-card)]">
            <div className="flex shrink-0 items-start justify-between gap-3 border-b border-wardian-border p-3">
              <div className="min-w-0">
                <div className="truncate text-sm font-bold text-[var(--color-wardian-text)]">
                  {selectedNode.name ?? selectedNodeDef?.label ?? selectedNode.id}
                </div>
                <div className="mt-1 flex min-w-0 items-center gap-2 text-[10px] text-muted">
                  <span className="shrink-0 font-bold text-[var(--color-wardian-text-muted)]">
                    {selectedNodeDef?.label ?? selectedNode.type}
                  </span>
                  <button
                    type="button"
                    className="min-w-0 truncate font-mono hover:text-[var(--color-wardian-accent)]"
                    title="Copy node ID"
                    onClick={() => void navigator.clipboard?.writeText(selectedNode.id)}
                  >
                    {selectedNode.id}
                  </button>
                </div>
              </div>
              <button
                type="button"
                className="inline-flex h-7 w-7 items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                onClick={() => setSelectedNodeId(null)}
                aria-label="Close node inspector"
                title="Close"
              >
                <X className="h-3.5 w-3.5" aria-hidden />
              </button>
            </div>
            <div className="grid min-h-0 gap-4 overflow-y-auto p-3">
              <NodeConfigForm node={selectedNode} onChange={updateNodeField} />
              <VariableAssistant blueprint={activeBlueprint} selectedNodeId={selectedNode.id} />
            </div>
          </aside>
        ) : null}
      </div>
      <DiagnosticsPanel diagnostics={diagnostics} onFocusNode={setSelectedNodeId} />
      {contextMenu ? (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          items={contextMenuItems}
          onClose={() => setContextMenu(null)}
        />
      ) : null}
      {libraryOpen ? (
        <div className="workflow-node-library-overlay absolute inset-0 z-30 flex items-start justify-center bg-[color-mix(in_srgb,var(--color-wardian-bg),transparent_20%)] p-8">
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

async function resolveScheduleBlueprint(schedule: WorkflowSchedule, currentBlueprint: Blueprint | null): Promise<{
  blueprint: Blueprint | null;
  path: string | null;
}> {
  if (currentBlueprint?.id === schedule.blueprint_id) {
    return { blueprint: currentBlueprint, path: null };
  }

  try {
    const refs = await invoke<Array<{ id: string; path: string }>>('workflow_list_blueprints');
    const ref = refs.find((candidate) => candidate.id === schedule.blueprint_id) ?? null;
    if (!ref) {
      return { blueprint: null, path: null };
    }
    const parsed = await invoke<{ blueprint: Blueprint; diagnostics: unknown[] }>('workflow_parse', { path: ref.path });
    return { blueprint: parsed.blueprint, path: ref.path };
  } catch {
    return { blueprint: null, path: null };
  }
}

function chooseRunForObserve(blueprintId: string, runs: RunSummary[], rememberedRunId?: string | null): RunSummary | null {
  const blueprintRuns = runs.filter((run) => run.blueprint_id === blueprintId);
  if (blueprintRuns.length === 0) return null;

  if (rememberedRunId) {
    const rememberedRun = blueprintRuns.find((run) => run.run_id === rememberedRunId);
    if (rememberedRun) return rememberedRun;
  }

  return [...blueprintRuns].sort(compareRunsForObserve)[0] ?? null;
}

function compareRunsForObserve(a: RunSummary, b: RunSummary): number {
  const statusDelta = observeStatusPriority(a.status) - observeStatusPriority(b.status);
  if (statusDelta !== 0) return statusDelta;

  const bTime = runSortTime(b);
  const aTime = runSortTime(a);
  const timeDelta = bTime.localeCompare(aTime);
  if (timeDelta !== 0) return timeDelta;

  return b.run_id.localeCompare(a.run_id);
}

function observeStatusPriority(status: RunSummary['status']): number {
  if (status === 'awaiting_approval') return 0;
  if (status === 'running') return 1;
  if (status === 'failed') return 2;
  return 3;
}

function runSortTime(run: RunSummary): string {
  return run.updated_at ?? run.completed_at ?? run.started_at ?? '';
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
