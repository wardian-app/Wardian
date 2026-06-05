import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';

import { useSettingsStore } from '../../store/useSettingsStore';
import type { AgentConfig, ProviderReadiness, UserFacingProviderName } from '../../types';
import {
  buildProviderOptions,
  buildUngatedProviderOptions,
  isUserFacingProviderName,
  resolveEffectiveProvider,
} from '../agents/providerOptions';
import { ScheduleEditor } from './ScheduleEditor';
import type { Blueprint } from './builder/blueprintTypes';
import type {
  WorkflowAgentConversation,
  WorkflowAssignments,
  WorkflowBusyPolicy,
  ScheduleDefinition,
  WorkflowSchedule,
} from '../../types/workflow';

export interface RunInputParam {
  name: string;
  type: 'string' | 'number' | 'boolean';
}

const EMPTY_INPUT_PARAMS: RunInputParam[] = [];
const PROVIDER_TARGET_PREFIX = 'provider:';
const AGENT_TARGET_PREFIX = 'agent:';

interface RunLaunchDialogProps {
  path: string;
  blueprintId?: string;
  blueprint?: Blueprint | null;
  inputParams?: RunInputParam[];
  onLaunched: (runId: string) => void;
  onCancel: () => void;
  onScheduled?: () => void;
  editSchedule?: WorkflowSchedule;
}

export function RunLaunchDialog({
  path,
  blueprintId,
  blueprint,
  inputParams = EMPTY_INPUT_PARAMS,
  onLaunched,
  onCancel,
  onScheduled,
  editSchedule,
}: RunLaunchDialogProps) {
  const defaultProvider = useSettingsStore((state) => state.default_provider);
  const [providerReadiness, setProviderReadiness] = useState<ProviderReadiness[] | null>(null);
  const editProvider = isUserFacingProviderName(editSchedule?.provider) ? editSchedule.provider : null;
  const [provider, setProvider] = useState<UserFacingProviderName>(editProvider ?? 'claude');
  const [providerTouched, setProviderTouched] = useState(Boolean(editProvider));
  const [providerNote, setProviderNote] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState(editSchedule?.workspace ?? '');
  const invocation = useMemo(() => workflowInvocationFromBlueprint(blueprint), [blueprint]);
  const roles = invocation.roles;
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const roleKey = useMemo(() => roles.map((role) => role.name).join('|'), [roles]);
  const roleBindingsKey = `${roleKey}|${editSchedule?.id ?? ''}`;
  const previousRoleBindingsKey = useRef(roleBindingsKey);
  const [roleBindings, setRoleBindings] = useState<Record<string, string>>(
    () => initialRoleTargets(editSchedule?.bindings, editSchedule?.assignments),
  );
  const [roleConversations, setRoleConversations] = useState<Record<string, WorkflowAgentConversation>>(
    () => initialRoleConversations(editSchedule?.assignments),
  );
  const [roleBusyPolicies, setRoleBusyPolicies] = useState<Record<string, WorkflowBusyPolicy>>(
    () => initialRoleBusyPolicies(editSchedule?.assignments),
  );
  const inputParamKey = useMemo(
    () => inputParams.map((param) => `${param.name}:${param.type}`).join('|'),
    [inputParams],
  );
  const inputValuesKey = `${inputParamKey}|${editSchedule?.id ?? ''}`;
  const previousInputParamKey = useRef(inputValuesKey);
  const [inputValues, setInputValues] = useState<Record<string, string | boolean>>(
    () => initialInputValues(inputParams, editSchedule?.input),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launchMode, setLaunchMode] = useState<'run' | 'schedule'>(editSchedule ? 'schedule' : 'run');
  const [pickerRole, setPickerRole] = useState<string | null>(null);
  const [agentSearch, setAgentSearch] = useState('');
  const [scheduleName, setScheduleName] = useState(editSchedule?.name ?? '');
  const [scheduleDef, setScheduleDef] = useState<Partial<ScheduleDefinition>>(
    editSchedule?.schedule ?? { schedule_type: 'interval', interval_minutes: 60, active: true },
  );

  useEffect(() => {
    if (previousInputParamKey.current === inputValuesKey) return;
    previousInputParamKey.current = inputValuesKey;
    setInputValues(initialInputValues(inputParams, editSchedule?.input));
  }, [editSchedule?.input, inputParams, inputValuesKey]);

  useEffect(() => {
    let cancelled = false;

    invoke<ProviderReadiness[]>('list_provider_readiness')
      .then((readiness) => {
        if (!cancelled) {
          setProviderReadiness(readiness);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setProviderNote('Unable to check provider readiness.');
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    invoke<AgentConfig[]>('list_agents')
      .then((nextAgents) => {
        if (!cancelled) {
          setAgents(Array.isArray(nextAgents) ? nextAgents : []);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setAgents([]);
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (previousRoleBindingsKey.current === roleBindingsKey) return;
    previousRoleBindingsKey.current = roleBindingsKey;
    setRoleBindings(initialRoleTargets(editSchedule?.bindings, editSchedule?.assignments));
    setRoleConversations(initialRoleConversations(editSchedule?.assignments));
    setRoleBusyPolicies(initialRoleBusyPolicies(editSchedule?.assignments));
  }, [editSchedule?.assignments, editSchedule?.bindings, roleBindingsKey]);

  const providerOptions = useMemo(
    () => (providerReadiness ? buildProviderOptions(providerReadiness) : buildUngatedProviderOptions()),
    [providerReadiness],
  );

  const selectedProviderAvailable = providerReadiness
    ? providerOptions.some((option) => option.value === provider && option.available)
    : true;
  const selectedRoleTargets = useMemo(
    () => Object.fromEntries(roles.map((role) => [role.name, roleBindings[role.name] || providerTarget(provider)])),
    [provider, roleBindings, roles],
  );
  const roleProviderSelections = Object.values(selectedRoleTargets)
    .filter((target) => target.startsWith(PROVIDER_TARGET_PREFIX))
    .map((target) => target.slice(PROVIDER_TARGET_PREFIX.length));
  const roleProvidersAvailable = providerReadiness
    ? roleProviderSelections.every((roleProvider) => (
      providerOptions.some((option) => option.value === roleProvider && option.available)
    ))
    : true;
  const showGlobalProvider = !blueprint || invocation.hasGlobalProviderBackedAgent;
  const hasFreshRoleAssignments = roleProviderSelections.length > 0;
  const showWorkspace = !blueprint
    || invocation.needsRunWorkspace
    || invocation.hasGlobalProviderBackedAgent
    || hasFreshRoleAssignments;
  const launchProviderAvailable = (!showGlobalProvider || selectedProviderAvailable) && roleProvidersAvailable;

  useEffect(() => {
    if (!providerReadiness) return;

    const selectedOption = providerOptions.find((option) => option.value === provider);
    if (providerTouched && selectedOption?.available) {
      setProviderNote(null);
      return;
    }

    const resolved = resolveEffectiveProvider(providerReadiness, defaultProvider);
    setProviderNote(resolved.note);
    if (resolved.provider) {
      setProvider(resolved.provider);
    }
  }, [defaultProvider, provider, providerOptions, providerReadiness, providerTouched]);

  const run = async () => {
    if (!launchProviderAvailable) {
      setProviderNote('Choose an installed provider before running this workflow.');
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const input = collectInput(inputParams, inputValues);
      const bindings = collectBindings(roles, roleBindings, provider);
      const assignments = collectAssignments(roles, selectedRoleTargets, roleConversations, roleBusyPolicies, launchMode, workspace);
      const res = await invoke<{ ok: boolean; run_id?: string; diagnostics?: unknown[] }>('workflow_run', {
        path,
        ...(showGlobalProvider ? { provider } : {}),
        ...(showWorkspace && workspace ? { workspace } : {}),
        ...(inputParams.length > 0 ? { input } : {}),
        ...(Object.keys(bindings).length > 0 ? { bindings } : {}),
        ...(Object.keys(assignments).length > 0 ? { assignments } : {}),
      });

      if (res.ok && res.run_id) {
        onLaunched(res.run_id);
      } else {
        setError('Blueprint is invalid. Fix diagnostics before running.');
      }
    } catch (launchError) {
      setError(String(launchError));
    } finally {
      setBusy(false);
    }
  };

  const saveSchedule = async () => {
    if (!blueprintId) {
      setError('No blueprint selected to schedule.');
      return;
    }
    if (!launchProviderAvailable) {
      setProviderNote('Choose an installed provider before scheduling this workflow.');
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const input = collectInput(inputParams, inputValues);
      const schedule = { active: true, ...scheduleDef } as ScheduleDefinition;
      if (editSchedule) {
        await invoke('schedule_remove', { id: editSchedule.id });
      }
      const bindings = collectBindings(roles, roleBindings, provider);
      const assignments = collectAssignments(roles, selectedRoleTargets, roleConversations, roleBusyPolicies, launchMode, workspace);
      await invoke('schedule_create', {
        blueprintId,
        name: scheduleName || blueprintId,
        schedule,
        ...(showGlobalProvider ? { provider } : {}),
        ...(showWorkspace && workspace ? { workspace } : {}),
        ...(inputParams.length > 0 ? { input } : editSchedule ? { input: editSchedule.input } : {}),
        ...(Object.keys(bindings).length > 0 ? { bindings } : {}),
        ...(Object.keys(assignments).length > 0 ? { assignments } : {}),
      });
      onScheduled?.();
    } catch (scheduleError) {
      setError(String(scheduleError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="run-launch-dialog rounded border border-wardian-border bg-[var(--color-wardian-card)] p-4 text-primary"
      role="dialog"
      data-testid="run-launch-dialog"
    >
      <h3 className="mb-2 text-sm font-semibold">Run workflow</h3>
      <div className="mb-3 flex rounded border border-wardian-border p-0.5" role="radiogroup" aria-label="Launch mode">
        {(['run', 'schedule'] as const).map((mode) => (
          <button
            key={mode}
            type="button"
            role="radio"
            aria-checked={launchMode === mode}
            aria-label={mode === 'run' ? 'Run now' : 'Schedule'}
            className={`flex-1 rounded px-3 py-1 text-xs font-bold capitalize ${
              launchMode === mode
                ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)]'
                : 'text-muted'
            }`}
            onClick={() => setLaunchMode(mode)}
          >
            {mode === 'run' ? 'Run now' : 'Schedule'}
          </button>
        ))}
      </div>
      {roles.length > 0 && (
        <div className="mb-3 grid gap-2 border-t border-wardian-border pt-3">
          <div>
            <h4 className="text-xs font-bold text-primary">Agent assignments</h4>
            <p className="mt-0.5 text-[10px] text-muted">Choose an existing agent or a workflow-owned temporary agent for each role.</p>
          </div>
          {roles.map((role) => {
            const id = `run-role-${role.name}`;
            const selectedTarget = selectedRoleTargets[role.name] ?? providerTarget(provider);
            const pickerOpen = pickerRole === role.name;
            return (
              <div key={role.name} className="relative rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-2">
                <div className="flex min-w-0 items-start justify-between gap-2">
                  <div className="min-w-0">
                    <div id={id} className="text-xs font-bold text-primary">
                      {role.name}
                    </div>
                    <div className="mt-0.5 truncate text-[11px] text-muted" title={roleTargetSummary(selectedTarget, providerOptions, agents)}>
                      {roleTargetSummary(selectedTarget, providerOptions, agents)}
                    </div>
                  </div>
                  <button
                    type="button"
                    aria-label={`Change ${role.name} assignment`}
                    aria-expanded={pickerOpen}
                    aria-controls={`${id}-picker`}
                    className="shrink-0 rounded border border-wardian-border px-2 py-1 text-[10px] font-bold text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                    onClick={() => {
                      setPickerRole(pickerOpen ? null : role.name);
                      setAgentSearch('');
                    }}
                  >
                    Change
                  </button>
                </div>
                {pickerOpen ? (
                  <RoleAssignmentPicker
                    id={`${id}-picker`}
                    agents={agents}
                    providerOptions={providerOptions}
                    selectedTarget={selectedTarget}
                    search={agentSearch}
                    onSearch={setAgentSearch}
                    onSelect={(value) => {
                      setRoleBindings((current) => ({ ...current, [role.name]: value }));
                      setPickerRole(null);
                      setAgentSearch('');
                    }}
                  />
                ) : null}
                {selectedTarget.startsWith(AGENT_TARGET_PREFIX) && (
                  <div className="mt-2 grid grid-cols-2 gap-2">
                    <div>
                      <label className="mb-1 block text-[10px] font-bold text-muted" htmlFor={`${id}-conversation`}>
                        Conversation
                      </label>
                      <select
                        id={`${id}-conversation`}
                        className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
                        value={roleConversations[role.name] ?? 'current'}
                        onChange={(event) => {
                          const value = event.currentTarget.value as WorkflowAgentConversation;
                          setRoleConversations((current) => ({ ...current, [role.name]: value }));
                        }}
                      >
                        <option value="current">Current conversation</option>
                        <option value="fresh_background">Separate background conversation</option>
                      </select>
                    </div>
                    {(roleConversations[role.name] ?? 'current') === 'current' && (
                      <div>
                        <label className="mb-1 block text-[10px] font-bold text-muted" htmlFor={`${id}-busy`}>
                          When busy
                        </label>
                        <select
                          id={`${id}-busy`}
                          className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
                          value={roleBusyPolicies[role.name] ?? (launchMode === 'schedule' ? 'skip' : 'fail')}
                          onChange={(event) => {
                            const value = event.currentTarget.value as WorkflowBusyPolicy;
                            setRoleBusyPolicies((current) => ({ ...current, [role.name]: value }));
                          }}
                        >
                          <option value="skip">Skip this run</option>
                          <option value="fail">Fail this run</option>
                        </select>
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
      {(showGlobalProvider || showWorkspace) && (
        <div className={`${roles.length > 0 ? 'mb-3 grid gap-2 border-t border-wardian-border pt-3' : 'mb-3 grid gap-2'}`}>
          {roles.length > 0 ? (
            <div>
              <h4 className="text-xs font-bold text-primary">Run context</h4>
              <p className="mt-0.5 text-[10px] text-muted">Used only by fresh agents and local nodes that need the run workspace.</p>
            </div>
          ) : null}
          {showGlobalProvider && (
            <div>
              <label className="mb-1 block text-xs text-muted" htmlFor="run-provider">
                Provider
              </label>
              <select
                id="run-provider"
                className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
                value={provider}
                onChange={(event) => {
                  setProviderTouched(true);
                  setProvider(event.currentTarget.value as UserFacingProviderName);
                }}
              >
                {providerOptions.map((option) => (
                  <option key={option.value} value={option.value} disabled={!option.available}>
                    {option.label}
                  </option>
                ))}
              </select>
            </div>
          )}
          {providerNote && <div className="text-xs text-wardian-warning">{providerNote}</div>}
          {showWorkspace && (
            <div>
              <label className="mb-1 block text-xs text-muted" htmlFor="run-workspace">
                Workspace (optional)
              </label>
              <input
                id="run-workspace"
                className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
                value={workspace}
                onChange={(event) => setWorkspace(event.currentTarget.value)}
                placeholder="Defaults to the run directory"
              />
            </div>
          )}
        </div>
      )}
      {inputParams.length > 0 && (
        <div className="mb-3 grid gap-2 border-t border-wardian-border pt-3">
          {inputParams.map((param) => {
            const id = `run-param-${param.name}`;
            if (param.type === 'boolean') {
              return (
                <label key={param.name} className="flex items-center gap-2 text-xs text-muted" htmlFor={id}>
                  <input
                    id={id}
                    type="checkbox"
                    className="h-4 w-4 accent-[var(--color-wardian-accent)]"
                    checked={Boolean(inputValues[param.name])}
                    onChange={(event) => {
                      const checked = event.currentTarget.checked;
                      setInputValues((current) => ({
                        ...current,
                        [param.name]: checked,
                      }));
                    }}
                  />
                  {param.name}
                </label>
              );
            }
            return (
              <div key={param.name}>
                <label className="mb-1 block text-xs text-muted" htmlFor={id}>
                  {param.name}
                </label>
                <input
                  id={id}
                  type={param.type === 'number' ? 'number' : 'text'}
                  className="w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
                  value={String(inputValues[param.name] ?? '')}
                  onChange={(event) => {
                    const value = event.currentTarget.value;
                    setInputValues((current) => ({
                      ...current,
                      [param.name]: value,
                    }));
                  }}
                />
              </div>
            );
          })}
        </div>
      )}
      {launchMode === 'schedule' && (
        <div className="mb-3 border-t border-wardian-border pt-3">
          <label className="mb-1 block text-xs text-muted" htmlFor="schedule-name">
            Schedule name
          </label>
          <input
            id="schedule-name"
            className="mb-2 w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
            value={scheduleName}
            onChange={(event) => setScheduleName(event.currentTarget.value)}
          />
          <ScheduleEditor value={scheduleDef} onChange={setScheduleDef} compact />
        </div>
      )}
      {error && <div className="mb-2 text-xs text-wardian-error">{error}</div>}
      <div className="flex justify-end gap-2">
        <button
          type="button"
          className="rounded border border-wardian-border px-3 py-1 text-xs text-primary"
          onClick={onCancel}
        >
          Cancel
        </button>
        {launchMode === 'run' ? (
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-accent)] px-3 py-1 text-xs text-[var(--color-wardian-bg)] disabled:bg-wardian-off/30"
            disabled={busy || !launchProviderAvailable}
            onClick={run}
          >
            Run
          </button>
        ) : (
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-accent)] px-3 py-1 text-xs text-[var(--color-wardian-bg)] disabled:bg-wardian-off/30"
            disabled={busy || !launchProviderAvailable}
            onClick={saveSchedule}
          >
            Save schedule
          </button>
        )}
      </div>
    </div>
  );
}

interface RoleAssignmentPickerProps {
  id: string;
  agents: AgentConfig[];
  providerOptions: ReturnType<typeof buildProviderOptions>;
  selectedTarget: string;
  search: string;
  onSearch: (value: string) => void;
  onSelect: (value: string) => void;
}

function RoleAssignmentPicker({
  id,
  agents,
  providerOptions,
  selectedTarget,
  search,
  onSearch,
  onSelect,
}: RoleAssignmentPickerProps) {
  const visibleAgents = filterAgentsForPicker(agents, search, selectedTarget);
  const filteredTotal = countMatchingAgents(agents, search);
  const omittedCount = Math.max(0, filteredTotal - visibleAgents.length);

  return (
    <div
      id={id}
      className="absolute left-2 right-2 top-[calc(100%-0.25rem)] z-30 rounded border border-wardian-border bg-[var(--color-wardian-card)] p-2 shadow-xl"
    >
      <input
        type="search"
        aria-label="Search agents"
        value={search}
        onChange={(event) => onSearch(event.currentTarget.value)}
        placeholder="Search by name, class, provider, or workspace"
        className="mb-2 w-full rounded border border-wardian-border bg-[var(--color-wardian-bg)] px-2 py-1 text-xs text-primary"
        autoFocus
      />
      <PickerGroup title="Temporary agents">
        <div className="grid grid-cols-2 gap-1">
          {providerOptions.map((option) => {
            const target = providerTarget(option.value);
            return (
              <button
                key={option.value}
                type="button"
                className={pickerButtonClass(target === selectedTarget)}
                disabled={!option.available}
                onClick={() => onSelect(target)}
              >
                <span className="truncate text-left">New temporary {option.label}</span>
                {!option.available ? <span className="text-[9px] text-wardian-warning">Unavailable</span> : null}
              </button>
            );
          })}
        </div>
      </PickerGroup>
      <PickerGroup title="Existing agents">
        {visibleAgents.length > 0 ? (
          <div className="grid max-h-60 gap-1 overflow-y-auto pr-1">
            {visibleAgents.map((agent) => {
              const target = agentTarget(agent.session_id);
              return (
                <button
                  key={agent.session_id}
                  type="button"
                  className={pickerButtonClass(target === selectedTarget)}
                  onClick={() => onSelect(target)}
                >
                  <span className="flex min-w-0 items-center justify-between gap-2">
                    <span className="truncate text-left font-bold">{agent.session_name || agent.session_id}</span>
                    <span className="shrink-0 rounded border border-wardian-border px-1 py-0.5 text-[9px] text-muted">
                      {agent.is_off ? 'Off' : 'Idle'}
                    </span>
                  </span>
                  <span className="truncate text-left text-[10px] text-muted">
                    {[agent.agent_class, agent.provider, workspaceLabel(agent.folder)].filter(Boolean).join(' - ')}
                  </span>
                </button>
              );
            })}
          </div>
        ) : (
          <div className="rounded border border-dashed border-wardian-border p-2 text-xs text-muted">
            No matching agents.
          </div>
        )}
        {omittedCount > 0 ? (
          <div className="mt-1 text-[10px] text-muted">
            {omittedCount} more matching agents. Search to narrow the list.
          </div>
        ) : null}
      </PickerGroup>
    </div>
  );
}

function PickerGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="mb-2 last:mb-0">
      <div className="mb-1 text-[10px] font-bold text-muted">{title}</div>
      {children}
    </div>
  );
}

function pickerButtonClass(selected: boolean) {
  return `min-w-0 rounded border px-2 py-1 text-xs disabled:cursor-not-allowed disabled:opacity-50 ${
    selected
      ? 'border-[var(--color-wardian-accent)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_88%)] text-primary'
      : 'border-wardian-border bg-[var(--color-wardian-bg)] text-primary hover:border-[var(--color-wardian-accent)]'
  }`;
}

function collectInput(inputParams: RunInputParam[], inputValues: Record<string, string | boolean>) {
  return Object.fromEntries(inputParams.map((param) => {
    const value = inputValues[param.name];
    if (param.type === 'boolean') return [param.name, Boolean(value)];
    if (param.type === 'number') return [param.name, Number(value ?? 0)];
    return [param.name, String(value ?? '')];
  }));
}

function collectBindings(
  roles: Array<{ name: string }>,
  roleBindings: Record<string, string>,
  defaultProvider: UserFacingProviderName,
) {
  const entries = roles.length > 0
    ? roles.map((role) => [role.name, roleBindings[role.name] || providerTarget(defaultProvider)] as const)
    : Object.entries(roleBindings);

  return Object.fromEntries(
    entries
      .map(([role, target]) => [role, bindingValue(target)] as const)
      .filter(([, value]) => value.trim().length > 0),
  );
}

function collectAssignments(
  roles: Array<{ name: string }>,
  selectedRoleTargets: Record<string, string>,
  roleConversations: Record<string, WorkflowAgentConversation>,
  roleBusyPolicies: Record<string, WorkflowBusyPolicy>,
  launchMode: 'run' | 'schedule',
  workspace: string,
): WorkflowAssignments {
  const entries: Array<[string, WorkflowAssignments[string]]> = roles.map((role) => {
      const target = selectedRoleTargets[role.name];
      if (target?.startsWith(AGENT_TARGET_PREFIX)) {
        const conversation = roleConversations[role.name] ?? 'current';
        return [role.name, {
          target_type: 'agent',
          agent_id: target.slice(AGENT_TARGET_PREFIX.length),
          conversation,
          ...(conversation === 'current'
            ? { busy_policy: roleBusyPolicies[role.name] ?? (launchMode === 'schedule' ? 'skip' : 'fail') }
            : {}),
        }];
      }
      const provider = target?.startsWith(PROVIDER_TARGET_PREFIX)
        ? target.slice(PROVIDER_TARGET_PREFIX.length)
        : '';
      return [role.name, {
        target_type: 'temporary_provider',
        provider,
        ...(workspace.trim() ? { workspace: workspace.trim() } : {}),
      }];
    });

  return Object.fromEntries(
    entries.filter(([, assignment]) => {
      if (assignment.target_type === 'agent') return assignment.agent_id.trim().length > 0;
      return assignment.provider.trim().length > 0;
    }),
  );
}

function workflowInvocationFromBlueprint(blueprint?: Blueprint | null) {
  if (!blueprint) {
    return { roles: [], hasGlobalProviderBackedAgent: true, needsRunWorkspace: true };
  }
  const seen = new Set<string>();
  const roles: Array<{ name: string }> = [];
  let hasGlobalProviderBackedAgent = false;
  let needsRunWorkspace = false;

  for (const node of blueprint.nodes) {
    if (node.type === 'shell' && !stringField(node.fields?.cwd)) {
      needsRunWorkspace = true;
    }
    if (node.type === 'script') {
      needsRunWorkspace = true;
    }

    if (node.type !== 'task' && node.type !== 'decision') continue;

    const agent = node.fields?.agent;
    if (typeof agent !== 'string' || agent.trim() === '' || agent === 'ephemeral') {
      hasGlobalProviderBackedAgent = true;
      continue;
    }
    const prefix = agent.startsWith('role:') ? 'role:' : agent.startsWith('class:') ? 'class:' : null;
    if (!prefix) {
      hasGlobalProviderBackedAgent = true;
      continue;
    }
    const name = agent.slice(prefix.length).trim();
    if (!name || seen.has(name)) continue;
    seen.add(name);
    roles.push({ name });
  }

  return { roles, hasGlobalProviderBackedAgent, needsRunWorkspace };
}

function agentLabel(agent: AgentConfig) {
  const name = agent.session_name || agent.session_id;
  const provider = agent.provider ? ` - ${agent.provider}` : '';
  const state = agent.is_off ? ' (off)' : '';
  return `${name}${provider}${state}`;
}

function roleTargetSummary(
  target: string,
  providerOptions: ReturnType<typeof buildProviderOptions>,
  agents: AgentConfig[],
) {
  if (target.startsWith(PROVIDER_TARGET_PREFIX)) {
    const providerValue = target.slice(PROVIDER_TARGET_PREFIX.length);
    const providerOption = providerOptions.find((option) => option.value === providerValue);
    return `New temporary ${providerOption?.label ?? providerValue} agent`;
  }
  if (target.startsWith(AGENT_TARGET_PREFIX)) {
    const agentId = target.slice(AGENT_TARGET_PREFIX.length);
    const agent = agents.find((candidate) => candidate.session_id === agentId);
    return agent ? `Use ${agentLabel(agent)}` : `Use agent ${agentId}`;
  }
  return target;
}

function filterAgentsForPicker(agents: AgentConfig[], search: string, selectedTarget: string) {
  const query = search.trim().toLowerCase();
  const selectedAgentId = selectedTarget.startsWith(AGENT_TARGET_PREFIX)
    ? selectedTarget.slice(AGENT_TARGET_PREFIX.length)
    : null;
  const matches = query
    ? agents.filter((agent) => agentSearchText(agent).includes(query))
    : agents;
  const sorted = [...matches].sort(compareAgentsForPicker);
  const visible = sorted.slice(0, query ? 12 : 8);
  if (selectedAgentId && !visible.some((agent) => agent.session_id === selectedAgentId)) {
    const selectedAgent = agents.find((agent) => agent.session_id === selectedAgentId);
    if (selectedAgent && (!query || agentSearchText(selectedAgent).includes(query))) {
      return [selectedAgent, ...visible.slice(0, Math.max(0, visible.length - 1))];
    }
  }
  return visible;
}

function countMatchingAgents(agents: AgentConfig[], search: string) {
  const query = search.trim().toLowerCase();
  if (!query) return agents.length;
  return agents.filter((agent) => agentSearchText(agent).includes(query)).length;
}

function compareAgentsForPicker(a: AgentConfig, b: AgentConfig) {
  const statusDelta = Number(a.is_off) - Number(b.is_off);
  if (statusDelta !== 0) return statusDelta;
  return (a.session_name || a.session_id).localeCompare(b.session_name || b.session_id);
}

function agentSearchText(agent: AgentConfig) {
  return [
    agent.session_name,
    agent.session_id,
    agent.agent_class,
    agent.provider,
    agent.folder,
  ].filter(Boolean).join(' ').toLowerCase();
}

function workspaceLabel(folder: string) {
  const normalized = folder.replace(/\\/g, '/');
  const parts = normalized.split('/').filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : folder;
}

function initialInputValues(inputParams: RunInputParam[], initialInput?: unknown) {
  const initialInputRecord = isRecord(initialInput) ? initialInput : {};
  return Object.fromEntries(
    inputParams.map((param) => {
      if (Object.prototype.hasOwnProperty.call(initialInputRecord, param.name)) {
        const value = initialInputRecord[param.name];
        if (param.type === 'boolean') return [param.name, Boolean(value)];
        return [param.name, String(value ?? '')];
      }
      return [param.name, param.type === 'boolean' ? false : ''];
    }),
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function providerTarget(provider: string) {
  return `${PROVIDER_TARGET_PREFIX}${provider}`;
}

function agentTarget(sessionId: string) {
  return `${AGENT_TARGET_PREFIX}${sessionId}`;
}

function bindingValue(target: string) {
  if (target.startsWith(PROVIDER_TARGET_PREFIX)) {
    return target.slice(PROVIDER_TARGET_PREFIX.length);
  }
  if (target.startsWith(AGENT_TARGET_PREFIX)) {
    return target.slice(AGENT_TARGET_PREFIX.length);
  }
  return target;
}

function initialRoleTargets(bindings?: Record<string, string>, assignments?: WorkflowAssignments) {
  if (assignments && Object.keys(assignments).length > 0) {
    return Object.fromEntries(
      Object.entries(assignments).map(([role, assignment]) => [
        role,
        assignment.target_type === 'agent'
          ? agentTarget(assignment.agent_id)
          : providerTarget(assignment.provider),
      ]),
    );
  }
  return Object.fromEntries(
    Object.entries(bindings ?? {}).map(([role, target]) => [
      role,
      isUserFacingProviderName(target) ? providerTarget(target) : agentTarget(target),
    ]),
  );
}

function initialRoleConversations(assignments?: WorkflowAssignments) {
  return Object.fromEntries(
    Object.entries(assignments ?? {})
      .filter(([, assignment]) => assignment.target_type === 'agent')
      .map(([role, assignment]) => [
        role,
        assignment.target_type === 'agent' ? assignment.conversation : 'current',
      ]),
  );
}

function initialRoleBusyPolicies(assignments?: WorkflowAssignments) {
  return Object.fromEntries(
    Object.entries(assignments ?? {})
      .filter(([, assignment]) => assignment.target_type === 'agent')
      .map(([role, assignment]) => [
        role,
        assignment.target_type === 'agent' ? assignment.busy_policy ?? 'fail' : 'fail',
      ]),
  );
}

function stringField(value: unknown) {
  return typeof value === 'string' && value.trim().length > 0;
}
