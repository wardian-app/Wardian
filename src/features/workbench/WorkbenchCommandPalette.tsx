import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";

import type {
  WorkbenchCommandAction,
  WorkbenchCommandId,
} from "./useWorkbenchCommands";
import "./workbench-command-palette.css";

export type WorkbenchCommandPaletteProps = {
  open: boolean;
  actions: readonly WorkbenchCommandAction[];
  is_enabled: (commandId: WorkbenchCommandId) => boolean;
  on_execute: (commandId: WorkbenchCommandId) => Promise<boolean>;
  on_close: () => void;
};

type ScoredAction = {
  action: WorkbenchCommandAction;
  score: number;
};

function fuzzyScore(value: string, rawQuery: string): number | null {
  const candidate = value.toLocaleLowerCase();
  const query = rawQuery.trim().toLocaleLowerCase();
  if (!query) return 0;
  const contiguous = candidate.indexOf(query);
  if (contiguous >= 0) return contiguous;

  let cursor = 0;
  let score = 100;
  let previousMatch = -1;
  for (const character of query) {
    const match = candidate.indexOf(character, cursor);
    if (match < 0) return null;
    score += match - cursor;
    if (previousMatch >= 0 && match === previousMatch + 1) score -= 2;
    previousMatch = match;
    cursor = match + 1;
  }
  return score;
}

export function filterWorkbenchCommands(
  actions: readonly WorkbenchCommandAction[],
  query: string,
): readonly WorkbenchCommandAction[] {
  return actions
    .filter((action) => action.command_id !== "workbench.command_palette")
    .map((action): ScoredAction | null => {
      const score = fuzzyScore(`${action.title} ${action.command_id}`, query);
      return score === null ? null : { action, score };
    })
    .filter((entry): entry is ScoredAction => entry !== null)
    .sort((left, right) => left.score - right.score || left.action.title.localeCompare(right.action.title))
    .map(({ action }) => action);
}

export function formatWorkbenchShortcut(shortcut: string): string {
  const isMac = typeof navigator !== "undefined"
    && /Mac|iPhone|iPad/.test(navigator.platform);
  return shortcut.replace("Mod", isMac ? "⌘" : "Ctrl");
}

export function WorkbenchCommandPalette({
  open,
  actions,
  is_enabled: isEnabled,
  on_execute: onExecute,
  on_close: onClose,
}: WorkbenchCommandPaletteProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const [query, setQuery] = useState("");
  const filteredActions = useMemo(
    () => filterWorkbenchCommands(actions, query),
    [actions, query],
  );
  const enabledIndexes = filteredActions
    .map((action, index) => isEnabled(action.command_id) ? index : -1)
    .filter((index) => index >= 0);
  const enabledIndexKey = enabledIndexes.join(",");
  const [selectedIndex, setSelectedIndex] = useState(-1);

  useEffect(() => {
    if (!open) return;
    returnFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    setQuery("");
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  useEffect(() => {
    if (!open) return;
    setSelectedIndex(enabledIndexes[0] ?? -1);
  }, [enabledIndexKey, filteredActions.length, open, query]);

  const requestClose = useCallback(() => {
    onClose();
    requestAnimationFrame(() => returnFocusRef.current?.focus());
  }, [onClose]);

  const execute = useCallback(async (action: WorkbenchCommandAction) => {
    if (!isEnabled(action.command_id)) return;
    if (await onExecute(action.command_id)) requestClose();
  }, [isEnabled, onExecute, requestClose]);

  const moveSelection = (delta: number): void => {
    if (enabledIndexes.length === 0) return;
    const current = enabledIndexes.indexOf(selectedIndex);
    const next = current < 0
      ? (delta > 0 ? 0 : enabledIndexes.length - 1)
      : (current + delta + enabledIndexes.length) % enabledIndexes.length;
    setSelectedIndex(enabledIndexes[next]);
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>): void => {
    if (event.key === "Escape") {
      event.preventDefault();
      requestClose();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key === "Enter" && selectedIndex >= 0) {
      event.preventDefault();
      const action = filteredActions[selectedIndex];
      if (action) void execute(action);
    }
  };

  const handleBackdropMouseDown = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (event.target === event.currentTarget) requestClose();
  };

  if (!open) return null;

  return (
    <div className="wardian-command-palette-backdrop" onMouseDown={handleBackdropMouseDown}>
      <section
        role="dialog"
        aria-modal="true"
        aria-label="Command Palette"
        className="wardian-command-palette"
      >
        <input
          ref={inputRef}
          role="combobox"
          aria-label="Search commands"
          aria-autocomplete="list"
          aria-controls="wardian-command-palette-results"
          aria-expanded="true"
          aria-activedescendant={selectedIndex >= 0
            ? `wardian-command-${filteredActions[selectedIndex]?.command_id.replace(/\./g, "-")}`
            : undefined}
          value={query}
          placeholder="Type a command"
          autoComplete="off"
          spellCheck="false"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        <div
          id="wardian-command-palette-results"
          role="listbox"
          aria-label="Commands"
          className="wardian-command-palette-results"
        >
          {filteredActions.map((action, index) => {
            const enabled = isEnabled(action.command_id);
            const selected = index === selectedIndex;
            return (
              <button
                id={`wardian-command-${action.command_id.replace(/\./g, "-")}`}
                key={action.command_id}
                type="button"
                role="option"
                aria-selected={selected}
                aria-disabled={!enabled}
                disabled={!enabled}
                className="wardian-command-palette-option"
                onMouseEnter={() => { if (enabled) setSelectedIndex(index); }}
                onClick={() => { void execute(action); }}
              >
                <span>{action.title}</span>
                {action.shortcut && <kbd>{formatWorkbenchShortcut(action.shortcut)}</kbd>}
              </button>
            );
          })}
          {filteredActions.length === 0 && (
            <p className="wardian-command-palette-empty">No matching commands</p>
          )}
        </div>
      </section>
    </div>
  );
}
