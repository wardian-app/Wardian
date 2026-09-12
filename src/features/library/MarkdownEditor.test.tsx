import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MarkdownEditor } from './MarkdownEditor';

describe('MarkdownEditor', () => {
  it('previews Markdown by default and preserves the exact source when switching modes', () => {
    const value = '---\nname: Example\n---\n# Instructions\n\n| Step | Result |\n| --- | --- |\n| Inspect | Evidence |\n\n<details><summary>More</summary>Context</details>\n\n<script>unsafe()</script>';
    render(<MarkdownEditor value={value} onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale={false} onReloadExternal={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'Instructions' })).toBeVisible();
    expect(screen.getByRole('table')).toBeVisible();
    expect(screen.getByText('Document metadata')).toBeVisible();
    expect(screen.getByText('More')).toBeVisible();
    expect(screen.queryByTestId('markdown-editor-textarea')).not.toBeInTheDocument();
    expect(screen.getByTestId('library-markdown-preview').querySelector('script')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    expect(screen.getByRole('textbox', { name: 'Markdown source' })).toHaveValue(value);
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(screen.getByRole('heading', { name: 'Instructions' })).toBeVisible();
  });

  it('saves from Preview and blocks both save controls until a stale conflict is resolved', () => {
    const onSave = vi.fn();
    const props = { value: '# Draft', onChange: vi.fn(), onSave, dirty: true, onReloadExternal: vi.fn() };
    const { rerender } = render(<MarkdownEditor {...props} stale={false} />);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(onSave).toHaveBeenCalledOnce();
    rerender(<MarkdownEditor {...props} stale />);
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    fireEvent.keyDown(screen.getByTestId('markdown-editor'), { key: 's', ctrlKey: true });
    expect(onSave).toHaveBeenCalledOnce();
  });

  it('keeps local references inert and removes unsafe links and image sources', () => {
    render(<MarkdownEditor value={'[Local](notes.md)\n\n[Unsafe](javascript:alert)\n\n[Website](https://example.com)\n\n![Diagram](diagram.png)'} onChange={vi.fn()} onSave={vi.fn()} dirty={false} stale={false} onReloadExternal={vi.fn()} />);
    expect(screen.queryByRole('link', { name: 'Local' })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Unsafe' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Website' })).toHaveAttribute('href', 'https://example.com/');
    expect(screen.getByTestId('library-markdown-preview').querySelector('img')).toBeNull();
  });

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

    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
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

    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
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

    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
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

    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
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
