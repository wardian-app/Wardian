import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenu } from './ContextMenu';

describe('ContextMenu', () => {
  it('runs item actions and closes the menu', async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    const onClose = vi.fn();

    render(
      <ContextMenu
        x={20}
        y={30}
        onClose={onClose}
        items={[
          { label: 'Copy', onClick: onAction },
          { divider: true },
          { label: 'Delete', danger: true, onClick: onAction },
        ]}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Copy' }));

    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('opens submenus and closes on outside interactions', async () => {
    const user = userEvent.setup();
    const subAction = vi.fn();
    const onClose = vi.fn();

    render(
      <ContextMenu
        x={window.innerWidth + 200}
        y={window.innerHeight + 200}
        onClose={onClose}
        items={[
          {
            label: 'Insert Variable',
            subItems: [{ label: 'Base Output', onClick: subAction }],
          },
        ]}
      />,
    );

    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Insert Variable' }).parentElement!);
    await user.click(await screen.findByRole('button', { name: 'Base Output' }));
    expect(subAction).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.mouseDown(document.body);
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
