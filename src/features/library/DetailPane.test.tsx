import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { DetailPane } from './DetailPane';
import { useLibraryStore } from '../../store/useLibraryStore';
import { LibraryEntry, LibraryIndex } from '../../types';

const mockInvoke = vi.mocked(invoke);

function entry(overrides: Partial<LibraryEntry> & Pick<LibraryEntry, 'kind' | 'name' | 'path' | 'entry_ref'>): LibraryEntry {
  return {
    description: '',
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
    ...overrides,
  };
}

const emptyTree = { path: '', name: 'Root', children: [] };

function buildIndex(): LibraryIndex {
  return {
    sections: {
      skills: {
        stubbed: false,
        tree: {
          path: '',
          name: 'Root',
          children: [entry({ kind: 'skill', name: 'planner', path: 'planner', entry_ref: 'skills/planner' })],
        },
      },
      prompts: {
        stubbed: false,
        tree: {
          path: '',
          name: 'Root',
          children: [entry({ kind: 'prompt', name: 'greet', path: 'greet.md', entry_ref: 'prompts/greet.md' })],
        },
      },
      workflows: {
        stubbed: false,
        tree: {
          path: '',
          name: 'Root',
          children: [entry({ kind: 'workflow', name: 'triage', path: 'triage.md', entry_ref: 'workflows/triage.md' })],
        },
      },
      classes: {
        stubbed: false,
        tree: {
          path: '',
          name: 'Root',
          children: [entry({ kind: 'class', name: 'Architect', path: 'Architect', entry_ref: 'classes/Architect' })],
        },
      },
      mcps: { stubbed: true, tree: emptyTree },
    },
    deployments: {},
    orphans: [],
  };
}

function baseState(overrides: Partial<ReturnType<typeof useLibraryStore.getState>> = {}) {
  return {
    index: buildIndex(),
    activeSection: 'skills' as const,
    selection: null,
    selectedContent: null,
    contentStale: false,
    markEditorDirty: vi.fn(),
    select: vi.fn().mockResolvedValue(undefined),
    revertSelection: vi.fn(),
    resolveStale: vi.fn(),
    reloadSelectedContent: vi.fn().mockResolvedValue(undefined),
    saveItem: vi.fn().mockResolvedValue(undefined),
    updateMetadata: vi.fn().mockResolvedValue(undefined),
    renameEntry: vi.fn().mockResolvedValue(undefined),
    deleteEntry: vi.fn().mockResolvedValue(undefined),
    setSkillDeployments: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

describe('DetailPane', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async () => []);
    useLibraryStore.setState(baseState() as never, true);
  });

  it('shows an empty state when nothing is selected', () => {
    render(<DetailPane selectedAgentIds={new Set()} />);
    expect(screen.getByTestId('library-detail-empty')).toBeInTheDocument();
  });

  it('shows the MCP stub when the active section is mcps, regardless of selection', () => {
    useLibraryStore.setState({ activeSection: 'mcps' });
    render(<DetailPane selectedAgentIds={new Set()} />);
    expect(screen.getByTestId('mcp-stub-detail')).toBeInTheDocument();
  });

  it('renders SkillDetail for a skill selection', async () => {
    useLibraryStore.setState({
      selection: { section: 'skills', entryRef: 'skills/planner' },
      selectedContent: '# planner',
    });
    render(<DetailPane selectedAgentIds={new Set()} />);
    expect(await screen.findByTestId('skill-detail')).toBeInTheDocument();
  });

  it('renders PromptDetail for a prompt selection', async () => {
    useLibraryStore.setState({
      selection: { section: 'prompts', entryRef: 'prompts/greet.md' },
      selectedContent: '# greet',
    });
    render(<DetailPane selectedAgentIds={new Set()} />);
    expect(await screen.findByTestId('prompt-detail')).toBeInTheDocument();
  });

  it('renders WorkflowDetail for a workflow selection', async () => {
    useLibraryStore.setState({
      selection: { section: 'workflows', entryRef: 'workflows/triage.md' },
      selectedContent: '# triage',
    });
    render(<DetailPane selectedAgentIds={new Set()} />);
    expect(await screen.findByTestId('workflow-detail')).toBeInTheDocument();
  });

  it('renders ClassDetail for a class selection', async () => {
    useLibraryStore.setState({
      selection: { section: 'classes', entryRef: 'classes/Architect' },
      selectedContent: '# Role: Architect',
    });
    render(<DetailPane selectedAgentIds={new Set()} />);
    expect(await screen.findByTestId('class-detail')).toBeInTheDocument();
  });

  it('disables the prompt Run button when no agents are selected, and enables it once agents are selected', async () => {
    useLibraryStore.setState({
      selection: { section: 'prompts', entryRef: 'prompts/greet.md' },
      selectedContent: '# greet',
    });
    const { rerender } = render(<DetailPane selectedAgentIds={new Set()} />);

    expect(await screen.findByTestId('prompt-run-button')).toBeDisabled();

    rerender(<DetailPane selectedAgentIds={new Set(['agent-1'])} />);
    expect(screen.getByTestId('prompt-run-button')).not.toBeDisabled();
  });

  it('toggles star via updateMetadata from the shared header', async () => {
    const updateMetadata = vi.fn().mockResolvedValue(undefined);
    useLibraryStore.setState({
      selection: { section: 'skills', entryRef: 'skills/planner' },
      selectedContent: '# planner',
      updateMetadata,
    });
    render(<DetailPane selectedAgentIds={new Set()} />);

    const star = await screen.findByTestId('detail-star-toggle');
    star.click();

    expect(updateMetadata).toHaveBeenCalledWith('skills/planner', expect.objectContaining({ is_starred: true }));
  });

  // MAJOR 1: ClassDetail already owns a correct delete_agent_class flow;
  // the shared header's generic Delete (which always fails for classes,
  // since core rejects Classes-section deletes via delete_library_entry)
  // must not also render.
  it('does not render the generic header Delete button for a class selection', async () => {
    useLibraryStore.setState({
      selection: { section: 'classes', entryRef: 'classes/Architect' },
      selectedContent: '# Role: Architect',
    });
    render(<DetailPane selectedAgentIds={new Set()} />);

    await screen.findByTestId('class-detail');
    expect(screen.queryByTestId('detail-delete-button')).not.toBeInTheDocument();
  });

  it('still renders the generic header Delete button for non-class selections', async () => {
    useLibraryStore.setState({
      selection: { section: 'skills', entryRef: 'skills/planner' },
      selectedContent: '# planner',
    });
    render(<DetailPane selectedAgentIds={new Set()} />);

    expect(await screen.findByTestId('detail-delete-button')).toBeInTheDocument();
  });

  // MAJOR 2: Ctrl+S must not silently overwrite disk while the stale/
  // conflict bar is showing; the user has to resolve it via the bar first.
  describe('stale-guarded save', () => {
    it('Ctrl+S while stale does not call saveItem', async () => {
      const saveItem = vi.fn().mockResolvedValue(undefined);
      useLibraryStore.setState({
        selection: { section: 'skills', entryRef: 'skills/planner' },
        selectedContent: '# planner',
        contentStale: true,
        saveItem,
      });
      render(<DetailPane selectedAgentIds={new Set()} />);

      const textarea = await screen.findByTestId('markdown-editor-textarea');
      fireEvent.keyDown(textarea, { key: 's', ctrlKey: true });

      await Promise.resolve();
      expect(saveItem).not.toHaveBeenCalled();
    });

    it('"Keep mine" resolves the conflict so a subsequent Ctrl+S proceeds', async () => {
      const saveItem = vi.fn().mockResolvedValue(undefined);
      const resolveStale = vi.fn(() => useLibraryStore.setState({ contentStale: false }));
      useLibraryStore.setState({
        selection: { section: 'skills', entryRef: 'skills/planner' },
        selectedContent: '# planner',
        contentStale: true,
        saveItem,
        resolveStale,
      });
      render(<DetailPane selectedAgentIds={new Set()} />);

      fireEvent.click(await screen.findByRole('button', { name: 'Keep mine' }));
      expect(resolveStale).toHaveBeenCalledTimes(1);
      expect(useLibraryStore.getState().contentStale).toBe(false);

      const textarea = screen.getByTestId('markdown-editor-textarea');
      fireEvent.keyDown(textarea, { key: 's', ctrlKey: true });

      await waitFor(() => expect(saveItem).toHaveBeenCalledWith('skills', 'planner', '# planner'));
    });

    it('a successful save clears contentStale', async () => {
      const saveItem = vi.fn().mockResolvedValue(undefined);
      const resolveStale = vi.fn();
      useLibraryStore.setState({
        selection: { section: 'skills', entryRef: 'skills/planner' },
        selectedContent: '# planner',
        contentStale: false,
        saveItem,
        resolveStale,
      });
      render(<DetailPane selectedAgentIds={new Set()} />);

      const textarea = await screen.findByTestId('markdown-editor-textarea');
      fireEvent.keyDown(textarea, { key: 's', ctrlKey: true });

      await waitFor(() => expect(resolveStale).toHaveBeenCalledTimes(1));
      expect(saveItem).toHaveBeenCalledWith('skills', 'planner', '# planner');
    });

    it('Reload adopts on-disk content and clears the dirty draft', async () => {
      const reloadSelectedContent = vi.fn().mockImplementation(async () => {
        useLibraryStore.setState({ selectedContent: '# updated on disk', contentStale: false });
      });
      useLibraryStore.setState({
        selection: { section: 'skills', entryRef: 'skills/planner' },
        selectedContent: '# planner',
        contentStale: true,
        reloadSelectedContent,
      });
      render(<DetailPane selectedAgentIds={new Set()} />);

      fireEvent.click(await screen.findByRole('button', { name: 'Reload' }));

      await waitFor(() =>
        expect(screen.getByTestId('markdown-editor-textarea')).toHaveValue('# updated on disk'),
      );
    });
  });
});
