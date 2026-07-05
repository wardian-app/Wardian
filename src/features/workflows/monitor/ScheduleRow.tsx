import type { WorkflowAssignments, WorkflowSchedule } from '../../../types/workflow';
import { useState } from 'react';
import { MoreHorizontal, Pause, Pencil, Play, RotateCcw, Trash2 } from 'lucide-react';
import { cadenceLabel, nextRunLabel, scheduleStatusColor, scheduleStatusLabel } from './scheduleStatus';

interface ScheduleRowProps {
  schedule: WorkflowSchedule;
  agentLabels?: Record<string, string>;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onRemove: (id: string) => void;
  onEdit: (schedule: WorkflowSchedule) => void;
}

const actionClass =
  'inline-flex h-7 w-7 cursor-pointer select-none items-center justify-center rounded border border-wardian-border text-muted hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]';

export function ScheduleRow({ schedule, agentLabels = {}, onPause, onResume, onRunNow, onRemove, onEdit }: ScheduleRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);

  return (
    <tr
      data-testid={`schedule-row-${schedule.id}`}
      className="select-text border-b border-wardian-border/70 bg-[var(--color-wardian-bg)] align-middle last:border-b-0 hover:bg-[color-mix(in_srgb,var(--color-wardian-card),transparent_45%)]"
    >
      <td className="w-[92px] px-3 py-2">
        <div className="flex items-center gap-2 text-[10px] font-bold text-muted">
          <span
            className="h-2 w-2 shrink-0 rounded-full"
            style={{ backgroundColor: scheduleStatusColor(schedule) }}
            aria-hidden
          />
          <span>{scheduleStatusLabel(schedule)}</span>
        </div>
      </td>
      <td className="min-w-0 px-3 py-2">
        <div className="min-w-0">
          <div className="truncate text-xs font-bold text-[var(--color-wardian-text)]" title={schedule.name}>{schedule.name}</div>
          <div className="mt-0.5 truncate text-[10px] text-muted" title={schedule.blueprint_id}>{schedule.blueprint_id}</div>
        </div>
      </td>
      <td className="min-w-0 px-3 py-2">
        <div className="truncate text-[10px] text-muted" title={cadenceLabel(schedule.schedule)}>{cadenceLabel(schedule.schedule)}</div>
        <div className="mt-0.5 truncate text-[10px] text-muted" title={nextRunLabel(schedule)}>next {nextRunLabel(schedule)}</div>
      </td>
      <td className="min-w-0 px-3 py-2">
        <AssignmentSummary
          assignments={schedule.assignments}
          bindings={schedule.bindings}
          provider={schedule.provider}
          agentLabels={agentLabels}
        />
        {schedule.last_run_error ? (
          <div className="mt-0.5 truncate text-[10px] text-[var(--color-wardian-error)]">{schedule.last_run_error}</div>
        ) : null}
      </td>
      <td className="relative w-[150px] px-3 py-2 text-right">
        <div className="inline-flex shrink-0 items-center gap-1">
        {schedule.is_paused ? (
          <button type="button" className={actionClass} onClick={() => onResume(schedule.id)} aria-label={`Resume ${schedule.name}`} title="Resume">
            <RotateCcw className="h-3.5 w-3.5" aria-hidden />
          </button>
        ) : (
          <button type="button" className={actionClass} onClick={() => onPause(schedule.id)} aria-label={`Pause ${schedule.name}`} title="Pause">
            <Pause className="h-3.5 w-3.5" aria-hidden />
          </button>
        )}
        <button type="button" className={actionClass} onClick={() => onRunNow(schedule.id)} aria-label={`Run ${schedule.name} now`} title="Run now">
          <Play className="h-3.5 w-3.5" aria-hidden />
        </button>
        <button type="button" className={actionClass} onClick={() => onEdit(schedule)} aria-label={`Edit ${schedule.name}`} title="Edit">
          <Pencil className="h-3.5 w-3.5" aria-hidden />
        </button>
        <button
          type="button"
          className={actionClass}
          onClick={() => setMenuOpen((open) => !open)}
          aria-label={`More actions for ${schedule.name}`}
          title="More actions"
          aria-expanded={menuOpen}
        >
          <MoreHorizontal className="h-3.5 w-3.5" aria-hidden />
        </button>
        </div>
        {menuOpen ? (
          <div className="absolute right-3 top-10 z-20 w-32 rounded border border-wardian-border bg-[var(--color-wardian-card)] p-1 text-left shadow-lg">
            <button
              type="button"
              className="flex w-full cursor-pointer select-none items-center gap-2 rounded px-2 py-1.5 text-[11px] text-muted hover:bg-[var(--color-wardian-bg)] hover:text-[var(--color-wardian-error)]"
              onClick={() => {
                setMenuOpen(false);
                onRemove(schedule.id);
              }}
            >
              <Trash2 className="h-3.5 w-3.5" aria-hidden />
              Remove
            </button>
          </div>
        ) : null}
      </td>
    </tr>
  );
}

function AssignmentSummary({
  assignments,
  bindings,
  provider,
  agentLabels,
}: {
  assignments?: WorkflowAssignments;
  bindings?: Record<string, string>;
  provider?: string | null;
  agentLabels: Record<string, string>;
}) {
  const labels = assignmentLabels(assignments, bindings, provider, agentLabels);
  if (labels.length === 0) return null;

  return (
    <div className="flex max-w-[360px] flex-wrap gap-1">
      {labels.slice(0, 3).map((label) => (
        <span
          key={label}
          className="max-w-[220px] truncate rounded border border-wardian-border bg-[var(--color-wardian-card)] px-1.5 py-0.5 text-[10px] text-muted"
          title={label}
        >
          {label}
        </span>
      ))}
      {labels.length > 3 ? (
        <span className="rounded border border-wardian-border bg-[var(--color-wardian-card)] px-1.5 py-0.5 text-[10px] text-muted">
          +{labels.length - 3} roles
        </span>
      ) : null}
    </div>
  );
}

function assignmentLabels(
  assignments?: WorkflowAssignments,
  bindings?: Record<string, string>,
  provider?: string | null,
  agentLabels: Record<string, string> = {},
) {
  if (assignments && Object.keys(assignments).length > 0) {
    return Object.entries(assignments).sort(compareRoleEntries).map(([role, assignment]) => {
      if (assignment.target_type === 'agent') {
        return `${role}: ${agentLabels[assignment.agent_id] ?? assignment.agent_id}`;
      }
      return `${role}: temp ${assignment.provider}`;
    });
  }
  const bindingLabels = Object.entries(bindings ?? {})
    .sort(compareRoleEntries)
    .map(([role, target]) => `${role}: ${agentLabels[target] ?? target}`);
  if (bindingLabels.length > 0) return bindingLabels;
  return provider ? [`temp ${provider}`] : [];
}

function compareRoleEntries(left: [string, unknown], right: [string, unknown]) {
  return left[0].localeCompare(right[0]);
}
