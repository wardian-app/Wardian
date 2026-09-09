import React from "react";
import { Circle, Group, Text } from "react-konva";
import type { GardenAgentUnit } from "./garden.types";
import { gardenAgentStatusColor, isActiveAgentStatus } from "./gardenStatus";
import { resolveCssVar } from "./resolveColor";
import { SkillCrown } from "./SkillCrown";
import type { GardenDetail, GardenSkillGlyph } from "./skillGlyphs";
import { PULSE_BASE_RADIUS, PULSE_HALO_NAME } from "./useGardenPulse";
import type { GardenTheme } from "./useGardenTheme";
import { agentMonogram } from "./agentMonogram";
import { revealBetween } from "./gardenSpatialZoom";

/** Konva node `name` used to identify agent units during canvas hit-testing. */
export const AGENT_UNIT_NAME = "agent-unit";

interface AgentUnitProps {
  unit: GardenAgentUnit;
  selected: boolean;
  /**
   * True when a skill is selected and this agent carries it.
   *
   * Instancing a skill across its carriers removes the ability to point at
   * *the* place it lives, so the reverse highlight has to answer the same
   * question with a set. Distinct from `selected`, which is a single unit.
   */
  highlighted?: boolean;
  detail: GardenDetail;
  /** Habitat uses a signal; only Workstream permits authored placement. */
  signal?: boolean;
  draggable?: boolean;
  dragging?: boolean;
  scale?: number;
  continuousZoom?: boolean;
  populationOpacity?: number;
  /** Available screen pixels between neighboring name labels. Full name stays in the selection summary. */
  labelWidthPx?: number;
  theme: GardenTheme;
  /** entry_ref of the selected skill, so its glyph rings on every carrier. */
  selectedSkillRef?: string | null;
  onSelect: (id: string) => void;
  onOpen: (id: string) => void;
  onSelectSkill: (glyph: GardenSkillGlyph) => void;
  onOpenSkill: (glyph: GardenSkillGlyph) => void;
  /** Fired once, when the drag finishes. See the note on the handler. */
  onDragEnd: (key: string, x: number, y: number) => void;
  onDragStart?: (id: string) => void;
}

const AgentUnitImpl: React.FC<AgentUnitProps> = ({
  unit,
  selected,
  highlighted = false,
  detail,
  signal = false,
  draggable = true,
  dragging = false,
  scale = 1,
  continuousZoom = false,
  populationOpacity = 1,
  labelWidthPx = 140,
  theme,
  selectedSkillRef = null,
  onSelect,
  onOpen,
  onSelectSkill,
  onOpenSkill,
  onDragEnd,
  onDragStart,
}) => {
  const fill = resolveCssVar(gardenAgentStatusColor(unit.status));
  const active = isActiveAgentStatus(unit.status);
  const exteriorOpacity = continuousZoom && !dragging ? 1 - revealBetween(scale * 32, 70, 150) : 1;
  const bodyOpacity = continuousZoom ? revealBetween(scale * 32, 12, 28) : signal ? 0 : 1;
  const showBody = continuousZoom || !signal;
  const crownOpacity = continuousZoom ? 1 - revealBetween(scale, 6, 12) : 1;

  return (
    <Group
      opacity={populationOpacity}
      visible={populationOpacity > 0 || dragging}
      listening={populationOpacity > .1}
      x={unit.position.x}
      y={unit.position.y}
      // id + name let the canvas resolve which agent was right-clicked via
      // Konva hit-testing (see GardenCanvas), without per-node DOM handlers.
      id={unit.ref.id}
      name={AGENT_UNIT_NAME}
      draggable={draggable}
      onMouseEnter={(event) => {
        event.target.getStage()?.container().style.setProperty("cursor", "pointer");
      }}
      onMouseLeave={(event) => {
        event.target.getStage()?.container().style.setProperty("cursor", "default");
      }}
      onClick={() => onSelect(unit.ref.id)}
      onTap={() => onSelect(unit.ref.id)}
      onDblClick={() => onOpen(unit.ref.id)}
      onDblTap={() => onOpen(unit.ref.id)}
      onDragStart={(event) => {
        event.target.getStage()?.container().style.setProperty("cursor", "grabbing");
        onDragStart?.(unit.ref.id);
      }}
      // Committed on drag *end*, not on every move. A move-by-move commit
      // pinned the unit and re-ran the whole layout on each mouse event, so
      // the map re-solved and slid under the cursor while the user dragged.
      // Konva moves the node locally in the meantime, which is all the
      // feedback a drag needs.
      onDragEnd={(e) => {
        e.target.getStage()?.container().style.setProperty("cursor", "pointer");
        onDragEnd(unit.ref.id, e.target.x(), e.target.y());
      }}
    >
      {/* Named so the canvas' single pulse animation can find it. The radius is
          mutated on the Konva node rather than through React, so a busy agent
          does not re-render its whole crown once per frame. */}
      {showBody && <Circle
        name={active && exteriorOpacity > 0 ? PULSE_HALO_NAME : undefined}
        radius={PULSE_BASE_RADIUS}
        fill={fill}
        opacity={0.18 * exteriorOpacity * bodyOpacity}
        listening={false}
      />}
      {/* Drawn outside the status halo so a carrier stands out without
          overriding the status colour, which stays the primary channel. */}
      {highlighted && (
        <Circle
          radius={21}
          stroke={theme.selection}
          strokeWidth={2}
          opacity={0.85}
          dash={[3, 3]}
          listening={false}
        />
      )}
      <Circle
        // The fading shell has a decorative hairline; a stage-sized perfect-draw
        // buffer per inhabitant costs far more than its subtle edge overlap.
        perfectDrawEnabled={false}
        opacity={exteriorOpacity * (continuousZoom ? bodyOpacity : 1)}
        radius={continuousZoom ? 4 / scale + (16 - 4 / scale) * bodyOpacity : signal ? 4 / scale : 16}
        hitStrokeWidth={signal ? 16 / scale : 12}
        fill={continuousZoom ? theme.groundFile : signal ? fill : theme.groundFile}
        stroke={selected ? theme.selection : signal ? "transparent" : theme.groundBorder}
        strokeWidth={selected ? 3 / scale : 1}
      />
      {continuousZoom && <Circle radius={4 / scale} opacity={1 - bodyOpacity} fill={fill} hitStrokeWidth={16 / scale} />}
      {showBody && <Circle opacity={exteriorOpacity * bodyOpacity} radius={4} x={10} y={10} fill={fill} listening={false} />}
      {showBody && <Text opacity={exteriorOpacity * bodyOpacity} text={agentMonogram(unit.label)} x={-14} y={continuousZoom ? -Math.min(22, 13 * scale) / (2 * scale) : -7} width={28}
        align="center" fontSize={continuousZoom ? Math.min(22, 13 * scale) / scale : 13} fontFamily={theme.font} fill={theme.label} listening={false} />}
      {showBody && crownOpacity > 0 && <Group opacity={crownOpacity}><SkillCrown
        crown={continuousZoom ? unit.crown : unit.crown.slice(0, 3)}
        scale={continuousZoom ? scale : undefined}
        convergence={continuousZoom ? revealBetween(scale, 4, 10) : 0}
        detail={detail}
        theme={theme}
        selectedEntryRef={selectedSkillRef}
        onSelect={onSelectSkill}
        onOpen={onOpenSkill}
      /></Group>}
      {!continuousZoom && !signal && unit.crown.length > 3 && <Text text={`+${unit.crown.length - 3}`} x={24} y={-28}
        fontSize={theme.subLabelSize / scale} fontFamily={theme.font} fill={theme.labelMuted} listening={false} />}
      {showBody && labelWidthPx >= 40 && <Text
        opacity={(continuousZoom && !dragging ? 1 - revealBetween(scale * 32, 180, 300) : exteriorOpacity) * bodyOpacity}
        text={unit.label}
        fontSize={Math.min(14, Math.max(12, theme.labelSize)) / scale}
        fontFamily={theme.font}
        fill={theme.label}
        y={24}
        width={labelWidthPx / scale}
        offsetX={labelWidthPx / (2 * scale)}
        height={18 / scale}
        wrap="none"
        ellipsis
        align="center"
        listening
        // Halo in the background colour: labels sit over status halos and
        // neighbouring units, and a map label has to stay readable wherever it
        // lands. Cheaper and less cluttered than a backdrop rectangle.
        shadowColor={theme.labelBackdrop}
        shadowBlur={4 / scale}
        shadowOpacity={1}
      />}
    </Group>
  );
};

/**
 * Compared field by field rather than by identity.
 *
 * `buildAgentUnits` rebuilds every unit object on each telemetry tick so status
 * and colour stay live, which means prop identity always changes and the
 * default shallow comparison would never skip anything. The fields below are
 * the complete set this component draws from; `position` and `crown` are
 * compared by reference because both come straight out of the layout result and
 * only change when the layout does.
 */
function propsEqual(previous: AgentUnitProps, next: AgentUnitProps): boolean {
  return (
    previous.unit.ref.id === next.unit.ref.id &&
    previous.unit.label === next.unit.label &&
    previous.unit.status === next.unit.status &&
    previous.unit.position === next.unit.position &&
    previous.unit.crown === next.unit.crown &&
    previous.selected === next.selected &&
    previous.highlighted === next.highlighted &&
    previous.detail === next.detail &&
    previous.signal === next.signal &&
    previous.draggable === next.draggable &&
    previous.dragging === next.dragging &&
    previous.scale === next.scale &&
    previous.continuousZoom === next.continuousZoom &&
    previous.populationOpacity === next.populationOpacity &&
    previous.labelWidthPx === next.labelWidthPx &&
    previous.theme === next.theme &&
    previous.selectedSkillRef === next.selectedSkillRef &&
    previous.onSelect === next.onSelect &&
    previous.onOpen === next.onOpen &&
    previous.onSelectSkill === next.onSelectSkill &&
    previous.onOpenSkill === next.onOpenSkill &&
    previous.onDragEnd === next.onDragEnd &&
    previous.onDragStart === next.onDragStart
  );
}

export const AgentUnit = React.memo(AgentUnitImpl, propsEqual);
