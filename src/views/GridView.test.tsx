import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { GridView } from './GridView';
import type { AgentConfig, AgentTelemetry } from '../types';

vi.mock('../features/terminal/AgentTerminal', () => ({
  AgentTerminal: ({ sessionId }: { sessionId: string }) => <div data-testid={`terminal-${sessionId}`}>Terminal {sessionId}</div>,
}));

const agents: AgentConfig[] = [
  { session_id: 'agent-1', session_name: 'Alpha', agent_class: 'Coder', folder: 'C:/project', is_off: false },
  { session_id: 'agent-2', session_name: 'Beta', agent_class: 'Architect', folder: 'C:/project', is_off: false },
];

const telemetry: Record<string, AgentTelemetry> = {};

function renderGrid(maximizedAgentId: string | null) {
  return render(
    <GridView
      filteredAgents={agents}
      telemetry={telemetry}
      terminalTitles={{}}
      currentThoughts={{}}
      selectedAgentIds={new Set()}
      offAgentIds={new Set()}
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
    />
  );
}

describe('GridView maximize behavior', () => {
  it('keeps maximized terminals scoped to the grid container', () => {
    const { container } = renderGrid('agent-1');

    const root = container.firstElementChild;
    expect(root?.className).toContain('relative');

    const card = screen.getByTestId('terminal-agent-1').closest('#agent-card-agent-1');
    expect(card?.className).toContain('absolute');
    expect(card?.className).toContain('inset-0');
    expect(card?.className).not.toContain('fixed');
    expect(card?.className).not.toContain('h-screen');
    expect(card?.className).not.toContain('w-screen');
  });
});
