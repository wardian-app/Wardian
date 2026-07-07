import React from 'react';
import { useLibraryStore } from '../../store/useLibraryStore';
import { LibraryEntry, LibraryItemMetadata, LibrarySectionId, OrphanDeployment } from '../../types';
import { ListToolbar } from './ListToolbar';
import { LIBRARY_SECTIONS } from './SectionRail';
import { filterStarred, flattenAllEntries, flattenTree, folderKey, searchEntries, ListRow } from './libraryListUtils';

const ENTRY_REF_MIME = 'text/wardian-entry-ref';

/** Sections whose content files live at `<name>.md` rather than `<name>/`. */
const MD_FILE_SECTIONS: LibrarySectionId[] = ['prompts', 'workflows'];

function newItemTemplate(name: string, kindLabel: string): string {
    return `# ${name}\n\nDescribe this ${kindLabel} here.\n`;
}

interface ListRowItemProps {
    row: ListRow;
    selected: boolean;
    hasRelatedOrphan: boolean;
    onSelect: (entry: LibraryEntry) => void;
    onToggleStar: (entry: LibraryEntry) => void;
    onDragStart: (e: React.DragEvent, entry: LibraryEntry) => void;
}

/** One dense entry row: name, truncated description, tag chips, badges, star. */
const ListRowItem: React.FC<ListRowItemProps> = ({
    row,
    selected,
    hasRelatedOrphan,
    onSelect,
    onToggleStar,
    onDragStart,
}) => {
    const entry = row.entry;
    if (!entry) return null;
    const showWarning = Boolean(entry.error) || hasRelatedOrphan;

    return (
        <div
            data-testid={`library-row-${entry.entry_ref}`}
            role="button"
            tabIndex={0}
            draggable
            aria-current={selected ? 'true' : undefined}
            onDragStart={(e) => onDragStart(e, entry)}
            onClick={() => onSelect(entry)}
            onKeyDown={(e) => e.key === 'Enter' && onSelect(entry)}
            style={{ paddingLeft: `${12 + row.depth * 16}px` }}
            className={`flex items-center gap-2 pr-3 py-1.5 border-b border-wardian-border cursor-pointer transition-colors ${
                selected ? 'bg-wardian-card-bg-muted' : 'hover:bg-wardian-card-bg-muted'
            }`}
        >
            <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 min-w-0">
                    <span className="text-xs font-medium text-primary truncate">{entry.name}</span>
                    {row.pathSubtitle !== undefined && row.pathSubtitle !== '' && (
                        <span
                            data-testid={`library-row-subtitle-${entry.entry_ref}`}
                            className="text-[10px] text-muted-neutral truncate"
                        >
                            {row.pathSubtitle}
                        </span>
                    )}
                </div>
                {entry.description && <p className="text-[11px] text-muted truncate">{entry.description}</p>}
                {entry.tags.length > 0 && (
                    <div className="flex flex-wrap gap-1 mt-0.5">
                        {entry.tags.map((tag) => (
                            <span
                                key={tag}
                                className="px-1.5 py-px rounded border border-wardian-border text-[10px] text-[var(--color-wardian-accent)]"
                            >
                                {tag}
                            </span>
                        ))}
                    </div>
                )}
            </div>
            {entry.deployment_count > 0 && (
                <span
                    data-testid={`library-deploy-badge-${entry.entry_ref}`}
                    title={`Deployed to ${entry.deployment_count} target(s)`}
                    className="shrink-0 text-[10px] text-[var(--color-wardian-success)]"
                >
                    ●{entry.deployment_count}
                </span>
            )}
            {showWarning && (
                <span
                    data-testid={`library-warn-badge-${entry.entry_ref}`}
                    title={entry.error ?? 'A deployed copy of this skill has drifted from its source'}
                    className="shrink-0 text-[10px] text-[var(--color-wardian-warning)]"
                >
                    ⚠
                </span>
            )}
            <button
                type="button"
                data-testid={`library-star-${entry.entry_ref}`}
                aria-label={`Toggle star for ${entry.name}`}
                aria-pressed={entry.is_starred}
                onClick={(e) => {
                    e.stopPropagation();
                    onToggleStar(entry);
                }}
                className={`shrink-0 text-sm leading-none transition-colors ${
                    entry.is_starred ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral hover:text-primary'
                }`}
            >
                {entry.is_starred ? '★' : '☆'}
            </button>
        </div>
    );
};

function relatedOrphan(entry: LibraryEntry, orphans: OrphanDeployment[]): boolean {
    return entry.kind === 'skill' && orphans.some((o) => o.skill_name === entry.name);
}

/**
 * Detailed list for the active library section: toolbar + collapsible folder
 * groups in browse mode, a flat ranked list in search mode. Rows drag into
 * folder headers to move entries (a move is a rename to the new folder path).
 */
export const LibraryList: React.FC = () => {
    const index = useLibraryStore((s) => s.index);
    const activeSection = useLibraryStore((s) => s.activeSection);
    const selection = useLibraryStore((s) => s.selection);
    const expandedFolders = useLibraryStore((s) => s.expandedFolders);
    const searchQuery = useLibraryStore((s) => s.searchQuery);
    const showStarredOnly = useLibraryStore((s) => s.showStarredOnly);
    const toggleFolder = useLibraryStore((s) => s.toggleFolder);
    const setSearchQuery = useLibraryStore((s) => s.setSearchQuery);
    const setShowStarredOnly = useLibraryStore((s) => s.setShowStarredOnly);
    const select = useLibraryStore((s) => s.select);
    const updateMetadata = useLibraryStore((s) => s.updateMetadata);
    const renameEntry = useLibraryStore((s) => s.renameEntry);
    const saveItem = useLibraryStore((s) => s.saveItem);
    const createFolder = useLibraryStore((s) => s.createFolder);
    const openLibraryFolder = useLibraryStore((s) => s.openLibraryFolder);

    const sectionMeta = LIBRARY_SECTIONS.find((s) => s.id === activeSection);
    const kindLabel = sectionMeta?.kindLabel ?? 'item';
    const kindLabelPlural = sectionMeta?.kindLabelPlural ?? 'items';

    if (activeSection === 'mcps') {
        return (
            <div data-testid="library-mcp-stub" className="h-full flex flex-col items-center justify-center gap-2 px-6 text-center">
                <h3 className="text-sm font-medium text-primary">MCP servers are coming to the library</h3>
                <p className="text-xs text-muted">
                    Define once, deploy to agents and classes — the same scoping skills use today.
                </p>
            </div>
        );
    }

    const tree = index?.sections[activeSection]?.tree ?? null;
    const searching = searchQuery.trim() !== '';
    // Starred filtering is a query over the whole tree, not a browse of the
    // user's current expansion state — folders default to collapsed, and a
    // collapsed folder emits no rows at all in flattenTree, which would hide
    // starred entries inside it. Fall back to the same fully-flat traversal
    // search mode uses so collapse state can't hide a starred entry.
    let rows: ListRow[] = tree
        ? searching
            ? searchEntries(tree, searchQuery)
            : showStarredOnly
              ? flattenAllEntries(tree)
              : flattenTree(tree, activeSection, expandedFolders)
        : [];
    if (showStarredOnly) rows = filterStarred(rows);

    const handleToggleStar = (entry: LibraryEntry) => {
        // The index does not expose the metadata id; the entry_ref is the
        // stable unique key the backend stores metadata under, so it doubles
        // as the id here.
        const metadata: LibraryItemMetadata = {
            id: entry.entry_ref,
            tags: entry.tags,
            is_starred: !entry.is_starred,
        };
        void updateMetadata(entry.entry_ref, metadata);
    };

    const handleDragStart = (e: React.DragEvent, entry: LibraryEntry) => {
        e.dataTransfer.setData(ENTRY_REF_MIME, entry.entry_ref);
        e.dataTransfer.effectAllowed = 'move';
    };

    const handleDropOnFolder = (e: React.DragEvent, folderPath: string) => {
        e.preventDefault();
        const entryRef = e.dataTransfer.getData(ENTRY_REF_MIME);
        if (!entryRef) return;
        const [section, ...rest] = entryRef.split('/');
        const fromPath = rest.join('/');
        if (section !== activeSection || !fromPath) return;
        const name = fromPath.split('/').pop() ?? fromPath;
        const toPath = folderPath ? `${folderPath}/${name}` : name;
        if (toPath === fromPath) return; // same-folder drop is a no-op
        void renameEntry(activeSection, fromPath, toPath);
    };

    const handleCreateItem = (name: string) => {
        const path = MD_FILE_SECTIONS.includes(activeSection) ? `${name}.md` : name;
        void saveItem(activeSection, path, newItemTemplate(name, kindLabel));
    };

    return (
        <div data-testid="library-list-content" className="h-full flex flex-col min-h-0">
            <ListToolbar
                kindLabel={kindLabel}
                kindLabelPlural={kindLabelPlural}
                searchQuery={searchQuery}
                showStarredOnly={showStarredOnly}
                canCreateFolder={activeSection !== 'classes'}
                onSearchChange={setSearchQuery}
                onToggleStarredOnly={() => setShowStarredOnly(!showStarredOnly)}
                onCreateItem={handleCreateItem}
                onCreateFolder={(name) => void createFolder(activeSection, name)}
                onReveal={() => void openLibraryFolder(activeSection)}
            />
            <div className="flex-1 overflow-y-auto">
                {rows.length === 0 ? (
                    <div data-testid="library-list-empty" className="px-4 py-8 text-center text-xs text-muted">
                        {searching
                            ? 'No matches. Try a different search.'
                            : showStarredOnly
                              ? `No starred ${kindLabelPlural}.`
                              : `No ${kindLabelPlural} yet. Use New to create one.`}
                    </div>
                ) : (
                    rows.map((row) =>
                        row.type === 'folder-header' ? (
                            <button
                                type="button"
                                key={`folder:${row.folderPath}`}
                                data-testid={`library-folder-${row.folderPath}`}
                                onClick={() => toggleFolder(folderKey(activeSection, row.folderPath ?? ''))}
                                onDragOver={(e) => e.preventDefault()}
                                onDrop={(e) => handleDropOnFolder(e, row.folderPath ?? '')}
                                aria-expanded={expandedFolders.has(folderKey(activeSection, row.folderPath ?? ''))}
                                style={{ paddingLeft: `${12 + row.depth * 16}px` }}
                                className="flex items-center gap-1.5 w-full pr-3 py-1 border-b border-wardian-border text-left text-xs text-muted transition-colors hover:text-primary hover:bg-wardian-card-bg-muted"
                            >
                                <span className="text-[10px] text-muted-neutral">
                                    {expandedFolders.has(folderKey(activeSection, row.folderPath ?? '')) ? '▾' : '▸'}
                                </span>
                                <span className="truncate">{(row.folderPath ?? '').split('/').pop()}</span>
                            </button>
                        ) : (
                            <ListRowItem
                                key={row.entry?.entry_ref}
                                row={row}
                                selected={selection?.entryRef === row.entry?.entry_ref}
                                hasRelatedOrphan={row.entry ? relatedOrphan(row.entry, index?.orphans ?? []) : false}
                                onSelect={(entry) => void select(entry.entry_ref)}
                                onToggleStar={handleToggleStar}
                                onDragStart={handleDragStart}
                            />
                        ),
                    )
                )}
            </div>
        </div>
    );
};
