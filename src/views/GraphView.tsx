import React, { useMemo, useState } from "react";
import { PanelRightOpen, X } from "lucide-react";
import type { AgentConfig, AgentTelemetry, CloneMode } from "../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../layout/watchlist/types";
import { AgentContextMenu } from "../components/AgentContextMenu";
import { GraphCanvas } from "../features/graph/GraphCanvas";
import {
  buildAgentGraph,
  type AgentGraphEdge,
  type GraphRelationshipReason,
} from "../features/graph/graphProjection";

type MaybePromise = void | Promise<void>;

interface GraphViewProps {
  filteredAgents: AgentConfig[];
  allAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  terminalTitles: Record<string, string>;
  currentThoughts: Record<string, string>;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  watchlists: Watchlist[];
  activeList: Watchlist | null;
  teams: AgentTeam[];
  interactions: AgentInteractions;
  onSelectionChange: (ids: Set<string>) => void;
  onOpenAgentInGrid: (agentId: string) => void;
  onInitiateRename: (agentId: string) => MaybePromise;
  onQuery: (agentId: string) => MaybePromise;
  onPause: (agentId: string) => MaybePromise;
  onRestart: (agentId: string) => MaybePromise;
  onClear: (agentId: string) => MaybePromise;
  onClone: (agentId: string, mode: CloneMode) => MaybePromise;
  onAddToList: (listId: string, agentId: string) => MaybePromise;
  onRemoveFromList: (listId: string, agentId: string) => MaybePromise;
  onAddAgentsToList: (listId: string, agentIds: string[]) => MaybePromise;
  onRemoveAgentsFromList: (listId: string, agentIds: string[]) => MaybePromise;
  onDelete: (agentId: string) => MaybePromise;
  onDeleteAgents: (agentIds: string[]) => MaybePromise;
  deriveCurrentThought: (
    title: string,
    thought: string,
    metrics: AgentTelemetry | undefined,
    isOff: boolean,
  ) => { thought: string; status: string };
}

const ALL_REASONS: GraphRelationshipReason[] = [
  "same_team",
  "shared_workspace",
  "same_worktree",
];

export const GraphView: React.FC<GraphViewProps> = (props) => {
  const [enabledReasons, setEnabledReasons] = useState<Set<GraphRelationshipReason>>(new Set(ALL_REASONS));
  const [inspectedAgentId, setInspectedAgentId] = useState<string | null>(
    Array.from(props.selectedAgentIds)[0] ?? null,
  );
  const [inspectorOpen, setInspectorOpen] = useState(true);
  const [contextMenu, setContextMenu] = useState<{ agentId: string; agentIds: string[]; x: number; y: number } | null>(null);

  const projection = useMemo(() => buildAgentGraph({
    agents: props.allAgents,
    telemetry: props.telemetry,
    teams: props.teams,
    activeList: props.activeList,
    interactions: props.interactions,
    selectedAgentIds: props.selectedAgentIds,
    enabledReasons,
  }), [
    props.allAgents,
    props.telemetry,
    props.teams,
    props.activeList,
    props.interactions,
    props.selectedAgentIds,
    enabledReasons,
  ]);

  const inspectedAgent = projection.nodes.find((node) => node.id === inspectedAgentId) ?? projection.nodes[0] ?? null;
  const relationshipEdges = inspectedAgent ? relatedEdges(projection.edges, inspectedAgent.id) : [];
  const filteredCount = props.filteredAgents.length;

  const toggleReason = (reason: GraphRelationshipReason) => {
    setEnabledReasons((current) => {
      const next = new Set(current);
      if (next.has(reason)) next.delete(reason);
      else next.add(reason);
      return next;
    });
  };

  const selectAgent = (agentId: string) => {
    setInspectedAgentId(agentId);
    setInspectorOpen(true);
    props.onSelectionChange(new Set([agentId]));
  };

  const openContextMenu = (agentId: string, x: number, y: number) => {
    const selectedIds = Array.from(props.selectedAgentIds);
    const agentIds = props.selectedAgentIds.has(agentId) && selectedIds.length > 1
      ? selectedIds
      : [agentId];

    setInspectedAgentId(agentId);
    if (agentIds.length === 1) {
      props.onSelectionChange(new Set([agentId]));
    }
    setContextMenu({ agentId, agentIds, x, y });
  };

  return (
    <div
      data-testid="graph-view"
      className="graph-view"
      onClick={(event) => {
        event.stopPropagation();
        if (!isInsideContextMenu(event.target)) setContextMenu(null);
      }}
      onContextMenu={(event) => event.stopPropagation()}
    >
      <div className="graph-toolbar">
        <div className="graph-scope">
          <div className="label-small">Scope</div>
          <div className="graph-scope-label">{projection.scopeLabel}</div>
          <div className="graph-scope-count">{filteredCount} agents visible</div>
        </div>
        <div className="graph-lenses" aria-label="Graph relationship lenses">
          {ALL_REASONS.map((reason) => (
            <button
              key={reason}
              type="button"
              className={`graph-lens ${enabledReasons.has(reason) ? "active" : ""}`}
              onClick={() => toggleReason(reason)}
            >
              {reason.replace(/_/g, " ")}
            </button>
          ))}
        </div>
        <div className="graph-toolbar-action">
          {!inspectorOpen && (
            <button
              type="button"
              className="graph-icon-button"
              aria-label="Show inspector"
              title="Show inspector"
              onClick={() => setInspectorOpen(true)}
            >
              <PanelRightOpen size={15} strokeWidth={2} />
            </button>
          )}
        </div>
      </div>

      <div className={`graph-body ${inspectorOpen ? "" : "graph-body--inspector-hidden"}`}>
        <div className="graph-canvas-shell">
          {projection.nodes.length === 0 ? (
            <div className="graph-empty-state">No agents in graph scope</div>
          ) : (
            <GraphCanvas
              projection={projection}
              onSelectAgent={selectAgent}
              onOpenAgent={props.onOpenAgentInGrid}
              onContextMenu={openContextMenu}
            />
          )}
        </div>
        {inspectorOpen && (
          <aside
            className="graph-inspector"
            onContextMenu={(event) => {
              if (!inspectedAgent) return;
              event.preventDefault();
              openContextMenu(inspectedAgent.id, event.clientX, event.clientY);
            }}
          >
            {inspectedAgent ? (
              <>
                <div className="graph-inspector-header">
                  <div className="label-small">Inspector</div>
                  <button
                    type="button"
                    className="graph-icon-button"
                    aria-label="Hide inspector"
                    title="Hide inspector"
                    onClick={() => setInspectorOpen(false)}
                  >
                    <X size={14} strokeWidth={2.2} />
                  </button>
                </div>
              <h2>{inspectedAgent.label}</h2>
              <p>{inspectedAgent.agent.agent_class} / {inspectedAgent.agent.provider ?? "unknown"}</p>
              <p>
                {props.deriveCurrentThought(
                  props.terminalTitles[inspectedAgent.id] ?? "",
                  props.currentThoughts[inspectedAgent.id] ?? "",
                  inspectedAgent.telemetry,
                  props.offAgentIds.has(inspectedAgent.id),
                ).status}
              </p>
              <p className="graph-inspector-path">{inspectedAgent.agent.folder || "No workspace"}</p>
              <dl className="graph-telemetry">
                <div>
                  <dt>CPU</dt>
                  <dd>{inspectedAgent.telemetry?.cpu_usage.toFixed(1) ?? "0.0"}%</dd>
                </div>
                <div>
                  <dt>Memory</dt>
                  <dd>{inspectedAgent.telemetry?.memory_mb.toFixed(0) ?? "0"} MB</dd>
                </div>
                <div>
                  <dt>Queries</dt>
                  <dd>{inspectedAgent.telemetry?.query_count ?? 0}</dd>
                </div>
              </dl>
              <div className="graph-relationships">
                <div className="label-small">Relationships</div>
                {relationshipEdges.length === 0 ? (
                  <p>No visible relationships under the current lenses.</p>
                ) : (
                  <ul>
                    {relationshipEdges.map((edge) => {
                      const neighborId = edge.source === inspectedAgent.id ? edge.target : edge.source;
                      const neighbor = projection.nodes.find((node) => node.id === neighborId);
                      return (
                        <li key={edge.id}>
                          <span>{neighbor?.label ?? neighborId}</span>
                          <small>{edge.reasons.join(", ").replace(/_/g, " ")}</small>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </div>
              <button type="button" className="graph-open-grid" onClick={() => props.onOpenAgentInGrid(inspectedAgent.id)}>
                Open in Grid
              </button>
              </>
            ) : (
              <p>Select an agent node to inspect relationships.</p>
            )}
          </aside>
        )}
      </div>

      {contextMenu && (
        <AgentContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          agentId={contextMenu.agentId}
          agentIds={contextMenu.agentIds}
          offAgentIds={props.offAgentIds}
          watchlists={props.watchlists}
          teams={props.teams}
          onInitiateRename={props.onInitiateRename}
          onQuery={props.onQuery}
          onPause={props.onPause}
          onRestart={props.onRestart}
          onClear={props.onClear}
          onClone={props.onClone}
          onAddToList={props.onAddToList}
          onRemoveFromList={props.onRemoveFromList}
          onAddAgentsToList={props.onAddAgentsToList}
          onRemoveAgentsFromList={props.onRemoveAgentsFromList}
          onDelete={props.onDelete}
          onDeleteAgents={props.onDeleteAgents}
          onClose={() => setContextMenu(null)}
        />
      )}
    </div>
  );
};

function relatedEdges(edges: AgentGraphEdge[], agentId: string) {
  return edges.filter((edge) => edge.source === agentId || edge.target === agentId);
}

function isInsideContextMenu(target: EventTarget | null) {
  return target instanceof Element && Boolean(target.closest('[data-testid="agent-context-menu"]'));
}
