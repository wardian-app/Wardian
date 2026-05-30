import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { RunControls } from './RunControls';

const base = {
  blueprintId: 'wf',
  runId: 'run-1',
  blueprintPath: '/x/wf.md',
  status: 'running' as const,
  awaitingNode: null,
};

describe('RunControls', () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it('cancels a running run', async () => {
    invokeMock.mockResolvedValueOnce({ ok: true });

    render(<RunControls {...base} onChanged={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /cancel/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('workflow_cancel_v2', { blueprintId: 'wf', runId: 'run-1' });
    });
  });

  it('shows resume for an interrupted run', () => {
    render(<RunControls {...base} status="interrupted" onChanged={() => {}} />);

    expect(screen.getByRole('button', { name: /resume/i })).toBeInTheDocument();
  });

  it('shows approve/reject when awaiting approval', async () => {
    invokeMock.mockResolvedValueOnce({ ok: true });

    render(<RunControls {...base} status="awaiting_approval" awaitingNode="gate" onChanged={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /approve/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        'workflow_approve_v2',
        expect.objectContaining({ blueprintId: 'wf', runId: 'run-1', node: 'gate', granted: true }),
      );
    });
  });
});
