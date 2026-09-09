import type { TerminalSnapshot } from "../../types";

/** Reconstruct broker history before an absolute visible-grid/cursor restore.
 * Called after resetting xterm, at the snapshot geometry for formatted state.
 */
export function decodeTerminalSnapshot(snapshot: TerminalSnapshot, useFormattedState: boolean) {
  const scrollback = snapshot.formatted_scrollback?.length === snapshot.scrollback.length
    ? snapshot.formatted_scrollback
    : snapshot.scrollback;
  if (useFormattedState && snapshot.terminal_state_base64) {
    try {
      const binary = atob(snapshot.terminal_state_base64);
      const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
      const state = new TextDecoder().decode(bytes);
      if (scrollback.length === 0) return state;
      // Home + erase-display in the absolute frame would erase history that
      // is still on the visible screen. Advance one screenful from the last
      // history row first, moving every supplied row (including empty rows)
      // into actual scrollback. The frame then restores the grid and cursor.
      return scrollback.join("\r\n") + "\r\n".repeat(snapshot.geometry.rows) + state;
    } catch {
      // A size-capped snapshot may omit or truncate the formatted state. The
      // bounded plain-text projection is the recovery fallback.
    }
  }
  return [...scrollback, snapshot.visible_grid].join("\r\n");
}
