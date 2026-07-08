import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { RunLaunchDialog } from '../../workflows/RunLaunchDialog';
import type { Blueprint } from '../../workflows/builder/blueprintTypes';
import { DetailPanelCommonProps } from '../DetailPane';
import { MarkdownEditor } from '../MarkdownEditor';

interface BlueprintRef {
    id: string;
    name: string;
    path: string;
}

interface WorkflowDetailProps extends DetailPanelCommonProps {
    /** Threaded through from LibraryView/App; no-op gracefully if absent
     * (App.tsx wiring lands in a later task). */
    onOpenWorkflowsView?: () => void;
}

function normalizeSlashes(path: string): string {
    return path.replace(/\\/g, '/');
}

/**
 * True when `candidatePath`'s trailing path segments exactly match
 * `entryPath`'s segments. Segment-aware (not a raw `endsWith` on the
 * strings) so an absolute path like `.../other-a/foo.md` does not
 * false-positive against an entry path of `a/foo.md` — `endsWith` matches
 * on the substring `a/foo.md` regardless of the segment boundary before it.
 */
function matchesEntryPath(candidatePath: string, entryPath: string): boolean {
    const candidateSegments = normalizeSlashes(candidatePath).split('/').filter(Boolean);
    const entrySegments = normalizeSlashes(entryPath).split('/').filter(Boolean);
    if (entrySegments.length === 0 || entrySegments.length > candidateSegments.length) return false;
    const tail = candidateSegments.slice(candidateSegments.length - entrySegments.length);
    return tail.every((segment, i) => segment === entrySegments[i]);
}

/**
 * Workflow panel: blueprint editor + "Launch Run" (resolves the entry to its
 * on-disk blueprint via `workflow_list_blueprints`/`workflow_parse`, then
 * reuses `RunLaunchDialog`) + a link back to the full Workflows view.
 */
export const WorkflowDetail: React.FC<WorkflowDetailProps> = ({
    entry,
    header,
    draft,
    dirty,
    stale,
    onChange,
    onSave,
    onReloadExternal,
    onKeepMine,
    onOpenWorkflowsView,
}) => {
    const [launchOpen, setLaunchOpen] = useState(false);
    const [blueprint, setBlueprint] = useState<Blueprint | null>(null);
    const [blueprintPath, setBlueprintPath] = useState<string | null>(null);
    const [resolveError, setResolveError] = useState<string | null>(null);
    const [resolving, setResolving] = useState(false);

    const handleOpenLaunch = async () => {
        setResolveError(null);
        setResolving(true);
        try {
            const refs = await invoke<BlueprintRef[]>('workflow_list_blueprints');
            const ref = refs.find((r) => matchesEntryPath(r.path, entry.path));
            if (!ref) {
                setResolveError('Could not locate this workflow file on disk.');
                return;
            }
            const parsed = await invoke<{ blueprint: Blueprint; diagnostics: unknown[] }>('workflow_parse', {
                path: ref.path,
            });
            setBlueprint(parsed.blueprint);
            setBlueprintPath(ref.path);
            setLaunchOpen(true);
        } catch (e) {
            setResolveError(e instanceof Error ? e.message : String(e));
        } finally {
            setResolving(false);
        }
    };

    return (
        <div data-testid="workflow-detail" className="relative flex h-full min-h-0 flex-col">
            {header}
            <div className="flex-1 min-h-0">
                <MarkdownEditor
                    value={draft}
                    onChange={onChange}
                    onSave={onSave}
                    dirty={dirty}
                    stale={stale}
                    onReloadExternal={onReloadExternal}
                    onKeepMine={onKeepMine}
                />
            </div>
            <div className="flex flex-col gap-2 border-t border-wardian-border p-3">
                {resolveError && (
                    <p data-testid="workflow-resolve-error" className="text-xs text-wardian-error">
                        {resolveError}
                    </p>
                )}
                <div className="flex gap-2">
                    <button
                        type="button"
                        data-testid="workflow-launch-run"
                        onClick={() => void handleOpenLaunch()}
                        disabled={resolving}
                        className="rounded bg-[var(--color-wardian-accent)] px-3 py-1.5 text-xs font-bold text-[var(--color-wardian-bg)] transition-all hover:brightness-110 disabled:opacity-50"
                    >
                        {resolving ? 'Resolving…' : 'Launch Run'}
                    </button>
                    <button
                        type="button"
                        data-testid="workflow-open-in-view"
                        onClick={() => onOpenWorkflowsView?.()}
                        disabled={!onOpenWorkflowsView}
                        title={onOpenWorkflowsView ? undefined : 'Not available yet'}
                        className="rounded border border-wardian-border px-3 py-1.5 text-xs text-primary transition-colors hover:bg-wardian-card-bg-muted disabled:opacity-50"
                    >
                        Open in Workflows view
                    </button>
                </div>
            </div>
            {launchOpen && blueprintPath && (
                <div className="absolute inset-0 z-20 flex items-start justify-center overflow-hidden bg-[color-mix(in_srgb,var(--color-wardian-bg),transparent_25%)] p-6">
                    <RunLaunchDialog
                        path={blueprintPath}
                        blueprintId={blueprint?.id}
                        blueprint={blueprint}
                        onLaunched={() => setLaunchOpen(false)}
                        onCancel={() => setLaunchOpen(false)}
                    />
                </div>
            )}
        </div>
    );
};
