import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Terminal, ILink } from "@xterm/xterm";
import type { ExternalEditorSetting } from "../../types/settings";

type ExternalEditorLaunchSettings = {
  external_editor: ExternalEditorSetting;
  external_editor_custom_executable: string | null;
};

export type TerminalDetectedLink = {
  kind: "url" | "file";
  text: string;
  target: string;
  startIndex: number;
};

type TerminalLinkProviderOptions = {
  getBasePath?: () => string | null | undefined;
  getExternalEditor: () => ExternalEditorLaunchSettings;
  onOpenError?: (message: string) => void;
  openFile?: (path: string, editor: ExternalEditorLaunchSettings) => Promise<void>;
  openUrl?: (url: string) => Promise<void>;
};

const URL_PATTERN = /\bhttps?:\/\/[^\s<>"'`]+/gi;
const FILE_PATTERN =
  /file:\/\/\/?[^\s<>"'`]+|[A-Za-z]:[\\/][^\s<>"'`]+|\\\\[^\s<>"'`]+|(?:\/|~[\\/]|\.{1,2}[\\/])[^\s<>"'`]+|(?:[A-Za-z0-9_@+~.-]+[\\/])+[^\s<>"'`]+|[A-Za-z0-9_@+~-]+\.[A-Za-z0-9]{1,16}(?::\d+(?::\d+)?)?/g;
const KNOWN_FILE_EXTENSIONS = new Set([
  "bat",
  "c",
  "cjs",
  "cmd",
  "cpp",
  "css",
  "env",
  "go",
  "h",
  "hpp",
  "html",
  "java",
  "js",
  "json",
  "jsx",
  "lock",
  "md",
  "mjs",
  "ps1",
  "py",
  "rs",
  "scss",
  "sh",
  "sql",
  "svelte",
  "toml",
  "ts",
  "tsx",
  "txt",
  "vue",
  "xml",
  "yaml",
  "yml",
]);

function defaultOpenFile(path: string, editor: ExternalEditorLaunchSettings) {
  return invoke("open_in_external_editor", { path, editor }) as Promise<void>;
}

function defaultOpenUrl(url: string) {
  return openUrl(url);
}

function trimTerminalToken(token: string) {
  let trimmed = token;
  while (/[.,;!?]$/.test(trimmed)) {
    trimmed = trimmed.slice(0, -1);
  }
  while (/[\])}]$/.test(trimmed)) {
    const last = trimmed[trimmed.length - 1];
    const opener = last === ")" ? "(" : last === "]" ? "[" : "{";
    if (trimmed.includes(opener)) {
      break;
    }
    trimmed = trimmed.slice(0, -1);
  }
  return trimmed;
}

function stripLocationSuffix(path: string) {
  const match = path.match(/^(.+?)(?::\d+){1,2}$/);
  return match?.[1] ?? path;
}

function pathWithoutFileScheme(path: string) {
  if (!path.toLowerCase().startsWith("file://")) {
    return path;
  }
  const withoutScheme = path.replace(/^file:\/\//i, "");
  const normalized = withoutScheme.startsWith("/") && /^[A-Za-z]:/.test(withoutScheme.slice(1))
    ? withoutScheme.slice(1)
    : withoutScheme;
  try {
    return decodeURIComponent(normalized);
  } catch {
    return normalized;
  }
}

function isAbsolutePath(path: string) {
  return /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\") || path.startsWith("/") || path.startsWith("~");
}

function normalizePathSegments(path: string, separator: "\\" | "/") {
  const isWindows = separator === "\\";
  const normalized = path.replace(/[\\/]+/g, separator);
  const prefixMatch = isWindows
    ? normalized.match(/^([A-Za-z]:|\\\\[^\\]+\\[^\\]+)(?:\\|$)/)
    : normalized.match(/^(\/)/);
  const prefix = prefixMatch?.[1] ?? "";
  const remainder = prefix ? normalized.slice(prefix.length) : normalized;
  const rest = prefix && remainder.startsWith(separator) ? remainder.slice(1) : remainder;
  const out: string[] = [];

  for (const part of rest.split(separator)) {
    if (!part || part === ".") {
      continue;
    }
    if (part === "..") {
      if (out.length > 0) {
        out.pop();
      }
      continue;
    }
    out.push(part);
  }

  if (!prefix) {
    return out.join(separator);
  }
  if (prefix === "/") {
    return `${prefix}${out.join(separator)}`;
  }
  return out.length > 0 ? `${prefix}${separator}${out.join(separator)}` : prefix;
}

function resolveFileTarget(path: string, basePath?: string | null) {
  const target = stripLocationSuffix(pathWithoutFileScheme(trimTerminalToken(path)));
  if (!basePath || isAbsolutePath(target)) {
    return target;
  }

  const separator = /^[A-Za-z]:/.test(basePath) || basePath.includes("\\") ? "\\" : "/";
  const normalizedBase = basePath.replace(/[\\/]+$/, "");
  return normalizePathSegments(`${normalizedBase}${separator}${target}`, separator);
}

function fileExtension(path: string) {
  const withoutLocation = stripLocationSuffix(path);
  const fileName = withoutLocation.split(/[\\/]/).pop() ?? withoutLocation;
  const extension = fileName.includes(".") ? fileName.split(".").pop()?.toLowerCase() : null;
  return extension ?? null;
}

function isLikelyFilePath(path: string) {
  const normalized = stripLocationSuffix(pathWithoutFileScheme(trimTerminalToken(path)));
  if (normalized.toLowerCase().startsWith("http://") || normalized.toLowerCase().startsWith("https://")) {
    return false;
  }
  if (normalized.startsWith("file://") || /^[A-Za-z]:[\\/]/.test(normalized) || normalized.startsWith("\\\\") || normalized.startsWith("/") || normalized.startsWith("~")) {
    return true;
  }
  if (normalized.includes("/") || normalized.includes("\\")) {
    return true;
  }
  const extension = fileExtension(normalized);
  return extension !== null && KNOWN_FILE_EXTENSIONS.has(extension);
}

function overlapsExistingLink(startIndex: number, length: number, links: TerminalDetectedLink[]) {
  const endIndex = startIndex + length;
  return links.some((link) => {
    const linkEnd = link.startIndex + link.text.length;
    return startIndex < linkEnd && endIndex > link.startIndex;
  });
}

export function findTerminalLinks(line: string, basePath?: string | null): TerminalDetectedLink[] {
  const links: TerminalDetectedLink[] = [];

  for (const match of line.matchAll(URL_PATTERN)) {
    const rawText = match[0];
    const text = trimTerminalToken(rawText);
    links.push({
      kind: "url",
      text,
      target: text,
      startIndex: match.index ?? 0,
    });
  }

  for (const match of line.matchAll(FILE_PATTERN)) {
    const rawText = match[0];
    const text = trimTerminalToken(rawText);
    const startIndex = match.index ?? 0;
    if (!text || overlapsExistingLink(startIndex, text.length, links) || !isLikelyFilePath(text)) {
      continue;
    }
    links.push({
      kind: "file",
      text,
      target: resolveFileTarget(text, basePath),
      startIndex,
    });
  }

  return links.sort((a, b) => a.startIndex - b.startIndex);
}

export function installTerminalLinkProvider(term: Terminal, options: TerminalLinkProviderOptions) {
  return term.registerLinkProvider({
    provideLinks(bufferLineNumber, callback) {
      const line = term.buffer.active.getLine(bufferLineNumber - 1)?.translateToString(true);
      if (!line) {
        callback(undefined);
        return;
      }

      const links = findTerminalLinks(line, options.getBasePath?.()).map<ILink>((link) => ({
        range: {
          start: { x: link.startIndex + 1, y: bufferLineNumber },
          end: { x: link.startIndex + link.text.length, y: bufferLineNumber },
        },
        text: link.text,
        decorations: {
          pointerCursor: true,
          underline: true,
        },
        activate: () => {
          const action = link.kind === "url"
            ? (options.openUrl ?? defaultOpenUrl)(link.target)
            : (options.openFile ?? defaultOpenFile)(link.target, options.getExternalEditor());
          action.catch((error) => {
            options.onOpenError?.(`Failed to open terminal link: ${String(error)}`);
          });
        },
      }));

      callback(links.length > 0 ? links : undefined);
    },
  });
}
