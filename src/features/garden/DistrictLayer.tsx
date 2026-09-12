import { Circle, Group, Text } from "react-konva";
import type { TerrainDistrict } from "./terrain";
import type { GardenTheme } from "./useGardenTheme";
import type { DistrictBand, DistrictPopulation } from "./canvasHierarchy";
import { gardenAgentStatusColor } from "./gardenStatus";
import { resolveCssVar } from "./resolveColor";
import { revealBetween } from "./gardenSpatialZoom";
import { pointInCanvasViewport, type CanvasWorldRect } from "./canvasVisibility";

export function DistrictLayer({ districts, labels, populations, bands, scale, selectedKey, theme, onSelect, onOpen, continuousZoom = false, viewport = null }: {
  viewport?: CanvasWorldRect | null;
  districts: ReadonlyMap<string, TerrainDistrict>;
  labels?: ReadonlyMap<string, string>;
  populations?: ReadonlyMap<string, DistrictPopulation>;
  bands?: ReadonlyMap<string, DistrictBand>;
  scale: number;
  continuousZoom?: boolean;
  selectedKey: string | null;
  theme: GardenTheme;
  onSelect: (id: string) => void;
  onOpen: (id: string) => void;
}) {
  return <>{[...districts].map(([id, district]) => {
    if (!pointInCanvasViewport(district.origin, viewport, district.radius)) return null;
    const population = populations?.get(id);
    const habitat = bands?.get(id) === "habitat";
    const labelPosition = { x: district.origin.x, y: district.origin.y - district.radius };
    const labelVisible = pointInCanvasViewport(labelPosition, viewport, 160 / scale);
    const summaryOpacity = continuousZoom ? 1 - revealBetween(district.radius * 2 * scale, 240, 400) : 1;
    const clusterOpacity = continuousZoom ? 1 - revealBetween(district.radius * 2 * scale, 80, 240) : 1;
    return <Group key={id}>
    <Circle x={district.origin.x} y={district.origin.y} radius={district.radius}
      fill={theme.ground} opacity={0.65} stroke={selectedKey === `district:${id}` ? theme.selection : theme.groundBorder}
      // As with terrain hairlines, avoid a full-stage buffer copy for translucent ground.
      perfectDrawEnabled={false}
      strokeWidth={(selectedKey === `district:${id}` ? 3 : 1.5) / scale}
      onClick={() => onSelect(id)} onTap={() => onSelect(id)} onDblClick={() => onOpen(id)} />
    {labelVisible && <Text x={district.origin.x - district.radius} y={district.origin.y - district.radius}
      width={district.radius * 2} align="center" text={labels?.get(id) ?? (id === "commons" ? "Commons" : id.replace(/^[^:]+:/, ""))}
      fontSize={theme.labelSize / scale} fontFamily={theme.font} fill={theme.label}
      onClick={() => onSelect(id)} onTap={() => onSelect(id)} onDblClick={() => onOpen(id)} />}
    {(habitat || continuousZoom) && population && labelVisible && summaryOpacity > 0 && <Text opacity={summaryOpacity} x={district.origin.x - 150 / scale} y={district.origin.y - district.radius + 20 / scale}
      width={300 / scale} align="center" text={population.summary} fontSize={theme.subLabelSize / scale}
      fontFamily={theme.font} fill={theme.labelMuted}
      onClick={() => onSelect(id)} onTap={() => onSelect(id)} onDblClick={() => onOpen(id)} />}
    {habitat && population?.clustered && clusterOpacity > 0 && pointInCanvasViewport(district.origin, viewport, 28 / scale) && <Group opacity={clusterOpacity} name="district-population" id={id} x={district.origin.x} y={district.origin.y}
      onClick={() => onSelect(id)} onTap={() => onSelect(id)} onDblClick={() => onOpen(id)}>
      <Circle radius={22 / scale} fill={theme.groundFile} stroke={selectedKey === `district:${id}` ? theme.selection : theme.groundBorder}
        perfectDrawEnabled={false}
        strokeWidth={2 / scale} hitStrokeWidth={4 / scale} />
      <Text x={-20 / scale} y={-7 / scale} width={40 / scale} align="center" text={String(population.agentIds.length)}
        fontFamily={theme.font} fontSize={theme.labelSize / scale} fill={theme.label} listening={false} />
      {population.statuses.map((entry, index) => <Circle key={entry.status}
        x={(index - (population.statuses.length - 1) / 2) * 9 / scale} y={15 / scale}
        radius={3 / scale} fill={resolveCssVar(gardenAgentStatusColor(entry.status))} listening={false} />)}
    </Group>}
  </Group>; })}</>;
}
