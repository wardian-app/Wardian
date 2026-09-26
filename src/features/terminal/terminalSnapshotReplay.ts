import type { TerminalSnapshot } from "../../types";

export type TerminalSnapshotReplay =
  | { kind: "formatted"; text: string }
  | { kind: "degraded"; reason: "missing_formatted_state" | "invalid_formatted_state"; text: string };

/** Reconstruct broker history before the absolute frame at snapshot geometry. */
export function decodeTerminalSnapshot(snapshot: TerminalSnapshot): TerminalSnapshotReplay {
  const scrollback = snapshot.formatted_scrollback?.length === snapshot.scrollback.length
    ? snapshot.formatted_scrollback
    : snapshot.scrollback;
  if (snapshot.terminal_state_base64) {
    try {
      const binary = atob(snapshot.terminal_state_base64);
      const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
      const state = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      if (!state) throw new Error("Empty formatted terminal state");
      if (scrollback.length === 0) return { kind: "formatted", text: state };
      // Home + erase-display in the absolute frame would erase history that
      // is still on the visible screen. Advance one screenful from the last
      // history row first, moving every supplied row (including empty rows)
      // into actual scrollback. The frame then restores the grid and cursor.
      return { kind: "formatted", text: scrollback.join("\r\n") + "\r\n".repeat(snapshot.geometry.rows) + state };
    } catch {
      return degradedSnapshot(snapshot, scrollback, "invalid_formatted_state");
    }
  }
  return degradedSnapshot(snapshot, scrollback, "missing_formatted_state");
}

function degradedSnapshot(
  snapshot: TerminalSnapshot,
  scrollback: string[],
  reason: "missing_formatted_state" | "invalid_formatted_state",
): TerminalSnapshotReplay {
  const plainProjection = [...scrollback, snapshot.visible_grid].join("\r\n");
  const text = snapshot.alternate_screen ? `\x1b[?1049h${plainProjection}` : plainProjection;
  return { kind: "degraded", reason, text };
}
