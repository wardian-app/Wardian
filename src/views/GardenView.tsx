import React, { useMemo } from "react";
import type { AgentConfig, AgentTelemetry } from "../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../layout/watchlist/types";
import { buildAgentGraph, type GraphRelationshipReason } from "../features/graph/graphProjection";
import { buildGardenAgentUnits, buildGardenWorkflowUnits } from "../features/garden/gardenProjection";
import { GardenCanvas } from "../features/garden/GardenCanvas";
import { useGardenWorkflows } from "../features/garden/useGardenWorkflows";
import { useGardenStore } from "../store/useGardenStore";

const ALL_REASONS: Set<GraphRelationshipReason> = new Set([
  "same_team",
  "shared_workspace",
  "same_worktree",
]);

interface GardenViewProps {
  filteredAgents: AgentConfig[];
  telemetry: Record<string, AgentTelemetry>;
  teams: AgentTeam[];
  activeList: Watchlist | null;
  interactions: AgentInteractions;
  selectedAgentIds: Set<string>;
  offAgentIds: Set<string>;
  onSelectionChange: (ids: Set<string>) => void;
  onOpenAgentInGrid: (agentId: string) => void;
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
  onOpenAgentInGrid,
}) => {
  const positions = useGardenStore((s) => s.positions);
  const setPosition = useGardenStore((s) => s.setPosition);
  const workflowInputs = useGardenWorkflows();

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

  const selectedId = selectedAgentIds.size === 1 ? [...selectedAgentIds][0] : null;

  return (
    <div className="flex-1 flex flex-col min-h-0">
      <GardenCanvas
        agentUnits={agentUnits}
        workflowUnits={workflowUnits}
        selectedId={selectedId}
        onSelect={(id) => onSelectionChange(new Set([id]))}
        onOpenAgent={onOpenAgentInGrid}
        onMoveUnit={(key, x, y) => setPosition(key, { x, y })}
      />
    </div>
  );
};
