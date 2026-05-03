import { act, render, screen, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ManageSkills } from './ManageSkills';
import { useLibraryStore } from '../../store/useLibraryStore';
import type { LibraryFolder } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

const skillTree = (...names: string[]): LibraryFolder => ({
  type: 'Folder',
  path: '',
  name: 'skills',
  children: names.map((name) => ({
    type: 'Skill',
    path: name,
    name,
    description: `# ${name}`,
    content: `# ${name}`,
    metadata: {
      id: name,
      tags: [],
      is_starred: false,
    },
  })),
});

describe('ManageSkills', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListen.mockResolvedValue(vi.fn());
    useLibraryStore.setState({
      promptTree: null,
      skillTree: null,
      isLoading: false,
      error: null,
      activeTab: 'prompts',
    });
  });

  it('fetches a fresh skill tree on mount even when cached skills exist', async () => {
    useLibraryStore.setState({ skillTree: skillTree('old-skill') });
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'get_library_tree') return skillTree('new-skill');
      if (command === 'list_deployed_skill_refs') return [];
      return undefined;
    });

    render(<ManageSkills targetType="agent" targetId="agent-1" />);

    expect(screen.getByRole('option', { name: 'old-skill' })).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole('option', { name: 'new-skill' })).toBeInTheDocument();
    });
    expect(screen.queryByRole('option', { name: 'old-skill' })).not.toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith('library_watch', { libraryType: 'skills' });
    expect(mockInvoke).toHaveBeenCalledWith('get_library_tree', { libraryType: 'skills' });
  });

  it('refreshes deployed skills when the skill tree changes', async () => {
    useLibraryStore.setState({ skillTree: skillTree('alpha') });
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_deployed_skill_refs') {
        const calls = mockInvoke.mock.calls.filter(([name]) => name === 'list_deployed_skill_refs');
        return calls.length === 1
          ? [{ name: 'alpha', source_path: 'alpha' }]
          : [{ name: 'beta', source_path: 'beta' }];
      }
      if (command === 'get_library_tree') return skillTree('alpha');
      return undefined;
    });

    render(<ManageSkills targetType="agent" targetId="agent-1" />);

    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === 'list_deployed_skill_refs').length).toBeGreaterThanOrEqual(1);
    });

    act(() => {
      useLibraryStore.setState({ skillTree: skillTree('alpha', 'beta') });
    });

    await waitFor(() => {
      expect(screen.getByText('beta')).toBeInTheDocument();
    });
    expect(mockInvoke.mock.calls.filter(([command]) => command === 'list_deployed_skill_refs').length).toBeGreaterThanOrEqual(2);
  });

  it('does not hide duplicate skill names unless the exact source path is deployed', async () => {
    useLibraryStore.setState({
      skillTree: {
        type: 'Folder',
        path: '',
        name: 'skills',
        children: [
          {
            type: 'Folder',
            path: 'group-a',
            name: 'group-a',
            children: [{
              type: 'Skill',
              path: 'group-a/planner',
              name: 'planner',
              description: '# planner',
              content: '# planner',
              metadata: { id: 'a', tags: [], is_starred: false },
            }],
          },
          {
            type: 'Folder',
            path: 'group-b',
            name: 'group-b',
            children: [{
              type: 'Skill',
              path: 'group-b/planner',
              name: 'planner',
              description: '# planner',
              content: '# planner',
              metadata: { id: 'b', tags: [], is_starred: false },
            }],
          },
        ],
      },
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_deployed_skill_refs') {
        return [{ name: 'planner', source_path: 'group-b/planner' }];
      }
      if (command === 'get_library_tree') return useLibraryStore.getState().skillTree;
      return undefined;
    });

    render(<ManageSkills targetType="agent" targetId="agent-1" />);

    expect(await screen.findByRole('option', { name: 'planner (group-a/planner)' })).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'planner (group-b/planner)' })).not.toBeInTheDocument();
  });

  it('hides duplicate skill names when an existing copied deployment has no source identity', async () => {
    useLibraryStore.setState({
      skillTree: {
        type: 'Folder',
        path: '',
        name: 'skills',
        children: [
          {
            type: 'Folder',
            path: 'group-a',
            name: 'group-a',
            children: [{
              type: 'Skill',
              path: 'group-a/planner',
              name: 'planner',
              description: '# planner',
              content: '# planner',
              metadata: { id: 'a', tags: [], is_starred: false },
            }],
          },
          {
            type: 'Folder',
            path: 'group-b',
            name: 'group-b',
            children: [{
              type: 'Skill',
              path: 'group-b/planner',
              name: 'planner',
              description: '# planner',
              content: '# planner',
              metadata: { id: 'b', tags: [], is_starred: false },
            }],
          },
        ],
      },
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_deployed_skill_refs') {
        return [{ name: 'planner', source_path: null }];
      }
      if (command === 'get_library_tree') return useLibraryStore.getState().skillTree;
      return undefined;
    });

    render(<ManageSkills targetType="agent" targetId="agent-1" />);

    await waitFor(() => {
      expect(screen.queryByRole('option', { name: 'planner (group-a/planner)' })).not.toBeInTheDocument();
      expect(screen.queryByRole('option', { name: 'planner (group-b/planner)' })).not.toBeInTheDocument();
    });
  });
});
