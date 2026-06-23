const VAR_PATTERN = /var\((--[^),]+)\)/;

/** Resolve a CSS custom property to a concrete color so Konva can paint it. */
export function resolveCssVar(value: string, fallback = "#94a3b8"): string {
  const match = value.match(VAR_PATTERN);
  if (!match) return value;
  if (typeof document === "undefined") return fallback;
  const resolved = getComputedStyle(document.documentElement)
    .getPropertyValue(match[1])
    .trim();
  return resolved || fallback;
}
