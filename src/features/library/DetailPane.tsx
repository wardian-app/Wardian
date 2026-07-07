import React, { useEffect, useMemo, useRef, useState } from 'react';
import { useLibraryStore } from '../../store/useLibraryStore';
import { useConfirm } from '../../components/ConfirmDialog';
import {
    LibraryEntry,
    LibraryEntryKind,
    LibraryIndexFolder,
    LibraryItemMetadata,
    LibrarySectionId,
    SkillDeployment,
    isLibraryEntry,
} from '../../types';
import { SkillDetail } from './detail/SkillDetail';
import { PromptDetail } from './detail/PromptDetail';
import { WorkflowDetail } from './detail/WorkflowDetail';
import { ClassDetail } from './detail/ClassDetail';
import { McpStubDetail } from './detail/McpStubDetail';

/** Sections whose content files live at `<name>.md` rather than `<name>/`. */
const MD_FILE_SECTIONS: LibrarySectionId[] = ['prompts', 'workflows'];

/** Props every per-kind detail panel shares: the entry, editor draft/dirty/
 * stale plumbing, and the pre-built shared header element. */
export interface DetailPanelCommonProps {
    entry: LibraryEntry;
    header: React.ReactNode;
    draft: string;
    dirty: boolean;
    stale: boolean;
    onChange: (value: string) => void;
    onSave: () => void;
    onReloadExternal: () => void;
    /** Resolves a stale-content conflict in favor of the local draft
     * ("Keep mine"): clears the store's `contentStale` so a subsequent save
     * is no longer blocked. */
    onKeepMine: () => void;
}

function findEntry(folder: LibraryIndexFolder, entryRef: string): LibraryEntry | null {
    for (const child of folder.children) {
        if (isLibraryEntry(child)) {
            if (child.entry_ref === entryRef) return child;
        } else {
            const found = findEntry(child, entryRef);
            if (found) return found;
        }
    }
    return null;
}

/** Preserve the parent folder and (for flat-file sections) the extension
 * when renaming an entry to a new leaf name. */
function buildRenamedPath(section: LibrarySectionId, path: string, newName: string): string {
    const parts = path.split('/');
    const isMdFile = MD_FILE_SECTIONS.includes(section);
    const ext = isMdFile ? '.md' : '';
    const trimmedName = ext && newName.toLowerCase().endsWith(ext) ? newName.slice(0, -ext.length) : newName;
    parts[parts.length - 1] = `${trimmedName}${ext}`;
    return parts.join('/');
}

interface DetailHeaderProps {
    entry: LibraryEntry;
    onToggleStar: () => void;
    /** Undefined hides the rename control (e.g. classes, whose identity is
     * referenced elsewhere and cannot be renamed from here). */
    onRename?: (newName: string) => Promise<void>;
    /** Undefined hides the generic delete control (classes: `delete_entry`
     * always rejects Classes-section deletes on the backend, and
     * `ClassDetail` already owns a correct `delete_agent_class` flow). */
    onDelete?: () => void;
}

/**
 * Header shared by every per-kind detail panel: star toggle + name
 * (+ inline rename) on the left, delete-with-confirm on the right, and a
 * tag editor below.
 */
const DetailHeader: React.FC<DetailHeaderProps> = ({ entry, onToggleStar, onRename, onDelete }) => {
    const [renaming, setRenaming] = useState(false);
    const [renameValue, setRenameValue] = useState(entry.name);
    const [tagsValue, setTagsValue] = useState(entry.tags.join(', '));
    const updateMetadata = useLibraryStore((s) => s.updateMetadata);

    useEffect(() => {
        setRenameValue(entry.name);
        setTagsValue(entry.tags.join(', '));
        setRenaming(false);
    }, [entry.entry_ref, entry.name, entry.tags]);

    const submitRename = async () => {
        const trimmed = renameValue.trim();
        if (!onRename || !trimmed || trimmed === entry.name) {
            setRenaming(false);
            return;
        }
        try {
            await onRename(trimmed);
            setRenaming(false);
        } catch {
            // Keep the rename input open on failure; the store surfaces the
            // error via its `error` state for the caller to display.
        }
    };

    const commitTags = () => {
        const tags = tagsValue
            .split(',')
            .map((t) => t.trim())
            .filter((t) => t.length > 0);
        // The store already surfaces the error via its `error` state; this
        // call site fires-and-forgets, so swallow the rejection here to
        // avoid an unhandled promise rejection.
        void updateMetadata(entry.entry_ref, {
            id: entry.entry_ref,
            tags,
            is_starred: entry.is_starred,
        } satisfies LibraryItemMetadata).catch(() => {});
    };

    return (
        <div data-testid="detail-header" className="flex flex-col gap-2 px-3 py-2 border-b border-wardian-border">
            <div className="flex items-center gap-2 min-w-0">
                <button
                    type="button"
                    data-testid="detail-star-toggle"
                    aria-pressed={entry.is_starred}
                    title={entry.is_starred ? 'Unstar' : 'Star'}
                    onClick={onToggleStar}
                    className={`shrink-0 text-base leading-none transition-colors ${
                        entry.is_starred ? 'text-[var(--color-wardian-accent)]' : 'text-muted-neutral hover:text-primary'
                    }`}
                >
                    {entry.is_starred ? '★' : '☆'}
                </button>
                {renaming ? (
                    <div className="flex flex-1 min-w-0 items-center gap-1">
                        <input
                            type="text"
                            autoFocus
                            data-testid="detail-rename-input"
                            value={renameValue}
                            onChange={(e) => setRenameValue(e.target.value)}
                            onKeyDown={(e) => {
                                if (e.key === 'Enter') void submitRename();
                                if (e.key === 'Escape') setRenaming(false);
                            }}
                            className="flex-1 min-w-0 bg-[var(--color-wardian-input-bg)] border border-[var(--color-wardian-accent)] rounded px-2 py-1 text-sm text-primary focus:outline-none"
                        />
                        <button
                            type="button"
                            data-testid="detail-rename-confirm"
                            onClick={() => void submitRename()}
                            className="rounded border border-wardian-border px-2 py-1 text-xs text-primary hover:bg-wardian-card-bg-muted"
                        >
                            Save
                        </button>
                        <button
                            type="button"
                            data-testid="detail-rename-cancel"
                            onClick={() => setRenaming(false)}
                            className="rounded border border-wardian-border px-2 py-1 text-xs text-muted-neutral hover:text-primary"
                        >
                            Cancel
                        </button>
                    </div>
                ) : (
                    <>
                        <h3 className="flex-1 min-w-0 truncate text-sm font-bold text-primary">{entry.name}</h3>
                        {onRename && (
                            <button
                                type="button"
                                data-testid="detail-rename-button"
                                title="Rename"
                                onClick={() => setRenaming(true)}
                                className="shrink-0 text-muted-neutral hover:text-primary"
                            >
                                Rename
                            </button>
                        )}
                    </>
                )}
                {onDelete && (
                    <button
                        type="button"
                        data-testid="detail-delete-button"
                        title="Delete"
                        onClick={onDelete}
                        className="shrink-0 text-muted-neutral hover:text-[var(--color-wardian-error)]"
                    >
                        Delete
                    </button>
                )}
            </div>
            <input
                type="text"
                data-testid="detail-tags-input"
                value={tagsValue}
                placeholder="tags, comma, separated"
                onChange={(e) => setTagsValue(e.target.value)}
                onBlur={commitTags}
                onKeyDown={(e) => e.key === 'Enter' && commitTags()}
                className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-2 py-1 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)]"
            />
        </div>
    );
};

interface DetailPaneProps {
    selectedAgentIds: Set<string>;
    /** Threaded through from LibraryView; App.tsx wiring lands separately —
     * no-op gracefully if not provided. */
    onOpenWorkflowsView?: () => void;
}

/**
 * Right-hand detail pane: empty-state when nothing is selected, the MCP stub
 * when the MCP section is active, otherwise the per-kind panel for the
 * selected entry. Owns the editor's dirty/stale/draft lifecycle so external
 * changes (via the library watcher) don't silently clobber in-progress
 * edits — see `contentStale` handling below.
 */
export const DetailPane: React.FC<DetailPaneProps> = ({ selectedAgentIds, onOpenWorkflowsView }) => {
    const index = useLibraryStore((s) => s.index);
    const activeSection = useLibraryStore((s) => s.activeSection);
    const selection = useLibraryStore((s) => s.selection);
    const selectedContent = useLibraryStore((s) => s.selectedContent);
    const contentStale = useLibraryStore((s) => s.contentStale);
    const markEditorDirty = useLibraryStore((s) => s.markEditorDirty);
    const select = useLibraryStore((s) => s.select);
    const revertSelection = useLibraryStore((s) => s.revertSelection);
    const resolveStale = useLibraryStore((s) => s.resolveStale);
    const reloadSelectedContent = useLibraryStore((s) => s.reloadSelectedContent);
    const saveItem = useLibraryStore((s) => s.saveItem);
    const updateMetadata = useLibraryStore((s) => s.updateMetadata);
    const renameEntry = useLibraryStore((s) => s.renameEntry);
    const deleteEntry = useLibraryStore((s) => s.deleteEntry);
    const setSkillDeployments = useLibraryStore((s) => s.setSkillDeployments);
    const confirm = useConfirm();

    const [draft, setDraft] = useState('');
    const [baseline, setBaseline] = useState('');
    const dirty = draft !== baseline;
    const trackedEntryRef = useRef<string | null>(null);
    // Set right before a discard-confirm decline reverts `selection` back to
    // the still-dirty entry. The adopt-effect below must skip exactly the
    // resulting `selectedContent` change (it holds the OTHER entry's disk
    // content, fetched when the user first attempted to switch away) so the
    // reverted-to draft is never clobbered. See the guard effect below.
    const suppressNextAdoptRef = useRef(false);

    const currentEntry = useMemo(() => {
        if (!selection || !index) return null;
        const section = index.sections[selection.section];
        if (!section) return null;
        return findEntry(section.tree, selection.entryRef);
    }, [selection, index]);

    // Unsaved-changes guard: when the store's selection changes to a
    // different entry while this pane still holds a dirty draft for the
    // previous one, confirm before discarding. If declined, switch the
    // store's selection back so the dirty entry stays open.
    useEffect(() => {
        const nextRef = selection?.entryRef ?? null;
        if (trackedEntryRef.current === nextRef) return;
        const previousRef = trackedEntryRef.current;
        const wasDirty = previousRef !== null && draft !== baseline;

        if (wasDirty && previousRef) {
            void confirm('Discard changes?').then((ok) => {
                if (ok) {
                    suppressNextAdoptRef.current = false;
                    trackedEntryRef.current = nextRef;
                    setDraft('');
                    setBaseline('');
                } else {
                    // Keep the dirty draft: revert the store's selection
                    // back to the previous entry WITHOUT re-reading it from
                    // disk (unlike `select()`). `selectedContent` still
                    // holds whatever was fetched for the entry the user
                    // tried (and declined) to switch to, so the adopt-effect
                    // below must not treat that as fresh content for
                    // `previousRef` — otherwise it would silently overwrite
                    // the very draft the user just chose to keep.
                    suppressNextAdoptRef.current = true;
                    trackedEntryRef.current = previousRef;
                    revertSelection(previousRef);
                }
            });
            return;
        }

        suppressNextAdoptRef.current = false;
        trackedEntryRef.current = nextRef;
        setDraft('');
        setBaseline('');
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [selection?.entryRef]);

    // Adopt freshly loaded/reloaded content for the entry currently tracked
    // (a deliberate load or an explicit Reload — the store only updates
    // `selectedContent` in those cases; a background change while dirty
    // sets `contentStale` instead, see useLibraryStore.subscribeToLibraryChanges).
    // Skipped exactly once when reverting a declined discard-confirm (see
    // above) so a dirty draft is never replaced except by explicit user
    // action (Reload/Discard) or a successful save.
    useEffect(() => {
        if (selection?.entryRef !== trackedEntryRef.current) return;
        if (selectedContent === null) return;
        if (suppressNextAdoptRef.current) {
            suppressNextAdoptRef.current = false;
            return;
        }
        setDraft(selectedContent);
        setBaseline(selectedContent);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [selectedContent, selection?.entryRef]);

    useEffect(() => {
        markEditorDirty(dirty);
    }, [dirty, markEditorDirty]);

    if (activeSection === 'mcps') {
        return (
            <div data-testid="detail-pane" className="h-full">
                <McpStubDetail />
            </div>
        );
    }

    if (!selection || !currentEntry) {
        return (
            <div data-testid="detail-pane" className="h-full flex items-center justify-center px-6 text-center">
                <p data-testid="library-detail-empty" className="text-xs text-muted">
                    Select an item from the list to view its details.
                </p>
            </div>
        );
    }

    const section = selection.section;

    const handleSave = async () => {
        // While the conflict bar is showing, a plain save must not silently
        // overwrite the external disk change — the user has to resolve it
        // first via "Keep mine" (clears `contentStale`, see `onKeepMine`
        // below) or "Reload" (adopts disk content, clearing `dirty`).
        if (contentStale) return;
        try {
            await saveItem(section, currentEntry.path, draft);
            setBaseline(draft);
            resolveStale();
        } catch {
            // saveItem already records the error on the store; keep the
            // draft so the user doesn't lose their edits.
        }
    };

    const handleToggleStar = () => {
        // The store already surfaces the error via its `error` state; this
        // call site fires-and-forgets, so swallow the rejection here to
        // avoid an unhandled promise rejection.
        void updateMetadata(currentEntry.entry_ref, {
            id: currentEntry.entry_ref,
            tags: currentEntry.tags,
            is_starred: !currentEntry.is_starred,
        }).catch(() => {});
    };

    const handleDelete = async () => {
        if (!(await confirm(`Delete "${currentEntry.name}"? This cannot be undone.`))) return;
        try {
            await deleteEntry(section, currentEntry.path);
            await select(null);
        } catch {
            // deleteEntry already records the error on the store.
        }
    };

    const header = (
        <DetailHeader
            entry={currentEntry}
            onToggleStar={handleToggleStar}
            onRename={
                section === 'classes'
                    ? undefined
                    : async (newName: string) => {
                          const toPath = buildRenamedPath(section, currentEntry.path, newName);
                          await renameEntry(section, currentEntry.path, toPath);
                      }
            }
            onDelete={section === 'classes' ? undefined : () => void handleDelete()}
        />
    );

    const common: DetailPanelCommonProps = {
        entry: currentEntry,
        header,
        draft,
        dirty,
        stale: contentStale,
        onChange: setDraft,
        onSave: () => void handleSave(),
        onReloadExternal: () => void reloadSelectedContent(),
        onKeepMine: resolveStale,
    };

    const renderPanel = (kind: LibraryEntryKind) => {
        switch (kind) {
            case 'skill':
                return (
                    <SkillDetail
                        {...common}
                        deployments={index?.deployments[currentEntry.entry_ref] ?? []}
                        onApplyDeployments={(targets: SkillDeployment[]) =>
                            // The store already surfaces the error via its
                            // `error` state; swallow the rejection here so it
                            // never reaches the caller (DeployTargetsControl
                            // awaits the returned promise only to know when
                            // to clear its own pending state, not to handle
                            // failure itself).
                            setSkillDeployments(currentEntry.path, targets).catch(() => {})
                        }
                    />
                );
            case 'prompt':
                return <PromptDetail {...common} selectedAgentIds={selectedAgentIds} />;
            case 'workflow':
                return <WorkflowDetail {...common} onOpenWorkflowsView={onOpenWorkflowsView} />;
            case 'class':
                return (
                    <ClassDetail
                        {...common}
                        deployments={index?.deployments ?? {}}
                        onRemoveSkillDeployment={(sourcePath: string) => {
                            const skillRef = `skills/${sourcePath}`;
                            const currentTargets = index?.deployments[skillRef] ?? [];
                            const remaining: SkillDeployment[] = currentTargets
                                .filter((t) => !(t.target_type === 'class' && t.target_id === currentEntry.path))
                                .map((t) => ({ target_type: t.target_type, target_id: t.target_id }));
                            // The store already surfaces the error via its
                            // `error` state; this call site fires-and-forgets,
                            // so swallow the rejection here to avoid an
                            // unhandled promise rejection.
                            void setSkillDeployments(sourcePath, remaining).catch(() => {});
                        }}
                        onDeleted={() => void select(null)}
                    />
                );
            default:
                return null;
        }
    };

    return (
        <div data-testid="detail-pane" className="h-full min-h-0">
            {renderPanel(currentEntry.kind)}
        </div>
    );
};

export { DetailHeader };
