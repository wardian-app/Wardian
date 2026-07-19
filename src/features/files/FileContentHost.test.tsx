import { lazy } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { FileContentDescriptorV1, FileResourceSnapshotV1 } from "../../types";
import type { FileEditorController } from "./fileEditorController";
import { FileContentHost } from "./FileContentHost";
import type { FileResourceClient } from "./fileResourceClient";
import { RendererRegistry, type FileRendererDefinition } from "./rendererRegistry";

const renderedMount = vi.fn();
const editorMount = vi.fn();
const Rendered = lazy(async () => ({
  default: ({ buffer_snapshot }: { buffer_snapshot?: { text: string } | null }) => {
    renderedMount();
    return <div data-testid="rendered">{buffer_snapshot?.text}</div>;
  },
}));
const Editor = lazy(async () => ({
  default: () => {
    editorMount();
    return <div data-testid="editor">Editor</div>;
  },
}));

function descriptor(rendererKind: FileContentDescriptorV1["renderer_kind"]): FileContentDescriptorV1 {
  return {
    schema: 1,
    canonical_path: `C:/work/file.${rendererKind === "markdown" ? "md" : "txt"}`,
    display_name: "file",
    extension: rendererKind === "markdown" ? "md" : "txt",
    mime_type: rendererKind === "markdown" ? "text/markdown" : "text/plain",
    encoding: rendererKind === "image" ? null : "utf-8",
    renderer_kind: rendererKind,
    size_bytes: 4,
    line_count: rendererKind === "image" ? null : 1,
    content_hash: "hash",
    modified_at_ms: 1,
    capabilities: { preview: true, changes: true, draft: true, stream: false },
    unavailable_reason: null,
  };
}

function definition(
  renderer_id: string,
  defaultPresentation: "rendered" | "editor",
  rendered: boolean,
  editor: boolean,
): FileRendererDefinition {
  const legacy = rendered ? Rendered : Editor;
  return {
    renderer_id,
    matches: ({ renderer_kind }) => renderer_kind === renderer_id,
    capabilities: { preview: true, changes: "line", draft: editor, annotations: "line_range" },
    render: legacy,
    create_renderer: () => legacy,
    default_presentation: defaultPresentation,
    rendered: rendered ? { render: Rendered, create_renderer: () => Rendered } : undefined,
    editor: editor ? { render: Editor, create_renderer: () => Editor } : undefined,
    editor_language: () => "plaintext",
  };
}

const registry = new RendererRegistry([
  definition("markdown", "rendered", true, true),
  definition("text", "editor", false, true),
  definition("image", "rendered", true, false),
  definition("unsupported", "rendered", true, false),
]);

function snapshot(rendererKind: FileContentDescriptorV1["renderer_kind"], revision = 1): FileResourceSnapshotV1 {
  return {
    resource_id: `file:C:/work/file.${rendererKind}`,
    subscription_id: "subscription",
    revision,
    descriptor: { ...descriptor(rendererKind), content_hash: `hash-${revision}` },
  };
}

function hostProps(rendererKind: FileContentDescriptorV1["renderer_kind"]) {
  const owner = snapshot(rendererKind);
  return {
    snapshot: owner,
    client: {} as FileResourceClient,
    lifecycle: { visible: true },
    registry,
    presentation: registry.resolve(owner.descriptor).default_presentation!,
    surface_id: "files-a",
    editor_controller: {} as FileEditorController,
    buffer_snapshot: Object.freeze({
      resource_id: owner.resource_id,
      revision: owner.revision,
      buffer_generation: 1,
      text: "dirty working text",
      dirty: true,
      read_only: false,
    }),
    on_open_file: vi.fn(),
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  };
}

describe("FileContentHost", () => {
  it("keeps Markdown rendered and editor presentations mounted across switches", async () => {
    renderedMount.mockClear();
    editorMount.mockClear();
    const props = hostProps("markdown");
    const view = render(<FileContentHost {...props} />);
    expect(await screen.findByTestId("rendered")).toHaveTextContent("dirty working text");
    expect(await screen.findByTestId("editor")).toBeInTheDocument();
    expect(screen.getByTestId("rendered").closest("[data-file-presentation]"))
      .not.toHaveAttribute("hidden");
    expect(screen.getByTestId("editor").closest("[data-file-presentation]"))
      .toHaveAttribute("hidden");

    view.rerender(<FileContentHost {...props} presentation="editor" />);
    expect(screen.getByTestId("rendered").closest("[data-file-presentation]"))
      .toHaveAttribute("hidden");
    expect(screen.getByTestId("editor").closest("[data-file-presentation]"))
      .not.toHaveAttribute("hidden");
  });

  it("renders source-only text without a redundant rendered layer", async () => {
    render(<FileContentHost {...hostProps("text")} />);
    expect(await screen.findByTestId("editor")).toBeInTheDocument();
    expect(screen.queryByTestId("rendered")).toBeNull();
  });

  it("keeps image content read-only", async () => {
    render(<FileContentHost {...hostProps("image")} editor_controller={null} buffer_snapshot={null} />);
    expect(await screen.findByTestId("rendered")).toBeInTheDocument();
    expect(screen.queryByTestId("editor")).toBeNull();
  });
});
