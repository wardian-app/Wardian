import { findTerminalLinks } from "../../terminal/terminalLinks";

const ALLOWED_PROTOCOLS = new Set(["http:", "https:", "file:"]);
const EXPLICIT_PROTOCOL_PATTERN = /^[A-Za-z][A-Za-z0-9+.-]*:/;
const RELATIVE_PATH_PATTERN = /^(?:\.{1,2}[\\/]|~[\\/]|[A-Za-z0-9_@+~.-]+[\\/])/;

function isPathLikeMarkdownTarget(rawUrl: string) {
  if (/^[A-Za-z]:[\\/]/.test(rawUrl)) {
    return true;
  }
  if (RELATIVE_PATH_PATTERN.test(rawUrl)) {
    return true;
  }
  if (findTerminalLinks(rawUrl).some((link) => link.kind === "file" && link.startIndex === 0 && link.text === rawUrl)) {
    return true;
  }
  if (EXPLICIT_PROTOCOL_PATTERN.test(rawUrl)) {
    return false;
  }
  return false;
}

export function safeMarkdownUrl(rawUrl: string | undefined): string | null {
  if (!rawUrl) return null;
  if (isPathLikeMarkdownTarget(rawUrl)) return rawUrl;
  try {
    const url = new URL(rawUrl, window.location.href);
    return ALLOWED_PROTOCOLS.has(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

export function markdownUrlTransform(rawUrl: string): string {
  return safeMarkdownUrl(rawUrl) ?? "";
}
