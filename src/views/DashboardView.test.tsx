import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentProps } from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { AgentConfig, AgentTelemetry } from '../types';
import { DashboardView } from './DashboardView';

const agent = (overrides: Partial<AgentConfig> = {}): AgentConfig => ({
  session_id: 'agent-1',
  session_name: 'Coder One',
  agent_class: 'Coder',
  folder: 'C:/work/app',
  is_off: false,
  provider: 'mock',
  ...overrides,
});

const telemetry = (overrides: Partial<AgentTelemetry> = {}): AgentTelemetry => ({
  session_id: 'agent-1',
  cpu_usage: 12.34,
  memory_mb: 256.7,
  uptime_seconds: 30,
  query_count: 4,
  init_timestamp: '2026-05-04T10:15:00Z',
  current_status: 'Processing...',
  log_path: null,
  ...overrides,
});

const renderDashboard = (props: Partial<ComponentProps<typeof DashboardView>> = {}) => {
  const defaults: ComponentProps<typeof DashboardView> = {
    filteredAgents: [agent()],
    telemetry: { 'agent-1': telemetry() },
    terminalTitles: { 'agent-1': 'Running tests' },
    currentThoughts: { 'agent-1': 'Validating changes' },
    selectedAgentIds: new Set(),
    offAgentIds: new Set(),
    draggedAgentId: null,
    dragOverAgentId: null,
    onMouseEnterCard: vi.fn(),
    onMouseUp: vi.fn(),
    onMouseDown: vi.fn(),
    onCardClick: vi.fn(),
    onPause: vi.fn(),
    onRestart: vi.fn(),
    onDelete: vi.fn(),
    onQuery: vi.fn(),
    deriveCurrentThought: vi.fn(() => ({ thought: 'Derived thought', status: 'Processing...' })),
    getStatusColorClass: vi.fn(() => 'bg-wardian-processing'),
  };

  const merged = { ...defaults, ...props };
  render(<DashboardView {...merged} />);
  return merged;
};

describe('DashboardView', () => {
  it('renders active agent telemetry and derived status', () => {
    const props = renderDashboard();

    expect(screen.getByText('Coder One')).toBeInTheDocument();
    expect(screen.getByText('Coder')).toBeInTheDocument();
    expect(screen.getByText('12.3% CPU')).toBeInTheDocument();
    expect(screen.getByText('257 MB')).toBeInTheDocument();
    expect(screen.getByText('4')).toBeInTheDocument();
    expect(screen.getByText('Derived thought')).toBeInTheDocument();
    expect(props.deriveCurrentThought).toHaveBeenCalledWith(
      'Running tests',
      'Validating changes',
      props.telemetry['agent-1'],
      false,
    );
    expect(props.getStatusColorClass).toHaveBeenCalledWith('Processing...');
  });

  it('exposes card regions for pane-local responsive layout', () => {
    renderDashboard();

    const card = document.getElementById('agent-card-agent-1')!;
    expect(card).toHaveClass('dashboard-agent-card');
    expect(card.querySelector('.dashboard-agent-card__identity')).toBeInTheDocument();
    expect(card.querySelector('.dashboard-agent-card__metadata')).toBeInTheDocument();
    expect(card.querySelector('.dashboard-agent-card__actions')).toBeInTheDocument();
  });

  it('omits off agents and shows the empty state when no agents are filtered in', () => {
    renderDashboard({
      filteredAgents: [agent({ session_id: 'off-1', session_name: 'Off Agent' })],
      telemetry: {},
      terminalTitles: {},
      currentThoughts: {},
      offAgentIds: new Set(['off-1']),
    });
    expect(screen.queryByText('Off Agent')).not.toBeInTheDocument();

    renderDashboard({ filteredAgents: [] });
    expect(screen.getByText('No Active Instances')).toBeInTheDocument();
  });

  it('routes card and control interactions to agent callbacks', async () => {
    const user = userEvent.setup();
    const props = renderDashboard();

    fireEvent.mouseEnter(document.getElementById('agent-card-agent-1')!);
    expect(props.onMouseEnterCard).toHaveBeenCalledWith('agent-1');

    await user.click(screen.getByText('Coder One'));
    expect(props.onCardClick).toHaveBeenCalledWith(expect.any(Object), 'agent-1');

    await user.click(screen.getByRole('button', { name: 'Pause' }));
    expect(props.onPause).toHaveBeenCalledWith('agent-1');

    await user.click(screen.getByRole('button', { name: 'Restart' }));
    expect(props.onRestart).toHaveBeenCalledWith('agent-1');

    await user.click(screen.getByRole('button', { name: 'Delete' }));
    expect(props.onDelete).toHaveBeenCalledWith('agent-1');
  });

  it('runs canned queries and resets the query select', () => {
    const props = renderDashboard();
    const querySelect = screen.getByRole('combobox');

    fireEvent.change(querySelect, { target: { value: 'Validate your recent changes and run tests.' } });

    expect(props.onQuery).toHaveBeenCalledWith('agent-1', 'Validate your recent changes and run tests.');
    expect(querySelect).toHaveValue('');
  });

});
