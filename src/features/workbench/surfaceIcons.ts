import {
  AppWindow,
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
  dashboard: Gauge,
  queue: ListTodo,
  graph: Network,
  garden: Sprout,
  library: LibraryIcon,
  workflows: GitBranch,
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

/** Resolves a compact visual identifier from a surface definition's icon token. */
export function surfaceIconForToken(icon: string): LucideIcon {
  return SURFACE_ICONS[icon] ?? AppWindow;
}
