import React, { useState } from 'react';
import type { AgentConfig } from '../../types';
import type { ActiveRunTracker, ScheduledRun, WorkflowDefinition } from '../../types/workflow';
import { ContextMenu, type ContextMenuItem } from '../../components/ContextMenu';

const StopIcon = () => <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 20 20"><path d="M5 5h10v10H5z" /></svg>;
const PlayIcon = () => <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 20 20"><path d="M4 4l12 6-12 6z" /></svg>;
const PauseIcon = () => <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 20 20"><path d="M5 4h3v12H5zm7 0h3v12h-3z" /></svg>;
const RunNowIcon = () => <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><path d="M13 3L4 14h7l-1 7 9-11h-7l1-7z" /></svg>;
const TrashIcon = () => <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><path d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" /></svg>;
const ClockIcon = () => <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></svg>;
const ChevronRightIcon = ({ expanded = false }: { expanded?: boolean }) => (
  <svg className={`w-3 h-3 transition-transform ${expanded ? 'rotate-90' : 'rotate-0'}`} fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2">
    <path d="M9 6l6 6-6 6" />
  </svg>
);

function formatNextRun(epochMs: number | null): string {
  if (!epochMs) return '—';
  const diff = epochMs - Date.now();
  if (diff <= 0) return 'Due';
  const minutes = Math.floor(diff / 60000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d`;
}

function formatScheduleSummary(schedule: ScheduledRun['schedule']): string {
  let base = "";
  switch (schedule.schedule_type) {
    case 'interval': {
      const mins = schedule.interval_minutes || 0;
      base = mins >= 60 && mins % 60 === 0 ? `Every ${mins / 60}h` : `Every ${mins}m`;
      break;
    }
    case 'daily':
      base = `Daily · ${schedule.time_of_day || '00:00'}`;
      break;
    case 'weekly': {
      const days = (schedule.days_of_week || []).join(', ');
      const time = schedule.time_of_day || '00:00';
      if (schedule.repeat_every && schedule.repeat_every > 1) {
        base = `Every ${schedule.repeat_every} wks · ${days} ${time}`.trim();
      } else {
        base = `Weekly · ${days || '—'} ${time}`.trim();
      }
      break;
    }
    case 'monthly': {
      const mDays = (schedule.days_of_month || []).join(', ');
      base = `Monthly · Day ${mDays} ${schedule.time_of_day || '00:00'}`.trim();
      break;
    }
    case 'specific_dates': {
      const count = (schedule.specific_dates || []).length;
      base = `${count} date(s) · ${schedule.time_of_day || '00:00'}`;
      break;
    }
    case 'one_time':
      base = `Once · ${schedule.run_at || '?'}`;
      break;
    default:
      base = schedule.schedule_type;
      break;
  }
  
  if (schedule.end_condition === 'on_date') {
    base += ` (Ends ${schedule.end_date || '?'})`;
  } else if (schedule.end_condition === 'after_occurrences') {
    const total = schedule.max_occurrences || '?';
    base += ` (Ends after ${total} runs)`;
  }
  return base;
}

function getScheduleStatus(schedule: ScheduledRun, isRunning: boolean): 'Paused' | 'Due' | 'Live' | 'Running' | 'Failed' {
  if (isRunning) return 'Running';
  if (schedule.last_run_status === 'failed') return 'Failed';
  if (schedule.is_paused) return 'Paused';
  if (schedule.next_run_epoch_ms && schedule.next_run_epoch_ms <= Date.now()) return 'Due';
  return 'Live';
}

function summarizeScheduleTarget(
  schedule: ScheduledRun,
  workflow: WorkflowDefinition | undefined,
  agents: AgentConfig[]
): string {
  const idToName = new Map(agents.map(agent => [agent.session_id, agent.session_name || agent.session_id]));

  const mappedIds = Object.values(schedule.role_mappings || {}).filter(Boolean);
  if (mappedIds.length > 0) {
    const names = mappedIds.map(id => idToName.get(id) || id);
    if (names.length === 1) return `Agent · ${names[0]}`;
    return `Agents · ${names.slice(0, 2).join(', ')}${names.length > 2 ? ` +${names.length - 2}` : ''}`;
  }

  const agentNodes = workflow?.nodes.filter(node => node.type === 'agent') || [];
  const directAgentIds = agentNodes
    .map(node => node.config?.agent_id)
    .filter((id): id is string => typeof id === 'string' && id.trim().length > 0);
  if (directAgentIds.length > 0) {
    const names = directAgentIds.map(id => idToName.get(id) || id);
    if (names.length === 1) return `Agent · ${names[0]}`;
    return `Agents · ${names.slice(0, 2).join(', ')}${names.length > 2 ? ` +${names.length - 2}` : ''}`;
  }

  const roles = agentNodes
    .map(node => node.config?.role)
    .filter((role): role is string => typeof role === 'string' && role.trim().length > 0);
  if (roles.length > 0) {
    if (roles.length === 1) return `Role · ${roles[0]}`;
    return `Roles · ${roles.slice(0, 2).join(', ')}${roles.length > 2 ? ` +${roles.length - 2}` : ''}`;
  }

  return 'Target · Unassigned';
}

interface ActiveMonitoringProps {
  activeRuns: ActiveRunTracker[];
  schedules: ScheduledRun[];
  activeWorkflows: WorkflowDefinition[];
  availableWorkflows: WorkflowDefinition[];
  agents: AgentConfig[];
  onStopRun: (id: string) => void;
  onStopTrigger: (id: string) => void;
  onToggleSchedule: (id: string) => void;
  onDeleteSchedule?: (id: string) => void;
  onRunNow?: (scheduleId: string) => void;
  onOpenWorkflow?: (workflowId: string) => void;
}

export const ActiveMonitoring: React.FC<ActiveMonitoringProps> = ({
  activeRuns,
  schedules,
  activeWorkflows,
  availableWorkflows,
  agents,
  onStopRun,
  onStopTrigger,
  onToggleSchedule,
  onDeleteSchedule,
  onRunNow,
  onOpenWorkflow,
}) => {
  const [openSections, setOpenSections] = useState({
    runs: true,
    triggers: true,
    schedules: true,
  });
  const [selectedScheduleId, setSelectedScheduleId] = useState<string | null>(null);
  const [scheduleContextMenu, setScheduleContextMenu] = useState<{ schedule: ScheduledRun; x: number; y: number } | null>(null);

  const toggle = (section: keyof typeof openSections) =>
    setOpenSections((prev: typeof openSections) => ({ ...prev, [section]: !prev[section] }));

  const SectionHeader = ({ title, count, section }: { title: string; count: number; section: keyof typeof openSections }) => (
    <button
      onClick={() => toggle(section)}
      className="w-full flex items-center justify-between py-2 text-xs font-bold text-muted-neutral hover:text-primary transition-colors tracking-wide border-t border-wardian-border/10 first:border-t-0 mt-1"
    >
      <div className="flex items-center gap-2">
        <span>{title}</span>
        {count > 0 && <span className="text-[9px] bg-white/5 px-1.5 py-0.5 rounded-full font-mono">{count}</span>}
      </div>
      <span className={`transform transition-transform duration-200 opacity-30 ${openSections[section] ? 'rotate-0' : '-rotate-90'}`}>▼</span>
    </button>
  );

  return (
    <div className="flex flex-col gap-1">
      <SectionHeader title="Active Runs" count={activeRuns.length} section="runs" />
      {openSections.runs && (
        <div className="px-2 space-y-2 pb-3 pt-1">
          {activeRuns.length === 0 ? (
            <div className="text-[9px] text-muted-neutral italic py-4 text-center border border-dashed border-wardian-border/20 rounded-lg">No active runs</div>
          ) : (
            activeRuns.map(run => (
              <div key={run.run_instance_id} className="p-2.5 rounded-lg bg-[var(--color-wardian-card-bg)] border border-wardian-border/40 group hover:border-[var(--color-wardian-accent)]/30 transition-colors">
                <div className="flex justify-between items-start mb-2">
                  <span className="text-[11px] font-bold text-primary truncate leading-tight tracking-tight">{run.workflow_name}</span>
                  <button
                    onClick={() => onStopRun(run.run_instance_id)}
                    className="opacity-0 group-hover:opacity-100 p-1 text-muted hover:text-red-500 transition-all hover:scale-110"
                    title="Terminate Run"
                  >
                    <StopIcon />
                  </button>
                </div>
                <div className="flex items-center justify-between text-[9px] mb-1.5">
                  <span className="text-cyan-400 font-bold tracking-wide">Step {run.current_step}/{run.total_steps}</span>
                  <span className="text-muted-neutral truncate italic max-w-[60%]">{run.active_node_name}</span>
                </div>
                <div className="w-full bg-[var(--color-wardian-input-bg)] h-1.5 rounded-full overflow-hidden border border-white/5">
                  <div className="bg-cyan-500 h-full transition-all duration-700 cubic-bezier(0.4, 0, 0.2, 1)" style={{ width: `${(run.current_step / run.total_steps) * 100}%` }} />
                </div>
              </div>
            ))
          )}
        </div>
      )}

      <SectionHeader title="Live Listeners" count={activeWorkflows.length} section="triggers" />
      {openSections.triggers && (
        <div className="px-2 space-y-1 pb-3 pt-1">
          {activeWorkflows.length === 0 ? (
            <div className="text-[9px] text-muted-neutral italic py-4 text-center border border-dashed border-wardian-border/20 rounded-lg">No live listeners</div>
          ) : (
            activeWorkflows.map(wf => (
              <div key={wf.id} className="flex justify-between items-center px-3 py-2 rounded-lg bg-emerald-500/5 border border-emerald-500/10 group hover:bg-emerald-500/10 transition-colors">
                <div className="flex items-center gap-2.5 truncate">
                  <div className="relative"><div className="w-2 h-2 rounded-full bg-emerald-400" /></div>
                  <span className="text-[11px] font-semibold text-emerald-400 truncate tracking-tight">{wf.name}</span>
                </div>
                <button
                  onClick={() => onStopTrigger(wf.id)}
                  className="opacity-0 group-hover:opacity-100 p-1 text-emerald-600 hover:text-red-500 transition-all"
                  title="Pause Listener"
                >
                  <StopIcon />
                </button>
              </div>
            ))
          )}
        </div>
      )}

      <SectionHeader title="Scheduled Tasks" count={schedules.length} section="schedules" />
      {openSections.schedules && (
        <div className="px-2 space-y-1.5 pb-4 pt-1">
          {schedules.length === 0 ? (
            <div className="text-[9px] text-muted-neutral italic py-4 text-center border border-dashed border-wardian-border/20 rounded-lg">No scheduled tasks</div>
          ) : (
            schedules.map(schedule => {
              const workflow = availableWorkflows.find(item => item.id === schedule.workflow_id);
              const isExpanded = selectedScheduleId === schedule.id;
              const isRunning = activeRuns.some((run) => run.scheduled_run_id === schedule.id);
              const status = getScheduleStatus(schedule, isRunning);
              const targetSummary = summarizeScheduleTarget(schedule, workflow, agents);
              const roleMappings = Object.entries(schedule.role_mappings || {});
              const lastFailure = schedule.last_run_status === 'failed' ? schedule.last_run_error || 'Workflow failed' : null;

              return (
                <div
                  key={schedule.id}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setScheduleContextMenu({ schedule, x: event.clientX, y: event.clientY });
                  }}
                  className={`rounded-xl border transition-all ${isExpanded ? 'border-[var(--color-wardian-accent)]/35 bg-[color-mix(in_srgb,var(--color-wardian-card-bg),white_3%)]' : 'border-wardian-border/30 bg-[var(--color-wardian-card-bg)] hover:border-[var(--color-wardian-accent)]/30'}`}
                >
                  <div className="flex items-start gap-2 px-2.5 py-2">
                    <button
                      type="button"
                      onClick={() => setSelectedScheduleId(isExpanded ? null : schedule.id)}
                      aria-label={`${schedule.workflow_name} schedule details`}
                      className="min-w-0 flex-1 text-left cursor-pointer focus:outline-none"
                    >
                      <div className="flex items-center gap-1.5 min-w-0">
                        <div className="text-[var(--color-wardian-accent)] shrink-0"><ClockIcon /></div>
                        <span className="min-w-0 flex-1 truncate text-[11px] font-bold text-primary leading-tight tracking-tight">{schedule.workflow_name}</span>
                        <span className={`shrink-0 text-[8px] font-black px-1.5 py-0.5 rounded ${status === 'Paused' ? 'bg-amber-500/20 text-amber-500' : status === 'Failed' ? 'bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_80%)] text-[var(--color-wardian-error)]' : status === 'Due' ? 'bg-cyan-500/20 text-cyan-400' : status === 'Running' ? 'bg-cyan-500/20 text-cyan-400' : 'bg-emerald-500/20 text-emerald-400'}`}>
                          {status}
                        </span>
                      </div>
                      <div className="mt-0.5 flex items-center gap-1.5 text-[9px] text-muted-neutral font-medium min-w-0">
                        <span className="truncate">{formatScheduleSummary(schedule.schedule)}</span>
                        <span className="opacity-30">•</span>
                        <span className="truncate">{targetSummary}</span>
                      </div>
                      <div className="mt-1 flex items-center gap-1.5 text-[9px] font-bold uppercase tracking-[0.12em] text-[var(--color-wardian-accent)]/80">
                        <span>
                          {lastFailure
                            ? `Last failed: ${lastFailure}`
                            : status === 'Running'
                            ? 'Running now'
                            : (() => {
                                const label = formatNextRun(schedule.next_run_epoch_ms);
                                if (label === 'Due') {
                                  return 'Due';
                                }
                                if (label === '—') {
                                  return 'No next run';
                                }
                                return `Next in ${label}`;
                              })()}
                        </span>
                        <ChevronRightIcon expanded={isExpanded} />
                      </div>
                    </button>
                    <button
                      type="button"
                      onClick={() => onToggleSchedule(schedule.id)}
                      className="mt-0.5 shrink-0 p-1.5 rounded-lg border border-wardian-border/30 text-muted-neutral hover:text-primary hover:border-[var(--color-wardian-accent)]/40 transition-colors"
                      title={schedule.is_paused ? 'Resume schedule' : 'Pause schedule'}
                      aria-label={schedule.is_paused ? 'Resume schedule' : 'Pause schedule'}
                    >
                      {schedule.is_paused ? <PlayIcon /> : <PauseIcon />}
                    </button>
                  </div>

                  {isExpanded && (
                    <div className="border-t border-wardian-border/20 px-2.5 py-2.5">
                      <div className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1 text-[9px] text-muted-neutral">
                        <span className="font-bold uppercase tracking-[0.12em]">Schedule</span>
                        <span className="truncate">{formatScheduleSummary(schedule.schedule)}</span>
                        <span className="font-bold uppercase tracking-[0.12em]">Status</span>
                        <span className="truncate">{status}</span>
                        <span className="font-bold uppercase tracking-[0.12em]">Next</span>
                        <span className="truncate">{formatNextRun(schedule.next_run_epoch_ms)}</span>
                        <span className="font-bold uppercase tracking-[0.12em]">Target</span>
                        <span className="truncate">{targetSummary}</span>
                        {roleMappings.map(([role, agentId]) => {
                          const agentName = agents.find(agent => agent.session_id === agentId)?.session_name || agentId;
                          return (
                            <React.Fragment key={role}>
                              <span className="font-mono uppercase tracking-[0.08em]">{role}</span>
                              <span className="truncate">{agentName}</span>
                            </React.Fragment>
                          );
                        })}
                      </div>

                      <div className="mt-2 flex flex-wrap gap-1.5">
                        {onRunNow && (
                          <button type="button" onClick={() => onRunNow(schedule.id)} className="px-2 py-1 rounded-lg bg-cyan-500/10 border border-cyan-500/20 text-cyan-400 text-[9px] font-bold tracking-wide hover:bg-cyan-500/15 transition-colors">Run Now</button>
                        )}
                        {onOpenWorkflow && (
                          <button type="button" onClick={() => onOpenWorkflow(schedule.workflow_id)} className="inline-flex items-center gap-1 px-2 py-1 rounded-lg bg-[var(--color-wardian-accent)]/12 border border-[var(--color-wardian-accent)]/35 text-primary text-[9px] font-bold tracking-wide hover:bg-[var(--color-wardian-accent)]/18 hover:border-[var(--color-wardian-accent)]/55 transition-colors"><svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><path d="M11 4H6a2 2 0 00-2 2v12a2 2 0 002 2h12a2 2 0 002-2v-5"/><path d="M18.5 2.5a2.121 2.121 0 113 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>Edit Workflow</button>
                        )}
                        {onDeleteSchedule && (
                          <button type="button" onClick={() => { onDeleteSchedule(schedule.id); setSelectedScheduleId(null); }} className="px-2 py-1 rounded-lg bg-red-500/10 border border-red-500/20 text-red-400 text-[9px] font-bold tracking-wide hover:bg-red-500/15 transition-colors">Delete Schedule</button>
                        )}
                      </div>
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
      )}

      {scheduleContextMenu && (
        <ContextMenu
          x={scheduleContextMenu.x}
          y={scheduleContextMenu.y}
          onClose={() => setScheduleContextMenu(null)}
          items={[
            {
              label: scheduleContextMenu.schedule.is_paused ? 'Resume Schedule' : 'Pause Schedule',
              icon: scheduleContextMenu.schedule.is_paused ? <PlayIcon /> : <PauseIcon />,
              onClick: () => onToggleSchedule(scheduleContextMenu.schedule.id),
            },
            ...(onRunNow
              ? [{
                  label: 'Run Now',
                  icon: <RunNowIcon />,
                  onClick: () => onRunNow(scheduleContextMenu.schedule.id),
                } satisfies ContextMenuItem]
              : []),
            ...(onOpenWorkflow
              ? [{
                  label: 'Edit Workflow',
                  icon: <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><path d="M11 4H6a2 2 0 00-2 2v12a2 2 0 002 2h12a2 2 0 002-2v-5"/><path d="M18.5 2.5a2.121 2.121 0 113 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>,
                  onClick: () => onOpenWorkflow(scheduleContextMenu.schedule.workflow_id),
                } satisfies ContextMenuItem]
              : []),
            ...(onDeleteSchedule
              ? [
                  { divider: true } satisfies ContextMenuItem,
                  {
                    label: 'Delete Schedule',
                    icon: <TrashIcon />,
                    danger: true,
                    onClick: () => {
                      onDeleteSchedule(scheduleContextMenu.schedule.id);
                      setSelectedScheduleId(null);
                    },
                  } satisfies ContextMenuItem,
                ]
              : []),
          ]}
        />
      )}
    </div>
  );
};
