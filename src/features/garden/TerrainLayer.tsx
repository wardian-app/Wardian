import React, { useMemo } from "react";
import { Group, Rect, Text } from "react-konva";
import type Konva from "konva";

import type { TerrainCell, TerrainDistrict } from "./terrain";
import { changeAlpha, type TerrainPaint } from "./terrainPaint";
import type { GardenTheme } from "./useGardenTheme";

interface TerrainLayerProps {
  cells: readonly TerrainCell[];
  districts: ReadonlyMap<string, TerrainDistrict>;
  /** Live world-to-screen factor, for the label legibility gate only. */
  scale: number;
  theme: GardenTheme;
  /** Change tint per path, including ancestors. Empty when nothing is loaded. */
  paint?: ReadonlyMap<string, TerrainPaint>;
  /** Currently selected cell path, if the selection is a piece of ground. */
  selectedPath?: string | null;
  /**
   * Ground written to by the selected agent.
   *
   * The inverse of the skill crown's reverse index, and the reason to build any
   * of this: selecting an agent lights up every patch of disk it has touched,
   * across every district. No list view can show that.
   */
  highlightedPaths?: ReadonlySet<string>;
  onSelectPath?: (path: string) => void;
  onOpenPath?: (path: string) => void;
}

/** Smallest cell, in screen pixels, that gets a name written on it. */
const LABEL_MIN_WIDTH_PX = 100;
const LABEL_MIN_HEIGHT_PX = 36;

/**
 * Ground opacity by depth.
 *
 * The ground is context, not content: units and their status must stay the
 * brightest thing on the map. Deeper cells sit slightly stronger than their
 * parents so nesting reads without borders doing all the work.
 */
const DEPTH_OPACITY = [0.58, 0.7, 0.8, 0.88];

function opacityForDepth(depth: number): number {
  return DEPTH_OPACITY[Math.min(depth, DEPTH_OPACITY.length - 1)];
}

/**
 * The ground beneath the districts.
 *
 * Rendered as its own group *below* the units, and non-interactive in this
 * slice: `listening={false}` keeps two thousand rectangles out of Konva's
 * hit-testing graph entirely, which is what makes drawing them affordable.
 *
 * Cells never move in response to zoom. Zooming changes which levels are drawn
 * — see `terrainFrontier.ts` — and a cell drawn at two zoom levels occupies the
 * same world rect at both, because its rect is a function of its parent's rect
 * and its siblings and of nothing else.
 */
export const TerrainLayer: React.FC<TerrainLayerProps> = React.memo(
  ({
    cells,
    districts,
    scale,
    theme,
    paint,
    selectedPath,
    highlightedPaths,
    onSelectPath,
    onOpenPath,
  }) => {
    // Grouped per district so each can be clipped to its own territory. The
    // ground is the bounding square of the district's disc, so without the clip
    // a populous district would paint into the gap its neighbours rely on.
    const byDistrict = useMemo(() => {
      const grouped = new Map<string, TerrainCell[]>();
      for (const cell of cells) {
        const existing = grouped.get(cell.districtId);
        if (existing) existing.push(cell);
        else grouped.set(cell.districtId, [cell]);
      }
      return grouped;
    }, [cells]);

    return (
      <>
        {[...byDistrict.entries()].map(([districtId, districtCells]) => {
          const district = districts.get(districtId);
          if (!district) return null;
          // The same radius `buildTerrain` measured the ground square against,
          // so the clip is exactly the territory and never a cell wider than it.
          const radius = district.radius;
          if (radius <= 0) return null;
          return (
            <Group
              key={districtId}
              clipFunc={(context: Konva.Context) => {
                context.arc(district.origin.x, district.origin.y, radius, 0, Math.PI * 2, false);
              }}
            >
              {districtCells.map((cell) => (
                <TerrainCellShape
                  key={cell.path}
                  cell={cell}
                  scale={scale}
                  theme={theme}
                  paint={paint?.get(cell.path)}
                  selected={selectedPath === cell.path}
                  highlighted={highlightedPaths?.has(cell.path) ?? false}
                  onSelectPath={onSelectPath}
                  onOpenPath={onOpenPath}
                />
              ))}
            </Group>
          );
        })}
      </>
    );
  },
);
TerrainLayer.displayName = "TerrainLayer";

const TerrainCellShape: React.FC<{
  cell: TerrainCell;
  scale: number;
  theme: GardenTheme;
  paint?: TerrainPaint;
  selected: boolean;
  highlighted: boolean;
  onSelectPath?: (path: string) => void;
  onOpenPath?: (path: string) => void;
}> = ({ cell, scale, theme, paint, selected, highlighted, onSelectPath, onOpenPath }) => {
  const { rect } = cell;
  const fill = cell.depth === 0 ? theme.ground : cell.isDir ? theme.groundDir : theme.groundFile;
  // Crisp material edges remain screen-sized while the authored footprint is fixed.
  const cornerRadius = Math.min(Math.min(rect.width, rect.height) * .08, (cell.isDir ? 8 : 3) / scale);
  const showLabel =
    rect.width * scale >= LABEL_MIN_WIDTH_PX && rect.height * scale >= LABEL_MIN_HEIGHT_PX;

  return (
    <>
      <Rect
        x={rect.x}
        y={rect.y}
        width={rect.width}
        height={rect.height}
        fill={fill}
        opacity={opacityForDepth(cell.depth)}
        stroke={theme.groundBorder}
        strokeWidth={(cell.depth === 0 ? 1.5 : 1) / scale}
        cornerRadius={cornerRadius}
        // Konva's perfect-draw pass allocates an offscreen canvas per shape to
        // composite fill and stroke correctly at partial opacity. At two
        // thousand ground cells that is the dominant cost. Keep the material
        // boundary to a screen-sized hairline instead of introducing buffers.
        perfectDrawEnabled={false}
        // Only the base rect listens. The tint, the highlight, and the label sit
        // on top of it, and a hit on any of them is a hit on this cell — so they
        // stay out of the hit graph rather than each adding a shape to it.
        onMouseEnter={onSelectPath || onOpenPath ? (event) => {
          event.target.getStage()?.container().style.setProperty("cursor", "pointer");
        } : undefined}
        onMouseLeave={onSelectPath || onOpenPath ? (event) => {
          event.target.getStage()?.container().style.setProperty("cursor", "default");
        } : undefined}
        onClick={onSelectPath ? () => onSelectPath(cell.path) : undefined}
        onTap={onSelectPath ? () => onSelectPath(cell.path) : undefined}
        onDblClick={onOpenPath ? () => onOpenPath(cell.path) : undefined}
        onDblTap={onOpenPath ? () => onOpenPath(cell.path) : undefined}
      />
      {paint && (
        // A separate tint rect rather than a blended fill, so the ground keeps
        // reading as ground and the change reads as something laid over it.
        // `attributed` strokes solid and `inferred` dashed: the change-review
        // spec makes that discriminant load-bearing, and a visual difference is
        // read where a badge in a tooltip is not.
        <Rect
          x={rect.x}
          y={rect.y}
          width={rect.width}
          height={rect.height}
          fill={theme.change[paint.kind]}
          // A folder whose children are drawn has already said this in more
          // detail; painting both at full strength would stack two washes and
          // put depth in the channel that carries churn.
          opacity={changeAlpha(paint, cell.isDir && !cell.truncated)}
          stroke={theme.change[paint.kind]}
          strokeWidth={cell.depth === 0 ? 1.5 : 0.75}
          dash={paint.evidence === "inferred" ? [4, 3] : undefined}
          cornerRadius={cornerRadius}
          perfectDrawEnabled={false}
          listening={false}
        />
      )}
      {(selected || highlighted) && (
        <Rect
          x={rect.x}
          y={rect.y}
          width={rect.width}
          height={rect.height}
          stroke={theme.selection}
          strokeWidth={selected ? 2 : 1.25}
          // Highlight is an outline rather than a wash: the fill is already
          // carrying change, and two meanings in one channel is one too many.
          opacity={selected ? 1 : 0.7}
          cornerRadius={cornerRadius}
          perfectDrawEnabled={false}
          listening={false}
        />
      )}
      {showLabel && (
        <Text
          x={rect.x + 12 / scale}
          y={rect.y + 12 / scale}
          width={Math.max(0, rect.width - 24 / scale)}
          text={paint ? `${cell.name} · ${paint.count} changed · ${paint.agentIds.length} agents` : cell.name}
          fontFamily={theme.font}
          fontSize={12 / scale}
          fill={theme.label}
          opacity={0.95}
          ellipsis
          wrap="none"
          listening={false}
          perfectDrawEnabled={false}
        />
      )}
    </>
  );
};
