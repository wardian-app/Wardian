import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { WorkflowSidebar } from './WorkflowSidebar';
import { useWorkflowStore } from '../../store/useWorkflowStore';
import { ConfirmProvider } from '../../components/ConfirmDialog';

vi.mock('../../store/useWorkflowStore', () => ({
  useWorkflowStore: vi.fn(),
}));

vi.mock('./WorkflowLibrary', () => ({
  WorkflowLibrary: ({ workflows, onRun }: any) => (
    <div data-testid="workflow-library">
      {workflows.map((wf: any) => (
        <button
          key={wf.id}
          data-workflow-id={wf.id}
          data-trigger-status={wf.trigger_status}
          data-trigger-type={wf.trigger_type}
          onClick={() => onRun(wf.id)}
        >
          {wf.name}
        </button>
      ))}
    </div>
  ),
}));

vi.mock('./ActiveMonitoring', () => ({
  ActiveMonitoring: () => <div data-testid="active-monitoring">Monitoring Area</div>,
}));

const renderWithProvider = (ui: React.ReactElement) =>
  render(<ConfirmProvider>{ui}</ConfirmProvider>);

describe('WorkflowSidebar', () => {
  const mockFetchWorkflows = vi.fn();
  const mockStopAllTriggers = vi.fn();
  const mockPauseAllTriggers = vi.fn();
  const mockResumeAllTriggers = vi.fn();
  const mockRunWorkflowById = vi.fn();
  const mockSaveWorkflow = vi.fn();
  const mockCreateScheduledRun = vi.fn();

  const defaultStoreValues = {
    availableWorkflows: [
      {
        id: 'wf-1',
        name: 'Alpha Workflow',
        settings: { max_iterations: 10, on_limit_reached: 'pause' },
        role_mappings: {},
        nodes: [{ id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { schedule_type: 'Hours', interval: '2', status: 'active' } }],
      },
      {
        id: 'wf-2',
        name: 'Beta Workflow',
        settings: { max_iterations: 10, on_limit_reached: 'pause' },
        role_mappings: {},
        nodes: [{ id: 'trigger-2', type: 'trigger', name: 'Manual Trigger', config: { type: 'manual', status: 'off' } }],
      },
    ],
    fetchWorkflows: mockFetchWorkflows,
    runWorkflowById: mockRunWorkflowById,
    loadWorkflow: vi.fn(),
    saveWorkflow: mockSaveWorkflow,
    deleteWorkflow: vi.fn(),
    stopAllTriggers: mockStopAllTriggers,
    stopWorkflowTriggers: vi.fn(),
    stopWorkflowRun: vi.fn(),
    pauseAllTriggers: mockPauseAllTriggers,
    resumeAllTriggers: mockResumeAllTriggers,
    agents: [],
    activeRuns: [],
    scheduledRuns: [],
    loadScheduledRuns: vi.fn(),
    createScheduledRun: mockCreateScheduledRun,
    toggleScheduledRun: vi.fn(),
    deleteScheduledRun: vi.fn(),
    runScheduledWorkflowNow: vi.fn(),
  };

  beforeEach(() => {
    vi.clearAllMocks();
    (useWorkflowStore as any).mockReturnValue(defaultStoreValues);
  });

  it('renders fixed header controls correctly', () => {
    renderWithProvider(<WorkflowSidebar />);

    expect(screen.getByText('Workflows')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Search workflows...')).toBeInTheDocument();
    expect(screen.getByTitle('Stop All (Panic)')).toBeInTheDocument();
    expect(screen.getByTitle('Pause All')).toBeInTheDocument();
    expect(screen.getByTitle('Resume All')).toBeInTheDocument();
  });

  it('filters workflows based on search query', () => {
    renderWithProvider(<WorkflowSidebar />);

    const searchInput = screen.getByPlaceholderText('Search workflows...');

    expect(screen.getByText('Alpha Workflow')).toBeInTheDocument();
    expect(screen.getByText('Beta Workflow')).toBeInTheDocument();

    fireEvent.change(searchInput, { target: { value: 'Alpha' } });

    expect(screen.getByText('Alpha Workflow')).toBeInTheDocument();
    expect(screen.queryByText('Beta Workflow')).not.toBeInTheDocument();
  });

  it('triggers governance actions correctly', async () => {
    renderWithProvider(<WorkflowSidebar />);

    fireEvent.click(screen.getByTitle('Stop All (Panic)'));
    await waitFor(() => expect(screen.getByText(/STOP ALL/)).toBeInTheDocument());
    fireEvent.click(screen.getByText('Confirm'));
    await waitFor(() => expect(mockStopAllTriggers).toHaveBeenCalled());
    await waitFor(() => expect(mockFetchWorkflows).toHaveBeenCalled());

    fireEvent.click(screen.getByTitle('Pause All'));
    expect(mockPauseAllTriggers).toHaveBeenCalled();

    fireEvent.click(screen.getByTitle('Resume All'));
    expect(mockResumeAllTriggers).toHaveBeenCalled();
  });

  it('delegates configurable workflow runs to the main view instead of opening the modal in the sidebar', async () => {
    const mockOpenRunModalInMain = vi.fn();
    renderWithProvider(<WorkflowSidebar onOpenRunModalInMain={mockOpenRunModalInMain} />);

    fireEvent.click(screen.getByText('Alpha Workflow'));

    await waitFor(() => expect(mockOpenRunModalInMain).toHaveBeenCalledTimes(1));
    expect(mockOpenRunModalInMain).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'wf-1' }),
      'scheduled',
    );
    expect(screen.queryByRole('button', { name: /Schedule Workflow/i })).not.toBeInTheDocument();
    expect(mockCreateScheduledRun).not.toHaveBeenCalled();
    expect(mockRunWorkflowById).not.toHaveBeenCalled();
  });

  it('correctly identifies active workflows for monitoring', () => {
    renderWithProvider(<WorkflowSidebar />);
    expect(screen.getByTestId('active-monitoring')).toBeInTheDocument();
  });


  it('derives scheduled workflow status from scheduled runs instead of trigger node config', () => {
    (useWorkflowStore as any).mockReturnValue({
      ...defaultStoreValues,
      availableWorkflows: [
        {
          id: 'wf-1',
          name: 'Alpha Workflow',
          settings: { max_iterations: 10, on_limit_reached: 'pause' },
          role_mappings: {},
          nodes: [{ id: 'trigger-1', type: 'trigger', name: 'Scheduled Trigger', config: { schedule_type: 'Hours', interval: '2', status: 'off' } }],
        },
      ],
      scheduledRuns: [
        {
          id: 'schedule-1',
          workflow_id: 'wf-1',
          workflow_name: 'Alpha Workflow',
          schedule: { schedule_type: 'hours', value: '2', active: true },
          role_mappings: {},
          next_run_epoch_ms: Date.now() + 1000,
          is_paused: false,
        },
      ],
    });

    renderWithProvider(<WorkflowSidebar />);

    expect(screen.getByRole('button', { name: 'Alpha Workflow' })).toHaveAttribute('data-trigger-status', 'active');
  });

  it('marks a workflow active in the library while it has an in-flight run', () => {
    (useWorkflowStore as any).mockReturnValue({
      ...defaultStoreValues,
      availableWorkflows: [
        {
          id: 'wf-2',
          name: 'Beta Workflow',
          settings: { max_iterations: 10, on_limit_reached: 'pause' },
          role_mappings: {},
          nodes: [{ id: 'trigger-2', type: 'trigger', name: 'Manual Trigger', config: { type: 'manual', status: 'off' } }],
        },
      ],
      activeRuns: [
        {
          workflow_id: 'wf-2',
          node_id: 'agent-1',
          node_name: 'Agent',
          status: 'running',
          output: null,
        },
      ],
    });

    renderWithProvider(<WorkflowSidebar />);

    expect(screen.getByRole('button', { name: 'Beta Workflow' })).toHaveAttribute('data-trigger-status', 'active');
  });
});
