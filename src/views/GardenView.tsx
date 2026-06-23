import React, { useMemo, useState } from "react";
import type { AgentConfig, AgentTelemetry } from "../types";
import type { AgentInteractions, AgentTeam, Watchlist } from "../layout/watchlist/types";
import { buildAgentGraph, type GraphRelationshipReason } from "../features/graph/graphProjection";
import { buildGardenAgentUnits, buildGardenWorkflowUnits } from "../features/garden/gardenProjection";
import { GardenCanvas } from "../features/garden/GardenCanvas";
import { unitKey } from "../features/garden/garden.types";
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

  // Canvas highlight is keyed by unitKey so agent and workflow ids can't collide,
  // and it stays local so selecting a workflow never leaks into the app's
  // agent-only selection set. Agent clicks still propagate up (for Grid routing).
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

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
    <div className="flex-1 flex flex-col min-h-0">
      <GardenCanvas
        agentUnits={agentUnits}
        workflowUnits={workflowUnits}
        selectedKey={selectedKey ?? externalAgentKey}
        onSelect={(ref) => {
          setSelectedKey(unitKey(ref));
          if (ref.kind === "agent") {
            onSelectionChange(new Set([ref.id]));
          }
        }}
        onOpenAgent={onOpenAgentInGrid}
        onMoveUnit={(key, x, y) => setPosition(key, { x, y })}
      />
    </div>
  );
};
