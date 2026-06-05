import type { NodeStatusKind, RunEvent, RunState } from './runTypes';

interface NodeInspectorProps {
  selectedNodeId: string | null;
  state: RunState | null;
  currentStatuses: Record<string, NodeStatusKind>;
  events: RunEvent[];
}

function nodePayload(events: RunEvent[], nodeId: string): { output?: unknown; error?: string } {
  const payload: { output?: unknown; error?: string } = {};
  for (const event of events) {
    if (!('node' in event) || event.node !== nodeId) {
      continue;
    }
    if (event.kind === 'node_completed') {
      payload.output = event.output;
      payload.error = undefined;
    }
    if (event.kind === 'node_failed') {
      payload.error = event.error;
    }
  }
  return payload;
}

const TIMESTAMP_FIELD_PATTERN = /(^ts$|timestamp|_at$|At$)/;

function inspectorDisplayValue(value: unknown, key?: string): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => inspectorDisplayValue(item));
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([entryKey, entryValue]) => [
        entryKey,
        inspectorDisplayValue(entryValue, entryKey),
      ]),
    );
  }
  if (typeof value === 'string' && key && TIMESTAMP_FIELD_PATTERN.test(key)) {
    const date = new Date(value);
    if (Number.isFinite(date.getTime())) {
      return date.toLocaleString();
    }
  }
  return value;
}

export function NodeInspector({ selectedNodeId, state, currentStatuses, events }: NodeInspectorProps) {
  if (!selectedNodeId) {
    return (
      <div className="rounded-lg border border-dashed border-wardian-border p-4 text-center text-xs text-[var(--color-wardian-text-muted)]">
        Select a node to inspect it.
      </div>
    );
  }

  const status = currentStatuses[selectedNodeId] ?? state?.nodes[selectedNodeId] ?? 'pending';
  const payload = nodePayload(events, selectedNodeId);
  const output = payload.output === undefined ? null : JSON.stringify(inspectorDisplayValue(payload.output), null, 2);

  return (
    <aside className="flex h-full min-h-0 select-text flex-col gap-4 rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] p-4">
      <div>
        <div className="text-[10px] font-bold uppercase tracking-wide text-[var(--color-wardian-text-muted)]">Node</div>
        <div className="mt-1 truncate text-lg font-bold text-[var(--color-wardian-text)]">{selectedNodeId}</div>
      </div>

      <div className="grid grid-cols-[88px_1fr] gap-2 text-xs">
        <span className="font-bold text-[var(--color-wardian-text-muted)]">Status</span>
        <span className="font-mono text-[var(--color-wardian-text)]">{status}</span>
        <span className="font-bold text-[var(--color-wardian-text-muted)]">Run</span>
        <span className="truncate font-mono text-[var(--color-wardian-text)]">{state?.run_id ?? '-'}</span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mb-2 text-[10px] font-bold uppercase tracking-wide text-[var(--color-wardian-text-muted)]">Output</div>
        {output ? (
          <pre className="overflow-x-auto rounded border border-wardian-border bg-[var(--color-wardian-bg)] p-3 text-[11px] text-[var(--color-wardian-text)]">
            {output}
          </pre>
        ) : (
          <div className="rounded border border-dashed border-wardian-border p-3 text-xs text-[var(--color-wardian-text-muted)]">
            No output recorded.
          </div>
        )}
      </div>

      {payload.error ? (
        <div>
          <div className="mb-2 text-[10px] font-bold uppercase tracking-wide text-[var(--color-wardian-error)]">Failure</div>
          <div className="rounded border border-[color-mix(in_srgb,var(--color-wardian-error),transparent_55%)] bg-[color-mix(in_srgb,var(--color-wardian-error),transparent_90%)] p-3 text-xs text-[var(--color-wardian-error)]">
            {payload.error}
          </div>
        </div>
      ) : null}
    </aside>
  );
}
