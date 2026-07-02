/**
 * Shared color resolution and transformation utilities for graph rendering.
 * Used by both Sigma-based graph rendering (GraphCanvas) and Canvas2D overlays (EdgeActivityOverlay).
 */

/**
 * Resolve CSS variable references to actual colors.
 * If the color string is a CSS var(), extract the variable name and look up its computed value.
 * Optionally supports a fallback color after a comma in the var() syntax.
 *
 * @param color - A CSS color value (e.g., "var(--color-wardian-accent)", "#10b981", "rgb(16, 185, 129)")
 * @param container - A DOM element to use for computing CSS variable values
 * @returns The resolved color value, or the original color if it can't be resolved
 */
export function resolveGraphColor(color: string, container: HTMLElement): string {
  const match = color.match(/^var\((--[^,\s)]+)(?:,\s*([^)]+))?\)$/);
  if (!match) return color;

  const computed = container.ownerDocument.defaultView
    ?.getComputedStyle(container.ownerDocument.documentElement)
    .getPropertyValue(match[1])
    .trim();

  return computed || match[2]?.trim() || color;
}

/**
 * Convert a hex or rgba color to rgba with the given alpha value.
 * Handles hex (#RRGGBB), rgb(), and rgba() formats.
 *
 * @param color - A color value in hex or rgb(a) format
 * @param alpha - Alpha channel value (0..1)
 * @returns An rgba() color string
 */
export function withAlpha(color: string, alpha: number): string {
  const hex = color.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const value = hex[1];
    const r = Number.parseInt(value.slice(0, 2), 16);
    const g = Number.parseInt(value.slice(2, 4), 16);
    const b = Number.parseInt(value.slice(4, 6), 16);
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }

  const rgb = color.match(/^rgba?\(\s*([0-9]+)\s*,\s*([0-9]+)\s*,\s*([0-9]+)/i);
  if (rgb) return `rgba(${rgb[1]}, ${rgb[2]}, ${rgb[3]}, ${alpha})`;

  return color;
}
