import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, within, waitFor } from '@testing-library/react';
import AgentWatchlist from './AgentWatchlist';
import type { AgentConfig, AgentTelemetry } from '../../types';
import type { Watchlist, WatchlistPrefs, AgentInteractions } from './types';

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
  const mockOnClear = vi.fn();
  const mockOnDelete = vi.fn();
  const mockOnDeleteAgents = vi.fn();
  const mockOnClone = vi.fn();
  const mockOnAddAgentsToList = vi.fn();
  const mockOnRemoveAgentsFromList = vi.fn();
  const mockOnCreateTeam = vi.fn();
  const mockOnUngroupTeam = vi.fn();
  const mockOnAddAgentToTeam = vi.fn();
  const mockOnRemoveAgentFromTeam = vi.fn();
  const mockOnRemoveAgentFromTeamAtEntry = vi.fn();
  const mockOnRenameTeam = vi.fn(async () => {});
  const mockOnReorderTeamMember = vi.fn();
  const mockOnActiveListChange = vi.fn();
  const mockOnWatchlistsChange = vi.fn(async () => {});

  const sampleAgents: AgentConfig[] = [
    { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/test', is_off: false },
    { session_id: 'agent-2', session_name: 'Beta', agent_class: 'Architect', folder: 'C:/test', is_off: true },
  ];

  const sampleTelemetry: Record<string, AgentTelemetry> = {
    'agent-1': { session_id: 'agent-1', current_status: 'idle', cpu_usage: 0, memory_mb: 0, uptime_seconds: 0, query_count: 0, init_timestamp: null, log_path: null },
    'agent-2': { session_id: 'agent-2', current_status: 'offline', cpu_usage: 0, memory_mb: 0, uptime_seconds: 0, query_count: 0, init_timestamp: null, log_path: null },
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
    onClear: mockOnClear,
    onDelete: mockOnDelete,
    onDeleteAgents: mockOnDeleteAgents,
    onClone: mockOnClone,
    onCreateTeam: mockOnCreateTeam,
    onUngroupTeam: mockOnUngroupTeam,
    onAddAgentToTeam: mockOnAddAgentToTeam,
    onRemoveAgentFromTeam: mockOnRemoveAgentFromTeam,
    onRenameTeam: mockOnRenameTeam,
    onReorderTeamMember: mockOnReorderTeamMember,
    onAddToList: vi.fn(),
    onRemoveFromList: vi.fn(),
    onAddAgentsToList: mockOnAddAgentsToList,
    onRemoveAgentsFromList: mockOnRemoveAgentsFromList,
    onRemoveAgentFromTeamAtEntry: mockOnRemoveAgentFromTeamAtEntry,
    collapsed: false,
    watchlists: sampleWatchlists,
    activeListId: 'all',
    onActiveListChange: mockOnActiveListChange,
    onWatchlistsChange: mockOnWatchlistsChange,
  };

  const defaultPrefs: WatchlistPrefs = {
    columns: [
      { id: 'uptime', visible: false },
      { id: 'provider_model', visible: false },
      { id: 'last_queried', visible: true },
    ],
    sort: null,
    preserve_team_grouping_when_sorted: false,
    collapsed_team_ids: [],
  };
  const defaultInteractions: AgentInteractions = {};
  const mockOnPrefsChange = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
  });

  const setRect = (element: Element, top: number, height: number) => {
    Object.defineProperty(element, 'getBoundingClientRect', {
      configurable: true,
      value: () => ({
        top,
        bottom: top + height,
        left: 0,
        right: 240,
        width: 240,
        height,
        x: 0,
        y: top,
        toJSON: () => {},
      }),
    });
  };

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
    expect(within(screen.getByTestId('agent-context-menu')).getByRole('button', { name: 'Clear' })).toBeInTheDocument();
    expect(screen.getByText('Delete')).toBeInTheDocument();
  });

  it('triggers onClear action from the context menu', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);
    fireEvent.click(within(screen.getByTestId('agent-context-menu')).getByRole('button', { name: 'Clear' }));

    expect(mockOnClear).toHaveBeenCalledWith('agent-1');
  });

  it('runs a fresh clone when clicking Clone from a single-agent context menu', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);
    fireEvent.click(within(screen.getByTestId('agent-context-menu')).getByRole('button', { name: 'Clone' }));

    expect(mockOnClone).toHaveBeenCalledWith('agent-1', 'fresh');
  });

  it('offers fresh, profile, and custom clone modes from the clone submenu', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);
    fireEvent.mouseEnter(within(screen.getByTestId('agent-context-menu')).getByRole('button', { name: 'Clone' }));

    expect(screen.getByRole('button', { name: 'Fresh Clone' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Custom Clone' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Profile Clone' }));

    expect(mockOnClone).toHaveBeenCalledWith('agent-1', 'profile');
  });

  it('runs custom clone from the clone submenu', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);
    fireEvent.mouseEnter(within(screen.getByTestId('agent-context-menu')).getByRole('button', { name: 'Clone' }));
    fireEvent.click(screen.getByRole('button', { name: 'Custom Clone' }));

    expect(mockOnClone).toHaveBeenCalledWith('agent-1', 'custom');
  });

  it('uses a bulk context menu when right-clicking inside a multi-selection', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        selectedAgentIds={new Set(['agent-1', 'agent-2'])}
      />
    );
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);

    const menu = screen.getByTestId('agent-context-menu');
    expect(within(menu).getByRole('button', { name: 'Rename' })).toBeDisabled();
    expect(within(menu).getByRole('button', { name: 'Clear Selected' })).toBeInTheDocument();
    expect(within(menu).queryByRole('button', { name: 'Clone' })).not.toBeInTheDocument();

    fireEvent.click(within(menu).getByRole('button', { name: 'Clear Selected' }));

    await waitFor(() => {
      expect(mockOnClear).toHaveBeenCalledWith('agent-1');
      expect(mockOnClear).toHaveBeenCalledWith('agent-2');
    });
  });

  it('applies bulk query, pause, restart, and delete actions to the full selection', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        selectedAgentIds={new Set(['agent-1', 'agent-2'])}
        offAgentIds={new Set()}
      />
    );
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row')!;

    fireEvent.contextMenu(agentRow);
    fireEvent.click(screen.getByRole('button', { name: 'Query Selected' }));
    await waitFor(() => {
      expect(mockOnQuery).toHaveBeenCalledWith('agent-1');
      expect(mockOnQuery).toHaveBeenCalledWith('agent-2');
    });

    fireEvent.contextMenu(agentRow);
    fireEvent.click(screen.getByTestId('context-pause'));
    await waitFor(() => {
      expect(mockOnPause).toHaveBeenCalledWith('agent-1');
      expect(mockOnPause).toHaveBeenCalledWith('agent-2');
    });

    fireEvent.contextMenu(agentRow);
    fireEvent.click(screen.getByTestId('context-start'));
    await waitFor(() => {
      expect(mockOnRestart).toHaveBeenCalledWith('agent-1');
      expect(mockOnRestart).toHaveBeenCalledWith('agent-2');
    });

    fireEvent.contextMenu(agentRow);
    fireEvent.click(screen.getByRole('button', { name: 'Delete Selected' }));
    await waitFor(() => {
      expect(mockOnDeleteAgents).toHaveBeenCalledWith(['agent-1', 'agent-2']);
    });
  });

  it('uses one bulk delete action for a multi-selection', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        selectedAgentIds={new Set(['agent-1', 'agent-2'])}
      />
    );
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row')!;

    fireEvent.contextMenu(agentRow);
    fireEvent.click(screen.getByRole('button', { name: 'Delete Selected' }));

    await waitFor(() => {
      expect(mockOnDeleteAgents).toHaveBeenCalledWith(['agent-1', 'agent-2']);
    });
    expect(mockOnDeleteAgents).toHaveBeenCalledTimes(1);
    expect(mockOnDelete).not.toHaveBeenCalled();
  });

  it('offers team creation for a multi-selection', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        selectedAgentIds={new Set(['agent-1', 'agent-2'])}
      />
    );
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);
    fireEvent.click(screen.getByRole('button', { name: 'Create Team' }));

    expect(mockOnCreateTeam).toHaveBeenCalledWith(['agent-1', 'agent-2']);
  });

  it('offers team creation for a single-agent context menu', async () => {
    render(<AgentWatchlist {...defaultProps} />);
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row');

    fireEvent.contextMenu(agentRow!);
    fireEvent.click(screen.getByRole('button', { name: 'Create Team' }));

    expect(mockOnCreateTeam).toHaveBeenCalledWith(['agent-1']);
  });

  it('renders global teams as grouped blocks with team header actions', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    expect(screen.getByText('Core Dev Swarm')).toBeInTheDocument();
    const teamBlock = screen.getByTestId('team-block-team-1');
    expect(within(teamBlock).getByText('Alpha')).toBeInTheDocument();
    expect(within(teamBlock).getByText('Beta')).toBeInTheDocument();

    fireEvent.contextMenu(screen.getByTestId('team-header-team-1'));
    fireEvent.click(screen.getByRole('button', { name: 'Ungroup Team' }));

    expect(mockOnUngroupTeam).toHaveBeenCalledWith('team-1');
  });

  it('applies normal context actions to every team member from the team menu', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.contextMenu(screen.getByTestId('team-header-team-1'));

    const menu = screen.getByTestId('agent-context-menu');
    expect(within(menu).getByRole('button', { name: 'Rename Team' })).toBeInTheDocument();
    expect(within(menu).queryByRole('button', { name: 'Clone' })).not.toBeInTheDocument();
    fireEvent.click(within(menu).getByRole('button', { name: 'Query Team' }));

    await waitFor(() => {
      expect(mockOnQuery).toHaveBeenCalledWith('agent-1');
      expect(mockOnQuery).toHaveBeenCalledWith('agent-2');
    });
  });

  it('renames a team from the team context menu', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.contextMenu(screen.getByTestId('team-header-team-1'));
    fireEvent.click(screen.getByRole('button', { name: 'Rename Team' }));
    const input = screen.getByDisplayValue('Core Dev Swarm');
    fireEvent.change(input, { target: { value: 'Ops Team' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(mockOnRenameTeam).toHaveBeenCalledWith('team-1', 'Ops Team'));
  });

  it('includes list actions in the team context menu', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        watchlists={[
          { id: 'all', name: 'All Agents', entries: [{ type: 'team', teamId: 'team-1' }] },
          { id: 'later', name: 'Later', entries: [] },
        ]}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.contextMenu(screen.getByTestId('team-header-team-1'));

    const addButton = screen.getByRole('button', { name: 'Add Team to List' });
    fireEvent.mouseEnter(addButton);
    fireEvent.click(screen.getByRole('button', { name: '1. Later' }));

    expect(mockOnAddAgentsToList).toHaveBeenCalledWith('later', ['agent-1', 'agent-2']);
    expect(defaultProps.onAddToList).not.toHaveBeenCalled();
  });

  it('renders team blocks expanded when no team is collapsed', () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
        prefs={{ ...defaultPrefs, collapsed_team_ids: [] }}
        onPrefsChange={mockOnPrefsChange}
      />
    );

    expect(screen.getByRole('button', { name: 'Collapse Core Dev Swarm' })).toBeInTheDocument();
    expect(within(screen.getByTestId('team-block-team-1')).getByText('Alpha')).toBeInTheDocument();
    expect(within(screen.getByTestId('team-block-team-1')).getByText('Beta')).toBeInTheDocument();
  });

  it('toggles a team collapsed from the chevron without selecting the team', () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
        prefs={{ ...defaultPrefs, collapsed_team_ids: [] }}
        onPrefsChange={mockOnPrefsChange}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Collapse Core Dev Swarm' }));

    expect(mockOnSelectionChange).not.toHaveBeenCalled();
    expect(mockOnPrefsChange).toHaveBeenCalledWith({
      ...defaultPrefs,
      collapsed_team_ids: ['team-1'],
    });
  });

  it('hides team members when a team is collapsed but keeps team actions available', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
        prefs={{ ...defaultPrefs, collapsed_team_ids: ['team-1'] }}
        onPrefsChange={mockOnPrefsChange}
      />
    );

    const teamBlock = screen.getByTestId('team-block-team-1');
    expect(screen.getByRole('button', { name: 'Expand Core Dev Swarm' })).toBeInTheDocument();
    expect(within(teamBlock).queryByText('Alpha')).not.toBeInTheDocument();
    expect(within(teamBlock).queryByText('Beta')).not.toBeInTheDocument();

    fireEvent.contextMenu(screen.getByTestId('team-header-team-1'));
    expect(await screen.findByRole('button', { name: 'Query Team' })).toBeInTheDocument();
  });

  it('batches list additions and removals from a multi-selection context menu', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        selectedAgentIds={new Set(['agent-1', 'agent-2'])}
        watchlists={[
          { id: 'inbox', name: 'Inbox', entries: [{ type: 'agent', agentId: 'agent-1' }, { type: 'agent', agentId: 'agent-2' }] },
          { id: 'later', name: 'Later', entries: [] },
        ]}
      />
    );
    const agentRow = screen.getByText('Alpha').closest('.watchlist-row')!;

    fireEvent.contextMenu(agentRow);
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Add Selected to List' }));
    fireEvent.click(screen.getByRole('button', { name: '1. Later' }));

    expect(mockOnAddAgentsToList).toHaveBeenCalledWith('later', ['agent-1', 'agent-2']);
    expect(defaultProps.onAddToList).not.toHaveBeenCalled();

    fireEvent.contextMenu(agentRow);
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Remove Selected from List' }));
    fireEvent.click(screen.getByRole('button', { name: '1. Inbox' }));

    expect(mockOnRemoveAgentsFromList).toHaveBeenCalledWith('inbox', ['agent-1', 'agent-2']);
    expect(defaultProps.onRemoveFromList).not.toHaveBeenCalled();
  });

  it('renders sorted team members as individual rows by default', async () => {
    const agents: AgentConfig[] = [
      { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/test', is_off: false },
      { session_id: 'agent-2', session_name: 'Beta', agent_class: 'Architect', folder: 'C:/test', is_off: false },
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    const telemetry: Record<string, AgentTelemetry> = {
      'agent-1': { ...sampleTelemetry['agent-1'], query_count: 10 },
      'agent-2': { ...sampleTelemetry['agent-2'], query_count: 30, current_status: 'idle' },
      'agent-3': { ...sampleTelemetry['agent-1'], session_id: 'agent-3', query_count: 20 },
    };
    const { container } = render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        telemetry={telemetry}
        offAgentIds={new Set()}
        prefs={{ ...defaultPrefs, sort: { column_id: 'query_count', direction: 'desc' } }}
        onPrefsChange={mockOnPrefsChange}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    expect(screen.queryByTestId('team-block-team-1')).not.toBeInTheDocument();
    const rowText = Array.from(container.querySelectorAll('.watchlist-row')).map((row) => row.textContent);
    expect(rowText[0]).toContain('Beta');
    expect(rowText[1]).toContain('Gamma');
    expect(rowText[2]).toContain('Alpha');
  });

  it('keeps sorted flattened teams without collapse controls', () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        prefs={{ ...defaultPrefs, sort: { column_id: 'agent_name', direction: 'asc' }, collapsed_team_ids: ['team-1'] }}
        onPrefsChange={mockOnPrefsChange}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    expect(screen.queryByTestId('team-block-team-1')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Expand Core Dev Swarm' })).not.toBeInTheDocument();
    expect(screen.getByText('Alpha')).toBeInTheDocument();
    expect(screen.getByText('Beta')).toBeInTheDocument();
  });

  it('renders team blocks when a sort is active and preserving sorted team grouping', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        prefs={{ ...defaultPrefs, sort: { column_id: 'agent_name', direction: 'asc' }, preserve_team_grouping_when_sorted: true }}
        onPrefsChange={mockOnPrefsChange}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    expect(screen.getByTestId('team-block-team-1')).toBeInTheDocument();
    expect(screen.getByText('Core Dev Swarm')).toBeInTheDocument();
  });

  it('adds a dragged solo agent to a team when dropped on the team header', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseUp(screen.getByTestId('team-header-team-1'));

    expect(mockOnAddAgentToTeam).toHaveBeenCalledWith('team-1', 'agent-3');
  });

  it('moves a dragged team member to another team when dropped on that team header', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[
          { id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] },
          { id: 'team-2', name: 'Support Swarm', agentIds: ['agent-3'] },
        ]}
      />
    );

    fireEvent.mouseDown(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);
    fireEvent.mouseUp(screen.getByTestId('team-header-team-2'));

    expect(mockOnAddAgentToTeam).toHaveBeenCalledWith('team-2', 'agent-1');
  });

  it('moves a dragged team member next to the target team in All Agents', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[
          { id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] },
          { id: 'team-2', name: 'Support Swarm', agentIds: ['agent-3'] },
        ]}
      />
    );

    fireEvent.mouseDown(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);
    fireEvent.mouseUp(screen.getByTestId('team-header-team-2'));

    expect(mockOnAddAgentToTeam).toHaveBeenCalledWith('team-2', 'agent-1');
    expect(mockOnReorderAgents).toHaveBeenCalledWith(['agent-2', 'agent-3', 'agent-1']);
  });

  it('reorders team members within the team', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(within(screen.getByTestId('team-block-team-1')).getByText('Beta').closest('.watchlist-row')!);
    fireEvent.mouseUp(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);

    expect(mockOnReorderTeamMember).toHaveBeenCalledWith('team-1', 'agent-2', 'agent-1', 'before');
    expect(mockOnAddAgentToTeam).not.toHaveBeenCalled();
  });

  it('reorders a team member after another member when dropped on the lower half', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );
    const alphaRow = within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!;
    const betaRow = within(screen.getByTestId('team-block-team-1')).getByText('Beta').closest('.watchlist-row')!;
    setRect(betaRow, 100, 40);

    fireEvent.mouseDown(alphaRow);
    fireEvent.mouseMove(betaRow, { clientY: 135 });
    fireEvent.mouseUp(betaRow, { clientY: 135 });

    expect(mockOnReorderTeamMember).toHaveBeenCalledWith('team-1', 'agent-1', 'agent-2', 'after');
  });

  it('adds a solo agent to a team when dropped on a team member row', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseUp(within(screen.getByTestId('team-block-team-1')).getByText('Beta').closest('.watchlist-row')!);

    expect(mockOnAddAgentToTeam).toHaveBeenCalledWith('team-1', 'agent-3');
    expect(mockOnReorderAgents).not.toHaveBeenCalled();
  });

  it('adds a solo watchlist row to a team when dropped on a team member row', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        watchlists={[
          {
            id: 'today',
            name: 'Today',
            entries: [
              { type: 'agent', agentId: 'agent-3' },
              { type: 'team', teamId: 'team-1' },
            ],
          },
        ]}
        activeListId="today"
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseUp(within(screen.getByTestId('team-block-team-1')).getByText('Beta').closest('.watchlist-row')!);

    expect(mockOnAddAgentToTeam).toHaveBeenCalledWith('team-1', 'agent-3');
    expect(mockOnWatchlistsChange).not.toHaveBeenCalled();
  });

  it('highlights the whole team block when a solo agent is dragged over a team member row', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseEnter(within(screen.getByTestId('team-block-team-1')).getByText('Beta').closest('.watchlist-row')!);

    expect(screen.getByTestId('team-block-team-1')).toHaveClass('team-drop-inside');
  });

  it('places a solo agent before a team when dropped on the upper edge of the team', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );
    const beforeZone = screen.getByTestId('team-drop-before-team-1');

    fireEvent.mouseDown(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseEnter(beforeZone);
    fireEvent.mouseUp(beforeZone);

    expect(mockOnAddAgentToTeam).not.toHaveBeenCalled();
    expect(mockOnReorderAgents).toHaveBeenCalledWith(['agent-3', 'agent-1', 'agent-2']);
  });

  it('reorders team blocks inside a watchlist', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        watchlists={[
          {
            id: 'today',
            name: 'Today',
            entries: [{ type: 'team', teamId: 'team-1' }, { type: 'team', teamId: 'team-2' }],
          },
        ]}
        activeListId="today"
        teams={[
          { id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1'] },
          { id: 'team-2', name: 'Support Swarm', agentIds: ['agent-2'] },
        ]}
      />
    );

    fireEvent.mouseDown(screen.getByTestId('team-header-team-2'));
    fireEvent.mouseEnter(screen.getByTestId('team-header-team-1'));
    fireEvent.mouseUp(screen.getByTestId('team-header-team-1'));

    await waitFor(() => expect(mockOnWatchlistsChange).toHaveBeenCalledWith([
      {
        id: 'today',
        name: 'Today',
        agentIds: [],
        entries: [{ type: 'team', teamId: 'team-2' }, { type: 'team', teamId: 'team-1' }],
      },
    ]));
  });

  it('reorders a team when dropped on another teams member row', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        watchlists={[
          {
            id: 'today',
            name: 'Today',
            entries: [{ type: 'team', teamId: 'team-1' }, { type: 'team', teamId: 'team-2' }],
          },
        ]}
        activeListId="today"
        teams={[
          { id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1'] },
          { id: 'team-2', name: 'Support Swarm', agentIds: ['agent-2'] },
        ]}
      />
    );

    fireEvent.mouseDown(screen.getByTestId('team-header-team-2'));
    fireEvent.mouseEnter(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);
    fireEvent.mouseUp(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);

    await waitFor(() => expect(mockOnWatchlistsChange).toHaveBeenCalledWith([
      {
        id: 'today',
        name: 'Today',
        agentIds: [],
        entries: [{ type: 'team', teamId: 'team-2' }, { type: 'team', teamId: 'team-1' }],
      },
    ]));
  });

  it('reorders a team relative to a solo row inside a watchlist', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        watchlists={[
          {
            id: 'today',
            name: 'Today',
            entries: [
              { type: 'agent', agentId: 'agent-3' },
              { type: 'team', teamId: 'team-1' },
            ],
          },
        ]}
        activeListId="today"
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(screen.getByTestId('team-header-team-1'));
    fireEvent.mouseEnter(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseUp(screen.getByText('Gamma').closest('.watchlist-row')!);

    await waitFor(() => expect(mockOnWatchlistsChange).toHaveBeenCalledWith([
      {
        id: 'today',
        name: 'Today',
        agentIds: ['agent-3'],
        entries: [
          { type: 'team', teamId: 'team-1' },
          { type: 'agent', agentId: 'agent-3' },
        ],
      },
    ]));
  });

  it('removes a dragged team member from its team when dropped on a solo row', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);
    fireEvent.mouseEnter(screen.getByText('Gamma').closest('.watchlist-row')!);
    fireEvent.mouseUp(screen.getByText('Gamma').closest('.watchlist-row')!);

    expect(mockOnRemoveAgentFromTeam).toHaveBeenCalledWith('team-1', 'agent-1', 'agent-3', 'before');
  });

  it('extracts a team member to a team edge in a watchlist with one atomic callback', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        watchlists={[
          {
            id: 'today',
            name: 'Today',
            entries: [{ type: 'team', teamId: 'team-1' }],
          },
        ]}
        activeListId="today"
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );
    const beforeZone = screen.getByTestId('team-drop-before-team-1');

    fireEvent.mouseDown(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);
    fireEvent.mouseEnter(beforeZone);
    fireEvent.mouseUp(beforeZone);

    expect(mockOnRemoveAgentFromTeamAtEntry).toHaveBeenCalledWith(
      'team-1',
      'agent-1',
      { type: 'team', teamId: 'team-1' },
      'before',
      'today',
    );
    expect(mockOnRemoveAgentFromTeam).not.toHaveBeenCalled();
    expect(mockOnWatchlistsChange).not.toHaveBeenCalled();
  });

  it('keeps a team member below the remaining team when dragged out in All Agents', async () => {
    const agents: AgentConfig[] = [
      ...sampleAgents,
      { session_id: 'agent-3', session_name: 'Gamma', agent_class: 'QA', folder: 'C:/test', is_off: false },
    ];
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={agents}
        teams={[{ id: 'team-1', name: 'Core Dev Swarm', agentIds: ['agent-1', 'agent-2'] }]}
      />
    );

    fireEvent.mouseDown(within(screen.getByTestId('team-block-team-1')).getByText('Alpha').closest('.watchlist-row')!);
    fireEvent.mouseUp(screen.getByText('Gamma').closest('.watchlist-row')!);

    expect(mockOnRemoveAgentFromTeam).toHaveBeenCalledWith('team-1', 'agent-1', 'agent-3', 'before');
    expect(mockOnReorderAgents).toHaveBeenCalledWith(['agent-2', 'agent-1', 'agent-3']);
  });

  it('switches to single-agent context when right-clicking outside a multi-selection', async () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        selectedAgentIds={new Set(['agent-1'])}
      />
    );
    const betaRow = screen.getByText('Beta').closest('.watchlist-row');

    fireEvent.contextMenu(betaRow!);

    expect(mockOnSelectionChange).toHaveBeenCalledWith(new Set(['agent-2']));
    expect(screen.getByRole('button', { name: 'Rename' })).not.toBeDisabled();
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

  it('shows last_queried column header when visible in prefs', () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        prefs={defaultPrefs}
        onPrefsChange={mockOnPrefsChange}
        interactions={defaultInteractions}
      />
    );
    expect(screen.getByText('Last')).toBeInTheDocument();
  });

  it('hides uptime column when not visible in prefs', () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        prefs={defaultPrefs}
        onPrefsChange={mockOnPrefsChange}
        interactions={defaultInteractions}
      />
    );
    expect(screen.queryByText('Up')).not.toBeInTheDocument();
  });

  it('formats known provider names in the provider column', () => {
    render(
      <AgentWatchlist
        {...defaultProps}
        agents={[
          {
            ...sampleAgents[0],
            provider: 'antigravity',
            model: 'planner',
          },
        ]}
        prefs={{
          ...defaultPrefs,
          columns: defaultPrefs.columns.map((column) =>
            column.id === 'provider_model' ? { ...column, visible: true } : column,
          ),
        }}
        onPrefsChange={mockOnPrefsChange}
        interactions={defaultInteractions}
      />
    );

    expect(screen.getByText('Antigravity · planner')).toBeInTheDocument();
    expect(screen.queryByText('antigravity · planner')).not.toBeInTheDocument();
  });
});
