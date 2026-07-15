# Workflow Monitor Adaptive Cards Lean Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace workflow Monitor rows and sidebar metadata lines with adaptive cards that make assigned agents and mode-relevant times readable.

**Architecture:** Keep existing stores, polling, filtering, actions, paging, and bounded History virtualization. Extract small time and assignment presentation helpers, then use them in a full Monitor card and compact sidebar card.

**Tech Stack:** React 19, TypeScript, semantic Wardian theme variables, Vitest/React Testing Library, and Playwright.

## Global Constraints

- GitHub issue: [#668](https://github.com/wardian-app/Wardian/issues/668).
- Human-approved test scope amendment on 2026-07-15: avoid duplicate pure-model and component suites. Keep only tests for user-visible hierarchy, time fallbacks, multi-agent expansion, existing actions, search, and bounded virtualization. A missing new export/module is an acceptable initial RED result for this plan.
- Do not modify Rust, Tauri commands, workflow DTOs, execution semantics, polling intervals, history retention, or Observe mode.
- Preserve and never stage the unrelated changes in `src-tauri/src/commands/agent.rs` and `src-tauri/src/manager/telemetry.rs`.
- Use semantic Wardian theme variables/classes only; do not introduce hardcoded Tailwind palette colors.
- Do not introduce `any`.
- Show two role-aware agent chips, then an accessible `+N agents` expansion.
- Scheduled emphasizes next run/cadence; History emphasizes run time/outcome; Running and Needs attention emphasize state/ownership.
- Keep existing open, pause, resume, run-now, and edit action behavior and labels.
- Keep History rendering bounded to at most 32 mounted cards.

---

### Task 1: Shared time and assignment presentation

**Files:**
- Create: `src/features/workflows/monitor/workflowTime.ts`
- Create: `src/features/workflows/monitor/assignmentPresentation.ts`
- Create: `src/features/workflows/monitor/WorkflowAssignmentSummary.tsx`
- Create: `src/features/workflows/monitor/monitorPresentation.test.tsx`

**Interfaces:**
- Produces `formatWorkflowTime`, `runTimestampValue`, `formatRunDuration`, `buildAgentLabelMap`, `workflowAssignmentItems`, and `WorkflowAssignmentSummary`.

- [ ] Add one compact failing test file covering the essential contract:

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { formatWorkflowTime } from './workflowTime';
import { workflowAssignmentItems } from './assignmentPresentation';
import { WorkflowAssignmentSummary } from './WorkflowAssignmentSummary';

const now = new Date(2026, 6, 14, 12, 0);

describe('monitor presentation', () => {
  it('uses calendar-aware labels and explicit fallbacks', () => {
    expect(formatWorkflowTime(new Date(2026, 6, 14, 15, 20), { now, locale: 'en-US' }).primary).toBe('Today, 3:20 PM');
    expect(formatWorkflowTime(new Date(2026, 6, 15, 9, 45), { now, locale: 'en-US' }).primary).toBe('Tomorrow, 9:45 AM');
    expect(formatWorkflowTime(new Date(2026, 6, 16, 9, 35), { now, locale: 'en-US' }).primary).toBe('Thu, Jul 16 · 9:35 AM');
    expect(formatWorkflowTime(new Date(2026, 9, 1, 8), { now, locale: 'en-US' }).primary).toBe('Oct 1, 2026 · 8:00 AM');
    expect(formatWorkflowTime(null, { now, emptyLabel: 'Never run' }).primary).toBe('Never run');
    expect(formatWorkflowTime('invalid', { now }).primary).toBe('invalid');
  });

  it('shows two stable role assignments and expands the rest', () => {
    const items = workflowAssignmentItems({
      writer: { target_type: 'agent', agent_id: 'missing', conversation: 'fresh_background' },
      analyst: { target_type: 'agent', agent_id: 'a1', conversation: 'current' },
      reviewer: { target_type: 'temporary_provider', provider: 'codex' },
    }, {}, null, { a1: 'Researcher · Claude' });
    expect(items.map((item) => item.role)).toEqual(['analyst', 'reviewer', 'writer']);
    const onExpandedChange = vi.fn();
    const { rerender } = render(<WorkflowAssignmentSummary workflowName="Strategy" items={items} expanded={false} onExpandedChange={onExpandedChange} />);
    expect(screen.getByText('analyst · Researcher · Claude')).toBeVisible();
    expect(screen.getByText('reviewer · Temporary Codex')).toBeVisible();
    expect(screen.queryByText('writer · missing')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show 1 more agents for Strategy' }));
    expect(onExpandedChange).toHaveBeenCalledWith(true);
    rerender(<WorkflowAssignmentSummary workflowName="Strategy" items={items} expanded onExpandedChange={onExpandedChange} />);
    expect(screen.getByText('writer · missing')).toBeVisible();
  });
});
```

- [ ] Run `npm run test -- src/features/workflows/monitor/monitorPresentation.test.tsx` and record the expected RED caused by missing presentation modules.

- [ ] Implement the helpers using the approved interfaces from `docs/specs/2026-07-14-workflow-monitor-adaptive-cards.md`:

```ts
export interface WorkflowTimeLabel { primary: string; exact: string | null; valid: boolean }
export interface WorkflowTimeOptions { now?: Date; locale?: string; emptyLabel?: string }
export function formatWorkflowTime(value: string | number | Date | null | undefined, options?: WorkflowTimeOptions): WorkflowTimeLabel;
export function runTimestampValue(run: RunSummary): string | null;
export function formatRunDuration(run: RunSummary): string | null;

export interface WorkflowAssignmentItem {
  key: string;
  role: string;
  targetLabel: string;
  detailLabel: string;
  fullLabel: string;
}
export function buildAgentLabelMap(agents: AgentConfig[]): Record<string, string>;
export function workflowAssignmentItems(
  assignments: WorkflowAssignments | undefined,
  bindings: Record<string, string> | undefined,
  provider: string | null | undefined,
  labels: Record<string, string>,
): WorkflowAssignmentItem[];
```

`formatWorkflowTime` uses `Today`, `Tomorrow`, a weekday/date within six calendar days, and a full date otherwise. It exposes exact local date/time/timezone in `exact`, keeps invalid input unchanged, and uses `emptyLabel` for missing input. Assignments sort by role, resolve live names, preserve missing ids, and humanize temporary providers.

`WorkflowAssignmentSummary` supports controlled and uncontrolled expansion, defaults to two chips, uses `aria-expanded`, renders a complete scrollable role map when expanded, and uses accent/text theme variables rather than status colors.

- [ ] Re-run the focused test and require PASS.
- [ ] Commit: `feat(workflows): add monitor presentation helpers`.

---

### Task 2: Adaptive full Monitor cards

**Files:**
- Create: `src/features/workflows/monitor/monitorModel.ts`
- Create: `src/features/workflows/monitor/WorkflowActivityCard.tsx`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.tsx`
- Modify: `src/features/workflows/monitor/WorkflowMonitor.test.tsx`

**Interfaces:**
- Move the existing activity types and pure builder/group/sort helpers into `monitorModel.ts` without changing their behavior.
- Produce `WorkflowActivityCard` with the existing action callbacks and `data-mode={activity.section}`.

- [ ] Add only these failing user-visible assertions to the existing Monitor suite:

```tsx
expect(scheduledCard).toHaveAttribute('data-mode', 'scheduled');
expect(scheduledCard).toHaveTextContent('Next run');
expect(scheduledCard).toHaveTextContent('Cadence');
expect(scheduledCard).toHaveTextContent('Last run');
expect(historyCard).toHaveAttribute('data-mode', 'history');
expect(historyCard).toHaveTextContent('Ran');
expect(historyCard).toHaveTextContent('Outcome');
expect(historyCard).not.toHaveTextContent('Next run');
expect(screen.getByRole('button', { name: /show 2 more agents/i })).toHaveAttribute('aria-expanded', 'false');
```

Retain existing tests for grouping, filters, failures, actions, polling, paging, and bounded History rendering. Delete assertions coupled to the old six-column grid.

- [ ] Run `npm run test -- src/features/workflows/monitor/WorkflowMonitor.test.tsx` and record assertion RED.

- [ ] Extract the current model code into `monitorModel.ts`, exporting `ActivityFilter`, `ActivitySection`, `ActivityTone`, `WorkflowActivity`, `WorkflowMonitorModel`, `buildActivities`, `buildMonitorModel`, `historyActivityFromRun`, `groupActivities`, `sectionsForFilter`, and `scheduleLookupById`. Use `schedule?.name ?? run.blueprint_id` for scheduled History display names.

- [ ] Implement `WorkflowActivityCard.tsx`:

```ts
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

Render workflow/status/actions in the header, assignments as primary content, then a mode-specific definition list. Scheduled renders Next run/Cadence/Last run; History renders Ran/Outcome/optional Duration; Running renders Started/Updated/Status; Needs attention renders Action required/Updated/Status. Render errors semantically and raw ids in a muted footer. Paused shows `Paused`; missing next run shows `Not scheduled`; missing prior run shows `Never run`.

- [ ] Replace `ActivityRow` with adaptive cards. Non-History sections use `grid gap-2 xl:grid-cols-2`; History remains one chronological column.

- [ ] Preserve bounded variable-height History virtualization using 132px collapsed and 272px expanded heights, a single controlled expanded History card, four-card overscan, and at most 32 mounted cards. Update the existing virtualization test to expand one card and still assert the 32-card bound.

- [ ] Re-run `npm run test -- src/features/workflows/monitor/WorkflowMonitor.test.tsx` and require PASS.
- [ ] Commit: `feat(workflows): render adaptive monitor cards`.

---

### Task 3: Compact workflow sidebar cards

**Files:**
- Create: `src/features/workflows/monitor/WorkflowGlanceCard.tsx`
- Modify: `src/features/workflows/monitor/WorkflowMonitorGlance.tsx`
- Modify: `src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx`
- Modify: `src/layout/SidebarContentPane.tsx`
- Modify: `src/layout/SidebarContentPane.test.tsx`

**Interfaces:**
- `WorkflowMonitorGlance` gains `agents: AgentConfig[]`.
- `WorkflowGlanceCard` is a discriminated union for `kind: 'schedule'` and `kind: 'run'`.

- [ ] Add two failing tests to existing suites: one renders three role assignments and expects two named agents plus an accessible `+1 agents`; the other searches by resolved agent name and expects the owning workflow. Update the layout mock to assert the existing `agents` prop reaches the glance.
- [ ] Run `npm run test -- src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx src/layout/SidebarContentPane.test.tsx` and record assertion RED.
- [ ] Implement compact cards. Scheduled cards show workflow/next run, assignments, then cadence/last run. Run cards show workflow/status, schedule-derived ownership when available, and started/updated time. Preserve pause/resume/run-now/open labels.
- [ ] Build labels with `buildAgentLabelMap(agents)`, match runs to schedules by `schedule_id`, and include assignment full labels in the search haystack.
- [ ] Pass the already-loaded `agents` array from `SidebarContentPane` through `WorkflowsGlancePane`; do not invoke `list_agents` from the sidebar.
- [ ] Re-run the two focused suites and require PASS.
- [ ] Commit: `feat(workflows): clarify workflow sidebar cards`.

---

### Task 4: Browser proof, guide, and screenshots

**Files:**
- Modify: `e2e/tests/schedule-monitor.spec.ts`
- Modify: `docs/guide/workflows.md`
- Create: `e2e/screenshots/workflow-monitor-adaptive-cards/*/monitor-adaptive-cards.png`
- Create: `e2e/screenshots/workflow-monitor-adaptive-cards/*/sidebar-multi-agent-card.png`

- [ ] Extend the existing IPC fixture with three named agents, one three-role schedule, and one completed scheduled run. Preserve the current schedule-create/pause path.
- [ ] Add one browser test flow that proves: sidebar shows two agents and `+1`; Scheduled shows Next run/Cadence; History shows Ran/Outcome and omits Next run; pause still changes to resume.
- [ ] Run `npm run test:e2e -- e2e/tests/schedule-monitor.spec.ts` and require PASS.
- [ ] Update `docs/guide/workflows.md` with the mode hierarchy, `+N agents`, calendar-aware labels, and compact sidebar behavior.
- [ ] Capture and inspect timestamped Monitor and sidebar element screenshots. Reject clipped, empty, or agent-less captures. Force-add the two selected PNGs because `e2e/screenshots/` is ignored.
- [ ] Commit: `test(workflows): document adaptive monitor cards`.

---

### Task 5: Full verification and branch review

**Files:** Verify only; fix only demonstrated feature defects.

- [ ] Run focused tests:

```bash
npm run test -- src/features/workflows/monitor/monitorPresentation.test.tsx src/features/workflows/monitor/WorkflowMonitor.test.tsx src/features/workflows/monitor/WorkflowMonitorGlance.test.tsx src/layout/SidebarContentPane.test.tsx
```

- [ ] Run full frontend and docs gates: `npm run lint`, `npm run test`, `npm run build`, `npm run test:e2e`, `npm run docs:check-llms`, and `npm run docs:build`.
- [ ] Run backend gates in `src-tauri`: `cargo clippy`, `cargo test`, and `cargo check`. If unrelated Rust edits cause failure, report evidence and do not change/stage them.
- [ ] Run `git diff --check origin/main...HEAD`, inspect `git diff --name-only origin/main...HEAD`, scan for credential-like content and machine-specific paths, and confirm unrelated Rust files are absent from commits.
- [ ] Generate a whole-branch review package and complete final code review. Address all Critical/Important findings with focused regression tests.
- [ ] Prepare the PR template with `Closes #668`, verification results, and an embedded HTTPS Monitor screenshot; run `npm run check:frontend-screenshot -- origin/main HEAD` against the final body.
