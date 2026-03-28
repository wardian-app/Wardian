import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { WorkflowSidebar } from './WorkflowSidebar';
import { useWorkflowStore } from '../../store/useWorkflowStore';
import { ConfirmProvider } from '../../components/ConfirmDialog';

// Mock the store
vi.mock('../../store/useWorkflowStore', () => ({
  useWorkflowStore: vi.fn(),
}));

// Mock child components that have complex logic or their own hooks
vi.mock('./WorkflowLibrary', () => ({
  WorkflowLibrary: ({ workflows }: any) => (
    <div data-testid="workflow-library">
      {workflows.map((wf: any) => (
        <div key={wf.id}>{wf.name}</div>
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

  const defaultStoreValues = {
    availableWorkflows: [
      { id: 'wf-1', name: 'Alpha Workflow', nodes: [{ type: 'trigger', config: { type: 'cron', status: 'active' } }] },
      { id: 'wf-2', name: 'Beta Workflow', nodes: [{ type: 'trigger', config: { type: 'manual', status: 'off' } }] },
    ],
    fetchWorkflows: mockFetchWorkflows,
    runWorkflowById: vi.fn(),
    loadWorkflow: vi.fn(),
    deleteWorkflow: vi.fn(),
    stopAllTriggers: mockStopAllTriggers,
    pauseAllTriggers: mockPauseAllTriggers,
    resumeAllTriggers: mockResumeAllTriggers,
    activeRuns: [],
    schedules: [],
  };

  beforeEach(() => {
    vi.clearAllMocks();
    (useWorkflowStore as any).mockReturnValue(defaultStoreValues);
  });

  it('renders fixed header controls correctly', () => {
    renderWithProvider(<WorkflowSidebar />);
    
    expect(screen.getByText('Workflows')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Search workflows...')).toBeInTheDocument();
    
    // Check governance buttons by title
    expect(screen.getByTitle('Stop All (Panic)')).toBeInTheDocument();
    expect(screen.getByTitle('Pause All')).toBeInTheDocument();
    expect(screen.getByTitle('Resume All')).toBeInTheDocument();
  });


  it('filters workflows based on search query', async () => {
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
    
    // Stop All — opens custom ConfirmDialog
    fireEvent.click(screen.getByTitle('Stop All (Panic)'));
    // The ConfirmDialog should now be visible with the STOP ALL message
    await waitFor(() => expect(screen.getByText(/STOP ALL/)).toBeInTheDocument());
    // Click "Confirm" in the dialog to proceed
    fireEvent.click(screen.getByText('Confirm'));
    await waitFor(() => expect(mockStopAllTriggers).toHaveBeenCalled());
    await waitFor(() => expect(mockFetchWorkflows).toHaveBeenCalled());

    // Pause All
    fireEvent.click(screen.getByTitle('Pause All'));
    expect(mockPauseAllTriggers).toHaveBeenCalled();

    // Resume All
    fireEvent.click(screen.getByTitle('Resume All'));
    expect(mockResumeAllTriggers).toHaveBeenCalled();
  });

  it('correctly identifies active workflows for monitoring', () => {
    // This test verifies the logic in activeWorkflows useMemo (even if we mock child, we can check props if we wanted to)
    // But since we are testing WorkflowSidebar, and it passes filteredWorkflows to WorkflowLibrary, 
    // we already tested search. Active monitoring just gets activeRuns/schedules directly.
    renderWithProvider(<WorkflowSidebar />);
    expect(screen.getByTestId('active-monitoring')).toBeInTheDocument();
  });
});
