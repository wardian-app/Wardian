import { dirPage } from "../../test/pageFixtures";
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { FileTree } from './FileTree';
import { invoke } from '@tauri-apps/api/core';
import { WARDIAN_FILE_PATH_MIME, WARDIAN_FILE_PATHS_MIME } from '../../utils/fileDrop';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

describe('FileTree Component', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('renders a directory tree and handles folder expansion', async () => {
    const mockNodes = [
      { name: 'folderA', path: '/test/folderA', is_dir: true, extension: null },
      { name: 'fileB.txt', path: '/test/fileB.txt', is_dir: false, extension: 'txt' }
    ];

    vi.mocked(invoke).mockResolvedValueOnce(dirPage(mockNodes));

    render(<FileTree path="/test" />);

    await waitFor(() => {
      expect(screen.getByText('folderA')).toBeInTheDocument();
      expect(screen.getByText('fileB.txt')).toBeInTheDocument();
    });

    // Mock the subsequent invoke for expanding the folder
    vi.mocked(invoke).mockResolvedValueOnce(dirPage([{
      name: 'subfile.js', path: '/test/folderA/subfile.js', is_dir: false, extension: 'js'
    }]));

    const folderEl = screen.getByText('folderA');
    await userEvent.click(folderEl);

    // Verify subfile loads
    await waitFor(() => {
      expect(screen.getByText('subfile.js')).toBeInTheDocument();
    });
  });

  it('renders a bounded preview before the canonical directory listing settles', async () => {
    let paintCallback: FrameRequestCallback | undefined;
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
      paintCallback = callback;
      return 1;
    });
    let resolveListing: ((value: ReturnType<typeof dirPage>) => void) | undefined;
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === 'get_directory_preview') {
        return Promise.resolve({
          nodes: [{ name: 'visible-now.txt', path: '/test/visible-now.txt', is_dir: false, extension: 'txt' }],
          truncated: true,
          next_offset: null,
        });
      }
      if (command === 'get_directory_tree') {
        return new Promise((resolve) => {
          resolveListing = resolve;
        });
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    render(<FileTree path="/test" />);

    expect(await screen.findByText('visible-now.txt')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Loading remaining files');
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'get_directory_tree')).toBe(false);

    act(() => paintCallback?.(0));
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'get_directory_tree')).toBe(false);
    await waitFor(() => {
      expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'get_directory_tree')).toBe(true);
    });

    await act(async () => {
      resolveListing?.(dirPage([
        { name: 'canonical.txt', path: '/test/canonical.txt', is_dir: false, extension: 'txt' },
      ]));
    });

    expect(await screen.findByText('canonical.txt')).toBeInTheDocument();
    expect(screen.queryByText('visible-now.txt')).not.toBeInTheDocument();
  });

  it('does not replay an inherited watcher refresh when a branch first mounts', async () => {
    let resolvePreview: ((value: ReturnType<typeof dirPage>) => void) | undefined;
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === 'get_directory_preview') {
        return new Promise((resolve) => {
          resolvePreview = resolve;
        });
      }
      if (command === 'get_directory_tree') return Promise.resolve(dirPage([]));
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    render(
      <FileTree
        path="/test/src"
        refreshToken={7}
        changedPaths={['/test/src/new.txt']}
      />,
    );

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_directory_preview', { path: '/test/src' });
    });
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'get_directory_tree')).toBe(false);

    await act(async () => {
      resolvePreview?.(dirPage([
        { name: 'new.txt', path: '/test/src/new.txt', is_dir: false, extension: 'txt' },
      ]));
    });

    expect(await screen.findByText('new.txt')).toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'get_directory_tree')).toBe(false);
  });

  it('keeps a watcher refresh result when an older preview settles later', async () => {
    let resolvePreview: ((value: ReturnType<typeof dirPage>) => void) | undefined;
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === 'get_directory_preview') {
        return new Promise((resolve) => {
          resolvePreview = resolve;
        });
      }
      if (command === 'get_directory_tree') {
        return Promise.resolve(dirPage([
          { name: 'fresh.txt', path: '/test/src/fresh.txt', is_dir: false, extension: 'txt' },
        ]));
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    const { rerender } = render(
      <FileTree path="/test/src" refreshToken={0} changedPaths={[]} />,
    );
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_directory_preview', { path: '/test/src' });
    });

    rerender(
      <FileTree
        path="/test/src"
        refreshToken={1}
        changedPaths={['/test/src/fresh.txt']}
      />,
    );

    expect(await screen.findByText('fresh.txt')).toBeInTheDocument();

    await act(async () => {
      resolvePreview?.(dirPage([
        { name: 'stale.txt', path: '/test/src/stale.txt', is_dir: false, extension: 'txt' },
      ]));
    });

    expect(screen.getByText('fresh.txt')).toBeInTheDocument();
    expect(screen.queryByText('stale.txt')).not.toBeInTheDocument();
  });

  it('blocks pagination until a watcher refresh establishes the current page offset', async () => {
    let treeReads = 0;
    let resolveRefresh: ((value: ReturnType<typeof dirPage>) => void) | undefined;
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (command === 'get_directory_preview') {
        return Promise.resolve({
          nodes: [{ name: 'old.txt', path: '/test/old.txt', is_dir: false, extension: 'txt' }],
          truncated: true,
          next_offset: null,
        });
      }
      if (command === 'get_directory_tree') {
        treeReads += 1;
        if (treeReads === 1) {
          return Promise.resolve({
            nodes: [{ name: 'old.txt', path: '/test/old.txt', is_dir: false, extension: 'txt' }],
            truncated: true,
            next_offset: 500,
          });
        }
        if (treeReads === 2) {
          return new Promise((resolve) => {
            resolveRefresh = resolve;
          });
        }
        expect(args).toEqual({ path: '/test', offset: 300 });
        return Promise.resolve(dirPage([
          { name: 'next.txt', path: '/test/next.txt', is_dir: false, extension: 'txt' },
        ]));
      }
      return Promise.reject(new Error(`Unexpected command: ${command}`));
    });

    const { rerender } = render(
      <FileTree path="/test" refreshToken={0} changedPaths={[]} />,
    );
    expect(await screen.findByText('old.txt')).toBeInTheDocument();
    const initialLoadMore = await screen.findByRole('button', { name: 'Load next 500' });

    rerender(
      <FileTree
        path="/test"
        refreshToken={1}
        changedPaths={['/test/old.txt']}
      />,
    );

    await waitFor(() => expect(treeReads).toBe(2));
    expect(screen.getByRole('button', { name: 'Loading…' })).toBeDisabled();
    fireEvent.click(initialLoadMore);
    expect(treeReads).toBe(2);

    await act(async () => {
      resolveRefresh?.({
        nodes: [{ name: 'fresh.txt', path: '/test/fresh.txt', is_dir: false, extension: 'txt' }],
        truncated: true,
        next_offset: 300,
      });
    });

    expect(await screen.findByText('fresh.txt')).toBeInTheDocument();
    expect(screen.queryByText('old.txt')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Load next 500' }));
    expect(await screen.findByText('next.txt')).toBeInTheDocument();
    expect(treeReads).toBe(3);
  });

  it('makes workspace files draggable with a Wardian path payload', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(dirPage([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]));

    render(<FileTree path="/test" />);

    const file = await screen.findByRole('treeitem', { name: 'notes.md' });
    const values = new Map<string, string>();
    const dataTransfer = {
      setData: (type: string, value: string) => values.set(type, value),
      effectAllowed: '',
    } as unknown as DataTransfer;

    fireEvent.dragStart(file, { dataTransfer });

    expect(values.get(WARDIAN_FILE_PATH_MIME)).toBe('/test/notes.md');
    expect(values.get(WARDIAN_FILE_PATHS_MIME)).toBe('/test/notes.md');
    expect(dataTransfer.effectAllowed).toBe('copy');
    expect(file).toHaveAttribute('draggable', 'true');
  });

  it('shows when a directory listing is partial', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        nodes: [{ name: 'file.txt', path: '/test/file.txt', is_dir: false, extension: 'txt' }],
        truncated: true,
      })
      .mockResolvedValueOnce({
        nodes: [{ name: 'file.txt', path: '/test/file.txt', is_dir: false, extension: 'txt' }],
        truncated: true,
        next_offset: 500,
      });

    render(<FileTree path="/test" />);

    expect(await screen.findByText(/first 500 items/i)).toBeInTheDocument();
  });

  it('loads one more bounded directory page', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        nodes: [{ name: 'first.txt', path: '/test/first.txt', is_dir: false, extension: 'txt' }],
        truncated: true,
        next_offset: null,
      })
      .mockResolvedValueOnce({
        nodes: [{ name: 'first.txt', path: '/test/first.txt', is_dir: false, extension: 'txt' }],
        truncated: true,
        next_offset: 500,
      })
      .mockResolvedValueOnce({
        nodes: [{ name: 'next.txt', path: '/test/next.txt', is_dir: false, extension: 'txt' }],
        truncated: false,
        next_offset: null,
      });

    render(<FileTree path="/test" />);
    fireEvent.click(await screen.findByRole('button', { name: /load next 500/i }));
    expect(await screen.findByText('next.txt')).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenLastCalledWith('get_directory_tree', { path: '/test', offset: 500 });
  });

  it('calls onContextMenu when an item is right-clicked', async () => {
    const mockNodes = [
      { name: 'fileC.png', path: '/test/fileC.png', is_dir: false, extension: 'png' }
    ];

    vi.mocked(invoke).mockResolvedValueOnce(dirPage(mockNodes));

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
    vi.mocked(invoke).mockResolvedValueOnce(dirPage([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]));
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

  it('opens a file in a new tab immediately on Ctrl/Cmd-click', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(dirPage([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]));
    const onSelect = vi.fn();
    render(<FileTree path="/test" onSelect={onSelect} />);

    const file = await screen.findByText('notes.md');
    fireEvent.click(file, { ctrlKey: true });
    fireEvent.click(file, { metaKey: true });

    expect(onSelect).toHaveBeenCalledTimes(2);
    expect(onSelect).toHaveBeenNthCalledWith(1, '/test/notes.md', false, true);
    expect(onSelect).toHaveBeenNthCalledWith(2, '/test/notes.md', false, true);
  });

  it('cancels delayed selection when the same file is double-clicked open', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(dirPage([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]));
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
    vi.mocked(invoke).mockResolvedValueOnce(dirPage([
      { name: 'notes.md', path: '/test/notes.md', is_dir: false, extension: 'md' },
    ]));
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
        return dirPage([
          { name: 'left', path: '/test/left', is_dir: true, extension: null },
          { name: 'right', path: '/test/right', is_dir: true, extension: null },
        ]);
      }
      if (requestedPath === '/test/left') {
        return dirPage([{ name: 'a.md', path: '/test/left/a.md', is_dir: false, extension: 'md' }]);
      }
      if (requestedPath === '/test/right') {
        return dirPage([{ name: 'b.md', path: '/test/right/b.md', is_dir: false, extension: 'md' }]);
      }
      return dirPage([]);
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
        ? dirPage([{ name: 'stale.md', path: '/first/stale.md', is_dir: false, extension: 'md' }]) : dirPage([{ name: 'fresh.md', path: '/second/fresh.md', is_dir: false, extension: 'md' }]);
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
    vi.mocked(invoke).mockResolvedValue(dirPage([
      { name: 'stale.md', path: '/test/stale.md', is_dir: false, extension: 'md' },
    ]));
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
    vi.mocked(invoke).mockResolvedValue(dirPage([
      { name: 'stale.md', path: '/test/stale.md', is_dir: false, extension: 'md' },
    ]));
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
        return dirPage([
          { name: 'left', path: '/test/left', is_dir: true, extension: null },
          { name: 'right', path: '/test/right', is_dir: true, extension: null },
        ]);
      }
      return requestedPath === '/test/left'
        ? dirPage([{ name: 'a.md', path: '/test/left/a.md', is_dir: false, extension: 'md' }]) : dirPage([{ name: 'b.md', path: '/test/right/b.md', is_dir: false, extension: 'md' }]);
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
        return dirPage([
          { name: 'src', path: '/test/src', is_dir: true, extension: null },
          { name: 'root.md', path: '/test/root.md', is_dir: false, extension: 'md' },
        ]);
      }
      return dirPage([{ name: 'child.md', path: '/test/src/child.md', is_dir: false, extension: 'md' }]);
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
      (args as { path: string }).path === '/test' ? dirPage(rootNodes) : dirPage([])
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
        ? dirPage([{ name: 'src', path: '/test/src', is_dir: true, extension: null }]) : dirPage([]);
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

    vi.mocked(invoke).mockResolvedValueOnce(dirPage(mockNodes));

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
      if (path === '/test') return dirPage(rootNodes);
      if (path === '/test/src') {
        srcReads += 1;
        return srcReads === 1 ? dirPage(initialSrcNodes) : dirPage(refreshedSrcNodes);
      }
      return dirPage([]);
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
      if (path === rootPath) return dirPage(rootNodes);
      if (path === srcPath) {
        srcReads += 1;
        return srcReads === 1 ? dirPage(initialSrcNodes) : dirPage(refreshedSrcNodes);
      }
      return dirPage([]);
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
      if (path === '/test') return dirPage(rootNodes);
      if (path === '/test/src') return dirPage(srcNodes);
      return dirPage([]);
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
