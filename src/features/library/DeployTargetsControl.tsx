import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { X } from 'lucide-react';
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
    /**
     * Applies a full desired target set immediately (add/remove is computed
     * by this control as "current deployments ± one target" and handed to
     * the caller, which wraps `setSkillDeployments`). Returns a promise that
     * settles once the store's mutation attempt is done (success or
     * failure) so this control can clear its own pending/disabled state;
     * the promise is not expected to reject — error reporting is the
     * store's `error` state, surfaced elsewhere (see `LibraryView`'s error
     * banner), not this control's job.
     */
    onApply: (targets: SkillDeployment[]) => Promise<void>;
}

type TargetType = 'user' | 'class' | 'agent';

interface TargetOption {
    target_type: TargetType;
    target_id: string;
    label: string;
}

const GROUP_ORDER: TargetType[] = ['user', 'class', 'agent'];
const GROUP_LABELS: Record<TargetType, string> = { user: 'USER', class: 'CLASSES', agent: 'AGENTS' };

function targetKey(t: { target_type: string; target_id: string }): string {
    return `${t.target_type}:${t.target_id}`;
}

function toSkillDeployment(t: { target_type: string; target_id: string }): SkillDeployment {
    return { target_type: t.target_type, target_id: t.target_id };
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

/** Resolves a chip's display label. Falls back to the raw `target_id` for a
 * deployment whose target isn't among the known options (e.g. a persisted
 * but no-longer-live agent) — the chip must still render so every current
 * deployment stays visible and individually removable (final-review FIX-NOW
 * 2's guarantee carries over from the old checklist design). */
function labelFor(target: { target_type: string; target_id: string }, known: TargetOption[]): string {
    if (target.target_type === 'user') return 'User (global)';
    const match = known.find((t) => t.target_type === target.target_type && t.target_id === target.target_id);
    return match?.label ?? target.target_id;
}

/**
 * Deploy targets for a skill, rendered as removable chips (current
 * deployments) plus a searchable "+ Add target…" popover for adding more.
 * Both add and remove apply immediately via `onApply` — there is no longer
 * a batch Apply step. Chips are derived directly from `deployments` (the
 * index's actual persisted deployments), so every current deployment,
 * including one to a class/agent the picker can't resolve a label for, has
 * a chip and can be removed individually without disturbing the others.
 * Targets that are currently a *copy* rather than a live link surface the
 * amber "copied — edits won't sync" marker. The control also accepts a drop
 * of a (different) skill's entry ref — dropping one switches the library
 * selection to that skill, so a quick drag from the list can jump straight
 * to configuring its deployments here.
 */
export const DeployTargetsControl: React.FC<DeployTargetsControlProps> = ({ entry, deployments, onApply }) => {
    const classesTree = useLibraryStore((s) => s.index?.sections?.classes?.tree ?? null);
    const select = useLibraryStore((s) => s.select);
    const [agents, setAgents] = useState<AgentConfig[]>([]);
    const [pending, setPending] = useState<Set<string>>(new Set());
    const [pickerOpen, setPickerOpen] = useState(false);
    const [query, setQuery] = useState('');
    const [highlighted, setHighlighted] = useState(0);
    const buttonRef = useRef<HTMLButtonElement>(null);
    const popoverRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);

    /**
     * Locally authoritative "desired" target set. Add/remove deltas are
     * computed from THIS ref, never from the `deployments` prop directly —
     * the prop is a snapshot that only advances after a full `onApply` →
     * `setSkillDeployments` → `invoke` → `fetchIndex` round trip, so two
     * rapid ops closing over the same stale prop would otherwise silently
      * undo each other. Re-synced from `deployments` in the effect below
     * whenever a fresh index lands and nothing is in flight, so queued ops
     * compound instead of reverting one another.
     */
    const desiredRef = useRef<SkillDeployment[]>(deployments.map(toSkillDeployment));
    /**
     * Chains `onApply` invocations so the underlying `set_skill_deployments`
     * calls for this skill never overlap. The backend re-scans live disk
     * state per call rather than trusting the caller's set wholesale, which
     * makes any two overlapping calls racy even when each call's own
     * `desired` set is individually correct (fresh-state accumulation alone
     * isn't enough — see finding C1's trace). `queueIdleRef` lets the first
     * op of an idle queue run its `onApply` call synchronously (matching
     * the control's long-standing single-op behavior/tests) while any op
     * that arrives before the queue drains waits its turn.
     */
    const queueRef = useRef<Promise<void>>(Promise.resolve());
    const queueIdleRef = useRef(true);

    // Close the picker, drop any in-progress search, and reset the local
    // desired-set/queue when the displayed entry changes — otherwise a
    // stale query/open popover, or a desired set computed for the
    // previously selected skill, would leak into this one.
    useEffect(() => {
        setPickerOpen(false);
        setQuery('');
        setPending(new Set());
        queueRef.current = Promise.resolve();
        queueIdleRef.current = true;
        desiredRef.current = deployments.map(toSkillDeployment);
        // Deliberately keyed only on the entry switching, not `deployments`
        // — see the re-sync effect below for the steady-state case.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [entry.entry_ref]);

    // Re-sync the local desired set from the freshest `deployments` prop
    // once every in-flight op has settled (`pending.size === 0`). Skipped
    // while an op is pending so a `deployments` prop from *before* that op
    // lands can't clobber a desired set that already accounts for it.
    useEffect(() => {
        if (pending.size === 0) {
            desiredRef.current = deployments.map(toSkillDeployment);
        }
    }, [deployments, pending]);

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
    const allTargets = useMemo<TargetOption[]>(
        () => [{ target_type: 'user', target_id: 'global', label: 'User (global)' }, ...classOptions, ...agentOptions],
        [classOptions, agentOptions],
    );

    const deployedKeys = useMemo(() => new Set(deployments.map(targetKey)), [deployments]);

    // Targets already deployed are hidden from the picker entirely (rather
    // than shown checked-and-disabled) — once `onApply` resolves and the
    // index refreshes, the newly-deployed target simply drops out of this
    // list on its own.
    const availableTargets = useMemo(
        () => allTargets.filter((t) => !deployedKeys.has(targetKey(t))),
        [allTargets, deployedKeys],
    );

    const filteredGroups = useMemo(() => {
        const q = query.trim().toLowerCase();
        const matches = (t: TargetOption) => (q ? t.label.toLowerCase().includes(q) : true);
        const groups: Record<TargetType, TargetOption[]> = { user: [], class: [], agent: [] };
        for (const t of availableTargets) {
            if (matches(t)) groups[t.target_type].push(t);
        }
        return groups;
    }, [availableTargets, query]);

    const flatFiltered = useMemo(
        () => GROUP_ORDER.flatMap((type) => filteredGroups[type]),
        [filteredGroups],
    );

    // Keep the keyboard cursor in range whenever the (filtered) list shrinks
    // or grows, e.g. after a search keystroke or a target becoming deployed.
    useEffect(() => {
        setHighlighted((h) => Math.min(h, Math.max(flatFiltered.length - 1, 0)));
    }, [flatFiltered.length]);

    useEffect(() => {
        if (pickerOpen) inputRef.current?.focus();
    }, [pickerOpen]);

    useEffect(() => {
        if (!pickerOpen) return;
        const handlePointerDown = (e: MouseEvent) => {
            const target = e.target as Node;
            if (popoverRef.current?.contains(target)) return;
            if (buttonRef.current?.contains(target)) return;
            setPickerOpen(false);
        };
        document.addEventListener('mousedown', handlePointerDown);
        return () => document.removeEventListener('mousedown', handlePointerDown);
    }, [pickerOpen]);

    /**
     * Marks `key` pending immediately, then enqueues the actual `onApply`
     * call onto `queueRef` so it runs only after every earlier queued call
     * (for this skill) has fully settled — serializing the invokes without
     * blocking the picker or other chips from queuing further ops in the
     * meantime (`pending` only affects the one chip/row's own disabled
     * state).
     */
    const applyChange = (key: string, desired: SkillDeployment[]) => {
        setPending((prev) => {
            const next = new Set(prev);
            next.add(key);
            return next;
        });
        const run = async () => {
            try {
                await onApply(desired);
            } catch {
                // The store already surfaces failures via its `error` state
                // (see LibraryView's error banner) and re-fetches the index
                // on failure — but that round trip hasn't necessarily
                // reached this component's `deployments` prop yet. Abandon
                // the local optimistic set now and re-sync from whatever
                // the store's index already has (falling back to the
                // still-current prop) so no phantom chip lingers.
                const fresh = useLibraryStore.getState().index?.deployments[entry.entry_ref];
                desiredRef.current = (fresh ?? deployments).map(toSkillDeployment);
            } finally {
                setPending((prev) => {
                    const next = new Set(prev);
                    next.delete(key);
                    return next;
                });
            }
        };
        if (queueIdleRef.current) {
            // Nothing else is in flight for this skill — run (and thus call
            // `onApply`) synchronously, matching the control's existing
            // single-op behavior.
            queueIdleRef.current = false;
            queueRef.current = run().finally(() => {
                queueIdleRef.current = true;
            });
        } else {
            // Something else is already queued/in flight — this op's
            // `onApply` call must wait for it, so overlapping
            // `set_skill_deployments` calls never race each other on disk.
            queueRef.current = queueRef.current.then(() => {
                queueIdleRef.current = false;
                return run().finally(() => {
                    queueIdleRef.current = true;
                });
            });
        }
    };

    const handleRemove = (target: DeploymentTarget) => {
        const key = targetKey(target);
        const desired = desiredRef.current.filter((d) => targetKey(d) !== key);
        desiredRef.current = desired;
        applyChange(key, desired);
    };

    const handleAdd = (target: TargetOption) => {
        const key = targetKey(target);
        const desired = [...desiredRef.current, toSkillDeployment(target)];
        desiredRef.current = desired;
        applyChange(key, desired);
    };

    const openPicker = () => {
        setQuery('');
        setHighlighted(0);
        setPickerOpen(true);
    };

    const handleSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === 'Escape') {
            e.preventDefault();
            setPickerOpen(false);
            buttonRef.current?.focus();
            return;
        }
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            setHighlighted((h) => Math.min(h + 1, flatFiltered.length - 1));
            return;
        }
        if (e.key === 'ArrowUp') {
            e.preventDefault();
            setHighlighted((h) => Math.max(h - 1, 0));
            return;
        }
        if (e.key === 'Enter') {
            e.preventDefault();
            const target = flatFiltered[highlighted];
            if (target && !pending.has(targetKey(target))) handleAdd(target);
        }
    };

    const handleDrop = (e: React.DragEvent) => {
        e.preventDefault();
        const droppedRef = e.dataTransfer.getData(ENTRY_REF_MIME);
        if (droppedRef && droppedRef.startsWith('skills/') && droppedRef !== entry.entry_ref) {
            void select(droppedRef);
        }
    };

    const anchorRect = pickerOpen ? buttonRef.current?.getBoundingClientRect() ?? null : null;
    // Position the popover to avoid going off-screen (mirrors the
    // off-screen adjustment ContextMenu already does). The "+ Add
    // target…" button sits directly under the chips it deploys/removes
    // from, so when there isn't enough room to open downward, anchor beside
    // the button instead of above it — flipping upward would cover the
    // very chips the picker is meant to be read alongside.
    const POPOVER_WIDTH = 256;
    const POPOVER_MAX_HEIGHT = 320;
    const popoverStyle: React.CSSProperties = { position: 'fixed', top: 0, left: 0 };
    if (anchorRect) {
        const spaceBelow = window.innerHeight - anchorRect.bottom;
        if (spaceBelow >= 140) {
            popoverStyle.left = anchorRect.left;
            popoverStyle.top = anchorRect.bottom + 4;
            popoverStyle.maxHeight = Math.min(POPOVER_MAX_HEIGHT, spaceBelow - 8);
        } else {
            const spaceRight = window.innerWidth - anchorRect.right;
            const openLeft = spaceRight < POPOVER_WIDTH + 16;
            popoverStyle.left = openLeft ? anchorRect.left - POPOVER_WIDTH - 8 : anchorRect.right + 8;
            const maxHeight = Math.min(POPOVER_MAX_HEIGHT, window.innerHeight - 16);
            // Anchor the popover's bottom edge flush with the button's
            // bottom edge (rather than aligning tops, which could clamp the
            // popover well above the button and read as visually
            // disconnected from its trigger — see
            // deploy-redesign-review.md Minor #1), clamped so it never
            // runs off the top of the viewport.
            popoverStyle.top = Math.max(8, anchorRect.bottom - maxHeight);
            popoverStyle.maxHeight = maxHeight;
        }
    }

    return (
        <div
            data-testid="deploy-targets-control"
            onDragOver={(e) => e.preventDefault()}
            onDrop={handleDrop}
            className="flex flex-col gap-2"
        >
            <h4 className="text-xs font-bold text-muted">Deployed to ({deployments.length})</h4>
            <div className="flex flex-wrap gap-1.5" data-testid="deploy-chips">
                {deployments.length === 0 && <span className="text-[11px] text-muted">Not deployed anywhere</span>}
                {deployments.map((target) => {
                    const key = targetKey(target);
                    const label = labelFor(target, allTargets);
                    const copied = !target.linked;
                    const isPending = pending.has(key);
                    return (
                        <span
                            key={key}
                            data-testid={`deploy-chip-${key}`}
                            className="inline-flex max-w-full items-center gap-1 rounded-full border border-wardian-border bg-wardian-card-bg-muted px-2 py-0.5 text-[11px] text-primary"
                        >
                            <span className="truncate max-w-[160px]">{label}</span>
                            {copied && (
                                <span
                                    data-testid={`deploy-chip-copied-${key}`}
                                    className="shrink-0 text-wardian-warning"
                                    title="copied — edits won't sync"
                                >
                                    ●
                                </span>
                            )}
                            <button
                                type="button"
                                data-testid={`deploy-chip-remove-${key}`}
                                aria-label={`Remove ${label}`}
                                disabled={isPending}
                                onClick={() => handleRemove(target)}
                                className="shrink-0 rounded-full p-0.5 text-muted transition-colors hover:text-primary disabled:cursor-not-allowed disabled:opacity-40"
                            >
                                <X className="h-3 w-3" aria-hidden />
                            </button>
                        </span>
                    );
                })}
            </div>
            <button
                type="button"
                ref={buttonRef}
                data-testid="deploy-targets-add-button"
                onClick={() => (pickerOpen ? setPickerOpen(false) : openPicker())}
                aria-expanded={pickerOpen}
                className="self-start rounded border border-wardian-border px-3 py-1 text-xs text-primary transition-colors hover:bg-wardian-card-bg-muted"
            >
                + Add target…
            </button>
            {pickerOpen &&
                createPortal(
                    <div
                        ref={popoverRef}
                        data-testid="deploy-picker"
                        role="listbox"
                        aria-label="Add deploy target"
                        style={popoverStyle}
                        className="flex w-64 flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] shadow-2xl z-[1000]"
                    >
                        <div className="shrink-0 border-b border-wardian-border p-2">
                            <input
                                ref={inputRef}
                                type="search"
                                data-testid="deploy-picker-search"
                                aria-label="Search deploy targets"
                                value={query}
                                onChange={(e) => setQuery(e.currentTarget.value)}
                                onKeyDown={handleSearchKeyDown}
                                placeholder="Search targets…"
                                className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)]"
                            />
                        </div>
                        <div className="min-h-0 flex-1 overflow-y-auto p-1">
                            {flatFiltered.length === 0 && (
                                <div className="p-2 text-[11px] text-muted">No matching targets</div>
                            )}
                            {GROUP_ORDER.map((type) => {
                                const options = filteredGroups[type];
                                if (options.length === 0) return null;
                                return (
                                    <div key={type} className="mb-1">
                                        <div
                                            data-testid={`deploy-picker-group-${type}`}
                                            className="px-2 py-1 text-[10px] font-bold text-muted"
                                        >
                                            {GROUP_LABELS[type]} ({options.length})
                                        </div>
                                        {options.map((option) => {
                                            const key = targetKey(option);
                                            const index = flatFiltered.indexOf(option);
                                            const isHighlighted = index === highlighted;
                                            const isPending = pending.has(key);
                                            return (
                                                <button
                                                    key={key}
                                                    type="button"
                                                    role="option"
                                                    aria-selected={isHighlighted}
                                                    data-testid={`deploy-picker-option-${key}`}
                                                    disabled={isPending}
                                                    onMouseEnter={() => setHighlighted(index)}
                                                    onClick={() => handleAdd(option)}
                                                    className={`flex w-full items-center rounded px-2 py-1 text-left text-xs text-primary transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                                                        isHighlighted ? 'bg-wardian-card-bg-muted' : 'hover:bg-wardian-card-bg-muted'
                                                    }`}
                                                >
                                                    <span className="truncate">{option.label}</span>
                                                </button>
                                            );
                                        })}
                                    </div>
                                );
                            })}
                        </div>
                    </div>,
                    document.body,
                )}
        </div>
    );
};
