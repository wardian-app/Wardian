import { blueprintPage } from "../../test/pageFixtures";
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

  it('lists blueprints from automation_list_blueprints and opens one', async () => {
    invokeMock.mockResolvedValueOnce(blueprintPage([{ id: 'wf', name: 'WF', path: '/x/wf.md' }]));
    const onOpen = vi.fn();

    render(<BlueprintSelector onOpen={onOpen} onNew={() => {}} />);

    await waitFor(() => expect(screen.getByText('WF')).toBeInTheDocument());
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '/x/wf.md' } });

    expect(onOpen).toHaveBeenCalledWith('/x/wf.md');
  });

  it('fires onNew', async () => {
    invokeMock.mockResolvedValueOnce(blueprintPage([]));
    const onNew = vi.fn();

    render(<BlueprintSelector onOpen={() => {}} onNew={onNew} />);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('automation_list_blueprints'));
    fireEvent.click(screen.getByRole('button', { name: /new/i }));

    expect(onNew).toHaveBeenCalled();
  });

  it('shows only workflows assigned to selected agents while preserving the open workflow', async () => {
    invokeMock.mockResolvedValueOnce(blueprintPage([
      { id: 'selected', name: 'Selected workflow', path: '/x/selected.md' },
      { id: 'other', name: 'Other workflow', path: '/x/other.md' },
      { id: 'open', name: 'Open workflow', path: '/x/open.md' },
    ]));

    render(
      <BlueprintSelector
        selectedPath="/x/open.md"
        visibleBlueprintIds={new Set(['selected'])}
        onOpen={() => {}}
        onNew={() => {}}
      />,
    );

    await waitFor(() => expect(screen.getByText('Selected workflow')).toBeInTheDocument());
    expect(screen.getByText('Open workflow')).toBeInTheDocument();
    expect(screen.queryByText('Other workflow')).toBeNull();
  });

  it('explains when no saved workflow belongs to the selected agents', async () => {
    invokeMock.mockResolvedValueOnce(blueprintPage([
      { id: 'other', name: 'Other workflow', path: '/x/other.md' },
    ]));

    render(
      <BlueprintSelector
        visibleBlueprintIds={new Set()}
        onOpen={() => {}}
        onNew={() => {}}
      />,
    );

    expect(await screen.findByText('No workflows for selected agents')).toBeInTheDocument();
    expect(screen.queryByText('Other workflow')).toBeNull();
  });

  it('marks a partial automation catalog', async () => {
    invokeMock.mockResolvedValueOnce({
      blueprints: [{ id: 'wf', name: 'WF', path: '/x/wf.md' }],
      truncated: true,
    });

    render(<BlueprintSelector onOpen={() => {}} onNew={() => {}} />);

    expect(await screen.findByRole('status')).toHaveTextContent('first 500');
  });

  it('loads one more bounded catalog page', async () => {
    invokeMock
      .mockResolvedValueOnce({ blueprints: [{ id: 'wf-1', name: 'WF 1', path: '/x/1.md' }], truncated: true, next_offset: 500 })
      .mockResolvedValueOnce({ blueprints: [{ id: 'wf-2', name: 'WF 2', path: '/x/2.md' }], truncated: false, next_offset: null });

    render(<BlueprintSelector onOpen={() => {}} onNew={() => {}} />);
    fireEvent.click(await screen.findByRole('button', { name: /load next 500/i }));

    await waitFor(() => expect(screen.getByText('WF 2')).toBeInTheDocument());
    expect(invokeMock).toHaveBeenLastCalledWith('automation_list_blueprints', { offset: 500 });
  });
});
