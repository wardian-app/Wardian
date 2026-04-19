import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { ExplorerPanel } from './ExplorerPanel';

vi.mock('./FileTree', () => ({
  FileTree: ({ path }: { path: string }) => <div data-testid="file-tree">{path}</div>,
}));

describe('ExplorerPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('opens the current explorer root in the local file system', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\.wardian';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} />);

    const openButton = await screen.findByRole('button', { name: 'Open in local file system' });
    await userEvent.click(openButton);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('reveal_in_explorer', {
        path: 'C:\\Users\\test\\.wardian',
      });
    });
  });
});
