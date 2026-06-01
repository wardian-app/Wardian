import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import type React from 'react';
import { ExplorerPanel } from './ExplorerPanel';
import type { AgentConfig } from '../../types';
import { useSettingsStore } from '../../store/useSettingsStore';

vi.mock('./FileTree', () => ({
  FileTree: ({
    path,
    onContextMenu,
    onSelect,
  }: {
    path: string;
    onContextMenu?: (event: React.MouseEvent, node: unknown) => void;
    onSelect?: (path: string, isDir: boolean) => void;
  }) => (
    <div data-testid="file-tree">
      <button
        type="button"
        data-testid="mock-file-row"
        onClick={() => onSelect?.('C:\\Users\\test\\repo\\notes.md', false)}
        onContextMenu={(event) => onContextMenu?.(event, {
          name: 'notes.md',
          path: 'C:\\Users\\test\\repo\\notes.md',
          is_dir: false,
          extension: 'md',
        })}
      >
        {path}
      </button>
      <button
        type="button"
        data-testid="mock-folder-row"
        onClick={() => onSelect?.('C:\\Users\\test\\repo\\src', true)}
      >
        src
      </button>
    </div>
  ),
}));

describe('ExplorerPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useSettingsStore.setState({
      externalEditor: 'system',
      externalEditorCustomExecutable: '',
      explorerFileClickAction: 'preview',
    });
  });

  it('uses compact sidebar title typography', () => {
    vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    expect(screen.getByRole('heading', { name: 'Explorer', level: 2 })).toHaveClass('text-sm');
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

  it('opens a right-clicked file in the configured external editor', async () => {
    useSettingsStore.setState({
      externalEditor: 'vscode',
      externalEditorCustomExecutable: '',
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    const tree = await screen.findByTestId('mock-file-row');
    await userEvent.pointer({ keys: '[MouseRight]', target: tree });
    await userEvent.click(await screen.findByRole('button', { name: 'Open in External App' }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('open_in_external_editor', {
        path: 'C:\\Users\\test\\repo\\notes.md',
        editor: {
          external_editor: 'vscode',
          external_editor_custom_executable: null,
        },
      });
    });
  });

  it('shows a visible error when the configured external editor cannot open', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    useSettingsStore.setState({
      externalEditor: 'vscode',
      externalEditorCustomExecutable: '',
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      if (command === 'open_in_external_editor') throw new Error('program not found');
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    const tree = await screen.findByTestId('mock-file-row');
    await userEvent.pointer({ keys: '[MouseRight]', target: tree });
    await userEvent.click(await screen.findByRole('button', { name: 'Open in External App' }));

    expect(await screen.findByText(/External app open failed for VS Code/i)).toBeInTheDocument();
    expect(screen.getByText(/program not found/i)).toBeInTheDocument();
    consoleError.mockRestore();
  });

  it('previews a clicked file when Explorer file click action is Preview', async () => {
    useSettingsStore.setState({
      explorerFileClickAction: 'preview',
      externalEditor: 'system',
      externalEditorCustomExecutable: '',
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      if (command === 'read_file_preview') return '# Notes';
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    await userEvent.click(await screen.findByTestId('mock-file-row'));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('read_file_preview', {
        path: 'C:\\Users\\test\\repo\\notes.md',
      });
    });
    expect(await screen.findByText('# Notes')).toBeInTheDocument();
  });

  it('opens a clicked file externally when Explorer file click action is External app', async () => {
    useSettingsStore.setState({
      explorerFileClickAction: 'external',
      externalEditor: 'vscode',
      externalEditorCustomExecutable: '',
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    await userEvent.click(await screen.findByTestId('mock-file-row'));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('open_in_external_editor', {
        path: 'C:\\Users\\test\\repo\\notes.md',
        editor: {
          external_editor: 'vscode',
          external_editor_custom_executable: null,
        },
      });
    });
  });

  it('does not preview or externally open clicked folders', async () => {
    useSettingsStore.setState({
      explorerFileClickAction: 'external',
      externalEditor: 'vscode',
      externalEditorCustomExecutable: '',
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    await userEvent.click(await screen.findByTestId('mock-folder-row'));

    await waitFor(() => {
      expect(screen.getByTestId('file-tree')).toBeInTheDocument();
    });
    expect(invoke).not.toHaveBeenCalledWith('read_file_preview', expect.anything());
    expect(invoke).not.toHaveBeenCalledWith('open_in_external_editor', expect.anything());
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
