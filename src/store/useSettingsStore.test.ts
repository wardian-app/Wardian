import { describe, expect, it } from 'vitest';
import {
  defaultTerminalFontFamily,
  defaultTerminalFontSize,
  LINUX_TERMINAL_FONT_FAMILY,
  MACOS_TERMINAL_FONT_FAMILY,
  WINDOWS_TERMINAL_FONT_FAMILY,
} from './useSettingsStore';

describe('terminal appearance defaults', () => {
  it('matches VS Code font family defaults by platform', () => {
    expect(defaultTerminalFontFamily('windows')).toBe(WINDOWS_TERMINAL_FONT_FAMILY);
    expect(defaultTerminalFontFamily('macos')).toBe(MACOS_TERMINAL_FONT_FAMILY);
    expect(defaultTerminalFontFamily('linux')).toBe(LINUX_TERMINAL_FONT_FAMILY);
  });

  it('matches VS Code terminal font size defaults by platform', () => {
    expect(defaultTerminalFontSize('macos')).toBe(12);
    expect(defaultTerminalFontSize('windows')).toBe(14);
    expect(defaultTerminalFontSize('linux')).toBe(14);
  });
});
