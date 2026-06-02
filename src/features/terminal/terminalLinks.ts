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
  validateFile?: (path: string) => Promise<boolean>;
};

const URL_PATTERN = /\bhttps?:\/\/[^\s<>"'`]+/gi;
const FILE_PATTERN =
  /file:\/\/\/?[^\s<>"'`]+|[A-Za-z]:[\\/][^\s<>"'`]+|\\\\[^\s<>"'`]+|(?:\/|~[\\/]|\.{1,2}[\\/])[^\s<>"'`]+|(?:[A-Za-z0-9_@+~.-]+[\\/])+[^\s<>"'`]+|[A-Za-z0-9_@+~-]+\.[A-Za-z0-9]{1,16}(?::\d+(?::\d+)?)?/g;
const MAX_LINE_LENGTH = 2000;
const MAX_LINK_LENGTH = 1024;
const MAX_RESOLVED_FILE_LINKS = 10;
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

function defaultValidateFile(path: string) {
  return invoke<boolean>("terminal_link_target_exists", { path });
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
  const visualStudioMatch = path.match(/^(.+?)\(\d+(?:,\s*\d+)?\)$/);
  if (visualStudioMatch) {
    return visualStudioMatch[1];
  }
  const match = path.match(/^(.+?)(?::\d+){1,2}$/);
  return match?.[1] ?? path;
}

function normalizeFileToken(token: string) {
  let normalized = trimTerminalToken(token);
  const compilerSuffix = normalized.match(/^(.+\(\d+(?:,\s*\d+)?\)):/);
  if (compilerSuffix) {
    normalized = compilerSuffix[1];
  }
  return normalized;
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
  const target = stripLocationSuffix(pathWithoutFileScheme(normalizeFileToken(path)));
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
  const token = normalizeFileToken(path);
  if (token.toLowerCase().startsWith("file://")) {
    return true;
  }
  const normalized = stripLocationSuffix(pathWithoutFileScheme(token));
  if (normalized.toLowerCase().startsWith("http://") || normalized.toLowerCase().startsWith("https://")) {
    return false;
  }
  const extension = fileExtension(normalized);
  return extension !== null && KNOWN_FILE_EXTENSIONS.has(extension);
}

function hasStrongPathSignal(path: string) {
  return isLikelyFilePath(path);
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
  if (line.length > MAX_LINE_LENGTH) {
    return links;
  }

  for (const match of line.matchAll(URL_PATTERN)) {
    const rawText = match[0];
    const text = trimTerminalToken(rawText);
    if (text.length > MAX_LINK_LENGTH) {
      continue;
    }
    links.push({
      kind: "url",
      text,
      target: text,
      startIndex: match.index ?? 0,
    });
  }

  for (const match of line.matchAll(FILE_PATTERN)) {
    const rawText = match[0];
    const text = normalizeFileToken(rawText);
    const startIndex = match.index ?? 0;
    if (!text || text.length > MAX_LINK_LENGTH || overlapsExistingLink(startIndex, text.length, links) || !isLikelyFilePath(text)) {
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

export async function findValidatedTerminalLinks(
  line: string,
  basePath: string | null | undefined,
  validateFile: (path: string) => Promise<boolean>,
): Promise<TerminalDetectedLink[]> {
  const parsedLinks = findTerminalLinks(line, basePath);
  const links: TerminalDetectedLink[] = [];
  let resolvedFileLinks = 0;
  let resolvedFileAttempts = 0;

  for (const link of parsedLinks) {
    if (link.kind === "url") {
      links.push(link);
      continue;
    }
    if (resolvedFileAttempts >= MAX_RESOLVED_FILE_LINKS) {
      return links.sort((a, b) => a.startIndex - b.startIndex);
    }
    resolvedFileAttempts++;
    if (await validateFile(link.target)) {
      links.push(link);
      resolvedFileLinks++;
      if (resolvedFileLinks >= MAX_RESOLVED_FILE_LINKS) {
        return links.sort((a, b) => a.startIndex - b.startIndex);
      }
    }
  }

  if (line.length > MAX_LINE_LENGTH || resolvedFileLinks >= MAX_RESOLVED_FILE_LINKS) {
    return links;
  }

  for (const match of line.matchAll(FILE_PATTERN)) {
    const text = normalizeFileToken(match[0]);
    const startIndex = match.index ?? 0;
    if (
      !text ||
      text.length > MAX_LINK_LENGTH ||
      hasStrongPathSignal(text) ||
      overlapsExistingLink(startIndex, text.length, links)
    ) {
      continue;
    }

    const target = resolveFileTarget(text, basePath);
    if (resolvedFileAttempts >= MAX_RESOLVED_FILE_LINKS) {
      break;
    }
    resolvedFileAttempts++;
    if (await validateFile(target)) {
      links.push({
        kind: "file",
        text,
        target,
        startIndex,
      });
      resolvedFileLinks++;
      if (resolvedFileLinks >= MAX_RESOLVED_FILE_LINKS) {
        break;
      }
    }
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

      findValidatedTerminalLinks(line, options.getBasePath?.(), options.validateFile ?? defaultValidateFile)
        .then((links) => links.map<ILink>((link) => ({
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
        })))
        .then((links) => {
          callback(links.length > 0 ? links : undefined);
        })
        .catch((error) => {
          options.onOpenError?.(`Failed to resolve terminal links: ${String(error)}`);
          callback(undefined);
        });
    },
  });
}
