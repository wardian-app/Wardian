import React, { useEffect, useState } from 'react';
import { BookOpen, Code2, Save } from 'lucide-react';
import { LibraryMarkdown } from './LibraryMarkdown';
import './library.css';

interface MarkdownEditorProps {
    value: string;
    onChange: (value: string) => void;
    onSave: () => void;
    /** True when `value` differs from the last saved/loaded baseline. */
    dirty: boolean;
    /** True when the file changed on disk while a dirty draft is open. */
    stale: boolean;
    onReloadExternal: () => void;
    /** Called when the user picks "Keep mine" — in addition to dismissing
     * the bar locally, the caller uses this to resolve the underlying
     * `stale` state (e.g. clearing the store's `contentStale`) so a
     * subsequent save is no longer blocked. Optional so callers that don't
     * gate saves on `stale` can omit it. */
    onKeepMine?: () => void;
}

/**
 * Document preview and source editor shared by every per-kind detail panel.
 * `Ctrl+S`/`Cmd+S` saves; a conflict bar appears when `stale` is true,
 * offering Reload (discard the draft, adopt the on-disk content) or Keep
 * mine (dismiss the nudge locally AND, via `onKeepMine`, resolve the
 * caller's underlying stale state — a fresh external change still shows the
 * bar again, since `stale` flipping back to `true` re-arms `dismissed`
 * below).
 */
export const MarkdownEditor: React.FC<MarkdownEditorProps> = ({
    value,
    onChange,
    onSave,
    dirty,
    stale,
    onReloadExternal,
    onKeepMine,
}) => {
    const [dismissed, setDismissed] = useState(false);
    const [editing, setEditing] = useState(false);

    // A fresh external change always deserves a fresh nudge, even if the
    // previous one was dismissed with "Keep mine".
    useEffect(() => {
        if (stale) setDismissed(false);
    }, [stale]);

    const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
        const isSaveShortcut = (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's';
        if (isSaveShortcut) {
            e.preventDefault();
            if (!stale) onSave();
        }
    };

    return (
        <div data-testid="markdown-editor" className="library-document flex flex-col h-full min-h-0" onKeyDown={handleKeyDown}>
            <div className="library-document-toolbar">
                <div className="wardian-segmented-control" role="group" aria-label="Document mode">
                    <button type="button" className={`wardian-button gap-1.5 ${!editing ? 'library-mode-active' : 'text-muted'}`} aria-pressed={!editing} onClick={() => setEditing(false)}>
                        <BookOpen size={14} aria-hidden="true" /> Preview
                    </button>
                    <button type="button" data-testid="markdown-editor-edit" className={`wardian-button gap-1.5 ${editing ? 'library-mode-active' : 'text-muted'}`} aria-pressed={editing} onClick={() => setEditing(true)}>
                        <Code2 size={14} aria-hidden="true" /> Edit
                    </button>
                </div>
                <span className="library-save-status" role="status">
                {dirty && (
                    <span
                        data-testid="markdown-editor-dirty-dot"
                        title="Unsaved changes"
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--color-wardian-accent)]"
                    />
                )}
                <span>{dirty ? 'Unsaved changes' : 'Saved'}</span>
                </span>
                <button type="button" className="wardian-button wardian-button--primary gap-1.5" disabled={!dirty || stale} onClick={onSave} title="Save (Ctrl+S / Cmd+S)">
                    <Save size={14} aria-hidden="true" /> Save
                </button>
            </div>
            {stale && !dismissed && (
                <div
                    data-testid="markdown-editor-stale-bar"
                    className="flex items-center justify-between gap-2 px-3 py-1.5 border-b border-[color-mix(in_srgb,var(--color-wardian-warning),transparent_35%)] bg-[color-mix(in_srgb,var(--color-wardian-warning),transparent_88%)] text-xs text-wardian-warning"
                >
                    <span>File changed on disk —</span>
                    <div className="flex gap-3">
                        <button
                            type="button"
                            data-testid="markdown-editor-reload"
                            onClick={onReloadExternal}
                            className="wardian-button wardian-button--secondary"
                        >
                            Reload
                        </button>
                        <button
                            type="button"
                            data-testid="markdown-editor-keep-mine"
                            onClick={() => {
                                setDismissed(true);
                                onKeepMine?.();
                            }}
                            className="wardian-button wardian-button--secondary"
                        >
                            Keep mine
                        </button>
                    </div>
                </div>
            )}
            {editing ? <textarea
                aria-label="Markdown source"
                autoFocus
                data-testid="markdown-editor-textarea"
                value={value}
                onChange={(e) => onChange(e.target.value)}
                spellCheck={false}
                className="library-source flex-1 min-h-0 w-full resize-none font-mono text-xs text-primary"
            /> : <div className="library-document-preview flex-1 min-h-0 overflow-auto"><LibraryMarkdown value={value} /></div>}
        </div>
    );
};
