/**
 * Shared xterm themes for all Wardian terminals (agent panes and the user
 * terminal). Exported as singletons so identity comparisons against the
 * active theme (e.g. `termTheme === LIGHT_TERM_THEME`) keep working.
 *
 * The dark ANSI ramp is tuned for legibility on the #1a1a1a surface: TUIs
 * such as Claude Code and Antigravity render most body text in dim/bright-
 * black tones, so those entries sit well above xterm's defaults.
 */

import type { ITheme } from "@xterm/xterm";

/** xterm theme with the core colors Wardian always reads made non-optional. */
export type WardianTerminalTheme = ITheme & {
  background: string;
  foreground: string;
  cursor: string;
  selectionBackground: string;
};

/**
 * Matches VS Code's `terminal.integrated.minimumContrastRatio` default.
 * TUIs (Claude Code, Antigravity) emit truecolor/256-color dim grays that no
 * 16-color theme can reach; xterm lightens any foreground that falls below
 * this ratio against the background.
 */
export const TERMINAL_MINIMUM_CONTRAST_RATIO = 4.5;

/**
 * Antigravity renders nearly all body text in dim grays, so the 4.5 floor
 * that suits Claude Code still reads murky there; lift it to AAA (7:1).
 */
export function terminalMinimumContrastRatio(provider?: string): number {
  return provider === "antigravity" ? 7 : TERMINAL_MINIMUM_CONTRAST_RATIO;
}

export const DARK_TERM_THEME: WardianTerminalTheme = {
  background: "#1a1a1a",
  foreground: "#ebebeb",
  cursor: "#f2c14e",
  selectionBackground: "#3d3d3d",
  black: "#2e2e2e",
  red: "#ef6363",
  green: "#3dd68c",
  yellow: "#f5c84c",
  blue: "#5caeef",
  magenta: "#d38aea",
  cyan: "#39c5cf",
  white: "#d8d8d8",
  brightBlack: "#808080",
  brightRed: "#ff8080",
  brightGreen: "#56e8a8",
  brightYellow: "#ffd76b",
  brightBlue: "#82c4ff",
  brightMagenta: "#e6a6f2",
  brightCyan: "#5ee0ea",
  brightWhite: "#ffffff",
};

export const ANTIGRAVITY_DARK_TERM_THEME: WardianTerminalTheme = {
  ...DARK_TERM_THEME,
  foreground: "#c9d1d9",
};

export const LIGHT_TERM_THEME: WardianTerminalTheme = {
  background: "#fcfaf5",
  foreground: "#111827",
  cursor: "#b8860b",
  selectionBackground: "#e5e7eb",
};

export function terminalThemeForProvider(
  theme: "dark" | "light",
  provider?: string,
): WardianTerminalTheme {
  if (theme === "light") {
    return LIGHT_TERM_THEME;
  }
  return provider === "antigravity" ? ANTIGRAVITY_DARK_TERM_THEME : DARK_TERM_THEME;
}
