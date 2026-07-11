import React, { useEffect } from 'react';
import { useLibraryStore } from '../store/useLibraryStore';
import { SectionRail } from '../features/library/SectionRail';
import { LibraryList } from '../features/library/LibraryList';
import { DetailPane } from '../features/library/DetailPane';
import { SidebarResizeHandle } from '../components/SidebarResizeHandle';
import { LibrarySectionId } from '../types';

export interface LibraryViewProps {
    surfaceId?: string;
    selectedAgentIds: Set<string>;
    /** Threaded through to the workflow detail panel's "Open in Workflows
     * view" link. Optional and no-op when absent — App.tsx wiring lands in
     * a later task. */
    onOpenWorkflowsView?: () => void;
}

/**
 * Library view shell: SectionRail (section switcher) | LibraryList (rows for
 * the active section) | DetailPane (selected entry, with the inline editor
 * and per-kind panels).
 */
export const LibraryView: React.FC<LibraryViewProps> = ({
    surfaceId = 'legacy-library',
    selectedAgentIds,
    onOpenWorkflowsView,
}) => {
    const index = useLibraryStore((s) => s.index);
    const isLoading = useLibraryStore((s) => s.isLoading);
    const error = useLibraryStore((s) => s.error);
    const activeSection = useLibraryStore((s) => s.activeSection);
    const setActiveSection = useLibraryStore((s) => s.setActiveSection);
    const subscribeToLibraryChanges = useLibraryStore((s) => s.subscribeToLibraryChanges);
    const fetchIndex = useLibraryStore((s) => s.fetchIndex);
    const libraryDetailWidth = useLibraryStore((s) => s.libraryDetailWidth);
    const setLibraryDetailWidth = useLibraryStore((s) => s.setLibraryDetailWidth);

    useEffect(() => subscribeToLibraryChanges(), []);

    const handleSelectSection = (section: LibrarySectionId) => {
        setActiveSection(section);
    };

    const handleRetry = () => {
        void fetchIndex();
    };

    // First load, nothing to show yet: a lightweight loading state instead of
    // an empty rail/list/detail shell.
    if (isLoading && !index) {
        return (
            <div
                data-testid="library-view"
                className="flex-1 h-full flex items-center justify-center bg-wardian-bg text-primary overflow-hidden"
            >
                <div
                    data-testid="library-loading"
                    role="status"
                    aria-live="polite"
                    className="text-sm text-[var(--color-wardian-text-muted)] animate-pulse px-1"
                >
                    Loading library...
                </div>
            </div>
        );
    }

    // Initial fetch failed and there's no stale index to fall back on: a
    // compact error surface with a retry affordance.
    if (error && !index) {
        return (
            <div
                data-testid="library-view"
                className="flex-1 h-full flex items-center justify-center bg-wardian-bg text-primary overflow-hidden"
            >
                <div className="flex flex-col items-center gap-3 text-center px-6">
                    <p data-testid="library-error" className="text-sm text-[var(--color-wardian-error)]">
                        Error: {error}
                    </p>
                    <button
                        type="button"
                        data-testid="library-retry"
                        onClick={handleRetry}
                        className="rounded border border-wardian-border px-3 py-1 text-xs text-[var(--color-wardian-text-muted)] transition-colors hover:bg-wardian-card-bg-muted hover:text-primary"
                    >
                        Retry
                    </button>
                </div>
            </div>
        );
    }

    return (
        <div data-testid="library-view" className="flex-1 h-full flex flex-col bg-wardian-bg text-primary overflow-hidden">
            {/* A background refetch (e.g. a failed reload after a
                library-changed event) failed while we still have a stale
                index to show — surface it non-destructively instead of
                blanking the already-loaded content. */}
            {error && index && (
                <div
                    data-testid="library-error-banner"
                    className="flex items-center justify-between gap-3 px-3 py-1.5 border-b border-[color-mix(in_srgb,var(--color-wardian-error),transparent_60%)] bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_88%)] text-xs text-[var(--color-wardian-error)]"
                >
                    <span>Error: {error}</span>
                    <button
                        type="button"
                        data-testid="library-error-banner-retry"
                        onClick={handleRetry}
                        className="shrink-0 rounded border border-[color-mix(in_srgb,var(--color-wardian-error),transparent_35%)] px-2 py-0.5 text-[11px] text-[var(--color-wardian-error)] transition-colors hover:bg-wardian-card-bg-muted"
                    >
                        Retry
                    </button>
                </div>
            )}
            <div className="flex-1 flex min-h-0">
                <SectionRail
                    activeSection={activeSection}
                    sections={index?.sections ?? null}
                    onSelect={handleSelectSection}
                />
                <div data-testid="library-list" className="flex-1 min-w-[320px] min-h-0 overflow-hidden">
                    <LibraryList />
                </div>
                <div
                    data-testid="library-detail"
                    className="relative flex-shrink-0 border-l border-wardian-border overflow-y-auto"
                    style={{ width: `${libraryDetailWidth}px` }}
                >
                    <SidebarResizeHandle
                        baseWidth={libraryDetailWidth}
                        edge="left"
                        onResize={setLibraryDetailWidth}
                        onReset={() => setLibraryDetailWidth(480)}
                    />
                    <DetailPane
                        surfaceId={surfaceId}
                        selectedAgentIds={selectedAgentIds}
                        onOpenWorkflowsView={onOpenWorkflowsView}
                    />
                </div>
            </div>
        </div>
    );
};
