const ALLOWED_PROTOCOLS = new Set(["http:", "https:", "file:"]);

export function safeMarkdownUrl(rawUrl: string | undefined): string | null {
  if (!rawUrl) return null;
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
