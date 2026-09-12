import React from "react";
import { Circle, Group, Text } from "react-konva";
import type Konva from "konva";
import {
  CROWN_CAP,
  GLYPH_RADIUS,
  crownPositions,
  crownReveal,
  crownLabelLayout,
  crownConvergence,
  type GardenDetail,
  type GardenSkillGlyph,
} from "./skillGlyphs";
import type { GardenTheme } from "./useGardenTheme";

/**
 * Mid lightness on purpose: it lets the monogram be painted in the *background*
 * colour, which reads as white on the light theme and near-black on the dark
 * one. One rule, both themes, no per-theme palette to keep in sync.
 */
function glyphColor(hue: number): string {
  return `hsl(${hue}, 55%, 45%)`;
}

interface SkillCrownProps {
  crown: readonly GardenSkillGlyph[];
  detail: GardenDetail;
  /** Camera scale enables smooth disclosure; omitted retains detail caps. */
  scale?: number;
  /** Eased 0..1 progress from the crown into the cell's Capabilities rows. */
  convergence?: number;
  theme: GardenTheme;
  /** entry_ref of the selected skill, so its glyph rings on every carrier. */
  selectedEntryRef: string | null;
  onSelect: (glyph: GardenSkillGlyph) => void;
  onOpen: (glyph: GardenSkillGlyph) => void;
}

/**
 * The ring of skill glyphs around an agent.
 *
 * Provenance is carried by the shape and sync state by the stroke, so the two
 * stay independently readable: a class-inherited skill is ringed whether or not
 * it happened to fall back to a copy.
 */
export const SkillCrown: React.FC<SkillCrownProps> = ({
  crown,
  detail,
  scale,
  convergence = 0,
  theme,
  selectedEntryRef,
  onSelect,
  onOpen,
}) => {
  const cap = scale === undefined ? CROWN_CAP[detail] : CROWN_CAP.near;
  if (cap <= 0 || crown.length === 0) return null;

  const shown = crown.slice(0, cap);
  const hidden = crown.length - shown.length;
  // The overflow counter occupies a slot of its own so it cannot land on top of
  // the last glyph.
  const positions = crownPositions(shown.length + (hidden > 0 ? 1 : 0));

  // A click on a glyph must not also select the agent underneath it.
  const stop = (event: Konva.KonvaEventObject<MouseEvent | TouchEvent>) => {
    event.cancelBubble = true;
  };

  return (
    <Group listening>
      {shown.map((glyph, index) => {
        const reveal = scale === undefined ? null : crownReveal(scale, index);
        const migration = crownConvergence(positions[index], index, convergence);
        const glyphScale = (reveal?.glyphScale ?? 1) * migration.glyphScale;
        const color = glyphColor(glyph.hue);
        const filled = glyph.provenance !== "global";
        const selected = selectedEntryRef !== null &&
          glyph.entryRef.toLowerCase() === selectedEntryRef.toLowerCase();
        return (
          <Group
            key={glyph.entryRef}
            x={migration.x}
            y={migration.y}
            opacity={reveal?.opacity ?? 1}
            // Opacity zero alone still draws Konva children and their buffers.
            // Keep keyed marks positioned for reverse zoom, but skip invisible paint.
            visible={!reveal || reveal.opacity > 0}
            listening={!reveal || reveal.opacity > 0}
            onMouseEnter={(event) => {
              event.target.getStage()?.container().style.setProperty("cursor", "pointer");
            }}
            onMouseLeave={(event) => {
              event.target.getStage()?.container().style.setProperty("cursor", "default");
            }}
            onClick={(event) => {
              stop(event);
              onSelect(glyph);
            }}
            onTap={(event) => {
              stop(event);
              onSelect(glyph);
            }}
            onDblClick={(event) => {
              stop(event);
              onOpen(glyph);
            }}
            onDblTap={(event) => {
              stop(event);
              onOpen(glyph);
            }}
          >
            <Group scaleX={glyphScale} scaleY={glyphScale}>
              {selected && (
                <Circle radius={GLYPH_RADIUS + 3.5} stroke={theme.selection} strokeWidth={1.5} />
              )}
              {/* An inherited skill gets a second ring: it belongs to the agent's
                  class, so it can change without the agent being touched. */}
              {glyph.provenance === "class" && (
                <Circle radius={GLYPH_RADIUS + 2} stroke={color} strokeWidth={1} opacity={0.7} />
              )}
              <Circle
                radius={GLYPH_RADIUS}
                // A simple disk needs no full-stage fill/stroke compositing buffer
                // while its parent crown fades. Keep this local to the glyph shell.
                perfectDrawEnabled={false}
                fill={filled ? color : theme.labelBackdrop}
                stroke={color}
                strokeWidth={1}
                // A copy is a fork whose edits never sync back to the library.
                dash={glyph.copied ? [2, 2] : undefined}
              />
              {(!reveal || reveal.monogramOpacity > 0) && <Text
                text={glyph.monogram}
                fontSize={8}
                fontStyle="bold"
                fontFamily={theme.font}
                fill={filled ? theme.labelBackdrop : color}
                width={GLYPH_RADIUS * 4}
                offsetX={GLYPH_RADIUS * 2}
                offsetY={4}
                align="center"
                listening={false}
                opacity={reveal?.monogramOpacity ?? 1}
              />}
            </Group>
            {reveal && reveal.labelOpacity > 0 && scale !== undefined && <Text
              {...crownLabelLayout(positions[index], scale, crown.length)}
              text={glyph.label}
              fontFamily={theme.font}
              fill={theme.label}
              opacity={reveal.labelOpacity * migration.labelOpacity}
              wrap="none"
              ellipsis
              listening={false}
              shadowColor={theme.labelBackdrop}
              shadowBlur={2 / scale}
            />}
          </Group>
        );
      })}
      {hidden > 0 && (
        <Text
          text={`+${hidden}`}
          x={positions[positions.length - 1].x}
          y={positions[positions.length - 1].y}
          fontSize={theme.subLabelSize}
          fontFamily={theme.font}
          fill={theme.labelMuted}
          width={GLYPH_RADIUS * 4}
          offsetX={GLYPH_RADIUS * 2}
          offsetY={5}
          align="center"
          listening={false}
          opacity={(scale === undefined ? 1 : crownReveal(scale, CROWN_CAP.near).opacity) *
            crownConvergence(positions[positions.length - 1], CROWN_CAP.near, convergence).labelOpacity}
        />
      )}
    </Group>
  );
};
