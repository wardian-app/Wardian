import type { ComponentProps } from "react";
import type { Group, Text, Circle } from "react-konva";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SkillCrown } from "./SkillCrown";
import { crownPositions, type GardenSkillGlyph } from "./skillGlyphs";
import { GARDEN_THEME_FALLBACK as theme } from "./useGardenTheme";

const captured = vi.hoisted(() => ({ groups: [] as ComponentProps<typeof Group>[], circles: [] as ComponentProps<typeof Circle>[] }));
vi.mock("react-konva", () => ({
  Group: (props: ComponentProps<typeof Group>) => {
    captured.groups.push(props);
    return <div>{props.children}</div>;
  },
  Circle: (props: ComponentProps<typeof Circle>) => { captured.circles.push(props); return null; },
  Text: (props: ComponentProps<typeof Text>) => <span data-rotation={props.rotation} data-opacity={props.opacity} data-width={props.width} data-wrap={props.wrap} data-ellipsis={props.ellipsis}>{props.text}</span>,
}));

const crown: GardenSkillGlyph[] = Array.from({ length: 15 }, (_, index) => ({
  entryRef: `skills/${index}`, label: `Skill ${index} with a long descriptive name`, monogram: `S${index}`,
  hue: index * 20, provenance: index === 0 ? "class" : index === 1 ? "global" : "direct", copied: index === 2,
}));
const props = { crown, detail: "mid" as const, theme, selectedEntryRef: "SKILLS/0", onSelect: vi.fn(), onOpen: vi.fn() };
beforeEach(() => { captured.groups.length = 0; captured.circles.length = 0; vi.clearAllMocks(); });

describe("SkillCrown", () => {
  it("avoids shell compositing buffers and reversibly hides unrevealed keyed marks", () => {
    const view = render(<SkillCrown {...props} scale={0.32} />);
    for (const scale of [0.32, 0.75, 0.32]) {
      captured.groups.length = 0;
      captured.circles.length = 0;
      view.rerender(<SkillCrown {...props} scale={scale} />);
      const glyphs = captured.groups.filter(group => group.onClick);
      expect(glyphs.map(({ x, y }) => ({ x, y }))).toEqual(crownPositions(13).slice(0, 12));
      expect(glyphs.every(group => group.visible === ((group.opacity ?? 1) > 0))).toBe(true);
      expect(glyphs[0].visible).toBe(scale === 0.75);
      const shells = captured.circles.filter(circle => circle.fill && circle.stroke);
      expect(shells).toHaveLength(12);
      // Per-shape configuration; no global Konva or prototype override.
      expect(shells.every(circle => circle.perfectDrawEnabled === false)).toBe(true);
    }
  });

  it("uses horizontal labels only for a total crown size of one and retains the migration fade", () => {
    const view = render(<SkillCrown {...props} crown={crown.slice(0, 1)} scale={3} />);
    const label = screen.getByText(crown[0].label);
    expect(label).toHaveAttribute("data-rotation", "0");
    expect(label).toHaveAttribute("data-width", "24");
    expect(label).toHaveAttribute("data-ellipsis", "true");
    view.rerender(<SkillCrown {...props} crown={crown.slice(0, 1)} scale={3} convergence={0.5} />);
    expect(label).toHaveAttribute("data-opacity", "0.5");
    view.rerender(<SkillCrown {...props} crown={crown.slice(0, 3)} scale={3} />);
    expect(screen.getByText(crown[1].label)).toHaveAttribute("data-rotation", "-90");
  });

  it("preserves detail-only caps, overflow and monograms", () => {
    const view = render(<SkillCrown {...props} />);
    expect(screen.getByText("S5")).toBeInTheDocument();
    expect(screen.queryByText("S6")).not.toBeInTheDocument();
    expect(screen.getByText("+9")).toBeInTheDocument();
    view.rerender(<SkillCrown {...props} detail="far" />);
    expect(view.container).toBeEmptyDOMElement();
    view.rerender(<SkillCrown {...props} detail="near" />);
    expect(screen.getByText("S11")).toBeInTheDocument();
    expect(screen.getByText("+3")).toBeInTheDocument();
  });

  it("uses the same full near positions across zoom and detail boundaries", () => {
    const view = render(<SkillCrown {...props} scale={0.69999} />);
    for (const scale of [0.69999, 0.70001, 1.29999, 1.30001, 2.6]) {
      captured.groups.length = 0;
      view.rerender(<SkillCrown {...props} scale={scale} detail={scale < 1 ? "far" : "near"} />);
      const glyphs = captured.groups.filter((group) => group.onClick);
      expect(glyphs.map(({ x, y }) => ({ x, y }))).toEqual(crownPositions(13).slice(0, 12));
      expect(glyphs[0].opacity).toBeGreaterThan(0);
      if (scale < 1) {
        expect(glyphs[11].opacity).toBe(0);
        expect(glyphs[11].listening).toBe(false);
        expect(screen.queryByText("S0")).not.toBeInTheDocument();
      }
    }
    expect(screen.getByText(crown[0].label)).toHaveAttribute("data-wrap", "none");
    expect(screen.getByText(crown[0].label)).toHaveAttribute("data-ellipsis", "true");
  });

  it("retains canonical pointer/touch actions and provenance styling", () => {
    render(<SkillCrown {...props} scale={3} />);
    const glyph = captured.groups.find((group) => group.onClick)!;
    const invoke = <Event extends { cancelBubble: boolean },>(handler: (event: Event) => void) => {
      const event = { cancelBubble: false } as Event;
      handler(event);
      expect(event.cancelBubble).toBe(true);
    };
    invoke(glyph.onClick!);
    invoke(glyph.onTap!);
    invoke(glyph.onDblClick!);
    invoke(glyph.onDblTap!);
    expect(props.onSelect).toHaveBeenCalledTimes(2);
    expect(props.onOpen).toHaveBeenCalledTimes(2);
    expect(props.onOpen).toHaveBeenCalledWith(crown[0]);
    expect(captured.circles.some((circle) => circle.stroke === theme.selection)).toBe(true);
    expect(captured.circles.some((circle) => circle.radius === 8.5 && circle.stroke === "hsl(0, 55%, 45%)")).toBe(true);
    expect(captured.circles.some((circle) => circle.fill === theme.labelBackdrop && circle.stroke === "hsl(20, 55%, 45%)")).toBe(true);
    expect(captured.circles.some((circle) => circle.dash?.join() === "2,2")).toBe(true);
  });

  it("moves the same keyed glyphs and scales their complete marks into the cell", () => {
    const view = render(<SkillCrown {...props} scale={10} />);
    const originalMonogram = screen.getByText("S0");
    for (const convergence of [0, 0.5, 1]) {
      captured.groups.length = 0;
      view.rerender(<SkillCrown {...props} scale={10} convergence={convergence} />);
      const glyphs = captured.groups.filter((group) => group.onClick);
      const marks = captured.groups.filter((group) => group.scaleX !== undefined);
      expect(glyphs).toHaveLength(12);
      expect(screen.getByText("S0")).toBe(originalMonogram);
      expect(marks.every((mark) => mark.scaleX === 1 * (1 - convergence) + 0.085 * convergence && mark.scaleY === mark.scaleX)).toBe(true);
      expect(screen.getByText(crown[0].label)).toHaveAttribute("data-opacity", String(1 - convergence));
      if (convergence === 1) {
        expect(glyphs.map(({ x, y }) => ({ x, y }))).toEqual(crown.slice(0, 12).map((_, index) => ({ x: -9.5 + (index % 3) * 2.4, y: -7.4 + Math.floor(index / 3) * 3.25 })));
        expect(screen.getByText("+3")).toHaveAttribute("data-opacity", "0");
        const event = { cancelBubble: false } as Parameters<NonNullable<typeof glyphs[0]["onClick"]>>[0];
        glyphs[0].onClick!(event);
        expect(props.onSelect).toHaveBeenCalledWith(crown[0]);
        expect(event.cancelBubble).toBe(true);
      }
    }
  });
});
