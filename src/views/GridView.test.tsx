import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import { GridView } from './GridView';
import type { AgentConfig, AgentTelemetry } from '../types';
import { useLayoutStore } from '../store/useLayoutStore';
import { useSettingsStore } from '../store/useSettingsStore';

vi.mock('../features/terminal/AgentTerminal', () => ({
  AgentTerminal: ({
    sessionId,
    onTerminalFocus,
  }: {
    sessionId: string;
    onTerminalFocus?: () => void;
  }) => (
    <div
      data-testid={`terminal-${sessionId}`}
      tabIndex={0}
      onFocus={onTerminalFocus}
    >
      Terminal {sessionId}
    </div>
  ),
}));

vi.mock('../features/grid/AgentChatView', () => ({
  AgentChatView: ({
    sessionId,
  }: {
    sessionId: string;
  }) => <div data-testid={`chat-${sessionId}`}>Chat {sessionId}</div>,
}));

const agents: AgentConfig[] = [
  { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/project', is_off: false },
  { session_id: 'agent-2', session_name: 'Beta', agent_class: 'Architect', folder: 'C:/project', is_off: false },
];

const telemetry: Record<string, AgentTelemetry> = {};

function renderGrid(
  maximizedAgentId: string | null,
  filteredAgents: AgentConfig[] = agents,
  onTerminalFocus = vi.fn(),
  options: {
    selectedAgentIds?: Set<string>;
    offAgentIds?: Set<string>;
  } = {},
) {
  return render(
    <GridView
      filteredAgents={filteredAgents}
      telemetry={telemetry}
      terminalTitles={{}}
      currentThoughts={{}}
      selectedAgentIds={options.selectedAgentIds ?? new Set()}
      offAgentIds={options.offAgentIds ?? new Set()}
      maximizedAgentId={maximizedAgentId}
      draggedAgentId={null}
      dragOverAgentId={null}
      editingAgentId={null}
      tempName=""
      theme="dark"
      onMouseEnterCard={() => {}}
      onMouseUp={() => {}}
      onMouseDown={() => {}}
      onCardClick={() => {}}
      onMaximize={() => {}}
      onDelete={() => {}}
      onRename={() => {}}
      setEditingAgentId={() => {}}
      setTempName={() => {}}
      handleTitleChange={() => {}}
      deriveCurrentThought={() => ({ thought: '', status: 'Idle' })}
      getStatusColorClass={() => 'bg-wardian-success'}
      watchlists={[]}
      onAddToList={vi.fn()}
      onRemoveFromList={vi.fn()}
      onQuery={vi.fn()}
      onPause={vi.fn()}
      onRestart={vi.fn()}
      onClear={vi.fn()}
      onTerminalFocus={onTerminalFocus}
    />
  );
}

beforeEach(() => {
  localStorage.clear();
  useSettingsStore.setState({ grid_card_display_mode: 'terminal' });
});

describe('GridView maximize behavior', () => {
  it('reports the owning agent when its terminal receives focus', () => {
    const onTerminalFocus = vi.fn();
    renderGrid(null, agents, onTerminalFocus);

    screen.getByTestId('terminal-agent-2').focus();

    expect(onTerminalFocus).toHaveBeenCalledWith('agent-2');
  });

  it('does not size each mobile card to the full viewport height', () => {
    const originalWidth = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 800 });
    act(() => {
      window.dispatchEvent(new Event('resize'));
    });

    try {
      const { container } = renderGrid(null);

      const root = container.firstElementChild as HTMLElement;
      expect(root.style.gridTemplateColumns).toBe('1fr');
      expect(root.style.gridAutoRows).not.toBe('100%');
      expect(screen.getByTestId('terminal-agent-1')).toBeInTheDocument();
      expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();
    } finally {
      Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth });
      act(() => {
        window.dispatchEvent(new Event('resize'));
      });
    }
  });

  it('maximized terminals fill the grid container', () => {
    renderGrid('agent-1');

    const card = screen.getByTestId('terminal-agent-1').closest('#agent-card-agent-1');
    expect(card?.className).toContain('h-full');
    expect(card?.className).toContain('w-full');
    expect(card?.className).not.toContain('fixed');
  });

  it('falls back to the filtered grid when the maximized agent is no longer visible', () => {
    const visibleSubset = agents.filter((agent) => agent.session_id !== 'agent-1');
    const { container } = renderGrid('agent-1', visibleSubset);

    const root = container.firstElementChild;
    // New grid implementation uses grid display
    expect((root as HTMLElement).style.display).toBe('grid');
    expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();
  });

  it('hides a selected off agent from the main grid', () => {
    renderGrid(null, agents, vi.fn(), {
      selectedAgentIds: new Set(['agent-1']),
      offAgentIds: new Set(['agent-1']),
    });

    expect(screen.queryByTestId('terminal-agent-1')).not.toBeInTheDocument();
    expect(screen.queryByTestId('terminal-agent-2')).toBeInTheDocument();
  });

  it('falls back to the active grid when the maximized agent is off', () => {
    const { container } = renderGrid('agent-1', agents, vi.fn(), {
      offAgentIds: new Set(['agent-1']),
    });
    const root = container.firstElementChild as HTMLElement;

    expect(root.style.gridAutoRows).not.toBe('100%');
    expect(screen.queryByTestId('terminal-agent-1')).not.toBeInTheDocument();
    expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();
  });

  it('renders terminal cards when Grid card display is terminal', () => {
    useSettingsStore.getState().setGridCardDisplayMode('terminal');

    renderGrid(null, agents);

    expect(screen.getByTestId('terminal-agent-1')).toBeInTheDocument();
    expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();
    expect(screen.queryByTestId('chat-agent-1')).not.toBeInTheDocument();
  });

  it('renders chat cards when Grid card display is chat', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');

    renderGrid(null, agents);

    expect(screen.getByTestId('chat-agent-1')).toBeInTheDocument();
    expect(screen.getByTestId('chat-agent-2')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-agent-1')).not.toBeInTheDocument();
  });

  it('gives a single visible agent the full grid width instead of a stale narrow track', () => {
    act(() => useLayoutStore.getState().setColumnTracks([0.2, 0.8]));

    const { container } = renderGrid(null, [agents[0]]);

    const root = container.firstElementChild as HTMLElement;
    expect(root.style.gridTemplateColumns).toBe('1fr');
  });

  it('keeps the grid wide enough for terminal input rows when the app shell is narrow', () => {
    const { container } = renderGrid(null, [agents[0]]);

    const root = container.firstElementChild as HTMLElement;
    expect(root.style.minWidth).toBe('520px');
  });
});

describe('GridView density', () => {
  it('renders compact single-row card headers with visible agent class', () => {
    renderGrid(null);

    const header = screen.getByTestId('agent-card-header-agent-1');
    const agentName = screen.getByRole('heading', { name: 'Alpha (Coder)' });
    expect(header).toHaveAttribute('data-density', 'compact');
    expect(agentName).toHaveClass('text-[15px]');
    expect(agentName).toHaveClass('leading-5');
    expect(screen.getByText('(Coder)')).toBeInTheDocument();
  });

  it('uses a larger status orb in compact card headers', () => {
    renderGrid(null);

    const header = screen.getByTestId('agent-card-header-agent-1');
    const statusOrb = header.querySelector('[data-testid="agent-card-status-orb"]');
    expect(statusOrb).toHaveClass('w-2.5');
    expect(statusOrb).toHaveClass('h-2.5');
  });

  it('keeps dense UI proportions roomy enough for VSCode-style scanning', async () => {
    const { readFileSync } = await import("node:fs");
    const { cwd } = await import("node:process");
    const appStyles = readFileSync(`${cwd()}/src/styles/App.css`, "utf8") as string;

    expect(appStyles).toContain("--sidebar-primary-width: 48px;");
    expect(appStyles).toContain("--density-grid-header-min-height: 44px;");
    expect(appStyles).toContain("--density-grid-header-padding-y: 8px;");
  });
});

describe('GridView stacked mode', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('renders single column when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    const { container } = renderGrid(null, agents);
    const grid = container.firstElementChild as HTMLElement;
    expect(grid.style.gridTemplateColumns).toBe('1fr');
  });

  it('uses a single-column minimum width when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    const { container } = renderGrid(null, agents);
    const grid = container.firstElementChild as HTMLElement;
    expect(grid.style.minWidth).toBe('520px');
  });

  it('renders per-cell stack-exit handles when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    const { container } = renderGrid(null, agents);
    const handles = container.querySelectorAll('[data-resize-handle="stack-exit"]');
    expect(handles.length).toBe(agents.length);
  });

  it('hides inter-column gutters when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    const { container } = renderGrid(null, agents);
    expect(container.querySelectorAll('[data-resize-handle="h"]').length).toBe(0);
  });

  it('keeps row resize gutters available when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    const { container } = renderGrid(null, agents);
    expect(container.querySelectorAll('[data-resize-handle="v"]').length).toBe(agents.length - 1);
  });

  it('defines a visible horizontal resize guide style', async () => {
    const { readFileSync } = await import("node:fs");
    const { cwd } = await import("node:process");
    const appStyles = readFileSync(`${cwd()}/src/styles/App.css`, "utf8") as string;

    expect(appStyles).toContain(".grid-guide-line-h");
    expect(appStyles).toContain("border-top: 1px dashed var(--color-wardian-accent);");
  });
});
