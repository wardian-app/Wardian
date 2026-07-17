import { create } from "zustand";

import type {
  FileContentDescriptorV1,
  FileRendererKind,
  SurfaceBadge,
  SurfaceDefinition,
  WorkbenchSurfaceV1,
} from "../../types";
import type { DirtySurfacePrompt } from "../workbench/surfaces/dirtySurfaceGuards";

export type FilesPresentationEntry = {
  resource_key: string;
  descriptor: FileContentDescriptorV1 | null;
  dirty: boolean;
  attention: boolean;
};

type FilesPresentationStore = {
  presentations: Readonly<Record<string, FilesPresentationEntry>>;
  setPresentation: (surfaceId: string, entry: FilesPresentationEntry) => void;
  clearPresentation: (surfaceId: string) => void;
  reset: () => void;
};

export const useFilesPresentationStore = create<FilesPresentationStore>((set) => ({
  presentations: {},
  setPresentation: (surfaceId, entry) => set((state) => ({
    presentations: { ...state.presentations, [surfaceId]: entry },
  })),
  clearPresentation: (surfaceId) => set((state) => {
    if (!(surfaceId in state.presentations)) return state;
    const presentations = { ...state.presentations };
    delete presentations[surfaceId];
    return { presentations };
  }),
  reset: () => set({ presentations: {} }),
}));

const ICON_BY_RENDERER: Readonly<Record<FileRendererKind, string>> = {
  text: "files-text",
  markdown: "files-markdown",
  image: "files-image",
  pdf: "files-pdf",
  unsupported: "files-unsupported",
};

function resourceValue(resourceKey: string | undefined): string | undefined {
  if (!resourceKey?.startsWith("file:") && !resourceKey?.startsWith("artifact:")) {
    return undefined;
  }
  const value = resourceKey.slice(resourceKey.indexOf(":") + 1).replace(/\\/g, "/");
  return value.length > 0 ? value : undefined;
}

function basename(resourceKey: string | undefined): string | undefined {
  const value = resourceValue(resourceKey)?.replace(/\/+$/, "");
  if (!value) return undefined;
  const name = value.slice(value.lastIndexOf("/") + 1);
  return name || undefined;
}

function fallbackIcon(resourceKey: string | undefined): string {
  if (resourceKey?.startsWith("artifact:")) return "files-artifact";
  const name = basename(resourceKey)?.toLowerCase();
  if (!name) return "files";
  const extension = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  if (["md", "markdown", "mdx"].includes(extension)) return "files-markdown";
  if (["png", "jpg", "jpeg", "gif", "webp", "bmp"].includes(extension)) {
    return "files-image";
  }
  if (extension === "pdf") return "files-pdf";
  if ([
    "c", "cc", "cpp", "css", "csv", "go", "h", "hpp", "html", "ini", "java",
    "js", "json", "jsx", "log", "py", "rs", "sh", "sql", "toml", "ts", "tsx",
    "txt", "xml", "yaml", "yml",
  ].includes(extension)) return "files-text";
  return "files";
}

function presentationEntry(
  surfaceId: string,
  resourceKey: string | undefined,
): FilesPresentationEntry | undefined {
  if (resourceKey === undefined) return undefined;
  const entry = useFilesPresentationStore.getState().presentations[surfaceId];
  return entry?.resource_key === resourceKey ? entry : undefined;
}

export function filesPresentationTitle(
  surfaceId: string,
  resourceKey: string | undefined,
): string {
  const descriptor = presentationEntry(surfaceId, resourceKey)?.descriptor;
  return descriptor?.display_name.trim() || basename(resourceKey) || "Files";
}

export function filesPresentationIcon(
  surfaceId: string,
  resourceKey: string | undefined,
): string {
  const descriptor = presentationEntry(surfaceId, resourceKey)?.descriptor;
  return descriptor ? ICON_BY_RENDERER[descriptor.renderer_kind] : fallbackIcon(resourceKey);
}

export function filesPresentationBadges(
  surfaceId: string,
  resourceKey: string | undefined,
): readonly SurfaceBadge[] {
  const entry = presentationEntry(surfaceId, resourceKey);
  if (!entry) return [];
  const badges: SurfaceBadge[] = [];
  if (entry.dirty) badges.push({ badge_id: "dirty", label: "Unsaved changes" });
  if (entry.attention) badges.push({ badge_id: "attention", label: "Attention requested" });
  return badges;
}

/** Bridges the current generic dirty prompt until durable Files drafts own close actions. */
export function createFilesSurfaceCloseGuard(
  prompt: DirtySurfacePrompt,
): NonNullable<SurfaceDefinition["can_close"]> {
  const pendingBySurface = new Map<string, Promise<"allow" | "cancel">>();
  return (surface: WorkbenchSurfaceV1) => {
    const entry = presentationEntry(surface.surface_id, surface.resource_key);
    if (!entry?.dirty) return "allow";
    const existing = pendingBySurface.get(surface.surface_id);
    if (existing) return existing;
    const pending = Promise.resolve(prompt({
      surface_type: "files",
      title: filesPresentationTitle(surface.surface_id, surface.resource_key),
      message: "Keep or discard Files changes before closing?",
      choices: ["save", "discard", "cancel"],
    })).then((choice) => choice === "save" || choice === "discard" ? "allow" : "cancel")
      .catch(() => "cancel" as const)
      .finally(() => {
        if (pendingBySurface.get(surface.surface_id) === pending) {
          pendingBySurface.delete(surface.surface_id);
        }
      });
    pendingBySurface.set(surface.surface_id, pending);
    return pending;
  };
}
