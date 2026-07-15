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

  if (items.length === 0) {
    return <span className="text-[10px] text-muted">Default assignment</span>;
  }

  const collapsedItems = items.slice(0, maxVisible);
  const hiddenCount = Math.max(0, items.length - collapsedItems.length);

  return (
    <div className="min-w-0">
      <div className="flex min-w-0 flex-wrap gap-1">
        {collapsedItems.map((item) => (
          <span
            key={item.key}
            title={item.fullLabel}
            className="max-w-full truncate rounded-full border border-[color-mix(in_srgb,var(--color-wardian-accent),transparent_60%)] bg-[color-mix(in_srgb,var(--color-wardian-accent),transparent_90%)] px-2 py-0.5 text-[10px] font-medium text-[var(--color-wardian-text)]"
          >
            {item.fullLabel}
          </span>
        ))}
        {hiddenCount > 0 ? (
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={expanded
              ? `Hide additional agents for ${workflowName}`
              : `Show ${hiddenCount} more agents for ${workflowName}`}
            className="rounded-full border border-wardian-border px-2 py-0.5 text-[10px] text-muted hover:text-[var(--color-wardian-text)]"
            onClick={() => setExpanded(!expanded)}
          >
            {expanded ? 'Show less' : `+${hiddenCount} agents`}
          </button>
        ) : null}
      </div>
      {expanded && hiddenCount > 0 ? (
        <div
          data-testid="expanded-workflow-assignments"
          className={`${compact ? 'max-h-24' : 'max-h-28'} mt-2 overflow-y-auto rounded border border-wardian-border bg-[var(--color-wardian-card)] p-2`}
        >
          {items.map((item) => (
            <div
              key={item.key}
              className="grid grid-cols-[minmax(80px,auto)_minmax(0,1fr)] gap-2 py-1 text-[10px]"
            >
              <span className="font-bold text-[var(--color-wardian-text)]">{item.fullLabel}</span>
              <span className="min-w-0 break-words text-muted">{item.detailLabel}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
