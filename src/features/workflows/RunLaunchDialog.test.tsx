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
      if (command === 'workflow_run_v2') {
        return { ok: true, run_id: 'run-9', blueprint_id: 'wf', run_dir: '/r' };
      }
      return null;
    });
  });

  it('prefills provider from settings and launches via workflow_run_v2', async () => {
    const onLaunched = vi.fn();

    render(<RunLaunchDialog path="/x/wf.md" onLaunched={onLaunched} onCancel={() => {}} />);

    await waitFor(() => {
      expect(screen.getByLabelText(/provider/i)).toHaveValue('codex');
    });
    fireEvent.click(screen.getByRole('button', { name: /^run$/i }));

    await waitFor(() => expect(onLaunched).toHaveBeenCalledWith('run-9'));
    expect(invokeMock).toHaveBeenCalledWith(
      'workflow_run_v2',
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
      'workflow_run_v2',
      expect.objectContaining({ path: '/x/wf.md', input: { symbol: 'SPY' } }),
    ));
  });
});
