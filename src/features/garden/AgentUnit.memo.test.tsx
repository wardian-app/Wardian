import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

let unitRenders = 0;

// Counting stand-ins. The label `Text` is drawn exactly once per AgentUnit
// render, so it is a faithful render counter for the component under test.
vi.mock("react-konva", () => ({
  Group: ({ children }: any) => <div>{children}</div>,
  Circle: () => <div />,
  Text: (props: { text: string }) => {
    if (props.text === "Alpha") unitRenders += 1;
    return <div />;
  },
}));
vi.mock("./SkillCrown", () => ({ SkillCrown: () => <div /> }));

import { AgentUnit } from "./AgentUnit";
import type { GardenAgentUnit } from "./garden.types";
import type { GardenSkillGlyph } from "./skillGlyphs";
import { GARDEN_THEME_FALLBACK, type GardenTheme } from "./useGardenTheme";

const theme: GardenTheme = {
  ...GARDEN_THEME_FALLBACK,
  font: "sans-serif",
  labelSize: 12,
  subLabelSize: 10,
};

// Both come straight out of the layout result and are compared by reference.
const position = { x: 10, y: 20 };
const crown: GardenSkillGlyph[] = [];

const handlers = {
  onSelect: vi.fn(),
  onOpen: vi.fn(),
  onSelectSkill: vi.fn(),
  onOpenSkill: vi.fn(),
  onDragEnd: vi.fn(),
};

/**
 * A fresh unit object with the same content, mimicking `buildAgentUnits`, which
 * rebuilds every unit on each telemetry tick so status and colour stay live.
 */
function unitAt(status = "Idle"): GardenAgentUnit {
  return {
    ref: { kind: "agent", id: "a1" },
    label: "Alpha",
    status,
    color: "var(--color-wardian-success)",
    position,
    crown,
  };
}

describe("AgentUnit memoization", () => {
  beforeEach(() => {
    unitRenders = 0;
  });

  it("does not re-render when a telemetry tick rebuilds an unchanged unit", () => {
    // The Garden re-renders on every tick. Without a field-by-field comparison
    // the new unit object alone would re-render every agent's whole crown, which
    // is what made a busy map expensive.
    const { rerender } = render(
      <AgentUnit unit={unitAt()} selected={false} detail="near" theme={theme} {...handlers} />,
    );
    expect(unitRenders).toBe(1);

    for (let tick = 0; tick < 5; tick += 1) {
      rerender(
        <AgentUnit unit={unitAt()} selected={false} detail="near" theme={theme} {...handlers} />,
      );
    }
    expect(unitRenders).toBe(1);
  });

  it("re-renders when something it draws actually changes", () => {
    const { rerender } = render(
      <AgentUnit unit={unitAt()} selected={false} detail="near" theme={theme} {...handlers} />,
    );

    rerender(
      <AgentUnit
        unit={unitAt("Processing...")}
        selected={false}
        detail="near"
        theme={theme}
        {...handlers}
      />,
    );
    expect(unitRenders).toBe(2);

    rerender(
      <AgentUnit
        unit={unitAt("Processing...")}
        selected
        detail="near"
        theme={theme}
        {...handlers}
      />,
    );
    expect(unitRenders).toBe(3);

    rerender(
      <AgentUnit
        unit={{ ...unitAt("Processing..."), position: { x: 99, y: 99 } }}
        selected
        detail="near"
        theme={theme}
        {...handlers}
      />,
    );
    expect(unitRenders).toBe(4);
  });

  it("re-renders when the zoom detail level changes", () => {
    // Detail decides how much of the crown is drawn, so it must not be skipped.
    const { rerender } = render(
      <AgentUnit unit={unitAt()} selected={false} detail="mid" theme={theme} {...handlers} />,
    );
    rerender(
      <AgentUnit unit={unitAt()} selected={false} detail="near" theme={theme} {...handlers} />,
    );
    expect(unitRenders).toBe(2);
  });

  it("re-renders when a fresh callback identity arrives, so stale closures cannot stick", () => {
    // The flip side of the optimization: the canvas must hand down stable
    // callbacks, and if it does not, correctness still wins over skipping.
    const { rerender } = render(
      <AgentUnit unit={unitAt()} selected={false} detail="near" theme={theme} {...handlers} />,
    );
    rerender(
      <AgentUnit
        unit={unitAt()}
        selected={false}
        detail="near"
        theme={theme}
        {...handlers}
        onSelect={vi.fn()}
      />,
    );
    expect(unitRenders).toBe(2);
  });
});
