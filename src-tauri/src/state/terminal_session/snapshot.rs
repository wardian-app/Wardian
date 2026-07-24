use base64::Engine;
use wardian_core::models::{TerminalGeometry, TerminalSnapshot};

pub(super) const SNAPSHOT_SCROLLBACK_LINES: usize = 1_000;
pub(super) const MAX_SNAPSHOT_BYTES: usize = 2 * 1_024 * 1_024;

pub(super) fn build_snapshot(
    session_id: &str,
    runtime_generation: u64,
    sequence_barrier: u64,
    geometry: TerminalGeometry,
    screen: &vt100::Screen,
    snapshot_number: u64,
) -> TerminalSnapshot {
    let visible_grid = screen.contents();
    let (scrollback, formatted_scrollback) = snapshot_scrollback(screen);
    let mut snapshot = TerminalSnapshot {
        snapshot_id: format!(
            "terminal-snapshot-{runtime_generation}-{sequence_barrier}-{snapshot_number}"
        ),
        session_id: session_id.to_string(),
        runtime_generation,
        sequence_barrier,
        geometry,
        terminal_state_base64: base64::engine::general_purpose::STANDARD
            .encode(screen.state_formatted()),
        visible_grid,
        scrollback,
        formatted_scrollback,
    };
    enforce_serialized_limit(&mut snapshot);
    snapshot
}

fn snapshot_scrollback(screen: &vt100::Screen) -> (Vec<String>, Vec<String>) {
    let mut view = screen.clone();
    view.set_scrollback(SNAPSHOT_SCROLLBACK_LINES);
    let retained = view.scrollback().min(SNAPSHOT_SCROLLBACK_LINES);
    let mut lines = Vec::with_capacity(retained);
    let mut formatted_lines = Vec::with_capacity(retained);
    for offset in (1..=retained).rev() {
        view.set_scrollback(offset);
        lines.push(view.rows(0, view.size().1).next().unwrap_or_default());
        // `rows_formatted` emits row-local SGR/control sequences rather than a
        // geometry-bound screen paint. Appending a newline after each entry is
        // therefore safe when a client restores into a differently sized card.
        formatted_lines.push(
            String::from_utf8(
                view.rows_formatted(0, view.size().1)
                    .next()
                    .unwrap_or_default(),
            )
            .unwrap_or_default(),
        );
    }
    (lines, formatted_lines)
}

fn enforce_serialized_limit(snapshot: &mut TerminalSnapshot) {
    while serialized_len(snapshot) > MAX_SNAPSHOT_BYTES && !snapshot.scrollback.is_empty() {
        let remove_count = snapshot.scrollback.len().min(32);
        snapshot.scrollback.drain(..remove_count);
        snapshot
            .formatted_scrollback
            .drain(..remove_count.min(snapshot.formatted_scrollback.len()));
    }
    if serialized_len(snapshot) > MAX_SNAPSHOT_BYTES {
        // A vt100 formatted state is an atomic restore payload. Truncating its base64
        // representation can still decode successfully while yielding an incomplete
        // terminal control stream. Omit it instead so both clients deliberately fall
        // back to the independently bounded visible grid.
        snapshot.terminal_state_base64.clear();
    }
    while serialized_len(snapshot) > MAX_SNAPSHOT_BYTES && !snapshot.visible_grid.is_empty() {
        let excess = serialized_len(snapshot).saturating_sub(MAX_SNAPSHOT_BYTES);
        let mut target = snapshot
            .visible_grid
            .len()
            .saturating_sub(excess.saturating_add(16));
        while target > 0 && !snapshot.visible_grid.is_char_boundary(target) {
            target -= 1;
        }
        snapshot.visible_grid.truncate(target);
    }
    assert!(
        serialized_len(snapshot) <= MAX_SNAPSHOT_BYTES,
        "terminal snapshot exceeded the serialized payload limit"
    );
}

fn serialized_len(snapshot: &TerminalSnapshot) -> usize {
    serde_json::to_vec(snapshot)
        .expect("terminal snapshot DTO must serialize")
        .len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_state(terminal_state_base64: String) -> TerminalSnapshot {
        TerminalSnapshot {
            snapshot_id: "snapshot".to_string(),
            session_id: "session".to_string(),
            runtime_generation: 1,
            sequence_barrier: 0,
            geometry: TerminalGeometry { cols: 80, rows: 24 },
            terminal_state_base64,
            visible_grid: "visible fallback".to_string(),
            scrollback: Vec::new(),
            formatted_scrollback: Vec::new(),
        }
    }

    #[test]
    fn oversized_formatted_state_is_omitted_instead_of_truncated() {
        let mut snapshot = snapshot_with_state("A".repeat(MAX_SNAPSHOT_BYTES));

        enforce_serialized_limit(&mut snapshot);

        assert!(snapshot.terminal_state_base64.is_empty());
        assert_eq!(snapshot.visible_grid, "visible fallback");
        assert!(serialized_len(&snapshot) <= MAX_SNAPSHOT_BYTES);
    }

    #[test]
    fn formatted_state_is_retained_when_snapshot_fits() {
        let state = base64::engine::general_purpose::STANDARD.encode(b"formatted terminal state");
        let mut snapshot = snapshot_with_state(state.clone());

        enforce_serialized_limit(&mut snapshot);

        assert_eq!(snapshot.terminal_state_base64, state);
    }

    #[test]
    fn formatted_state_excludes_scrollback_that_clients_must_restore_separately() {
        let mut parser = vt100::Parser::new(4, 80, SNAPSHOT_SCROLLBACK_LINES);
        let output = (1..=12)
            .map(|line| format!("history row {line:02}\r\n"))
            .collect::<String>();
        parser.process(output.as_bytes());

        let snapshot = build_snapshot(
            "session",
            1,
            12,
            TerminalGeometry { cols: 80, rows: 4 },
            parser.screen(),
            1,
        );
        let formatted = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(&snapshot.terminal_state_base64)
                .expect("formatted terminal state"),
        )
        .expect("utf-8 terminal state");

        assert!(snapshot
            .scrollback
            .iter()
            .any(|row| row.contains("history row 01")));
        assert_eq!(
            snapshot.formatted_scrollback.len(),
            snapshot.scrollback.len()
        );
        assert!(!formatted.contains("history row 01"));
        assert!(formatted.contains("history row 12"));
    }

    #[test]
    fn formatted_scrollback_retains_row_colors() {
        let mut parser = vt100::Parser::new(2, 80, SNAPSHOT_SCROLLBACK_LINES);
        parser.process(b"\x1b[31mred history\x1b[m\r\nplain history\r\ncurrent\r\n");

        let snapshot = build_snapshot(
            "session",
            1,
            3,
            TerminalGeometry { cols: 80, rows: 2 },
            parser.screen(),
            1,
        );

        assert!(snapshot
            .scrollback
            .iter()
            .any(|row| row.contains("red history")));
        assert!(snapshot
            .formatted_scrollback
            .iter()
            .any(|row| row.contains("\x1b[31mred history")));
    }
}
