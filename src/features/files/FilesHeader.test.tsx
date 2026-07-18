import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";

import type { FileContentDescriptorV1 } from "../../types";
import { FilesHeader } from "./FilesHeader";

const descriptor: FileContentDescriptorV1 = {
  schema: 1,
  canonical_path: "\\\\?\\C:\\work\\deep\\nested\\report.md",
  display_name: "report.md",
  extension: "md",
  mime_type: "text/markdown",
  encoding: "utf-8",
  renderer_kind: "markdown",
  size_bytes: 42,
  line_count: 2,
  content_hash: "sha256:report",
  modified_at_ms: 1,
  capabilities: { preview: true, changes: true, draft: true, stream: false },
  unavailable_reason: null,
};

function header(overrides: Partial<ComponentProps<typeof FilesHeader>> = {}) {
  return (
    <FilesHeader
      resource_key="file:C:/work/deep/nested/report.md"
      descriptor={descriptor}
      presentation="rendered"
      presentation_toggle_available
      dirty={false}
      save_available
      save_as_available
      saving={false}
      resource_actions_available
      on_presentation_change={vi.fn()}
      on_save={vi.fn().mockResolvedValue(undefined)}
      on_save_as={vi.fn().mockResolvedValue(undefined)}
      on_open_with={vi.fn().mockResolvedValue(undefined)}
      on_reveal={vi.fn().mockResolvedValue(undefined)}
      {...overrides}
    />
  );
}

describe("FilesHeader", () => {
  it("shows the current presentation icon while describing the action", async () => {
    const user = userEvent.setup();
    const onPresentationChange = vi.fn();
    const view = render(header({ on_presentation_change: onPresentationChange }));

    const edit = screen.getByRole("button", { name: "Edit source" });
    expect(edit).toHaveAttribute("title", "Edit source");
    expect(edit).toHaveAttribute("aria-pressed", "false");
    expect(edit.querySelector("svg.lucide-book-open")).not.toBeNull();
    await user.click(edit);
    expect(onPresentationChange).toHaveBeenCalledWith("editor");

    view.rerender(header({ presentation: "editor" }));
    const rendered = screen.getByRole("button", { name: "View rendered" });
    expect(rendered).toHaveAttribute("aria-pressed", "true");
    expect(rendered.querySelector("svg.lucide-pencil")).not.toBeNull();
  });

  it("uses an Explorer-safe middle-elided path and a separate dirty breadcrumb dot", () => {
    const { container } = render(header({ dirty: true }));
    const breadcrumb = screen.getByRole("navigation", { name: "File location" });
    expect(breadcrumb).toHaveAttribute("title", "C:\\work\\deep\\nested\\report.md");
    expect(breadcrumb).not.toHaveTextContent("\\\\?\\");
    expect(breadcrumb).toHaveTextContent("report.md");
    expect(container.querySelector(".files-breadcrumb-ellipsis")).not.toBeNull();
    expect(screen.getByLabelText("Unsaved changes")).toHaveClass("files-breadcrumb-dirty");
  });

  it("uses the exact Workbench overflow geometry and no legacy mode tabs", () => {
    const { container } = render(header());
    const trigger = screen.getByRole("button", { name: "File actions" });
    const icon = trigger.querySelector("svg.lucide-ellipsis");
    expect(trigger).toHaveClass("files-overflow-trigger");
    expect(trigger).toHaveAttribute("data-hit-size", "26");
    expect(icon).toHaveAttribute("width", "17");
    expect(icon).toHaveAttribute("height", "17");
    expect(icon).toHaveAttribute("stroke-width", "1.75");
    expect(container.querySelector('[role="tablist"]')).toBeNull();
    expect(screen.queryByText("Preview")).toBeNull();
    expect(screen.queryByText("Changes")).toBeNull();
    expect(screen.queryByText("Draft")).toBeNull();

    const css = readFileSync(resolve(process.cwd(), "src/features/files/FilesSurface.css"), "utf8");
    expect(css).toMatch(/\.files-overflow-trigger\s*\{[^}]*width:\s*26px[^}]*height:\s*26px/s);
    expect(css).toMatch(/\.files-header\s*\{[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\)\s+auto/s);
  });

  it("exposes Save and Save As only when supported and preserves keyboard menu behavior", async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockResolvedValue(undefined);
    const onSaveAs = vi.fn().mockResolvedValue(undefined);
    const view = render(header({ on_save: onSave, on_save_as: onSaveAs }));
    const trigger = screen.getByRole("button", { name: "File actions" });

    trigger.focus();
    await user.keyboard("{ArrowDown}");
    const menu = screen.getByRole("menu", { name: "File actions" });
    await waitFor(() => expect(within(menu).getByRole("menuitem", { name: "Save" })).toHaveFocus());
    await user.keyboard("{Enter}");
    expect(onSave).toHaveBeenCalledOnce();

    await user.click(trigger);
    await user.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Save As" }));
    expect(onSaveAs).toHaveBeenCalledOnce();

    view.rerender(header({ save_available: false, save_as_available: false }));
    await user.click(screen.getByRole("button", { name: "File actions" }));
    expect(screen.queryByRole("menuitem", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Save As" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Open With" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Reveal" })).toBeInTheDocument();
  });

  it("contains asynchronous action failures after closing the menu", async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockRejectedValue(new Error("save failed"));
    render(header({ on_save: onSave }));
    await user.click(screen.getByRole("button", { name: "File actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Save" }));
    expect(screen.queryByRole("menu")).toBeNull();
    await Promise.resolve();
    expect(onSave).toHaveBeenCalledOnce();
  });

  it("dismisses on outside pointer interactions", async () => {
    const user = userEvent.setup();
    render(<>{header()}<button type="button">Outside</button></>);
    await user.click(screen.getByRole("button", { name: "File actions" }));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Outside" }));
    expect(screen.queryByRole("menu")).toBeNull();
  });
});
