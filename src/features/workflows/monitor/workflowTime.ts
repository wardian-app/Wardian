import type { RunSummary } from '../run/runTypes';

export interface WorkflowTimeLabel {
  primary: string;
  exact: string | null;
  valid: boolean;
}

export interface WorkflowTimeOptions {
  now?: Date;
  locale?: string;
  emptyLabel?: string;
}

type WorkflowTimeValue = string | number | Date | null | undefined;

function localDaySerial(value: Date): number {
  return Date.UTC(value.getFullYear(), value.getMonth(), value.getDate()) / 86_400_000;
}

function originalLabel(value: Exclude<WorkflowTimeValue, null | undefined>): string {
  return value instanceof Date ? value.toString() : String(value);
}

export function formatWorkflowTime(
  value: WorkflowTimeValue,
  { now = new Date(), locale, emptyLabel = 'Unknown' }: WorkflowTimeOptions = {},
): WorkflowTimeLabel {
  if (value === null || value === undefined || value === '') {
    return { primary: emptyLabel, exact: null, valid: false };
  }

  const date = value instanceof Date ? new Date(value.getTime()) : new Date(value);
  if (!Number.isFinite(date.getTime())) {
    return { primary: originalLabel(value), exact: null, valid: false };
  }

  const time = new Intl.DateTimeFormat(locale, {
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
  const dayDelta = localDaySerial(date) - localDaySerial(now);

  let primary: string;
  if (dayDelta === 0) {
    primary = `Today, ${time}`;
  } else if (dayDelta === 1) {
    primary = `Tomorrow, ${time}`;
  } else if (Math.abs(dayDelta) <= 6) {
    const nearbyDate = new Intl.DateTimeFormat(locale, {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    }).format(date);
    primary = `${nearbyDate} · ${time}`;
  } else {
    const dated = new Intl.DateTimeFormat(locale, {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
    }).format(date);
    primary = `${dated} · ${time}`;
  }

  return {
    primary,
    exact: new Intl.DateTimeFormat(locale, {
      dateStyle: 'full',
      timeStyle: 'long',
    }).format(date),
    valid: true,
  };
}

export function runTimestampValue(run: RunSummary): string | null {
  return run.updated_at ?? run.completed_at ?? run.started_at ?? null;
}

export function formatRunDuration(run: RunSummary): string | null {
  if (!run.started_at) return null;

  const end = run.completed_at ?? run.updated_at;
  if (!end) return null;

  const durationMs = Date.parse(end) - Date.parse(run.started_at);
  if (!Number.isFinite(durationMs) || durationMs < 0) return null;

  const seconds = Math.round(durationMs / 1_000);
  if (seconds < 60) return `${seconds}s`;

  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return remainder === 0 ? `${minutes}m` : `${minutes}m ${remainder}s`;
}
