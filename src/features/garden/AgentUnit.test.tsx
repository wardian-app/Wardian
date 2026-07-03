import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

// Render Konva primitives as plain DOM for assertions.
vi.mock("react-konva", () => ({
  Group: ({ children, ...p }: any) => <div data-konva="group" {...filterProps(p)}>{children}</div>,
  Circle: (p: any) => <div data-konva="circle" data-fill={p.fill} />,
  Rect: (p: any) => <div data-konva="rect" data-fill={p.fill} />,
  Text: (p: any) => <div data-konva="text">{p.text}</div>,
}));

function filterProps(p: Record<string, unknown>) {
  // strip non-DOM props so React does not warn
  const { draggable, onDragMove, onDblClick, ...rest } = p;
  return rest;
}

import { AgentUnit } from "./AgentUnit";

describe("AgentUnit", () => {
  it("renders the agent label and a resolvable status fill", () => {
    document.documentElement.style.setProperty("--color-wardian-success", "#10b981");
    render(
      <AgentUnit
        unit={{
          ref: { kind: "agent", id: "a1" },
          label: "Alpha",
          status: "Idle",
          color: "var(--color-wardian-success)",
          position: { x: 0, y: 0 },
        }}
        selected={false}
        onSelect={vi.fn()}
        onOpen={vi.fn()}
        onDragMove={vi.fn()}
      />,
    );
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    const circles = screen.getAllByText("", { selector: '[data-konva="circle"]' });
    expect(circles.some((c) => c.getAttribute("data-fill") === "#10b981")).toBe(true);
  });
});
