import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/react';
import { SidebarResizeHandle } from './SidebarResizeHandle';

describe('SidebarResizeHandle', () => {
  it('emits cumulative width during pointer drag', () => {
    const onResize = vi.fn();
    render(<SidebarResizeHandle baseWidth={260} edge="right" onResize={onResize} onReset={() => {}} />);
    const handle = screen.getByTestId('sidebar-resize-handle');

    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 140 });
    expect(onResize).toHaveBeenLastCalledWith(300); // 260 + 40

    fireEvent.pointerUp(window, { clientX: 140 });
  });

  it('inverts delta when edge="left"', () => {
    const onResize = vi.fn();
    render(<SidebarResizeHandle baseWidth={260} edge="left" onResize={onResize} onReset={() => {}} />);
    const handle = screen.getByTestId('sidebar-resize-handle');

    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 60 });
    expect(onResize).toHaveBeenLastCalledWith(300); // 260 + (100 - 60)

    fireEvent.pointerUp(window, { clientX: 60 });
  });

  it('calls onReset on double click', () => {
    const onReset = vi.fn();
    render(<SidebarResizeHandle baseWidth={260} edge="right" onResize={() => {}} onReset={onReset} />);
    fireEvent.doubleClick(screen.getByTestId('sidebar-resize-handle'));
    expect(onReset).toHaveBeenCalledOnce();
  });
});
