import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { NodeLibrary } from './NodeLibrary';

describe('NodeLibrary', () => {
  it('filters registry nodes by search text and adds the selected definition', () => {
    const onAdd = vi.fn();
    render(<NodeLibrary mode="panel" onAdd={onAdd} />);

    fireEvent.change(screen.getByRole('searchbox', { name: /search nodes/i }), {
      target: { value: 'shell' },
    });

    expect(screen.getByRole('button', { name: /shell/i })).toBeVisible();
    expect(screen.queryByRole('button', { name: /manual trigger/i })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /shell/i }));

    expect(onAdd).toHaveBeenCalledWith(expect.objectContaining({ id: 'shell' }));
  });

  it('renders rich registry details for node discovery', () => {
    render(<NodeLibrary mode="panel" onAdd={() => {}} />);

    const task = screen.getByRole('button', { name: /task/i });
    expect(within(task).getByText(/Delegate work to an agent/i)).toBeVisible();
    expect(within(task).getByText(/Requires Agent, Prompt/i)).toBeVisible();
    expect(within(task).queryByText(/In In/i)).toBeNull();
    expect(within(task).queryByText(/Out Out/i)).toBeNull();

    const branch = cardByTitle('Branch');
    expect(within(branch).getByText(/Routes True, False/i)).toBeVisible();

    const trigger = cardByTitle('Manual Trigger');
    expect(within(trigger).getByText(/Starts workflow/i)).toBeVisible();
  });
});

function cardByTitle(title: string): HTMLButtonElement {
  const button = screen.getByText(title).closest('button');
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Expected ${title} to be rendered inside a button`);
  }
  return button;
}
