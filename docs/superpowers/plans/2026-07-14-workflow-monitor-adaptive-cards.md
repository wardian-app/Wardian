# Workflow Monitor Adaptive Cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Workflows Monitor's universal metadata-heavy rows and the sidebar's unreadable schedule line with adaptive cards that prioritize assigned agents and the time information relevant to each operational mode.

**Architecture:** Extract pure workflow activity, assignment, and time presentation helpers from the current 960-line Monitor module. Render those models through a full-size adaptive card in Monitor and a separate compact card in the sidebar; keep stores, polling, filters, schedule actions, history paging, and bounded virtualization in their existing containers.

**Tech Stack:** React 19, TypeScript, Zustand-backed workflow stores, Tailwind utility classes with Wardian semantic theme variables, Vitest/React Testing Library, and Playwright browser E2E.

## Global Constraints

- GitHub issue: [#668](https://github.com/wardian-app/Wardian/issues/668).
- Do not change Rust, Tauri commands, workflow DTOs, schedule execution semantics, polling intervals, history retention, or Observe mode.
- Preserve the two pre-existing uncommitted Rust modifications in `src-tauri/src/commands/agent.rs` and `src-tauri/src/manager/telemetry.rs`; never stage them with this feature.
- The branch is based on the `origin/main` tip that existed when it was created and is currently one commit behind. Do not rebase or stash the unrelated Rust work merely to implement frontend files.
- Use only semantic Wardian theme variables/classes for color; do not add hardcoded Tailwind palette colors.
- Keep TypeScript strict and do not introduce `any`.
- Collapsed assignment summaries show two role-aware agents and an accessible `+N agents` control.
- Calendar labels use `Today`, `Tomorrow`, weekday plus date for nearby dates, and a full dated timestamp for farther dates; exact local time and timezone remain available to assistive technology or as a title.
- Scheduled cards emphasize next run and cadence; History emphasizes run time and outcome; Running and Needs attention emphasize live state and ownership.
- Retain existing action labels and behavior for open run, pause, resume, run now, and edit.
- Keep History DOM rendering bounded to at most 32 cards while supporting expanded assignment detail.
- User-facing documentation and commands remain cross-platform. Show POSIX shell syntax first and label PowerShell alternatives when syntax differs.

---

### Task 1: Calendar-aware workflow time presentation

**Files:**
- Create: `src/features/workflows/monitor/workflowTime.ts`
- Create: `src/features/workflows/monitor/workflowTime.test.ts`

**Interfaces:**
- Consumes: `RunSummary` from `src/features/workflows/run/runTypes.ts`.
- Produces: `WorkflowTimeLabel`, `formatWorkflowTime(value, options)`, `runTimestampValue(run)`, and `formatRunDuration(run)`.

- [ ] **Step 1: Write failing tests for calendar labels, fallbacks, exact timestamps, and duration**

```ts
import { describe, expect, it } from 'vitest';
import { formatRunDuration, formatWorkflowTime, runTimestampValue } from './workflowTime';

const now = new Date(2026, 6, 14, 12, 0, 0);
const options = { now, locale: 'en-US' };

describe('formatWorkflowTime', () => {
  it.each([
    [new Date(2026, 6, 14, 15, 20), 'Today, 3:20 PM'],
    [new Date(2026, 6, 15, 9, 45), 'Tomorrow, 9:45 AM'],
    [new Date(2026, 6, 16, 9, 35), 'Thu, Jul 16 · 9:35 AM'],
    [new Date(2026, 9, 1, 8, 0), 'Oct 1, 2026 · 8:00 AM'],
  ])('formats %s as %s', (value, expected) => {
    const label = formatWorkflowTime(value, options);
    expect(label.primary).toBe(expected);
    expect(label.exact).toContain('2026');
    expect(label.valid).toBe(true);
  });

  it('uses the requested empty label when no timestamp exists', () => {
    expect(formatWorkflowTime(null, { ...options, emptyLabel: 'Never run' })).toEqual({
      primary: 'Never run',
      exact: null,
      valid: false,
    });
  });

  it('returns an invalid string unchanged', () => {
    expect(formatWorkflowTime('not-a-date', options)).toEqual({
      primary: 'not-a-date',
      exact: null,
      valid: false,
    });
  });
});

describe('run time helpers', () => {
  it('prefers updated, completed, then started timestamps', () => {
    expect(runTimestampValue({
      run_id: 'run-1', blueprint_id: 'wf', status: 'completed', node_count: 1,
      path: '/run', started_at: 'start', completed_at: 'complete', updated_at: 'update',
    })).toBe('update');
  });

  it('calculates duration when both endpoints are valid', () => {
    expect(formatRunDuration({
      run_id: 'run-1', blueprint_id: 'wf', status: 'completed', node_count: 1,
      path: '/run', started_at: '2026-07-14T12:00:00Z', completed_at: '2026-07-14T12:00:12Z',
    })).toBe('12s');
  });
});
```

- [ ] **Step 2: Run the focused test and verify the red state**

Run:

```bash
npm run test -- src/features/workflows/monitor/workflowTime.test.ts
```

Expected: FAIL because `./workflowTime` does not exist.

- [ ] **Step 3: Implement the pure formatter and run helpers**

```ts
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

  const time = new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit' }).format(date);
  const dayDelta = localDaySerial(date) - localDaySerial(now);
  let primary: string;
  if (dayDelta === 0) {
    primary = `Today, ${time}`;
  } else if (dayDelta === 1) {
    primary = `Tomorrow, ${time}`;
  } else if (Math.abs(dayDelta) <= 6) {
    const nearbyDate = new Intl.DateTimeFormat(locale, {
      weekday: 'short', month: 'short', day: 'numeric',
    }).format(date);
    primary = `${nearbyDate} · ${time}`;
  } else {
    const dated = new Intl.DateTimeFormat(locale, {
      month: 'short', day: 'numeric', year: 'numeric',
    }).format(date);
    primary = `${dated} · ${time}`;
  }

  return {
    primary,
    exact: new Intl.DateTimeFormat(locale, { dateStyle: 'full', timeStyle: 'long' }).format(date),
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
  const seconds = Math.round(durationMs / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return remainder === 0 ? `${minutes}m` : `${minutes}m ${remainder}s`;
}
```

- [ ] **Step 4: Run the focused test and verify green**

Run:

```bash
npm run test -- src/features/workflows/monitor/workflowTime.test.ts
```

Expected: PASS with all calendar, invalid, fallback, and duration cases green.

- [ ] **Step 5: Commit the time presentation unit**

```bash
git add src/features/workflows/monitor/workflowTime.ts src/features/workflows/monitor/workflowTime.test.ts
git commit -m "feat(workflows): add calendar-aware monitor times"
```

---

### Task 2: Shared role-aware assignment presentation

**Files:**
- Create: `src/features/workflows/monitor/assignmentPresentation.ts`
- Create: `src/features/workflows/monitor/assignmentPresentation.test.ts`
- Create: `src/features/workflows/monitor/WorkflowAssignmentSummary.tsx`
- Create: `src/features/workflows/monitor/WorkflowAssignmentSummary.test.tsx`

**Interfaces:**
- Consumes: `AgentConfig`, `WorkflowAssignments`, schedule bindings, and optional provider.
- Produces: `WorkflowAssignmentItem`, `buildAgentLabelMap(agents)`, `workflowAssignmentItems(...)`, and `<WorkflowAssignmentSummary workflowName items maxVisible expanded onExpandedChange />`.

- [ ] **Step 1: Write failing pure-model tests for stable roles and fallback targets**

```ts
import { describe, expect, it } from 'vitest';
import { buildAgentLabelMap, workflowAssignmentItems } from './assignmentPresentation';

describe('workflowAssignmentItems', () => {
  const labels = buildAgentLabelMap([{
    session_id: 'agent-1', session_name: 'Librarian', agent_class: 'Researcher',
    folder: '/workspace', is_off: false, provider: 'codex',
  }]);

  it('sorts roles and resolves named, missing, and temporary targets', () => {
    const items = workflowAssignmentItems({
      reviewer: { target_type: 'temporary_provider', provider: 'claude' },
      analyst: { target_type: 'agent', agent_id: 'agent-1', conversation: 'current' },
      writer: { target_type: 'agent', agent_id: 'missing-agent', conversation: 'fresh_background' },
    }, {}, null, labels);

    expect(items.map((item) => item.role)).toEqual(['analyst', 'reviewer', 'writer']);
    expect(items[0]).toMatchObject({ targetLabel: 'Librarian · Codex', detailLabel: 'Current session' });
    expect(items[1]).toMatchObject({ targetLabel: 'Temporary Claude', detailLabel: 'Ephemeral' });
    expect(items[2]).toMatchObject({ targetLabel: 'missing-agent', detailLabel: 'Fresh background' });
  });

  it('resolves legacy bindings and default provider assignments', () => {
    expect(workflowAssignmentItems(undefined, { planner: 'agent-1' }, null, labels)[0])
      .toMatchObject({ role: 'planner', targetLabel: 'Librarian · Codex' });
    expect(workflowAssignmentItems(undefined, {}, 'codex', labels)[0])
      .toMatchObject({ role: 'default', targetLabel: 'Temporary Codex' });
  });
});
```

- [ ] **Step 2: Run the pure-model test and verify the red state**

Run:

```bash
npm run test -- src/features/workflows/monitor/assignmentPresentation.test.ts
```

Expected: FAIL because `assignmentPresentation.ts` does not exist.

- [ ] **Step 3: Implement the assignment model with exact exported shapes**

```ts
import type { AgentConfig } from '../../../types';
import type { WorkflowAssignments } from '../../../types/workflow';

export interface WorkflowAssignmentItem {
  key: string;
  role: string;
  targetLabel: string;
  detailLabel: string;
  fullLabel: string;
}

function humanize(value: string): string {
  if (value.toLowerCase() === 'opencode') return 'OpenCode';
  return value.charAt(0).toUpperCase() + value.slice(1);
}

export function buildAgentLabelMap(agents: AgentConfig[]): Record<string, string> {
  return Object.fromEntries(agents.map((agent) => {
    const provider = agent.provider?.trim();
    return [agent.session_id, provider
      ? `${agent.session_name} · ${humanize(provider)}`
      : agent.session_name];
  }));
}

export function workflowAssignmentItems(
  assignments: WorkflowAssignments | undefined,
  bindings: Record<string, string> | undefined,
  provider: string | null | undefined,
  agentLabels: Record<string, string>,
): WorkflowAssignmentItem[] {
  if (assignments && Object.keys(assignments).length > 0) {
    return Object.entries(assignments).sort(([left], [right]) => left.localeCompare(right)).map(([role, assignment]) => {
      const targetLabel = assignment.target_type === 'agent'
        ? agentLabels[assignment.agent_id] ?? assignment.agent_id
        : `Temporary ${humanize(assignment.provider)}`;
      const detailLabel = assignment.target_type === 'temporary_provider'
        ? 'Ephemeral'
        : assignment.conversation === 'fresh_background' ? 'Fresh background' : 'Current session';
      return { key: role, role, targetLabel, detailLabel, fullLabel: `${role} · ${targetLabel}` };
    });
  }

  const bindingItems = Object.entries(bindings ?? {}).sort(([left], [right]) => left.localeCompare(right)).map(([role, target]) => ({
    key: role,
    role,
    targetLabel: agentLabels[target] ?? target,
    detailLabel: 'Legacy binding',
    fullLabel: `${role} · ${agentLabels[target] ?? target}`,
  }));
  if (bindingItems.length > 0) return bindingItems;
  if (!provider) return [];
  const targetLabel = `Temporary ${humanize(provider)}`;
  return [{ key: 'default', role: 'default', targetLabel, detailLabel: 'Ephemeral', fullLabel: targetLabel }];
}
```

- [ ] **Step 4: Write the failing component test for two chips plus accessible expansion**

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WorkflowAssignmentSummary } from './WorkflowAssignmentSummary';
import type { WorkflowAssignmentItem } from './assignmentPresentation';

const items: WorkflowAssignmentItem[] = ['analyst', 'reviewer', 'writer', 'publisher'].map((role) => ({
  key: role,
  role,
  targetLabel: `${role} agent`,
  detailLabel: 'Current session',
  fullLabel: `${role} · ${role} agent`,
}));

describe('WorkflowAssignmentSummary', () => {
  it('shows two roles and expands every assignment from the count button', () => {
    const onExpandedChange = vi.fn();
    const { rerender } = render(
      <WorkflowAssignmentSummary workflowName="Strategy" items={items} expanded={false} onExpandedChange={onExpandedChange} />,
    );
    expect(screen.getByText('analyst · analyst agent')).toBeVisible();
    expect(screen.getByText('reviewer · reviewer agent')).toBeVisible();
    expect(screen.queryByText('publisher · publisher agent')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show 2 more agents for Strategy' }));
    expect(onExpandedChange).toHaveBeenCalledWith(true);

    rerender(
      <WorkflowAssignmentSummary workflowName="Strategy" items={items} expanded onExpandedChange={onExpandedChange} />,
    );
    expect(screen.getByText('publisher · publisher agent')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Hide additional agents for Strategy' })).toHaveAttribute('aria-expanded', 'true');
  });
});
```

- [ ] **Step 5: Implement the controlled/uncontrolled summary component**

Implement `WorkflowAssignmentSummary.tsx` with this public signature and behavior:

```tsx
import { useState } from 'react';
import type { WorkflowAssignmentItem } from './assignmentPresentation';

interface WorkflowAssignmentSummaryProps {
  workflowName: string;
  items: WorkflowAssignmentItem[];
  maxVisible?: number;
  compact?: boolean;
  expanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
}

export function WorkflowAssignmentSummary({
  workflowName,
  items,
  maxVisible = 2,
  compact = false,
  expanded: controlledExpanded,
  onExpandedChange,
}: WorkflowAssignmentSummaryProps) {
  const [localExpanded, setLocalExpanded] = useState(false);
  const expanded = controlledExpanded ?? localExpanded;
  const setExpanded = (next: boolean) => {
    if (controlledExpanded === undefined) setLocalExpanded(next);
    onExpandedChange?.(next);
  };
  if (items.length === 0) return <span className="text-[10px] text-muted">Default assignment</span>;

  const collapsedItems = items.slice(0, maxVisible);
  const hiddenCount = Math.max(0, items.length - collapsedItems.length);
  return (
    <div className="min-w-0">
      <div className="flex min-w-0 flex-wrap gap-1">
        {collapsedItems.map((item) => (
          <span key={item.key} title={item.fullLabel} className="max-w-full truncate rounded-full border border-[color-mix(in_srgb,var(--color-wardian-accent),transparent_60%)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_90%)] px-2 py-0.5 text-[10px] font-medium text-[var(--color-wardian-text)]">
            {item.fullLabel}
          </span>
        ))}
        {hiddenCount > 0 ? (
          <button type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)} className="rounded-full border border-wardian-border px-2 py-0.5 text-[10px] text-muted hover:text-[var(--color-wardian-text)]" aria-label={expanded ? `Hide additional agents for ${workflowName}` : `Show ${hiddenCount} more agents for ${workflowName}`}>
            {expanded ? 'Show less' : `+${hiddenCount} agents`}
          </button>
        ) : null}
      </div>
      {expanded && hiddenCount > 0 ? (
        <div data-testid="expanded-workflow-assignments" className={`${compact ? 'max-h-24' : 'max-h-28'} mt-2 overflow-y-auto rounded border border-wardian-border bg-[var(--color-wardian-card)] p-2`}>
          {items.map((item) => (
            <div key={item.key} className="grid grid-cols-[minmax(80px,auto)_minmax(0,1fr)] gap-2 py-1 text-[10px]">
              <span className="font-bold text-[var(--color-wardian-text)]">{item.role}</span>
              <span className="min-w-0 break-words text-muted">{item.targetLabel} · {item.detailLabel}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
```

- [ ] **Step 6: Run both assignment test files**

Run:

```bash
npm run test -- src/features/workflows/monitor/assignmentPresentation.test.ts src/features/workflows/monitor/WorkflowAssignmentSummary.test.tsx
```

Expected: PASS; the expanded map exposes all roles and the missing agent id remains readable.

- [ ] **Step 7: Commit the shared assignment unit**

```bash
git add src/features/workflows/monitor/assignmentPresentation.ts src/features/workflows/monitor/assignmentPresentation.test.ts src/features/workflows/monitor/WorkflowAssignmentSummary.tsx src/features/workflows/monitor/WorkflowAssignmentSummary.test.tsx
git commit -m "feat(workflows): add role-aware assignment summaries"
```

---

### Task 3: Extract the workflow activity model from the Monitor container

**Files:**
- Create: `src/features/workflows/monitor/monitorModel.ts`
- Create: `src/features/workflows/monitor/monitorModel.test.ts`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.tsx:18-46,587-912`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.test.tsx:1-3,600-960`

**Interfaces:**
- Consumes: `RunSummary[]` and `WorkflowSchedule[]`.
- Produces: exported `ActivityFilter`, `ActivitySection`, `ActivityTone`, `WorkflowActivity`, `WorkflowMonitorModel`, `buildActivities`, `buildMonitorModel`, `historyActivityFromRun`, `groupActivities`, `sectionsForFilter`, and `scheduleLookupById`.

- [ ] **Step 1: Write a failing contract test against the new module**

```ts
import { describe, expect, it } from 'vitest';
import { buildMonitorModel, historyActivityFromRun } from './monitorModel';

describe('monitorModel', () => {
  it('keeps scheduled, active, attention, and history modes distinct', () => {
    const model = buildMonitorModel([
      { run_id: 'running', blueprint_id: 'live', status: 'running', node_count: 1, path: '/running' },
      { run_id: 'approval', blueprint_id: 'gate', status: 'awaiting_approval', node_count: 1, path: '/approval' },
      { run_id: 'done', blueprint_id: 'done', status: 'completed', node_count: 1, path: '/done' },
    ], [{
      id: 'schedule-1', blueprint_id: 'scheduled', name: 'Scheduled', input: {}, bindings: {},
      schedule: { schedule_type: 'daily', time_of_day: '09:00', active: true },
      next_run_epoch_ms: Date.UTC(2026, 6, 15, 13), is_paused: false,
    }]);

    expect(model.activities.map((activity) => activity.section)).toEqual(expect.arrayContaining(['running', 'attention', 'scheduled', 'history']));
    expect(historyActivityFromRun({
      run_id: 'failed', blueprint_id: 'audit', status: 'failed', node_count: 1,
      path: '/failed', failure: 'Provider crashed',
    }).issue).toBe('Provider crashed');
  });
});
```

- [ ] **Step 2: Run the model test and verify the red state**

Run:

```bash
npm run test -- src/features/workflows/monitor/monitorModel.test.ts
```

Expected: FAIL because `monitorModel.ts` does not exist.

- [ ] **Step 3: Move model types and pure functions without changing behavior**

Move the current activity types and functions from `WorkflowMonitor.tsx` into `monitorModel.ts`. Export the exact public surface below; keep the existing function bodies and ordering logic intact:

```ts
export type ActivityFilter = 'all' | 'attention' | 'running' | 'scheduled' | 'history';
export type ActivitySection = Exclude<ActivityFilter, 'all'>;
export type ActivityTone = 'error' | 'active' | 'warning' | 'accent' | 'success' | 'muted';

export interface WorkflowActivity {
  activityId: string;
  blueprintId: string;
  name: string;
  schedules: WorkflowSchedule[];
  latestRun: RunSummary | null;
  activeRun: RunSummary | null;
  nextSchedule: WorkflowSchedule | null;
  statusLabel: string;
  tone: ActivityTone;
  section: ActivitySection;
  issue: string | null;
}

export interface WorkflowMonitorModel {
  activities: WorkflowActivity[];
  historyRuns: RunSummary[];
  upcomingSchedules: WorkflowSchedule[];
  stats: { failedCount: number; runningCount: number; awaitingCount: number; pausedCount: number };
}
```

Move `buildActivities`, `buildMonitorModel`, `latestRunForSchedule`, `setLatestRun`, `mostRecentRun`, `activityFromParts`, `historyActivityFromRun`, `activityState`, `groupActivities`, `sectionsForFilter`, `compareActivities`, `activitySortTime`, `compareScheduleRecency`, `scheduleLookupById`, `compareRunRecency`, `runSortTime`, and `historyTone`. Import `runTimestampValue` from `workflowTime.ts` instead of retaining the local duplicate.

- [ ] **Step 4: Point the container and tests at the extracted module**

In `WorkflowMonitor.tsx`, import the model surface:

```ts
import {
  buildMonitorModel,
  groupActivities,
  historyActivityFromRun,
  scheduleLookupById,
  sectionsForFilter,
} from './monitorModel';
import type { ActivityFilter, ActivitySection, ActivityTone, WorkflowActivity } from './monitorModel';
```

In `WorkflowMonitor.test.tsx`, import `buildActivities` and `buildMonitorModel` from `./monitorModel`. Move the model-only test cases into `monitorModel.test.ts`; leave rendering, polling, filters, actions, and virtualization tests in `WorkflowMonitor.test.tsx`.

- [ ] **Step 5: Run the model and Monitor suites**

Run:

```bash
npm run test -- src/features/workflows/monitor/monitorModel.test.ts src/features/workflows/monitor/WorkflowMonitor.test.tsx
```

Expected: PASS with existing stats, grouping, schedule association, and history ordering unchanged.

- [ ] **Step 6: Commit the behavior-preserving extraction**

```bash
git add src/features/workflows/monitor/monitorModel.ts src/features/workflows/monitor/monitorModel.test.ts src/features/workflows/monitor/WorkflowMonitor.tsx src/features/workflows/monitor/WorkflowMonitor.test.tsx
git commit -m "refactor(workflows): isolate monitor presentation model"
```

---

### Task 4: Full-size adaptive Monitor cards

**Files:**
- Create: `src/features/workflows/monitor/WorkflowActivityCard.tsx`
- Create: `src/features/workflows/monitor/WorkflowActivityCard.test.tsx`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.tsx:63-584`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.test.tsx`

**Interfaces:**
- Consumes: `WorkflowActivity`, resolved `agentLabels`, time helpers, assignment helpers, and existing action callbacks.
- Produces: `<WorkflowActivityCard activity agentLabels now virtualized expandedAssignments onExpandedAssignmentsChange ...actions />` and section-specific `data-mode` values.

- [ ] **Step 1: Write failing card tests for Scheduled and History priorities**

Build typed `WorkflowActivity` fixtures and assert the public card contract:

```tsx
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WorkflowActivityCard } from './WorkflowActivityCard';
import type { WorkflowActivity } from './monitorModel';

const schedule = {
  id: 'schedule-1', blueprint_id: 'strategy', name: 'Strategy Report', input: {}, bindings: {},
  assignments: {
    analyst: { target_type: 'agent' as const, agent_id: 'agent-1', conversation: 'current' as const },
  },
  schedule: { schedule_type: 'daily' as const, time_of_day: '09:45', active: true },
  next_run_epoch_ms: new Date(2026, 6, 15, 9, 45).getTime(),
  last_run_epoch_ms: new Date(2026, 6, 14, 9, 45).getTime(),
  is_paused: false,
};

const actions = {
  onOpenRun: vi.fn(), onPause: vi.fn(), onResume: vi.fn(), onRunNow: vi.fn(), onEditSchedule: vi.fn(),
};

describe('WorkflowActivityCard', () => {
  it('emphasizes next run and cadence for Scheduled', () => {
    const activity: WorkflowActivity = {
      activityId: 'schedule:schedule-1', blueprintId: 'strategy', name: 'Strategy Report',
      schedules: [schedule], latestRun: null, activeRun: null, nextSchedule: schedule,
      statusLabel: 'Scheduled', tone: 'accent', section: 'scheduled', issue: null,
    };
    render(<WorkflowActivityCard activity={activity} agentLabels={{ 'agent-1': 'Researcher · Codex' }} now={new Date(2026, 6, 14, 12)} {...actions} />);
    const card = screen.getByTestId('workflow-activity-card-schedule:schedule-1');
    expect(card).toHaveAttribute('data-mode', 'scheduled');
    expect(card).toHaveTextContent('analyst · Researcher · Codex');
    expect(card).toHaveTextContent('Next run');
    expect(card).toHaveTextContent('Tomorrow, 9:45 AM');
    expect(card).toHaveTextContent('Daily 09:45');
    expect(card).toHaveTextContent('Last run');
  });

  it('emphasizes run time and outcome for History', () => {
    const activity: WorkflowActivity = {
      activityId: 'history:run-1', blueprintId: 'strategy', name: 'Strategy Report',
      schedules: [schedule], nextSchedule: schedule, activeRun: null,
      latestRun: { run_id: 'run-1', blueprint_id: 'strategy', schedule_id: 'schedule-1', status: 'completed', node_count: 2, path: '/run', updated_at: '2026-07-14T13:45:00' },
      statusLabel: 'Completed', tone: 'success', section: 'history', issue: null,
    };
    render(<WorkflowActivityCard activity={activity} agentLabels={{ 'agent-1': 'Researcher · Codex' }} now={new Date(2026, 6, 14, 12)} {...actions} />);
    const card = screen.getByTestId('workflow-activity-card-history:run-1');
    expect(card).toHaveAttribute('data-mode', 'history');
    expect(card).toHaveTextContent('Ran');
    expect(card).toHaveTextContent('Today, 1:45 PM');
    expect(card).toHaveTextContent('Outcome');
    expect(card).not.toHaveTextContent('Next run');
  });
});
```

- [ ] **Step 2: Add failing tests for Running, Needs attention, errors, actions, and expansion**

Add cases that assert:

```tsx
expect(runningCard).toHaveAttribute('data-mode', 'running');
expect(runningCard).toHaveTextContent('Started');
expect(attentionCard).toHaveAttribute('data-mode', 'attention');
expect(attentionCard).toHaveTextContent('Awaiting approval');
expect(failedCard).toHaveTextContent('Provider crashed');
expect(pausedCard).toHaveTextContent('Paused');
expect(unscheduledCard).toHaveTextContent('Not scheduled');
expect(screen.getByRole('button', { name: 'Run Strategy Report now' })).toBeVisible();
expect(screen.getByRole('button', { name: 'Show 2 more agents for Strategy Report' })).toHaveAttribute('aria-expanded', 'false');
```

Run:

```bash
npm run test -- src/features/workflows/monitor/WorkflowActivityCard.test.tsx
```

Expected: FAIL because `WorkflowActivityCard.tsx` does not exist.

- [ ] **Step 3: Implement the adaptive card component**

Implement the card with the exact prop contract below:

```tsx
interface WorkflowActivityCardProps {
  activity: WorkflowActivity;
  agentLabels: Record<string, string>;
  now?: Date;
  virtualized?: boolean;
  expandedAssignments?: boolean;
  onExpandedAssignmentsChange?: (expanded: boolean) => void;
  onOpenRun: (blueprintId: string, runId: string) => void;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onRunNow: (id: string) => void;
  onEditSchedule: (schedule: WorkflowSchedule) => void;
}
```

Derive `schedule`, `assignments`, `runTime`, `nextTime`, `lastTime`, and `duration` once at the top of the component. A paused schedule uses the literal primary label `Paused`; an active schedule without `next_run_epoch_ms` passes `emptyLabel: 'Not scheduled'` to `formatWorkflowTime`; a missing prior run passes `emptyLabel: 'Never run'`. Render:

1. Header: semantic status dot/text, workflow name, and direct icon actions.
2. Primary ownership row: `WorkflowAssignmentSummary` with two visible items.
3. Mode-specific facts in a `<dl>`:
   - Scheduled: `Next run`, `Cadence`, `Last run`.
   - History: `Ran`, `Outcome`, optional `Duration`.
   - Running: `Started`, `Updated`, `Status`.
   - Needs attention: `Action required`, `Updated`, `Status`.
4. Error summary in `var(--color-wardian-error)`.
5. Secondary blueprint and run ids in a muted footer.

Use `title={time.exact ?? undefined}` on every valid time and preserve current action callbacks and accessible labels. Use `data-testid={`workflow-activity-card-${activity.activityId}`}` and `data-mode={activity.section}` on the root `<article>`.

- [ ] **Step 4: Replace `ActivityRow` with the card and adaptive section grids**

Before rendering History cards, change `historyActivityFromRun` in
`monitorModel.ts` to use `schedule?.name ?? run.blueprint_id` for its display
name. Add a model assertion that a scheduled History run shows the human
schedule name while retaining `run.blueprint_id` as `blueprintId`.

In `ActivitySection`, render non-History activities as:

```tsx
<div className={section === 'history' ? 'grid gap-2' : 'grid gap-2 xl:grid-cols-2'}>
  {activities.map((activity) => (
    <WorkflowActivityCard
      key={activity.activityId}
      activity={activity}
      agentLabels={agentLabels}
      onOpenRun={onOpenRun}
      onPause={onPause}
      onResume={onResume}
      onRunNow={onRunNow}
      onEditSchedule={onEditSchedule}
    />
  ))}
</div>
```

Add `section: ActivitySection` to `ActivitySection`'s props and pass
`section={section}` from the existing `visibleSections.map(...)` call. This
keeps layout selection tied to the model mode instead of inferring it from the
human-readable heading.

Delete the repeated per-cell `Time`, `Workflow`, `Status`, `Schedule`, and `Assignment` JSX and the local assignment/time/provider helpers now supplied by Tasks 1 and 2.

- [ ] **Step 5: Preserve bounded History virtualization with one controlled expansion**

Rename `VirtualHistoryRows` to `VirtualHistoryCards`. Use these constants:

```ts
const HISTORY_CARD_COLLAPSED_HEIGHT_PX = 132;
const HISTORY_CARD_EXPANDED_HEIGHT_PX = 272;
const HISTORY_OVERSCAN_CARDS = 4;
const HISTORY_MAX_RENDERED_CARDS = 32;
```

Keep `expandedRunId: string | null` inside `VirtualHistoryCards`. Build an offset array from the controlled expansion state:

```ts
const layout = useMemo(() => {
  let top = 0;
  const cards = runs.map((run) => {
    const height = run.run_id === expandedRunId
      ? HISTORY_CARD_EXPANDED_HEIGHT_PX
      : HISTORY_CARD_COLLAPSED_HEIGHT_PX;
    const item = { run, top, height };
    top += height;
    return item;
  });
  return { cards, totalHeight: top };
}, [expandedRunId, runs]);
```

Find the first card whose `top + height` intersects the viewport, subtract `HISTORY_OVERSCAN_CARDS`, and render no more than `HISTORY_MAX_RENDERED_CARDS`. Position each card by its computed `top`; pass controlled assignment expansion into `WorkflowActivityCard`. Only one History card may be expanded at once, preventing unbounded layout growth while its internal assignment map remains scrollable and complete.

- [ ] **Step 6: Update Monitor integration tests to the adaptive contract**

Replace assertions tied to the old grid columns and labels with:

```tsx
expect(screen.getByTestId('workflow-activity-card-schedule:schedule-1')).toHaveAttribute('data-mode', 'scheduled');
expect(screen.queryByRole('columnheader')).toBeNull();
expect(screen.getByText('Next run')).toBeVisible();
```

Retain all existing grouping, filtering, action, schedule association, paging, sort, polling, and failure-stat tests. Update History test ids to `workflow-history-card-${run.run_id}`. Add a virtualization regression that expands a four-agent History card, verifies the virtual container height grows from 132 to 272 pixels for that item, scrolls past it, and still asserts at most 32 History cards are mounted.

- [ ] **Step 7: Run the card and Monitor suites**

Run:

```bash
npm run test -- src/features/workflows/monitor/WorkflowActivityCard.test.tsx src/features/workflows/monitor/WorkflowMonitor.test.tsx src/features/workflows/monitor/monitorModel.test.ts
```

Expected: PASS; adaptive mode assertions and all existing Monitor behaviors are green.

- [ ] **Step 8: Commit the full Monitor redesign**

```bash
git add src/features/workflows/monitor/WorkflowActivityCard.tsx src/features/workflows/monitor/WorkflowActivityCard.test.tsx src/features/workflows/monitor/WorkflowMonitor.tsx src/features/workflows/monitor/WorkflowMonitor.test.tsx
git commit -m "feat(workflows): render adaptive monitor cards"
```

---

### Task 5: Compact workflow sidebar cards and agent-roster plumbing

**Files:**
- Create: `src/features/workflows/monitor/WorkflowGlanceCard.tsx`
- Create: `src/features/workflows/monitor/WorkflowGlanceCard.test.tsx`
- Modify: `src/features/workflows/monitor/WorkflowMonitorGlance.tsx:1-242`
- Modify: `src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx`
- Modify: `src/layout/SidebarContentPane.tsx:100-177`
- Modify: `src/layout/SidebarContentPane.test.tsx:28-184`

**Interfaces:**
- Consumes: `WorkflowSchedule`, optional matching `RunSummary`, resolved assignments, and the existing glance callbacks.
- Produces: `<WorkflowGlanceCard kind="schedule" | "run" ... />`; `WorkflowMonitorGlance` gains `agents: AgentConfig[]`.

- [ ] **Step 1: Write failing compact-card tests for agents, dates, and actions**

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WorkflowGlanceCard } from './WorkflowGlanceCard';

describe('WorkflowGlanceCard', () => {
  it('shows ownership, next time, cadence, and last run for a schedule', () => {
    const onPause = vi.fn();
    render(<WorkflowGlanceCard
      kind="schedule"
      schedule={{
        id: 'schedule-1', blueprint_id: 'strategy', name: 'Strategy Report', input: {}, bindings: {},
        assignments: {
          analyst: { target_type: 'agent', agent_id: 'agent-1', conversation: 'current' },
          reviewer: { target_type: 'agent', agent_id: 'agent-2', conversation: 'current' },
          writer: { target_type: 'temporary_provider', provider: 'codex' },
        },
        schedule: { schedule_type: 'daily', time_of_day: '09:45', active: true },
        next_run_epoch_ms: new Date(2026, 6, 15, 9, 45).getTime(),
        last_run_epoch_ms: new Date(2026, 6, 14, 9, 45).getTime(), is_paused: false,
      }}
      agentLabels={{ 'agent-1': 'Researcher · Codex', 'agent-2': 'Reviewer · Claude' }}
      now={new Date(2026, 6, 14, 12)}
      onPauseSchedule={onPause}
      onResumeSchedule={vi.fn()}
      onRunScheduleNow={vi.fn()}
    />);

    expect(screen.getByText('analyst · Researcher · Codex')).toBeVisible();
    expect(screen.getByText('reviewer · Reviewer · Claude')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Show 1 more agents for Strategy Report' })).toBeVisible();
    expect(screen.getByText('Tomorrow, 9:45 AM')).toBeVisible();
    expect(screen.getByText('Daily 09:45')).toBeVisible();
    expect(screen.getByText('Today, 9:45 AM')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Pause Strategy Report' }));
    expect(onPause).toHaveBeenCalledWith('schedule-1');
  });
});
```

- [ ] **Step 2: Run the compact-card test and verify the red state**

Run:

```bash
npm run test -- src/features/workflows/monitor/WorkflowGlanceCard.test.tsx
```

Expected: FAIL because `WorkflowGlanceCard.tsx` does not exist.

- [ ] **Step 3: Implement the compact discriminated-union card**

Use this prop shape so impossible action combinations remain unrepresentable:

```ts
type WorkflowGlanceCardProps = {
  agentLabels: Record<string, string>;
  now?: Date;
} & (
  | {
      kind: 'schedule';
      schedule: WorkflowSchedule;
      onPauseSchedule: (id: string) => void;
      onResumeSchedule: (id: string) => void;
      onRunScheduleNow: (id: string) => void;
    }
  | {
      kind: 'run';
      run: RunSummary;
      schedule: WorkflowSchedule | null;
      onOpenRun: (blueprintId: string, runId: string) => void;
    }
);
```

For `kind="schedule"`, render workflow name and next time on the first line, `WorkflowAssignmentSummary compact` on the second, and cadence plus last-run time on the third. For `kind="run"`, render workflow name plus status, schedule-derived ownership when available, and `Started` or `Updated` time. Preserve the existing pause/resume/run-now/open icon labels.

- [ ] **Step 4: Replace sidebar metadata lines with compact cards**

Update `WorkflowMonitorGlance` to accept:

```ts
interface GlanceProps {
  agents: AgentConfig[];
  schedules: WorkflowSchedule[];
  activeRuns: RunSummary[];
  onOpenRun: (blueprintId: string, runId: string) => void;
  onOpenMonitor: () => void;
  onPauseSchedule: (id: string) => void;
  onResumeSchedule: (id: string) => void;
  onRunScheduleNow: (id: string) => void;
}
```

Build `agentLabels` with `buildAgentLabelMap(agents)`. Resolve a run's schedule through `run.schedule_id`, and include every assignment item's `fullLabel` in the search haystack so searching for an agent or workflow role finds the owning workflow. Keep the current Needs attention, Running, and Next sections and counts.

- [ ] **Step 5: Pass the already-loaded roster through `SidebarContentPane`**

Change the mount and local pane props:

```tsx
{activeTab === 'workflows' && (
  <WorkflowsGlancePane agents={agents} onOpenWorkflowsView={onOpenWorkflowsView} />
)}

interface WorkflowsGlancePaneProps {
  agents: AgentConfig[];
  onOpenWorkflowsView: () => void;
}
```

Pass `agents={agents}` into `WorkflowMonitorGlance`. Do not invoke `list_agents` from the sidebar.

- [ ] **Step 6: Update glance and layout tests**

Add tests that:

- Resolve three assigned agents and expose `+1 agents`.
- Format tomorrow and distant dates without truncating the primary time.
- Find a schedule by agent name through the existing search box.
- Preserve attention/running/next ordering and actions.
- Assert `SidebarContentPane` passes its `agents` prop to the mocked glance.

Run:

```bash
npm run test -- src/features/workflows/monitor/WorkflowGlanceCard.test.tsx src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx src/layout/SidebarContentPane.test.tsx
```

Expected: PASS with named agents and readable times on the narrow surface.

- [ ] **Step 7: Commit the sidebar redesign**

```bash
git add src/features/workflows/monitor/WorkflowGlanceCard.tsx src/features/workflows/monitor/WorkflowGlanceCard.test.tsx src/features/workflows/monitor/WorkflowMonitorGlance.tsx src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx src/layout/SidebarContentPane.tsx src/layout/SidebarContentPane.test.tsx
git commit -m "feat(workflows): clarify workflow sidebar cards"
```

---

### Task 6: Browser proof, user guide, and screenshot evidence

**Files:**
- Modify: `e2e/tests/schedule-monitor.spec.ts`
- Modify: `docs/guide/workflows.md:76-96`
- Create during verification: `e2e/screenshots/workflow-monitor-adaptive-cards/*/monitor-adaptive-cards.png`
- Create during verification: `e2e/screenshots/workflow-monitor-adaptive-cards/*/sidebar-multi-agent-card.png`

**Interfaces:**
- Consumes: the adaptive Monitor and sidebar data-test/accessibility contracts from Tasks 4 and 5.
- Produces: browser-level regression coverage, user documentation, and PR evidence.

- [ ] **Step 1: Extend the E2E IPC fixture with agents, assignments, and history**

Return three named `AgentConfig` records from `list_agents`. Seed one schedule with three role assignments, a `last_run_epoch_ms`, and a next run far enough in the future to exercise a dated label. Return one completed scheduled run from `workflow_list_runs` with the seeded `schedule_id`.

Keep the existing schedule-create/pause flow and add `assignments: args?.assignments ?? {}` to the mocked `schedule_create` result so newly created schedules preserve assignment context.

- [ ] **Step 2: Add browser assertions for both adaptive modes and the sidebar**

Add these assertions to `schedule-monitor.spec.ts` after app boot and before the existing create flow:

```ts
await page.getByTestId('sidebar-tab-workflows').click();
const sidebar = page.locator('aside').nth(1);
await expect(sidebar.getByText('Researcher')).toBeVisible();
await expect(sidebar.getByRole('button', { name: /show 1 more agents/i })).toBeVisible();

await page.locator('.titlebar-center').getByRole('button', { name: 'Workflows' }).click();
await page.getByTestId('workflows-view').getByRole('button', { name: /^monitor$/i }).click();
await page.getByRole('button', { name: /^scheduled$/i }).click();
await expect(page.locator('[data-mode="scheduled"]').first()).toContainText('Next run');
await expect(page.locator('[data-mode="scheduled"]').first()).toContainText('Cadence');

await page.getByRole('button', { name: /^history$/i }).click();
await expect(page.locator('[data-mode="history"]').first()).toContainText('Ran');
await expect(page.locator('[data-mode="history"]').first()).toContainText('Outcome');
await expect(page.locator('[data-mode="history"]').first()).not.toContainText('Next run');
```

- [ ] **Step 3: Run the focused browser E2E**

Run:

```bash
npm run test:e2e -- e2e/tests/schedule-monitor.spec.ts
```

Expected: PASS for the existing schedule/pause behavior and new mode-specific information hierarchy.

- [ ] **Step 4: Update the workflow guide**

Replace the current generic Monitor-row description with concise user guidance:

```md
Monitor adapts each workflow card to the question you are asking. Scheduled
cards lead with assigned agents, the next run, and cadence. History cards lead
with assigned agents, when the run happened, and its outcome. Running and Needs
attention lead with the current state and workflow owners.

Cards show two role-aware assignments by default. Use **+N agents** to inspect
the full role-to-agent map. Times use calendar labels such as **Today**,
**Tomorrow**, or a dated local timestamp; hover a time for its exact local time
and timezone.

The workflow sidebar uses the same hierarchy in compact form, so upcoming work
shows its agents, next run, cadence, and previous run without compressing them
into one metadata line.
```

- [ ] **Step 5: Capture feature-specific screenshots**

Create a timestamped evidence directory and run the focused E2E with a stable evidence timestamp.

POSIX shell:

```bash
export WARDIAN_SCREENSHOT_TIMESTAMP="$(date -u +%Y%m%d-%H%M%S)"
npm run test:e2e -- e2e/tests/schedule-monitor.spec.ts
```

PowerShell:

```powershell
$env:WARDIAN_SCREENSHOT_TIMESTAMP = (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss')
npm run test:e2e -- e2e/tests/schedule-monitor.spec.ts
```

Update the E2E test to write full Monitor and sidebar element screenshots beneath `e2e/screenshots/workflow-monitor-adaptive-cards/${process.env.WARDIAN_SCREENSHOT_TIMESTAMP}/`. Inspect both images and reject any capture with clipped cards, missing agents, or empty state.

Add the imports and capture helper directly to `schedule-monitor.spec.ts`:

```ts
import fs from 'node:fs';
import path from 'node:path';

function screenshotDirectory(): string {
  const timestamp = process.env.WARDIAN_SCREENSHOT_TIMESTAMP
    ?? new Date().toISOString().replace(/[:.]/g, '-');
  const directory = path.join('e2e', 'screenshots', 'workflow-monitor-adaptive-cards', timestamp);
  fs.mkdirSync(directory, { recursive: true });
  return directory;
}
```

Use one directory per test run:

```ts
const evidenceDirectory = screenshotDirectory();
await page.getByTestId('workflow-monitor').screenshot({
  path: path.join(evidenceDirectory, 'monitor-adaptive-cards.png'),
});
await sidebar.screenshot({
  path: path.join(evidenceDirectory, 'sidebar-multi-agent-card.png'),
});
```

- [ ] **Step 6: Commit browser proof, guide, and selected screenshot evidence**

```bash
git add e2e/tests/schedule-monitor.spec.ts docs/guide/workflows.md
git add -f "e2e/screenshots/workflow-monitor-adaptive-cards/${WARDIAN_SCREENSHOT_TIMESTAMP}/monitor-adaptive-cards.png" "e2e/screenshots/workflow-monitor-adaptive-cards/${WARDIAN_SCREENSHOT_TIMESTAMP}/sidebar-multi-agent-card.png"
git commit -m "test(workflows): document adaptive monitor cards"
```

PowerShell uses `$env:WARDIAN_SCREENSHOT_TIMESTAMP` in the same paths:

```powershell
git add e2e/tests/schedule-monitor.spec.ts docs/guide/workflows.md
git add -f "e2e/screenshots/workflow-monitor-adaptive-cards/$env:WARDIAN_SCREENSHOT_TIMESTAMP/monitor-adaptive-cards.png" "e2e/screenshots/workflow-monitor-adaptive-cards/$env:WARDIAN_SCREENSHOT_TIMESTAMP/sidebar-multi-agent-card.png"
git commit -m "test(workflows): document adaptive monitor cards"
```

---

### Task 7: Full repository verification and PR readiness

**Files:**
- Verify only; modify code only if a verification failure demonstrates a defect in Tasks 1-6.

**Interfaces:**
- Consumes: all implementation, documentation, and evidence from prior tasks.
- Produces: pre-commit evidence and a clean feature diff excluding unrelated Rust work.

- [ ] **Step 1: Run focused frontend tests once more**

```bash
npm run test -- src/features/workflows/monitor/workflowTime.test.ts src/features/workflows/monitor/assignmentPresentation.test.ts src/features/workflows/monitor/WorkflowAssignmentSummary.test.tsx src/features/workflows/monitor/monitorModel.test.ts src/features/workflows/monitor/WorkflowActivityCard.test.tsx src/features/workflows/monitor/WorkflowMonitor.test.tsx src/features/workflows/monitor/WorkflowGlanceCard.test.tsx src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx src/layout/SidebarContentPane.test.tsx
```

Expected: all focused files pass with zero failed tests.

- [ ] **Step 2: Run the full frontend gates**

```bash
npm run lint
npm run test
npm run build
npm run test:e2e
```

Expected: every command exits 0. Record existing warnings separately; do not describe warnings as failures.

- [ ] **Step 3: Run documentation gates**

```bash
npm run docs:check-llms
npm run docs:build
```

Expected: both commands exit 0.

- [ ] **Step 4: Run the backend pre-commit gates despite the frontend-only diff**

POSIX shell:

```bash
cd src-tauri
cargo clippy
cargo test
cargo check
cd ..
```

PowerShell:

```powershell
Push-Location src-tauri
cargo clippy
cargo test
cargo check
Pop-Location
```

Expected: every Cargo command exits 0. If a failure is caused by the pre-existing Rust working-tree changes, report that evidence explicitly and do not edit or stage those files as part of this feature.

- [ ] **Step 5: Audit the final diff, secrets, and staging boundary**

```bash
git diff --check origin/main...HEAD
git diff --stat origin/main...HEAD
git status --short
git diff --name-only origin/main...HEAD
```

Expected feature paths are limited to:

```text
docs/guide/workflows.md
docs/specs/2026-07-14-workflow-monitor-adaptive-cards.md
docs/superpowers/plans/2026-07-14-workflow-monitor-adaptive-cards.md
e2e/tests/schedule-monitor.spec.ts
e2e/screenshots/workflow-monitor-adaptive-cards/*/*.png
src/features/workflows/monitor/*
src/layout/SidebarContentPane.tsx
src/layout/SidebarContentPane.test.tsx
```

The two unrelated Rust files may remain modified in `git status`, but must not appear in `git diff --name-only origin/main...HEAD` or any feature commit. Scan the committed diff for credential-like keys, tokens, `.env` names, and machine-specific absolute paths before pushing.

- [ ] **Step 6: Prepare PR evidence for issue #668**

Use the repository PR template. Link `Closes #668`, explain why Monitor modes use different card hierarchies, list every verification command and result, and generate the committed screenshot URL from the recorded timestamp:

```bash
printf '%s\n' "![Workflow Monitor adaptive cards](https://raw.githubusercontent.com/wardian-app/Wardian/feat/workflow-monitor-table/e2e/screenshots/workflow-monitor-adaptive-cards/${WARDIAN_SCREENSHOT_TIMESTAMP}/monitor-adaptive-cards.png)"
```

PowerShell:

```powershell
"![Workflow Monitor adaptive cards](https://raw.githubusercontent.com/wardian-app/Wardian/feat/workflow-monitor-table/e2e/screenshots/workflow-monitor-adaptive-cards/$env:WARDIAN_SCREENSHOT_TIMESTAMP/monitor-adaptive-cards.png)"
```

Also embed the sidebar screenshot when it materially demonstrates multi-agent readability. Verify the final PR body with:

```bash
npm run check:frontend-screenshot -- origin/main HEAD
```

Expected: exit 0 with the actual PR body or configured body source containing the HTTPS image.
