import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { LibraryList } from './LibraryList';
import { useLibraryStore } from '../../store/useLibraryStore';
import { LibraryEntry, LibraryIndex } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

function entry(overrides: Partial<LibraryEntry> & Pick<LibraryEntry, 'name' | 'path' | 'entry_ref'>): LibraryEntry {
  return {
    kind: 'skill',
    description: '',
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
    ...overrides,
  };
}

const emptyTree = { path: '', name: 'Root', children: [] };

const index: LibraryIndex = {
  sections: {
    skills: {
      stubbed: false,
      tree: {
        path: '',
        name: 'Root',
        children: [
          {
            path: 'dev',
            name: 'dev',
            children: [
              entry({
                name: 'planner',
                path: 'dev/planner',
                entry_ref: 'skills/dev/planner',
                description: 'Plans work ahead',
                tags: ['strategy'],
                is_starred: true,
                deployment_count: 2,
              }),
            ],
          },
          entry({ name: 'reviewer', path: 'reviewer', entry_ref: 'skills/reviewer', error: 'unreadable' }),
          entry({ name: 'ghost', path: 'ghost', entry_ref: 'skills/ghost' }),
        ],
      },
    },
    prompts: { stubbed: false, tree: emptyTree },
    workflows: { stubbed: false, tree: emptyTree },
    classes: { stubbed: false, tree: emptyTree },
    mcps: { stubbed: true, tree: emptyTree },
  },
  deployments: {},
  orphans: [{ target_type: 'user', target_id: 'global', skill_name: 'ghost' }],
};

describe('LibraryList', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined);
    useLibraryStore.setState({
      index,
      activeSection: 'skills',
      selection: null,
      expandedFolders: new Set<string>(),
      searchQuery: '',
      showStarredOnly: false,
      select: vi.fn().mockResolvedValue(undefined),
      updateMetadata: vi.fn().mockResolvedValue(undefined),
      renameEntry: vi.fn().mockResolvedValue(undefined),
      saveItem: vi.fn().mockResolvedValue(undefined),
      createFolder: vi.fn().mockResolvedValue(undefined),
      openLibraryFolder: vi.fn().mockResolvedValue(undefined),
      fetchIndex: vi.fn().mockResolvedValue(undefined),
    });
  });

  it('renders entry rows with name, description, tags, deployment badge, and star', () => {
    useLibraryStore.setState({ expandedFolders: new Set(['skills/dev']) });
    render(<LibraryList />);

    const row = screen.getByTestId('library-row-skills/dev/planner');
    expect(row).toHaveTextContent('planner');
    expect(row).toHaveTextContent('Plans work ahead');
    expect(row).toHaveTextContent('strategy');
    expect(screen.getByTestId('library-deploy-badge-skills/dev/planner')).toHaveTextContent('●2');
    expect(screen.getByTestId('library-star-skills/dev/planner')).toHaveAttribute('aria-pressed', 'true');
  });

  it('shows the warning badge for entries with an error or a related orphan', () => {
    render(<LibraryList />);
    expect(screen.getByTestId('library-warn-badge-skills/reviewer')).toBeInTheDocument();
    expect(screen.getByTestId('library-warn-badge-skills/ghost')).toBeInTheDocument();
  });

  it('collapses folders by default and expands them via the header toggle', () => {
    render(<LibraryList />);

    expect(screen.queryByTestId('library-row-skills/dev/planner')).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId('library-folder-dev'));
    expect(screen.getByTestId('library-row-skills/dev/planner')).toBeInTheDocument();
    expect(screen.getByTestId('library-folder-dev')).toHaveAttribute('aria-expanded', 'true');

    fireEvent.click(screen.getByTestId('library-folder-dev'));
    expect(screen.queryByTestId('library-row-skills/dev/planner')).not.toBeInTheDocument();
  });

  it('switches to a flat ranked list with path subtitles while searching', () => {
    render(<LibraryList />);

    fireEvent.change(screen.getByTestId('library-search'), { target: { value: 'planner' } });

    expect(screen.queryByTestId('library-folder-dev')).not.toBeInTheDocument();
    expect(screen.getByTestId('library-row-skills/dev/planner')).toBeInTheDocument();
    expect(screen.getByTestId('library-row-subtitle-skills/dev/planner')).toHaveTextContent('dev');
    expect(screen.queryByTestId('library-row-skills/reviewer')).not.toBeInTheDocument();
  });

  it('surfaces a starred entry inside a collapsed (default-state) folder as a flat row with a path subtitle', () => {
    // expandedFolders defaults to empty (all folders collapsed) via beforeEach.
    // Before the fix, flattenTree emitted no rows at all for "dev" while
    // collapsed, so the starred "planner" entry inside it was unreachable.
    render(<LibraryList />);

    expect(screen.queryByTestId('library-row-skills/dev/planner')).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId('library-star-filter'));

    expect(screen.getByTestId('library-row-skills/dev/planner')).toBeInTheDocument();
    expect(screen.getByTestId('library-row-subtitle-skills/dev/planner')).toHaveTextContent('dev');
    expect(screen.queryByTestId('library-row-skills/reviewer')).not.toBeInTheDocument();
    expect(screen.queryByTestId('library-list-empty')).not.toBeInTheDocument();
  });

  it('shows the empty starred state only when genuinely nothing is starred', () => {
    // Same tree as `index`, but the only starred entry ("planner") is un-starred.
    const noStarredIndex: LibraryIndex = {
      ...index,
      sections: {
        ...index.sections,
        skills: {
          stubbed: false,
          tree: {
            path: '',
            name: 'Root',
            children: [
              {
                path: 'dev',
                name: 'dev',
                children: [
                  entry({
                    name: 'planner',
                    path: 'dev/planner',
                    entry_ref: 'skills/dev/planner',
                    description: 'Plans work ahead',
                    tags: ['strategy'],
                    is_starred: false,
                    deployment_count: 2,
                  }),
                ],
              },
              entry({ name: 'reviewer', path: 'reviewer', entry_ref: 'skills/reviewer', error: 'unreadable' }),
              entry({ name: 'ghost', path: 'ghost', entry_ref: 'skills/ghost' }),
            ],
          },
        },
      },
    };
    useLibraryStore.setState({ index: noStarredIndex });
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-star-filter'));

    expect(screen.getByTestId('library-list-empty')).toHaveTextContent('No starred skills.');
    expect(screen.queryByTestId('library-row-skills/dev/planner')).not.toBeInTheDocument();
  });

  it('combines the star filter with an active search', () => {
    render(<LibraryList />);

    fireEvent.change(screen.getByTestId('library-search'), { target: { value: 'e' } });
    // "reviewer" and "planner" both match "e"; only planner is starred.
    expect(screen.getByTestId('library-row-skills/dev/planner')).toBeInTheDocument();
    expect(screen.getByTestId('library-row-skills/reviewer')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('library-star-filter'));

    expect(screen.getByTestId('library-row-skills/dev/planner')).toBeInTheDocument();
    expect(screen.queryByTestId('library-row-skills/reviewer')).not.toBeInTheDocument();
  });

  it('toggles the star through updateMetadata with entry_ref and flipped is_starred', () => {
    useLibraryStore.setState({ expandedFolders: new Set(['skills/dev']) });
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-star-skills/dev/planner'));

    expect(useLibraryStore.getState().updateMetadata).toHaveBeenCalledWith('skills/dev/planner', {
      id: 'skills/dev/planner',
      tags: ['strategy'],
      is_starred: false,
    });
  });

  it('sets the drag payload to the entry ref on drag start', () => {
    render(<LibraryList />);

    const setData = vi.fn();
    fireEvent.dragStart(screen.getByTestId('library-row-skills/reviewer'), {
      dataTransfer: { setData, effectAllowed: 'none' },
    });

    expect(setData).toHaveBeenCalledWith('text/wardian-entry-ref', 'skills/reviewer');
  });

  it('moves a dropped entry into the folder via renameEntry', () => {
    render(<LibraryList />);

    fireEvent.drop(screen.getByTestId('library-folder-dev'), {
      dataTransfer: { getData: vi.fn(() => 'skills/reviewer') },
    });

    expect(useLibraryStore.getState().renameEntry).toHaveBeenCalledWith('skills', 'reviewer', 'dev/reviewer');
  });

  it('treats a same-folder drop as a no-op', () => {
    render(<LibraryList />);

    fireEvent.drop(screen.getByTestId('library-folder-dev'), {
      dataTransfer: { getData: vi.fn(() => 'skills/dev/planner') },
    });

    expect(useLibraryStore.getState().renameEntry).not.toHaveBeenCalled();
  });

  it('ignores drops from another section', () => {
    render(<LibraryList />);

    fireEvent.drop(screen.getByTestId('library-folder-dev'), {
      dataTransfer: { getData: vi.fn(() => 'prompts/greet.md') },
    });

    expect(useLibraryStore.getState().renameEntry).not.toHaveBeenCalled();
  });

  it('selects an entry on row click', () => {
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-row-skills/reviewer'));

    expect(useLibraryStore.getState().select).toHaveBeenCalledWith('skills/reviewer');
  });

  it('activates a row via Enter or Space on the keyboard, preventing the default Space scroll', () => {
    render(<LibraryList />);
    const row = screen.getByTestId('library-row-skills/reviewer');

    fireEvent.keyDown(row, { key: 'Enter' });
    expect(useLibraryStore.getState().select).toHaveBeenCalledWith('skills/reviewer');

    vi.mocked(useLibraryStore.getState().select).mockClear();

    // fireEvent returns false when preventDefault() was called during
    // dispatch — Space's native default action on a focused role="button"
    // element is to scroll the nearest scrollable ancestor, so activation
    // must suppress it.
    const spaceEvent = fireEvent.keyDown(row, { key: ' ' });
    expect(useLibraryStore.getState().select).toHaveBeenCalledWith('skills/reviewer');
    expect(spaceEvent).toBe(false);
  });

  it('ignores keys other than Enter and Space', () => {
    render(<LibraryList />);
    const row = screen.getByTestId('library-row-skills/reviewer');

    fireEvent.keyDown(row, { key: 'Tab' });

    expect(useLibraryStore.getState().select).not.toHaveBeenCalled();
  });

  it('creates a new item with template content through the New menu', () => {
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-new'));
    fireEvent.click(screen.getByTestId('library-new-item'));
    fireEvent.change(screen.getByTestId('library-new-name'), { target: { value: 'tester' } });
    fireEvent.keyDown(screen.getByTestId('library-new-name'), { key: 'Enter' });

    expect(useLibraryStore.getState().saveItem).toHaveBeenCalledWith(
      'skills',
      'tester',
      expect.stringContaining('# tester'),
    );
  });

  it('appends .md for new prompt items and creates folders via createFolder', () => {
    useLibraryStore.setState({ activeSection: 'prompts' });
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-new'));
    fireEvent.click(screen.getByTestId('library-new-item'));
    fireEvent.change(screen.getByTestId('library-new-name'), { target: { value: 'greet' } });
    fireEvent.keyDown(screen.getByTestId('library-new-name'), { key: 'Enter' });
    expect(useLibraryStore.getState().saveItem).toHaveBeenCalledWith('prompts', 'greet.md', expect.any(String));

    fireEvent.click(screen.getByTestId('library-new'));
    fireEvent.click(screen.getByTestId('library-new-folder'));
    fireEvent.change(screen.getByTestId('library-new-name'), { target: { value: 'greetings' } });
    fireEvent.keyDown(screen.getByTestId('library-new-name'), { key: 'Enter' });
    expect(useLibraryStore.getState().createFolder).toHaveBeenCalledWith('prompts', 'greetings');
  });

  // FIX-NOW 1: the classes section's New flow must register the class via
  // `create_agent_class` (classes.json + provider stubs), never the generic
  // `save_library_item` write, which produced an unregistered "phantom"
  // class invisible to the spawn dropdown and undeletable from the UI.
  it('creates a new class through create_agent_class, not save_library_item, and refreshes the index', async () => {
    useLibraryStore.setState({ activeSection: 'classes' });
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-new'));
    fireEvent.click(screen.getByTestId('library-new-item'));
    fireEvent.change(screen.getByTestId('library-new-name'), { target: { value: 'Strategist' } });
    fireEvent.keyDown(screen.getByTestId('library-new-name'), { key: 'Enter' });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('create_agent_class', {
        name: 'Strategist',
        description: '',
        instructionContent: expect.stringContaining('Strategist'),
      }),
    );
    expect(useLibraryStore.getState().saveItem).not.toHaveBeenCalled();
    expect(mockInvoke).not.toHaveBeenCalledWith('save_library_item', expect.anything());
    await waitFor(() => expect(useLibraryStore.getState().fetchIndex).toHaveBeenCalled());
  });

  it('hides the new-folder action for the flat classes section', () => {
    useLibraryStore.setState({ activeSection: 'classes' });
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-new'));

    expect(screen.getByTestId('library-new-item')).toBeInTheDocument();
    expect(screen.queryByTestId('library-new-folder')).not.toBeInTheDocument();
  });

  it('reveals the section folder in the file explorer', () => {
    render(<LibraryList />);

    fireEvent.click(screen.getByTestId('library-reveal'));

    expect(useLibraryStore.getState().openLibraryFolder).toHaveBeenCalledWith('skills');
  });

  it('renders the MCP stub copy instead of rows for the mcps section', () => {
    useLibraryStore.setState({ activeSection: 'mcps' });
    render(<LibraryList />);

    expect(screen.getByTestId('library-mcp-stub')).toHaveTextContent('MCP servers are coming to the library');
    expect(screen.getByTestId('library-mcp-stub')).toHaveTextContent(
      'Define once, deploy to agents and classes — the same scoping skills use today.',
    );
    expect(screen.queryByTestId('library-toolbar')).not.toBeInTheDocument();
  });

  it('shows empty states for empty sections and empty search results', () => {
    useLibraryStore.setState({ activeSection: 'workflows' });
    render(<LibraryList />);
    expect(screen.getByTestId('library-list-empty')).toHaveTextContent('No workflows yet');

    fireEvent.change(screen.getByTestId('library-search'), { target: { value: 'nothing' } });
    expect(screen.getByTestId('library-list-empty')).toHaveTextContent('No matches');
  });

  it('pluralizes the classes section label correctly instead of naively appending "s"', () => {
    useLibraryStore.setState({ activeSection: 'classes' });
    render(<LibraryList />);

    // Regression test for the "Search classs..." / "No classs yet." bug:
    // "classes" is an irregular plural of "class" that naive `${kindLabel}s`
    // string concatenation gets wrong.
    expect(screen.getByTestId('library-search')).toHaveAttribute('placeholder', 'Search classes...');
    expect(screen.getByTestId('library-search')).toHaveAttribute('aria-label', 'Search classes');
    expect(screen.getByTestId('library-list-empty')).toHaveTextContent('No classes yet. Use New to create one.');
    expect(screen.getByTestId('library-list-empty')).not.toHaveTextContent('classs');
  });

  it('shows the starred-only empty state with the correct plural label', () => {
    useLibraryStore.setState({ activeSection: 'classes', showStarredOnly: true });
    render(<LibraryList />);

    expect(screen.getByTestId('library-list-empty')).toHaveTextContent('No starred classes.');
  });
});
