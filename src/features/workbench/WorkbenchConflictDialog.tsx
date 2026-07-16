export type WorkbenchConflictDialogProps = {
  mode: "revision_conflict" | "future_schema";
  resolving?: boolean;
  on_use_disk: () => void | Promise<void>;
  on_replace_disk: () => void | Promise<void>;
  on_export_local: () => void;
};

const buttonStyle = {
  border: "1px solid var(--color-wardian-border)",
  background: "var(--color-wardian-card)",
  color: "var(--color-wardian-text)",
} as const;

/** Explicit, non-merging recovery choices for a frozen workbench draft. */
export function WorkbenchConflictDialog({
  mode,
  resolving = false,
  on_use_disk,
  on_replace_disk,
  on_export_local,
}: WorkbenchConflictDialogProps) {
  const futureSchema = mode === "future_schema";
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="workbench-conflict-title"
      style={{
        background: "var(--color-wardian-overlay)",
        color: "var(--color-wardian-text)",
      }}
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
    >
      <section
        className="w-full max-w-lg rounded-lg border p-5 shadow-xl"
        style={{
          background: "var(--color-wardian-bg)",
          borderColor: "var(--color-wardian-border-heavy)",
        }}
      >
        <h2 id="workbench-conflict-title" className="text-lg font-semibold">
          {futureSchema ? "Newer workbench version" : "Workbench changed on disk"}
        </h2>
        <p
          className="mt-2 text-sm"
          style={{ color: "var(--color-wardian-text-muted)" }}
        >
          {futureSchema
            ? "This workbench is read-only in this Wardian version. Export the local draft before upgrading."
            : "Saving is paused. Choose which complete document to keep; Wardian will not merge layouts automatically."}
        </p>
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          {!futureSchema && (
            <>
              <button
                type="button"
                style={buttonStyle}
                className="rounded px-3 py-2 text-sm"
                disabled={resolving}
                onClick={() => void on_use_disk()}
              >
                Use Disk
              </button>
              <button
                type="button"
                style={{
                  ...buttonStyle,
                  background: "var(--color-wardian-accent)",
                }}
                className="rounded px-3 py-2 text-sm"
                disabled={resolving}
                onClick={() => void on_replace_disk()}
              >
                Replace Disk
              </button>
            </>
          )}
          <button
            type="button"
            style={buttonStyle}
            className="rounded px-3 py-2 text-sm"
            disabled={resolving}
            onClick={on_export_local}
          >
            Export Local JSON
          </button>
        </div>
      </section>
    </div>
  );
}
