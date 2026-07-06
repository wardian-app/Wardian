import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useConfirm } from '../../../components/ConfirmDialog';
import { AgentClassDefinition, DeploymentTarget } from '../../../types';
import { DetailPanelCommonProps } from '../DetailPane';
import { MarkdownEditor } from '../MarkdownEditor';

const SKILLS_PREFIX = 'skills/';

interface ClassDetailProps extends DetailPanelCommonProps {
    /** Full deployments map (keyed by skill entry_ref) — filtered here down
     * to targets deployed to this class. */
    deployments: Record<string, DeploymentTarget[]>;
    onRemoveSkillDeployment: (sourcePath: string) => void;
    /** Called after a successful delete so the caller can clear selection. */
    onDeleted: () => void;
}

interface DeployedSkillRow {
    sourcePath: string;
    linked: boolean;
}

function deployedSkillsForClass(
    deployments: Record<string, DeploymentTarget[]>,
    className: string,
): DeployedSkillRow[] {
    const rows: DeployedSkillRow[] = [];
    for (const [skillRef, targets] of Object.entries(deployments)) {
        if (!skillRef.startsWith(SKILLS_PREFIX)) continue;
        const target = targets.find((t) => t.target_type === 'class' && t.target_id === className);
        if (target) {
            rows.push({ sourcePath: skillRef.slice(SKILLS_PREFIX.length), linked: target.linked });
        }
    }
    rows.sort((a, b) => a.sourcePath.localeCompare(b.sourcePath));
    return rows;
}

/**
 * Class panel: AGENTS.md editor (via the same `read_library_item`/
 * `save_library_item` path every other section uses) + the list of skills
 * currently deployed to this class (with per-skill remove) + provider
 * defaults from `list_agent_classes` + reset-to-default/delete, reusing the
 * existing class commands.
 */
export const ClassDetail: React.FC<ClassDetailProps> = ({
    entry,
    header,
    draft,
    dirty,
    stale,
    onChange,
    onSave,
    onReloadExternal,
    deployments,
    onRemoveSkillDeployment,
    onDeleted,
}) => {
    const confirm = useConfirm();
    const [classDef, setClassDef] = useState<AgentClassDefinition | null>(null);
    const [busy, setBusy] = useState(false);
    const [actionError, setActionError] = useState<string | null>(null);

    useEffect(() => {
        let cancelled = false;
        invoke<AgentClassDefinition[]>('list_agent_classes')
            .then((all) => {
                if (!cancelled) setClassDef(all.find((c) => c.name === entry.path) ?? null);
            })
            .catch(() => {
                if (!cancelled) setClassDef(null);
            });
        return () => {
            cancelled = true;
        };
    }, [entry.path]);

    const deployedSkills = useMemo(() => deployedSkillsForClass(deployments, entry.path), [deployments, entry.path]);

    const handleResetToDefault = async () => {
        setBusy(true);
        setActionError(null);
        try {
            await invoke('reset_class_to_default', { name: entry.path });
            await onReloadExternal();
        } catch (e) {
            setActionError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    };

    const handleDeleteClass = async () => {
        if (!(await confirm(`Delete custom class "${entry.path}"? This will also remove its directory.`))) return;
        setBusy(true);
        setActionError(null);
        try {
            await invoke('delete_agent_class', { name: entry.path });
            onDeleted();
        } catch (e) {
            setActionError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <div data-testid="class-detail" className="flex flex-col h-full min-h-0">
            {header}
            <div className="flex-1 min-h-0">
                <MarkdownEditor
                    value={draft}
                    onChange={onChange}
                    onSave={onSave}
                    dirty={dirty}
                    stale={stale}
                    onReloadExternal={onReloadExternal}
                />
            </div>
            <div className="flex flex-col gap-3 border-t border-wardian-border p-3">
                {classDef && (
                    <div data-testid="class-provider-defaults" className="text-xs text-muted">
                        <span className="font-bold text-primary">{classDef.is_default ? 'Default class' : 'Custom class'}</span>
                        {classDef.description && <p className="mt-0.5">{classDef.description}</p>}
                    </div>
                )}
                <div>
                    <h4 className="mb-1 text-xs font-bold text-muted">Deployed skills</h4>
                    {deployedSkills.length === 0 ? (
                        <p className="text-[11px] italic text-muted-neutral">No skills deployed to this class.</p>
                    ) : (
                        <ul className="flex flex-col gap-1">
                            {deployedSkills.map((skill) => (
                                <li
                                    key={skill.sourcePath}
                                    className="flex items-center justify-between gap-2 text-xs text-primary"
                                >
                                    <span className="truncate font-mono">{skill.sourcePath}</span>
                                    {!skill.linked && (
                                        <span className="shrink-0 text-[10px] text-wardian-warning">copied — edits won&apos;t sync</span>
                                    )}
                                    <button
                                        type="button"
                                        data-testid={`class-skill-remove-${skill.sourcePath}`}
                                        onClick={() => onRemoveSkillDeployment(skill.sourcePath)}
                                        className="shrink-0 text-muted-neutral hover:text-[var(--color-wardian-error)]"
                                    >
                                        Remove
                                    </button>
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
                {actionError && (
                    <p data-testid="class-action-error" className="text-xs text-wardian-error">
                        {actionError}
                    </p>
                )}
                <div className="flex gap-2">
                    {classDef?.is_default && (
                        <button
                            type="button"
                            data-testid="class-reset-default"
                            disabled={busy}
                            onClick={() => void handleResetToDefault()}
                            className="rounded border border-wardian-border px-3 py-1.5 text-xs text-primary transition-colors hover:bg-wardian-card-bg-muted disabled:opacity-50"
                        >
                            Reset to default
                        </button>
                    )}
                    {classDef && !classDef.is_default && (
                        <button
                            type="button"
                            data-testid="class-delete"
                            disabled={busy}
                            onClick={() => void handleDeleteClass()}
                            className="rounded border border-wardian-border px-3 py-1.5 text-xs text-[var(--color-wardian-error)] transition-colors hover:bg-wardian-card-bg-muted disabled:opacity-50"
                        >
                            Delete class
                        </button>
                    )}
                </div>
            </div>
        </div>
    );
};
