import { describe, expect, it } from "vitest";
import { DARK_TERMINAL_THEME, terminalRgbTriplet, terminalThemeForMode } from "./terminalTheme";

function hexToRgb(hex: string) {
  const cleaned = hex.replace("#", "");
  return {
    r: Number.parseInt(cleaned.slice(0, 2), 16),
    g: Number.parseInt(cleaned.slice(2, 4), 16),
    b: Number.parseInt(cleaned.slice(4, 6), 16),
  };
}

function channelLuminance(value: number) {
  const normalized = value / 255;
  return normalized <= 0.03928
    ? normalized / 12.92
    : ((normalized + 0.055) / 1.055) ** 2.4;
}

function contrastRatio(foreground: string, background: string) {
  const fg = hexToRgb(foreground);
  const bg = hexToRgb(background);
  const fgLum =
    channelLuminance(fg.r) * 0.2126 +
    channelLuminance(fg.g) * 0.7152 +
    channelLuminance(fg.b) * 0.0722;
  const bgLum =
    channelLuminance(bg.r) * 0.2126 +
    channelLuminance(bg.g) * 0.7152 +
    channelLuminance(bg.b) * 0.0722;
  const lighter = Math.max(fgLum, bgLum);
  const darker = Math.min(fgLum, bgLum);
  return (lighter + 0.05) / (darker + 0.05);
}

describe("terminalTheme", () => {
  it("keeps dark terminal semantic colors readable against the terminal background", () => {
    const colors = [
      DARK_TERMINAL_THEME.foreground,
      DARK_TERMINAL_THEME.red,
      DARK_TERMINAL_THEME.green,
      DARK_TERMINAL_THEME.yellow,
      DARK_TERMINAL_THEME.cyan,
      DARK_TERMINAL_THEME.brightBlack,
    ];

    colors.forEach((color) => {
      expect(contrastRatio(color, DARK_TERMINAL_THEME.background)).toBeGreaterThanOrEqual(4.5);
    });
  });

  it("falls back to the dark terminal theme when CSS variables are unavailable", () => {
    expect(terminalThemeForMode("dark")).toMatchObject(DARK_TERMINAL_THEME);
  });

  it("formats xterm OSC color replies as slash-delimited RGB triplets", () => {
    expect(terminalRgbTriplet("#08100d", "00/00/00")).toBe("08/10/0d");
    expect(terminalRgbTriplet(undefined, "00/00/00")).toBe("00/00/00");
  });
});
