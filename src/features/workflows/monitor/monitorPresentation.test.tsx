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
    expect(screen.getAllByText(/^(?:Agent · (?:Current session|Fresh background)|Temporary provider · Ephemeral)$/)).toHaveLength(3);
  });
});
