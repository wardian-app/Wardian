import type { GardenEntityRef } from "./garden.types";
import type { SituatedAutomationInput } from "./automationProjection";
import type { AgentConfig } from "../../types";
import { automationRunStatusColor } from "../automations/run/statusLabels";

/** Each concurrent execution keeps its own immutable blueprint, assignments and evidence. */
export function GardenAutomationInterior({ automation, agents, selectedKey, onSelect, onEnter, stageId, onOpenDefinition, onInspectRun, onManageSchedule }: {
  automation?: SituatedAutomationInput;
  agents: readonly AgentConfig[];
  selectedKey: string | null;
  onSelect: (ref: GardenEntityRef) => void;
  onEnter: (ref: GardenEntityRef) => void;
  stageId?: string;
  onOpenDefinition: (path: string) => void;
  onInspectRun?: (blueprintId: string, runId: string) => void;
  onManageSchedule?: () => void;
}) {
  if (!automation) return <p>This routine is no longer in the current activity window. Return to its workstream or use Automation Monitor for historical runs.</p>;
  const staleNotice = automation.stale && <p role="status">Showing the last loaded snapshot. {automation.evidenceErrors?.join("; ")}</p>;
  const lanes = automation.runLanes.length ? automation.runLanes : [{ runId: "schedule", stages: automation.stages, executionAgentIds: automation.executionAgentIds }];
  if (stageId) {
    let reference: unknown;
    try { reference = JSON.parse(stageId); } catch { reference = null; }
    if (!Array.isArray(reference) || reference.length !== 2 || !reference.every((part) => typeof part === "string")) return <p>This saved stage reference is unavailable. Return to its run.</p>;
    const [runId, nodeId] = reference as [string, string];
    const evidence = automation.runEvidence.find((run) => run.summary.run_id === runId);
    const node = (evidence?.detail?.blueprint ?? automation.blueprint)?.nodes.find((item) => item.id === nodeId);
    const stage = lanes.find((lane) => lane.runId === runId)?.stages.find((item) => item.nodeId === nodeId);
    const events = evidence?.detail?.events.filter((event) => "node" in event && event.node === nodeId) ?? [];
    return <article className="garden-record"><span className="garden-eyebrow">Stage · Run evidence</span><h2>{node?.name ?? nodeId}</h2>{staleNotice}
      <dl className="garden-record-facts"><dt>Run</dt><dd>{runId === "schedule" ? "Saved routine" : runId}</dd><dt>State</dt><dd>{stage?.status ?? "Scheduled"}</dd><dt>Assignment</dt><dd>{stage?.agentId ? agents.find((agent) => agent.session_id === stage.agentId)?.session_name ?? stage.agentId : stage?.temporaryProvider ? `Temporary provider · ${stage.temporaryProvider}` : "Engine stage"}</dd><dt>Node type</dt><dd>{node?.type ?? "Definition unavailable"}</dd></dl>
      <h3>Inputs</h3><pre className="garden-record-text">{JSON.stringify(node?.fields ?? {}, null, 2)}</pre>
      <h3>Outputs and events</h3>{events.length ? events.map((event) => <section key={event.seq}><p>{event.ts} · {event.kind.replace(/_/g, " ")}</p><pre className="garden-record-text">{JSON.stringify(event, null, 2)}</pre></section>) : <p>No run evidence has been recorded for this stage.</p>}
      {automation.blueprintPath && <button onClick={() => onOpenDefinition(automation.blueprintPath!)}>Open automation definition</button>}
      {runId !== "schedule" && onInspectRun && <button onClick={() => onInspectRun(automation.blueprintId, runId)}>Inspect run evidence</button>}
    </article>;
  }
  return <section className="garden-automation-interior" aria-label="Automation composition"><div className="garden-interior-heading"><div><span className="garden-eyebrow">{automation.projectionKind} · {automation.runStatus.replace(/_/g, " ")}</span><h2>{automation.label}</h2><p>{automation.runs.length} recent runs · {automation.agentIds.length} assigned agents</p></div>{automation.blueprintPath && <button onClick={() => onOpenDefinition(automation.blueprintPath!)}>Open automation definition</button>}</div>
    {staleNotice}
    {automation.schedule && <p>Schedule: {automation.schedule.is_paused ? "Paused" : automation.schedule.schedule.active ? "Enabled" : "Inactive"} · {automation.schedule.name}{automation.schedule.next_run_epoch_ms ? ` · Next ${new Date(automation.schedule.next_run_epoch_ms).toLocaleString()}` : ""}</p>}
    {automation.schedule && onManageSchedule && <button className="garden-agent-interior-action" onClick={onManageSchedule}>Manage schedules in Monitor</button>}
    {lanes.map((lane) => <section className="garden-run-lane" key={lane.runId} aria-label={`Run ${lane.runId}`}><h3>{lane.runId === "schedule" ? "Saved execution path" : lane.runId}</h3>
      <div className="garden-run-stages">{lane.stages.map((stage, index) => {
        const node = (automation.runEvidence.find((run) => run.summary.run_id === lane.runId)?.detail?.blueprint ?? automation.blueprint)?.nodes.find((item) => item.id === stage.nodeId);
        const ref: GardenEntityRef = { kind: "stage", id: JSON.stringify([lane.runId, stage.nodeId]) };
        return <button className="garden-organelle garden-stage-object" data-garden-ref={`stage:${ref.id}`} key={`${stage.nodeId}:${index}`} aria-pressed={selectedKey === `stage:${ref.id}`} onClick={() => onSelect(ref)} onDoubleClick={() => onEnter(ref)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); onEnter(ref); } }}>
          <i className="garden-stage-node" aria-hidden="true" style={{ borderColor: automationRunStatusColor(stage.status ?? "none") }}>{index + 1}</i><strong>{node?.name ?? stage.nodeId}</strong><span>{stage.status ?? "Scheduled"}</span><span className="garden-stage-owner">{stage.agentId ? agents.find((agent) => agent.session_id === stage.agentId)?.session_name ?? stage.agentId : stage.temporaryProvider ? `Temporary · ${stage.temporaryProvider}` : stage.role ? `Unresolved role · ${stage.role}` : "Engine"}</span>
        </button>;
      })}</div>
      {lane.stages.length === 0 && <p>Stage definition unavailable for this run.</p>}
      {lane.runId !== "schedule" && onInspectRun && <button className="garden-agent-interior-action" onClick={() => onInspectRun(automation.blueprintId, lane.runId)}>Inspect run evidence</button>}
    </section>)}
  </section>;
}
