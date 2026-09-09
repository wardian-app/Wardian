import React from 'react';
import { Puzzle, MessageSquare, Layers, Workflow, Plug } from 'lucide-react';
import { LibraryIndex, LibraryIndexFolder, LibrarySectionId, isLibraryEntry } from '../../types';

/**
 * Static metadata for the five library sections, in rail display order.
 *
 * `kindLabel` is the singular form of the entry kind (used in "New skill",
 * "Describe this skill here."); `kindLabelPlural` is an explicit plural form
 * — not derived by naively appending "s" — since that breaks for "class"
 * ("classs") and would be wrong for irregular plurals in general.
 */
export const LIBRARY_SECTIONS: { id: LibrarySectionId; label: string; kindLabel: string; kindLabelPlural: string }[] = [
    { id: 'skills', label: 'Skills', kindLabel: 'skill', kindLabelPlural: 'skills' },
    { id: 'prompts', label: 'Prompts', kindLabel: 'prompt', kindLabelPlural: 'prompts' },
    { id: 'classes', label: 'Classes', kindLabel: 'class', kindLabelPlural: 'classes' },
    { id: 'automations', label: 'Automations', kindLabel: 'automation', kindLabelPlural: 'automations' },
    { id: 'mcps', label: 'MCPs', kindLabel: 'MCP server', kindLabelPlural: 'MCP servers' },
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
 * sections (skills/prompts/classes/automations/mcps). This is intentionally
 * scoped to the library view — it does not touch the global left sidebar.
 */
export const SectionRail: React.FC<SectionRailProps> = ({ activeSection, sections, onSelect }) => {
    const icons = { skills: Puzzle, prompts: MessageSquare, classes: Layers, automations: Workflow, mcps: Plug };
    return (
        <div
            data-testid="library-section-rail"
            className="w-24 flex-shrink-0 border-r border-wardian-border bg-wardian-sidebar-primary flex flex-col items-stretch overflow-y-auto py-2"
        >
            {LIBRARY_SECTIONS.map((section) => {
                const count = sections ? countEntries(sections[section.id].tree) : 0;
                const Icon = icons[section.id];
                return (
                    <button
                        key={section.id}
                        data-testid={`library-section-${section.id}`}
                        onClick={() => onSelect(section.id)}
                        title={section.label}
                        aria-current={activeSection === section.id ? 'true' : undefined}
                        className={`flex flex-col items-center gap-1 py-3 w-full border-l-2 transition-colors ${
                            activeSection === section.id
                                ? 'border-[var(--color-wardian-accent)] text-[var(--color-wardian-accent)] bg-wardian-card-bg-muted'
                                : 'border-transparent text-muted hover:text-primary'
                        }`}
                    >
                        <Icon size={18} strokeWidth={1.7} aria-hidden="true" />
                        <span className="text-[11px] font-medium text-center">{section.label}</span>
                        {count > 0 && <span className="text-[10px] text-muted-neutral">{count}</span>}
                    </button>
                );
            })}
        </div>
    );
};
