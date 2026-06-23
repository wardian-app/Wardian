import React from "react";
import { Circle, Group, Text } from "react-konva";
import type { GardenAgentUnit } from "./garden.types";
import { isActiveAgentStatus } from "./gardenStatus";
import { resolveCssVar } from "./resolveColor";
import { useGardenPulse } from "./useGardenPulse";

interface AgentUnitProps {
  unit: GardenAgentUnit;
  selected: boolean;
  onSelect: (id: string) => void;
  onOpen: (id: string) => void;
  onDragMove: (x: number, y: number) => void;
}

export const AgentUnit: React.FC<AgentUnitProps> = ({ unit, selected, onSelect, onOpen, onDragMove }) => {
  const fill = resolveCssVar(unit.color);
  const pulse = useGardenPulse(isActiveAgentStatus(unit.status));

  return (
    <Group
      x={unit.position.x}
      y={unit.position.y}
      draggable
      onClick={() => onSelect(unit.ref.id)}
      onTap={() => onSelect(unit.ref.id)}
      onDblClick={() => onOpen(unit.ref.id)}
      onDblTap={() => onOpen(unit.ref.id)}
      onDragMove={(e) => onDragMove(e.target.x(), e.target.y())}
    >
      <Circle radius={18 * pulse} fill={fill} opacity={0.18} />
      <Circle radius={11} fill={fill} stroke={selected ? "#ffffff" : "transparent"} strokeWidth={2} />
      <Text text={unit.label} fontSize={11} fill="#cbd5e1" y={24} width={120} offsetX={60} align="center" />
    </Group>
  );
};
