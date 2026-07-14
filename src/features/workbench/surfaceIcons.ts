import {
  AppWindow,
  FileCode2,
  Gauge,
  GitBranch,
  Globe2,
  LayoutGrid,
  Library as LibraryIcon,
  ListTodo,
  Network,
  Sprout,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react";

const SURFACE_ICONS: Readonly<Record<string, LucideIcon>> = {
  "agents-overview": LayoutGrid,
  dashboard: Gauge,
  queue: ListTodo,
  graph: Network,
  garden: Sprout,
  library: LibraryIcon,
  workflows: GitBranch,
  "agent-session": SquareTerminal,
  "file-editor": FileCode2,
  browser: Globe2,
};

/** Resolves a compact visual identifier for core and contributed surface types. */
export function surfaceIconForType(surfaceType: string): LucideIcon {
  return SURFACE_ICONS[surfaceType] ?? AppWindow;
}
