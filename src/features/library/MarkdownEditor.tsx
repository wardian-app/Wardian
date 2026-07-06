import React, { useEffect, useState } from 'react';

interface MarkdownEditorProps {
    value: string;
    onChange: (value: string) => void;
    onSave: () => void;
    /** True when `value` differs from the last saved/loaded baseline. */
    dirty: boolean;
    /** True when the file changed on disk while a dirty draft is open. */
    stale: boolean;
    onReloadExternal: () => void;
}

/**
 * Plain monospace textarea editor shared by every per-kind detail panel.
 * `Ctrl+S`/`Cmd+S` saves; a conflict bar appears when `stale` is true,
 * offering Reload (discard the draft, adopt the on-disk content) or Keep
 * mine (dismiss the nudge locally — the underlying `stale` state is owned
 * by the caller and is not cleared by this dismissal, so it reappears if a
 * *new* external change arrives).
 */
export const MarkdownEditor: React.FC<MarkdownEditorProps> = ({
    value,
    onChange,
    onSave,
    dirty,
    stale,
    onReloadExternal,
}) => {
    const [dismissed, setDismissed] = useState(false);

    // A fresh external change always deserves a fresh nudge, even if the
    // previous one was dismissed with "Keep mine".
    useEffect(() => {
        if (stale) setDismissed(false);
    }, [stale]);

    const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
        const isSaveShortcut = (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's';
        if (isSaveShortcut) {
            e.preventDefault();
            onSave();
        }
    };

    return (
        <div data-testid="markdown-editor" className="flex flex-col h-full min-h-0">
            <div className="flex items-center gap-1.5 px-3 py-1.5 border-b border-wardian-border text-[10px] text-muted-neutral">
                {dirty && (
                    <span
                        data-testid="markdown-editor-dirty-dot"
                        title="Unsaved changes"
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--color-wardian-accent)]"
                    />
                )}
                <span>{dirty ? 'Unsaved changes' : 'Saved'}</span>
                <span className="ml-auto">Ctrl+S / Cmd+S to save</span>
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
                            className="font-bold underline hover:no-underline"
                        >
                            Reload
                        </button>
                        <button
                            type="button"
                            data-testid="markdown-editor-keep-mine"
                            onClick={() => setDismissed(true)}
                            className="font-bold underline hover:no-underline"
                        >
                            Keep mine
                        </button>
                    </div>
                </div>
            )}
            <textarea
                data-testid="markdown-editor-textarea"
                value={value}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={handleKeyDown}
                spellCheck={false}
                className="flex-1 min-h-0 w-full resize-none bg-[var(--color-wardian-input-bg)] p-3 font-mono text-xs text-primary focus:outline-none"
            />
        </div>
    );
};
