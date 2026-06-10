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

export const LIGHT_TERM_THEME: WardianTerminalTheme = {
  background: "#fcfaf5",
  foreground: "#111827",
  cursor: "#b8860b",
  selectionBackground: "#e5e7eb",
};
