import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { ContextMenu } from './ContextMenu';
import { clampContextMenuPosition } from './useContextMenuSurface';

describe('ContextMenu', () => {
  it('keeps the measured menu within the viewport without moving it away from the cursor unnecessarily', () => {
    expect(clampContextMenuPosition(
      { x: 100, y: 120 },
      { width: 200, height: 150 },
      { width: 800, height: 600 },
    )).toEqual({ x: 100, y: 120 });

    expect(clampContextMenuPosition(
      { x: 790, y: 590 },
      { width: 200, height: 150 },
      { width: 800, height: 600 },
    )).toEqual({ x: 592, y: 442 });
  });

  it('closes an already-open context menu before another menu remains visible', async () => {
    const user = userEvent.setup();
    const MenuHarness = () => {
      const [firstOpen, setFirstOpen] = useState(true);
      const [secondOpen, setSecondOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setSecondOpen(true)}>Open second menu</button>
          {firstOpen && <ContextMenu x={20} y={30} items={[{ label: 'First action' }]} onClose={() => setFirstOpen(false)} />}
          {secondOpen && <ContextMenu x={40} y={50} items={[{ label: 'Second action' }]} onClose={() => setSecondOpen(false)} />}
        </>
      );
    };

    render(<MenuHarness />);
    await user.click(screen.getByRole('button', { name: 'Open second menu' }));

    await waitFor(() => expect(screen.queryByRole('button', { name: 'First action' })).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Second action' })).toBeInTheDocument();
  });

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
