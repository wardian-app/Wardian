import type { ReactNode } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-konva", () => {
  type Props = { children?: ReactNode; text?: string; name?: string; id?: string; x?: number; y?: number; height?: number; points?: number[]; dash?: number[]; opacity?: number; listening?: boolean; visible?: boolean; onClick?: () => void; onDblClick?: () => void };
  const shape = (kind: string) => (props: Props) => <div data-shape={kind} data-name={props.name} data-id={props.id}
    data-x={props.x} data-y={props.y} data-height={props.height} data-points={props.points?.join(",")} data-dash={props.dash?.join(",")}
    data-opacity={props.opacity} data-listening={props.listening} data-visible={props.visible}
    onClick={props.onClick} onDoubleClick={props.onDblClick}>{props.text}{props.children}</div>;
  return { Group: shape("Group"), Circle: shape("Circle"), Rect: shape("Rect"), Arrow: shape("Arrow"), Text: shape("Text") };
});

import { DistrictLayer } from "./DistrictLayer";
import { TerrainLayer } from "./TerrainLayer";
import { AutomationRoutesLayer } from "./AutomationRoutesLayer";
import { districtPopulations, situatedRoutes } from "./canvasHierarchy";
import type { CanvasAutomationInput } from "./automationCanvasPresentation";
import type { GardenAgentUnit } from "./garden.types";
import { GARDEN_THEME_FALLBACK as theme } from "./useGardenTheme";

const districts = new Map([["d", { origin: { x: 0, y: 0 }, radius: 100, roots: ["/workspace"] }]]);
const agents: GardenAgentUnit[] = ["a", "b"].map((id, index) => ({ ref: { kind: "agent", id }, label: id, status: index ? "Error" : "Idle", color: "", crown: [], position: { x: index * 10, y: 0 } }));

describe("canvas aggregate and route paint", () => {
  it("suppresses only cluster count text while retaining the cluster hit target", () => {
    const bands = new Map([["d", "habitat" as const]]);
    const populations = districtPopulations(agents, districts, bands, 1);
    const onSelect = vi.fn();
    const props = { districts, populations, bands, scale: 2, selectedKey: null, theme, onSelect, onOpen: vi.fn() };
    const { container, rerender } = render(<DistrictLayer {...props} visibleLabelIds={new Set(["district-count:d"])} />);
    expect(screen.getByText("2")).toHaveAttribute("data-height", String(theme.labelSize / 2));
    rerender(<DistrictLayer {...props} visibleLabelIds={new Set()} />);
    expect(screen.queryByText("2")).not.toBeInTheDocument();
    const cluster = container.querySelector('[data-name="district-population"]')!;
    expect(cluster.querySelectorAll('[data-shape="Circle"]').length).toBeGreaterThan(0);
    fireEvent.click(cluster);
    expect(onSelect).toHaveBeenCalledWith("d");
  });
  it("suppresses district text without removing selectable ground", () => {
    const onSelect = vi.fn();
    const { container } = render(<DistrictLayer districts={districts} labels={new Map([["d", "District heading"]])}
      scale={1} selectedKey={null} theme={theme} visibleLabelIds={new Set()} onSelect={onSelect} onOpen={vi.fn()} />);
    expect(screen.queryByText("District heading")).not.toBeInTheDocument();
    fireEvent.click(container.querySelector('[data-shape="Circle"]')!);
    expect(onSelect).toHaveBeenCalledWith("d");
  });

  it("suppresses nested terrain labels locally and respects a shared collision result", () => {
    const cells = [0, 1].map((depth) => ({ path: `/root/${depth}`, name: `Folder ${depth}`, depth,
      districtId: "d", isDir: true, truncated: false, rect: { x: -50 + depth, y: -50 + depth, width: 150, height: 100 } }));
    const onSelect = vi.fn();
    const { container, rerender } = render(<TerrainLayer cells={cells} districts={districts} scale={1} theme={theme} onSelectPath={onSelect} />);
    expect(screen.getByText("Folder 0")).toBeInTheDocument();
    expect(screen.queryByText("Folder 1")).not.toBeInTheDocument();
    rerender(<TerrainLayer cells={cells} districts={districts} scale={1} theme={theme} visibleLabelIds={new Set()} onSelectPath={onSelect} />);
    expect(screen.queryByText("Folder 0")).not.toBeInTheDocument();
    expect(container.querySelectorAll('[data-shape="Rect"]')).toHaveLength(2);
    fireEvent.click(container.querySelector('[data-shape="Rect"]')!);
    expect(onSelect).toHaveBeenCalledWith("/root/0");
  });

  it("keeps route hits and failure markers when their captions lose collisions", () => {
    const input: CanvasAutomationInput = { id: "r", label: "Collision route", nodeCount: 1, agentIds: ["a"], runStatus: "running",
      stages: [{ nodeId: "step", agentId: "a", status: "failed" }],
      runEvidence: [{ summary: { run_id: "run", blueprint_id: "bp", status: "running", node_count: 1, path: "run" }, invocation: null, detail: null }] };
    const routes = situatedRoutes([input], agents, districts);
    const onSelect = vi.fn();
    const { container } = render(<AutomationRoutesLayer routes={routes} theme={theme} scale={1} selectedKey={null}
      visibleLabelIds={new Set()} onSelect={onSelect} onOpen={vi.fn()} />);
    expect(screen.queryByText(routes[0].presentation.summary)).not.toBeInTheDocument();
    expect(screen.queryByText(routes[0].presentation.markers[0].label)).not.toBeInTheDocument();
    expect(screen.getByText("×")).toBeInTheDocument();
    fireEvent.click(container.querySelector('[data-shape="Arrow"]')!);
    expect(onSelect).toHaveBeenCalledWith({ kind: "automation", id: "r" });
  });

  it("culls offscreen districts while retaining ground enclosing the viewport", () => {
    const { rerender } = render(<DistrictLayer districts={districts} scale={1} selectedKey={null} theme={theme}
      viewport={{ x: 500, y: 500, width: 100, height: 100 }} onSelect={vi.fn()} onOpen={vi.fn()} />);
    expect(document.querySelector('[data-shape="Circle"]')).toBeNull();
    rerender(<DistrictLayer districts={districts} scale={10} selectedKey={null} theme={theme}
      viewport={{ x: -5, y: -5, width: 10, height: 10 }} onSelect={vi.fn()} onOpen={vi.fn()} />);
    expect(document.querySelector('[data-shape="Circle"]')).not.toBeNull();
  });

  it("smoothly fades ordinary connections and removes zero-opacity route hit regions", () => {
    const input: CanvasAutomationInput = { id: "quiet", label: "Quiet flow", nodeCount: 2, agentIds: ["a", "b"], runStatus: "none" };
    const routes = situatedRoutes([input], agents, districts);
    const onSelect = vi.fn();
    const { container, rerender } = render(<AutomationRoutesLayer routes={routes} theme={theme} scale={.32} continuousZoom
      selectedKey={null} onSelect={onSelect} onOpen={vi.fn()} />);
    const routePart = () => container.querySelector('[data-name="automation-route"]')!;
    for (const [scale, opacity] of [[.32, 0], [.45, 0], [.825, .5], [1.2, 1], [.825, .5], [.32, 0]]) {
      rerender(<AutomationRoutesLayer routes={routes} theme={theme} scale={scale} continuousZoom
        selectedKey={null} onSelect={onSelect} onOpen={vi.fn()} />);
      expect(Number(routePart().getAttribute("data-opacity"))).toBeCloseTo(opacity);
      expect(routePart()).toHaveAttribute("data-listening", String(opacity > 0));
      expect(routePart()).toHaveAttribute("data-visible", String(opacity > 0));
      expect(routePart().querySelector('[data-shape="Arrow"]')).not.toBeNull();
      expect(routePart().querySelector('[data-shape="Circle"]')).not.toBeNull();
    }
    expect(screen.queryByText(routes[0].presentation.summary)).not.toBeInTheDocument();
    rerender(<AutomationRoutesLayer routes={routes} theme={theme} scale={.32} continuousZoom
      selectedKey="automation:quiet" onSelect={onSelect} onOpen={vi.fn()} />);
    expect(routePart()).toHaveAttribute("data-opacity", "1");
    expect(routePart()).toHaveAttribute("data-listening", "true");
    expect(screen.getByText(routes[0].presentation.summary)).toBeInTheDocument();
    fireEvent.click(routePart().querySelector('[data-shape="Arrow"]')!);
    expect(onSelect).toHaveBeenCalledWith({ kind: "automation", id: "quiet" });
    rerender(<AutomationRoutesLayer routes={routes} theme={theme} scale={.32}
      selectedKey={null} onSelect={onSelect} onOpen={vi.fn()} />);
    expect(routePart()).toHaveAttribute("data-opacity", "1");
    expect(routePart()).toHaveAttribute("data-listening", "true");
    expect(screen.getByText(routes[0].presentation.summary)).toBeInTheDocument();
  });

  it("defers dense orbit labels until there is enough screen circumference", () => {
    const inputs: CanvasAutomationInput[] = Array.from({ length: 9 }, (_, index) => ({
      id: `routine-${index}`, label: `Routine ${index}`, nodeCount: 1, agentIds: ["a"], runStatus: "none",
    }));
    const routes = situatedRoutes(inputs, agents, districts);
    const { rerender } = render(<AutomationRoutesLayer routes={routes} theme={theme} scale={3.2} continuousZoom
      selectedKey={null} onSelect={vi.fn()} onOpen={vi.fn()} />);
    expect(screen.queryByText(routes[0].presentation.summary)).not.toBeInTheDocument();
    rerender(<AutomationRoutesLayer routes={routes} theme={theme} scale={3.2} continuousZoom
      selectedKey="automation:routine-0" onSelect={vi.fn()} onOpen={vi.fn()} />);
    expect(screen.getByText(routes[0].presentation.summary)).toBeInTheDocument();
    rerender(<AutomationRoutesLayer routes={routes} theme={theme} scale={5.2} continuousZoom
      selectedKey={null} onSelect={vi.fn()} onOpen={vi.fn()} />);
    expect(screen.getByText(routes[0].presentation.summary)).toBeInTheDocument();
  });

  it("retains a compact failed-stage hit without drawing the failed attention ring", () => {
    const input: CanvasAutomationInput = { id: "attention", label: "Attention", nodeCount: 1, agentIds: ["a"], runStatus: "failed",
      stages: [{ nodeId: "step", agentId: "a", status: "failed" }] };
    const routes = situatedRoutes([input], agents, districts);
    const onSelect = vi.fn(); const onOpen = vi.fn();
    const { container, rerender } = render(<AutomationRoutesLayer routes={routes} theme={theme} scale={.32} continuousZoom
      selectedKey={null} onSelect={onSelect} onOpen={onOpen} />);
    for (const mode of ["all", "markers"] as const) {
      rerender(<AutomationRoutesLayer routes={routes} theme={theme} scale={.32} continuousZoom mode={mode}
        selectedKey={null} onSelect={onSelect} onOpen={onOpen} />);
      const attention = container.querySelector('[data-id="stages:step:a"]')!;
      expect(attention).not.toBeNull();
      expect(attention.closest('[data-name="automation-route"]')).toBeNull();
      expect(attention.querySelector('[data-name="stage-attention"]')).toBeNull();
      expect(attention).toHaveTextContent("×");
      fireEvent.click(attention); fireEvent.doubleClick(attention);
      expect(onSelect).toHaveBeenLastCalledWith({ kind: "automation", id: "attention" });
      expect(onOpen).toHaveBeenLastCalledWith({ kind: "automation", id: "attention" });
    }
  });

  it("paints aggregate status and one population with selection/open semantics at Habitat", () => {
    const bands = new Map([["d", "habitat" as const]]);
    const onSelect = vi.fn(); const onOpen = vi.fn();
    const { container, rerender } = render(<DistrictLayer districts={districts} populations={districtPopulations(agents, districts, bands, 1)} bands={bands}
      scale={1} selectedKey={null} theme={theme} onSelect={onSelect} onOpen={onOpen} />);
    expect(screen.getByText("2 agents · 1 Error · 1 Idle")).toBeInTheDocument();
    const population = container.querySelector('[data-name="district-population"]')!;
    expect(population).not.toBeNull();
    fireEvent.click(population); fireEvent.doubleClick(population);
    expect(onSelect).toHaveBeenCalledWith("d"); expect(onOpen).toHaveBeenCalledWith("d");
    rerender(<DistrictLayer districts={districts} populations={districtPopulations(agents, districts, bands, 1)} bands={new Map([["d", "workstream"]])}
      scale={1} selectedKey={null} theme={theme} onSelect={onSelect} onOpen={onOpen} />);
    expect(screen.queryByText("2 agents · 1 Error · 1 Idle")).not.toBeInTheDocument();
    expect(container.querySelector('[data-name="district-population"]')).toBeNull();
  });

  it("paints localized failure, a temporary-provider marker, pause stroke and concurrent count", () => {
    const input: CanvasAutomationInput = { id: "s", label: "Build", nodeCount: 2, agentIds: ["a", "b"], runStatus: "running", activeRunCount: 2,
      schedule: { id: "s", blueprint_id: "bp", name: "Build", input: null, bindings: {}, is_paused: true, schedule: { schedule_type: "interval", active: true } },
      stages: [{ nodeId: "failed-step", agentId: "b", status: "failed" }, { nodeId: "temp", temporaryProvider: "provider", workspace: "/workspace" }],
      runEvidence: [{ summary: { run_id: "r", blueprint_id: "bp", status: "running", node_count: 2, path: "run" }, invocation: null, detail: null }],
    };
    const { container } = render(<AutomationRoutesLayer routes={situatedRoutes([input], agents, districts)} theme={theme} scale={1} selectedKey={null} onSelect={vi.fn()} onOpen={vi.fn()} />);
    expect(screen.getByText(/Schedule paused · 2 active runs/)).toBeInTheDocument();
    expect(container.querySelector('[data-shape="Arrow"]')).toHaveAttribute("data-dash", "7,4,1,4");
    expect(container.querySelector('[data-id="r:failed-step:b"]')).toHaveAttribute("data-x", "10");
    expect(container.querySelector('[data-id="r:failed-step:b"]')).toHaveAttribute("data-y", "0");
    expect(container.querySelector('[data-name="stage-attention"]')).toBeNull();
    expect(container.querySelector('[data-id="r:temp:provider"]')).toHaveAttribute("data-y", "65");
    expect(container.querySelector('[data-id="r:temp:provider"]')).toHaveTextContent("temp · Active run · Temporary provider");
    expect(screen.getByText("temp · Active run · Temporary provider")).toBeInTheDocument();
    expect(container.querySelector('[data-name="temporary-provider"]')).toBeNull();
    expect(screen.getByText(/failed-step · Failed/)).toBeInTheDocument();
  });
});
