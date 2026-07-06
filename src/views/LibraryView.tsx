import React, { useEffect } from 'react';
import { useLibraryStore } from '../store/useLibraryStore';
import { SectionRail } from '../features/library/SectionRail';
import { LibrarySectionId } from '../types';

interface LibraryViewProps {
    selectedAgentIds: Set<string>;
}

/**
 * Library view shell: SectionRail (section switcher) | LibraryList (rows for
 * the active section) | DetailPane (selected entry). LibraryList and
 * DetailPane are stubbed placeholders here — they land in Tasks 14 and 15.
 * `selectedAgentIds` is threaded through unused for now; it feeds prompt-run
 * wiring in Task 14.
 */
export const LibraryView: React.FC<LibraryViewProps> = ({ selectedAgentIds }) => {
    void selectedAgentIds;

    const index = useLibraryStore((s) => s.index);
    const activeSection = useLibraryStore((s) => s.activeSection);
    const setActiveSection = useLibraryStore((s) => s.setActiveSection);
    const subscribeToLibraryChanges = useLibraryStore((s) => s.subscribeToLibraryChanges);

    useEffect(() => subscribeToLibraryChanges(), []);

    const handleSelectSection = (section: LibrarySectionId) => {
        setActiveSection(section);
    };

    return (
        <div data-testid="library-view" className="flex-1 h-full flex bg-wardian-bg text-primary overflow-hidden">
            <SectionRail
                activeSection={activeSection}
                sections={index?.sections ?? null}
                onSelect={handleSelectSection}
            />
            <div data-testid="library-list" className="flex-1 min-w-0 overflow-y-auto">
                {/* LibraryList lands in Task 14. */}
            </div>
            <div
                data-testid="library-detail"
                className="w-[380px] flex-shrink-0 border-l border-wardian-border overflow-y-auto"
            >
                {/* DetailPane lands in Task 15. */}
            </div>
        </div>
    );
};
