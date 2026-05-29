type TerminalShortcutTarget = {
  attachCustomKeyEventHandler: (handler: (event: KeyboardEvent) => boolean) => void;
  selectAll: () => void;
};

type TerminalShortcutOptions = {
  platform?: string;
};

export function isMacPlatform(platform = navigator.platform) {
  return platform.toLowerCase().startsWith("mac");
}

export function installConservativeTerminalShortcuts(
  terminal: TerminalShortcutTarget,
  options: TerminalShortcutOptions = {},
) {
  const isMac = isMacPlatform(options.platform);

  terminal.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") {
      return true;
    }

    if (isMac && event.metaKey && !event.ctrlKey && !event.altKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      event.stopPropagation();
      terminal.selectAll();
      return false;
    }

    return true;
  });
}
