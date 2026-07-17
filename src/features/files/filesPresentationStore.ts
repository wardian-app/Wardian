import { create } from "zustand";

import type {
  FileContentDescriptorV1,
  FileRendererKind,
  SurfaceBadge,
  SurfaceDefinition,
  WorkbenchSurfaceV1,
} from "../../types";
import type { DirtySurfacePrompt } from "../workbench/surfaces/dirtySurfaceGuards";
import {
  decodeFileResourceKey,
  filePathIdentity,
  isWindowsAbsoluteFilePath,
  type DecodedFileResourceKey,
} from "./fileResourceKey";

export type FilesPresentationEntry = {
  resource_key: string;
  descriptor: FileContentDescriptorV1 | null;
  dirty: boolean;
  attention: boolean;
};

type FilesPresentationStore = {
  presentations: Readonly<Record<string, FilesPresentationEntry>>;
  setPresentation: (surfaceId: string, entry: FilesPresentationEntry) => void;
  syncPresentations: (surfaces: readonly WorkbenchSurfaceV1[]) => void;
  reset: () => void;
};

export const useFilesPresentationStore = create<FilesPresentationStore>((set) => ({
  presentations: {},
  setPresentation: (surfaceId, entry) => set((state) => ({
    presentations: { ...state.presentations, [surfaceId]: entry },
  })),
  syncPresentations: (surfaces) => set((state) => {
    const nextEntries: [string, FilesPresentationEntry][] = [];
    for (const surface of surfaces) {
      const resourceKey = surface.resource_key;
      if (surface.surface_type !== "files" || resourceKey === undefined) continue;
      const current = state.presentations[surface.surface_id];
      nextEntries.push([
        surface.surface_id,
        current?.resource_key === resourceKey
          ? current
          : {
              resource_key: resourceKey,
              descriptor: null,
              dirty: false,
              attention: false,
            },
      ]);
    }
    const next = Object.fromEntries(nextEntries);
    const currentIds = Object.keys(state.presentations);
    const nextIds = Object.keys(next);
    if (
      currentIds.length === nextIds.length
      && nextIds.every((surfaceId) => state.presentations[surfaceId] === next[surfaceId])
    ) return state;
    return { presentations: next };
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

function decodedResource(resourceKey: string | undefined): DecodedFileResourceKey | undefined {
  if (resourceKey === undefined) return undefined;
  try {
    return decodeFileResourceKey(resourceKey);
  } catch {
    return undefined;
  }
}

type PresentationPath = {
  display: readonly string[];
  comparison: readonly string[];
  identity: string;
};

function normalizedPresentationPath(value: string | undefined): PresentationPath | undefined {
  const trimmed = value?.trim();
  const normalized = trimmed ? filePathIdentity(trimmed).replace(/\/+$/g, "") : undefined;
  if (!normalized) return undefined;
  const display = normalized.split("/").filter(Boolean);
  if (display.length === 0) return undefined;
  const windowsPath = isWindowsAbsoluteFilePath(normalized);
  const comparison = windowsPath ? display.map((segment) => segment.toLowerCase()) : display;
  return { display, comparison, identity: comparison.join("/") };
}

function entryPath(entry: FilesPresentationEntry): PresentationPath | undefined {
  const descriptorPath = normalizedPresentationPath(entry.descriptor?.canonical_path);
  if (descriptorPath) return descriptorPath;
  const decoded = decodedResource(entry.resource_key);
  if (decoded?.resource_kind === "file") return normalizedPresentationPath(decoded.path);
  if (decoded?.resource_kind === "artifact") {
    return {
      display: [decoded.artifact_id],
      comparison: [decoded.artifact_id],
      identity: decoded.resource_key,
    };
  }
  return undefined;
}

function entryBaseName(entry: FilesPresentationEntry): string {
  return entry.descriptor?.display_name.trim() || basename(entry.resource_key) || "Files";
}

function suffixForDepth(path: PresentationPath | undefined, depth: number): string | undefined {
  if (!path || path.display.length < 2) return undefined;
  const parents = path.display.slice(0, -1);
  return parents.slice(-depth).join("/") || undefined;
}

function comparisonSuffixForDepth(
  path: PresentationPath | undefined,
  depth: number,
): string | undefined {
  if (!path || path.comparison.length < 2) return undefined;
  const parents = path.comparison.slice(0, -1);
  return parents.slice(-depth).join("/") || undefined;
}

function distinguishingTitle(surfaceId: string, entry: FilesPresentationEntry): string {
  const label = entryBaseName(entry);
  const collisions = Object.entries(useFilesPresentationStore.getState().presentations)
    .filter(([, candidate]) => entryBaseName(candidate) === label)
    .sort(([leftId], [rightId]) => leftId.localeCompare(rightId));
  if (collisions.length < 2) return label;

  const path = entryPath(entry);
  const maxDepth = Math.max(
    0,
    ...collisions.map(([, candidate]) => Math.max(0, (entryPath(candidate)?.display.length ?? 1) - 1)),
  );
  for (let depth = 1; depth <= maxDepth; depth += 1) {
    const candidateSuffix = comparisonSuffixForDepth(path, depth);
    if (!candidateSuffix) break;
    const suffixMatches = collisions.filter(([, candidate]) => (
      comparisonSuffixForDepth(entryPath(candidate), depth) === candidateSuffix
    ));
    if (suffixMatches.length === 1) {
      return `${suffixForDepth(path, depth)}/${label}`;
    }
  }

  const sameIdentity = collisions.filter(([, candidate]) => (
    entryPath(candidate)?.identity === path?.identity
  ));
  const fallbackCandidates = sameIdentity.length > 1 ? sameIdentity : collisions;
  const fallbackIndex = fallbackCandidates.findIndex(([candidateId]) => candidateId === surfaceId);
  return `${label} (${fallbackIndex >= 0 ? fallbackIndex + 1 : 1})`;
}

function basename(resourceKey: string | undefined): string | undefined {
  const decoded = decodedResource(resourceKey);
  if (decoded?.resource_kind === "artifact") return decoded.artifact_id;
  if (decoded?.resource_kind !== "file") return undefined;
  const display = normalizedPresentationPath(decoded.path)?.display;
  return display?.[display.length - 1];
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
  const entry = presentationEntry(surfaceId, resourceKey);
  if (!entry) return basename(resourceKey) || "Files";
  return distinguishingTitle(surfaceId, entry);
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
