import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn());
const settingsState = vi.hoisted(() => ({ default_provider: 'codex' }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock('../../store/useSettingsStore', () => ({
  useSettingsStore: <T,>(selector: (state: typeof settingsState) => T) => selector(settingsState),
}));

import { RunLaunchDialog } from './RunLaunchDialog';
import type { Blueprint } from './builder/blueprintTypes';
import type { WorkflowSchedule } from '../../types/workflow';

const providerReadiness = [
  { provider: 'claude', display_name: 'Claude', available: true, executable: 'claude', reason: null },
  { provider: 'codex', display_name: 'Codex', available: true, executable: 'codex', reason: null },
  { provider: 'gemini', display_name: 'Gemini', available: true, executable: 'gemini', reason: null },
  { provider: 'antigravity', display_name: 'Antigravity', available: true, executable: 'antigravity', reason: null },
  { provider: 'opencode', display_name: 'OpenCode', available: true, executable: 'opencode', reason: null },
];

describe('RunLaunchDialog', () => {
  beforeEach(() => {
    settingsState.default_provider = 'codex';
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'list_provider_readiness') return providerReadiness;
      if (command === 'list_agents') {
        return [
          {
            session_id: 'agent-1',
            session_name: 'Assistant',
            agent_class: 'Personal Assistant',
            folder: '/assistant',
            is_off: false,
            provider: 'gemini',
          },
          {
            session_id: 'agent-2',
            session_name: 'Offline Worker',
            agent_class: 'Worker',
            folder: '/offline',
            is_off: true,
            provider: 'codex',
          },
        ];
      }
      if (command === 'workflow_run') {
        return { ok: true, run_id: 'run-9', blueprint_id: 'wf', run_dir: '/r' };
      }
      return null;
    });
  });

  it('prefills provider from settings and launches via workflow_run', async () => {
    const onLaunched = vi.fn();

    render(<RunLaunchDialog path="/x/wf.md" onLaunched={onLaunched} onCancel={() => {}} />);

    await waitFor(() => {
      expect(screen.getByLabelText(/provider/i)).toHaveValue('codex');
    });
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(onLaunched).toHaveBeenCalledWith('run-9'));
    expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run',
      expect.objectContaining({ path: '/x/wf.md', provider: 'codex' }),
    );
  });

  it('renders params from the entry input schema and passes them as input', async () => {
    render(
      <RunLaunchDialog
        path="/x/wf.md"
        inputParams={[{ name: 'symbol', type: 'string' }]}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByLabelText(/provider/i)).toHaveValue('codex');
    });
    fireEvent.change(screen.getByLabelText(/symbol/i), { target: { value: 'SPY' } });
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run',
      expect.objectContaining({ path: '/x/wf.md', input: { symbol: 'SPY' } }),
    ));
  });

  it('schedules via schedule_create when toggled to Schedule', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'list_provider_readiness') return providerReadiness;
      if (command === 'schedule_create') return { id: 's1' };
      return null;
    });
    const onScheduled = vi.fn();

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprintId="wf"
        onLaunched={() => {}}
        onCancel={() => {}}
        onScheduled={onScheduled}
      />,
    );

    await waitFor(() => expect(screen.getByLabelText(/provider/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('radio', { name: /schedule/i }));
    fireEvent.change(screen.getByLabelText(/schedule name/i), { target: { value: 'Nightly' } });
    fireEvent.click(screen.getByRole('button', { name: /save schedule/i }));

    await waitFor(() => expect(onScheduled).toHaveBeenCalled());
    expect(invokeMock).toHaveBeenCalledWith(
      'schedule_create',
      expect.objectContaining({ blueprintId: 'wf', name: 'Nightly' }),
    );
  });

  it('keeps long workflow forms inside a scrollable viewport-bounded dialog', async () => {
    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprintId="wf"
        inputParams={Array.from({ length: 24 }, (_, index) => ({
          name: `param_${index + 1}`,
          type: 'string',
        }))}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByLabelText(/provider/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('radio', { name: /schedule/i }));

    expect(screen.getByTestId('run-launch-dialog')).toHaveClass(
      'flex',
      'max-h-[min(calc(100vh-4rem),100%)]',
      'overflow-hidden',
    );
    expect(screen.getByTestId('run-launch-dialog-body')).toHaveClass(
      'min-h-0',
      'overflow-y-auto',
    );
    expect(screen.getByTestId('run-launch-dialog-actions')).toHaveClass('shrink-0');
    expect(screen.getByLabelText(/param_24/i)).toBeInTheDocument();
  });

  it('preserves provider and input when editing an existing schedule', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'list_provider_readiness') return providerReadiness;
      if (command === 'schedule_remove') return null;
      if (command === 'schedule_create') return { id: 's2' };
      return null;
    });
    const editSchedule: WorkflowSchedule = {
      id: 's1',
      blueprint_id: 'wf',
      name: 'Nightly',
      provider: 'claude',
      workspace: null,
      input: { symbol: 'IBM' },
      bindings: { planner: 'agent-1' },
      schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
      is_paused: false,
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprintId="wf"
        inputParams={[{ name: 'symbol', type: 'string' }]}
        editSchedule={editSchedule}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByLabelText(/provider/i)).toHaveValue('claude'));
    expect(screen.getByLabelText(/symbol/i)).toHaveValue('IBM');
    fireEvent.click(screen.getByRole('button', { name: /save schedule/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('schedule_remove', { id: 's1' }));
    expect(invokeMock).toHaveBeenCalledWith(
      'schedule_create',
      expect.objectContaining({
        blueprintId: 'wf',
        provider: 'claude',
        input: { symbol: 'IBM' },
        bindings: { planner: 'agent-1' },
      }),
    );
  });

  it('lets a workflow role bind to an active agent when launched', async () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'heartbeat', type: 'task', fields: { agent: 'role:reasoning_gate', prompt: 'Check in.' } },
      ],
      edges: [],
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprint={blueprint}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByRole('button', { name: /change reasoning_gate assignment/i })).toBeInTheDocument());
    expect(screen.queryByText(/fresh agent defaults/i)).toBeNull();
    expect(screen.queryByLabelText(/provider for fresh agents/i)).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /change reasoning_gate assignment/i }));
    expect(screen.getByRole('button', { name: /offline worker/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /assistant/i }));
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run',
      expect.objectContaining({
        path: '/x/wf.md',
        bindings: { reasoning_gate: 'agent-1' },
        assignments: {
          reasoning_gate: {
            target_type: 'agent',
            agent_id: 'agent-1',
            conversation: 'current',
            busy_policy: 'fail',
          },
        },
      }),
    ));
  });

  it('uses a searchable picker for large agent rosters instead of rendering every agent inline', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'list_provider_readiness') return providerReadiness;
      if (command === 'list_agents') {
        return Array.from({ length: 36 }, (_, index) => ({
          session_id: `agent-${index + 1}`,
          session_name: `Agent ${index + 1}`,
          agent_class: index % 2 === 0 ? 'Coder' : 'Researcher',
          folder: `/workspace/${index + 1}`,
          is_off: false,
          provider: index % 2 === 0 ? 'codex' : 'gemini',
        }));
      }
      if (command === 'workflow_run') {
        return { ok: true, run_id: 'run-9', blueprint_id: 'wf', run_dir: '/r' };
      }
      return null;
    });
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'heartbeat', type: 'task', fields: { agent: 'role:reasoning_gate', prompt: 'Check in.' } },
      ],
      edges: [],
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprint={blueprint}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByRole('button', { name: /change reasoning_gate assignment/i })).toBeInTheDocument());
    expect(screen.queryByText(/Agent 36/)).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /change reasoning_gate assignment/i }));
    expect(screen.getByRole('searchbox', { name: /search agents/i })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Agent 36/i })).toBeNull();

    fireEvent.change(screen.getByRole('searchbox', { name: /search agents/i }), { target: { value: 'Agent 36' } });
    fireEvent.click(await screen.findByRole('button', { name: /Agent 36/i }));
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run',
      expect.objectContaining({
        assignments: expect.objectContaining({
          reasoning_gate: expect.objectContaining({
            target_type: 'agent',
            agent_id: 'agent-36',
          }),
        }),
      }),
    ));
  });

  it('wraps temporary provider choices inside the role assignment picker', async () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'heartbeat', type: 'task', fields: { agent: 'role:reasoning_gate', prompt: 'Check in.' } },
      ],
      edges: [],
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprint={blueprint}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByRole('button', { name: /change reasoning_gate assignment/i })).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /change reasoning_gate assignment/i }));

    const temporaryOptions = screen.getByTestId('temporary-provider-options');
    expect(temporaryOptions).toHaveClass('flex-wrap');
    expect(screen.getByRole('button', { name: /new temporary antigravity/i })).toHaveClass('max-w-full');
  });

  it('renders the role assignment picker in flow so scrollable dialogs do not clip it', async () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'heartbeat', type: 'task', fields: { agent: 'role:reasoning_gate', prompt: 'Check in.' } },
      ],
      edges: [],
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprint={blueprint}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByRole('button', { name: /change reasoning_gate assignment/i })).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /change reasoning_gate assignment/i }));

    const picker = screen.getByRole('searchbox', { name: /search agents/i }).parentElement;
    expect(picker).toHaveClass('mt-2');
    expect(picker).not.toHaveClass('absolute');
  });

  it('lets each workflow role choose its own fresh provider', async () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'wf',
      name: 'Workflow',
      nodes: [
        { id: 'plan', type: 'task', fields: { agent: 'role:planner', prompt: 'Plan.' } },
        { id: 'build', type: 'task', fields: { agent: 'role:builder', prompt: 'Build.' } },
      ],
      edges: [],
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprint={blueprint}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByText(/planner/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /change planner assignment/i }));
    fireEvent.click(screen.getByRole('button', { name: /new temporary claude/i }));
    fireEvent.click(screen.getByRole('button', { name: /change builder assignment/i }));
    fireEvent.click(screen.getByRole('button', { name: /new temporary gemini/i }));
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run',
      expect.objectContaining({
        path: '/x/wf.md',
        bindings: { planner: 'claude', builder: 'gemini' },
      }),
    ));
  });

  it('hides provider and workspace controls for workflows with no provider-backed or workspace-backed nodes', async () => {
    const blueprint: Blueprint = {
      schema: 2,
      id: 'loop-test',
      name: 'Loop Test',
      nodes: [
        { id: 'trigger-1', type: 'manual_trigger', fields: {} },
        { id: 'loop-1', type: 'loop', fields: { max_iterations: 3 } },
        { id: 'shell-1', type: 'shell', parent: 'loop-1', fields: { command: 'echo hi', cwd: '/tmp' } },
        { id: 'notify-1', type: 'notify', fields: { message: 'done' } },
      ],
      edges: [],
    };

    render(
      <RunLaunchDialog
        path="/x/wf.md"
        blueprint={blueprint}
        onLaunched={() => {}}
        onCancel={() => {}}
      />,
    );

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('list_provider_readiness'));
    expect(screen.queryByLabelText(/provider/i)).toBeNull();
    expect(screen.queryByLabelText(/workspace/i)).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run',
      expect.not.objectContaining({
        provider: expect.anything(),
        workspace: expect.anything(),
      }),
    ));
  });
});
