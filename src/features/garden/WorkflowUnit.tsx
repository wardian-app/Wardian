import React from "react";
import { Circle, Group, Rect, Text } from "react-konva";
import type { GardenWorkflowUnit } from "./garden.types";
import { isActiveWorkflowStatus, workflowStatusColor } from "./gardenStatus";
import { resolveCssVar } from "./resolveColor";
import { useGardenPulse } from "./useGardenPulse";

interface WorkflowUnitProps {
  unit: GardenWorkflowUnit;
  selected: boolean;
  onSelect: (id: string) => void;
  onDragMove: (x: number, y: number) => void;
}

const POD_WIDTH = 84;
const POD_HEIGHT = 34;
const MAX_PIPS = 6;

export const WorkflowUnit: React.FC<WorkflowUnitProps> = ({ unit, selected, onSelect, onDragMove }) => {
  const fill = resolveCssVar(workflowStatusColor(unit.runStatus));
  const pulse = useGardenPulse(isActiveWorkflowStatus(unit.runStatus));
  const pips = Math.min(Math.max(unit.nodeCount, 0), MAX_PIPS);

  return (
    <Group
      x={unit.position.x}
      y={unit.position.y}
      draggable
      onClick={() => onSelect(unit.ref.id)}
      onTap={() => onSelect(unit.ref.id)}
      onDragMove={(e) => onDragMove(e.target.x(), e.target.y())}
    >
      <Rect
        width={POD_WIDTH * (unit.runStatus === "none" ? 1 : pulse)}
        height={POD_HEIGHT}
        cornerRadius={10}
        fill={fill}
        opacity={0.22}
      />
      <Rect
        width={POD_WIDTH}
        height={POD_HEIGHT}
        cornerRadius={10}
        stroke={selected ? "#ffffff" : fill}
        strokeWidth={selected ? 2 : 1}
      />
      {Array.from({ length: pips }).map((_, i) => (
        <Circle key={i} x={10 + i * 11} y={POD_HEIGHT - 8} radius={3} fill={fill} />
      ))}
      <Text text={unit.label} fontSize={11} fill="#cbd5e1" y={-16} width={POD_WIDTH} align="center" />
    </Group>
  );
};
