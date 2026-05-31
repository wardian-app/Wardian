import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => vi.fn()) }));

vi.mock('../features/workflows/BlueprintSelector', () => ({
  BlueprintSelector: () => <div data-testid="blueprint-selector" />,
}));
vi.mock('../features/workflows/builder/BuilderCanvas', () => ({
  BuilderCanvas: () => <div data-testid="builder-canvas" />,
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
vi.mock('../features/workflows/builder/VariableAssistantV2', () => ({
  VariableAssistantV2: () => <div data-testid="variable-assistant" />,
}));
vi.mock('../features/workflows/monitor/WorkflowMonitor', () => ({
  WorkflowMonitor: () => <div data-testid="workflow-monitor" />,
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
  RunList: () => <div data-testid="run-list" />,
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
      if (command === 'schedule_list_v2') return [];
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
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('schedule_list_v2'));
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

  it('keeps the global runs drawer closed by default in observe mode', async () => {
    seedObserveRun();
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByTestId('run-dag')).toBeInTheDocument());
    expect(screen.queryByRole('heading', { name: 'Runs' })).toBeNull();
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

  it('surfaces observe failure state in the run header', async () => {
    seedObserveRun({
      status: 'failed',
      failure: 'Task crashed',
      nodes: { 'task-1': 'failed' },
    });
    render(<WorkflowsView theme="dark" />);

    await waitFor(() => expect(screen.getByText('failed')).toBeInTheDocument());
    expect(screen.getByText('Task crashed')).toBeInTheDocument();
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
    scrubIndex: 1,
  });
  useWorkflowsView.setState({
    mode: 'observe',
    blueprintPath: '<absolute-workspace-path>/library/workflows/wf.md',
    selectedRunId: 'run-1',
  });
}
