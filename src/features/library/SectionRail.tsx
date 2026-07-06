import React from 'react';
import { LibraryIndex, LibraryIndexFolder, LibrarySectionId, isLibraryEntry } from '../../types';

/** Static metadata for the five library sections, in rail display order. */
export const LIBRARY_SECTIONS: { id: LibrarySectionId; label: string; kindLabel: string }[] = [
    { id: 'skills', label: 'Skills', kindLabel: 'skill' },
    { id: 'prompts', label: 'Prompts', kindLabel: 'prompt' },
    { id: 'classes', label: 'Classes', kindLabel: 'class' },
    { id: 'workflows', label: 'Workflows', kindLabel: 'workflow' },
    { id: 'mcps', label: 'MCPs', kindLabel: 'MCP server' },
];

/** Recursively counts entries (not folders) under a section's tree. */
function countEntries(folder: LibraryIndexFolder): number {
    let count = 0;
    for (const child of folder.children) {
        count += isLibraryEntry(child) ? 1 : countEntries(child);
    }
    return count;
}

interface SectionRailProps {
    activeSection: LibrarySectionId;
    sections: LibraryIndex['sections'] | null;
    onSelect: (s: LibrarySectionId) => void;
}

/**
 * Slim vertical strip inside LibraryView for switching between library
 * sections (skills/prompts/classes/workflows/mcps). This is intentionally
 * scoped to the library view — it does not touch the global left sidebar.
 */
export const SectionRail: React.FC<SectionRailProps> = ({ activeSection, sections, onSelect }) => {
    return (
        <div
            data-testid="library-section-rail"
            className="w-14 flex-shrink-0 border-r border-wardian-border bg-wardian-sidebar-primary flex flex-col items-stretch overflow-y-auto"
        >
            {LIBRARY_SECTIONS.map((section) => {
                const count = sections ? countEntries(sections[section.id].tree) : 0;
                return (
                    <button
                        key={section.id}
                        data-testid={`library-section-${section.id}`}
                        onClick={() => onSelect(section.id)}
                        title={section.label}
                        aria-current={activeSection === section.id ? 'true' : undefined}
                        className={`flex flex-col items-center gap-1 py-3 w-full border-l-2 transition-colors ${
                            activeSection === section.id
                                ? 'border-[var(--color-wardian-accent)] text-primary bg-wardian-sidebar-primary'
                                : 'border-transparent text-muted hover:text-primary'
                        }`}
                    >
                        <span className="label-small">{section.label}</span>
                        {count > 0 && <span className="text-[10px] text-muted-neutral">{count}</span>}
                    </button>
                );
            })}
        </div>
    );
};
