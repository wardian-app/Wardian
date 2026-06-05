import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => vi.fn()) }));

vi.mock('../features/workflows/BlueprintSelector', () => ({
  BlueprintSelector: ({ onOpen, onNew }: { onOpen: (path: string) => void; onNew: () => void }) => (
    <div data-testid="blueprint-selector">
      <button type="button" onClick={() => onOpen('<absolute-workspace-path>/library/workflows/wf.md')}>
        Open Workflow
      </button>
      <button type="button" onClick={() => onOpen('<absolute-workspace-path>/library/workflows/other.md')}>
        Open Other
      </button>
      <button type="button" onClick={onNew}>
        New Workflow
      </button>
    </div>
  ),
}));
vi.mock('../features/workflows/builder/BuilderCanvas', () => ({
  BuilderCanvas: ({
    onNodeContextMenu,
    onEdgeContextMenu,
    onRequestAddNode,
    onSelectNode,
  }: {
    onNodeContextMenu?: (nodeId: string, x: number, y: number) => void;
    onEdgeContextMenu?: (edgeId: string, x: number, y: number) => void;
    onRequestAddNode?: () => void;
    onSelectNode?: (nodeId: string | null) => void;
  }) => (
    <div data-testid="builder-canvas">
      <button type="button" onClick={() => onSelectNode?.('task-1')}>
        Select task
      </button>
      <button type="button" onContextMenu={(event) => { event.preventDefault(); onRequestAddNode?.(); }}>
        Canvas pane
      </button>
      <button type="button" onContextMenu={(event) => { event.preventDefault(); onNodeContextMenu?.('task-1', 12, 24); }}>
        Node context
      </button>
      <button type="button" onContextMenu={(event) => { event.preventDefault(); onEdgeContextMenu?.('e0', 12, 24); }}>
        Edge context
      </button>
    </div>
  ),
}));
vi.mock('../features/workflows/builder/BuilderToolbar', () => ({
  BuilderToolbar: () => <div data-testid="builder-toolbar" />,
}));
vi.mock('../features/workflows/builder/DiagnosticsPanel', () => ({
  DiagnosticsPanel: () => <div data-testid="diagnostics-panel" />,
}));
vi.mock('../features/workflows/builder/NodeConfigForm', () => ({
  NodeConfigForm: () => <div data-testid="node-config-form" />,
}));
vi.mock('../features/workflows/builder/NodePalette', () => ({
  NodePalette: () => <div data-testid="node-palette" />,
}));
vi.mock('../features/workflows/builder/NodeLibrary', () => ({
  NodeLibrary: ({ onAdd }: { onAdd: (def: unknown) => void }) => (
    <div data-testid="node-library">
      <input type="search" aria-label="Search nodes" />
      <button
        type="button"
        onClick={() => onAdd({
          id: 'manual_trigger',
          label: 'Manual Trigger',
          fields: [],
          inputs: [],
          outputs: [{ id: 'out', label: 'Out' }],
        })}
      >
        Manual Trigger
      </button>
    </div>
  ),
}));
vi.mock('../features/workflows/builder/VariableAssistant', () => ({
  VariableAssistant: () => <div data-testid="variable-assistant" />,
}));
vi.mock('../features/workflows/monitor/WorkflowMonitor', () => ({
  WorkflowMonitor: ({
    onEditSchedule,
    onOpenRun,
  }: {
    onEditSchedule: (schedule: unknown) => void;
    onOpenRun: (blueprintId: string, runId: string) => void;
  }) => (
    <div data-testid="workflow-monitor">
      <button type="button" onClick={() => onOpenRun('other', 'run-other-old')}>
        Open mocked run
      </button>
      <button
        type="button"
        onClick={() => onEditSchedule({
          id: 'schedule-1',
          blueprint_id: 'heartbeat',
          name: 'Passive Heartbeat',
          provider: null,
          workspace: null,
          input: {},
          bindings: {},
          assignments: {
            reasoning_gate: {
              target_type: 'agent',
              agent_id: 'agent-1',
              conversation: 'current',
              busy_policy: 'skip',
            },
          },
          schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
          is_paused: false,
        })}
      >
        Edit mocked schedule
      </button>
    </div>
  ),
}));
vi.mock('../features/workflows/run/EventTimeline', () => ({
  EventTimeline: ({ collapsed, onSelectNode }: { collapsed?: boolean; onSelectNode?: (nodeId: string) => void }) => (
    <div data-testid="event-timeline" data-collapsed={collapsed ? 'true' : 'false'}>
      <button type="button" onClick={() => onSelectNode?.('task-1')}>Timeline task event</button>
    </div>
  ),
}));
vi.mock('../features/workflows/run/NodeInspector', () => ({
  NodeInspector: ({ selectedNodeId }: { selectedNodeId: string | null }) => <div data-testid="node-inspector">{selectedNodeId}</div>,
}));
vi.mock('../features/workflows/run/RunDag', () => ({
  RunDag: ({ onSelectNode }: { onSelectNode: (nodeId: string) => void }) => (
    <div data-testid="run-dag">
      <button type="button" onClick={() => onSelectNode('task-1')}>Graph task</button>
    </div>
  ),
}));
vi.mock('../features/workflows/run/RunList', () => ({
  RunList: ({
    runs,
    selectedRunId,
    onOpen,
  }: {
    runs: Array<{ blueprint_id: string; run_id: string }>;
    selectedRunId: string | null;
    onOpen: (blueprintId: string, runId: string) => void;
  }) => (
    <div data-testid="run-list" data-selected-run-id={selectedRunId ?? ''}>
      {runs.map((run) => (
        <button
          key={`${run.blueprint_id}:${run.run_id}`}
          type="button"
          onClick={() => onOpen(run.blueprint_id, run.run_id)}
        >
          Open {run.blueprint_id} {run.run_id}
        </button>
      ))}
    </div>
  ),
}));

import { useBuilderStore } from '../store/useBuilderStore';
import { useRunStore } from '../features/workflows/run/useRunStore';
import { useSchedulesStore } from '../store/useSchedulesStore';
import { useWorkflowsView } from '../store/useWorkflowsView';
import { WorkflowsView } from './WorkflowsView';
import type { Blueprint } from '../features/workflows/builder/blueprintTypes';

describe('WorkflowsView', () => {
  beforeEach(() => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'workflow_list_runs') return [];
      if (command === 'workflow_read_run') {
        return {
          state: useRunStore.getState().state,
          events: useRunStore.getState().events,
          blueprint: useRunStore.getState().blueprint,
        };
      }
      if (command === 'schedule_list') return [];
      if (command === 'workflow_validate') return { ok: true, diagnostics: [] };
      return null;
    });
    useBuilderStore.getState().reset();
    useRunStore.getState().reset();
    useSchedulesStore.setState({ schedules: [], loading: false, error: null });
    useWorkflowsView.getState().reset();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('renders a loaded manual-trigger blueprint when the trigger omits empty fields', async () => {
    const blueprintWithOmittedFields = {
      schema: 2,
      id: 'auto-fix-audit',
      name: 'Auto-Fix Audit',
      nodes: [
        {
          id: 'trigger-1',
          type: 'manual_trigger',
          position: { x: 0, y: 0 },
        },
      ],
      edges: [],
    } as unknown as Blueprint;

    useBuilderStore.setState({
      blueprint: blueprintWithOmittedFields,
      path: '<absolute-workspace-path>/library/workflows/auto-fix-audit.md',
      diagnostics: [],
      dirty: false,
    });
    useWorkflowsView.setState({
      mode: 'edit',
      blueprintPath: '<absolute-workspace-path>/library/workflows/auto-fix-audit.md',
      selectedRunId: null,
    });

    render(<WorkflowsView theme="dark" />);

    expect(screen.getByTestId('workflows-view')).toBeInTheDocument();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_list_runs'));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('schedule_list'));
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 450));
    });
  });

  it('keeps runs hidden by default in edit mode and opens them on demand', async () => {
    seedBuilderWithEmptyBlueprint();
    render(<WorkflowsView theme="dark" />);

    expect(screen.queryByRole('heading', { name: 'Runs' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /show runs/i }));
    expect(screen.getByRole('heading', { name: 'Runs' })).toBeVisible();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_list_runs'));
  });

  it('opens the registry library from the edit toolbar', async () => {
    seedBuilderWithEmptyBlueprint();
    render(<WorkflowsView theme="dark" />);

    fireEvent.click(screen.getByRole('button', { name: /add node/i }));

    expect(screen.getByTestId('node-library')).toBeVisible();
    expect(screen.getByRole('searchbox', { name: /search nodes/i })).toBeVisible();
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_list_runs'));
  });

  it('collapses edit controls into one command bar and moves graph counts onto the canvas', async () => {
    seedBuilderWithEmptyBlueprint();
    render(<WorkflowsView theme="dark" />);

    expect(screen.queryByTestId('builder-toolbar')).toBeNull();
    expect(screen.getByTestId('workflow-canvas-meta')).toHaveTextContent('0 nodes / 0 edges');
    expect(screen.getByLabelText(/workflow name/i)).toHaveValue('Workflow');
    expect(screen.getByRole('button', { name: /saved/i })).toBeDisabled();
  });

  it('opens the registry library from an edit canvas right click', async () => {
    seedBuilderWithEmptyBlueprint();
    render(<WorkflowsView theme="dark" />);

    fireEvent.contextMenu(screen.getByRole('button', { name: /canvas pane/i }));

    expect(screen.getByTestId('node-library')).toBeVisible();
  });

  it('duplicates and deletes nodes from the edit canvas context menu', async () => {
    seedBuilderWithConnectedBlueprint();
    render(<WorkflowsView theme="dark" />);

    fireEvent.contextMenu(screen.getByRole('button', { name: /node context/i }));
    fireEvent.click(screen.getByRole('button', { name: /duplicate node/i }));

    expect(useBuilderStore.getState().blueprint?.nodes.map((node) => node.id)).toContain('task-3');

    fireEvent.contextMenu(screen.getByRole('button', { name: /node context/i }));
    fireEvent.click(screen.getByRole('button', { name: /delete node/i }));

    expect(useBuilderStore.getState().blueprint?.nodes.map((node) => node.id)).not.toContain('task-1');
    expect(useBuilderStore.getState().blueprint?.edges).toEqual([]);
  });

  it('copies node ids from the edit canvas context menu', async () => {
    const writeText = vi.fn();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    seedBuilderWithConnectedBlueprint();
    render(<WorkflowsView theme="dark" />);

    fireEvent.contextMenu(screen.getByRole('button', { name: /node context/i }));
    fireEvent.click(screen.getByRole('button', { name: /copy node id/i }));

    expect(writeText).toHaveBeenCalledWith('task-1');
  });

  it('deletes edges from the edit canvas context menu', async () => {
    seedBuilderWithConnectedBlueprint();
    render(<WorkflowsView theme="dark" />);

    fireEvent.contextMenu(screen.getByRole('button', { name: /edge context/i }));
    fireEvent.click(screen.getByRole('button', { name: /delete connection/i }));

    expect(useBuilderStore.getState().blueprint?.edges).toEqual([]);
  });

  it('shows selected workflow nodes as the inspector header focus', async () => {
    seedBuilderWithConnectedBlueprint();
    render(<WorkflowsView theme="dark" />);

    fireEvent.click(screen.getByRole('button', { name: /select task/i }));

    const inspector = screen.getByTestId('workflow-inspector');
    expect(inspector).toHaveTextContent('First task');
    expect(inspector).toHaveTextContent('Task');
    expect(inspector).toHaveTextContent('task-1');
    expect(screen.queryByText(/^Inspector$/)).toBeNull();
  });

  it('keeps the global runs drawer closed by default in observe mode', async () => {
    seedObserveRun();
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByTestId('run-dag')).toBeInTheDocument());
    expect(screen.queryByRole('heading', { name: 'Runs' })).toBeNull();
  });

  it('opens the latest run when a blueprint is selected from observe mode', async () => {
    useBuilderStore.setState({
      blueprint: null,
      path: null,
      diagnostics: [],
      dirty: false,
    });
    useRunStore.setState({
      runs: [
        {
          run_id: 'run-old',
          blueprint_id: 'wf',
          status: 'completed',
          node_count: 1,
          path: '<absolute-workspace-path>/library/runs/run-old.json',
          updated_at: '2026-05-31T12:00:00Z',
        },
        {
          run_id: 'run-new',
          blueprint_id: 'wf',
          status: 'completed',
          node_count: 1,
          path: '<absolute-workspace-path>/library/runs/run-new.json',
          updated_at: '2026-05-31T13:00:00Z',
        },
      ],
      state: null,
      events: [],
      blueprint: null,
      scrubIndex: 0,
    });
    useWorkflowsView.setState({
      mode: 'observe',
      blueprintPath: null,
      selectedRunId: null,
      selectedRunIdsByBlueprint: {},
    });
    invokeMock.mockImplementation(async (command: string, args?: { path?: string; runId?: string }) => {
      if (command === 'workflow_list_runs') return useRunStore.getState().runs;
      if (command === 'workflow_parse') return { blueprint: workflowBlueprint(), diagnostics: [] };
      if (command === 'workflow_read_run') return readRunResult(args?.runId ?? 'run-new');
      if (command === 'schedule_list') return [];
      if (command === 'workflow_validate') return { ok: true, diagnostics: [] };
      return null;
    });

    render(<WorkflowsView theme="dark" />);
    fireEvent.click(screen.getByRole('button', { name: /open workflow/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_read_run', { blueprintId: 'wf', runId: 'run-new' }));
    expect(useWorkflowsView.getState().mode).toBe('observe');
    expect(useWorkflowsView.getState().selectedRunIdsByBlueprint.wf).toBe('run-new');
  });

  it('restores the remembered observe run when selecting a blueprint again', async () => {
    useBuilderStore.setState({
      blueprint: null,
      path: null,
      diagnostics: [],
      dirty: false,
    });
    useRunStore.setState({
      runs: [
        {
          run_id: 'run-old',
          blueprint_id: 'wf',
          status: 'completed',
          node_count: 1,
          path: '<absolute-workspace-path>/library/runs/run-old.json',
          updated_at: '2026-05-31T12:00:00Z',
        },
        {
          run_id: 'run-new',
          blueprint_id: 'wf',
          status: 'completed',
          node_count: 1,
          path: '<absolute-workspace-path>/library/runs/run-new.json',
          updated_at: '2026-05-31T13:00:00Z',
        },
      ],
      state: null,
      events: [],
      blueprint: null,
      scrubIndex: 0,
    });
    useWorkflowsView.setState({
      mode: 'observe',
      blueprintPath: null,
      selectedRunId: null,
      selectedRunIdsByBlueprint: { wf: 'run-old' },
    });
    invokeMock.mockImplementation(async (command: string, args?: { path?: string; runId?: string }) => {
      if (command === 'workflow_list_runs') return useRunStore.getState().runs;
      if (command === 'workflow_parse') return { blueprint: workflowBlueprint(), diagnostics: [] };
      if (command === 'workflow_read_run') return readRunResult(args?.runId ?? 'run-old');
      if (command === 'schedule_list') return [];
      if (command === 'workflow_validate') return { ok: true, diagnostics: [] };
      return null;
    });

    render(<WorkflowsView theme="dark" />);
    fireEvent.click(screen.getByRole('button', { name: /open workflow/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_read_run', { blueprintId: 'wf', runId: 'run-old' }));
    expect(useWorkflowsView.getState().selectedRunId).toBe('run-old');
  });

  it('opens observe details only after a graph node is selected', async () => {
    seedObserveRun();
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByTestId('run-dag')).toBeInTheDocument());
    expect(screen.queryByTestId('node-inspector')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Graph task' }));

    expect(screen.getByTestId('node-inspector')).toHaveTextContent('task-1');
  });

  it('selects observe details from timeline events', async () => {
    seedObserveRun();
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByTestId('event-timeline')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Timeline task event' }));

    expect(screen.getByTestId('node-inspector')).toHaveTextContent('task-1');
  });

  it('loads the scheduled blueprint before opening a Monitor schedule edit dialog', async () => {
    useBuilderStore.setState({
      blueprint: {
        schema: 2,
        id: 'other',
        name: 'Other Workflow',
        nodes: [],
        edges: [],
      },
      path: '<absolute-workspace-path>/library/workflows/other.md',
      diagnostics: [],
      dirty: false,
    });
    useWorkflowsView.setState({
      mode: 'monitor',
      blueprintPath: '<absolute-workspace-path>/library/workflows/other.md',
      selectedRunId: null,
    });
    invokeMock.mockImplementation(async (command: string, args?: { path?: string }) => {
      if (command === 'workflow_list_runs') return [];
      if (command === 'schedule_list') return [];
      if (command === 'workflow_list_blueprints') {
        return [
          { id: 'heartbeat', name: 'Heartbeat', path: '<absolute-workspace-path>/library/workflows/heartbeat.md' },
        ];
      }
      if (command === 'workflow_parse' && args?.path?.endsWith('heartbeat.md')) {
        return {
          blueprint: {
            schema: 2,
            id: 'heartbeat',
            name: 'Heartbeat',
            nodes: [
              { id: 'task-1', type: 'task', fields: { agent: 'role:reasoning_gate', prompt: 'Check in.' } },
            ],
            edges: [],
          },
          diagnostics: [],
        };
      }
      if (command === 'workflow_validate') return { ok: true, diagnostics: [] };
      if (command === 'list_provider_readiness') return [];
      if (command === 'list_agents') {
        return [{
          session_id: 'agent-1',
          session_name: 'Assistant',
          agent_class: 'Personal Assistant',
          folder: '/assistant',
          is_off: false,
          provider: 'gemini',
        }];
      }
      return null;
    });

    render(<WorkflowsView theme="dark" />);
    fireEvent.click(screen.getByRole('button', { name: /edit mocked schedule/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_list_blueprints'));
    await waitFor(() => expect(screen.getByRole('button', { name: /change reasoning_gate assignment/i })).toBeInTheDocument());
    expect(screen.queryByLabelText(/^provider$/i)).toBeNull();
  });

  it('locks the requested Monitor run before the run read resolves', async () => {
    let resolveRead: (value: ReturnType<typeof readRunResult>) => void = () => undefined;
    useRunStore.setState({
      runs: [
        {
          run_id: 'run-wf-new',
          blueprint_id: 'wf',
          status: 'completed',
          node_count: 1,
          path: '<absolute-workspace-path>/library/runs/run-wf-new.json',
          updated_at: '2026-05-31T13:00:00Z',
        },
        {
          run_id: 'run-other-old',
          blueprint_id: 'other',
          status: 'completed',
          node_count: 1,
          path: '<absolute-workspace-path>/library/runs/run-other-old.json',
          updated_at: '2026-05-31T12:00:00Z',
        },
      ],
      state: {
        run_id: 'run-wf-new',
        blueprint_id: 'wf',
        status: 'completed',
        nodes: { 'task-1': 'completed' },
      },
      events: [],
      blueprint: workflowBlueprint('wf'),
      scrubIndex: 0,
    });
    useWorkflowsView.setState({
      mode: 'monitor',
      blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
      selectedRunId: 'run-wf-new',
      observedBlueprintId: 'wf',
      selectedRunIdsByBlueprint: { wf: 'run-wf-new' },
    });
    invokeMock.mockImplementation(async (command: string, args?: { blueprintId?: string; runId?: string }) => {
      if (command === 'workflow_list_runs') return useRunStore.getState().runs;
      if (command === 'workflow_read_run') {
        return new Promise((resolve) => {
          resolveRead = resolve;
          void args;
        });
      }
      if (command === 'schedule_list') return [];
      if (command === 'workflow_validate') return { ok: true, diagnostics: [] };
      return null;
    });

    render(<WorkflowsView theme="dark" />);
    fireEvent.click(screen.getByRole('button', { name: /open mocked run/i }));

    await waitFor(() => expect(useWorkflowsView.getState().mode).toBe('observe'));
    expect(useWorkflowsView.getState().observedBlueprintId).toBe('other');
    expect(useWorkflowsView.getState().selectedRunId).toBe('run-other-old');
    expect(useWorkflowsView.getState().selectedRunIdsByBlueprint.other).toBe('run-other-old');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'workflow_read_run')).toHaveLength(1);

    await act(async () => {
      resolveRead(readRunResult('run-other-old', 'other'));
    });
  });

  it('surfaces observe failure state in the run header', async () => {
    seedObserveRun({
      status: 'failed',
      failure: 'Task crashed',
      nodes: { 'task-1': 'failed' },
    });
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByText('Failed')).toBeInTheDocument());
    expect(screen.getByText('Task crashed')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /save/i })).toBeNull();
  });

  it('uses the blueprint file path for observe approval controls', async () => {
    seedObserveRun({
      status: 'awaiting_approval',
      nodes: { 'task-1': 'running' },
    });
    useRunStore.setState({
      blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
      events: [
        { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
        { seq: 1, ts: 't1', kind: 'awaiting_approval', node: 'task-1' },
      ],
    });
    render(<WorkflowsView theme="dark" />);

    fireEvent.click(screen.getByRole('button', { name: /approve/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        'workflow_approve',
        expect.objectContaining({
          blueprintId: 'wf',
          runId: 'run-1',
          blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
          node: 'task-1',
          granted: true,
        }),
      );
    });
    expect(invokeMock).not.toHaveBeenCalledWith(
      'workflow_approve',
      expect.objectContaining({ blueprintPath: '<absolute-workspace-path>/library/runs/run-1.json' }),
    );
  });

  it('clears observe details when a different run opens', async () => {
    seedObserveRun();
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByTestId('run-dag')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Graph task' }));
    expect(screen.getByTestId('node-inspector')).toHaveTextContent('task-1');

    await act(async () => {
      useRunStore.setState({
        state: {
          run_id: 'run-2',
          blueprint_id: 'wf',
          status: 'running',
          nodes: { 'task-2': 'running' },
        },
        events: [
          { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
          { seq: 1, ts: 't1', kind: 'node_started', node: 'task-2' },
        ],
        blueprint: {
          schema: 2,
          id: 'wf',
          name: 'Workflow',
          nodes: [{ id: 'task-2', type: 'task', name: 'Task' }],
          edges: [],
        },
        scrubIndex: 1,
      });
    });

    await waitFor(() => expect(screen.queryByTestId('node-inspector')).toBeNull());
  });
});

function seedBuilderWithEmptyBlueprint() {
  useBuilderStore.setState({
    blueprint: { schema: 2, id: 'wf', name: 'Workflow', nodes: [], edges: [] },
    path: '<absolute-workspace-path>/library/workflows/wf.md',
    diagnostics: [],
    dirty: false,
  });
  useWorkflowsView.setState({
    mode: 'edit',
    blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
    selectedRunId: null,
  });
}

function seedBuilderWithConnectedBlueprint() {
  useBuilderStore.setState({
    blueprint: {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'task-1', type: 'task', name: 'First task', fields: {}, position: { x: 10, y: 20 } },
        { id: 'task-2', type: 'task', name: 'Second task', fields: {}, position: { x: 220, y: 20 } },
      ],
      edges: [{ from: 'task-1', to: 'task-2', from_port: 'out', to_port: 'in' }],
    },
    path: '<absolute-workspace-path>/library/workflows/wf.md',
    diagnostics: [],
    dirty: false,
  });
  useWorkflowsView.setState({
    mode: 'edit',
    blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
    selectedRunId: null,
  });
}

function seedObserveRun(stateOverrides: Partial<ReturnType<typeof useRunStore.getState>['state']> = {}) {
  const state = {
    run_id: 'run-1',
    blueprint_id: 'wf',
    status: 'running' as const,
    nodes: { 'task-1': 'running' as const },
    ...stateOverrides,
  };
  useRunStore.setState({
    runs: [{
      run_id: 'run-1',
      blueprint_id: 'wf',
      status: state.status,
      node_count: 1,
      failure: state.failure,
      path: '<absolute-workspace-path>/library/runs/run-1.json',
      blueprint_path: '<absolute-workspace-path>/library/workflows/wf.md',
    }],
    state,
    events: [
      { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: 'wf', schema: 2, trigger: {} },
      { seq: 1, ts: 't1', kind: 'node_started', node: 'task-1' },
    ],
    blueprint: {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [{ id: 'task-1', type: 'task', name: 'Task' }],
      edges: [],
    },
    blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
    scrubIndex: 1,
  });
  useWorkflowsView.setState({
    mode: 'observe',
    blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
    selectedRunId: 'run-1',
    selectedRunIdsByBlueprint: { wf: 'run-1' },
  });
}

function workflowBlueprint(id = 'wf'): Blueprint {
  return {
    schema: 2,
    id,
    name: id === 'wf' ? 'Workflow' : 'Other Workflow',
    nodes: [{ id: 'task-1', type: 'task', name: 'Task' }],
    edges: [],
  };
}

function readRunResult(runId: string, blueprintId = 'wf') {
  return {
    state: {
      run_id: runId,
      blueprint_id: blueprintId,
      status: 'completed',
      nodes: { 'task-1': 'completed' },
    },
    events: [
      { seq: 0, ts: 't0', kind: 'run_started', blueprint_id: blueprintId, schema: 2, trigger: {} },
      { seq: 1, ts: 't1', kind: 'run_completed' },
    ],
    blueprint: workflowBlueprint(blueprintId),
    blueprint_path: `<absolute-workspace-path>/library/workflows/${blueprintId}.md`,
  };
}
