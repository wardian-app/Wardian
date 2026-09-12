import React, { useState } from 'react';
import { FolderOpen, Star, Plus, ChevronDown, Search } from 'lucide-react';

interface ListToolbarProps {
    /** Singular label of the active section's entry kind, e.g. "skill". */
    kindLabel: string;
    /** Explicit plural label of the active section's entry kind, e.g. "classes". */
    kindLabelPlural: string;
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
 * "New" split-button (new item / new folder via an inline name input), and an
 * "Open in local file system" shortcut matching the icon/wording used for the
 * same action elsewhere in the app (see ExplorerPanel's root-reveal button).
 */
export const ListToolbar: React.FC<ListToolbarProps> = ({
    kindLabel,
    kindLabelPlural,
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
            className="flex flex-col gap-3 p-3 border-b border-wardian-border bg-wardian-sidebar-primary"
        >
            <div className="flex items-center gap-2">
                <label className="relative flex flex-1 min-w-0 items-center">
                <Search size={14} className="absolute left-2.5 text-muted-neutral" aria-hidden="true" />
                <input
                    type="text"
                    data-testid="library-search"
                    value={searchQuery}
                    onChange={(e) => onSearchChange(e.target.value)}
                    placeholder={`Search ${kindLabelPlural}...`}
                    aria-label={`Search ${kindLabelPlural}`}
                    className="w-full min-w-0 h-8 bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-md pl-8 pr-2 text-xs text-primary placeholder:text-muted-neutral focus:outline-none focus:border-[var(--color-wardian-accent)]"
                />
                </label>
                <button
                    type="button"
                    data-testid="library-star-filter"
                    onClick={onToggleStarredOnly}
                    aria-pressed={showStarredOnly}
                    title={showStarredOnly ? 'Show all items' : 'Show starred only'}
                    aria-label="Show starred only"
                    className={`wardian-icon-button shrink-0 ${
                        showStarredOnly
                            ? 'border-[var(--color-wardian-accent)] text-[var(--color-wardian-accent)] bg-wardian-card-bg-muted'
                            : 'border-wardian-border text-muted-neutral hover:text-primary'
                    }`}
                >
                    <Star size={15} fill={showStarredOnly ? 'currentColor' : 'none'} aria-hidden="true" />
                </button>
                <div className="relative">
                    <button
                        type="button"
                        data-testid="library-new"
                        onClick={() => setMenuOpen((open) => !open)}
                        aria-expanded={menuOpen}
                        aria-haspopup="menu"
                        className="wardian-button wardian-button--primary gap-1"
                    >
                        <Plus size={14} aria-hidden="true" /> New <ChevronDown size={12} aria-hidden="true" />
                    </button>
                    {menuOpen && (
                        <div
                            role="menu"
                            className="wardian-menu absolute right-0 top-full mt-1 z-10 min-w-[140px] overflow-hidden"
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
                    title="Open in local file system"
                    aria-label="Open in local file system"
                    className="wardian-icon-button wardian-icon-button--secondary shrink-0"
                >
                    <FolderOpen aria-hidden="true" size={14} strokeWidth={2} />
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
                        className="wardian-button wardian-button--primary"
                    >
                        Create
                    </button>
                    <button
                        type="button"
                        data-testid="library-new-cancel"
                        onClick={() => setCreating(null)}
                        className="wardian-button wardian-button--secondary"
                    >
                        Cancel
                    </button>
                </div>
            )}
        </div>
    );
};
