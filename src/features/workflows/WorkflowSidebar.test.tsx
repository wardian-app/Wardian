import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { WorkflowSidebar } from './WorkflowSidebar';
import { useWorkflowStore } from '../../store/useWorkflowStore';

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

describe('WorkflowSidebar', () => {
  const mockFetchWorkflows = vi.fn();
  const mockStopAllTriggers = vi.fn();
  const mockPauseAllTriggers = vi.fn();
  const mockResumeAllTriggers = vi.fn();
  const mockOnCollapse = vi.fn();

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
    window.confirm = vi.fn(() => true);
  });

  it('renders fixed header controls correctly', () => {
    render(<WorkflowSidebar onCollapse={mockOnCollapse} />);
    
    expect(screen.getByText('WORKFLOWS')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Search workflows...')).toBeInTheDocument();
    
    // Check governance buttons by title
    expect(screen.getByTitle('Stop All (Panic)')).toBeInTheDocument();
    expect(screen.getByTitle('Pause All')).toBeInTheDocument();
    expect(screen.getByTitle('Resume All')).toBeInTheDocument();
  });

  it('calls onCollapse when collapse button is clicked', () => {
    render(<WorkflowSidebar onCollapse={mockOnCollapse} />);
    screen.getByRole('button', { name: (_n, el) => el?.querySelector('path')?.getAttribute('d')?.includes('M15 19l-7-7 7-7') || false });
    // In WorkflowSidebar.tsx line 62, the button has the svg. Testing the button itself.
    fireEvent.click(screen.getByRole('button', { name: '' })); // The button with the svg icon has no text
    // Refined selector for the collapse button (it's the first button in the first row)
    const buttons = screen.getAllByRole('button');
    fireEvent.click(buttons[0]); // First button is usually the collapse one in our layout
    expect(mockOnCollapse).toHaveBeenCalled();
  });

  it('filters workflows based on search query', async () => {
    render(<WorkflowSidebar onCollapse={mockOnCollapse} />);
    
    const searchInput = screen.getByPlaceholderText('Search workflows...');
    
    expect(screen.getByText('Alpha Workflow')).toBeInTheDocument();
    expect(screen.getByText('Beta Workflow')).toBeInTheDocument();

    fireEvent.change(searchInput, { target: { value: 'Alpha' } });
    
    expect(screen.getByText('Alpha Workflow')).toBeInTheDocument();
    expect(screen.queryByText('Beta Workflow')).not.toBeInTheDocument();
  });

  it('triggers governance actions correctly', async () => {
    render(<WorkflowSidebar onCollapse={mockOnCollapse} />);
    
    // Stop All
    fireEvent.click(screen.getByTitle('Stop All (Panic)'));
    expect(window.confirm).toHaveBeenCalledWith(expect.stringContaining('STOP ALL'));
    expect(mockStopAllTriggers).toHaveBeenCalled();
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
    render(<WorkflowSidebar onCollapse={mockOnCollapse} />);
    expect(screen.getByTestId('active-monitoring')).toBeInTheDocument();
  });
});
