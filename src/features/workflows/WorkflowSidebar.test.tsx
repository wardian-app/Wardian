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
        <button key={wf.id} onClick={() => onRun(wf.id)}>{wf.name}</button>
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

  it('creates a new scheduled run from the sidebar instead of overwriting the workflow definition', async () => {
    renderWithProvider(<WorkflowSidebar />);

    fireEvent.click(screen.getByText('Alpha Workflow'));

    await waitFor(() => expect(mockCreateScheduledRun).toHaveBeenCalledTimes(1));
    expect(mockSaveWorkflow).not.toHaveBeenCalled();
    expect(mockRunWorkflowById).not.toHaveBeenCalled();
  });

  it('correctly identifies active workflows for monitoring', () => {
    renderWithProvider(<WorkflowSidebar />);
    expect(screen.getByTestId('active-monitoring')).toBeInTheDocument();
  });
});
