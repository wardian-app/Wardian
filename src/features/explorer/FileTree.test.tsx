import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
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
});
