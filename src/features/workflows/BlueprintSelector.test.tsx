import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { BlueprintSelector } from './BlueprintSelector';

describe('BlueprintSelector', () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it('lists blueprints from workflow_list_blueprints and opens one', async () => {
    invokeMock.mockResolvedValueOnce([{ id: 'wf', name: 'WF', path: '/x/wf.md' }]);
    const onOpen = vi.fn();

    render(<BlueprintSelector onOpen={onOpen} onNew={() => {}} />);

    await waitFor(() => expect(screen.getByText('WF')).toBeInTheDocument());
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '/x/wf.md' } });

    expect(onOpen).toHaveBeenCalledWith('/x/wf.md');
  });

  it('fires onNew', async () => {
    invokeMock.mockResolvedValueOnce([]);
    const onNew = vi.fn();

    render(<BlueprintSelector onOpen={() => {}} onNew={onNew} />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('workflow_list_blueprints'));
    fireEvent.click(screen.getByRole('button', { name: /new/i }));

    expect(onNew).toHaveBeenCalled();
  });
});
