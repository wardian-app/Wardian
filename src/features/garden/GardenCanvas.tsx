import React, { useEffect, useRef, useState } from "react";
import { Layer, Stage } from "react-konva";
import type Konva from "konva";
import { AgentUnit } from "./AgentUnit";
import { WorkflowUnit } from "./WorkflowUnit";
import type { GardenAgentUnit, GardenEntityRef, GardenWorkflowUnit } from "./garden.types";
import { unitKey } from "./garden.types";

interface GardenCanvasProps {
  agentUnits: GardenAgentUnit[];
  workflowUnits: GardenWorkflowUnit[];
  selectedKey: string | null;
  onSelect: (ref: GardenEntityRef) => void;
  onOpenAgent: (id: string) => void;
  onMoveUnit: (key: string, x: number, y: number) => void;
}

const MIN_SCALE = 0.4;
const MAX_SCALE = 2.5;
const ZOOM_STEP = 1.05;

export const GardenCanvas: React.FC<GardenCanvasProps> = ({
  agentUnits,
  workflowUnits,
  selectedKey,
  onSelect,
  onOpenAgent,
  onMoveUnit,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [scale, setScale] = useState(1);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const update = () => setSize({ width: el.clientWidth, height: el.clientHeight });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const handleWheel = (e: Konva.KonvaEventObject<WheelEvent>) => {
    e.evt.preventDefault();
    setScale((prev) => {
      const next = e.evt.deltaY < 0 ? prev * ZOOM_STEP : prev / ZOOM_STEP;
      return Math.min(MAX_SCALE, Math.max(MIN_SCALE, next));
    });
  };

  return (
    <div ref={containerRef} className="flex-1 min-h-0 garden-canvas">
      <Stage
        width={size.width}
        height={size.height}
        draggable
        scaleX={scale}
        scaleY={scale}
        onWheel={handleWheel}
      >
        <Layer>
          {workflowUnits.map((unit) => (
            <WorkflowUnit
              key={unitKey(unit.ref)}
              unit={unit}
              selected={selectedKey === unitKey(unit.ref)}
              onSelect={() => onSelect(unit.ref)}
              onDragMove={(x, y) => onMoveUnit(unitKey(unit.ref), x, y)}
            />
          ))}
          {agentUnits.map((unit) => (
            <AgentUnit
              key={unitKey(unit.ref)}
              unit={unit}
              selected={selectedKey === unitKey(unit.ref)}
              onSelect={() => onSelect(unit.ref)}
              onOpen={onOpenAgent}
              onDragMove={(x, y) => onMoveUnit(unitKey(unit.ref), x, y)}
            />
          ))}
        </Layer>
      </Stage>
    </div>
  );
};
