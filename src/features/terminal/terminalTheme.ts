import type { ITheme } from "@xterm/xterm";

export type TerminalThemeMode = "dark" | "light";

export const TERMINAL_LINE_HEIGHT = 1.25;

export const DARK_TERMINAL_THEME: Required<Pick<
  ITheme,
  | "background"
  | "foreground"
  | "cursor"
  | "selectionBackground"
  | "black"
  | "red"
  | "green"
  | "yellow"
  | "blue"
  | "magenta"
  | "cyan"
  | "white"
  | "brightBlack"
  | "brightRed"
  | "brightGreen"
  | "brightYellow"
  | "brightBlue"
  | "brightMagenta"
  | "brightCyan"
  | "brightWhite"
>> = {
  background: "#08100d",
  foreground: "#edf3ea",
  cursor: "#e6c76b",
  selectionBackground: "#263729",
  black: "#27302a",
  red: "#ff6b66",
  green: "#69d489",
  yellow: "#e7c15d",
  blue: "#7ab7ff",
  magenta: "#d69cff",
  cyan: "#6fd4e6",
  white: "#edf3ea",
  brightBlack: "#748176",
  brightRed: "#ff8b86",
  brightGreen: "#8de3a3",
  brightYellow: "#f0d77b",
  brightBlue: "#9bc8ff",
  brightMagenta: "#e3b6ff",
  brightCyan: "#91e0ec",
  brightWhite: "#fbfff8",
};

export const LIGHT_TERMINAL_THEME: typeof DARK_TERMINAL_THEME = {
  background: "#fcfaf5",
  foreground: "#111827",
  cursor: "#926a09",
  selectionBackground: "#e8dcc0",
  black: "#111827",
  red: "#b91c1c",
  green: "#047857",
  yellow: "#926a09",
  blue: "#1d4ed8",
  magenta: "#7e22ce",
  cyan: "#0e7490",
  white: "#f9fafb",
  brightBlack: "#6b7280",
  brightRed: "#dc2626",
  brightGreen: "#059669",
  brightYellow: "#b45309",
  brightBlue: "#2563eb",
  brightMagenta: "#9333ea",
  brightCyan: "#0891b2",
  brightWhite: "#ffffff",
};

const TERMINAL_THEME_VARS: Record<keyof typeof DARK_TERMINAL_THEME, string> = {
  background: "--color-wardian-terminal-bg",
  foreground: "--color-wardian-terminal-fg",
  cursor: "--color-wardian-terminal-cursor",
  selectionBackground: "--color-wardian-terminal-selection",
  black: "--color-wardian-terminal-black",
  red: "--color-wardian-terminal-red",
  green: "--color-wardian-terminal-green",
  yellow: "--color-wardian-terminal-yellow",
  blue: "--color-wardian-terminal-blue",
  magenta: "--color-wardian-terminal-magenta",
  cyan: "--color-wardian-terminal-cyan",
  white: "--color-wardian-terminal-white",
  brightBlack: "--color-wardian-terminal-bright-black",
  brightRed: "--color-wardian-terminal-bright-red",
  brightGreen: "--color-wardian-terminal-bright-green",
  brightYellow: "--color-wardian-terminal-bright-yellow",
  brightBlue: "--color-wardian-terminal-bright-blue",
  brightMagenta: "--color-wardian-terminal-bright-magenta",
  brightCyan: "--color-wardian-terminal-bright-cyan",
  brightWhite: "--color-wardian-terminal-bright-white",
};

function readThemeVar(name: string, mode: TerminalThemeMode) {
  if (typeof document === "undefined") {
    return "";
  }
  if (document.documentElement.getAttribute("data-theme") !== mode) {
    return "";
  }
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export function terminalThemeForMode(mode: TerminalThemeMode): ITheme {
  const fallback = mode === "light" ? LIGHT_TERMINAL_THEME : DARK_TERMINAL_THEME;
  return Object.fromEntries(
    Object.entries(TERMINAL_THEME_VARS).map(([key, varName]) => [
      key,
      readThemeVar(varName, mode) || fallback[key as keyof typeof fallback],
    ]),
  ) as typeof DARK_TERMINAL_THEME;
}

export function terminalRgbTriplet(value: string | undefined, fallback: string) {
  const cleaned = String(value ?? "").replace("#", "");
  return cleaned.length === 6
    ? `${cleaned.slice(0, 2)}/${cleaned.slice(2, 4)}/${cleaned.slice(4, 6)}`
    : fallback;
}
