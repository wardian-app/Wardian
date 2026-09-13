import { Arrow, Circle, Group, Rect, Text } from "react-konva";
import type { SituatedRoute } from "./canvasHierarchy";
import type { GardenEntityRef } from "./garden.types";
import type { GardenTheme } from "./useGardenTheme";
import { revealBetween } from "./gardenSpatialZoom";
import { pointInCanvasViewport, rectInCanvasViewport, type CanvasWorldRect } from "./canvasVisibility";

/** Derived attachments and execution paths; deliberately never draggable. */
export function AutomationRoutesLayer({ routes, theme, scale, selectedKey, onSelect, onOpen, mode = "all", viewport = null, continuousZoom = false }: {
  viewport?: CanvasWorldRect | null;
  continuousZoom?: boolean;
  routes: readonly SituatedRoute[];
  theme: GardenTheme;
  scale: number;
  selectedKey: string | null;
  onSelect: (ref: GardenEntityRef) => void;
  onOpen: (ref: GardenEntityRef) => void;
  /** Keep route hits below agents and stage attention above their bodies. */
  mode?: "all" | "routes" | "markers";
}) {
  const attachmentCounts = new Map<string, number>();
  for (const route of routes) {
    const key = route.points.map((point) => `${point.x},${point.y}`).join("→");
    attachmentCounts.set(key, (attachmentCounts.get(key) ?? 0) + 1);
  }
  return <>{routes.map(({ input, points, anchor, presentation }) => {
    const ref: GardenEntityRef = { kind: "automation", id: input.id };
    const selected = selectedKey === `automation:${input.id}`;
    const routeOpacity = selected || !continuousZoom ? 1 : revealBetween(scale, .45, 1.2);
    const attachmentKey = points.map((point) => `${point.x},${point.y}`).join("→");
    const denseAttachment = (attachmentCounts.get(attachmentKey) ?? 0) > 8;
    const labelOpacity = selected || !continuousZoom ? 1
      : revealBetween(scale, denseAttachment ? 3.8 : 1.4, denseAttachment ? 5.2 : 2.6);
    const anchorVisible = pointInCanvasViewport(anchor, viewport, 200 / scale);
    const labelOnLeft = anchor.x < points[0].x;
    const labelWidth = 150 / scale;
    const labelX = anchor.x + (labelOnLeft ? -labelWidth - 10 / scale : 10 / scale);
    const markerRows = new Map<string, number>();
    return <Group key={input.id} onClick={() => onSelect(ref)} onTap={() => onSelect(ref)} onDblClick={() => onOpen(ref)}>
      {mode !== "markers" && <Group name="automation-route" opacity={routeOpacity} visible={routeOpacity > 0} listening={routeOpacity > 0}>
      {/* Invisible connections must not claim hit pixels; local markers remain siblings. */}
      <Arrow points={(points.length === 1 ? [points[0], anchor] : points).flatMap((point) => [point.x, point.y])}
        perfectDrawEnabled={false}
        stroke={selected ? theme.selection : theme.labelMuted} fill={theme.labelMuted}
        strokeWidth={(selected ? 2 : 1) / scale} hitStrokeWidth={24 / scale}
        dash={presentation.paused ? [7 / scale, 4 / scale, 1 / scale, 4 / scale] : !presentation.live ? [5 / scale, 4 / scale] : undefined}
        pointerLength={5 / scale} pointerWidth={5 / scale} />
      {anchorVisible && <Circle perfectDrawEnabled={false} x={anchor.x} y={anchor.y} radius={6 / scale} fill={theme.groundFile} stroke={selected ? theme.selection : theme.labelMuted} />}
      {anchorVisible && labelOpacity > 0 && <Text opacity={labelOpacity} x={labelX} y={anchor.y - 6 / scale} text={presentation.summary}
        width={labelWidth} align={labelOnLeft ? "right" : "left"} wrap="none" ellipsis fontFamily={theme.font} fontSize={12 / scale} fill={theme.label} />}</Group>}
      {mode !== "routes" && presentation.markers.map((marker) => {
        const positionKey = `${marker.position.x}:${marker.position.y}`;
        const labelRow = markerRows.get(positionKey) ?? 0;
        markerRows.set(positionKey, labelRow + 1);
        const labelRect = { x: marker.position.x + 28 / scale, y: marker.position.y + (-24 + labelRow * 16) / scale, width: 200 / scale, height: 16 / scale };
        if (!pointInCanvasViewport(marker.position, viewport, 36 / scale)
          && !(labelOpacity > 0 && rectInCanvasViewport(labelRect, viewport))) return null;
        const color = marker.attention === "failed" ? theme.change.deleted : marker.attention ? theme.change.modified : theme.labelMuted;
        return <Group key={marker.key} name="automation-stage-marker" id={marker.key} x={marker.position.x} y={marker.position.y}>
          {marker.temporary && <Rect name="temporary-provider" x={-10 / scale} y={-10 / scale} width={20 / scale} height={20 / scale}
            perfectDrawEnabled={false}
            cornerRadius={5 / scale} fill={theme.groundFile} stroke={color} strokeWidth={1.5 / scale} dash={[3 / scale, 3 / scale]} />}
          {marker.attention === "awaiting_approval" && <Circle name="stage-attention" radius={18 / scale} stroke={color} strokeWidth={2 / scale} />}
          {marker.attention && <Text x={10 / scale} y={-18 / scale} text={marker.attention === "failed" ? "×" : "!"}
            fontSize={15 / scale} fontFamily={theme.font} fill={color} />}
          {labelOpacity > 0 && <Text opacity={labelOpacity} width={200 / scale} wrap="none" ellipsis x={28 / scale} y={(-24 + labelRow * 16) / scale} text={marker.label}
            fontFamily={theme.font} fontSize={12 / scale} fill={color}
            shadowColor={theme.labelBackdrop} shadowBlur={4 / scale} shadowOpacity={1} />}
        </Group>;
      })}
    </Group>;
  })}</>;
}
