import { LibraryEntry, LibraryIndexFolder, LibrarySectionId, isLibraryEntry } from '../../types';

/**
 * One renderable row of the library list. Browse mode interleaves
 * folder-header and entry rows with indentation depth; search mode and the
 * starred filter (both queries rather than browse modes — see
 * `flattenAllEntries`) emit a flat list of entry rows carrying a
 * `pathSubtitle` (the parent folder), since folder-header rows may be absent.
 */
export interface ListRow {
    type: 'folder-header' | 'entry';
    depth: number;
    folderPath?: string; // for folder-header
    entry?: LibraryEntry; // for entry
    pathSubtitle?: string; // parent folder path, shown in search mode
}

/**
 * Key used in the store's `expandedFolders` set. Section-qualified so the
 * same folder path in two sections cannot collide.
 */
export function folderKey(section: LibrarySectionId, folderPath: string): string {
    return `${section}/${folderPath}`;
}

/** Browse mode: hierarchical rows honoring expandedFolders. */
export function flattenTree(
    tree: LibraryIndexFolder,
    section: LibrarySectionId,
    expandedFolders: Set<string>,
): ListRow[] {
    const rows: ListRow[] = [];
    const walk = (folder: LibraryIndexFolder, depth: number) => {
        for (const child of folder.children) {
            if (isLibraryEntry(child)) {
                rows.push({ type: 'entry', depth, entry: child });
            } else {
                rows.push({ type: 'folder-header', depth, folderPath: child.path });
                if (expandedFolders.has(folderKey(section, child.path))) {
                    walk(child, depth + 1);
                }
            }
        }
    };
    walk(tree, 0);
    return rows;
}

/**
 * Flat entry rows (no folder headers) with `pathSubtitle` set to the parent
 * folder path, for every entry in the tree — ignoring collapse state
 * entirely. Shared by search mode and the starred filter: both are queries
 * over the whole tree, not a browse of the user's current expansion state.
 */
export function flattenAllEntries(tree: LibraryIndexFolder): ListRow[] {
    const rows: ListRow[] = [];
    const walk = (folder: LibraryIndexFolder, parentPath: string) => {
        for (const child of folder.children) {
            if (isLibraryEntry(child)) {
                rows.push({ type: 'entry', depth: 0, entry: child, pathSubtitle: parentPath });
            } else {
                walk(child, child.path);
            }
        }
    };
    walk(tree, '');
    return rows;
}

/** Match rank: lower sorts first. -1 = no match. */
function rankEntry(entry: LibraryEntry, query: string): number {
    if (entry.name.toLowerCase().includes(query)) return 0;
    if (entry.description.toLowerCase().includes(query)) return 1;
    if (entry.tags.some((tag) => tag.toLowerCase().includes(query))) return 2;
    return -1;
}

/** Search mode: rank name > description > tags; flat entry rows with pathSubtitle. */
export function searchEntries(tree: LibraryIndexFolder, query: string): ListRow[] {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const matches: { row: ListRow; rank: number; name: string }[] = [];
    for (const row of flattenAllEntries(tree)) {
        const rank = row.entry ? rankEntry(row.entry, q) : -1;
        if (rank >= 0 && row.entry) {
            matches.push({ row, rank, name: row.entry.name });
        }
    }
    matches.sort((a, b) => a.rank - b.rank || a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }));
    return matches.map((m) => m.row);
}

/**
 * Starred filter over already-flattened rows: keeps starred entries plus any
 * folder header that still has a kept (starred) entry inside its span.
 */
export function filterStarred(rows: ListRow[]): ListRow[] {
    const keep = rows.map((row) => row.type === 'entry' && row.entry?.is_starred === true);
    for (let i = 0; i < rows.length; i++) {
        if (rows[i].type !== 'folder-header') continue;
        for (let j = i + 1; j < rows.length && rows[j].depth > rows[i].depth; j++) {
            if (keep[j] && rows[j].type === 'entry') {
                keep[i] = true;
                break;
            }
        }
    }
    return rows.filter((_, i) => keep[i]);
}
