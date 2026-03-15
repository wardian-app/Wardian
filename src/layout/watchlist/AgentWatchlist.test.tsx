import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import AgentWatchlist from './AgentWatchlist';
import type { AgentConfig, AgentTelemetry } from '../../types';
import type { Watchlist } from './types';

// Mock matchMedia
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation(query => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

describe('AgentWatchlist', () => {
  const mockOnSelectionChange = vi.fn();
  const mockOnAgentClick = vi.fn();
  const mockOnRename = vi.fn(async () => {});
  const mockOnReorderAgents = vi.fn();
  const mockOnQuery = vi.fn();
  const mockOnPause = vi.fn();
  const mockOnRestart = vi.fn();
  const mockOnDelete = vi.fn();
  const mockOnActiveListChange = vi.fn();
  const mockOnWatchlistsChange = vi.fn(async () => {});

  const sampleAgents: AgentConfig[] = [
    { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/test', is_off: false },
    { session_id: 'agent-2', session_name: 'Beta', agent_class: 'Architect', folder: 'C:/test', is_off: true },
  ];

  const sampleTelemetry: Record<string, AgentTelemetry> = {
    'agent-1': { status: 'idle', cpu: 0, memory: 0, uptime: 0, last_seen: '' },
    'agent-2': { status: 'offline', cpu: 0, memory: 0, uptime: 0, last_seen: '' },
  };

  const sampleWatchlists: Watchlist[] = [
    { id: 'all', name: 'All Agents', agentIds: ['agent-1', 'agent-2'] },
  ];

  const defaultProps = {
    agents: sampleAgents,
    telemetry: sampleTelemetry,
    terminalTitles: {},
    currentThoughts: {},
    selectedAgentIds: new Set<string>(),
    offAgentIds: new Set(['agent-2']),
    onSelectionChange: mockOnSelectionChange,
    onAgentClick: mockOnAgentClick,
    onRename: mockOnRename,
    onReorderAgents: mockOnReorderAgents,
    onQuery: mockOnQuery,
    onPause: mockOnPause,
    onRestart: mockOnRestart,
    onDelete: mockOnDelete,
    collapsed: false,
    onCollapse: vi.fn(),
    watchlists: sampleWatchlists,
    activeListId: 'all',
    onActiveListChange: mockOnActiveListChange,
    onWatchlistsChange: mockOnWatchlistsChange,
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders agent rows correctly', () => {
    render(<AgentWatchlist {...defaultProps} />);
    expect(screen.getByText('Alpha')).toBeInTheDocument();
    expect(screen.getByText('Beta')).toBeInTheDocument();
  });

  it('filters agents by search term', () => {
    render(<AgentWatchlist {...defaultProps} />);
    const searchInput = screen.getByPlaceholderText('Search agents...');
    
    fireEvent.change(searchInput, { target: { value: 'Alpha' } });
    
    expect(screen.getByText('Alpha')).toBeInTheDocument();
    expect(screen.queryByText('Beta')).not.toBeInTheDocument();
  });

  it('opens context menu on right click', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');
    expect(agentRow).not.toBeNull();

    fireEvent.contextMenu(agentRow!);

    expect(screen.getByText('Rename')).toBeInTheDocument();
    expect(screen.getByText('Pause')).toBeInTheDocument();
    expect(screen.getByText('Restart')).toBeInTheDocument();
    expect(screen.getByText('Delete')).toBeInTheDocument();
  });

  it('disables Pause for already off agents', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Beta').closest('.watchlist-row');
    
    fireEvent.contextMenu(agentRow!);

    const pauseButton = screen.getByText('Pause').closest('button');
    expect(pauseButton).toBeDisabled();
    expect(pauseButton).toHaveClass('opacity-50');
  });

  it('triggers onPause action', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');
    
    fireEvent.contextMenu(agentRow!);
    fireEvent.click(screen.getByText('Pause'));

    expect(mockOnPause).toHaveBeenCalledWith('agent-1');
  });

  it('shows Start instead of Restart for off agents', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Beta').closest('.watchlist-row');
    
    fireEvent.contextMenu(agentRow!);
    
    expect(screen.getByText('Start')).toBeInTheDocument();
    expect(screen.queryByText('Restart')).not.toBeInTheDocument();
  });
});
