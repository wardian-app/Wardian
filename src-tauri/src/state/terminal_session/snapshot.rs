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

    const TEST_HYPERLINK_URI_BYTES: usize = 8 * 1_024;

    fn process_fragmented(parser: &mut vt100::Parser, bytes: &[u8]) {
        for byte in bytes {
            parser.process(std::slice::from_ref(byte));
        }
    }

    fn osc8_start(uri: &str, terminator: &[u8]) -> Vec<u8> {
        let mut bytes = b"\x1b]8;;".to_vec();
        bytes.extend_from_slice(uri.as_bytes());
        bytes.extend_from_slice(terminator);
        bytes
    }

    fn osc8_close() -> &'static [u8] {
        b"\x1b]8;;\x1b\\"
    }

    fn decoded_state(snapshot: &TerminalSnapshot) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(&snapshot.terminal_state_base64)
            .expect("formatted terminal state")
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn full_length_uri(index: usize) -> String {
        let prefix = format!("https://example.test/{index}/");
        format!(
            "{prefix}{}",
            "x".repeat(TEST_HYPERLINK_URI_BYTES - prefix.len())
        )
    }

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

    #[test]
    fn snapshot_preserves_osc8_cells_and_unlinked_adjacent_text() {
        let uri = "https://example.test/pr/1324";
        let mut output = osc8_start(uri, b"\x1b\\");
        output.extend_from_slice(b"PR #1324");
        output.extend_from_slice(osc8_close());
        output.extend_from_slice(b" tail");

        let mut parser = vt100::Parser::new(2, 80, SNAPSHOT_SCROLLBACK_LINES);
        process_fragmented(&mut parser, &output);

        for col in 0..8 {
            assert_eq!(
                parser
                    .screen()
                    .cell(0, col)
                    .and_then(|cell| cell.hyperlink()),
                Some(uri)
            );
        }
        assert_eq!(
            parser.screen().cell(0, 8).and_then(|cell| cell.hyperlink()),
            None
        );

        let snapshot = build_snapshot(
            "session",
            1,
            1,
            TerminalGeometry { cols: 80, rows: 2 },
            parser.screen(),
            1,
        );
        let formatted = decoded_state(&snapshot);
        assert!(snapshot.visible_grid.starts_with("PR #1324 tail"));
        assert!(contains_bytes(&formatted, uri.as_bytes()));
        assert!(contains_bytes(&formatted, osc8_close()));
        assert!(contains_bytes(&formatted, b"PR #1324"));
    }

    #[test]
    fn osc8_accepts_bel_and_preserves_uri_semicolons() {
        let uri = "https://example.test/pr;1324?x=1;2";
        let mut output = osc8_start(uri, b"\x07");
        output.extend_from_slice(b"label");
        output.extend_from_slice(osc8_close());

        let mut parser = vt100::Parser::new(1, 80, 0);
        parser.process(&output);

        assert_eq!(
            parser.screen().cell(0, 0).and_then(|cell| cell.hyperlink()),
            Some(uri)
        );
        let formatted = parser.screen().contents_formatted();
        let expected_start = osc8_start(uri, b"\x1b\\");
        assert!(contains_bytes(&formatted, &expected_start));
    }

    #[test]
    fn active_osc8_state_is_restored_for_followup_output() {
        let uri = "https://example.test/pr/1324";
        let mut parser = vt100::Parser::new(2, 80, 0);
        let mut output = osc8_start(uri, b"\x1b\\");
        output.extend_from_slice(b"PR #1324");
        parser.process(&output);

        let snapshot = build_snapshot(
            "session",
            1,
            1,
            TerminalGeometry { cols: 80, rows: 2 },
            parser.screen(),
            1,
        );
        let state = decoded_state(&snapshot);
        assert!(contains_bytes(&state, &osc8_start(uri, b"\x1b\\")));

        let mut restored = vt100::Parser::new(2, 80, 0);
        restored.process(&state);
        restored.process(b" next");

        for col in 8..13 {
            assert_eq!(
                restored
                    .screen()
                    .cell(0, col)
                    .and_then(|cell| cell.hyperlink()),
                Some(uri)
            );
        }
    }

    #[test]
    fn formatted_scrollback_preserves_osc8_targets() {
        let uri = "https://example.test/history";
        let mut output = osc8_start(uri, b"\x1b\\");
        output.extend_from_slice(b"history");
        output.extend_from_slice(osc8_close());
        output.extend_from_slice(b"\r\ncurrent\r\n");

        let mut parser = vt100::Parser::new(2, 40, SNAPSHOT_SCROLLBACK_LINES);
        parser.process(&output);

        let snapshot = build_snapshot(
            "session",
            1,
            1,
            TerminalGeometry { cols: 40, rows: 2 },
            parser.screen(),
            1,
        );
        assert!(snapshot
            .formatted_scrollback
            .iter()
            .any(|row| row.contains("history") && row.contains(uri)));
    }

    #[test]
    fn osc8_rejects_unsafe_and_over_limit_targets() {
        let over_limit = format!(
            "https://example.test/{}",
            "x".repeat(TEST_HYPERLINK_URI_BYTES)
        );
        let mut parser = vt100::Parser::new(1, 80, 0);
        let mut output = osc8_start(&over_limit, b"\x1b\\");
        output.extend_from_slice(b"X");
        parser.process(&output);
        assert_eq!(
            parser.screen().cell(0, 0).and_then(|cell| cell.hyperlink()),
            None
        );

        let mut unsafe_uri = b"\x1b]8;;https://example.test/unsafe".to_vec();
        unsafe_uri.push(0x1b);
        unsafe_uri.extend_from_slice(b"[31m");
        unsafe_uri.extend_from_slice(osc8_close());
        unsafe_uri.extend_from_slice(b"Y");
        let mut unsafe_parser = vt100::Parser::new(1, 80, 0);
        unsafe_parser.process(&unsafe_uri);
        assert_eq!(
            unsafe_parser
                .screen()
                .cell(0, 0)
                .and_then(|cell| cell.hyperlink()),
            None
        );
    }

    #[test]
    fn hyperlink_budget_reclaims_erased_targets() {
        let mut parser = vt100::Parser::new(1, 300, 0);
        let mut output = Vec::new();
        for index in 0..256 {
            output.extend_from_slice(&osc8_start(&full_length_uri(index), b"\x1b\\"));
            output.push(b'x');
            output.extend_from_slice(osc8_close());
        }
        parser.process(&output);

        let exhausted_uri = full_length_uri(256);
        let mut exhausted = osc8_start(&exhausted_uri, b"\x1b\\");
        exhausted.push(b'x');
        parser.process(&exhausted);
        assert_eq!(
            parser
                .screen()
                .cell(0, 256)
                .and_then(|cell| cell.hyperlink()),
            None
        );

        parser.process(b"\x1b[H\x1b[256X");
        let reclaimed_uri = full_length_uri(257);
        let mut reclaimed = osc8_start(&reclaimed_uri, b"\x1b\\");
        reclaimed.push(b'R');
        parser.process(&reclaimed);
        assert_eq!(
            parser.screen().cell(0, 0).and_then(|cell| cell.hyperlink()),
            Some(reclaimed_uri.as_str())
        );
    }

    #[test]
    fn one_hyperlink_target_is_shared_across_a_long_link_run() {
        let uri = full_length_uri(0);
        let mut parser = vt100::Parser::new(1, 300, 0);
        let mut output = osc8_start(&uri, b"\x1b\\");
        output.extend(std::iter::repeat_n(b'x', 300));
        output.extend_from_slice(osc8_close());
        parser.process(&output);

        for col in 0..300 {
            assert_eq!(
                parser
                    .screen()
                    .cell(0, col)
                    .and_then(|cell| cell.hyperlink()),
                Some(uri.as_str())
            );
        }
    }

    #[test]
    fn long_osc8_target_is_coalesced_across_multirow_snapshot() {
        let uri = full_length_uri(1_000);
        let mut parser = vt100::Parser::new(10, 40, 0);
        let mut output = osc8_start(&uri, b"\x1b\\");
        output.extend(std::iter::repeat_n(b'x', 400));
        output.extend_from_slice(osc8_close());
        output.push(b'\r');
        parser.process(&output);

        for row in 0..10 {
            assert_eq!(
                parser
                    .screen()
                    .cell(row, 0)
                    .and_then(|cell| cell.hyperlink()),
                Some(uri.as_str())
            );
            assert_eq!(
                parser
                    .screen()
                    .cell(row, 39)
                    .and_then(|cell| cell.hyperlink()),
                Some(uri.as_str())
            );
        }

        let snapshot = build_snapshot(
            "session",
            1,
            1,
            TerminalGeometry { cols: 40, rows: 10 },
            parser.screen(),
            1,
        );
        let formatted = decoded_state(&snapshot);
        let start = osc8_start(&uri, b"\x1b\\");
        let open_count = formatted
            .windows(start.len())
            .filter(|window| *window == start.as_slice())
            .count();

        assert!(open_count <= 10);
        assert!(formatted.len() < 128 * 1_024);
        assert!(contains_bytes(&formatted, uri.as_bytes()));
        assert!(!snapshot.terminal_state_base64.is_empty());
    }

    #[test]
    fn hyperlink_metadata_adds_eight_bytes_to_each_cell() {
        assert_eq!(std::mem::size_of::<vt100::Cell>(), 40);
    }
}
