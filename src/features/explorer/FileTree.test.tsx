import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { FileTree } from './FileTree';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

describe('FileTree Component', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders a directory tree and handles folder expansion', async () => {
    const mockNodes = [
      { name: 'folderA', path: '/test/folderA', is_dir: true, extension: null },
      { name: 'fileB.txt', path: '/test/fileB.txt', is_dir: false, extension: 'txt' }
    ];

    vi.mocked(invoke).mockResolvedValueOnce(mockNodes);

    render(<FileTree path="/test" />);

    await waitFor(() => {
      expect(screen.getByText('folderA')).toBeInTheDocument();
      expect(screen.getByText('fileB.txt')).toBeInTheDocument();
    });

    // Mock the subsequent invoke for expanding the folder
    vi.mocked(invoke).mockResolvedValueOnce([{
      name: 'subfile.js', path: '/test/folderA/subfile.js', is_dir: false, extension: 'js'
    }]);

    const folderEl = screen.getByText('folderA');
    await userEvent.click(folderEl);

    // Verify subfile loads
    await waitFor(() => {
      expect(screen.getByText('subfile.js')).toBeInTheDocument();
    });
  });

  it('calls onContextMenu when an item is right-clicked', async () => {
    const mockNodes = [
      { name: 'fileC.png', path: '/test/fileC.png', is_dir: false, extension: 'png' }
    ];

    vi.mocked(invoke).mockResolvedValueOnce(mockNodes);

    const contextMenuSpy = vi.fn();
    render(<FileTree path="/test" onContextMenu={contextMenuSpy} />);

    await waitFor(() => {
      expect(screen.getByText('fileC.png')).toBeInTheDocument();
    });

    const fileEl = screen.getByText('fileC.png').closest('div');
    expect(fileEl).not.toBeNull();
    
    // Using FireEvent context menu or pointer event
    await userEvent.pointer({ keys: '[MouseRight]', target: fileEl as Element });
    
    expect(contextMenuSpy).toHaveBeenCalledTimes(1);
    expect(contextMenuSpy).toHaveBeenCalledWith(expect.anything(), mockNodes[0]);
  });

  it('delays file selection so a single click can become a transient preview', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]);
    const onSelect = vi.fn();
    render(<FileTree path="/test" onSelect={onSelect} />);

    const file = await screen.findByText('notes.md');
    vi.useFakeTimers();
    try {
      fireEvent.click(file, { detail: 1 });
      expect(onSelect).not.toHaveBeenCalled();

      act(() => vi.advanceTimersByTime(250));
      expect(onSelect).toHaveBeenCalledOnce();
      expect(onSelect).toHaveBeenCalledWith('/test/notes.md', false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('cancels delayed selection when the same file is double-clicked open', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]);
    const onSelect = vi.fn();
    const onOpen = vi.fn();
    render(<FileTree path="/test" onSelect={onSelect} onOpen={onOpen} />);

    const row = (await screen.findByText('notes.md')).closest('[role="treeitem"]');
    expect(row).not.toBeNull();
    vi.useFakeTimers();
    try {
      fireEvent.click(row as HTMLElement, { detail: 1 });
      fireEvent.click(row as HTMLElement, { detail: 2 });
      fireEvent.doubleClick(row as HTMLElement, { detail: 2 });
      act(() => vi.advanceTimersByTime(250));

      expect(onSelect).not.toHaveBeenCalled();
      expect(onOpen).toHaveBeenCalledOnce();
      expect(onOpen).toHaveBeenCalledWith('/test/notes.md', false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('opens a focused file with Enter and clears any pending single-click timer', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]);
    const onSelect = vi.fn();
    const onOpen = vi.fn();
    const { unmount } = render(
      <FileTree path="/test" onSelect={onSelect} onOpen={onOpen} />,
    );

    const row = (await screen.findByText('notes.md')).closest('[role="treeitem"]');
    expect(row).not.toBeNull();
    vi.useFakeTimers();
    try {
      fireEvent.click(row as HTMLElement, { detail: 1 });
      fireEvent.keyDown(row as HTMLElement, { key: 'Enter' });
      unmount();
      act(() => vi.advanceTimersByTime(250));

      expect(onSelect).not.toHaveBeenCalled();
      expect(onOpen).toHaveBeenCalledOnce();
      expect(onOpen).toHaveBeenCalledWith('/test/notes.md', false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('coordinates pending selections across nested branches when another branch opens', async () => {
    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const requestedPath = (args as { path: string }).path;
      if (requestedPath === '/test') {
        return [
          { name: 'left', path: '/test/left', is_dir: true, extension: null },
          { name: 'right', path: '/test/right', is_dir: true, extension: null },
        ];
      }
      if (requestedPath === '/test/left') {
        return [{ name: 'a.md', path: '/test/left/a.md', is_dir: false, extension: 'md' }];
      }
      if (requestedPath === '/test/right') {
        return [{ name: 'b.md', path: '/test/right/b.md', is_dir: false, extension: 'md' }];
      }
      return [];
    });
    const onSelect = vi.fn();
    const onOpen = vi.fn();
    render(<FileTree path="/test" onSelect={onSelect} onOpen={onOpen} />);

    fireEvent.click(await screen.findByRole('treeitem', { name: 'left' }), { detail: 1 });
    fireEvent.click(await screen.findByRole('treeitem', { name: 'right' }), { detail: 1 });
    const a = await screen.findByRole('treeitem', { name: 'a.md' });
    const b = await screen.findByRole('treeitem', { name: 'b.md' });

    vi.useFakeTimers();
    fireEvent.click(a, { detail: 1 });
    fireEvent.click(b, { detail: 2 });
    fireEvent.doubleClick(b, { detail: 2 });
    act(() => vi.advanceTimersByTime(250));

    expect(onSelect).not.toHaveBeenCalledWith('/test/left/a.md', false);
    expect(onOpen).toHaveBeenCalledOnce();
    expect(onOpen).toHaveBeenCalledWith('/test/right/b.md', false);
  });

  it('cancels pending selection when the root path changes', async () => {
    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const requestedPath = (args as { path: string }).path;
      return requestedPath === '/first'
        ? [{ name: 'stale.md', path: '/first/stale.md', is_dir: false, extension: 'md' }]
        : [{ name: 'fresh.md', path: '/second/fresh.md', is_dir: false, extension: 'md' }];
    });
    const onSelect = vi.fn();
    const { rerender } = render(<FileTree path="/first" onSelect={onSelect} />);
    const stale = await screen.findByRole('treeitem', { name: 'stale.md' });

    vi.useFakeTimers();
    fireEvent.click(stale, { detail: 1 });
    rerender(<FileTree path="/second" onSelect={onSelect} />);
    act(() => vi.advanceTimersByTime(250));

    expect(onSelect).not.toHaveBeenCalled();
  });

  it('cancels pending selection when the explorer root identity changes', async () => {
    vi.mocked(invoke).mockResolvedValue([
      { name: 'stale.md', path: '/test/stale.md', is_dir: false, extension: 'md' },
    ]);
    const onSelect = vi.fn();
    const { rerender } = render(
      <FileTree path="/test" explorerRoot="/test" onSelect={onSelect} />,
    );
    const stale = await screen.findByRole('treeitem', { name: 'stale.md' });

    vi.useFakeTimers();
    fireEvent.click(stale, { detail: 1 });
    rerender(<FileTree path="/test" explorerRoot="/other" onSelect={onSelect} />);
    act(() => vi.advanceTimersByTime(250));

    expect(onSelect).not.toHaveBeenCalled();
  });

  it('cancels pending selection when the entire tree unmounts', async () => {
    vi.mocked(invoke).mockResolvedValue([
      { name: 'stale.md', path: '/test/stale.md', is_dir: false, extension: 'md' },
    ]);
    const onSelect = vi.fn();
    const { unmount } = render(<FileTree path="/test" onSelect={onSelect} />);
    const stale = await screen.findByRole('treeitem', { name: 'stale.md' });

    vi.useFakeTimers();
    fireEvent.click(stale, { detail: 1 });
    unmount();
    act(() => vi.advanceTimersByTime(250));

    expect(onSelect).not.toHaveBeenCalled();
  });

  it('keeps only the latest pending selection across branches', async () => {
    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const requestedPath = (args as { path: string }).path;
      if (requestedPath === '/test') {
        return [
          { name: 'left', path: '/test/left', is_dir: true, extension: null },
          { name: 'right', path: '/test/right', is_dir: true, extension: null },
        ];
      }
      return requestedPath === '/test/left'
        ? [{ name: 'a.md', path: '/test/left/a.md', is_dir: false, extension: 'md' }]
        : [{ name: 'b.md', path: '/test/right/b.md', is_dir: false, extension: 'md' }];
    });
    const onSelect = vi.fn();
    render(<FileTree path="/test" onSelect={onSelect} />);

    fireEvent.click(await screen.findByRole('treeitem', { name: 'left' }), { detail: 1 });
    fireEvent.click(await screen.findByRole('treeitem', { name: 'right' }), { detail: 1 });
    const a = await screen.findByRole('treeitem', { name: 'a.md' });
    const b = await screen.findByRole('treeitem', { name: 'b.md' });

    vi.useFakeTimers();
    fireEvent.click(a, { detail: 1 });
    fireEvent.click(b, { detail: 1 });
    act(() => vi.advanceTimersByTime(250));

    expect(onSelect).toHaveBeenCalledTimes(3);
    expect(onSelect).toHaveBeenLastCalledWith('/test/right/b.md', false);
    expect(onSelect).not.toHaveBeenCalledWith('/test/left/a.md', false);
  });

  it('uses one roving tab stop and supports tree keyboard navigation', async () => {
    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const requestedPath = (args as { path: string }).path;
      if (requestedPath === '/test') {
        return [
          { name: 'src', path: '/test/src', is_dir: true, extension: null },
          { name: 'root.md', path: '/test/root.md', is_dir: false, extension: 'md' },
        ];
      }
      return [{ name: 'child.md', path: '/test/src/child.md', is_dir: false, extension: 'md' }];
    });
    render(<FileTree path="/test" />);

    const src = await screen.findByRole('treeitem', { name: 'src' });
    const rootFile = await screen.findByRole('treeitem', { name: 'root.md' });
    expect(src).toHaveAttribute('tabindex', '0');
    expect(rootFile).toHaveAttribute('tabindex', '-1');

    src.focus();
    fireEvent.keyDown(src, { key: 'ArrowRight' });
    const child = await screen.findByRole('treeitem', { name: 'child.md' });
    expect(src).toHaveAttribute('aria-expanded', 'true');
    expect(child.closest('[role="group"]')?.parentElement).toBe(src);

    fireEvent.keyDown(src, { key: 'ArrowRight' });
    expect(child).toHaveFocus();
    expect(child).toHaveAttribute('tabindex', '0');
    expect(src).toHaveAttribute('tabindex', '-1');

    fireEvent.keyDown(child, { key: 'ArrowDown' });
    expect(rootFile).toHaveFocus();
    fireEvent.keyDown(rootFile, { key: 'Home' });
    expect(src).toHaveFocus();
    fireEvent.keyDown(src, { key: 'End' });
    expect(rootFile).toHaveFocus();
    fireEvent.keyDown(rootFile, { key: 'ArrowUp' });
    expect(child).toHaveFocus();
    fireEvent.keyDown(child, { key: 'ArrowLeft' });
    expect(src).toHaveFocus();
    fireEvent.keyDown(src, { key: 'ArrowLeft' });
    expect(src).toHaveAttribute('aria-expanded', 'false');
  });

  it('restores one keyboard target when refresh removes the active item or repopulates an empty tree', async () => {
    let rootNodes = [
      { name: 'a.md', path: '/test/a.md', is_dir: false, extension: 'md' },
      { name: 'b.md', path: '/test/b.md', is_dir: false, extension: 'md' },
    ];
    vi.mocked(invoke).mockImplementation(async (_command, args) => (
      (args as { path: string }).path === '/test' ? rootNodes : []
    ));
    const onOpen = vi.fn();
    const { rerender } = render(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={0}
        changedPaths={[]}
        onOpen={onOpen}
      />,
    );

    const a = await screen.findByRole('treeitem', { name: 'a.md' });
    a.focus();
    expect(a).toHaveFocus();
    expect(a).toHaveAttribute('tabindex', '0');

    rootNodes = [
      { name: 'b.md', path: '/test/b.md', is_dir: false, extension: 'md' },
    ];
    rerender(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={1}
        changedPaths={['/test/a.md']}
        onOpen={onOpen}
      />,
    );

    await waitFor(() => expect(screen.queryByRole('treeitem', { name: 'a.md' })).not.toBeInTheDocument());
    const b = screen.getByRole('treeitem', { name: 'b.md' });
    expect(screen.getAllByRole('treeitem').filter((item) => item.tabIndex === 0)).toEqual([b]);
    await userEvent.tab();
    expect(b).toHaveFocus();
    fireEvent.keyDown(b, { key: 'Enter' });
    expect(onOpen).toHaveBeenLastCalledWith('/test/b.md', false);

    rootNodes = [];
    rerender(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={2}
        changedPaths={['/test/b.md']}
        onOpen={onOpen}
      />,
    );
    await waitFor(() => expect(screen.queryAllByRole('treeitem')).toHaveLength(0));

    rootNodes = [
      { name: 'c.md', path: '/test/c.md', is_dir: false, extension: 'md' },
    ];
    rerender(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={3}
        changedPaths={['/test/c.md']}
        onOpen={onOpen}
      />,
    );

    const c = await screen.findByRole('treeitem', { name: 'c.md' });
    expect(screen.getAllByRole('treeitem').filter((item) => item.tabIndex === 0)).toEqual([c]);
    await userEvent.tab();
    expect(c).toHaveFocus();
    fireEvent.keyDown(c, { key: 'Enter' });
    expect(onOpen).toHaveBeenLastCalledWith('/test/c.md', false);
  });

  it('keeps directories as accessible expand/collapse items without opening Files', async () => {
    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      return (args as { path: string }).path === '/test'
        ? [{ name: 'src', path: '/test/src', is_dir: true, extension: null }]
        : [];
    });
    const onSelect = vi.fn();
    const onOpen = vi.fn();
    render(<FileTree path="/test" onSelect={onSelect} onOpen={onOpen} />);

    const row = await screen.findByRole('treeitem', { name: 'src' });
    expect(row).toHaveAttribute('tabindex', '0');
    expect(row).toHaveAttribute('aria-expanded', 'false');
    fireEvent.keyDown(row, { key: 'Enter' });

    expect(row).toHaveAttribute('aria-expanded', 'true');
    expect(onSelect).toHaveBeenCalledWith('/test/src', true);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('uses the expand chevron without a folder glyph for directories and keeps file type icons', async () => {
    const mockNodes = [
      { name: 'src', path: '/test/src', is_dir: true, extension: null },
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ];

    vi.mocked(invoke).mockResolvedValueOnce(mockNodes);

    render(<FileTree path="/test" />);

    await waitFor(() => {
      expect(screen.getByText('src')).toBeInTheDocument();
      expect(screen.getByText('notes.md')).toBeInTheDocument();
    });

    const directoryRow = screen.getByText('src').closest('div');
    const fileRow = screen.getByText('notes.md').closest('div');

    expect(directoryRow?.querySelectorAll('svg')).toHaveLength(1);
    expect(fileRow?.querySelectorAll('svg')).toHaveLength(1);
  });

  it('refetches an expanded directory when a refresh event affects that directory', async () => {
    const rootNodes = [
      { name: 'src', path: '/test/src', is_dir: true, extension: null },
    ];
    const initialSrcNodes = [
      { name: 'before.ts', path: '/test/src/before.ts', is_dir: false, extension: 'ts' },
    ];
    const refreshedSrcNodes = [
      { name: 'after.ts', path: '/test/src/after.ts', is_dir: false, extension: 'ts' },
    ];
    let srcReads = 0;

    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const path = (args as { path: string }).path;
      if (path === '/test') return rootNodes;
      if (path === '/test/src') {
        srcReads += 1;
        return srcReads === 1 ? initialSrcNodes : refreshedSrcNodes;
      }
      return [];
    });

    const { rerender } = render(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={0}
        changedPaths={[]}
      />,
    );

    await userEvent.click(await screen.findByText('src'));
    expect(await screen.findByText('before.ts')).toBeInTheDocument();

    rerender(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={1}
        changedPaths={['/test/src/after.ts']}
      />,
    );

    expect(await screen.findByText('after.ts')).toBeInTheDocument();
    expect(screen.queryByText('before.ts')).not.toBeInTheDocument();
    expect(screen.getByText('src')).toBeInTheDocument();
  });

  it('refetches an expanded directory when a Windows watcher reports a verbatim changed path', async () => {
    const rootPath = 'C:\\Users\\test\\repo';
    const srcPath = 'C:\\Users\\test\\repo\\src';
    const rootNodes = [
      { name: 'src', path: srcPath, is_dir: true, extension: null },
    ];
    const initialSrcNodes = [
      { name: 'before.ts', path: `${srcPath}\\before.ts`, is_dir: false, extension: 'ts' },
    ];
    const refreshedSrcNodes = [
      { name: 'after.ts', path: `${srcPath}\\after.ts`, is_dir: false, extension: 'ts' },
    ];
    let srcReads = 0;

    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const path = (args as { path: string }).path;
      if (path === rootPath) return rootNodes;
      if (path === srcPath) {
        srcReads += 1;
        return srcReads === 1 ? initialSrcNodes : refreshedSrcNodes;
      }
      return [];
    });

    const { rerender } = render(
      <FileTree
        path={rootPath}
        explorerRoot={rootPath}
        refreshToken={0}
        changedPaths={[]}
      />,
    );

    await userEvent.click(await screen.findByText('src'));
    expect(await screen.findByText('before.ts')).toBeInTheDocument();

    rerender(
      <FileTree
        path={rootPath}
        explorerRoot={rootPath}
        refreshToken={1}
        changedPaths={['\\\\?\\C:\\Users\\test\\repo\\src\\after.ts']}
      />,
    );

    expect(await screen.findByText('after.ts')).toBeInTheDocument();
    expect(screen.queryByText('before.ts')).not.toBeInTheDocument();
  });

  it('does not refetch an expanded directory when a refresh event is unrelated', async () => {
    const rootNodes = [
      { name: 'src', path: '/test/src', is_dir: true, extension: null },
    ];
    const srcNodes = [
      { name: 'before.ts', path: '/test/src/before.ts', is_dir: false, extension: 'ts' },
    ];

    vi.mocked(invoke).mockImplementation(async (_command, args) => {
      const path = (args as { path: string }).path;
      if (path === '/test') return rootNodes;
      if (path === '/test/src') return srcNodes;
      return [];
    });

    const { rerender } = render(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={0}
        changedPaths={[]}
      />,
    );

    await userEvent.click(await screen.findByText('src'));
    expect(await screen.findByText('before.ts')).toBeInTheDocument();

    const callsBeforeRefresh = vi.mocked(invoke).mock.calls.length;
    rerender(
      <FileTree
        path="/test"
        explorerRoot="/test"
        refreshToken={1}
        changedPaths={['/test/docs/readme.md']}
      />,
    );

    await waitFor(() => {
      expect(vi.mocked(invoke).mock.calls.length).toBe(callsBeforeRefresh);
    });
  });
});
