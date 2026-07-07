import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useLibraryStore } from '../../store/useLibraryStore';
import {
    AgentConfig,
    DeploymentTarget,
    LibraryEntry,
    LibraryIndexFolder,
    SkillDeployment,
    isLibraryEntry,
} from '../../types';

const ENTRY_REF_MIME = 'text/wardian-entry-ref';

interface DeployTargetsControlProps {
    entry: LibraryEntry;
    /** Current deployments for `entry`, from `index.deployments[entry.entry_ref]`. */
    deployments: DeploymentTarget[];
    onApply: (targets: SkillDeployment[]) => void;
}

interface TargetOption {
    target_type: 'user' | 'class' | 'agent';
    target_id: string;
    label: string;
}

function targetKey(t: { target_type: string; target_id: string }): string {
    return `${t.target_type}:${t.target_id}`;
}

function collectClassOptions(tree: LibraryIndexFolder): TargetOption[] {
    const options: TargetOption[] = [];
    const walk = (folder: LibraryIndexFolder) => {
        for (const child of folder.children) {
            if (isLibraryEntry(child)) {
                options.push({ target_type: 'class', target_id: child.path, label: child.name });
            } else {
                walk(child);
            }
        }
    };
    walk(tree);
    return options;
}

/**
 * Checklist of deploy targets for a skill: the global user profile, every
 * class, and every persisted agent. Checking/unchecking builds a desired
 * set applied in one shot via `onApply` (which the caller wires to
 * `setSkillDeployments`). Targets that are currently a *copy* rather than a
 * live link surface the amber "copied — edits won't sync" note. The control
 * also accepts a drop of a (different) skill's entry ref — dropping one
 * switches the library selection to that skill, so a quick drag from the
 * list can jump straight to configuring its deployments here.
 */
export const DeployTargetsControl: React.FC<DeployTargetsControlProps> = ({ entry, deployments, onApply }) => {
    const classesTree = useLibraryStore((s) => s.index?.sections?.classes?.tree ?? null);
    const select = useLibraryStore((s) => s.select);
    const [agents, setAgents] = useState<AgentConfig[]>([]);
    const [checked, setChecked] = useState<Set<string>>(() => new Set(deployments.map(targetKey)));

    // Re-seed the checklist whenever the displayed entry (or its known
    // deployments) changes — otherwise stale checkbox state from the
    // previously selected skill would leak into this one.
    useEffect(() => {
        setChecked(new Set(deployments.map(targetKey)));
    }, [entry.entry_ref, deployments]);

    useEffect(() => {
        let cancelled = false;
        invoke<AgentConfig[]>('list_agents')
            .then((list) => {
                if (!cancelled) setAgents(Array.isArray(list) ? list : []);
            })
            .catch(() => {
                if (!cancelled) setAgents([]);
            });
        return () => {
            cancelled = true;
        };
    }, []);

    const classOptions = useMemo(() => (classesTree ? collectClassOptions(classesTree) : []), [classesTree]);
    const agentOptions = useMemo<TargetOption[]>(
        () => agents.map((a) => ({ target_type: 'agent', target_id: a.session_id, label: a.session_name || a.session_id })),
        [agents],
    );
    const targets = useMemo<TargetOption[]>(
        () => [{ target_type: 'user', target_id: 'global', label: 'User (global)' }, ...classOptions, ...agentOptions],
        [classOptions, agentOptions],
    );

    const toggle = (target: TargetOption) => {
        setChecked((prev) => {
            const next = new Set(prev);
            const key = targetKey(target);
            if (next.has(key)) {
                next.delete(key);
            } else {
                next.add(key);
            }
            return next;
        });
    };

    const handleApply = () => {
        const desired: SkillDeployment[] = targets
            .filter((t) => checked.has(targetKey(t)))
            .map((t) => ({ target_type: t.target_type, target_id: t.target_id }));
        // `targets` only covers the global user profile, classes, and
        // currently-live agents (`list_agents` returns only agents in
        // AppState). A deployment can exist for a persisted-but-not-live
        // agent, or for any agent at all if `list_agents` rejected (the
        // catch above falls back to []) — such a target has no rendered
        // checklist row, so it can never appear in `desired`. Preserve any
        // existing deployment whose target isn't among the rendered options
        // so Apply can't silently undeploy something it can't see
        // (final-review FIX-NOW 2).
        const known = new Set(targets.map(targetKey));
        const preserved: SkillDeployment[] = deployments
            .filter((d) => !known.has(targetKey(d)))
            .map((d) => ({ target_type: d.target_type, target_id: d.target_id }));
        onApply([...preserved, ...desired]);
    };

    const handleDrop = (e: React.DragEvent) => {
        e.preventDefault();
        const droppedRef = e.dataTransfer.getData(ENTRY_REF_MIME);
        if (droppedRef && droppedRef.startsWith('skills/') && droppedRef !== entry.entry_ref) {
            void select(droppedRef);
        }
    };

    return (
        <div
            data-testid="deploy-targets-control"
            onDragOver={(e) => e.preventDefault()}
            onDrop={handleDrop}
            className="flex flex-col gap-2"
        >
            <h4 className="text-xs font-bold text-muted">Deploy to</h4>
            <div className="flex flex-col gap-1 max-h-48 overflow-y-auto">
                {targets.map((target) => {
                    const key = targetKey(target);
                    const existing = deployments.find((d) => targetKey(d) === key);
                    const copied = Boolean(existing && !existing.linked);
                    return (
                        <label
                            key={key}
                            data-testid={`deploy-target-${key}`}
                            className="flex items-center gap-2 text-xs text-primary"
                        >
                            <input
                                type="checkbox"
                                checked={checked.has(key)}
                                onChange={() => toggle(target)}
                                className="h-3.5 w-3.5 accent-[var(--color-wardian-accent)]"
                            />
                            <span className="flex-1 truncate">{target.label}</span>
                            {copied && (
                                <span
                                    data-testid={`deploy-target-copied-${key}`}
                                    className="text-[10px] text-wardian-warning"
                                >
                                    copied — edits won&apos;t sync
                                </span>
                            )}
                        </label>
                    );
                })}
            </div>
            <button
                type="button"
                data-testid="deploy-targets-apply"
                onClick={handleApply}
                className="self-start rounded border border-wardian-border px-3 py-1 text-xs text-primary transition-colors hover:bg-wardian-card-bg-muted"
            >
                Apply
            </button>
        </div>
    );
};
