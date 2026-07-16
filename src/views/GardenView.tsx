import React, { useEffect, useMemo, useRef, useState } from "react";
import type { AgentConfig, AgentTelemetry } from "../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../layout/watchlist/types";
import { buildAgentGraph, type GraphRelationshipReason } from "../features/graph/graphProjection";
import { buildGardenAgentUnits, buildGardenWorkflowUnits } from "../features/garden/gardenProjection";
import { GardenCanvas } from "../features/garden/GardenCanvas";
import { unitKey } from "../features/garden/garden.types";
import { useGardenWorkflows } from "../features/garden/useGardenWorkflows";
import { useGardenStore } from "../store/useGardenStore";
import type { GardenSurfaceState } from "../features/workbench/surfaces/coreSurfaceMetadata";

const ALL_REASONS: Set<GraphRelationshipReason> = new Set([
  "same_team",
  "shared_workspace",
  "same_worktree",
]);

export interface GardenViewProps {
  visibility?: "visible" | "hidden";
  rendererActive?: boolean;
  initialSurfaceState?: GardenSurfaceState;
  onSurfaceStateChange?: (state: GardenSurfaceState) => void;
  filteredAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  teams: AgentTeam[];
  activeList: Watchlist | null;
  interactions: AgentInteractions;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  onSelectionChange: (ids: Set<string>) => void;
  onOpenAgent?: (agentId: string) => void;
  /** @deprecated Legacy flag-off adapter. Workbench surfaces use onOpenAgent. */
  onOpenAgentInGrid?: (agentId: string) => void;
}

export const GardenView: React.FC<GardenViewProps> = ({
  filteredAgents,
  telemetry,
  teams,
  activeList,
  interactions,
  selectedAgentIds,
  offAgentIds,
  onSelectionChange,
  onOpenAgent,
  onOpenAgentInGrid,
  visibility = "visible",
  rendererActive = true,
  initialSurfaceState,
  onSurfaceStateChange,
}) => {
  const positions = useGardenStore((s) => s.positions);
  const setPosition = useGardenStore((s) => s.setPosition);
  const resetLayout = useGardenStore((s) => s.reset);
  const workflowInputs = useGardenWorkflows(visibility === "visible");

  // Canvas highlight is keyed by unitKey so agent and workflow ids can't collide,
  // and it stays local so selecting a workflow never leaks into the app's
  // agent-only selection set. Agent clicks still propagate up (for Grid routing).
  const [selectedKey, setSelectedKey] = useState<string | null>(
    initialSurfaceState?.selected_unit_key ?? null,
  );
  const onSurfaceStateChangeRef = useRef(onSurfaceStateChange);
  onSurfaceStateChangeRef.current = onSurfaceStateChange;
  useEffect(() => {
    onSurfaceStateChangeRef.current?.({ selected_unit_key: selectedKey });
  }, [selectedKey]);

  const projection = useMemo(
    () =>
      buildAgentGraph({
        agents: filteredAgents,
        telemetry,
        teams,
        activeList,
        interactions,
        selectedAgentIds,
        enabledReasons: ALL_REASONS,
        offAgentIds,
      }),
    [filteredAgents, telemetry, teams, activeList, interactions, selectedAgentIds, offAgentIds],
  );

  const agentUnits = useMemo(() => buildGardenAgentUnits(projection, positions), [projection, positions]);
  const workflowUnits = useMemo(() => buildGardenWorkflowUnits(workflowInputs, positions), [workflowInputs, positions]);

  // Fall back to an externally-selected single agent (e.g. chosen in Grid) when
  // there is no local Garden selection yet.
  const externalAgentKey =
    selectedAgentIds.size === 1 ? unitKey({ kind: "agent", id: [...selectedAgentIds][0] }) : null;

  return (
    <div className="garden-view flex-1 flex flex-col min-h-0">
      {rendererActive ? <GardenCanvas
        agentUnits={agentUnits}
        workflowUnits={workflowUnits}
        selectedKey={selectedKey ?? externalAgentKey}
        onSelect={(ref) => {
          setSelectedKey(unitKey(ref));
          if (ref.kind === "agent") {
            onSelectionChange(new Set([ref.id]));
          }
        }}
        onOpenAgent={(agentId) => (onOpenAgent ?? onOpenAgentInGrid)?.(agentId)}
        onMoveUnit={(key, x, y) => setPosition(key, { x, y })}
        onResetLayout={() => {
          resetLayout();
          setSelectedKey(null);
        }}
      /> : (
        <div className="flex flex-1 items-center justify-center text-sm text-muted">
          Garden renderer paused while hidden
        </div>
      )}
    </div>
  );
};
