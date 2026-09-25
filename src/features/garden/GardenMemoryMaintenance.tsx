import { useEffect, useId, useRef, useState, type ChangeEvent, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import "./garden-memory-maintenance.css";

const MAX_PLAN_BYTES = 1024 * 1024;
const MAX_TEXT_CHARACTERS = 8192;
const MAX_LOCATOR_CHARACTERS = 4096;
const MAX_SOURCES_PER_OPERATION = 64;

interface MaintenancePlan {
  schema_version: 1;
  plan_id: string;
  agent_id: string;
  idempotency_key: string;
  operations: Record<string, unknown>[];
}

interface MemorySummary {
  memory_id?: string | null;
  revision_id?: string | null;
  text: string;
  kind: "stable" | "current";
  scope: { kind: "agent" } | { kind: "workspace"; path: string };
  evidence_excerpt: string;
  sources: Record<string, unknown>[];
}

interface MaintenanceChange {
  operation_index: number;
  op: string;
  before?: MemorySummary | null;
  after?: MemorySummary | null;
  source_additions: Record<string, unknown>[];
  absorbed_memory_ids: string[];
  reason?: string | null;
}

interface MaintenanceConflict {
  operation_index: number;
  code: string;
  explanation: string;
}

interface MaintenancePreview {
  plan_id: string;
  agent_id: string;
  operation_count: number;
  preview_digest: string;
  changes: MaintenanceChange[];
  conflicts: MaintenanceConflict[];
}

interface MaintenanceReceipt {
  plan_id: string;
  agent_id: string;
  idempotency_key: string;
  preview_digest: string;
  applied_at: string;
  [field: string]: unknown;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exceedsCharacters(value: string, limit: number): boolean {
  return Array.from(value).length > limit;
}

/** The browser bounds only import and owner identity; the backend validates every operation. */
function readPlan(value: unknown, agentId: string): MaintenancePlan {
  if (!isObject(value) || value.schema_version !== 1 ||
      typeof value.plan_id !== "string" || !value.plan_id ||
      typeof value.agent_id !== "string" ||
      typeof value.idempotency_key !== "string" || !value.idempotency_key ||
      !Array.isArray(value.operations) || value.operations.length < 1 || value.operations.length > 100 ||
      !value.operations.every(isObject)) {
    throw new Error("Invalid maintenance plan: expected version 1 with 1–100 operations.");
  }
  if (value.agent_id !== agentId) throw new Error(`Plan owner ${value.agent_id} does not match selected agent ${agentId}.`);
  for (const [index, operation] of value.operations.entries()) {
    if (operation.op === "revise" || operation.op === "create") {
      if (typeof operation.text !== "string" || exceedsCharacters(operation.text, MAX_TEXT_CHARACTERS) ||
          typeof operation.evidence_excerpt !== "string" || exceedsCharacters(operation.evidence_excerpt, MAX_TEXT_CHARACTERS)) {
        throw new Error(`Operation ${index + 1} exceeds the 8,192-character text or evidence limit.`);
      }
      const sources = operation.op === "revise" ? operation.add_sources : operation.sources;
      if (sources !== undefined && (!Array.isArray(sources) || sources.length > MAX_SOURCES_PER_OPERATION ||
          !sources.every((source) => isObject(source) &&
            (source.locator === undefined || source.locator === null ||
              (typeof source.locator === "string" && !exceedsCharacters(source.locator, MAX_LOCATOR_CHARACTERS)))))) {
        throw new Error(`Operation ${index + 1} exceeds the 64-source or 4,096-character locator limit.`);
      }
    }
  }
  return value as unknown as MaintenancePlan;
}

function readPreview(value: MaintenancePreview, plan: MaintenancePlan): MaintenancePreview {
  if (!isObject(value) || value.plan_id !== plan.plan_id || value.agent_id !== plan.agent_id ||
      value.operation_count !== plan.operations.length ||
      typeof value.preview_digest !== "string" || !value.preview_digest ||
      !Array.isArray(value.changes) || !Array.isArray(value.conflicts)) {
    throw new Error("Preview response does not match the imported plan.");
  }
  return value;
}

function scopeLabel(scope: MemorySummary["scope"]): string {
  return scope.kind === "agent" ? "Agent-wide" : `Workspace: ${scope.path}`;
}

function Sources({ sources, empty }: { sources: Record<string, unknown>[]; empty: string }) {
  return sources.length ? <ol className="garden-maintenance-sources">{sources.map((source, index) =>
    <li key={index}><pre>{JSON.stringify(source, null, 2)}</pre></li>)}</ol> : <span>{empty}</span>;
}

function RecordSummary({ title, record, operationNumber }: { title: string; record?: MemorySummary | null; operationNumber: number }) {
  return <section className="garden-maintenance-record" aria-label={`Operation ${operationNumber} ${title}`}>
    <h4>{title}</h4>
    {!record ? <p>No active record</p> : <>
      <dl>
        <dt>Memory ID</dt><dd>{record.memory_id || "Allocated on apply"}</dd>
        <dt>Revision ID</dt><dd>{record.revision_id || "Allocated on apply"}</dd>
        <dt>Kind</dt><dd>{record.kind}</dd>
        <dt>Scope</dt><dd>{scopeLabel(record.scope)}</dd>
        <dt>Evidence</dt><dd>{record.evidence_excerpt}</dd>
        <dt>Sources</dt><dd><Sources sources={record.sources} empty="No sources" /></dd>
      </dl>
      <h5>Full text</h5><pre className="garden-maintenance-text">{record.text}</pre>
    </>}
  </section>;
}

function Change({ change, position }: { change: MaintenanceChange; position: number }) {
  const retirement = change.op === "retire";
  return <li className="garden-maintenance-change">
    <h3>Operation {position + 1}: {change.op.replace(/_/g, " ")}{retirement ? " — explicit retirement" : ""}</h3>
    <p>Plan operation index: {change.operation_index}</p>
    {change.reason && <p><strong>Reason:</strong> {change.reason}</p>}
    <div className="garden-maintenance-records">
      <RecordSummary title="Before" record={change.before} operationNumber={position + 1} />
      <RecordSummary title={retirement ? "After: retired" : "After"} record={change.after} operationNumber={position + 1} />
    </div>
    <h4>Source additions</h4><Sources sources={change.source_additions} empty="No source additions" />
    <p><strong>Absorbed memories:</strong> {change.absorbed_memory_ids.length ? change.absorbed_memory_ids.join(", ") : "None"}</p>
  </li>;
}

export interface GardenMemoryMaintenanceProps {
  agentId: string;
  agentName: string;
  onApplied: () => void;
}

/** Desktop operator review surface. The host command owns confirmation and atomic apply. */
export function GardenMemoryMaintenance({ agentId, agentName, onApplied }: GardenMemoryMaintenanceProps) {
  const [open, setOpen] = useState(false);
  const [plan, setPlan] = useState<MaintenancePlan | null>(null);
  const [preview, setPreview] = useState<MaintenancePreview | null>(null);
  const [receipt, setReceipt] = useState<MaintenanceReceipt | null>(null);
  const [fileName, setFileName] = useState("");
  const [busy, setBusy] = useState(false);
  const [checkingReceipt, setCheckingReceipt] = useState(false);
  const [applyAttempted, setApplyAttempted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const attemptedDigest = useRef<string | null>(null);
  const dialogRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();

  useEffect(() => {
    if (!open) return;
    const trigger = triggerRef.current;
    closeRef.current?.focus();
    return () => { trigger?.focus(); };
  }, [open]);

  useEffect(() => () => { requestId.current += 1; }, []);

  const close = () => { if (!busy) setOpen(false); };
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") { event.preventDefault(); close(); return; }
    if (event.key !== "Tab") return;
    const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ) ?? []);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) { event.preventDefault(); return; }
    if (event.shiftKey && (document.activeElement === first || !dialogRef.current?.contains(document.activeElement))) {
      event.preventDefault(); last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || !dialogRef.current?.contains(document.activeElement))) {
      event.preventDefault(); first.focus();
    }
  };

  const runPreview = async (candidate: MaintenancePlan, currentRequest: number) => {
    setApplyAttempted(false); attemptedDigest.current = null;
    setBusy(true); setError(null);
    try {
      const answer = readPreview(await invoke<MaintenancePreview>("memory_maintenance_preview", { plan: candidate }), candidate);
      if (currentRequest === requestId.current) setPreview(answer);
    } catch (cause: unknown) {
      if (currentRequest === requestId.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (currentRequest === requestId.current) setBusy(false);
    }
  };

  const importFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    const currentRequest = ++requestId.current;
    setPlan(null); setPreview(null); setReceipt(null); setError(null);
    setApplyAttempted(false); attemptedDigest.current = null;
    setFileName(file?.name ?? "");
    if (!file) return;
    if (file.size > MAX_PLAN_BYTES) {
      setError("Plan exceeds the 1 MiB import limit.");
      return;
    }
    try {
      const rawJson = await file.text();
      const parsed = await invoke<unknown>("memory_maintenance_parse", { rawJson });
      const candidate = readPlan(parsed, agentId);
      if (currentRequest !== requestId.current) return;
      setPlan(candidate);
      await runPreview(candidate, currentRequest);
    } catch (cause: unknown) {
      if (currentRequest === requestId.current) setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const apply = async () => {
    if (!plan || !preview || busy || preview.conflicts.length) return;
    const currentRequest = requestId.current;
    attemptedDigest.current = preview.preview_digest;
    setApplyAttempted(true);
    setBusy(true); setError(null);
    try {
      const answer = await invoke<MaintenanceReceipt>("memory_maintenance_apply", { plan, previewDigest: preview.preview_digest });
      if (currentRequest !== requestId.current) return;
      if (answer.plan_id !== plan.plan_id || answer.agent_id !== agentId ||
          answer.idempotency_key !== plan.idempotency_key || answer.preview_digest !== preview.preview_digest) {
        throw new Error("Apply returned a receipt for a different plan. Check the receipt before retrying.");
      }
      setReceipt(answer);
      setPreview(null);
      setApplyAttempted(false);
      onApplied();
    } catch (cause: unknown) {
      if (currentRequest !== requestId.current) return;
      setPreview(null); // Every failed apply needs a fresh, revision-bound preview.
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (currentRequest === requestId.current) setBusy(false);
    }
  };

  const checkReceipt = async () => {
    if (!plan || !applyAttempted || busy) return;
    const currentRequest = requestId.current;
    setBusy(true); setCheckingReceipt(true); setError(null);
    try {
      const answer = await invoke<MaintenanceReceipt | null>("memory_maintenance_receipt", {
        agentId, idempotencyKey: plan.idempotency_key,
      });
      if (currentRequest !== requestId.current) return;
      if (!answer) {
        setError("No apply receipt found. Preview the plan again before applying.");
      } else if (answer.plan_id !== plan.plan_id || answer.agent_id !== agentId ||
                 answer.idempotency_key !== plan.idempotency_key || answer.preview_digest !== attemptedDigest.current) {
        setError("Receipt does not match the attempted plan. Do not retry without reviewing the record.");
      } else {
        setReceipt(answer);
        setApplyAttempted(false);
        onApplied();
      }
    } catch (cause: unknown) {
      if (currentRequest === requestId.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (currentRequest === requestId.current) { setBusy(false); setCheckingReceipt(false); }
    }
  };

  return <>
    <button ref={triggerRef} type="button" className="garden-agent-interior-action" onClick={() => setOpen(true)}>Maintain memory…</button>
    {open && createPortal(<div className="garden-maintenance-overlay" onMouseDown={(event) => { if (event.target === event.currentTarget) close(); }}>
      <section ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby={titleId} className="garden-maintenance-dialog" onKeyDown={onKeyDown}>
        <header><div><h2 id={titleId}>Memory maintenance</h2><p>{agentName} · owner {agentId}</p></div>
          <button ref={closeRef} type="button" onClick={close} disabled={busy} aria-label="Close memory maintenance">Close</button></header>
        <div className="garden-maintenance-body">
          <p>Import a local JSON plan for this agent. Review every change before applying. The desktop will request native confirmation.</p>
          <label className="garden-maintenance-file">Maintenance plan (.json)
            <input type="file" accept=".json,application/json" disabled={busy} onChange={(event) => { void importFile(event); }} />
          </label>
          <p className="garden-maintenance-hint">Maximum 1 MiB · 1–100 operations{fileName ? ` · ${fileName}` : ""}</p>
          {busy && <p role="status">{checkingReceipt ? "Checking receipt…" : preview ? "Applying plan…" : "Previewing plan…"}</p>}
          {error && <p role="alert" className="garden-maintenance-error">{error}</p>}
          {applyAttempted && !busy && <button type="button" onClick={() => { void checkReceipt(); }}>Check apply receipt</button>}
          {plan && !preview && !receipt && !busy && <button type="button" onClick={() => { void runPreview(plan, ++requestId.current); }}>Preview again</button>}
          {preview && <>
            <dl className="garden-maintenance-facts"><dt>Owner</dt><dd>{preview.agent_id}</dd><dt>Operations</dt><dd>{preview.operation_count}</dd>
              <dt>Preview digest</dt><dd className="garden-maintenance-digest">{preview.preview_digest}</dd></dl>
            <h3>Conflicts ({preview.conflicts.length})</h3>
            {preview.conflicts.length ? <ol className="garden-maintenance-conflicts">{preview.conflicts.map((conflict, index) =>
              <li key={`${conflict.operation_index}:${conflict.code}:${index}`}>Operation index {conflict.operation_index} · {conflict.code}: {conflict.explanation}</li>)}</ol>
              : <p>No conflicts found in this preview.</p>}
            <h3>Changes ({preview.changes.length})</h3>
            <ol className="garden-maintenance-changes">{preview.changes.map((change, index) => <Change key={`${change.operation_index}:${index}`} change={change} position={index} />)}</ol>
            <button type="button" className="garden-maintenance-apply" disabled={busy || preview.conflicts.length > 0} onClick={() => { void apply(); }}>Apply reviewed plan…</button>
          </>}
          {receipt && <section className="garden-maintenance-receipt" aria-label="Memory maintenance receipt" role="status">
            <h3>Applied</h3><p>Plan {receipt.plan_id} · owner {receipt.agent_id} · {receipt.applied_at}</p>
            <p>Idempotency key: {receipt.idempotency_key}</p><p>Digest: {receipt.preview_digest}</p>
            <h4>Full receipt</h4><pre>{JSON.stringify(receipt, null, 2)}</pre>
          </section>}
        </div>
      </section>
    </div>, document.body)}
  </>;
}
