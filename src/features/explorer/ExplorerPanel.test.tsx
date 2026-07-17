import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type React from 'react';
import { ExplorerPanel } from './ExplorerPanel';
import type { AgentConfig } from '../../types';
import { useSettingsStore } from '../../store/useSettingsStore';
import { ConfirmProvider } from '../../components/ConfirmDialog';
import type { WorkbenchNavigationService } from '../workbench/navigationService';

const mockListen = vi.mocked(listen);

vi.mock('./FileTree', () => ({
  FileTree: ({
    path,
    onContextMenu,
    onSelect,
    onOpen,
    refreshToken,
    changedPaths,
  }: {
    path: string;
    onContextMenu?: (event: React.MouseEvent, node: unknown) => void;
    onSelect?: (path: string, isDir: boolean) => void;
    onOpen?: (path: string, isDir: boolean) => void;
    refreshToken?: number;
    changedPaths?: string[];
  }) => (
    <div data-testid="file-tree">
      <output data-testid="file-tree-refresh-token">{refreshToken ?? 0}</output>
      <output data-testid="file-tree-changed-paths">{(changedPaths ?? []).join('|')}</output>
      <button
        type="button"
        data-testid="mock-file-row"
        onClick={() => onSelect?.('C:\\Users\\test\\repo\\notes.md', false)}
        onDoubleClick={() => onOpen?.('C:\\Users\\test\\repo\\notes.md', false)}
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

function makeNavigation() {
  return {
    open: vi.fn(() => 'files-surface'),
    open_transient: vi.fn(() => 'files-surface'),
    pin_transient: vi.fn(),
    open_to_side: vi.fn(() => 'files-side-surface'),
  } as unknown as WorkbenchNavigationService;
}

describe('ExplorerPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListen.mockResolvedValue(vi.fn());
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

  it('places root action buttons in the Explorer title row', async () => {
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

    const heading = screen.getByRole('heading', { name: 'Explorer', level: 2 });
    const titleRow = heading.parentElement;
    expect(titleRow).not.toBeNull();

    await waitFor(() => {
      expect(within(titleRow as HTMLElement).getByRole('button', { name: 'Open in local file system' })).toBeInTheDocument();
      expect(within(titleRow as HTMLElement).getByRole('button', { name: 'Open root in VS Code' })).toBeInTheDocument();
    });

    const pathRow = screen.getByTitle('C:\\Users\\test\\repo').parentElement;
    expect(pathRow).not.toBeNull();
    expect(within(pathRow as HTMLElement).queryByRole('button', { name: 'Open in local file system' })).not.toBeInTheDocument();
    expect(within(pathRow as HTMLElement).queryByRole('button', { name: 'Open root in VS Code' })).not.toBeInTheDocument();
  });

  it('hides the root external action when it would duplicate system file manager behavior', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    await screen.findByRole('button', { name: 'Open in local file system' });
    expect(screen.queryByRole('button', {
      name: 'Choose VS Code or a custom external app in Settings to open the root externally',
    })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Open root in/i })).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('open_in_external_editor', expect.anything());
  });

  it('opens the current explorer root in the configured external editor', async () => {
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

    await userEvent.click(await screen.findByRole('button', { name: 'Open root in VS Code' }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('open_in_external_editor', {
        path: 'C:\\Users\\test\\repo',
        editor: {
          external_editor: 'vscode',
          external_editor_custom_executable: null,
        },
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

  it('surfaces missing Workbench navigation locally and recovers on the next action', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const unhandled = vi.fn();
    window.addEventListener('unhandledrejection', unhandled);
    const { rerender } = render(
      <ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={null} />,
    );

    await userEvent.click(await screen.findByTestId('mock-file-row'));

    expect(await screen.findByRole('alert')).toHaveTextContent(/File preview failed/i);
    expect(screen.getByRole('alert')).toHaveTextContent(/Workbench navigation is unavailable/i);
    expect(unhandled).not.toHaveBeenCalled();

    const navigation = makeNavigation();
    rerender(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);
    await userEvent.click(await screen.findByTestId('mock-file-row'));

    await waitFor(() => expect(navigation.open_transient).toHaveBeenCalledOnce());
    expect(screen.queryByText(/File preview failed/i)).not.toBeInTheDocument();
    expect(unhandled).not.toHaveBeenCalled();
    window.removeEventListener('unhandledrejection', unhandled);
  });

  it('contains rejected navigation actions and allows a later action to succeed', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const tabs: string[] = [];
    const navigation = makeNavigation();
    vi.mocked(navigation.open_transient)
      .mockImplementationOnce(() => Promise.reject(new Error('navigation offline')) as never)
      .mockImplementation(() => {
        tabs.push('notes');
        return 'files-surface';
      });
    const unhandled = vi.fn();
    window.addEventListener('unhandledrejection', unhandled);
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    const row = await screen.findByTestId('mock-file-row');
    await userEvent.click(row);

    expect(await screen.findByRole('alert')).toHaveTextContent(/navigation offline/i);
    expect(tabs).toEqual([]);
    expect(unhandled).not.toHaveBeenCalled();

    await userEvent.click(row);
    await waitFor(() => expect(tabs).toEqual(['notes']));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(unhandled).not.toHaveBeenCalled();
    window.removeEventListener('unhandledrejection', unhandled);
  });

  it('contains synchronous navigation failures without mutating tabs', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const tabs: string[] = [];
    const navigation = makeNavigation();
    vi.mocked(navigation.open).mockImplementation(() => {
      throw new Error('navigation not ready');
    });
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    fireEvent.doubleClick(await screen.findByTestId('mock-file-row'));

    expect(await screen.findByRole('alert')).toHaveTextContent(/navigation not ready/i);
    expect(tabs).toEqual([]);
    expect(navigation.pin_transient).not.toHaveBeenCalled();
  });

  it('clears an external command error after a successful retry', async () => {
    useSettingsStore.setState({
      explorerFileClickAction: 'external',
      externalEditor: 'vscode',
      externalEditorCustomExecutable: '',
    });
    let attempts = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      if (command === 'open_in_external_editor' && attempts++ === 0) {
        throw new Error('program warming up');
      }
      return null;
    });
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={makeNavigation()} />);

    const row = await screen.findByTestId('mock-file-row');
    await userEvent.click(row);
    expect(await screen.findByRole('alert')).toHaveTextContent(/program warming up/i);

    await userEvent.click(row);
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(attempts).toBe(2);
  });

  it('routes an internal single click to one transient Files surface without reading preview bytes', async () => {
    useSettingsStore.setState({
      explorerFileClickAction: 'preview',
      externalEditor: 'system',
      externalEditorCustomExecutable: '',
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const navigation = makeNavigation();

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    await userEvent.click(await screen.findByTestId('mock-file-row'));

    await waitFor(() => {
      expect(navigation.open_transient).toHaveBeenCalledWith({
        surface_type: 'files',
        resource_key: 'file:C:/Users/test/repo/notes.md',
        state: {
          resource_kind: 'file',
          mode: 'preview',
          transient_preview: true,
          review_drawer_open: false,
          selected_version_id: null,
          optional_checkpoint_id: null,
        },
      });
    });
    expect(invoke).not.toHaveBeenCalledWith('read_file_preview', expect.anything());
  });

  it('pins a matching transient or opens a permanent Files surface on double click', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const navigation = makeNavigation();
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    fireEvent.doubleClick(await screen.findByTestId('mock-file-row'));

    expect(navigation.open).toHaveBeenCalledOnce();
    expect(navigation.open).toHaveBeenCalledWith(expect.objectContaining({
      surface_type: 'files',
      resource_key: 'file:C:/Users/test/repo/notes.md',
      state: expect.objectContaining({ transient_preview: false }),
    }));
    expect(navigation.pin_transient).toHaveBeenCalledWith('files-surface');
    expect(navigation.open_transient).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith('read_file_preview', expect.anything());
  });

  it('uses permanent Open and standard horizontal Open to Side context actions', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const navigation = makeNavigation();
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    const tree = await screen.findByTestId('mock-file-row');
    await userEvent.pointer({ keys: '[MouseRight]', target: tree });
    await userEvent.click(await screen.findByRole('button', { name: 'Open' }));
    expect(navigation.open).toHaveBeenCalledWith(expect.objectContaining({
      resource_key: 'file:C:/Users/test/repo/notes.md',
      state: expect.objectContaining({ transient_preview: false }),
    }));
    expect(navigation.pin_transient).toHaveBeenCalledWith('files-surface');

    await userEvent.pointer({ keys: '[MouseRight]', target: tree });
    await userEvent.click(await screen.findByRole('button', { name: 'Open to Side' }));
    expect(navigation.open_to_side).toHaveBeenCalledWith(expect.objectContaining({
      resource_key: 'file:C:/Users/test/repo/notes.md',
      state: expect.objectContaining({ transient_preview: false }),
    }), 'horizontal');
    expect(invoke).not.toHaveBeenCalledWith('read_file_preview', expect.anything());
  });

  it('reports when Open to Side is rejected because the current pane is too small', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });
    const navigation = makeNavigation();
    vi.mocked(navigation.open_to_side).mockReturnValue(null);
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    const tree = await screen.findByTestId('mock-file-row');
    await userEvent.pointer({ keys: '[MouseRight]', target: tree });
    await userEvent.click(await screen.findByRole('button', { name: 'Open to Side' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Open to Side failed: Error: The current pane is too small to open a file to the side.',
    );
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

    const navigation = makeNavigation();
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

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
    expect(navigation.open).not.toHaveBeenCalled();
    expect(navigation.open_transient).not.toHaveBeenCalled();
    expect(navigation.open_to_side).not.toHaveBeenCalled();
  });

  it('opens a double-clicked file externally without creating or pinning a Files tab', async () => {
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
    const navigation = makeNavigation();
    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} navigation={navigation} />);

    fireEvent.doubleClick(await screen.findByTestId('mock-file-row'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('open_in_external_editor', {
      path: 'C:\\Users\\test\\repo\\notes.md',
      editor: {
        external_editor: 'vscode',
        external_editor_custom_executable: null,
      },
    }));
    expect(navigation.open).not.toHaveBeenCalled();
    expect(navigation.open_transient).not.toHaveBeenCalled();
    expect(navigation.pin_transient).not.toHaveBeenCalled();
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

  it('starts the explorer watcher only after the change listener is ready', async () => {
    const unlisten = vi.fn();
    let resolveListen: ((value: typeof unlisten) => void) | undefined;
    mockListen.mockReturnValue(new Promise((resolve) => {
      resolveListen = resolve;
    }));
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    expect(await screen.findByTestId('file-tree')).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('explorer_watch', { rootPath: 'C:\\Users\\test\\repo' });

    resolveListen?.(unlisten);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('explorer_watch', { rootPath: 'C:\\Users\\test\\repo' });
    });
  });

  it('passes matching explorer change events to FileTree refresh props', async () => {
    let handler: ((event: { payload: { root_path: string; changed_paths: string[] } }) => void) | undefined;
    mockListen.mockImplementation(async (_event, callback) => {
      handler = callback as typeof handler;
      return () => {};
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    expect(await screen.findByTestId('file-tree-refresh-token')).toHaveTextContent('0');
    // The change listener registers in an async effect; dispatching before it
    // resolves would silently no-op and leave the refresh token at 0.
    await waitFor(() => expect(handler).toBeDefined());

    await act(async () => {
      handler?.({
        payload: {
          root_path: 'C:\\Users\\test\\repo',
          changed_paths: ['C:\\Users\\test\\repo\\src\\new-file.ts'],
        },
      });
    });

    expect(await screen.findByTestId('file-tree-refresh-token')).toHaveTextContent('1');
    expect(screen.getByTestId('file-tree-changed-paths')).toHaveTextContent('C:\\Users\\test\\repo\\src\\new-file.ts');
  });

  it('passes explorer change events when the watcher reports a Windows verbatim root path', async () => {
    let handler: ((event: { payload: { root_path: string; changed_paths: string[] } }) => void) | undefined;
    mockListen.mockImplementation(async (_event, callback) => {
      handler = callback as typeof handler;
      return () => {};
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    expect(await screen.findByTestId('file-tree-refresh-token')).toHaveTextContent('0');
    await waitFor(() => expect(handler).toBeDefined());

    await act(async () => {
      handler?.({
        payload: {
          root_path: '\\\\?\\C:\\Users\\test\\repo',
          changed_paths: ['\\\\?\\C:\\Users\\test\\repo\\src\\new-file.ts'],
        },
      });
    });

    expect(await screen.findByTestId('file-tree-refresh-token')).toHaveTextContent('1');
    expect(screen.getByTestId('file-tree-changed-paths')).toHaveTextContent('\\\\?\\C:\\Users\\test\\repo\\src\\new-file.ts');
  });

  it('ignores explorer change events from other roots', async () => {
    let handler: ((event: { payload: { root_path: string; changed_paths: string[] } }) => void) | undefined;
    mockListen.mockImplementation(async (_event, callback) => {
      handler = callback as typeof handler;
      return () => {};
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    expect(await screen.findByTestId('file-tree-refresh-token')).toHaveTextContent('0');
    await waitFor(() => expect(handler).toBeDefined());

    await act(async () => {
      handler?.({
        payload: {
          root_path: 'D:\\Other\\repo',
          changed_paths: ['D:\\Other\\repo\\src\\new-file.ts'],
        },
      });
    });

    await waitFor(() => {
      expect(screen.getByTestId('file-tree-refresh-token')).toHaveTextContent('0');
    });
  });

  it('stops the explorer watcher and listener on unmount', async () => {
    const unlisten = vi.fn();
    mockListen.mockResolvedValue(unlisten);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      return null;
    });

    const { unmount } = render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('explorer_watch', { rootPath: 'C:\\Users\\test\\repo' });
    });

    unmount();

    await waitFor(() => {
      expect(unlisten).toHaveBeenCalledTimes(1);
      expect(invoke).toHaveBeenCalledWith('explorer_unwatch', { rootPath: 'C:\\Users\\test\\repo' });
    });
  });

  it('unwatches after a pending explorer watcher resolves if unmounted first', async () => {
    const unlisten = vi.fn();
    let resolveWatch: (() => void) | undefined;
    mockListen.mockResolvedValue(unlisten);
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === 'get_explorer_root') return Promise.resolve('C:\\Users\\test\\repo');
      if (command === 'git_status') return Promise.resolve({ files: [] });
      if (command === 'explorer_watch') {
        return new Promise((resolve) => {
          resolveWatch = () => resolve(null);
        });
      }
      return Promise.resolve(null);
    });

    const { unmount } = render(<ExplorerPanel selectedAgentIds={new Set()} agents={[]} />);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('explorer_watch', { rootPath: 'C:\\Users\\test\\repo' });
    });

    unmount();

    expect(invoke).not.toHaveBeenCalledWith('explorer_unwatch', { rootPath: 'C:\\Users\\test\\repo' });

    await act(async () => {
      resolveWatch?.();
    });

    await waitFor(() => {
      expect(unlisten).toHaveBeenCalledTimes(1);
      expect(invoke).toHaveBeenCalledWith('explorer_unwatch', { rootPath: 'C:\\Users\\test\\repo' });
    });
  });

  it('refreshes the tree after deleting a file without remounting FileTree', async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'get_explorer_root') return 'C:\\Users\\test\\repo';
      if (command === 'git_status') return { files: [] };
      if (command === 'delete_file') return null;
      return null;
    });

    render(
      <ConfirmProvider>
        <ExplorerPanel selectedAgentIds={new Set()} agents={[]} />
      </ConfirmProvider>,
    );

    const tree = await screen.findByTestId('mock-file-row');
    await userEvent.pointer({ keys: '[MouseRight]', target: tree });
    await userEvent.click(await screen.findByRole('button', { name: 'Delete' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Confirm' }));

    expect(await screen.findByTestId('file-tree-refresh-token')).toHaveTextContent('1');
    expect(screen.getByTestId('file-tree-changed-paths')).toHaveTextContent('C:\\Users\\test\\repo\\notes.md');
  });
});
