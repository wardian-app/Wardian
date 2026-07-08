import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MarkdownEditor } from './MarkdownEditor';

describe('MarkdownEditor', () => {
  it('fires onSave when Ctrl+S is pressed in the textarea', () => {
    const onSave = vi.fn();
    render(
      <MarkdownEditor
        value="body"
        onChange={vi.fn()}
        onSave={onSave}
        dirty={false}
        stale={false}
        onReloadExternal={vi.fn()}
      />,
    );

    fireEvent.keyDown(screen.getByTestId('markdown-editor-textarea'), { key: 's', ctrlKey: true });
    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it('fires onSave when Cmd+S (metaKey) is pressed', () => {
    const onSave = vi.fn();
    render(
      <MarkdownEditor
        value="body"
        onChange={vi.fn()}
        onSave={onSave}
        dirty={false}
        stale={false}
        onReloadExternal={vi.fn()}
      />,
    );

    fireEvent.keyDown(screen.getByTestId('markdown-editor-textarea'), { key: 's', metaKey: true });
    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it('does not fire onSave for a plain "s" keypress', () => {
    const onSave = vi.fn();
    render(
      <MarkdownEditor
        value="body"
        onChange={vi.fn()}
        onSave={onSave}
        dirty={false}
        stale={false}
        onReloadExternal={vi.fn()}
      />,
    );

    fireEvent.keyDown(screen.getByTestId('markdown-editor-textarea'), { key: 's' });
    expect(onSave).not.toHaveBeenCalled();
  });

  it('shows a dirty dot when dirty is true and not when false', () => {
    const { rerender } = render(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty onReloadExternal={vi.fn()} stale={false} />,
    );
    expect(screen.getByTestId('markdown-editor-dirty-dot')).toBeInTheDocument();

    rerender(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} onReloadExternal={vi.fn()} stale={false} />,
    );
    expect(screen.queryByTestId('markdown-editor-dirty-dot')).not.toBeInTheDocument();
  });

  it('calls onChange with the new textarea value', () => {
    const onChange = vi.fn();
    render(
      <MarkdownEditor value="body" onChange={onChange} onSave={vi.fn()} dirty={false} stale={false} onReloadExternal={vi.fn()} />,
    );

    fireEvent.change(screen.getByTestId('markdown-editor-textarea'), { target: { value: 'new body' } });
    expect(onChange).toHaveBeenCalledWith('new body');
  });

  it('renders the stale conflict bar and calls onReloadExternal from Reload', () => {
    const onReloadExternal = vi.fn();
    render(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale onReloadExternal={onReloadExternal} />,
    );

    expect(screen.getByTestId('markdown-editor-stale-bar')).toHaveTextContent('File changed on disk');
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
    expect(onReloadExternal).toHaveBeenCalledTimes(1);
  });

  it('does not render the stale bar when stale is false', () => {
    render(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale={false} onReloadExternal={vi.fn()} />,
    );
    expect(screen.queryByTestId('markdown-editor-stale-bar')).not.toBeInTheDocument();
  });

  it('"Keep mine" dismisses the stale bar without calling onReloadExternal', () => {
    const onReloadExternal = vi.fn();
    render(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale onReloadExternal={onReloadExternal} />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Keep mine' }));
    expect(screen.queryByTestId('markdown-editor-stale-bar')).not.toBeInTheDocument();
    expect(onReloadExternal).not.toHaveBeenCalled();
  });

  it('re-shows the stale bar if a new external change arrives after "Keep mine"', () => {
    const { rerender } = render(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale onReloadExternal={vi.fn()} />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Keep mine' }));
    expect(screen.queryByTestId('markdown-editor-stale-bar')).not.toBeInTheDocument();

    rerender(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale={false} onReloadExternal={vi.fn()} />,
    );
    rerender(
      <MarkdownEditor value="body" onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale onReloadExternal={vi.fn()} />,
    );
    expect(screen.getByTestId('markdown-editor-stale-bar')).toBeInTheDocument();
  });
});
