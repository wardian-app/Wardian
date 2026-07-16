import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { AgentsOverviewView, agentsOverviewGridTemplateColumns } from './AgentsOverviewView';
import type { AgentConfig, AgentTelemetry } from '../types';
import { useLayoutStore } from '../store/useLayoutStore';
import { useSettingsStore } from '../store/useSettingsStore';

const terminalRenderSpy = vi.hoisted(() => vi.fn());

vi.mock('../features/terminal/AgentTerminal', async () => {
  const React = await vi.importActual<typeof import('react')>('react');
  return {
    AgentTerminal: React.memo((props: {
      presentationId: string;
      sessionId: string;
      visibility: "visible" | "hidden";
      renderState: "mounted" | "suspended";
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
): React.ComponentProps<typeof AgentsOverviewView> {
  return {
    surfaceId: 'overview-surface',
    mode: maximizedAgentId ? 'single' : 'grid',
    filteredAgents,
    telemetry,
    terminalTitles: {},
    currentThoughts: {},
    selectedAgentIds: options.selectedAgentIds ?? new Set(),
    offAgentIds: options.offAgentIds ?? new Set(),
    focusedAgentId: maximizedAgentId,
    draggedAgentId: null,
    dragOverAgentId: null,
    editingAgentId: null,
    tempName: "",
    theme: "dark",
    onMouseEnterCard: () => {},
    onMouseUp: () => {},
    onMouseDown: () => {},
    onCardClick: () => {},
    onModeChange: () => {},
    onExitSingle: () => {},
    onFocusedAgentChange: () => {},
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
  return render(<AgentsOverviewView {...gridProps(maximizedAgentId, filteredAgents, onTerminalFocus, options)} />);
}

beforeEach(() => {
  localStorage.clear();
  act(() => useLayoutStore.getState().resetGridLayout());
  useSettingsStore.setState({ gridCardDisplayMode: 'terminal' });
  terminalRenderSpy.mockClear();
});

describe('AgentsOverviewView maximize behavior', () => {
  it('keeps every terminal resident across viewport exit and re-entry at or below capacity', async () => {
    const originalIntersectionObserver = globalThis.IntersectionObserver;
    let observerCallback: IntersectionObserverCallback | null = null;
    const observedCards = new Map<string, Element>();
    globalThis.IntersectionObserver = class IntersectionObserver {
      root = null;
      rootMargin = '';
      thresholds = [];
      constructor(callback: IntersectionObserverCallback) {
        observerCallback = callback;
      }
      observe(target: Element) {
        const agentId = (target as HTMLElement).dataset.agentGridCardId;
        if (agentId) observedCards.set(agentId, target);
      }
      unobserve() {}
      disconnect() {}
      takeRecords() { return []; }
    } as unknown as typeof IntersectionObserver;

    try {
      renderGrid(null, agents);

      await waitFor(() => {
        const latestProps = new Map(
          terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
        );
        expect(latestProps.get('agent-1')).toMatchObject({ renderState: 'mounted' });
        expect(latestProps.get('agent-2')).toMatchObject({ renderState: 'mounted' });
      });

      const firstCard = observedCards.get('agent-1');
      if (!observerCallback || !firstCard) throw new Error('expected viewport observer');
      act(() => observerCallback!([{
        isIntersecting: false,
        target: firstCard,
      } as IntersectionObserverEntry], {} as IntersectionObserver));
      act(() => observerCallback!([{
        isIntersecting: true,
        target: firstCard,
      } as IntersectionObserverEntry], {} as IntersectionObserver));

      const latestProps = new Map(
        terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
      );
      expect(latestProps.get('agent-1')).toMatchObject({ renderState: 'mounted' });
      expect(latestProps.get('agent-2')).toMatchObject({ renderState: 'mounted' });
    } finally {
      globalThis.IntersectionObserver = originalIntersectionObserver;
    }
  });

  it('evicts a non-near resident only when admitting an approaching card above capacity', async () => {
    const originalIntersectionObserver = globalThis.IntersectionObserver;
    const rectSpy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      width: 900,
      height: 600,
      top: 0,
      left: 0,
      right: 900,
      bottom: 600,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    } as DOMRect);
    let observerCallback: IntersectionObserverCallback | null = null;
    const observedCards = new Map<string, Element>();
    globalThis.IntersectionObserver = class IntersectionObserver {
      root = null;
      rootMargin = '';
      thresholds = [];
      constructor(callback: IntersectionObserverCallback) {
        observerCallback = callback;
      }
      observe(target: Element) {
        const agentId = (target as HTMLElement).dataset.agentGridCardId;
        if (agentId) observedCards.set(agentId, target);
      }
      unobserve() {}
      disconnect() {}
      takeRecords() { return []; }
    } as unknown as typeof IntersectionObserver;

    const manyAgents = Array.from({ length: 30 }, (_, index): AgentConfig => ({
      session_id: `agent-${index + 1}`,
      session_name: `Agent ${index + 1}`,
      agent_class: 'Coder',
      folder: 'C:/project',
      is_off: false,
    }));

    try {
      renderGrid(null, manyAgents);

      await waitFor(() => {
        const latestProps = new Map<string, {
          visibility: 'visible' | 'hidden';
          renderState: 'mounted' | 'suspended';
        }>();
        for (const [props] of terminalRenderSpy.mock.calls) {
          latestProps.set(props.sessionId, props);
        }
        expect(Array.from(latestProps.values()).filter(
          ({ renderState }) => renderState === 'mounted',
        )).toHaveLength(24);
        expect(latestProps.get('agent-25')).toMatchObject({
          visibility: 'hidden',
          renderState: 'suspended',
        });
      });

      const firstCard = observedCards.get('agent-1');
      const approachingCard = observedCards.get('agent-25');
      if (!observerCallback || !firstCard || !approachingCard) throw new Error('expected viewport observer');
      act(() => observerCallback!([
        {
          isIntersecting: false,
          target: firstCard,
        } as IntersectionObserverEntry,
        {
          isIntersecting: true,
          target: approachingCard,
        } as IntersectionObserverEntry,
      ], {} as IntersectionObserver));

      await waitFor(() => {
        const latestProps = new Map(
          terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
        );
        expect(latestProps.get('agent-1')).toMatchObject({ renderState: 'suspended' });
        expect(latestProps.get('agent-25')).toMatchObject({ renderState: 'mounted' });
      });
    } finally {
      globalThis.IntersectionObserver = originalIntersectionObserver;
      rectSpy.mockRestore();
    }
  });

  it('keeps every renderer suspended while the Dockview viewport has zero geometry', async () => {
    const originalIntersectionObserver = globalThis.IntersectionObserver;
    globalThis.IntersectionObserver = class IntersectionObserver {
      root = null;
      rootMargin = '';
      thresholds = [];
      observe() {}
      unobserve() {}
      disconnect() {}
      takeRecords() { return []; }
    } as unknown as typeof IntersectionObserver;

    try {
      renderGrid(null, Array.from({ length: 30 }, (_, index): AgentConfig => ({
        session_id: `zero-agent-${index + 1}`,
        session_name: `Agent ${index + 1}`,
        agent_class: 'Coder',
        folder: 'C:/project',
        is_off: false,
      })));

      await waitFor(() => {
        const latestProps = new Map(
          terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
        );
        expect(Array.from(latestProps.values()).filter(
          ({ renderState }) => renderState === 'mounted',
        )).toHaveLength(0);
      });
    } finally {
      globalThis.IntersectionObserver = originalIntersectionObserver;
    }
  });

  it('derives stable terminal presentation IDs from the surface and agent IDs', () => {
    renderGrid(null, agents);

    expect(terminalRenderSpy).toHaveBeenCalledWith(expect.objectContaining({
      presentationId: 'overview-surface:agent:agent-1',
      sessionId: 'agent-1',
    }));
    expect(terminalRenderSpy).toHaveBeenCalledWith(expect.objectContaining({
      presentationId: 'overview-surface:agent:agent-2',
      sessionId: 'agent-2',
    }));
  });

  it('suspends every terminal presentation while the containing surface is hidden', () => {
    render(<AgentsOverviewView {...gridProps(null, agents)} surfaceVisibility="hidden" />);

    expect(terminalRenderSpy).toHaveBeenCalledTimes(2);
    for (const [props] of terminalRenderSpy.mock.calls) {
      expect(props).toMatchObject({ visibility: "hidden", renderState: "suspended" });
    }
  });

  it('preserves resident terminals while hidden without admitting new renderers', async () => {
    const originalIntersectionObserver = globalThis.IntersectionObserver;
    globalThis.IntersectionObserver = undefined as unknown as typeof IntersectionObserver;

    const manyAgents = Array.from({ length: 25 }, (_, index): AgentConfig => ({
      session_id: `resident-agent-${index + 1}`,
      session_name: `Agent ${index + 1}`,
      agent_class: 'Coder',
      folder: 'C:/project',
      is_off: false,
    }));

    try {
      const view = render(
        <AgentsOverviewView {...gridProps(null, manyAgents)} surfaceVisibility="visible" />,
      );

      await waitFor(() => {
        const latestProps = new Map(
          terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
        );
        expect(latestProps.get('resident-agent-1')).toMatchObject({
          visibility: 'visible',
          renderState: 'mounted',
        });
        expect(latestProps.get('resident-agent-25')).toMatchObject({
          visibility: 'hidden',
          renderState: 'suspended',
        });
      });

      view.rerender(
        <AgentsOverviewView {...gridProps(null, manyAgents)} surfaceVisibility="hidden" />,
      );

      await waitFor(() => {
        const latestProps = new Map(
          terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
        );
        expect(latestProps.get('resident-agent-1')).toMatchObject({
          visibility: 'hidden',
          renderState: 'mounted',
        });
        expect(latestProps.get('resident-agent-25')).toMatchObject({
          visibility: 'hidden',
          renderState: 'suspended',
        });
      });

      view.rerender(
        <AgentsOverviewView {...gridProps(null, manyAgents)} surfaceVisibility="visible" />,
      );

      await waitFor(() => {
        const latestProps = new Map(
          terminalRenderSpy.mock.calls.map(([props]) => [props.sessionId, props]),
        );
        expect(latestProps.get('resident-agent-1')).toMatchObject({
          visibility: 'visible',
          renderState: 'mounted',
        });
        expect(latestProps.get('resident-agent-25')).toMatchObject({
          visibility: 'hidden',
          renderState: 'suspended',
        });
      });
    } finally {
      globalThis.IntersectionObserver = originalIntersectionObserver;
    }
  });

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

  it('preserves explicit Grid columns and row sizing in a narrow viewport', () => {
    const originalWidth = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 800 });
    act(() => {
      window.dispatchEvent(new Event('resize'));
    });

    try {
      renderGrid(null);

      const root = screen.getByTestId('agent-grid');
      expect(root.style.gridTemplateColumns).toBe('minmax(0, 0.5fr) minmax(0, 0.5fr)');
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

  it('restores explicit Single through the surface callback', () => {
    const onExitSingle = vi.fn();
    render(<AgentsOverviewView {...gridProps('agent-1')} onExitSingle={onExitSingle} />);

    fireEvent.click(screen.getByRole('button', { name: 'Minimize Alpha' }));

    expect(onExitSingle).toHaveBeenCalledTimes(1);
  });

  it('does not present responsive Auto Single as an explicit maximized card', () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;
    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
    vi.useFakeTimers();

    try {
      const props = { ...gridProps(null, [agents[0]]), mode: 'auto' as const };
      render(<AgentsOverviewView {...props} />);
      const container = screen.getByTestId('agents-overview-container');
      act(() => {
        resizeCallback?.([{
          target: container,
          contentRect: { width: 800, height: 600 } as DOMRectReadOnly,
        } as unknown as ResizeObserverEntry], {} as ResizeObserver);
        vi.advanceTimersByTime(120);
      });

      expect(screen.queryByRole('button', { name: 'Minimize Alpha' })).not.toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Maximize Alpha' })).toBeInTheDocument();
      expect(terminalRenderSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        sessionId: 'agent-1',
        isMaximized: false,
      }));
    } finally {
      vi.useRealTimers();
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it('keeps a narrow Auto roster visible in a floor-sized one-column grid', () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;
    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
    vi.useFakeTimers();

    try {
      render(<AgentsOverviewView {...gridProps(null, agents)} mode="auto" />);
      const container = screen.getByTestId('agents-overview-container');
      act(() => {
        resizeCallback?.([{
          target: container,
          contentRect: { width: 800, height: 600 } as DOMRectReadOnly,
        } as unknown as ResizeObserverEntry], {} as ResizeObserver);
        vi.advanceTimersByTime(120);
      });

      const grid = screen.getByTestId('agent-grid');
      expect(grid).toHaveAttribute('data-overview-mode', 'grid');
      expect(grid.style.gridTemplateColumns).toBe('1fr');
      expect(grid.style.minWidth).toBe('520px');
      expect(screen.getAllByTestId('agent-card')).toHaveLength(2);
      expect(screen.queryByRole('button', { name: /^Minimize / })).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it('does not animate terminal card geometry during maximize restore', () => {
    renderGrid(null);

    const card = screen.getByTestId('terminal-agent-1').closest('#agent-card-agent-1');
    expect(card?.className).not.toContain('transition-all');
    expect(card?.className).toContain('transition-colors');
  });

  it('falls back to the filtered grid when the maximized agent is no longer visible', () => {
    const visibleSubset = agents.filter((agent) => agent.session_id !== 'agent-1');
    renderGrid('agent-1', visibleSubset);

    const root = screen.getByTestId('agent-grid');
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

  it('falls back to the remaining active agent in Single mode when focus is off', () => {
    renderGrid('agent-1', agents, vi.fn(), {
      offAgentIds: new Set(['agent-1']),
    });
    const root = screen.getByTestId('agent-grid');

    expect(root.style.gridAutoRows).toBe('100%');
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

  it('keeps terminal panes memoized when only card header state changes', async () => {
    const stableProps = {
      surfaceId: 'overview-surface',
      mode: 'grid' as const,
      filteredAgents: agents,
      telemetry,
      terminalTitles: {},
      selectedAgentIds: new Set<string>(),
      offAgentIds: new Set<string>(),
      focusedAgentId: null,
      draggedAgentId: null,
      dragOverAgentId: null,
      editingAgentId: null,
      tempName: "",
      theme: "dark" as const,
      onMouseEnterCard: vi.fn(),
      onMouseUp: vi.fn(),
      onMouseDown: vi.fn(),
      onCardClick: vi.fn(),
      onModeChange: vi.fn(),
      onExitSingle: vi.fn(),
      onFocusedAgentChange: vi.fn(),
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
    const { rerender } = render(<AgentsOverviewView {...stableProps} />);
    await waitFor(() => {
      const latestProps = new Map<string, { renderState: string }>();
      for (const [props] of terminalRenderSpy.mock.calls) latestProps.set(props.sessionId, props);
      expect(latestProps.get('agent-1')?.renderState).toBe('mounted');
      expect(latestProps.get('agent-2')?.renderState).toBe('mounted');
    });

    terminalRenderSpy.mockClear();
    rerender(
      <AgentsOverviewView
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

    rerender(<AgentsOverviewView {...gridProps('agent-2', agents)} />);

    expect(screen.getByTestId('chat-agent-1')).not.toBeVisible();
    expect(screen.getByTestId('terminal-agent-2')).toBeInTheDocument();

    rerender(<AgentsOverviewView {...gridProps(null, agents)} />);

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

    renderGrid(null, [agents[0]]);
    const root = screen.getByTestId('agent-grid');
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

    renderGrid(null, [agents[0]]);

    const root = screen.getByTestId('agent-grid');
    expect(root.style.gridTemplateColumns).toBe('1fr');
  });

  it('exposes the resolved Overview presentation through a semantic state attribute', () => {
    renderGrid(null, agents);

    const root = screen.getByTestId('agent-grid');
    expect(root).toHaveAttribute('data-overview-mode', 'grid');
    expect(root).toHaveAttribute('data-presentation-mode', 'grid');
  });

  it('keeps the grid wide enough for terminal input rows when the app shell is narrow', () => {
    renderGrid(null, [agents[0]]);

    const root = screen.getByTestId('agent-grid');
    expect(root.style.minWidth).toBe('520px');
  });

  it('clamps explicit Grid tracks to card floors when manual weights are uneven', () => {
    expect(agentsOverviewGridTemplateColumns('grid', [0.2, 0.8], 520)).toBe(
      'minmax(0, 0.2fr) minmax(0, 0.8fr)',
    );
  });

  it('uses persisted column tracks and row height in explicit Grid mode', () => {
    act(() => {
      useLayoutStore.getState().setColumnTracks([0.2, 0.3, 0.5]);
      useLayoutStore.getState().setRowHeight(612);
    });

    renderGrid(null, [...agents, {
      session_id: 'agent-3',
      session_name: 'Gamma',
      agent_class: 'Reviewer',
      folder: '/workspace',
      is_off: false,
    }]);

    const root = screen.getByTestId('agent-grid');
    expect(root.style.gridTemplateColumns).toBe(
      'minmax(0, 0.2fr) minmax(0, 0.3fr) minmax(0, 0.5fr)',
    );
    expect(root.style.gridAutoRows).toBe('612px');
  });

  it('positions later row gutters from grid padding and accumulated gaps', () => {
    act(() => {
      useLayoutStore.getState().setColumnTracks([1]);
      useLayoutStore.getState().setRowHeight(450);
    });
    const thirdAgent = {
      session_id: 'agent-3',
      session_name: 'Gamma',
      agent_class: 'Reviewer',
      folder: '/workspace',
      is_off: false,
    };

    const { container } = renderGrid(null, [...agents, thirdAgent]);
    const handles = container.querySelectorAll<HTMLElement>('[data-resize-handle="v"]');

    expect(screen.getByTestId('agent-grid').style.gap).toBe('6px');
    expect(screen.getByTestId('agent-grid').style.padding).toBe('6px');
    expect(handles).toHaveLength(2);
    expect(handles[0].style.top).toBe('450px');
    expect(handles[1].style.top).toBe('906px');
  });

  it('uses a narrower minimum width for chat cards', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');

    renderGrid(null, [agents[0]]);

    const root = screen.getByTestId('agent-grid');
    expect(root.style.minWidth).toBe('360px');
  });
});

describe('AgentsOverviewView density', () => {
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

describe('AgentsOverviewView stacked mode', () => {
  beforeEach(() => {
    localStorage.clear();
    act(() => useLayoutStore.getState().resetLayout());
  });

  it('renders single column when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    renderGrid(null, agents);
    const grid = screen.getByTestId('agent-grid');
    expect(grid.style.gridTemplateColumns).toBe('1fr');
  });

  it('uses a single-column minimum width when gridStacked is true', () => {
    act(() => useLayoutStore.getState().setGridStacked(true));
    renderGrid(null, agents);
    const grid = screen.getByTestId('agent-grid');
    expect(grid.style.minWidth).toBe('520px');
  });

  it('uses the chat minimum width when gridStacked is true in chat mode', () => {
    useSettingsStore.getState().setGridCardDisplayMode('chat');
    act(() => useLayoutStore.getState().setGridStacked(true));

    renderGrid(null, agents);

    const grid = screen.getByTestId('agent-grid');
    expect(grid.style.minWidth).toBe('360px');
  });

  it('previews the persisted columns during a stack-exit gesture', () => {
    const originalWidth = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 800 });
    act(() => {
      window.dispatchEvent(new Event('resize'));
      useLayoutStore.getState().setGridStacked(true);
    });

    try {
      const { container } = renderGrid(null, agents);
      const grid = screen.getByTestId('agent-grid');

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

  it('positions stacked exit handles from each padded row origin', () => {
    act(() => {
      useLayoutStore.getState().setGridStacked(true);
      useLayoutStore.getState().setRowHeight(450);
    });
    const { container } = renderGrid(null, agents);
    const handles = container.querySelectorAll<HTMLElement>('[data-resize-handle="stack-exit"]');

    expect(handles[0].style.top).toBe('6px');
    expect(handles[1].style.top).toBe('462px');
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
