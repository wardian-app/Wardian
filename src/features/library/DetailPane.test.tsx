import { render, screen } from '@testing-library/react';
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
});
