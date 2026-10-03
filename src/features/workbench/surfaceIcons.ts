import {
  AppWindow,
  ChartNoAxesGantt,
  FileCode2,
  FileCheck2,
  FileImage,
  FileQuestion,
  FileText,
  FileType2,
  Gauge,
  GitBranch,
  Globe2,
  LayoutGrid,
  Library as LibraryIcon,
  ListTodo,
  Network,
  FilePlus2,
  Sprout,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react";

const SURFACE_ICONS: Readonly<Record<string, LucideIcon>> = {
  "agents-overview": LayoutGrid,
  // A gauge reads "now" and a run of offset bars reads "over time", which is
  // exactly how these two surfaces divide: the Dashboard is a live meter, and
  // Analytics is rows against a time axis. The glyph is also a fair likeness of
  // what Analytics actually draws.
  dashboard: Gauge,
  analytics: ChartNoAxesGantt,
  inbox: ListTodo,
  graph: Network,
  garden: Sprout,
  library: LibraryIcon,
  automations: GitBranch,
  "agent-session": SquareTerminal,
  files: FileCode2,
  "files-text": FileText,
  "files-markdown": FileType2,
  "files-image": FileImage,
  "files-pdf": FileText,
  "files-artifact": FileCheck2,
  "files-unsupported": FileQuestion,
  browser: Globe2,
  "new-tab": FilePlus2,
};

/**
 * Resolves a compact visual identifier from a surface definition's icon token.
 *
 * The fallback is a real fallback, not a default. A core surface with no entry
 * above gets the generic window glyph and looks like an unclassified tab —
 * Analytics shipped that way, because a surface definition's token is its type
 * and nothing forced this table to know about it. `surfaceIcons.test.ts` now
 * requires every registered surface to appear here.
 */
export function surfaceIconForToken(icon: string): LucideIcon {
  return SURFACE_ICONS[icon] ?? AppWindow;
}

/**
 * Icon tokens with an explicit glyph. Exported so the registry can be checked.
 *
 * @internal Test support, no production caller: surfaceIcons tests check registry coverage with it.
 */
export function mappedSurfaceIconTokens(): readonly string[] {
  return Object.keys(SURFACE_ICONS);
}
