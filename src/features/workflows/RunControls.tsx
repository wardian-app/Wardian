import { invoke } from '@tauri-apps/api/core';

export type RunControlStatus = 'running' | 'awaiting_approval' | 'completed' | 'failed' | 'interrupted';

interface RunControlsProps {
  blueprintId: string;
  runId: string;
  blueprintPath: string;
  status: RunControlStatus;
  awaitingNode: string | null;
  onChanged: () => void;
}

export function RunControls({
  blueprintId,
  runId,
  blueprintPath,
  status,
  awaitingNode,
  onChanged,
}: RunControlsProps) {
  const call = async (cmd: string, args: Record<string, unknown>) => {
    try {
      await invoke(cmd, args);
      onChanged();
    } catch {
      // The observing container refreshes run state and surfaces backend failures.
    }
  };

  return (
    <div className="run-controls flex gap-2" data-testid="run-controls">
      {status === 'awaiting_approval' && awaitingNode && (
        <>
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-success)] px-2 py-1 text-xs text-[var(--color-wardian-bg)]"
            onClick={() =>
              call('workflow_approve', {
                blueprintId,
                runId,
                blueprintPath,
                node: awaitingNode,
                granted: true,
                actor: 'user',
              })
            }
          >
            Approve
          </button>
          <button
            type="button"
            className="rounded bg-[var(--color-wardian-error)] px-2 py-1 text-xs text-[var(--color-wardian-bg)]"
            onClick={() =>
              call('workflow_approve', {
                blueprintId,
                runId,
                blueprintPath,
                node: awaitingNode,
                granted: false,
                actor: 'user',
              })
            }
          >
            Reject
          </button>
        </>
      )}
      {status === 'interrupted' && (
        <button
          type="button"
          className="rounded border border-wardian-border px-2 py-1 text-xs text-primary"
          onClick={() => call('workflow_resume', { blueprintId, runId, blueprintPath })}
        >
          Resume
        </button>
      )}
      {status === 'running' && (
        <button
          type="button"
          className="rounded border border-wardian-border px-2 py-1 text-xs text-primary"
          onClick={() => call('workflow_cancel', { blueprintId, runId })}
        >
          Cancel
        </button>
      )}
    </div>
  );
}
