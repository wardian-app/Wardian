import type { Diagnostic } from './blueprintTypes';

export function DiagnosticsPanel({ diagnostics, onFocusNode }: { diagnostics: Diagnostic[]; onFocusNode: (id: string) => void }) {
  if (diagnostics.length === 0) return <div className="diagnostics-panel text-muted">No issues.</div>;
  return (
    <div className="diagnostics-panel" data-testid="diagnostics-panel">
      {diagnostics.map((d, i) => (
        <button key={i} className={`diagnostic ${d.severity}`} onClick={() => d.node && onFocusNode(d.node)}>
          <span className="code">{d.code}</span> <span className="message">{d.message}</span>
        </button>
      ))}
    </div>
  );
}
