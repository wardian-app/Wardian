import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import type { FileContentDescriptorV1, FilesSurfaceStateV1 } from "../../types";
import { FilesModeBar } from "./FilesModeBar";

const state: FilesSurfaceStateV1 = {
  resource_kind: "file",
  mode: "preview",
  transient_preview: false,
  review_drawer_open: false,
  selected_version_id: null,
  optional_checkpoint_id: null,
};

const descriptor: FileContentDescriptorV1 = {
  schema: 1,
  canonical_path: "C:/work/report.pdf",
  display_name: "report.pdf",
  extension: "pdf",
  mime_type: "application/pdf",
  encoding: null,
  renderer_kind: "pdf",
  size_bytes: 42,
  line_count: null,
  content_hash: "sha256:report",
  modified_at_ms: 1,
  capabilities: { preview: true, changes: false, draft: false, stream: true },
  unavailable_reason: null,
};

function modeBar(overrides: {
  on_open_with?: (path: string) => Promise<void> | void;
  on_reveal?: (path: string) => Promise<void> | void;
} = {}) {
  return (
    <FilesModeBar
      resource_key="file:C:/work/report.pdf"
      state={state}
      descriptor={descriptor}
      on_open_with={overrides.on_open_with ?? vi.fn()}
      on_reveal={overrides.on_reveal ?? vi.fn()}
    />
  );
}

describe("FilesModeBar actions menu", () => {
  it("keeps narrow-pane modes scrollable and constrains actions to the pane", () => {
    const { container } = render(modeBar());
    expect(screen.getByRole("navigation", { name: "File location" })).toBeInTheDocument();
    expect(screen.getByRole("tablist", { name: "File mode" })).toHaveClass("files-mode-tabs");
    expect(screen.getByRole("button", { name: "File actions" })).toBeInTheDocument();

    const css = readFileSync(resolve(process.cwd(), "src/features/files/FilesSurface.css"), "utf8");
    expect(css).toMatch(/\.files-overflow-menu\s*\{[^}]*max-width:\s*calc\(100cqw - 8px\)/s);
    expect(css).toMatch(/@container \(max-width:\s*220px\)[\s\S]*\.files-mode-tabs\s*\{[^}]*overflow-x:\s*auto/s);
    expect(container.querySelector(".files-mode-bar")).toBeInTheDocument();
  });

  it("opens from the trigger at the directional edge requested by the arrow key", async () => {
    const user = userEvent.setup();
    render(modeBar());
    const trigger = screen.getByRole("button", { name: "File actions" });
    trigger.focus();

    await user.keyboard("{ArrowUp}");
    await waitFor(() => expect(screen.getByRole("menuitem", { name: "Reveal" })).toHaveFocus());
    await user.keyboard("{Escape}");
    expect(trigger).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("menuitem", { name: "Open With" })).toHaveFocus());
  });

  it("focuses and cycles menu items, then restores the trigger on Escape", async () => {
    const user = userEvent.setup();
    render(modeBar());
    const trigger = screen.getByRole("button", { name: "File actions" });

    await user.click(trigger);
    const openWith = screen.getByRole("menuitem", { name: "Open With" });
    const reveal = screen.getByRole("menuitem", { name: "Reveal" });
    await waitFor(() => expect(openWith).toHaveFocus());
    await user.keyboard("{ArrowDown}");
    expect(reveal).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(openWith).toHaveFocus();
    await user.keyboard("{End}");
    expect(reveal).toHaveFocus();
    await user.keyboard("{Home}");
    expect(openWith).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(reveal).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it("dismisses on outside pointer and focus-out interactions", async () => {
    const user = userEvent.setup();
    render(<>{modeBar()}<button type="button">Outside</button></>);
    const trigger = screen.getByRole("button", { name: "File actions" });
    const outside = screen.getByRole("button", { name: "Outside" });

    await user.click(trigger);
    fireEvent.pointerDown(outside);
    expect(screen.queryByRole("menu")).toBeNull();

    await user.click(trigger);
    const openWith = screen.getByRole("menuitem", { name: "Open With" });
    fireEvent.blur(openWith, { relatedTarget: outside });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("closes before invoking an async action and safely contains rejections", async () => {
    const user = userEvent.setup();
    const onOpenWith = vi.fn().mockRejectedValue(new Error("external app failed"));
    render(modeBar({ on_open_with: onOpenWith }));
    const trigger = screen.getByRole("button", { name: "File actions" });

    await user.click(trigger);
    await user.click(screen.getByRole("menuitem", { name: "Open With" }));
    expect(screen.queryByRole("menu")).toBeNull();
    expect(trigger).toHaveFocus();
    expect(onOpenWith).toHaveBeenCalledWith("C:/work/report.pdf");
    await Promise.resolve();
  });
});
