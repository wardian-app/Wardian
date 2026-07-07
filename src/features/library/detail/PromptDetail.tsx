import React, { useState } from 'react';
import { flattenPromptForInjection, submitInputToAgents } from '../../../utils/terminalInput';
import { DetailPanelCommonProps } from '../DetailPane';
import { MarkdownEditor } from '../MarkdownEditor';

interface PromptDetailProps extends DetailPanelCommonProps {
    selectedAgentIds: Set<string>;
}

/**
 * Prompt panel: editor + Run, preserving today's behavior
 * (`LibraryView.tsx:103-120` pre-redesign): flatten newlines out of the
 * prompt body, then submit it to every selected agent. Disabled (with a
 * tooltip) when no agents are selected — unlike the legacy CommandPanel
 * quick-inject flow, this no longer falls back to a broadcast-to-everyone
 * confirm, since the detail pane always has an explicit agent selection to
 * defer to.
 */
export const PromptDetail: React.FC<PromptDetailProps> = ({
    header,
    draft,
    dirty,
    stale,
    onChange,
    onSave,
    onReloadExternal,
    onKeepMine,
    selectedAgentIds,
}) => {
    const [running, setRunning] = useState(false);
    const hasAgents = selectedAgentIds.size > 0;

    const handleRun = async () => {
        if (!hasAgents || running) return;
        setRunning(true);
        try {
            const flattened = flattenPromptForInjection(draft);
            await submitInputToAgents(selectedAgentIds, flattened);
        } catch (e) {
            console.error('Failed to run prompt', e);
        } finally {
            setRunning(false);
        }
    };

    return (
        <div data-testid="prompt-detail" className="flex flex-col h-full min-h-0">
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
            <div className="border-t border-wardian-border p-3">
                <button
                    type="button"
                    data-testid="prompt-run-button"
                    onClick={() => void handleRun()}
                    disabled={!hasAgents || running}
                    title={hasAgents ? undefined : 'Select at least one agent to run this prompt'}
                    className="rounded bg-[var(--color-wardian-accent)] px-3 py-1.5 text-xs font-bold text-[var(--color-wardian-bg)] transition-all hover:brightness-110 disabled:opacity-50 disabled:hover:brightness-100"
                >
                    {running ? 'Running…' : 'Run'}
                </button>
            </div>
        </div>
    );
};
