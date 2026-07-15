import React, { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PanelRightOpen, Plus, RotateCcw, Waypoints, X } from "lucide-react";
import type { AgentConfig, AgentTelemetry, CloneMode, TopologySnapshot, PairActivityEntry } from "../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../layout/watchlist/types";
import { AgentContextMenu } from "../components/AgentContextMenu";
import { GraphCanvas } from "../features/graph/GraphCanvas";
import { isUserFacingProviderName, providerDisplayName } from "../features/agents/providerOptions";
import type { GraphSurfaceState } from "../features/workbench/surfaces/coreSurfaceMetadata";

function formatProviderName(provider: string | null | undefined): string {
  if (!provider) return "unknown";
  return isUserFacingProviderName(provider) ? providerDisplayName(provider) : provider;
}
import {
  buildAgentGraph,
  RELATIONSHIP_REASON_LABELS,
  type GraphRelationshipReason,
} from "../features/graph/graphProjection";

type MaybePromise = void | Promise<void>;

export interface GraphViewProps {
  visibility?: "visible" | "hidden";
  rendererActive?: boolean;
  initialSurfaceState?: GraphSurfaceState;
  onSurfaceStateChange?: (state: GraphSurfaceState) => void;
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
  onOpenAgent?: (agentId: string) => void;
  /** @deprecated Legacy flag-off adapter. Workbench surfaces use onOpenAgent. */
  onOpenAgentInGrid?: (agentId: string) => void;
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
  const openAgent = props.onOpenAgent ?? props.onOpenAgentInGrid;
  const initialSurfaceState = props.initialSurfaceState;
  const [enabledReasons, setEnabledReasons] = useState<Set<GraphRelationshipReason>>(
    () => new Set(initialSurfaceState?.enabled_reasons ?? []),
  );
  const [topology, setTopology] = useState<TopologySnapshot | null>(null);
  const [pairActivity, setPairActivity] = useState<PairActivityEntry[]>([]);
  const [inspectedAgentId, setInspectedAgentId] = useState<string | null>(
    initialSurfaceState?.inspected_agent_id ?? Array.from(props.selectedAgentIds)[0] ?? null,
  );
  const [inspectorOpen, setInspectorOpen] = useState(initialSurfaceState?.inspector_open ?? true);
  const [resetSignal, setResetSignal] = useState(0);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(
    initialSurfaceState?.selected_edge_id ?? null,
  );
  const [contextMenu, setContextMenu] = useState<{ agentId: string; agentIds: string[]; x: number; y: number } | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerSearch, setPickerSearch] = useState(initialSurfaceState?.picker_search ?? "");
  const onSurfaceStateChangeRef = useRef(props.onSurfaceStateChange);
  onSurfaceStateChangeRef.current = props.onSurfaceStateChange;
  // Layout freeze: node positions are captured once topology data is loaded
  // and reused across edge edits, so drawing or deleting edges never moves
  // nodes. A re-layout (button or node-set change) clears the freeze.
  const frozenLayoutRef = useRef<{ nodeKey: string; positions: Map<string, { x: number; y: number }> } | null>(null);
  const [layoutNonce, setLayoutNonce] = useState(0);

  useEffect(() => {
    onSurfaceStateChangeRef.current?.({
      enabled_reasons: ALL_REASONS.filter((reason) => enabledReasons.has(reason)),
      inspected_agent_id: inspectedAgentId,
      inspector_open: inspectorOpen,
      selected_edge_id: selectedEdgeId,
      picker_search: pickerSearch,
    });
  }, [enabledReasons, inspectedAgentId, inspectorOpen, pickerSearch, selectedEdgeId]);

  const rerunLayout = () => {
    frozenLayoutRef.current = null;
    setLayoutNonce((value) => value + 1);
  };

  useEffect(() => {
    if (props.visibility === "hidden") return;
    let cancelled = false;
    const refreshTopology = async () => {
      try {
        const t = await invoke<TopologySnapshot>("get_topology");
        if (!cancelled) setTopology(t);
      } catch {
        // Silently ignore errors
      }
    };
    const refreshActivity = async () => {
      try {
        const a = await invoke<PairActivityEntry[]>("get_pair_activity");
        if (!cancelled) setPairActivity(a);
      } catch {
        // Silently ignore errors
      }
    };

    void refreshTopology();
    void refreshActivity();

    // Capture the listen() promises synchronously so cleanup can always
    // reach the unsubscribe functions, even if unmount happens before the
    // registrations resolve.
    const unlistenPromises: Promise<() => void>[] = [
      listen("topology-changed", refreshTopology),
      listen("pair-activity-changed", refreshActivity),
    ];

    return () => {
      cancelled = true;
      unlistenPromises.forEach((p) => {
        p.then((un) => un()).catch(() => {});
      });
    };
  }, [props.visibility]);

  const projection = useMemo(() => buildAgentGraph({
    agents: props.allAgents,
    telemetry: props.telemetry,
    teams: props.teams,
    activeList: props.activeList,
    interactions: props.interactions,
    selectedAgentIds: props.selectedAgentIds,
    enabledReasons,
    offAgentIds: props.offAgentIds,
    topology: topology ?? undefined,
    pairActivity,
    frozenPositions: frozenLayoutRef.current?.positions,
  }), [
    props.allAgents,
    props.telemetry,
    props.teams,
    props.activeList,
    props.interactions,
    props.selectedAgentIds,
    props.offAgentIds,
    enabledReasons,
    topology,
    pairActivity,
    layoutNonce,
  ]);
  const projectionNodeIds = useMemo(
    () => new Set(projection.nodes.map((node) => node.id)),
    [projection.nodes],
  );

  // Capture the layout once topology is loaded, and re-run it when the set
  // of visible nodes changes (agents added/removed or scope switched).
  useEffect(() => {
    if (!topology) return; // don't freeze the pre-topology layout
    const nodeKey = projection.nodes.map((node) => node.id).sort().join("|");
    const frozen = frozenLayoutRef.current;
    if (frozen && frozen.nodeKey !== nodeKey) {
      frozenLayoutRef.current = null;
      setLayoutNonce((value) => value + 1);
      return;
    }
    if (!frozen) {
      frozenLayoutRef.current = {
        nodeKey,
        positions: new Map(projection.nodes.map((node) => [node.id, { x: node.x, y: node.y }])),
      };
    }
  }, [projection.nodes, topology]);

  useEffect(() => {
    const selectedIds = Array.from(props.selectedAgentIds);
    const selectedAgentId = selectedIds.length === 1 ? selectedIds[0] : null;

    if (selectedAgentId && projectionNodeIds.has(selectedAgentId)) {
      setInspectedAgentId((current) => current === selectedAgentId ? current : selectedAgentId);
      return;
    }

    setInspectedAgentId((current) => {
      if (current && projectionNodeIds.has(current)) return current;
      return projection.nodes[0]?.id ?? null;
    });
  }, [projection.nodes, projectionNodeIds, props.selectedAgentIds]);

  useEffect(() => {
    if (props.visibility === "hidden") return;
    const handleKeyDown = async (event: KeyboardEvent) => {
      if (event.key === "Delete" && selectedEdgeId) {
        // Skip if typing in an input or contentEditable element
        if (event.target instanceof HTMLInputElement ||
            event.target instanceof HTMLTextAreaElement ||
            (event.target instanceof HTMLElement && event.target.contentEditable === "true")) {
          return;
        }

        const edge = projection.commEdges.find((e) => e.id === selectedEdgeId);
        if (edge && edge.origin === "manual") {
          try {
            await invoke("remove_topology_edge", { a: edge.source, b: edge.target });
            setSelectedEdgeId(null);
          } catch {
            // Silently ignore errors
          }
        }
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [props.visibility, selectedEdgeId, projection.commEdges]);

  const inspectedAgent = projection.nodes.find((node) => node.id === inspectedAgentId) ?? projection.nodes[0] ?? null;
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
      className={`graph-view ${inspectorOpen ? "graph-view--inspector-open" : "graph-view--inspector-hidden"}`}
      onClick={(event) => {
        event.stopPropagation();
        if (!isInsideContextMenu(event.target)) setContextMenu(null);
      }}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
    >
      <div className="graph-toolbar graph-toolbar--stable-centered">
        <div className="graph-scope graph-toolbar-primary">
          <div className="label-small">Scope</div>
          <div className="graph-scope-label">{projection.scopeLabel}</div>
          <div className="graph-scope-count">{filteredCount} agents visible</div>
        </div>
        <div className="graph-lenses" aria-label="Graph relationship lenses">
          <div className="graph-shift-drag-hint" title="Shift-drag between agents to create connections">
            <Plus size={14} strokeWidth={2.2} />
            <span className="text-muted">Shift-drag to connect</span>
          </div>
          <div className="graph-lens-separator" />
          {ALL_REASONS.map((reason) => (
            <button
              key={reason}
              type="button"
              className={`graph-lens graph-lens--${reason.replace(/_/g, "-")} ${enabledReasons.has(reason) ? "active" : ""}`}
              onClick={() => toggleReason(reason)}
            >
              {RELATIONSHIP_REASON_LABELS[reason]}
            </button>
          ))}
        </div>
        <div className="graph-toolbar-action">
          <button
            type="button"
            className="graph-icon-button"
            aria-label="Re-run layout"
            title="Re-run layout (applies edge changes to node positions)"
            onClick={rerunLayout}
          >
            <Waypoints size={14} strokeWidth={2.2} />
          </button>
          <button
            type="button"
            className="graph-icon-button"
            aria-label="Reset graph view"
            title="Reset graph view"
            onClick={() => setResetSignal((value) => value + 1)}
          >
            <RotateCcw size={14} strokeWidth={2.2} />
          </button>
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

      <div className={`graph-body ${inspectorOpen ? "graph-body--inspector-open" : "graph-body--inspector-hidden"}`}>
        <div className="graph-canvas-shell">
          {props.rendererActive === false ? (
            <div className="graph-empty-state">Graph renderer paused while hidden</div>
          ) : projection.nodes.length === 0 ? (
            <div className="graph-empty-state">No agents in graph scope</div>
          ) : (
            <GraphCanvas
              projection={projection}
              resetSignal={resetSignal}
              onSelectAgent={selectAgent}
              onOpenAgent={(agentId) => openAgent?.(agentId)}
              onContextMenu={openContextMenu}
              selectedEdgeId={selectedEdgeId}
              onSelectEdge={setSelectedEdgeId}
              onConnect={(a, b) => {
                invoke("add_topology_edge", { a, b }).catch((error) => {
                  console.error("add_topology_edge failed", error);
                });
              }}
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
              <p>{inspectedAgent.agent.agent_class} / {formatProviderName(inspectedAgent.agent.provider)}</p>
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
                {renderNeighborsPanel(
                  inspectedAgent.id,
                  projection,
                  props.allAgents,
                  (a, b) => {
                    invoke("add_topology_edge", { a, b }).catch(() => {});
                  },
                  (a, b) => {
                    invoke("remove_topology_edge", { a, b }).catch(() => {});
                  },
                  (a, b) => {
                    invoke("ignore_topology_pair", { a, b }).catch(() => {});
                  },
                  openContextMenu,
                  pickerOpen,
                  setPickerOpen,
                  pickerSearch,
                  setPickerSearch,
                )}
              </div>
              <button type="button" className="graph-open-grid" onClick={() => openAgent?.(inspectedAgent.id)}>
                Open Agent
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

function isInsideContextMenu(target: EventTarget | null) {
  return target instanceof Element && Boolean(target.closest('[data-testid="agent-context-menu"]'));
}

function renderNeighborsPanel(
  agentId: string,
  projection: ReturnType<typeof buildAgentGraph>,
  allAgents: AgentConfig[],
  onAdd: (a: string, b: string) => void,
  onRemove: (a: string, b: string) => void,
  onIgnore: (a: string, b: string) => void,
  onContextMenu: (agentId: string, x: number, y: number) => void,
  pickerOpen: boolean,
  setPickerOpen: (open: boolean) => void,
  pickerSearch: string,
  setPickerSearch: (search: string) => void,
): React.ReactNode {
  const edges = projection.commEdges.filter((e) => e.source === agentId || e.target === agentId);

  return (
    <>
      {edges.length === 0 ? (
        <p>No visible connections.</p>
      ) : (
        <ul className="graph-neighbors-list">
          {edges.map((edge) => {
        const neighborId = edge.source === agentId ? edge.target : edge.source;
        const neighbor = projection.nodes.find((node) => node.id === neighborId);
        const label = neighbor?.label ?? neighborId;

        return (
          <li key={edge.id} className="graph-neighbors-row">
            <div
              className="graph-neighbors-row-info"
              onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                onContextMenu(neighborId, event.clientX, event.clientY);
              }}
            >
              <span className="graph-neighbors-name">{label}</span>
              {edge.origin === "ghost" && (
                <span className="graph-inspector-unmapped">Unmapped</span>
              )}
            </div>
            {edge.origin === "manual" && (
              <button
                type="button"
                className="graph-neighbors-action-btn"
                onClick={() => onRemove(edge.source, edge.target)}
                title="Disconnect"
              >
                ×
              </button>
            )}
            {edge.origin === "ghost" && (
              <div className="graph-neighbors-ghost-actions">
                <button
                  type="button"
                  className="graph-neighbors-action-btn graph-neighbors-action-formalize"
                  onClick={() => onAdd(edge.source, edge.target)}
                  title="Formalize edge"
                >
                  Formalize
                </button>
                <button
                  type="button"
                  className="graph-neighbors-action-btn graph-neighbors-action-ignore"
                  onClick={() => onIgnore(edge.source, edge.target)}
                  title="Ignore this pair"
                >
                  Ignore
                </button>
              </div>
            )}
          </li>
        );
          })}
        </ul>
      )}
      <div className={edges.length === 0 ? "graph-neighbors-add-standalone" : "graph-neighbors-row graph-neighbors-add-row"}>
        {pickerOpen ? (
          <div className="graph-neighbors-picker">
            <input
              type="text"
              className="graph-neighbors-picker-input"
              placeholder="Filter agents…"
              value={pickerSearch}
              onChange={(e) => setPickerSearch(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setPickerOpen(false);
                  setPickerSearch("");
                }
              }}
              autoFocus
            />
            {renderPickerList(
              agentId,
              allAgents,
              projection,
              pickerSearch,
              (selectedId) => {
                onAdd(agentId, selectedId);
                setPickerOpen(false);
                setPickerSearch("");
              },
            )}
          </div>
        ) : (
          <button
            type="button"
            className="graph-neighbors-add-btn"
            onClick={() => setPickerOpen(true)}
          >
            Add connection…
          </button>
        )}
      </div>
    </>
  );
}

function renderPickerList(
  agentId: string,
  allAgents: AgentConfig[],
  projection: ReturnType<typeof buildAgentGraph>,
  search: string,
  onSelect: (agentId: string) => void,
): React.ReactNode {
  // Get IDs of agents already connected via manual edges
  const connectedIds = new Set(
    projection.commEdges
      .filter((e) => e.origin === "manual" && (e.source === agentId || e.target === agentId))
      .map((e) => (e.source === agentId ? e.target : e.source)),
  );

  // Filter agents: visible in projection, not the inspected agent, not already connected
  const available = projection.nodes
    .filter((node) => node.id !== agentId && !connectedIds.has(node.id))
    .map((node) => node.id);

  // Apply search filter
  const searchLower = search.toLowerCase();
  const filtered = available.filter((id) => {
    const agent = allAgents.find((a) => a.session_id === id);
    return agent && agent.session_name.toLowerCase().includes(searchLower);
  });

  if (filtered.length === 0) {
    return <p className="graph-neighbors-picker-empty">No available agents</p>;
  }

  return (
    <ul className="graph-neighbors-picker-list">
      {filtered.map((id) => {
        const agent = allAgents.find((a) => a.session_id === id);
        return (
          <li key={id}>
            <button
              type="button"
              className="graph-neighbors-picker-item"
              onClick={() => onSelect(id)}
            >
              {agent?.session_name}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
