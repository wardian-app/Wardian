import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useConfirm } from '../../../components/ConfirmDialog';
import { AgentClassDefinition, DeploymentTarget } from '../../../types';
import { DetailPanelCommonProps } from '../DetailPane';
import { MarkdownEditor } from '../MarkdownEditor';
import { BookOpen, Layers, Puzzle, ArrowUpRight, Trash2 } from 'lucide-react';
import { useLibraryStore } from '../../../store/useLibraryStore';
import { flattenAllEntries } from '../libraryListUtils';

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
    onKeepMine,
    deployments,
    onRemoveSkillDeployment,
    onDeleted,
}) => {
    const confirm = useConfirm();
    const index = useLibraryStore((state) => state.index);
    const select = useLibraryStore((state) => state.select);
    const [classDef, setClassDef] = useState<AgentClassDefinition | null>(null);
    const [busy, setBusy] = useState(false);
    const [actionError, setActionError] = useState<string | null>(null);

    useEffect(() => {
        let cancelled = false;
        setClassDef(null);
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
    const skillEntries = useMemo(() => new Map(index
        ? flattenAllEntries(index.sections.skills.tree).flatMap((row) => row.entry ? [[row.entry.path, row.entry] as const] : [])
        : []), [index]);
    const sharedSkills = Object.entries(deployments)
        .filter(([ref, targets]) => ref.startsWith(SKILLS_PREFIX) && targets.some((target) => target.target_type === 'user'))
        .map(([ref]) => ref.slice(SKILLS_PREFIX.length)).sort();
    const orphanedSkills = index?.orphans.filter((orphan) => orphan.target_type === 'class' && orphan.target_id === entry.path) ?? [];

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
            <section aria-label="Class contents" className="library-class-contents flex flex-col gap-3">
                {classDef && (
                    <div data-testid="class-provider-defaults" className="text-xs text-muted">
                        <span className="font-bold text-primary">{classDef.is_default ? 'Default class' : 'Custom class'}</span>
                        {classDef.description && <p className="mt-0.5">{classDef.description}</p>}
                    </div>
                )}
                <div>
                    <div className="flex flex-wrap items-center gap-2 mb-3 text-xs text-muted">
                        <span className="inline-flex items-center gap-1.5"><BookOpen size={14} aria-hidden="true" /> AGENTS.md instructions</span>
                        <span className="inline-flex items-center gap-1.5"><Layers size={14} aria-hidden="true" /> {deployedSkills.length} class {deployedSkills.length === 1 ? 'skill' : 'skills'}</span>
                    </div>
                    <h4 className="mb-2 text-xs font-semibold text-primary">Included skills</h4>
                    {deployedSkills.length === 0 ? (
                        <p className="text-[11px] italic text-muted-neutral">No skills deployed to this class.</p>
                    ) : (
                        <ul className="flex flex-col gap-2">
                            {deployedSkills.map((skill) => (
                                <li
                                    key={skill.sourcePath}
                                    className="library-class-skill text-xs text-primary"
                                >
                                    <Puzzle size={16} className="shrink-0 text-muted" aria-hidden="true" />
                                    <div className="flex-1 min-w-0">
                                        {skillEntries.has(skill.sourcePath) ? <button type="button" className="inline-flex max-w-full items-center gap-1 font-semibold text-left hover:underline" onClick={() => void select(`skills/${skill.sourcePath}`)}>
                                            <span className="truncate">{skillEntries.get(skill.sourcePath)?.name}</span><ArrowUpRight size={12} className="shrink-0" aria-hidden="true" />
                                        </button> : <span className="font-semibold">{skill.sourcePath.split('/').pop()}</span>}
                                        {skillEntries.get(skill.sourcePath)?.description && <p className="mt-1 text-muted line-clamp-2">{skillEntries.get(skill.sourcePath)?.description}</p>}
                                        <p className="mt-1 text-[10px] text-muted-neutral break-all">{skill.sourcePath}</p>
                                        <p className={`mt-1 text-[10px] ${skill.linked ? 'text-muted' : 'text-wardian-warning'}`}>{skill.linked ? 'Linked · source edits sync' : 'copied — edits won\'t sync'}</p>
                                    </div>
                                    <button
                                        type="button"
                                        data-testid={`class-skill-remove-${skill.sourcePath}`}
                                        onClick={() => onRemoveSkillDeployment(skill.sourcePath)}
                                        aria-label={`Remove ${skill.sourcePath} from class`}
                                        title="Remove from class"
                                        className="wardian-icon-button wardian-icon-button--secondary shrink-0 hover:text-[var(--color-wardian-error)]"
                                    >
                                        <Trash2 size={14} aria-hidden="true" />
                                    </button>
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
                {orphanedSkills.length > 0 && <div role="status" className="text-xs text-wardian-warning">
                    <p className="font-semibold">Unresolved class skills</p>
                    <ul>{orphanedSkills.map((skill) => <li key={skill.skill_name}>{skill.skill_name} — source unavailable</li>)}</ul>
                </div>}
                <details className="text-xs text-muted">
                    <summary className="cursor-pointer">Shared with all agents · {sharedSkills.length} {sharedSkills.length === 1 ? 'skill' : 'skills'}</summary>
                    {sharedSkills.length ? <ul className="mt-2 flex flex-wrap gap-2">{sharedSkills.map((path) => <li key={path} className="rounded border border-wardian-border px-2 py-1 break-all">{path}</li>)}</ul> : <p className="mt-2">No globally deployed skills.</p>}
                </details>
            </section>
            <div className="flex-1 min-h-[240px]">
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
            <div className="flex flex-col gap-3 border-t border-wardian-border p-3 shrink-0">
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
                            className="wardian-button wardian-button--secondary"
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
                            className="wardian-button wardian-button--secondary text-wardian-error"
                        >
                            Delete class
                        </button>
                    )}
                </div>
            </div>
        </div>
    );
};
