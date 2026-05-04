import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { LibraryFolder, LibrarySkill } from '../../types';
import { LibraryGrid } from './LibraryGrid';

const skill = (name: string, path = name): LibrarySkill => ({
  type: 'Skill',
  path,
  name,
  description: `# ${name}`,
  content: `${name} content`,
  metadata: {
    id: path,
    tags: ['tag'],
    is_starred: false,
  },
});

describe('LibraryGrid', () => {
  it('sorts folders before items and sorts names within each group', () => {
    const folder: LibraryFolder = {
      type: 'Folder',
      path: '',
      name: 'root',
      children: [
        skill('Zulu'),
        { type: 'Folder', path: 'beta', name: 'Beta', children: [] },
        skill('Alpha'),
        { type: 'Folder', path: 'alpha', name: 'Alpha Folder', children: [] },
      ],
    };

    render(
      <LibraryGrid
        folder={folder}
        onItemClick={() => {}}
        onToggleStar={() => {}}
        onFolderClick={() => {}}
        onItemAction={() => {}}
      />,
    );

    const labels = screen.getAllByRole('heading', { level: 3 }).map((heading) => heading.textContent);
    expect(labels).toEqual(['Alpha Folder', 'Beta', 'Alpha', 'Zulu']);
  });

  it('routes folder, item action, edit, and star interactions to the right callbacks', async () => {
    const user = userEvent.setup();
    const item = skill('Planner');
    const childFolder: LibraryFolder = { type: 'Folder', path: 'plans', name: 'Plans', children: [] };
    const onFolderClick = vi.fn();
    const onItemAction = vi.fn();
    const onItemClick = vi.fn();
    const onToggleStar = vi.fn();

    render(
      <LibraryGrid
        folder={{ type: 'Folder', path: '', name: 'root', children: [item, childFolder] }}
        onItemClick={onItemClick}
        onToggleStar={onToggleStar}
        onFolderClick={onFolderClick}
        onItemAction={onItemAction}
      />,
    );

    await user.click(screen.getByText('Plans'));
    expect(onFolderClick).toHaveBeenCalledWith(childFolder);

    await user.click(screen.getByText('Planner'));
    expect(onItemAction).toHaveBeenCalledWith(item);

    await user.click(screen.getByTitle('Edit Item'));
    expect(onItemClick).toHaveBeenCalledWith(item);

    await user.click(screen.getByRole('button', { name: 'Toggle star for Planner' }));
    expect(onToggleStar).toHaveBeenCalledWith(item);
  });

  it('shows an empty-folder message', () => {
    render(
      <LibraryGrid
        folder={{ type: 'Folder', path: '', name: 'root', children: [] }}
        onItemClick={() => {}}
        onToggleStar={() => {}}
        onFolderClick={() => {}}
        onItemAction={() => {}}
      />,
    );

    expect(screen.getByText('This folder is empty.')).toBeInTheDocument();
  });
});
