import type { ComponentProps } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GardenSpatialCell } from "./GardenSpatialCell";

function props(): ComponentProps<typeof GardenSpatialCell> {
  return { target: { kind: "memory", id: "m1" }, label: "Evidence", bounds: { x: 10, y: 20, width: 40, height: 20 },
    camera: { position: { x: 100, y: 50 }, scale: 20 }, viewport: { width: 1000, height: 800 },
    children: <button>Read evidence</button>, onSelect: vi.fn(), onEnter: vi.fn() };
}

describe("GardenSpatialCell", () => {
  it("lays leaf readers out at projected width without scaling their text plane", () => {
    const input = props();
    const view = render(<GardenSpatialCell {...input} />);
    const shell = screen.getByLabelText("Evidence composition");
    expect(shell).toHaveStyle({ width: "800px", left: "300px" });
    expect(shell.style.transform).toBe("");
    const reader = screen.getByRole("region", { name: "Evidence reading area" });
    expect(reader).toHaveStyle({ width: "752px" });
    expect(reader).toHaveAttribute("tabindex", "0");
    expect(shell).toHaveAttribute("data-garden-world", JSON.stringify(input.bounds));
    view.rerender(<GardenSpatialCell {...input} camera={{ ...input.camera, scale: 40 }} />);
    expect(shell).toHaveStyle({ width: "1600px", left: "500px" });
    expect(shell.style.transform).toBe("");
    expect(reader).toHaveStyle({ width: "820px" });
    expect(shell).toHaveAttribute("data-garden-world", JSON.stringify(input.bounds));
  });

  it.each(["workspace", "automation"] as const)("keeps the %s child layout fixed while the camera zooms", (kind) => {
    const input = { ...props(), target: { kind, id: "container" }, children:
      <div style={{ position: "relative", width: "100%", height: 200 }}>
        <button style={{ position: "absolute", left: "25%", top: 40, width: "50%" }}>Child source</button>
      </div> };
    const view = render(<GardenSpatialCell {...input} />);
    const shell = screen.getByLabelText("Evidence composition");
    const contents = screen.getByRole("region", { name: "Evidence reading area" });
    const child = screen.getByRole("button", { name: "Child source" });
    const contentsLayout = contents.getAttribute("style");
    const childLayout = child.getAttribute("style");
    // jsdom has no physical layout. The invariant here is the child's fixed
    // containing width: its percentage coordinates cannot reflow with zoom.
    for (const scale of [20, 40, 15, 20]) {
      view.rerender(<GardenSpatialCell {...input} camera={{ ...input.camera, scale }}
        viewport={{ width: scale === 40 ? 400 : 1000, height: 800 }} />);
      expect(shell).toHaveStyle({ width: "900px", left: `${10 * scale + 100}px` });
      expect(Number.parseFloat(shell.style.height)).toBeCloseTo(702);
      expect(shell.style.transform).toBe(`scale(${40 * scale / 900})`);
      expect(contents).toHaveStyle({ width: "820px" });
      expect(contents.getAttribute("style")).toBe(contentsLayout);
      expect(contents.style.transform).toBe("");
      expect(screen.getByRole("button", { name: "Child source" })).toBe(child);
      expect(child.getAttribute("style")).toBe(childLayout);
      expect(shell).toHaveAttribute("data-garden-world", JSON.stringify(input.bounds));
    }
  });

  it("hides the agent parent context by 2400px and restores it on reverse zoom", () => {
    const input = { ...props(), target: { kind: "agent" as const, id: "parent" }, receding: true };
    const view = render(<GardenSpatialCell {...input} />);
    const shell = screen.getByLabelText("Evidence composition");
    const reader = shell.querySelector(".garden-spatial-contents");
    expect(reader).toBeVisible();
    view.rerender(<GardenSpatialCell {...input} camera={{ ...input.camera, scale: 60 }} />);
    expect(shell).toHaveStyle({ opacity: "0", pointerEvents: "none" });
    expect(reader).toHaveAttribute("aria-hidden", "true");
    expect(reader).toHaveAttribute("inert");
    expect(reader).not.toBeVisible();
    expect(shell).toHaveAttribute("data-garden-world", JSON.stringify(input.bounds));
    view.rerender(<GardenSpatialCell {...input} />);
    expect(shell).toHaveStyle({ opacity: "1", pointerEvents: "none" });
    expect(reader).toBeVisible();
    expect(reader).toHaveAttribute("aria-hidden", "false");
    expect(reader).not.toHaveAttribute("inert");
    expect(shell).toHaveAttribute("data-garden-world", JSON.stringify(input.bounds));
  });

  it("leaves agent placement hit-testing to the canvas while exposing a readable interior", () => {
    const input = { ...props(), target: { kind: "agent" as const, id: "agent" }, bounds: { x: 10, y: 20, width: 32, height: 32 } };
    const view = render(<GardenSpatialCell {...input} camera={{ ...input.camera, scale: 10 }} />);
    const shell = screen.getByLabelText("Evidence composition");
    const contents = shell.querySelector(".garden-spatial-contents");

    expect(shell).toHaveStyle({ pointerEvents: "none" });
    expect(contents).toHaveStyle({ pointerEvents: "none" });

    view.rerender(<GardenSpatialCell {...input} camera={{ ...input.camera, scale: 20 }} />);
    expect(shell).toHaveStyle({ pointerEvents: "none" });
    expect(contents).toHaveStyle({ pointerEvents: "auto" });
  });

  it("sets receding workspace detail to zero and restores accessible reading on reverse zoom", () => {
    const input = { ...props(), target: { kind: "workspace" as const, id: "/work" }, receding: true };
    const view = render(<GardenSpatialCell {...input} />);
    const shell = screen.getByLabelText("Evidence composition");
    expect(screen.getByRole("region", { name: "Evidence reading area" })).toHaveAttribute("tabindex", "0");
    view.rerender(<GardenSpatialCell {...input} camera={{ ...input.camera, scale: 60 }} />);
    expect(shell).toHaveStyle({ opacity: "0", pointerEvents: "none" });
    // CSS consumes --garden-detail for the non-agent reader's opacity. jsdom
    // cannot resolve that variable chain; browser tests prove the visual fade.
    expect(shell).toHaveAttribute("data-garden-detail", "0.000");
    expect(shell.style.getPropertyValue("--garden-detail")).toBe("0");
    expect(screen.queryByRole("region", { name: "Evidence reading area" })).not.toBeInTheDocument();
    expect(shell.querySelector(".garden-spatial-contents")).toHaveAttribute("inert");
    view.rerender(<GardenSpatialCell {...input} />);
    expect(shell).toHaveStyle({ opacity: "1", pointerEvents: "auto" });
    expect(shell).toHaveAttribute("data-garden-detail", "1.000");
    expect(screen.getByRole("region", { name: "Evidence reading area" })).toHaveAttribute("tabindex", "0");
    expect(shell.querySelector(".garden-spatial-contents")).not.toHaveAttribute("inert");
    expect(shell).toHaveAttribute("data-garden-world", JSON.stringify(input.bounds));
  });
});
