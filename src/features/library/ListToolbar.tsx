import React, { useState } from 'react';

interface ListToolbarProps {
    /** Singular label of the active section's entry kind, e.g. "skill". */
    kindLabel: string;
    searchQuery: string;
    showStarredOnly: boolean;
    /** Folders are not supported in flat sections (classes). */
    canCreateFolder: boolean;
    onSearchChange: (query: string) => void;
    onToggleStarredOnly: () => void;
    onCreateItem: (name: string) => void;
    onCreateFolder: (name: string) => void;
    onReveal: () => void;
}

/**
 * Toolbar above the library list: search input, starred filter toggle, a
 * "New" split-button (new item / new folder via an inline name input), and a
 * Reveal-in-Explorer shortcut.
 */
export const ListToolbar: React.FC<ListToolbarProps> = ({
    kindLabel,
    searchQuery,
    showStarredOnly,
    canCreateFolder,
    onSearchChange,
    onToggleStarredOnly,
    onCreateItem,
    onCreateFolder,
    onReveal,
}) => {
    const [menuOpen, setMenuOpen] = useState(false);
    const [creating, setCreating] = useState<'item' | 'folder' | null>(null);
    const [newName, setNewName] = useState('');

    const startCreating = (mode: 'item' | 'folder') => {
        setMenuOpen(false);
        setCreating(mode);
        setNewName('');
    };

    const submitNewName = () => {
        const name = newName.trim();
        if (!name || !creating) return;
        if (creating === 'item') {
            onCreateItem(name);
        } else {
            onCreateFolder(name);
        }
        setCreating(null);
        setNewName('');
    };

    const handleNameKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === 'Enter') submitNewName();
        if (e.key === 'Escape') setCreating(null);
    };

    return (
        <div
            data-testid="library-toolbar"
            className="flex flex-col gap-2 px-3 py-2 border-b border-wardian-border bg-wardian-sidebar-primary"
        >
            <div className="flex items-center gap-2">
                <input
                    type="text"
                    data-testid="library-search"
                    value={searchQuery}
                    onChange={(e) => onSearchChange(e.target.value)}
                    placeholder={`Search ${kindLabel}s...`}
                    aria-label={`Search ${kindLabel}s`}
                    className="flex-1 min-w-0 bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-2 py-1 text-xs text-primary placeholder:text-muted-neutral focus:outline-none focus:border-[var(--color-wardian-accent)]"
                />
                <button
                    type="button"
                    data-testid="library-star-filter"
                    onClick={onToggleStarredOnly}
                    aria-pressed={showStarredOnly}
                    title={showStarredOnly ? 'Show all items' : 'Show starred only'}
                    className={`px-2 py-1 rounded border text-xs transition-colors ${
                        showStarredOnly
                            ? 'border-[var(--color-wardian-accent)] text-[var(--color-wardian-accent)] bg-wardian-card-bg-muted'
                            : 'border-wardian-border text-muted-neutral hover:text-primary'
                    }`}
                >
                    ★
                </button>
                <div className="relative">
                    <button
                        type="button"
                        data-testid="library-new"
                        onClick={() => setMenuOpen((open) => !open)}
                        aria-expanded={menuOpen}
                        aria-haspopup="menu"
                        className="px-2 py-1 rounded border border-wardian-border text-xs text-muted transition-colors hover:text-primary hover:bg-wardian-card-bg-muted"
                    >
                        New ▾
                    </button>
                    {menuOpen && (
                        <div
                            role="menu"
                            className="absolute right-0 top-full mt-1 z-10 min-w-[140px] rounded border border-wardian-border bg-wardian-sidebar-secondary shadow-wardian-card overflow-hidden"
                        >
                            <button
                                type="button"
                                role="menuitem"
                                data-testid="library-new-item"
                                onClick={() => startCreating('item')}
                                className="block w-full text-left px-3 py-1.5 text-xs text-primary transition-colors hover:bg-wardian-card-bg-muted"
                            >
                                New {kindLabel}
                            </button>
                            {canCreateFolder && (
                                <button
                                    type="button"
                                    role="menuitem"
                                    data-testid="library-new-folder"
                                    onClick={() => startCreating('folder')}
                                    className="block w-full text-left px-3 py-1.5 text-xs text-primary transition-colors hover:bg-wardian-card-bg-muted"
                                >
                                    New folder
                                </button>
                            )}
                        </div>
                    )}
                </div>
                <button
                    type="button"
                    data-testid="library-reveal"
                    onClick={onReveal}
                    title="Reveal in Explorer"
                    aria-label="Reveal in Explorer"
                    className="px-2 py-1 rounded border border-wardian-border text-xs text-muted-neutral transition-colors hover:text-primary hover:bg-wardian-card-bg-muted"
                >
                    ⌖
                </button>
            </div>
            {creating && (
                <div className="flex items-center gap-2">
                    <input
                        type="text"
                        autoFocus
                        data-testid="library-new-name"
                        value={newName}
                        onChange={(e) => setNewName(e.target.value)}
                        onKeyDown={handleNameKeyDown}
                        placeholder={creating === 'item' ? `New ${kindLabel} name` : 'New folder name'}
                        aria-label={creating === 'item' ? `New ${kindLabel} name` : 'New folder name'}
                        className="flex-1 min-w-0 bg-[var(--color-wardian-input-bg)] border border-[var(--color-wardian-accent)] rounded px-2 py-1 text-xs text-primary focus:outline-none"
                    />
                    <button
                        type="button"
                        data-testid="library-new-confirm"
                        onClick={submitNewName}
                        className="px-2 py-1 rounded border border-wardian-border text-xs text-muted transition-colors hover:text-primary hover:bg-wardian-card-bg-muted"
                    >
                        Create
                    </button>
                    <button
                        type="button"
                        data-testid="library-new-cancel"
                        onClick={() => setCreating(null)}
                        className="px-2 py-1 rounded border border-wardian-border text-xs text-muted-neutral transition-colors hover:text-primary"
                    >
                        Cancel
                    </button>
                </div>
            )}
        </div>
    );
};
