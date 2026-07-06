import { describe, expect, it } from 'vitest';
import { filterStarred, flattenTree, folderKey, searchEntries, ListRow } from './libraryListUtils';
import { LibraryEntry, LibraryIndexFolder } from '../../types';

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

const tree: LibraryIndexFolder = {
  path: '',
  name: 'Root',
  children: [
    {
      path: 'dev',
      name: 'dev',
      children: [
        {
          path: 'dev/tools',
          name: 'tools',
          children: [
            entry({ name: 'linter', path: 'dev/tools/linter', entry_ref: 'skills/dev/tools/linter', is_starred: true }),
          ],
        },
        entry({
          name: 'planner',
          path: 'dev/planner',
          entry_ref: 'skills/dev/planner',
          description: 'Plans work ahead',
          tags: ['strategy'],
        }),
      ],
    },
    entry({ name: 'reviewer', path: 'reviewer', entry_ref: 'skills/reviewer', description: 'Reviews planner output' }),
  ],
};

describe('flattenTree', () => {
  it('renders collapsed folders as a single header row', () => {
    const rows = flattenTree(tree, 'skills', new Set());
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ type: 'folder-header', depth: 0, folderPath: 'dev' });
    expect(rows[1]).toMatchObject({ type: 'entry', depth: 0 });
    expect(rows[1].entry?.name).toBe('reviewer');
  });

  it('expands folders whose section-qualified key is in expandedFolders', () => {
    const rows = flattenTree(tree, 'skills', new Set([folderKey('skills', 'dev')]));
    expect(rows.map((r) => r.folderPath ?? r.entry?.name)).toEqual(['dev', 'dev/tools', 'planner', 'reviewer']);
    expect(rows[1]).toMatchObject({ type: 'folder-header', depth: 1, folderPath: 'dev/tools' });
    expect(rows[2]).toMatchObject({ type: 'entry', depth: 1 });
  });

  it('does not expand for keys qualified with another section', () => {
    const rows = flattenTree(tree, 'skills', new Set([folderKey('prompts', 'dev')]));
    expect(rows).toHaveLength(2);
  });

  it('recurses into nested expanded folders with increasing depth', () => {
    const expanded = new Set([folderKey('skills', 'dev'), folderKey('skills', 'dev/tools')]);
    const rows = flattenTree(tree, 'skills', expanded);
    const linter = rows.find((r) => r.entry?.name === 'linter');
    expect(linter).toMatchObject({ type: 'entry', depth: 2 });
  });
});

describe('searchEntries', () => {
  it('returns no rows for an empty or whitespace query', () => {
    expect(searchEntries(tree, '')).toEqual([]);
    expect(searchEntries(tree, '   ')).toEqual([]);
  });

  it('ranks name matches above description matches', () => {
    const rows = searchEntries(tree, 'planner');
    expect(rows.map((r) => r.entry?.name)).toEqual(['planner', 'reviewer']);
  });

  it('matches tags with the lowest rank', () => {
    const rows = searchEntries(tree, 'strategy');
    expect(rows).toHaveLength(1);
    expect(rows[0].entry?.name).toBe('planner');
  });

  it('emits flat entry rows with the parent folder as pathSubtitle', () => {
    const rows = searchEntries(tree, 'linter');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ type: 'entry', depth: 0, pathSubtitle: 'dev/tools' });
  });

  it('uses an empty pathSubtitle for root-level entries', () => {
    const rows = searchEntries(tree, 'reviewer');
    expect(rows[0].pathSubtitle).toBe('');
  });

  it('excludes entries that match nothing and is case-insensitive', () => {
    expect(searchEntries(tree, 'zzz-no-match')).toEqual([]);
    expect(searchEntries(tree, 'PLANNER')[0].entry?.name).toBe('planner');
  });
});

describe('filterStarred', () => {
  it('keeps starred entries and their ancestor folder headers', () => {
    const expanded = new Set([folderKey('skills', 'dev'), folderKey('skills', 'dev/tools')]);
    const rows = filterStarred(flattenTree(tree, 'skills', expanded));
    expect(rows.map((r) => r.folderPath ?? r.entry?.name)).toEqual(['dev', 'dev/tools', 'linter']);
  });

  it('drops folder headers without starred descendants in their span', () => {
    const rows: ListRow[] = [
      { type: 'folder-header', depth: 0, folderPath: 'empty' },
      { type: 'entry', depth: 1, entry: entry({ name: 'a', path: 'empty/a', entry_ref: 'skills/empty/a' }) },
      { type: 'entry', depth: 0, entry: entry({ name: 'b', path: 'b', entry_ref: 'skills/b', is_starred: true }) },
    ];
    const filtered = filterStarred(rows);
    expect(filtered.map((r) => r.folderPath ?? r.entry?.name)).toEqual(['b']);
  });

  it('returns an empty list when no starred entry row is present', () => {
    // linter (the only starred entry) is hidden inside collapsed folders, so
    // no starred entry row exists in the flattened output.
    const collapsed = flattenTree(tree, 'skills', new Set());
    expect(filterStarred(collapsed)).toEqual([]);
  });
});
