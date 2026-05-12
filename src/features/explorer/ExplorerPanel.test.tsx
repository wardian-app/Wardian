import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { ExplorerPanel } from './ExplorerPanel';
import type { AgentConfig } from '../../types';

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

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    const openButton = await screen.findByRole('button', { name: 'Open in local file system' });
    expect(openButton).toBeInTheDocument();
    expect(screen.queryByText('Open')).not.toBeInTheDocument();

    await userEvent.click(openButton);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('reveal_in_explorer', {
        path: 'C:\\Users\\test\\.wardian',
      });
    });
  });

  it('re-resolves the explorer root when the selected agent worktree assignment changes', async () => {
    const agent: AgentConfig = {
      session_id: 'agent-1',
      session_name: 'Agent',
      agent_class: 'Coder',
      folder: 'C:/repo',
      is_off: false,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:/repo-worktree';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    const { rerender } = render(
      <ExplorerPanel selectedAgentIds={new Set(['agent-1'])} agents={[agent]} />,
    );

    expect(await screen.findByTestId('file-tree')).toHaveTextContent('C:/repo-worktree');
    const callsBeforeRerender = vi.mocked(invoke).mock.calls.length;

    rerender(
      <ExplorerPanel
        selectedAgentIds={new Set(['agent-1'])}
        agents={[{ ...agent, git_worktree: true, git_worktree_folder: 'C:/repo-worktree' }]}
      />,
    );

    await waitFor(() => {
      expect(vi.mocked(invoke).mock.calls.length).toBeGreaterThan(callsBeforeRerender);
      expect(invoke).toHaveBeenCalledWith('get_explorer_root', { sessionId: 'agent-1' });
    });
  });
});
