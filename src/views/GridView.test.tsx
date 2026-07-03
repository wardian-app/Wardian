import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { GridView } from './GridView';
import type { AgentConfig, AgentTelemetry } from '../types';
import { useLayoutStore } from '../store/useLayoutStore';
import { useSettingsStore } from '../store/useSettingsStore';

const terminalRenderSpy = vi.hoisted(() => vi.fn());

vi.mock('../features/terminal/AgentTerminal', async () => {
  const React = await vi.importActual<typeof import('react')>('react');
  return {
    AgentTerminal: React.memo((props: {
      sessionId: string;
      onTerminalFocus?: () => void;
    }) => {
      terminalRenderSpy(props);
      return React.createElement(
        'div',
        {
          'data-testid': `terminal-${props.sessionId}`,
          tabIndex: 0,
          onFocus: props.onTerminalFocus,
        },
        `Terminal ${props.sessionId}`,
      );
    }),
  };
});

vi.mock('../features/grid/AgentChatView', () => ({
  AgentChatView: ({
    sessionId,
    autoFocusComposer,
    draft,
    onComposerAutoFocused,
    onDraftChange,
  }: {
    sessionId: string;
    autoFocusComposer?: boolean;
    draft?: string;
    onDraftChange?: (value: string) => void;
    onComposerAutoFocused?: () => void;
  }) => (
    <MockAgentChatView
      autoFocusComposer={autoFocusComposer}
      draft={draft}
      onComposerAutoFocused={onComposerAutoFocused}
      onDraftChange={onDraftChange}
      sessionId={sessionId}
    />
  ),
}));

function MockAgentChatView({
  autoFocusComposer,
  draft,
  onComposerAutoFocused,
  onDraftChange,
  sessionId,
}: {
  autoFocusComposer?: boolean;
  draft?: string;
  onComposerAutoFocused?: () => void;
  onDraftChange?: (value: string) => void;
  sessionId: string;
}) {
  useEffect(() => {
    if (!autoFocusComposer) return;
    document.querySelector<HTMLTextAreaElement>(`[data-testid="chat-${sessionId}"]`)?.focus();
    onComposerAutoFocused?.();
  }, [autoFocusComposer, onComposerAutoFocused, sessionId]);

  return (
    <label>
      Chat {sessionId}
      <textarea
        aria-label={`Mock chat composer ${sessionId}`}
        data-autofocus={autoFocusComposer ? "true" : "false"}
        data-testid={`chat-${sessionId}`}
        onChange={(event) => onDraftChange?.(event.target.value)}
        value={draft ?? ""}
      />
    </label>
  );
}

const agents: AgentConfig[] = [
  { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/project', is_off: false },
  { session_id: 'agent-2', session_name: 'Beta', agent_class: 'Architect', folder: 'C:/project', is_off: false },
];

const telemetry: Record<string, AgentTelemetry> = {};

function gridProps(
  maximizedAgentId: string | null,
  filteredAgents: AgentConfig[] = agents,
  onTerminalFocus = vi.fn(),
  options: {
    selectedAgentIds?: Set<string>;
    offAgentIds?: Set<string>;
    onDelete?: (agentId: string) => void;
  } = {},
): React.ComponentProps<typeof GridView> {
  return {
    filteredAgents,
    telemetry,
    terminalTitles: {},
    currentThoughts: {},
    selectedAgentIds: options.selectedAgentIds ?? new Set(),
    offAgentIds: options.offAgentIds ?? new Set(),
    maximizedAgentId,
    draggedAgentId: null,
    dragOverAgentId: null,
    editingAgentId: null,
    tempName: "",
    theme: "dark",
    onMouseEnterCard: () => {},
    onMouseUp: () => {},
    onMouseDown: () => {},
    onCardClick: () => {},
    onMaximize: () => {},
    onDelete: options.onDelete ?? (() => {}),
    onRename: () => {},
    setEditingAgentId: () => {},
    setTempName: () => {},
    handleTitleChange: () => {},
    deriveCurrentThought: () => ({ thought: '', status: 'Idle' }),
    getStatusColorClass: () => 'bg-wardian-success',
    watchlists: [],
    onAddToList: vi.fn(),
    onRemoveFromList: vi.fn(),
    onQuery: vi.fn(),
    onPause: vi.fn(),
    onRestart: vi.fn(),
    onClear: vi.fn(),
    onTerminalFocus,
  };
}

function renderGrid(
  maximizedAgentId: string | null,
  filteredAgents: AgentConfig[] = agents,
  onTerminalFocus = vi.fn(),
  options: {
    selectedAgentIds?: Set<string>;
    offAgentIds?: Set<string>;
    onDelete?: (agentId: string) => void;
  } = {},
) {
  return render(<GridView {...gridProps(maximizedAgentId, filteredAgents, onTerminalFocus, options)} />);
}

beforeEach(() => {
  localStorage.clear();
  useSettingsStore.setState({ gridCardDisplayMode: 'terminal' });
  terminalRenderSpy.mockClear();
});

describe('GridView maximize behavior', () => {
  it('reports the owning agent when its terminal receives focus', () => {
    const onTerminalFocus = vi.fn();
    renderGrid(null, agents, onTerminalFocus);

    screen.getByTestId('terminal-agent-2').focus();

    expect(onTerminalFocus).toHaveBeenCalledWith('agent-2');
  });

  it('keeps grid selection state out of terminal input props', () => {
    renderGrid(null, agents, vi.fn(), {
      selectedAgentIds: new Set(['agent-1']),
    });

    expect(terminalRenderSpy).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: 'agent-1',
    }));
    expect(terminalRenderSpy).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: 'agent-2',
    }));
    for (const [props] of terminalRenderSpy.mock.calls) {
      expect(props).not.toHaveProperty('isSelected');
    }
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

  it('does not animate terminal card geometry during maximize restore', () => {
    renderGrid(null);

    const card = screen.getByTestId('terminal-agent-1').closest('#agent-card-agent-1');
    expect(card?.className).not.toContain('transition-all');
    expect(card?.className).toContain('transition-colors');
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

  it('keeps terminal panes memoized when only card header state changes', () => {
    const stableProps = {
      filteredAgents: agents,
      telemetry,
      terminalTitles: {},
      selectedAgentIds: new Set<string>(),
      offAgentIds: new Set<string>(),
      maximizedAgentId: null,
      draggedAgentId: null,
      dragOverAgentId: null,
      editingAgentId: null,
      tempName: "",
      theme: "dark" as const,
      onMouseEnterCard: vi.fn(),
      onMouseUp: vi.fn(),
      onMouseDown: vi.fn(),
      onCardClick: vi.fn(),
      onMaximize: vi.fn(),
      onDelete: vi.fn(),
      onRename: vi.fn(),
      setEditingAgentId: vi.fn(),
      setTempName: vi.fn(),
      handleTitleChange: vi.fn(),
      deriveCurrentThought: vi.fn(() => ({ thought: '', status: 'Idle' })),
      getStatusColorClass: vi.fn(() => 'bg-wardian-success'),
      currentThoughts: {},
      watchlists: [],
      onAddToList: vi.fn(),
      onRemoveFromList: vi.fn(),
      onQuery: vi.fn(),
      onPause: vi.fn(),
      onRestart: vi.fn(),
      onClear: vi.fn(),
      onTerminalFocus: vi.fn(),
    };
    const { rerender } = render(<GridView {...stableProps} />);
    expect(terminalRenderSpy).toHaveBeenCalledTimes(2);

    terminalRenderSpy.mockClear();
    rerender(
      <GridView
        {...stableProps}
        currentThoughts={{ 'agent-1': 'Indexing files' }}
      />,
    );

    expect(terminalRenderSpy).not.toHaveBeenCalled();
  });

  it('renders chat cards when Grid card display is chat', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');

    renderGrid(null, agents);

    expect(screen.getByTestId('chat-agent-1')).toBeInTheDocument();
    expect(screen.getByTestId('chat-agent-2')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-agent-1')).not.toBeInTheDocument();
  });

  it('shows a per-card mode switch and toggles one terminal card into focused chat', async () => {
    useSettingsStore.getState().setGridCardDisplayMode('terminal');

    renderGrid(null, agents);

    const alphaMode = screen.getByRole('button', { name: 'Alpha mode: Terminal. Switch to Chat.' });
    expect(screen.getByRole('button', { name: 'Beta mode: Terminal. Switch to Chat.' })).toBeInTheDocument();

    fireEvent.click(alphaMode);

    expect(screen.getByTestId('chat-agent-1')).toHaveFocus();
    await waitFor(() => expect(screen.getByTestId('chat-agent-1')).toHaveAttribute('data-autofocus', 'false'));
    expect(screen.queryByTestId('terminal-agent-1')).not.toBeInTheDocument();
    expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Alpha mode: Chat. Switch to Terminal.' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Chat. Switch to Terminal.' }));

    expect(screen.getByTestId('terminal-agent-1')).toBeInTheDocument();
    expect(screen.queryByTestId('chat-agent-1')).not.toBeInTheDocument();
  });

  it('keeps a per-agent chat draft when switching modes', () => {
    useSettingsStore.getState().setGridCardDisplayMode('terminal');

    renderGrid(null, agents);

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Terminal. Switch to Chat.' }));
    fireEvent.change(screen.getByTestId('chat-agent-1'), { target: { value: 'Long prompt draft' } });

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Chat. Switch to Terminal.' }));
    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Terminal. Switch to Chat.' }));

    expect(screen.getByTestId('chat-agent-1')).toHaveValue('Long prompt draft');
  });

  it('preserves a hidden agent chat mode override while another agent is maximized', () => {
    useSettingsStore.getState().setGridCardDisplayMode('terminal');

    const { rerender } = renderGrid(null, agents);

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Terminal. Switch to Chat.' }));
    expect(screen.getByTestId('chat-agent-1')).toBeInTheDocument();

    rerender(<GridView {...gridProps('agent-2', agents)} />);

    expect(screen.queryByTestId('chat-agent-1')).not.toBeInTheDocument();
    expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();

    rerender(<GridView {...gridProps(null, agents)} />);

    expect(screen.getByTestId('chat-agent-1')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-agent-1')).not.toBeInTheDocument();
  });

  it('clears per-agent chat mode and draft state when deleting a card', () => {
    const onDelete = vi.fn();
    useSettingsStore.getState().setGridCardDisplayMode('terminal');

    renderGrid(null, agents, vi.fn(), { onDelete });

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Terminal. Switch to Chat.' }));
    fireEvent.change(screen.getByTestId('chat-agent-1'), { target: { value: 'Draft to discard' } });

    fireEvent.click(screen.getByRole('button', { name: 'Delete Alpha' }));

    expect(onDelete).toHaveBeenCalledWith('agent-1');
    expect(screen.getByTestId('terminal-agent-1')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Terminal. Switch to Chat.' }));

    expect(screen.getByTestId('chat-agent-1')).toHaveValue('');
  });

  it('uses terminal card width when a chat-default card is switched to terminal', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');

    const { container } = renderGrid(null, [agents[0]]);
    const root = container.firstElementChild as HTMLElement;
    expect(root.style.minWidth).toBe('360px');

    fireEvent.click(screen.getByRole('button', { name: 'Alpha mode: Chat. Switch to Terminal.' }));

    expect(screen.getByTestId('terminal-agent-1')).toBeInTheDocument();
    expect(root.style.minWidth).toBe('520px');
  });

  it('shows hidden card action buttons when they receive keyboard focus', () => {
    renderGrid(null, agents);

    expect(screen.getByRole('button', { name: 'Maximize Alpha' })).toHaveClass('focus:opacity-100');
    expect(screen.getByRole('button', { name: 'Delete Alpha' })).toHaveClass('focus:opacity-100');
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

  it('uses a narrower minimum width for chat cards', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');

    const { container } = renderGrid(null, [agents[0]]);

    const root = container.firstElementChild as HTMLElement;
    expect(root.style.minWidth).toBe('360px');
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

  it('uses the chat minimum width when gridStacked is true in chat mode', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');
    act(() => useLayoutStore.getState().setGridStacked(true));

    const { container } = renderGrid(null, agents);

    const grid = container.firstElementChild as HTMLElement;
    expect(grid.style.minWidth).toBe('360px');
  });

  it('keeps the deliberate two-column preview inside a small viewport', () => {
    const originalWidth = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 800 });
    act(() => {
      window.dispatchEvent(new Event('resize'));
      useLayoutStore.getState().setGridStacked(true);
    });

    try {
      const { container } = renderGrid(null, agents);
      const grid = container.firstElementChild as HTMLElement;

      act(() => {
        fireEvent.mouseDown(container.querySelector('[data-resize-handle="stack-exit"]') as HTMLElement);
      });

      expect(grid.style.gridTemplateColumns).toBe('minmax(0, 0.5fr) minmax(0, 0.5fr)');
      expect(grid.style.minWidth).toBe('100%');
    } finally {
      Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth });
      act(() => {
        window.dispatchEvent(new Event('resize'));
      });
    }
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
